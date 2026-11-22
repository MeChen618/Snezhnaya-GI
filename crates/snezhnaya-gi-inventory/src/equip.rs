use bevy_ecs::prelude::*;
use snezhnaya_gi_data::excel::common::{EquipType, ItemType};
use snezhnaya_gi_data::excel::{
    reliquary_affix_excel_config_collection, reliquary_excel_config_collection,
    reliquary_level_excel_config_collection, reliquary_main_prop_excel_config_collection,
    weapon_excel_config_collection, weapon_level_excel_config_collection,
};
use snezhnaya_gi_event::avatar::AvatarEquipChangeEvent;
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::normal::{
    ReliquaryUpgradeReq, ReliquaryUpgradeRsp, TakeoffEquipReq, TakeoffEquipRsp, WeaponAwakenReq,
    WeaponAwakenRsp, WeaponPromoteReq, WeaponPromoteRsp, WeaponUpgradeReq, WeaponUpgradeRsp,
    WearEquipReq, WearEquipRsp,
};
use snezhnaya_gi_proto::retcode::Retcode;
use snezhnaya_gi_proto::server_only::{equip_bin, item_bin};
use std::collections::HashSet;
use tracing::{debug, instrument};

#[instrument(skip_all)]
pub fn change_avatar_equip(
    mut events: MessageReader<ClientMessageEvent>,
    mut equip_change_events: MessageWriter<AvatarEquipChangeEvent>,
    mut players: ResMut<Players>,
    message_output: Res<MessageOutput>,
) {
    for message in events.read() {
        match message.message_name() {
            "WearEquipReq" => {
                if let Some(request) = message.decode::<WearEquipReq>() {
                    let Some(player_info) = players.get_mut(message.sender_uid()) else {
                        continue;
                    };

                    let Some(ref mut player_avatar_bin) = player_info.avatar_bin else {
                        continue;
                    };

                    let Some(wear_avatar_bin) =
                        player_avatar_bin.avatar_map.get_mut(&request.avatar_guid)
                    else {
                        debug!("avatar with guid {} doesn't exist", request.avatar_guid);
                        continue;
                    };

                    let Some(ref mut player_item_bin) = player_info.item_bin else {
                        continue;
                    };

                    let Some(mut wear_item_bin) =
                        player_item_bin.get_item(&request.equip_guid).cloned()
                    else {
                        debug!("item with guid {} doesn't exist", request.equip_guid);
                        continue;
                    };

                    let Some(item_bin::Detail::Equip(ref _equip)) = wear_item_bin.detail else {
                        debug!("item with guid {} is not equip", request.equip_guid);
                        continue;
                    };

                    let wear_equip_type;

                    if wear_item_bin.item_type == ItemType::WEAPON as u32 {
                        wear_equip_type = EquipType::Weapon;
                    } else {
                        let reliquary_excel_config_collection_clone =
                            std::sync::Arc::clone(reliquary_excel_config_collection::get());

                        match reliquary_excel_config_collection_clone.get(&wear_item_bin.item_id) {
                            None => {
                                debug!("reliquary config {} doesn't exist", wear_item_bin.item_id);
                                continue;
                            }
                            Some(reliquary_config) => wear_equip_type = reliquary_config.equip_type,
                        }
                    }

                    let replace_avatar_guid;

                    if wear_equip_type != EquipType::None {
                        if wear_item_bin.owner_guid == request.avatar_guid {
                            message_output.send(
                                message.sender_uid(),
                                "WearEquipRsp",
                                WearEquipRsp {
                                    avatar_guid: request.avatar_guid,
                                    equip_guid: request.equip_guid,
                                    retcode: 0,
                                },
                            );
                            continue;
                        }

                        replace_avatar_guid = wear_item_bin.owner_guid;

                        wear_item_bin.owner_guid = request.avatar_guid;
                        let replace_item_bin = wear_avatar_bin
                            .equip_map
                            .insert(wear_equip_type as u32, wear_item_bin.clone());
                        player_item_bin.add_item(wear_item_bin.guid, wear_item_bin.clone());

                        if replace_avatar_guid == 0 {
                            match replace_item_bin {
                                None => {}
                                Some(mut replace_item_bin) => {
                                    replace_item_bin.owner_guid = 0;
                                    player_item_bin
                                        .add_item(replace_item_bin.guid, replace_item_bin.clone());
                                }
                            }
                        } else {
                            match player_avatar_bin.avatar_map.get_mut(&replace_avatar_guid) {
                                None => {
                                    debug!(
                                        "avatar with guid {} doesn't exist",
                                        replace_avatar_guid
                                    );
                                    match replace_item_bin {
                                        None => {}
                                        Some(mut replace_item_bin) => {
                                            replace_item_bin.owner_guid = 0;
                                            player_item_bin.add_item(
                                                replace_item_bin.guid,
                                                replace_item_bin.clone(),
                                            );
                                        }
                                    }
                                }
                                Some(other_avatar_bin) => match replace_item_bin {
                                    None => {
                                        other_avatar_bin
                                            .equip_map
                                            .remove(&(wear_equip_type as u32));
                                    }
                                    Some(mut replace_item_bin) => {
                                        replace_item_bin.owner_guid = replace_avatar_guid;
                                        other_avatar_bin.equip_map.insert(
                                            wear_equip_type as u32,
                                            replace_item_bin.clone(),
                                        );
                                        player_item_bin.add_item(
                                            replace_item_bin.guid,
                                            replace_item_bin.clone(),
                                        );
                                    }
                                },
                            }
                        }
                    } else {
                        continue;
                    }

                    equip_change_events.write(AvatarEquipChangeEvent {
                        player_uid: message.sender_uid(),
                        avatar_guid: request.avatar_guid,
                        equip_type: wear_equip_type,
                    });

                    if replace_avatar_guid != 0 {
                        equip_change_events.write(AvatarEquipChangeEvent {
                            player_uid: message.sender_uid(),
                            avatar_guid: replace_avatar_guid,
                            equip_type: wear_equip_type,
                        });
                    }

                    message_output.send(
                        message.sender_uid(),
                        "WearEquipRsp",
                        WearEquipRsp {
                            retcode: Retcode::RetSucc.into(),
                            avatar_guid: request.avatar_guid,
                            equip_guid: request.equip_guid,
                        },
                    );
                }
            }
            "TakeoffEquipReq" => {
                if let Some(request) = message.decode::<TakeoffEquipReq>() {
                    let take_off_equip_type = EquipType::from(request.slot);
                    if take_off_equip_type == EquipType::None {
                        continue;
                    }

                    let Some(player_info) = players.get_mut(message.sender_uid()) else {
                        continue;
                    };

                    let Some(ref mut player_avatar_bin) = player_info.avatar_bin else {
                        continue;
                    };

                    let Some(take_off_avatar_bin) =
                        player_avatar_bin.avatar_map.get_mut(&request.avatar_guid)
                    else {
                        debug!("avatar with guid {} doesn't exist", request.avatar_guid);
                        continue;
                    };

                    let Some(ref mut player_item_bin) = player_info.item_bin else {
                        continue;
                    };

                    let take_off_item_bin = take_off_avatar_bin
                        .equip_map
                        .remove(&(take_off_equip_type as u32));

                    match take_off_item_bin {
                        None => {}
                        Some(mut take_off_item_bin) => {
                            take_off_item_bin.owner_guid = 0;
                            player_item_bin.add_item(take_off_item_bin.guid, take_off_item_bin);
                        }
                    }

                    equip_change_events.write(AvatarEquipChangeEvent {
                        player_uid: message.sender_uid(),
                        avatar_guid: request.avatar_guid,
                        equip_type: take_off_equip_type,
                    });

                    message_output.send(
                        message.sender_uid(),
                        "TakeoffEquipRsp",
                        TakeoffEquipRsp {
                            retcode: Retcode::RetSucc.into(),
                            avatar_guid: request.avatar_guid,
                            slot: request.slot,
                        },
                    );
                }
            }
            "ReliquaryUpgradeReq" => {
                if let Some(request) = message.decode::<ReliquaryUpgradeReq>() {
                    let Some(player) = players.get_mut(message.sender_uid()) else {
                        continue;
                    };
                    let response =
                        upgrade_relic(player, &request, &message_output).unwrap_or_else(|code| {
                            ReliquaryUpgradeRsp {
                                retcode: code.into(),
                                target_reliquary_guid: request.target_reliquary_guid,
                                ..Default::default()
                            }
                        });
                    if response.retcode == 0 {
                        equip_updated(
                            player,
                            request.target_reliquary_guid,
                            &mut equip_change_events,
                        );
                    }
                    message_output.send(message.sender_uid(), "ReliquaryUpgradeRsp", response);
                }
            }
            "WeaponUpgradeReq" => {
                if let Some(request) = message.decode::<WeaponUpgradeReq>() {
                    let Some(player) = players.get_mut(message.sender_uid()) else {
                        continue;
                    };
                    let response = upgrade_weapon(player, &request, &message_output)
                        .unwrap_or_else(|code| WeaponUpgradeRsp {
                            retcode: code.into(),
                            target_weapon_guid: request.target_weapon_guid,
                            ..Default::default()
                        });
                    if response.retcode == 0 {
                        equip_updated(player, request.target_weapon_guid, &mut equip_change_events);
                    }
                    message_output.send(message.sender_uid(), "WeaponUpgradeRsp", response);
                }
            }
            "WeaponPromoteReq" => {
                let Some(request) = message.decode::<WeaponPromoteReq>() else {
                    continue;
                };
                let Some(player) = players.get_mut(message.sender_uid()) else {
                    continue;
                };
                let response =
                    promote_weapon(player, &request, &message_output).unwrap_or_else(|code| {
                        WeaponPromoteRsp {
                            target_weapon_guid: request.target_weapon_guid,
                            retcode: code.into(),
                            ..Default::default()
                        }
                    });
                if response.retcode == 0 {
                    equip_updated(player, request.target_weapon_guid, &mut equip_change_events);
                }
                message_output.send(message.sender_uid(), "WeaponPromoteRsp", response);
            }
            "WeaponAwakenReq" => {
                let Some(request) = message.decode::<WeaponAwakenReq>() else {
                    continue;
                };
                let Some(player) = players.get_mut(message.sender_uid()) else {
                    continue;
                };
                let response =
                    awaken_weapon(player, &request, &message_output).unwrap_or_else(|code| {
                        WeaponAwakenRsp {
                            target_weapon_guid: request.target_weapon_guid,
                            retcode: code.into(),
                            ..Default::default()
                        }
                    });
                if response.retcode == 0 {
                    equip_updated(player, request.target_weapon_guid, &mut equip_change_events);
                }
                message_output.send(message.sender_uid(), "WeaponAwakenRsp", response);
            }
            &_ => {}
        }
    }
}

