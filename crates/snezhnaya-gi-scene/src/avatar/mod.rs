use bevy_ecs::prelude::*;
use snezhnaya_gi_data::prop_type::FightPropType;
use snezhnaya_gi_entity::avatar::ReplaceCurrentPlayerAvatarMarker;
use snezhnaya_gi_entity::avatar::{AvatarQueryReadOnly, CurrentPlayerAvatarMarker};
use snezhnaya_gi_entity::common::{FightProperties, LifeState, ToBeRemovedMarker, Visible};
use snezhnaya_gi_entity::transform::Transform;
use snezhnaya_gi_event::entity::{EntityDisappearEvent, EntityPropertySeparateUpdateEvent};
use snezhnaya_gi_event::scene::*;
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::normal::{
    AddBackupAvatarTeamRsp, AvatarDieAnimationEndReq, AvatarDieAnimationEndRsp, AvatarTeam,
    AvatarTeamAllDataNotify, AvatarTeamUpdateNotify, ChangeAvatarReq, ChangeAvatarRsp,
    ChangeTeamNameReq, ChangeTeamNameRsp, ChooseCurAvatarTeamReq, ChooseCurAvatarTeamRsp,
    DelBackupAvatarTeamReq, DelBackupAvatarTeamRsp, SetUpAvatarTeamReq, SetUpAvatarTeamRsp,
    VisionType,
};
use snezhnaya_gi_proto::retcode::Retcode;
use snezhnaya_gi_proto::server_only::{avatar_bin, PlayerAvatarCompBin, PlayerDataBin};
use std::collections::{HashMap, HashSet};
use tracing::{debug, instrument};

