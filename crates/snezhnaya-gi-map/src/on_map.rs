use bevy_ecs::prelude::*;

use snezhnaya_gi_data::excel::production::ProductionConfig;
use snezhnaya_gi_data::scene::scene_point_config::{
    get_scene_point_config_collection, resolve_point,
};
use snezhnaya_gi_event::command::*;
use snezhnaya_gi_event::scene::*;
use snezhnaya_gi_message::event::ClientMessageEvent;
use snezhnaya_gi_message::output::MessageOutput;
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::{
    normal::*,
    retcode::Retcode,
    server_only::{MapMarkPointBin, PlayerDataBin},
};

use tracing::instrument;

#[instrument(skip_all)]
pub fn message_on_map(
    mut events: MessageReader<ClientMessageEvent>,
    mut debug_events: MessageWriter<DebugCommandEvent>,
    mut jump_events: MessageWriter<ScenePlayerJumpEvent>,
    mut jump_point_events: MessageWriter<ScenePlayerJumpByPointEvent>,
    mut enter_dungeon_events: MessageWriter<ScenePlayerEnterDungeonEvent>,
    message_output: Res<MessageOutput>,
    mut players: ResMut<Players>,
) {
    for message in events.read() {
        match message.message_name() {
            "MarkMapReq" => {
                let Some(request) = message.decode::<MarkMapReq>() else {
                    continue;
                };
                let Some(player) = players.get_mut(message.sender_uid()) else {
                    continue;
                };
                let result = update_mark(player, &request);
                if result.is_ok() && request.op == mark_map_req::Operation::Add as i32 {
                    if let Some(mark) = &request.mark {
                        let first = mark.name.split_whitespace().next().unwrap_or("");
                        let pos = mark.pos.unwrap_or_default();
                        let kind =
                            match mark.point_type() {
                                MapMarkPointType::Npc => first.parse::<u32>().ok().map(|id| {
                                    CommandKind::QuickSpawnMonster {
                                        monster_id: Some(id),
                                        position: (pos.x, pos.z),
                                    }
                                }),
                                MapMarkPointType::Quest => first.parse::<u32>().ok().map(|id| {
                                    CommandKind::QuickSpawnGadget {
                                        gadget_id: Some(id),
                                        position: (pos.x, pos.z),
                                    }
                                }),
                                MapMarkPointType::Special => first
                                    .parse::<f32>()
                                    .ok()
                                    .filter(|y| y.is_finite())
                                    .map(|y| CommandKind::QuickTravel {
                                        scene_id: Some(mark.scene_id),
                                        position: (pos.x, Some(y), pos.z),
                                    }),
                                _ => None,
                            };
                        if let Some(kind) = kind {
                            debug_events.write(DebugCommandEvent {
                                executor_uid: player.uid,
                                kind,
                            });
                        }
                    }
                }
                message_output.send(
                    player.uid,
                    "MarkMapRsp",
                    MarkMapRsp {
                        retcode: result.err().unwrap_or(Retcode::RetSucc).into(),
                        mark_list: marks(player),
                    },
                );
            }
            "SceneTransToPointReq" => {
                let Some(request) = message.decode::<SceneTransToPointReq>() else {
                    continue;
                };
                let Some(player) = players.get(message.sender_uid()) else {
                    continue;
                };
                let result = check_point(player, request.scene_id, request.point_id);
                if result.is_ok() {
                    jump_point_events.write(ScenePlayerJumpByPointEvent(
                        player.uid,
                        request.scene_id,
                        request.point_id,
                    ));
                }
                message_output.send(
                    player.uid,
                    "SceneTransToPointRsp",
                    SceneTransToPointRsp {
                        retcode: result.err().unwrap_or(Retcode::RetSucc).into(),
                        scene_id: request.scene_id,
                        point_id: request.point_id,
                    },
                );
            }
            "PersonalSceneJumpReq" => {
                let Some(request) = message.decode::<PersonalSceneJumpReq>() else {
                    continue;
                };
                let Some(player) = players.get(message.sender_uid()) else {
                    continue;
                };
                let scene = player.scene_bin.as_ref().map_or(0, |s| s.my_cur_scene_id);
                let result = check_point(player, scene, request.point_id);
                let mut response = PersonalSceneJumpRsp::default();
                match result {
                    Ok((dest, point)) => {
                        response.dest_scene_id = dest;
                        response.dest_pos = Some(Vector {
                            x: point.tran_pos.x,
                            y: point.tran_pos.y,
                            z: point.tran_pos.z,
                        });
                        jump_point_events.write(ScenePlayerJumpByPointEvent(
                            player.uid,
                            scene,
                            request.point_id,
                        ));
                    }
                    Err(code) => response.retcode = code.into(),
                }
                message_output.send(player.uid, "PersonalSceneJumpRsp", response);
            }
            "PlayerEnterDungeonReq" => {
                if let Some(request) = message.decode::<PlayerEnterDungeonReq>() {
                    let uid = message.sender_uid();
                    let result = (|| -> Result<(), Retcode> {
                        let config =
                            snezhnaya_gi_data::excel::dungeon_excel_config_collection::get()
                                .get(&request.dungeon_id)
                                .ok_or(Retcode::RetEnterSceneFail)?;
                        if !snezhnaya_gi_data::scene::script_cache::get_scene_config_collection()
                            .contains_key(&config.scene_id)
                        {
                            return Err(Retcode::RetEnterSceneFail);
                        }
                        if players
                            .get(uid)
                            .and_then(|p| p.basic_bin.as_ref())
                            .map_or(0, |b| b.level)
                            < config.limit_level
                        {
                            return Err(Retcode::RetPlayerLevelLessThan);
                        }
                        Ok(())
                    })();
                    if result.is_ok() {
                        enter_dungeon_events
                            .write(ScenePlayerEnterDungeonEvent(uid, request.dungeon_id));
                    }
                    message_output.send(
                        uid,
                        "PlayerEnterDungeonRsp",
                        PlayerEnterDungeonRsp {
                            retcode: result.err().unwrap_or(Retcode::RetSucc).into(),
                            dungeon_id: request.dungeon_id,
                            point_id: request.point_id,
                        },
                    );
                }
            }
            "PlayerQuitDungeonReq" => {
                let Some(request) = message.decode::<PlayerQuitDungeonReq>() else {
                    continue;
                };
                let Some(player) = players.get(message.sender_uid()) else {
                    continue;
                };
                if player
                    .dungeon_bin
                    .as_ref()
                    .is_some_and(|d| d.cur_dungeon_id != 0 && d.is_ley_line_challenge)
                {
                    continue;
                }
                let result = (|| -> Result<(), Retcode> {
                    let dungeon = player
                        .dungeon_bin
                        .as_ref()
                        .filter(|d| d.cur_dungeon_id != 0)
                        .ok_or(Retcode::RetEnterSceneFail)?;
                    let pos = dungeon
                        .quit_pos
                        .filter(|p| p.is_valid())
                        .ok_or(Retcode::RetEnterSceneFail)?;
                    if dungeon.quit_scene_id == 0 {
                        return Err(Retcode::RetInvalidSceneId);
                    }
                    if request.is_quit_immediately {
                        jump_events.write(ScenePlayerJumpEvent(
                            player.uid,
                            dungeon.quit_scene_id,
                            EnterReason::DungeonQuit,
                            (pos.x, pos.y, pos.z),
                        ));
                    }
                    Ok(())
                })();
                message_output.send(
                    player.uid,
                    "PlayerQuitDungeonRsp",
                    PlayerQuitDungeonRsp {
                        retcode: result.err().unwrap_or(Retcode::RetSucc).into(),
                        ..Default::default()
                    },
                );
            }
            &_ => {}
        }
    }
}