fn upgrade_inputs(
    player: &snezhnaya_gi_proto::server_only::PlayerDataBin,
    target: u64,
    foods: &[u64],
    materials: &[snezhnaya_gi_proto::normal::ItemParam],
    relic: bool,
) -> crate::transaction::InventoryResult<(crate::transaction::Amounts, u32)> {
    use crate::transaction::*;
    use snezhnaya_gi_data::excel::{material_excel_config_collection, ItemUseOp};
    if foods.len() > 1000 || materials.len() > 100 || foods.contains(&target) {
        return Err(Retcode::RetItemInvalidUseCount);
    }
    let items = player.item_bin.as_ref().ok_or(Retcode::RetItemNotExist)?;
    if items
        .reliquary_transmuter
        .as_ref()
        .and_then(|s| s.pending_reforge.as_ref())
        .is_some_and(|p| p.guid == target)
    {
        return Err(Retcode::RetEquipIsLocked);
    }
    let mut seen = HashSet::new();
    let mut total = 0u64;
    let mut taxable = 0u64;
    for guid in foods {
        if !seen.insert(*guid) {
            return Err(Retcode::RetItemInvalidUseCount);
        }
        let item = items.get_item(guid).ok_or(Retcode::RetItemNotExist)?;
        if item.owner_guid != 0 {
            return Err(Retcode::RetEquipHasBeenWeared);
        }
        let Some(item_bin::Detail::Equip(equip)) = item.detail.as_ref() else {
            return Err(Retcode::RetItemInvalidTarget);
        };
        if equip.is_locked {
            return Err(Retcode::RetEquipIsLocked);
        }
        let (base, exp) = match equip.detail.as_ref() {
            Some(equip_bin::Detail::Reliquary(food)) if relic => {
                let config = reliquary_excel_config_collection::get()
                    .get(&item.item_id)
                    .ok_or(Retcode::RetNotFoundConfig)?;
                if food.level == 0 || food.level > config.max_level {
                    return Err(Retcode::RetItemInvalidTarget);
                }
                let mut exp = food.exp as u64;
                for level in 1..food.level {
                    exp += reliquary_level_excel_config_collection::get()
                        .get(&((config.rank_level << 8) + level))
                        .ok_or(Retcode::RetNotFoundConfig)?
                        .exp as u64;
                }
                (config.base_conv_exp as u64, exp)
            }
            Some(equip_bin::Detail::Weapon(food)) if !relic => {
                let config = weapon_excel_config_collection::get()
                    .get(&item.item_id)
                    .ok_or(Retcode::RetNotFoundConfig)?;
                if food.level == 0 || food.level > 100 {
                    return Err(Retcode::RetItemInvalidTarget);
                }
                let mut exp = food.exp as u64;
                for level in 1..food.level {
                    exp += weapon_exp(level, config.rank_level)? as u64;
                }
                (config.weapon_base_exp as u64, exp)
            }
            _ => return Err(Retcode::RetItemInvalidTarget),
        };
        total += base + exp * 4 / 5;
        taxable += base;
    }
    let mut costs = Amounts::new();
    for material in materials {
        if material.count == 0 {
            return Err(Retcode::RetItemInvalidUseCount);
        }
        let row = material_excel_config_collection::get()
            .get(&material.item_id)
            .ok_or(Retcode::RetNotFoundConfig)?;
        let exp = row
            .item_use
            .iter()
            .find(|op| {
                matches!(
                    (relic, op.use_op),
                    (true, ItemUseOp::AddReliquaryExp) | (false, ItemUseOp::AddWeaponExp)
                )
            })
            .and_then(|op| op.use_param.first())
            .and_then(|p| p.as_str().parse::<u32>().ok())
            .filter(|v| *v > 0)
            .ok_or(Retcode::RetItemInvalidTarget)?;
        add_amount(&mut costs, material.item_id, material.count)?;
        let amount = exp as u64 * material.count as u64;
        total += amount;
        taxable += amount;
    }
    if total == 0 || total > u32::MAX as u64 {
        return Err(Retcode::RetItemInvalidUseCount);
    }
    let coin = if relic { taxable } else { taxable / 10 };
    add_amount(
        &mut costs,
        202,
        u32::try_from(coin).map_err(|_| Retcode::RetItemExceedLimit)?,
    )?;
    Ok((costs, total as u32))
}