pub fn change_avatar(
    mut client_messages: MessageReader<ClientMessageEvent>,
    mut commands: Commands,
    mut players: ResMut<Players>,
    avatars: Query<
        (
            Entity,
            &FightProperties,
            &LifeState,
            AvatarQueryReadOnly,
            Option<&CurrentPlayerAvatarMarker>,
        ),
        Without<ToBeRemovedMarker>,
    >,
    message_output: Res<MessageOutput>,
    mut update_separate_property_entity_events: MessageWriter<EntityPropertySeparateUpdateEvent>,
    mut disappear_events: MessageWriter<EntityDisappearEvent>,
) {
    let mut positions = HashMap::new();
    for message in client_messages.read() {
        match message.message_name() {
            "ChangeAvatarReq" => {
                let Some(request) = message.decode::<ChangeAvatarReq>() else {
                    continue;
                };
                let uid = message.sender_uid();
                let Some(player) = players.get_mut(uid) else {
                    continue;
                };
                let current = player.avatar_bin.as_ref().map_or(0, |a| a.cur_avatar_guid);
                let result = (|| -> Result<_, Retcode> {
                    if common::player_cache::cache_get_is_tp(uid).unwrap_or(false) {
                        return Err(Retcode::RetInTransfer);
                    }
                    let stored = player
                        .avatar_bin
                        .as_ref()
                        .ok_or(Retcode::RetCanNotFindAvatar)?;
                    if !stored.avatar_map.contains_key(&request.guid) {
                        return Err(Retcode::RetCanNotFindAvatar);
                    }
                    if !stored.cur_avatar_guid_list.contains(&request.guid) {
                        return Err(Retcode::RetAvatarNotExistInTeam);
                    }
                    let (old_entity, _, _, old, current_marker) = avatars
                        .iter()
                        .find(|(_, _, _, data, _)| {
                            data.owner_player_uid.0 == uid && data.guid.0 == current
                        })
                        .ok_or(Retcode::RetAvatarNotOnScene)?;
                    if current_marker.is_none() && !positions.contains_key(&uid) {
                        return Err(Retcode::RetAvatarNotOnScene);
                    }
                    let (new_entity, _, life, _, _) = avatars
                        .iter()
                        .find(|(_, _, _, data, _)| {
                            data.owner_player_uid.0 == uid && data.guid.0 == request.guid
                        })
                        .ok_or(Retcode::RetAvatarNotOnScene)?;
                    if *life != LifeState::Alive {
                        return Err(Retcode::RetAvatarNotAlive);
                    }
                    let position = positions
                        .entry(uid)
                        .or_insert_with(|| old.transform.clone())
                        .clone();
                    Ok((old_entity, new_entity, old.entity_id.0, position))
                })();
                let retcode = match result {
                    Ok((old_entity, new_entity, old_id, position)) => {
                        if current != request.guid {
                            // 同一帧连续切人也按存档里的当前角色衔接，避免留下多个出场标记。
                            commands
                                .entity(old_entity)
                                .remove::<CurrentPlayerAvatarMarker>()
                                .remove::<Visible>();
                            disappear_events.write(EntityDisappearEvent(
                                old_id,
                                VisionType::VisionReplace.into(),
                            ));
                            commands.entity(new_entity).insert((
                                CurrentPlayerAvatarMarker,
                                Visible,
                                ReplaceCurrentPlayerAvatarMarker(old_id),
                                position.clone(),
                            ));
                            let stored = player.avatar_bin.as_mut().unwrap();
                            stored.cur_avatar_guid = request.guid;
                            if let Some(team) = stored.team_map.get_mut(&stored.cur_team_id) {
                                team.last_cur_avatar_guid = request.guid;
                            }
                            stored.last_change_avatar_time = common::time_util::unix_timestamp_ms();
                            if let Some(scene) = player.scene_bin.as_mut() {
                                scene.my_cur_scene_pos = Some(position.position);
                                scene.my_cur_scene_rot = Some(position.rotation);
                            }
                        }
                        Retcode::RetSucc
                    }
                    Err(code) => code,
                };
                message_output.send(
                    uid,
                    "ChangeAvatarRsp",
                    ChangeAvatarRsp {
                        cur_guid: player.avatar_bin.as_ref().map_or(0, |a| a.cur_avatar_guid),
                        skill_id: request.skill_id,
                        retcode: retcode.into(),
                    },
                );
            }
            "AvatarDieAnimationEndReq" => {
                if let Some(request) = message.decode::<AvatarDieAnimationEndReq>() {
                    let Some(player_info) = players.get_mut(message.sender_uid()) else {
                        continue;
                    };
                    let avatar_guid_list =
                        if let Some(ref player_avatar_bin) = player_info.avatar_bin {
                            player_avatar_bin.cur_avatar_guid_list.clone()
                        } else {
                            continue;
                        };

                    let mut all_dead = true;
                    let mut first_alive_avatar_guid = None;
                    let mut first_alive_avatar_entity = None;
                    let mut first_alive_avatar_transform = None;

                    for (avatar_entity, _, life_state, avatar_data, _) in
                        avatars.iter().filter(|(_, _, _, a, _)| {
                            a.owner_player_uid.0 == message.sender_uid()
                                && avatar_guid_list.contains(&a.guid.0)
                        })
                    {
                        if *life_state == LifeState::Alive && first_alive_avatar_guid.is_none() {
                            all_dead = false;
                            first_alive_avatar_guid = Some(avatar_data.guid.0);
                            first_alive_avatar_entity = Some(avatar_entity);
                            first_alive_avatar_transform = Some(avatar_data.transform.clone());
                        }
                    }

                    if let (Some(guid), Some(entity), Some(transform)) = (
                        first_alive_avatar_guid,
                        first_alive_avatar_entity,
                        first_alive_avatar_transform,
                    ) {
                        if let Some(ref mut player_avatar_bin) = player_info.avatar_bin {
                            player_avatar_bin.cur_avatar_guid = guid;
                        }

                        let transform = match request.reborn_pos {
                            Some(move_pos) => Transform {
                                position: move_pos.into(),
                                rotation: transform.rotation,
                            },
                            _ => transform,
                        };

                        debug!("transform:{}", transform);

                        commands
                            .entity(entity)
                            .insert(CurrentPlayerAvatarMarker)
                            .insert(Visible)
                            .insert(ReplaceCurrentPlayerAvatarMarker(0))
                            .insert(transform.clone());
                    }

                    debug!("all_dead:{}", all_dead);
                    if all_dead {
                        let Some(ref player_avatar_bin) = player_info.avatar_bin else {
                            continue;
                        };
                        for (avatar_entity, fight_props, _, avatar_data, _) in
                            avatars.iter().filter(|(_, _, _, a, _)| {
                                a.owner_player_uid.0 == message.sender_uid()
                                    && player_avatar_bin.cur_avatar_guid_list.contains(&a.guid.0)
                            })
                        {
                            let max_hp = fight_props.get_property(FightPropType::FIGHT_PROP_MAX_HP);
                            update_separate_property_entity_events.write(
                                EntityPropertySeparateUpdateEvent(
                                    avatar_entity,
                                    FightPropType::FIGHT_PROP_CUR_HP,
                                    max_hp,
                                ),
                            );
                            if player_avatar_bin.cur_avatar_guid == avatar_data.guid.0 {
                                let transform = match request.reborn_pos {
                                    Some(move_pos) => Transform {
                                        position: move_pos.into(),
                                        rotation: avatar_data.transform.rotation,
                                    },
                                    _ => avatar_data.transform.clone(),
                                };

                                debug!("transform:{}", transform);

                                commands
                                    .entity(avatar_entity)
                                    .insert(CurrentPlayerAvatarMarker)
                                    .insert(Visible)
                                    .insert(ReplaceCurrentPlayerAvatarMarker(0))
                                    .insert(transform.clone());
                            }
                        }
                    };

                    message_output.send(
                        message.sender_uid(),
                        "AvatarDieAnimationEndRsp",
                        AvatarDieAnimationEndRsp {
                            die_guid: request.die_guid,
                            retcode: 0,
                            skill_id: request.skill_id,
                        },
                    );
                }
            }
            &_ => {}
        }
    }
}

