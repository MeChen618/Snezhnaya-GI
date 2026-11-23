use bevy_ecs::change_detection::{Res, ResMut};
use bevy_ecs::message::{MessageReader, MessageWriter};
use bevy_ecs::prelude::Commands;
use common::gm_util::{Command, ItemAction};
use rand::rngs::SmallRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use snezhnaya_gi_data::excel::common::ItemType;
use snezhnaya_gi_data::excel::{
    material_excel_config_collection, reliquary_affix_excel_config_collection,
    reliquary_excel_config_collection, reliquary_main_prop_excel_config_collection,
    weapon_excel_config_collection,
};
use snezhnaya_gi_data::quest::quest_config::{QuestCond, QuestContent};
use snezhnaya_gi_entity::common::{EntityCounter, Visible};
use snezhnaya_gi_entity::gadget::spawn_gadget_entity;
use snezhnaya_gi_event::command::{ConsoleChatNotifyEvent, GmCommandEvent};
use snezhnaya_gi_event::inventory::{ItemAddEvent, ItemDropEvent, StoreItemChangeEvent};
use snezhnaya_gi_event::quest::{QuestAcceptCondEvent, QuestContentProgressEvent};
use snezhnaya_gi_event::scene::{WorldOwnerUID, WorldVersionConfig};
use snezhnaya_gi_message::output::MessageOutput;
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::normal::{
    equip, item, scene_gadget_info, Equip, Item, ItemAddHintNotify, ItemHint, Material, Reliquary,
    StoreItemChangeNotify, StoreItemDelNotify, StoreType, TrifleGadgetInfo, Weapon,
};
use snezhnaya_gi_proto::server_only::{equip_bin, item_bin, VectorBin};
use std::collections::HashMap;

pub fn item_command_handler(
    mut events: MessageReader<GmCommandEvent>,
    mut item_add_events: MessageWriter<ItemAddEvent>,
    mut item_drop_events: MessageWriter<ItemDropEvent>,
) {
    for GmCommandEvent(player_uid, command) in events.read() {
        let Command::Item(action) = command else {
            continue;
        };
        match action {
            ItemAction::Add {
                id,
                num,
                level,
                refinement,
                main_prop_id,
                append_prop_id_list,
            } => {
                item_add_events.write(ItemAddEvent(
                    *player_uid,
                    vec![(
                        *id,
                        *num,
                        *level,
                        *refinement,
                        *main_prop_id,
                        append_prop_id_list.clone(),
                    )],
                ));
            }
            ItemAction::Drop { id } => {
                item_drop_events.write(ItemDropEvent(*player_uid, None, vec![(*id, 1)]));
            }
            _ => {}
        }
    }
}