fn weapon_exp(level: u32, rank: u32) -> crate::transaction::InventoryResult<u32> {
    weapon_level_excel_config_collection::get()
        .get(&level)
        .and_then(|row| row.required_exps.get(rank.checked_sub(1)? as usize))
        .copied()
        .filter(|v| *v > 0)
        .ok_or(Retcode::RetNotFoundConfig)
}

fn commit_target(
    player: &mut snezhnaya_gi_proto::server_only::PlayerDataBin,
    item: snezhnaya_gi_proto::server_only::ItemBin,
    slot: EquipType,
    change: &mut crate::transaction::InventoryChange,
) {
    player
        .item_bin
        .as_mut()
        .unwrap()
        .add_item(item.guid, item.clone());
    change.changed.push(item.guid);
    if let Some(avatar) = player
        .avatar_bin
        .as_mut()
        .and_then(|a| a.avatar_map.get_mut(&item.owner_guid))
    {
        avatar.equip_map.insert(slot as u32, item);
    }
}

fn upgrade_weapon(
    player: &mut snezhnaya_gi_proto::server_only::PlayerDataBin,
    request: &WeaponUpgradeReq,
    output: &MessageOutput,
) -> crate::transaction::InventoryResult<WeaponUpgradeRsp> {
    use crate::transaction::*;
    let (costs, exp) = upgrade_inputs(
        player,
        request.target_weapon_guid,
        &request.food_weapon_guid_list,
        &request.item_param_list,
        false,
    )?;
    let mut target = player
        .item_bin
        .as_ref()
        .and_then(|i| i.get_item(&request.target_weapon_guid))
        .cloned()
        .ok_or(Retcode::RetItemNotExist)?;
    let row = weapon_excel_config_collection::get()
        .get(&target.item_id)
        .ok_or(Retcode::RetItemInvalidTarget)?;
    let Some(item_bin::Detail::Equip(equip)) = target.detail.as_mut() else {
        return Err(Retcode::RetItemInvalidTarget);
    };
    let Some(equip_bin::Detail::Weapon(weapon)) = equip.detail.as_mut() else {
        return Err(Retcode::RetItemInvalidTarget);
    };
    let old_level = weapon.level;
    let max = snezhnaya_gi_data::excel::weapon_promote_excel_config_collection::get()
        .get(&((row.weapon_promote_id << 8) | weapon.promote_level))
        .ok_or(Retcode::RetNotFoundConfig)?
        .unlock_max_level;
    if weapon.level == 0 || weapon.level >= max {
        return Err(Retcode::RetEquipLevelHigher);
    }
    let mut total = weapon.exp as u64 + exp as u64;
    while weapon.level < max {
        let required = weapon_exp(weapon.level, row.rank_level)? as u64;
        if total < required {
            break;
        }
        total -= required;
        weapon.level += 1;
    }
    weapon.exp = if weapon.level >= max {
        0
    } else {
        u32::try_from(total).map_err(|_| Retcode::RetItemExceedLimit)?
    };
    let response = WeaponUpgradeRsp {
        old_level,
        cur_level: weapon.level,
        target_weapon_guid: request.target_weapon_guid,
        ..Default::default()
    };
    let mut change = transact(
        player,
        &costs,
        &Amounts::new(),
        &request.food_weapon_guid_list,
    )?;
    commit_target(player, target, EquipType::Weapon, &mut change);
    notify_transaction(output, player, &change);
    Ok(response)
}

