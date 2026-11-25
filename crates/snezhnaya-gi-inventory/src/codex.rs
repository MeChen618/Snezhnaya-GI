use bevy_ecs::prelude::*;
use serde_json::json;
use snezhnaya_gi_data::excel::{
    codex::{reliquary_part, CodexConfig},
    common::QuestState,
};
use snezhnaya_gi_event::entity::ViewpointDiscoverEvent;
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::{progress_counter, Players};
use snezhnaya_gi_proto::{
    normal::{
        CodexDataFullNotify, CodexDataUpdateNotify, CodexType, CodexTypeData,
        QueryCodexMonsterBeKilledNumReq, SceneEntityDisappearNotify, ViewCodexReq, VisionType,
    },
    server_only::{
        equip_bin, item_bin, CodexTypeDataBin, PlayerCodexCompBin, PlayerDataBin, Uint32PairBin,
    },
};
use std::collections::HashSet;

fn unlock(
    codex: &mut PlayerCodexCompBin,
    kind: CodexType,
    id: u32,
    promote_level: u32,
    updates: &mut Vec<CodexDataUpdateNotify>,
) {
    if !codex
        .type_data_list
        .iter()
        .any(|data| data.r#type == kind as u32)
    {
        codex.type_data_list.push(CodexTypeDataBin {
            r#type: kind as u32,
            ..Default::default()
        });
    }
    let state = codex
        .type_data_list
        .iter_mut()
        .find(|data| data.r#type == kind as u32)
        .unwrap();
    let mut changed = false;
    if !state.codex_id_list.contains(&id) {
        state.codex_id_list.push(id);
        changed = true;
    }
    if kind == CodexType::CodexWeapon && promote_level > 0 {
        if let Some(entry) = state
            .weapon_max_promote_level_list
            .iter_mut()
            .find(|entry| entry.key == id)
        {
            if entry.value < promote_level {
                entry.value = promote_level;
                changed = true;
            }
        } else {
            state.weapon_max_promote_level_list.push(Uint32PairBin {
                key: id,
                value: promote_level,
            });
            changed = true;
        }
    }
    if changed {
        updates.push(CodexDataUpdateNotify {
            r#type: kind as i32,
            id,
            weapon_max_promote_level: state
                .weapon_max_promote_level_list
                .iter()
                .find(|entry| entry.key == id)
                .map_or(0, |entry| entry.value),
            ..Default::default()
        });
    }
}

pub fn record_item(
    player: &mut PlayerDataBin,
    item: u32,
    promote_level: u32,
    updates: &mut Vec<CodexDataUpdateNotify>,
) {
    let config = CodexConfig::get();
    for (kind, map) in [
        (CodexType::CodexWeapon, &config.weapons),
        (CodexType::CodexMaterial, &config.materials),
        (CodexType::CodexBooks, &config.books),
    ] {
        if let Some(&id) = map.get(&item) {
            unlock(
                player.codex_bin.get_or_insert_with(Default::default),
                kind,
                id,
                promote_level,
                updates,
            );
        }
    }

    let part = reliquary_part(item);
    let Some(sets) = config.reliquary_sets_by_part.get(&part) else {
        return;
    };
    let state = player.codex_bin.get_or_insert_with(Default::default);
    if state.reliquary_part_id_list.contains(&part) {
        return;
    }
    state.reliquary_part_id_list.push(part);
    for &id in sets {
        if config.reliquary_parts[&id]
            .iter()
            .all(|part| state.reliquary_part_id_list.contains(part))
        {
            unlock(state, CodexType::CodexReliquary, id, 0, updates);
        }
    }
}

pub fn notify_updates(output: &MessageOutput, uid: u32, updates: &[CodexDataUpdateNotify]) {
    for &update in updates {
        output.send(uid, "CodexDataUpdateNotify", update);
    }
}

pub fn discover_viewpoints(
    mut events: MessageReader<ViewpointDiscoverEvent>,
    mut players: ResMut<Players>,
    output: Res<MessageOutput>,
) {
    for event in events.read() {
        let Some(player) = players.get_mut(event.player_uid) else {
            continue;
        };
        let mut updates = Vec::new();
        unlock(
            player.codex_bin.get_or_insert_with(Default::default),
            CodexType::CodexView,
            event.codex_id,
            0,
            &mut updates,
        );
        if !updates.is_empty() {
            notify_updates(&output, event.player_uid, &updates);
            output.send(
                event.player_uid,
                "SceneEntityDisappearNotify",
                SceneEntityDisappearNotify {
                    entity_list: vec![event.gadget_entity_id],
                    disappear_type: VisionType::VisionGatherEscape as i32,
                    ..Default::default()
                },
            );
        }
    }
}

fn quest_updates(player: &mut PlayerDataBin, updates: &mut Vec<CodexDataUpdateNotify>) {
    let Some(quests) = player
        .quest_bin
        .as_ref()
        .and_then(|quests| quests.parent_quest_bin.as_ref())
    else {
        return;
    };
    let config = CodexConfig::get();
    for (&quest_id, quest) in &quests.parent_quest_map {
        if quest.state == QuestState::Finished as u32 || quest.total_finish_count > 0 {
            if let Some(&id) = config.quests.get(&quest_id) {
                unlock(
                    player.codex_bin.get_or_insert_with(Default::default),
                    CodexType::CodexQuest,
                    id,
                    0,
                    updates,
                );
            }
        }
    }
}

pub fn refresh_quests(player: &mut PlayerDataBin, output: &MessageOutput) {
    let mut updates = Vec::new();
    quest_updates(player, &mut updates);
    notify_updates(output, player.uid, &updates);
}

pub fn monster_killed(player: &mut PlayerDataBin, monster_id: u32, output: &MessageOutput) {
    if let Some(&id) = CodexConfig::get().monster_codex_ids.get(&monster_id) {
        let mut updates = Vec::new();
        unlock(
            player.codex_bin.get_or_insert_with(Default::default),
            CodexType::CodexAnimal,
            id,
            0,
            &mut updates,
        );
        notify_updates(output, player.uid, &updates);
    }
}

fn kills(player: &PlayerDataBin, codex_id: u32) -> u32 {
    CodexConfig::get()
        .monster_variants
        .get(&codex_id)
        .into_iter()
        .flatten()
        .map(|&id| progress_counter(player, "kill", id))
        .fold(0u64, u64::saturating_add)
        .min(u64::from(u32::MAX)) as u32
}

fn snapshot(state: &CodexTypeDataBin) -> CodexTypeData {
    let mut ids = state.codex_id_list.clone();
    ids.sort_unstable();
    ids.dedup();
    let viewed: HashSet<_> = state.viewed_codex_id_list.iter().copied().collect();
    CodexTypeData {
        r#type: state.r#type as i32,
        have_viewed_list: ids.iter().map(|id| viewed.contains(id)).collect(),
        codex_id_list: ids,
        weapon_max_promote_level_map: state
            .weapon_max_promote_level_list
            .iter()
            .map(|entry| (entry.key, entry.value))
            .collect(),
    }
}

pub fn login(mut players: ResMut<Players>, output: Res<MessageOutput>) {
    for player in players.values_mut() {
        let items: Vec<_> = player
            .item_bin
            .iter()
            .flat_map(|bin| bin.iter())
            .map(|(_, item)| {
                let promote_level = match item.detail.as_ref() {
                    Some(item_bin::Detail::Equip(equip)) => match equip.detail.as_ref() {
                        Some(equip_bin::Detail::Weapon(weapon)) => weapon.promote_level,
                        _ => 0,
                    },
                    _ => 0,
                };
                (item.item_id, promote_level)
            })
            .collect();
        let mut updates = Vec::new();
        for (item, promote_level) in items {
            record_item(player, item, promote_level, &mut updates);
        }
        quest_updates(player, &mut updates);
        for &id in CodexConfig::get().monster_variants.keys() {
            if kills(player, id) != 0 {
                unlock(
                    player.codex_bin.get_or_insert_with(Default::default),
                    CodexType::CodexAnimal,
                    id,
                    0,
                    &mut updates,
                );
            }
        }

        let state = player.codex_bin.get_or_insert_with(Default::default);
        let type_data_list = (1..=8)
            .map(|kind| {
                state
                    .type_data_list
                    .iter()
                    .find(|entry| entry.r#type == kind)
                    .map_or_else(
                        || CodexTypeData {
                            r#type: kind as i32,
                            ..Default::default()
                        },
                        snapshot,
                    )
            })
            .collect();
        output.send(
            player.uid,
            "CodexDataFullNotify",
            CodexDataFullNotify {
                type_data_list,
                recent_viewed_pushtips_list: state.recent_viewed_pushtips_list.clone(),
                last_read_pushtips_codex_id: state.last_read_pushtips_codex_id,
                last_read_pushtips_type_id: state.last_read_pushtips_type_id,
            },
        );
    }
}

pub fn handle(
    mut events: MessageReader<ClientMessageEvent>,
    mut players: ResMut<Players>,
    output: Res<MessageOutput>,
) {
    for message in events.read() {
        let Some(player) = players.get_mut(message.sender_uid()) else {
            continue;
        };
        match message.message_name() {
            "QueryCodexMonsterBeKilledNumReq" => {
                let Some(req) = message.decode::<QueryCodexMonsterBeKilledNumReq>() else {
                    continue;
                };
                let Some(animals) = player.codex_bin.as_ref().and_then(|codex| {
                    codex
                        .type_data_list
                        .iter()
                        .find(|entry| entry.r#type == CodexType::CodexAnimal as u32)
                }) else {
                    output.send_none(player.uid, "QueryCodexMonsterBeKilledNumRsp");
                    continue;
                };
                let ids: Vec<_> = req
                    .codex_id_list
                    .into_iter()
                    .filter(|id| animals.codex_id_list.contains(id))
                    .collect();
                let killed: Vec<_> = ids.iter().map(|&id| kills(player, id)).collect();
                // 7.2 抓包中字段 3 携带击败数，字段 10 与 ID 同长度。
                output.send_json(
                    player.uid,
                    "QueryCodexMonsterBeKilledNumRsp",
                    json!({
                        "codex_id_list": ids,
                        "AAACMIJHDCG": killed,
                        "IOIAHBHHJLM": vec![0u32; ids.len()],
                    }),
                );
            }
            "ViewCodexReq" => {
                let Some(req) = message.decode::<ViewCodexReq>() else {
                    continue;
                };
                let codex = player.codex_bin.get_or_insert_with(Default::default);
                let mut response = Vec::new();
                for request in req.type_data_list {
                    let Some(state) = codex
                        .type_data_list
                        .iter_mut()
                        .find(|entry| entry.r#type as i32 == request.r#type)
                    else {
                        continue;
                    };
                    let mut seen = HashSet::new();
                    let ids: Vec<_> = request
                        .codex_id_list
                        .into_iter()
                        .filter(|id| state.codex_id_list.contains(id) && seen.insert(*id))
                        .collect();
                    for &id in &ids {
                        if !state.viewed_codex_id_list.contains(&id) {
                            state.viewed_codex_id_list.push(id);
                        }
                    }
                    if !ids.is_empty() {
                        response.push(CodexTypeData {
                            r#type: request.r#type,
                            have_viewed_list: vec![true; ids.len()],
                            codex_id_list: ids,
                            ..Default::default()
                        });
                    }
                }
                output.send_json(
                    player.uid,
                    "ViewCodexRsp",
                    json!({
                        "type_data_list": response,
                        "recent_viewed_pushtips_list": codex.recent_viewed_pushtips_list,
                    }),
                );
            }
            _ => (),
        }
    }
}