pub fn item_add_handler(
    mut events: MessageReader<ItemAddEvent>,
    mut gm_notify_events: MessageWriter<ConsoleChatNotifyEvent>,
    mut quest_content_events: MessageWriter<QuestContentProgressEvent>,
    mut quest_accept_events: MessageWriter<QuestAcceptCondEvent>,
    mut players: ResMut<Players>,
    output: Res<MessageOutput>,
) {
    use crate::transaction::{notify_transaction, transact, Amounts};
    for ItemAddEvent(player_uid, item_list) in events.read() {
        let Some(player) = players.get_mut(*player_uid) else {
            continue;
        };
        for (item_id, num, level, refinement, main_prop_id, append_props) in item_list {
            let count = num.unwrap_or(1);
            if count == 0 || count > 30000 {
                continue;
            }
            if let Some(config) = reliquary_excel_config_collection::get().get(item_id) {
                if main_prop_id.is_some_and(|id| {
                    !reliquary_main_prop_excel_config_collection::get()
                        .get(&id)
                        .is_some_and(|p| p.prop_depot_id == config.main_prop_depot_id)
                }) || append_props.iter().any(|(id, count)| {
                    *count > 9
                        || !reliquary_affix_excel_config_collection::get()
                            .get(id)
                            .is_some_and(|p| p.depot_id == config.append_prop_depot_id)
                }) {
                    gm_notify_events.write(ConsoleChatNotifyEvent(
                        *player_uid,
                        "invalid artifact properties".into(),
                    ));
                    continue;
                }
            }
            match transact(
                player,
                &Amounts::new(),
                &Amounts::from([(*item_id, count)]),
                &[],
            ) {
                Err(code) => {
                    gm_notify_events.write(ConsoleChatNotifyEvent(
                        *player_uid,
                        format!("item {} failed: {}", item_id, i32::from(code)),
                    ));
                }
                Ok(change) => {
                    for guid in &change.added {
                        let Some(item) = player
                            .item_bin
                            .as_mut()
                            .and_then(|items| items.get_mut_item(guid))
                        else {
                            continue;
                        };
                        let Some(item_bin::Detail::Equip(equip)) = item.detail.as_mut() else {
                            continue;
                        };
                        match equip.detail.as_mut() {
                            Some(equip_bin::Detail::Weapon(weapon)) => {
                                weapon.level = level.unwrap_or(1).clamp(1, 100);
                                weapon.promote_level = get_min_promote_level(weapon.level);
                                for value in weapon.affix_map.values_mut() {
                                    *value = refinement.unwrap_or(1).clamp(1, 5) - 1;
                                }
                            }
                            Some(equip_bin::Detail::Reliquary(relic)) => {
                                let config = &reliquary_excel_config_collection::get()[item_id];
                                let target_level = level
                                    .unwrap_or_default()
                                    .saturating_add(1)
                                    .clamp(1, config.max_level.max(1));
                                relic.exp = 0;
                                if let Some(main) = main_prop_id {
                                    relic.main_prop_id = *main;
                                    if append_props.is_empty() {
                                        let main_type =
                                            reliquary_main_prop_excel_config_collection::get()
                                                [main]
                                                .prop_type;
                                        let mut types =
                                            std::collections::HashSet::from([main_type]);
                                        relic.append_prop_id_list.clear();
                                        for _ in 0..config.append_prop_num.min(4) {
                                            let choices: Vec<_> =
                                                reliquary_affix_excel_config_collection::get()
                                                    .values()
                                                    .filter(|p| {
                                                        p.depot_id == config.append_prop_depot_id
                                                            && p.weight > 0
                                                            && !types.contains(&p.prop_type)
                                                    })
                                                    .collect();
                                            if let Ok(prop) = choices
                                                .choose_weighted(&mut rand::thread_rng(), |p| {
                                                    p.weight
                                                })
                                            {
                                                relic.append_prop_id_list.push(prop.id);
                                                types.insert(prop.prop_type);
                                            }
                                        }
                                    }
                                }
                                if !append_props.is_empty() {
                                    relic.append_prop_id_list = expand_map_to_vec(append_props);
                                    relic.level = target_level;
                                } else if crate::equip::set_relic_level(relic, config, target_level)
                                    .is_err()
                                {
                                    gm_notify_events.write(ConsoleChatNotifyEvent(
                                        *player_uid,
                                        "artifact level config is invalid".into(),
                                    ));
                                }
                            }
                            _ => (),
                        }
                    }
                    notify_transaction(&output, player, &change);
                    for content_type in [QuestContent::ObtainItem, QuestContent::ObtainVariousItem]
                    {
                        quest_content_events.write(QuestContentProgressEvent {
                            player_uid: *player_uid,
                            content_type,
                            param: *item_id,
                            param2: 0,
                            param3: 0,
                            add_progress: count,
                        });
                    }
                    quest_accept_events.write(QuestAcceptCondEvent {
                        player_uid: *player_uid,
                        cond_type: QuestCond::PackHaveItem,
                        param: *item_id,
                    });
                }
            }
        }
    }
}