fn upgrade_relic(
    player: &mut snezhnaya_gi_proto::server_only::PlayerDataBin,
    request: &ReliquaryUpgradeReq,
    output: &MessageOutput,
) -> crate::transaction::InventoryResult<ReliquaryUpgradeRsp> {
    use crate::transaction::*;
    let (costs, exp) = upgrade_inputs(
        player,
        request.target_reliquary_guid,
        &request.food_reliquary_guid_list,
        &request.item_param_list,
        true,
    )?;
    let mut target = player
        .item_bin
        .as_ref()
        .and_then(|i| i.get_item(&request.target_reliquary_guid))
        .cloned()
        .ok_or(Retcode::RetItemNotExist)?;
    let row = reliquary_excel_config_collection::get()
        .get(&target.item_id)
        .ok_or(Retcode::RetItemInvalidTarget)?;
    let Some(item_bin::Detail::Equip(equip)) = target.detail.as_mut() else {
        return Err(Retcode::RetItemInvalidTarget);
    };
    let Some(equip_bin::Detail::Reliquary(relic)) = equip.detail.as_mut() else {
        return Err(Retcode::RetItemInvalidTarget);
    };
    if relic.level == 0 || relic.level >= row.max_level {
        return Err(Retcode::RetEquipLevelHigher);
    }
    let old_level = relic.level;
    let old_append_prop_list = relic.append_prop_id_list.clone();
    advance_relic(relic, row, exp)?;
    let response = ReliquaryUpgradeRsp {
        old_level,
        old_append_prop_list,
        cur_level: relic.level,
        cur_append_prop_list: relic.append_prop_id_list.clone(),
        power_up_rate: 1,
        target_reliquary_guid: request.target_reliquary_guid,
        ..Default::default()
    };
    let mut change = transact(
        player,
        &costs,
        &Amounts::new(),
        &request.food_reliquary_guid_list,
    )?;
    commit_target(player, target, row.equip_type, &mut change);
    notify_transaction(output, player, &change);
    Ok(response)
}