pub fn init_teams(mut players: ResMut<Players>) {
    for player in players.values_mut() {
        let Some(avatars) = player.avatar_bin.as_mut() else {
            continue;
        };
        for id in 1..=4 {
            avatars.team_map.entry(id).or_default();
        }
        let mut seen = HashSet::new();
        avatars
            .backup_team_id_order_list
            .retain(|id| *id > 4 && avatars.team_map.contains_key(id) && seen.insert(*id));
        let mut remaining: Vec<_> = avatars
            .team_map
            .keys()
            .filter(|id| **id > 4 && !seen.contains(id))
            .copied()
            .collect();
        remaining.sort_unstable();
        avatars.backup_team_id_order_list.extend(remaining);
    }
}

fn can_change_team(player: &PlayerDataBin, multiplayer: bool) -> Result<(), Retcode> {
    if multiplayer {
        return Err(Retcode::RetMpInMpMode);
    }
    if common::player_cache::cache_get_is_tp(player.uid).unwrap_or(false) {
        return Err(Retcode::RetInTransfer);
    }
    if player
        .dungeon_bin
        .as_ref()
        .is_some_and(|d| d.cur_dungeon_id != 0)
    {
        return Err(Retcode::RetCurDungeonNotAllowSwitchTeam);
    }
    if player
        .avatar_bin
        .as_ref()
        .is_some_and(|a| !a.temp_avatar_guid_list.is_empty() || a.cur_team_id == 0)
    {
        return Err(Retcode::RetIsUsingTempAvatar);
    }
    Ok(())
}
fn validate_team(avatars: &PlayerAvatarCompBin, guids: &[u64]) -> Result<(), Retcode> {
    if guids.len() > 4 {
        return Err(Retcode::RetTeamCostExceedLimit);
    }
    let mut seen = HashSet::new();
    let mut ids = HashSet::new();
    for guid in guids {
        if !seen.insert(*guid) {
            return Err(Retcode::RetDuplicateAvatar);
        }
        let avatar = avatars
            .avatar_map
            .get(guid)
            .ok_or(Retcode::RetCanNotFindAvatar)?;
        if !matches!(avatar.detail, Some(avatar_bin::Detail::FormalAvatar(_))) {
            return Err(Retcode::RetIsUsingTrialAvatar);
        }
        if !ids.insert(avatar.avatar_id) {
            return Err(Retcode::RetDuplicateAvatar);
        }
    }
    Ok(())
}
fn select_alive(
    avatars: &PlayerAvatarCompBin,
    guids: &[u64],
    preferred: u64,
) -> Result<u64, Retcode> {
    let alive = |guid: &u64| {
        avatars
            .avatar_map
            .get(guid)
            .is_some_and(|a| a.life_state == LifeState::Alive as u32)
    };
    if guids.contains(&preferred) && alive(&preferred) {
        return Ok(preferred);
    }
    guids
        .iter()
        .find(|guid| alive(guid))
        .copied()
        .ok_or(Retcode::RetAvatarNotAlive)
}
fn team_notify(player: &PlayerDataBin, out: &MessageOutput) {
    let Some(avatars) = player.avatar_bin.as_ref() else {
        return;
    };
    out.send(
        player.uid,
        "AvatarTeamUpdateNotify",
        AvatarTeamUpdateNotify {
            avatar_team_map: avatars
                .team_map
                .iter()
                .map(|(&id, team)| {
                    (
                        id,
                        AvatarTeam {
                            team_name: team.team_name.clone(),
                            avatar_guid_list: team.avatar_guid_list.clone(),
                        },
                    )
                })
                .collect(),
            ..Default::default()
        },
    );
}