fn marks(player: &PlayerDataBin) -> Vec<MapMarkPoint> {
    player
        .scene_bin
        .iter()
        .flat_map(|s| s.mark_point_list.iter())
        .map(|m| MapMarkPoint {
            scene_id: m.scene_id,
            name: m.name.clone(),
            pos: m.pos.map(Into::into),
            point_type: m.point_type as i32,
            monster_id: m.monster_id,
            from_type: m.from_type as i32,
            quest_id: m.quest_id,
            map_layer_id: m.map_layer_id,
        })
        .collect()
}
pub fn login_marks(players: Res<Players>, out: Res<MessageOutput>) {
    for uid in players.keys() {
        if let Some(player) = players.get(*uid) {
            out.send_json(*uid, "AllMarkPointNotify", serde_json::json!({"mark_list": marks(player)}));
        }
    }
}
fn update_mark(player: &mut PlayerDataBin, request: &MarkMapReq) -> Result<(), Retcode> {
    use mark_map_req::Operation;
    let operation = Operation::try_from(request.op).map_err(|_| Retcode::RetMarkUnknownType)?;
    if operation == Operation::Get {
        return Ok(());
    }
    let selected = request.mark.as_ref().ok_or(Retcode::RetMarkNotExists)?;
    let valid = |m: &MapMarkPoint| -> Result<(), Retcode> {
        if !get_scene_point_config_collection().contains_key(&m.scene_id)
            && !snezhnaya_gi_data::scene::script_cache::get_scene_config_collection()
                .contains_key(&m.scene_id)
        {
            return Err(Retcode::RetMarkInvalidSceneId);
        }
        if MapMarkPointType::try_from(m.point_type).is_err()
            || MapMarkFromType::try_from(m.from_type).is_err()
        {
            return Err(Retcode::RetMarkUnknownType);
        }
        if !m
            .pos
            .is_some_and(|p| p.x.is_finite() && p.y.is_finite() && p.z.is_finite())
        {
            return Err(Retcode::RetFloatError);
        }
        if m.name.encode_utf16().count() > 100 {
            return Err(Retcode::RetMarkNameTooLong);
        }
        if m.name
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>'))
        {
            return Err(Retcode::RetMarkNameIllegal);
        }
        Ok(())
    };
    valid(selected)?;
    let same = |a: &MapMarkPointBin, b: &MapMarkPoint| {
        a.scene_id == b.scene_id
            && a.map_layer_id == b.map_layer_id
            && a.pos
                .zip(b.pos)
                .is_some_and(|(x, y)| (x.x - y.x).abs() < 0.01 && (x.z - y.z).abs() < 0.01)
    };
    let current = player
        .scene_bin
        .as_ref()
        .ok_or(Retcode::RetMarkInvalidSceneId)?;
    let mut list = current.mark_point_list.clone();
    let limit = ProductionConfig::get()
        .constants
        .get("CONST_VALUE_MAP_MARK_MAX_LENGTH")
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(100);
    let index = if operation == Operation::Add {
        if list.iter().any(|m| same(m, selected)) {
            return Err(Retcode::RetMarkAlreadyExists);
        }
        if list.len() >= limit {
            return Err(Retcode::RetMarkOverflow);
        }
        None
    } else {
        let old = if operation == Operation::Mod {
            request.old.as_ref().ok_or(Retcode::RetMarkNotExists)?
        } else {
            selected
        };
        valid(old)?;
        Some(
            list.iter()
                .position(|m| same(m, old))
                .ok_or(Retcode::RetMarkNotExists)?,
        )
    };
    let convert = || MapMarkPointBin {
        scene_id: selected.scene_id,
        name: selected.name.clone(),
        pos: selected.pos.map(Into::into),
        point_type: selected.point_type as u32,
        monster_id: selected.monster_id,
        from_type: selected.from_type as u32,
        quest_id: selected.quest_id,
        map_layer_id: selected.map_layer_id,
    };
    match operation {
        Operation::Add => list.push(convert()),
        Operation::Mod => {
            let index = index.unwrap();
            if list
                .iter()
                .enumerate()
                .any(|(i, m)| i != index && same(m, selected))
            {
                return Err(Retcode::RetMarkAlreadyExists);
            }
            list[index] = convert();
        }
        Operation::Del => {
            list.remove(index.unwrap());
        }
        Operation::Get => (),
    }
    player.scene_bin.as_mut().unwrap().mark_point_list = list;
    Ok(())
}
fn check_point(
    player: &PlayerDataBin,
    scene: u32,
    point: u32,
) -> Result<
    (
        u32,
        snezhnaya_gi_data::scene::scene_point_config::ScenePointData,
    ),
    Retcode,
> {
    let destination = resolve_point(scene, point).ok_or(Retcode::RetPointNotUnlocked)?;
    if player
        .scene_bin
        .as_ref()
        .and_then(|s| s.world.as_ref())
        .and_then(|w| w.scene_map.get(&scene))
        .is_some_and(|s| !s.unlocked_point_list.contains(&point))
    {
        return Err(Retcode::RetPointNotUnlocked);
    }
    Ok(destination)
}