fn equip_updated(
    player: &snezhnaya_gi_proto::server_only::PlayerDataBin,
    guid: u64,
    events: &mut MessageWriter<AvatarEquipChangeEvent>,
) {
    if let Some(item) = player
        .item_bin
        .as_ref()
        .and_then(|i| i.get_item(&guid))
        .filter(|i| i.owner_guid != 0)
    {
        let slot = if item.item_type == ItemType::WEAPON as u32 {
            EquipType::Weapon
        } else {
            reliquary_excel_config_collection::get()
                .get(&item.item_id)
                .map_or(EquipType::None, |r| r.equip_type)
        };
        events.write(AvatarEquipChangeEvent {
            player_uid: player.uid,
            avatar_guid: item.owner_guid,
            equip_type: slot,
        });
    }
}

pub(crate) fn set_relic_level(
    relic: &mut snezhnaya_gi_proto::server_only::ReliquaryBin,
    row: &snezhnaya_gi_data::excel::ReliquaryExcelConfig,
    level: u32,
) -> crate::transaction::InventoryResult<()> {
    let mut exp = 0u32;
    for current in relic.level..level.min(row.max_level) {
        exp = exp
            .checked_add(
                reliquary_level_excel_config_collection::get()
                    .get(&((row.rank_level << 8) + current))
                    .ok_or(Retcode::RetNotFoundConfig)?
                    .exp,
            )
            .ok_or(Retcode::RetItemExceedLimit)?;
    }
    advance_relic(relic, row, exp.saturating_sub(relic.exp))
}