fn all_teams_notify(player: &PlayerDataBin, out: &MessageOutput) {
    let Some(avatars) = player.avatar_bin.as_ref() else {
        return;
    };
    out.send(
        player.uid,
        "AvatarTeamAllDataNotify",
        AvatarTeamAllDataNotify {
            avatar_team_map: avatars
                .team_map
                .iter()
                .map(|(&id, team)| {
                    (
                        id,
                        AvatarTeam {
                            team_name: team.team_name.clone(),
                            avatar_guid_list: team.avatar_guid_list.clone(),
                        },
                    )
                })
                .collect(),
            backup_avatar_team_order_list: avatars.backup_team_id_order_list.clone(),
            cur_avatar_team_id: avatars.cur_team_id,
            temp_avatar_guid_list: avatars.temp_avatar_guid_list.clone(),
        },
    );
}

fn move_team(avatars: &mut PlayerAvatarCompBin, from: u32, to: u32) -> Result<(), Retcode> {
    let mut ids: Vec<_> = (1..=4)
        .filter(|id| avatars.team_map.contains_key(id))
        .collect();
    let mut seen: HashSet<_> = ids.iter().copied().collect();
    ids.extend(
        avatars
            .backup_team_id_order_list
            .iter()
            .copied()
            .filter(|id| *id > 4 && avatars.team_map.contains_key(id) && seen.insert(*id)),
    );
    let first = ids
        .iter()
        .position(|id| *id == from)
        .ok_or(Retcode::RetCanNotFindTeam)?;
    let last = ids
        .iter()
        .position(|id| *id == to)
        .ok_or(Retcode::RetCanNotFindTeam)?;
    let mut teams: Vec<_> = ids
        .iter()
        .map(|id| (*id, avatars.team_map[id].clone()))
        .collect();
    let moved = teams.remove(first);
    teams.insert(last, moved);
    let current = avatars.cur_team_id;
    // 只移动预设槽位，当前队伍和出场角色保持对应，避免重新进场。
    for (id, (old_id, team)) in ids.into_iter().zip(teams) {
        if old_id == current {
            avatars.cur_team_id = id;
        }
        avatars.team_map.insert(id, team);
    }
    Ok(())
}