pub fn item_drop_handler(
    mut events: MessageReader<ItemDropEvent>,
    mut commands: Commands,
    mut entity_counter: ResMut<EntityCounter>,
    mut gm_notify_events: MessageWriter<ConsoleChatNotifyEvent>,
    players: Res<Players>,
    world_owner_uid: Res<WorldOwnerUID>,
    world_version_config: Res<WorldVersionConfig>,
) {
    let mut rng = SmallRng::from_entropy();

    let weapon_excel_config_collection_clone =
        std::sync::Arc::clone(weapon_excel_config_collection::get());

    let reliquary_excel_config_collection_clone =
        std::sync::Arc::clone(reliquary_excel_config_collection::get());

    let material_excel_config_collection_clone =
        std::sync::Arc::clone(material_excel_config_collection::get());

    for ItemDropEvent(player_uid, pos, item_list) in events.read() {
        let mut player_uid = *player_uid;
        if player_uid == 0 {
            player_uid = world_owner_uid.0;
        }

        let Some(player_info) = players.get(player_uid) else {
            continue;
        };

        let born_pos: (f32, f32, f32) = {
            match pos {
                None => match &player_info.scene_bin {
                    None => (0.0, 0.0, 0.0),
                    Some(player_scene_bin) => (
                        player_scene_bin.my_cur_scene_pos.unwrap_or_default().x,
                        player_scene_bin.my_cur_scene_pos.unwrap_or_default().y + 1.5,
                        player_scene_bin.my_cur_scene_pos.unwrap_or_default().z,
                    ),
                },
                Some(pos) => *pos,
            }
        };

        for (item_id, count) in item_list.iter() {
            let mut item_type = ItemType::NONE;
            let mut gadget_id = 0;
            let mut item = None;
            if weapon_excel_config_collection_clone.contains_key(item_id) {
                item_type = ItemType::WEAPON;
            } else if reliquary_excel_config_collection_clone.contains_key(item_id) {
                item_type = ItemType::RELIQUARY;
            } else if material_excel_config_collection_clone.contains_key(item_id) {
                match material_excel_config_collection_clone.get(&item_id) {
                    None => {}
                    Some(material_config) => {
                        item_type = material_config.item_type;
                    }
                }
            } else {
                gm_notify_events.write(ConsoleChatNotifyEvent(
                    player_uid,
                    format!("unknown id:{}", item_id),
                ));
            }

            match item_type {
                ItemType::NONE => {}
                ItemType::VIRTUAL | ItemType::MATERIAL => {
                    let Some(material_config) = material_excel_config_collection_clone.get(item_id)
                    else {
                        continue;
                    };
                    if material_config.use_on_gain {
                        continue;
                    }
                    gadget_id = material_config.gadget_id;
                    item = Some(Item {
                        item_id: *item_id,
                        guid: 0,
                        detail: Some(item::Detail::Material(Material {
                            delete_info: None,
                            count: *count,
                        })),
                    });
                }
                ItemType::RELIQUARY => {
                    let Some(reliquary_config) =
                        reliquary_excel_config_collection_clone.get(item_id)
                    else {
                        continue;
                    };

                    gadget_id = reliquary_config.gadget_id;

                    item = Some(Item {
                        item_id: *item_id,
                        guid: 0,
                        detail: Some(item::Detail::Equip(Equip {
                            is_locked: false,
                            detail: Some(equip::Detail::Reliquary(Reliquary {
                                level: 1,
                                ..Default::default()
                            })),
                        })),
                    });
                }
                ItemType::WEAPON => {
                    let Some(weapon_config) = weapon_excel_config_collection_clone.get(item_id)
                    else {
                        continue;
                    };

                    gadget_id = weapon_config.gadget_id;

                    item = Some(Item {
                        item_id: *item_id,
                        guid: 0,
                        detail: Some(item::Detail::Equip(Equip {
                            is_locked: false,
                            detail: Some(equip::Detail::Weapon(Weapon {
                                level: 1,
                                ..Default::default()
                            })),
                        })),
                    });
                }
                ItemType::DISPLAY => {}
                ItemType::FURNITURE => {}
            }

            match item {
                None => {}
                Some(item) => {
                    let born_pos = random_offset_vec3(born_pos, &mut rng);
                    let Some(gadget_entity) = spawn_gadget_entity(
                        world_version_config.protocol_version.clone(),
                        &mut commands,
                        &mut entity_counter,
                        born_pos.into(),
                        VectorBin::default(),
                        gadget_id,
                        1,
                        true,
                        Some(scene_gadget_info::Content::TrifleGadget(TrifleGadgetInfo {
                            item: Some(item),
                            ..Default::default()
                        })),
                        None,
                        0,
                        0,
                    ) else {
                        continue;
                    };

                    commands.entity(gadget_entity.1).insert(Visible);
                }
            }
        }
    }
}