fn advance_relic(
    relic: &mut snezhnaya_gi_proto::server_only::ReliquaryBin,
    row: &snezhnaya_gi_data::excel::ReliquaryExcelConfig,
    exp: u32,
) -> crate::transaction::InventoryResult<()> {
    use rand::seq::SliceRandom;
    let mut total = relic.exp as u64 + exp as u64;
    let main = reliquary_main_prop_excel_config_collection::get()
        .get(&relic.main_prop_id)
        .ok_or(Retcode::RetNotFoundConfig)?
        .prop_type;
    let mut rng = rand::thread_rng();
    while relic.level < row.max_level {
        let required = reliquary_level_excel_config_collection::get()
            .get(&((row.rank_level << 8) + relic.level))
            .filter(|r| r.exp > 0)
            .ok_or(Retcode::RetNotFoundConfig)?
            .exp as u64;
        if total < required {
            break;
        }
        total -= required;
        relic.level += 1;
        if row.add_prop_levels.contains(&relic.level) {
            let mut types = Vec::new();
            for id in &relic.append_prop_id_list {
                let prop = reliquary_affix_excel_config_collection::get()
                    .get(id)
                    .ok_or(Retcode::RetNotFoundConfig)?
                    .prop_type;
                if !types.contains(&prop) {
                    types.push(prop);
                }
            }
            let selected = if types.len() < 4 {
                None
            } else {
                let guaranteed = crate::reliquary::guaranteed_upgrade(relic, row)?;
                Some(
                    guaranteed
                        .unwrap_or(*types.choose(&mut rng).ok_or(Retcode::RetNotFoundConfig)?),
                )
            };
            let choices: Vec<_> = reliquary_affix_excel_config_collection::get()
                .values()
                .filter(|p| {
                    p.depot_id == row.append_prop_depot_id
                        && p.weight > 0
                        && selected.map_or(
                            p.prop_type != main && !types.contains(&p.prop_type),
                            |prop| p.prop_type == prop,
                        )
                })
                .collect();
            relic.append_prop_id_list.push(
                choices
                    .choose_weighted(&mut rng, |p| p.weight)
                    .map_err(|_| Retcode::RetNotFoundConfig)?
                    .id,
            );
        }
    }
    relic.exp = if relic.level >= row.max_level {
        0
    } else {
        u32::try_from(total).map_err(|_| Retcode::RetItemExceedLimit)?
    };
    Ok(())
}