#[instrument(skip_all)]
pub fn set_up_avatar_team(
    mut client_messages: MessageReader<ClientMessageEvent>,
    out: Res<MessageOutput>,
    mut players: ResMut<Players>,
    mut change_events: MessageWriter<PlayerAvatarTeamChanged>,
) {
    let multiplayer = players.keys().len() > 1;
    for message in client_messages.read() {
        let uid = message.sender_uid();
        let Some(player) = players.get_mut(uid) else {
            continue;
        };
        match message.message_name() {
            "AddBackupAvatarTeamReq" => {
                if message.decode_json().is_none() {
                    continue;
                }
                let result = (|| -> Result<(), Retcode> {
                    let avatars = player
                        .avatar_bin
                        .as_mut()
                        .ok_or(Retcode::RetCanNotFindTeam)?;
                    let id = (5..=20)
                        .find(|id| !avatars.team_map.contains_key(id))
                        .ok_or(Retcode::RetFullBackupTeam)?;
                    avatars.team_map.insert(id, Default::default());
                    avatars.backup_team_id_order_list.push(id);
                    Ok(())
                })();
                if result.is_ok() {
                    all_teams_notify(player, &out);
                }
                out.send(
                    uid,
                    "AddBackupAvatarTeamRsp",
                    AddBackupAvatarTeamRsp {
                        retcode: result.err().unwrap_or(Retcode::RetSucc).into(),
                    },
                );
            }
            "DelBackupAvatarTeamReq" => {
                let Some(req) = message.decode::<DelBackupAvatarTeamReq>() else {
                    continue;
                };
                let result = (|| -> Result<(), Retcode> {
                    let avatars = player
                        .avatar_bin
                        .as_mut()
                        .ok_or(Retcode::RetCanNotFindTeam)?;
                    if req.backup_avatar_team_id <= 4
                        || !avatars.team_map.contains_key(&req.backup_avatar_team_id)
                    {
                        return Err(Retcode::RetBackupTeamIdNotValid);
                    }
                    if avatars.cur_team_id == req.backup_avatar_team_id {
                        return Err(Retcode::RetBackupTeamIsCurTeam);
                    }
                    avatars.team_map.remove(&req.backup_avatar_team_id);
                    avatars
                        .backup_team_id_order_list
                        .retain(|id| *id != req.backup_avatar_team_id);
                    Ok(())
                })();
                if result.is_ok() {
                    all_teams_notify(player, &out);
                }
                out.send(
                    uid,
                    "DelBackupAvatarTeamRsp",
                    DelBackupAvatarTeamRsp {
                        backup_avatar_team_id: req.backup_avatar_team_id,
                        retcode: result.err().unwrap_or(Retcode::RetSucc).into(),
                    },
                );
            }
            "MoveTeamReq" => {
                let Some(req) = message.decode_json() else {
                    continue;
                };
                let field = |key| {
                    req.get(key)
                        .and_then(|v| v.as_u64())
                        .and_then(|v| u32::try_from(v).ok())
                        .unwrap_or(0)
                };
                let from = field("from_team_id");
                let to = field("to_team_id");
                let result = can_change_team(player, multiplayer).and_then(|_| {
                    move_team(
                        player
                            .avatar_bin
                            .as_mut()
                            .ok_or(Retcode::RetCanNotFindTeam)?,
                        from,
                        to,
                    )
                });
                if result.is_ok() {
                    all_teams_notify(player, &out);
                }
                out.send_json(
                    uid,
                    "MoveTeamRsp",
                    serde_json::json!({
                        "retcode": i32::from(result.err().unwrap_or(Retcode::RetSucc)),
                        "from_team_id": from, "to_team_id": to,
                    }),
                );
            }
            "SetUpAvatarTeamReq" => {
                let Some(request) = message.decode::<SetUpAvatarTeamReq>() else {
                    continue;
                };
                let result = (|| -> Result<_, Retcode> {
                    let avatars = player
                        .avatar_bin
                        .as_ref()
                        .ok_or(Retcode::RetCanNotFindTeam)?;
                    if !avatars.team_map.contains_key(&request.team_id) {
                        return Err(Retcode::RetCanNotFindTeam);
                    }
                    validate_team(avatars, &request.avatar_team_guid_list)?;
                    let active = avatars.cur_team_id == request.team_id;
                    let preferred = if request.cur_avatar_guid != 0 {
                        request.cur_avatar_guid
                    } else if !active
                        && avatars.team_map[&request.team_id].last_cur_avatar_guid != 0
                    {
                        avatars.team_map[&request.team_id].last_cur_avatar_guid
                    } else {
                        avatars.cur_avatar_guid
                    };
                    let current = if active {
                        can_change_team(player, multiplayer)?;
                        select_alive(avatars, &request.avatar_team_guid_list, preferred)?
                    } else {
                        request
                            .avatar_team_guid_list
                            .iter()
                            .copied()
                            .find(|guid| *guid == preferred)
                            .or_else(|| request.avatar_team_guid_list.first().copied())
                            .unwrap_or(0)
                    };
                    let avatars = player.avatar_bin.as_mut().unwrap();
                    avatars
                        .team_map
                        .get_mut(&request.team_id)
                        .unwrap()
                        .avatar_guid_list
                        .clone_from(&request.avatar_team_guid_list);
                    avatars
                        .team_map
                        .get_mut(&request.team_id)
                        .unwrap()
                        .last_cur_avatar_guid = current;
                    if active {
                        avatars.cur_avatar_guid = current;
                        avatars
                            .cur_avatar_guid_list
                            .clone_from(&request.avatar_team_guid_list);
                        change_events.write(PlayerAvatarTeamChanged {
                            uid,
                            avatar_team_guid_list: request.avatar_team_guid_list.clone(),
                            cur_avatar_guid: current,
                        });
                    } else {
                        team_notify(player, &out);
                    }
                    Ok(current)
                })();
                out.send(
                    uid,
                    "SetUpAvatarTeamRsp",
                    SetUpAvatarTeamRsp {
                        team_id: request.team_id,
                        cur_avatar_guid: result.as_ref().copied().unwrap_or(0),
                        avatar_team_guid_list: request.avatar_team_guid_list,
                        retcode: result.err().unwrap_or(Retcode::RetSucc).into(),
                    },
                );
            }
            "ChooseCurAvatarTeamReq" => {
                let Some(request) = message.decode::<ChooseCurAvatarTeamReq>() else {
                    continue;
                };
                let result = (|| -> Result<(), Retcode> {
                    can_change_team(player, multiplayer)?;
                    let avatars = player
                        .avatar_bin
                        .as_ref()
                        .ok_or(Retcode::RetCanNotFindTeam)?;
                    let guids = avatars
                        .team_map
                        .get(&request.team_id)
                        .ok_or(Retcode::RetCanNotFindTeam)?
                        .avatar_guid_list
                        .clone();
                    validate_team(avatars, &guids)?;
                    let last = avatars.team_map[&request.team_id].last_cur_avatar_guid;
                    let current = select_alive(
                        avatars,
                        &guids,
                        if last != 0 {
                            last
                        } else {
                            avatars.cur_avatar_guid
                        },
                    )?;
                    let avatars = player.avatar_bin.as_mut().unwrap();
                    avatars
                        .team_map
                        .get_mut(&request.team_id)
                        .unwrap()
                        .last_cur_avatar_guid = current;
                    if avatars.cur_team_id == request.team_id
                        && avatars.cur_avatar_guid_list == guids
                        && avatars.cur_avatar_guid == current
                    {
                        return Ok(());
                    }
                    avatars.cur_team_id = request.team_id;
                    avatars.cur_avatar_guid = current;
                    avatars.cur_avatar_guid_list.clone_from(&guids);
                    change_events.write(PlayerAvatarTeamChanged {
                        uid,
                        avatar_team_guid_list: guids,
                        cur_avatar_guid: current,
                    });
                    Ok(())
                })();
                out.send(
                    uid,
                    "ChooseCurAvatarTeamRsp",
                    ChooseCurAvatarTeamRsp {
                        retcode: result.err().unwrap_or(Retcode::RetSucc).into(),
                        cur_team_id: player.avatar_bin.as_ref().map_or(0, |a| a.cur_team_id),
                    },
                );
            }
            "ChangeTeamNameReq" => {
                let Some(request) = message.decode::<ChangeTeamNameReq>() else {
                    continue;
                };
                let result = (|| -> Result<(), Retcode> {
                    if request.team_id <= 0 {
                        return Err(Retcode::RetCanNotFindTeam);
                    }
                    let max_length = snezhnaya_gi_data::excel::production::ProductionConfig::get()
                        .constants
                        .get("CONST_VALUE_TEAMNAME_MAX_LENGTH")
                        .and_then(|v| v.first())
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(16);
                    if request.team_name.encode_utf16().count() > max_length
                        || request
                            .team_name
                            .chars()
                            .any(|c| c.is_control() || matches!(c, '<' | '>'))
                    {
                        return Err(Retcode::RetTeamNameIllegal);
                    }
                    let team = player
                        .avatar_bin
                        .as_mut()
                        .and_then(|a| a.team_map.get_mut(&(request.team_id as u32)))
                        .ok_or(Retcode::RetCanNotFindTeam)?;
                    team.team_name.clone_from(&request.team_name);
                    team_notify(player, &out);
                    Ok(())
                })();
                out.send(
                    uid,
                    "ChangeTeamNameRsp",
                    ChangeTeamNameRsp {
                        team_id: request.team_id,
                        team_name: request.team_name,
                        retcode: result.err().unwrap_or(Retcode::RetSucc).into(),
                    },
                );
            }
            _ => (),
        }
    }
}

pub fn replace_avatar_team(
    mut events: MessageReader<PlayerAvatarTeamChanged>,
    mut join_team_events: MessageWriter<PlayerJoinTeamEvent>,
) {
    for event in events.read() {
        join_team_events.write(PlayerJoinTeamEvent {
            player_uid: event.uid,
            avatar_guid_list: event.avatar_team_guid_list.clone(),
            appear_avatar_guid: event.cur_avatar_guid,
        });
    }
}

#[instrument(skip_all)]
pub fn notify_avatar_team_update(
    mut events: MessageReader<PlayerAvatarTeamChanged>,
    players: Res<Players>,
    out: Res<MessageOutput>,
) {
    for event in events.read() {
        debug!("{event:?}");

        let Some(player_info) = players.get(event.uid) else {
            continue;
        };

        team_notify(player_info, &out);
    }
}