pub fn update_player_store(
    mut events: MessageReader<StoreItemChangeEvent>,
    message_output: Res<MessageOutput>,
    players: Res<Players>,
) {
    let mut merged: HashMap<u32, HashMap<u64, i32>> = HashMap::new();

    for StoreItemChangeEvent(uid, map) in events.read() {
        let entry = merged.entry(*uid).or_insert_with(HashMap::new);

        for (guid, delta) in map {
            *entry.entry(*guid).or_insert(0) += *delta;
        }
    }

    for (uid, final_map) in merged {
        let Some(player_info) = players.get(uid) else {
            continue;
        };
        let Some(ref player_item_bin) = player_info.item_bin else {
            continue;
        };

        let change_guids: Vec<u64> = final_map
            .keys()
            .filter(|guid| player_item_bin.get_item(guid).is_some())
            .cloned()
            .collect();

        let del_guids: Vec<u64> = final_map
            .keys()
            .filter(|guid| player_item_bin.get_item(guid).is_none())
            .cloned()
            .collect();

        if !change_guids.is_empty() {
            message_output.send(
                uid,
                "StoreItemChangeNotify",
                StoreItemChangeNotify {
                    store_type: StoreType::StorePack.into(),
                    item_list: player_item_bin
                        .iter()
                        .filter(|(guid, _)| change_guids.contains(guid))
                        .filter_map(|(_, item)| item.to_normal_proto())
                        .collect(),
                    reason: 0,
                },
            );
            message_output.send(
                uid,
                "ItemAddHintNotify",
                ItemAddHintNotify {
                    item_list: player_item_bin
                        .iter()
                        .filter(|(guid, _)| {
                            change_guids.contains(guid)
                                && final_map.get(guid).copied().unwrap_or(0) > 0
                        })
                        .map(|(guid, item)| ItemHint {
                            count: final_map.get(guid).copied().unwrap_or(0) as u32,
                            item_id: item.item_id,
                            is_new: false,
                            guid: *guid,
                        })
                        .collect(),
                    ..Default::default()
                },
            );
        }

        if !del_guids.is_empty() {
            message_output.send(
                uid,
                "StoreItemDelNotify",
                StoreItemDelNotify {
                    guid_list: del_guids,
                    store_type: StoreType::StorePack.into(),
                },
            );
        }
    }
}

fn expand_map_to_vec(map: &HashMap<u32, u32>) -> Vec<u32> {
    let mut v = Vec::new();
    for (k, count) in map {
        for _ in 0..(*count).clamp(0, 5) {
            v.push(*k);
        }
    }
    v
}

pub fn get_min_promote_level(level: u32) -> u32 {
    if level > 80 {
        6
    } else if level > 70 {
        5
    } else if level > 60 {
        4
    } else if level > 50 {
        3
    } else if level > 40 {
        2
    } else if level > 20 {
        1
    } else {
        0
    }
}

pub fn random_offset_vec3((x, y, z): (f32, f32, f32), rng: &mut SmallRng) -> (f32, f32, f32) {
    let dx = rng.gen_range(-0.5..0.5);
    let dy = rng.gen_range(-0.5..0.5);
    let dz = rng.gen_range(-0.5..0.5);

    (x + dx, y + dy, z + dz)
}