fn promote_weapon(
    player: &mut snezhnaya_gi_proto::server_only::PlayerDataBin,
    request: &WeaponPromoteReq,
    output: &MessageOutput,
) -> crate::transaction::InventoryResult<WeaponPromoteRsp> {
    use crate::transaction::*;
    use snezhnaya_gi_data::excel::weapon_promote_excel_config_collection;
    let mut target = player
        .item_bin
        .as_ref()
        .and_then(|items| items.get_item(&request.target_weapon_guid))
        .cloned()
        .ok_or(Retcode::RetItemNotExist)?;
    let config = weapon_excel_config_collection::get()
        .get(&target.item_id)
        .ok_or(Retcode::RetItemInvalidTarget)?;
    let Some(item_bin::Detail::Equip(equip)) = target.detail.as_mut() else {
        return Err(Retcode::RetItemInvalidTarget);
    };
    let Some(equip_bin::Detail::Weapon(weapon)) = equip.detail.as_mut() else {
        return Err(Retcode::RetItemInvalidTarget);
    };
    let old = weapon.promote_level;
    let current = weapon_promote_excel_config_collection::get()
        .get(&((config.weapon_promote_id << 8) | old))
        .ok_or(Retcode::RetNotFoundConfig)?;
    let next_level = old
        .checked_add(1)
        .ok_or(Retcode::RetWeaponPromoteLevelExceedLimit)?;
    let next = weapon_promote_excel_config_collection::get()
        .get(&((config.weapon_promote_id << 8) | next_level))
        .ok_or(Retcode::RetWeaponPromoteLevelExceedLimit)?;
    if weapon.level < current.unlock_max_level {
        return Err(Retcode::RetWeaponLevelInvalid);
    }
    if player.basic_bin.as_ref().map_or(0, |bin| bin.level) < next.required_player_level {
        return Err(Retcode::RetPlayerLevelLessThan);
    }
    let mut costs = Amounts::new();
    add_amount(&mut costs, 202, next.coin_cost)?;
    for item in &next.cost_items {
        add_amount(&mut costs, item.id, item.count)?;
    }
    weapon.promote_level = next_level;
    let mut change = transact(player, &costs, &Amounts::new(), &[])?;
    commit_target(player, target, EquipType::Weapon, &mut change);
    notify_transaction(output, player, &change);
    Ok(WeaponPromoteRsp {
        target_weapon_guid: request.target_weapon_guid,
        old_promote_level: old,
        cur_promote_level: next_level,
        retcode: 0,
    })
}

fn awaken_weapon(
    player: &mut snezhnaya_gi_proto::server_only::PlayerDataBin,
    request: &WeaponAwakenReq,
    output: &MessageOutput,
) -> crate::transaction::InventoryResult<WeaponAwakenRsp> {
    use crate::transaction::*;
    let items = player.item_bin.as_ref().ok_or(Retcode::RetItemNotExist)?;
    let mut target = items
        .get_item(&request.target_weapon_guid)
        .cloned()
        .ok_or(Retcode::RetItemNotExist)?;
    let config = weapon_excel_config_collection::get()
        .get(&target.item_id)
        .filter(|config| config.rank_level >= 3 && config.skill_affix.iter().any(|id| *id != 0))
        .ok_or(Retcode::RetItemInvalidTarget)?;
    let Some(item_bin::Detail::Equip(equip)) = target.detail.as_mut() else {
        return Err(Retcode::RetItemInvalidTarget);
    };
    let Some(equip_bin::Detail::Weapon(weapon)) = equip.detail.as_mut() else {
        return Err(Retcode::RetItemInvalidTarget);
    };
    let old_level = weapon.affix_map.values().copied().max().unwrap_or(0);
    if old_level >= 4 || request.item_guid_list.len() > 4 || request.awaken_material_count > 4 {
        return Err(Retcode::RetItemInvalidUseCount);
    }
    let mut costs = Amounts::new();
    let mut gains = Vec::new();
    let mut remove = Vec::new();
    let mut seen = HashSet::new();
    if request.item_guid_list.is_empty() {
        if config.awaken_material == 0 || request.awaken_material_count == 0 {
            return Err(Retcode::RetItemInvalidUseCount);
        }
        add_amount(
            &mut costs,
            config.awaken_material,
            request.awaken_material_count,
        )?;
        gains.resize(request.awaken_material_count as usize, 1);
    } else {
        let count = request.item_guid_list.len() as u32;
        if request.awaken_material_count != 0
            && count != 1
            && request.awaken_material_count != count
        {
            return Err(Retcode::RetItemInvalidUseCount);
        }
        for guid in &request.item_guid_list {
            if *guid == target.guid || !seen.insert(*guid) {
                return Err(Retcode::RetItemInvalidTarget);
            }
            let food = items.get_item(guid).ok_or(Retcode::RetItemNotExist)?;
            if config.awaken_material != 0 {
                if food.item_id != config.awaken_material
                    || !matches!(food.detail, Some(item_bin::Detail::Material(_)))
                {
                    return Err(Retcode::RetItemInvalidTarget);
                }
                let amount = if count == 1 {
                    request.awaken_material_count.max(1)
                } else {
                    1
                };
                add_amount(&mut costs, config.awaken_material, amount)?;
                gains.extend(std::iter::repeat_n(1, amount as usize));
            } else {
                if food.item_id != target.item_id
                    || (request.awaken_material_count != 0
                        && request.awaken_material_count != count)
                {
                    return Err(Retcode::RetItemInvalidTarget);
                }
                let Some(item_bin::Detail::Equip(equip)) = &food.detail else {
                    return Err(Retcode::RetItemInvalidTarget);
                };
                let Some(equip_bin::Detail::Weapon(food_weapon)) = &equip.detail else {
                    return Err(Retcode::RetItemInvalidTarget);
                };
                let rank = food_weapon.affix_map.values().copied().max().unwrap_or(0);
                if rank > 4 {
                    return Err(Retcode::RetItemInvalidTarget);
                }
                gains.push(rank + 1);
                remove.push(*guid);
            }
        }
    }
    if gains.is_empty() || gains.len() > 4 {
        return Err(Retcode::RetItemInvalidUseCount);
    }
    let mut next = old_level;
    for gain in gains {
        if next >= 4 {
            return Err(Retcode::RetItemInvalidUseCount);
        }
        let cost = config
            .awaken_costs
            .get(next as usize)
            .copied()
            .ok_or(Retcode::RetNotFoundConfig)?;
        add_amount(&mut costs, 202, cost)?;
        // 已精炼的素材继承等级，摩拉按每件素材收取一次。
        next = (next + gain).min(4);
    }
    let old_affix_level_map = weapon.affix_map.clone();
    for id in config.skill_affix.iter().filter(|id| **id != 0) {
        weapon.affix_map.insert(*id, next);
    }
    let response = WeaponAwakenRsp {
        target_weapon_guid: target.guid,
        avatar_guid: target.owner_guid,
        old_affix_level_map,
        cur_affix_level_map: weapon.affix_map.clone(),
        target_weapon_awaken_level: next,
        retcode: 0,
    };
    // 被锁定或已穿戴的素材也由背包事务统一拒绝。
    let mut change = transact(player, &costs, &Amounts::new(), &remove)?;
    commit_target(player, target, EquipType::Weapon, &mut change);
    notify_transaction(output, player, &change);
    Ok(response)
}