pub fn handle_inventory_request(
    mut messages: MessageReader<snezhnaya_gi_message::event::ClientMessageEvent>,
    mut players: ResMut<Players>,
    output: Res<MessageOutput>,
) {
    use snezhnaya_gi_proto::normal::{DestroyMaterialReq, DestroyMaterialRsp};
    for message in messages.read() {
        if message.message_name() != "DestroyMaterialReq" {
            continue;
        }
        let Some(request) = message.decode::<DestroyMaterialReq>() else {
            continue;
        };
        let Some(player) = players.get_mut(message.sender_uid()) else {
            continue;
        };
        let response = destroy_materials(player, &request, &output).unwrap_or_else(|code| {
            DestroyMaterialRsp {
                retcode: code.into(),
                ..Default::default()
            }
        });
        output.send(player.uid, "DestroyMaterialRsp", response);
    }
}

fn destroy_materials(
    player: &mut snezhnaya_gi_proto::server_only::PlayerDataBin,
    request: &snezhnaya_gi_proto::normal::DestroyMaterialReq,
    output: &MessageOutput,
) -> crate::transaction::InventoryResult<snezhnaya_gi_proto::normal::DestroyMaterialRsp> {
    use crate::transaction::*;
    use snezhnaya_gi_data::excel::common::DestroyRule;
    use snezhnaya_gi_proto::{normal::DestroyMaterialRsp, retcode::Retcode};
    if request.material_list.is_empty() || request.material_list.len() > 30_000 {
        return Err(Retcode::RetItemInvalidUseCount);
    }
    let items = player.item_bin.as_ref().ok_or(Retcode::RetItemNotExist)?;
    let mut seen = std::collections::HashSet::new();
    let mut materials = Vec::new();
    let mut equips = Vec::new();
    let mut rewards = Amounts::new();
    for entry in &request.material_list {
        if entry.count == 0 || !seen.insert(entry.guid) {
            return Err(Retcode::RetItemInvalidUseCount);
        }
        let item = items
            .get_item(&entry.guid)
            .ok_or(Retcode::RetItemNotExist)?;
        let (rule, ids, counts) = match &item.detail {
            Some(item_bin::Detail::Material(_)) => {
                let config = material_excel_config_collection::get()
                    .get(&item.item_id)
                    .ok_or(Retcode::RetNotFoundConfig)?;
                if config.item_type != ItemType::MATERIAL || config.use_on_gain {
                    return Err(Retcode::RetItemNotDropable);
                }
                materials.push((entry.guid, entry.count));
                (
                    config.destroy_rule,
                    &config.destroy_return_material,
                    &config.destroy_return_material_count,
                )
            }
            Some(item_bin::Detail::Equip(equip)) => {
                if entry.count != 1 {
                    return Err(Retcode::RetItemInvalidUseCount);
                }
                equips.push(entry.guid);
                match &equip.detail {
                    Some(equip_bin::Detail::Weapon(_)) => {
                        let config = weapon_excel_config_collection::get()
                            .get(&item.item_id)
                            .ok_or(Retcode::RetNotFoundConfig)?;
                        (
                            config.destroy_rule,
                            &config.destroy_return_material,
                            &config.destroy_return_material_count,
                        )
                    }
                    Some(equip_bin::Detail::Reliquary(_)) => {
                        let config = reliquary_excel_config_collection::get()
                            .get(&item.item_id)
                            .ok_or(Retcode::RetNotFoundConfig)?;
                        (
                            config.destroy_rule,
                            &config.destroy_return_material,
                            &config.destroy_return_material_count,
                        )
                    }
                    _ => return Err(Retcode::RetItemNotDropable),
                }
            }
            _ => return Err(Retcode::RetItemNotDropable),
        };
        if rule != DestroyRule::ReturnMaterial {
            return Err(Retcode::RetItemNotDropable);
        }
        if ids.len() != counts.len() {
            return Err(Retcode::RetNotFoundConfig);
        }
        for (&id, &count) in ids.iter().zip(counts) {
            add_amount(
                &mut rewards,
                id,
                count
                    .checked_mul(entry.count)
                    .ok_or(Retcode::RetItemExceedLimit)?,
            )?;
        }
    }
    let change = transact_items(player, &materials, &equips, &rewards)?;
    notify_transaction(output, player, &change);
    Ok(DestroyMaterialRsp {
        item_id_list: rewards.keys().copied().collect(),
        item_count_list: rewards.values().copied().collect(),
        ..Default::default()
    })
}
