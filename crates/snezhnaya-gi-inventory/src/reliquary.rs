mod transmuter;
use crate::transaction::*;
use bevy_ecs::prelude::*;
use rand::seq::SliceRandom;
use snezhnaya_gi_data::{excel::*, prop_type::PROP_RELIQUARY_TEMPORARY_EXP};
use snezhnaya_gi_event::avatar::AvatarEquipChangeEvent;
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::{
    normal::*,
    retcode::Retcode,
    server_only::{
        equip_bin, item_bin, ItemBin, PlayerDataBin, ReliquaryWearCustomDataBin, Uint32PairBin,
    },
};
use std::collections::{HashMap, HashSet};
pub(crate) use transmuter::guaranteed_upgrade;
use transmuter::{offering, purchase, reforge, reforge_select, transmuter_notify};

fn filter_notify(player: &PlayerDataBin, output: &MessageOutput) {
    output.send(
        player.uid,
        "ReliquaryFilterStateNotify",
        ReliquaryFilterStateNotify {
            reliquary_filter_state_map: player
                .item_bin
                .iter()
                .flat_map(|i| i.reliquary_filter_state_list.iter())
                .map(|p| Uint32Pair {
                    key: p.key,
                    value: p.value,
                })
                .collect(),
        },
    );
}

pub fn login(mut players: ResMut<Players>, output: Res<MessageOutput>) {
    for player in players.values_mut() {
        filter_notify(player, &output);
        transfer_prop(player, &output);
        transmuter_notify(player, &output);
    }
}

pub fn handle(
    mut messages: MessageReader<ClientMessageEvent>,
    mut players: ResMut<Players>,
    output: Res<MessageOutput>,
    mut equip_events: MessageWriter<AvatarEquipChangeEvent>,
) {
    for message in messages.read() {
        let Some(player) = players.get_mut(message.sender_uid()) else {
            continue;
        };
        match message.message_name() {
            "GetReliquaryOfferingReq" => {
                if let Some(request) = message.decode::<GetReliquaryOfferingReq>() {
                    let response = offering(player, &request, &output).unwrap_or_else(|code| {
                        GetReliquaryOfferingRsp {
                            schedule_id: request.schedule_id,
                            retcode: code.into(),
                            ..Default::default()
                        }
                    });
                    output.send(player.uid, "GetReliquaryOfferingRsp", response);
                    transmuter_notify(player, &output);
                }
            }
            "GetReliquaryPurchaseReq" => {
                if let Some(request) = message.decode::<GetReliquaryPurchaseReq>() {
                    let response = purchase(player, &request, &output).unwrap_or_else(|code| {
                        GetReliquaryPurchaseRsp {
                            schedule_id: request.schedule_id,
                            config_id: request.config_id,
                            retcode: code.into(),
                        }
                    });
                    output.send(player.uid, "GetReliquaryPurchaseRsp", response);
                    transmuter_notify(player, &output);
                }
            }
            "GetReliquaryDustReq" => {
                if let Some(request) = message.decode::<GetReliquaryDustReq>() {
                    let response = reforge(player, &request, &output).unwrap_or_else(|code| {
                        GetReliquaryDustRsp {
                            guid: request.guid,
                            retcode: code.into(),
                        }
                    });
                    output.send(player.uid, "GetReliquaryDustRsp", response);
                }
            }
            "GetReliquaryDustSelectReq" => {
                if let Some(request) = message.decode::<GetReliquaryDustSelectReq>() {
                    let response = match reforge_select(player, &request, &output) {
                        Ok(response) => {
                            if request.is_use_new {
                                if let Some(item) = player
                                    .item_bin
                                    .as_ref()
                                    .and_then(|i| i.get_item(&request.guid))
                                    .filter(|i| i.owner_guid != 0)
                                {
                                    if let Some(config) =
                                        reliquary_excel_config_collection::get().get(&item.item_id)
                                    {
                                        equip_events.write(AvatarEquipChangeEvent {
                                            player_uid: player.uid,
                                            avatar_guid: item.owner_guid,
                                            equip_type: config.equip_type,
                                        });
                                    }
                                }
                            }
                            response
                        }
                        Err(code) => GetReliquaryDustSelectRsp {
                            retcode: code.into(),
                        },
                    };
                    output.send(player.uid, "GetReliquaryDustSelectRsp", response);
                }
            }
            "SetEquipLockStateReq" => {
                if let Some(request) = message.decode::<SetEquipLockStateReq>() {
                    let result = set_locked(
                        player,
                        &[request.target_equip_guid],
                        request.is_locked,
                        &output,
                    );
                    output.send(
                        player.uid,
                        "SetEquipLockStateRsp",
                        SetEquipLockStateRsp {
                            target_equip_guid: request.target_equip_guid,
                            is_locked: request.is_locked,
                            retcode: result.err().map_or(0, Into::into),
                        },
                    );
                }
            }
            "SetEquipLockStateBatchReq" => {
                if let Some(request) = message.decode::<SetEquipLockStateBatchReq>() {
                    let result =
                        set_locked(player, &request.equip_guid_list, request.is_locked, &output);
                    output.send(
                        player.uid,
                        "SetEquipLockStateBatchRsp",
                        SetEquipLockStateBatchRsp {
                            is_locked: request.is_locked,
                            retcode: result.err().map_or(0, Into::into),
                        },
                    );
                }
            }
            "SetReliquaryStarStateReq" => {
                if let Some(request) = message.decode::<SetReliquaryStarStateReq>() {
                    let result = set_starred(
                        player,
                        request.target_reliquary_guid,
                        request.is_relic_starred,
                        &output,
                    );
                    output.send(
                        player.uid,
                        "SetReliquaryStarStateRsp",
                        SetReliquaryStarStateRsp {
                            target_reliquary_guid: request.target_reliquary_guid,
                            is_relic_starred: request.is_relic_starred,
                            retcode: result.err().map_or(0, Into::into),
                        },
                    );
                }
            }
            "ReliquaryFilterStateSaveNotify" => {
                if let Some(request) = message.decode::<ReliquaryFilterStateSaveNotify>() {
                    if request.reliquary_filter_state_map.len() <= 1024 {
                        let mut seen = HashSet::new();
                        player
                            .item_bin
                            .get_or_insert_with(Default::default)
                            .reliquary_filter_state_list = request
                            .reliquary_filter_state_map
                            .into_iter()
                            .filter(|p| p.value != 0 && seen.insert((p.key, p.value)))
                            .map(|p| Uint32PairBin {
                                key: p.key,
                                value: p.value,
                            })
                            .collect();
                    }
                    filter_notify(player, &output);
                }
            }
            "ReliquaryDecomposeReq" => {
                if let Some(request) = message.decode::<ReliquaryDecomposeReq>() {
                    let response = match decompose(player, &request, &output) {
                        Ok(response) => response,
                        Err(code) => ReliquaryDecomposeRsp {
                            retcode: code.into(),
                            ..Default::default()
                        },
                    };
                    output.send(player.uid, "ReliquaryDecomposeRsp", response);
                }
            }
            "GetReliquaryTransferReq" => {
                if let Some(request) = message.decode::<GetReliquaryTransferReq>() {
                    let old_exp = player
                        .item_bin
                        .as_ref()
                        .map_or(0, |i| i.reliquary_transfer_exp);
                    let response = match transfer(player, &request, &output) {
                        Ok(response) => response,
                        Err(code) => GetReliquaryTransferRsp {
                            old_exp,
                            retcode: code.into(),
                            ..Default::default()
                        },
                    };
                    output.send(player.uid, "GetReliquaryTransferRsp", response);
                }
            }
            "GetReliquaryWearCustomDataReq" => {
                if let Some(request) = message.decode::<GetReliquaryWearCustomDataReq>() {
                    wear_custom_notify(
                        player,
                        request.avatar_id,
                        request.avatar_element_type,
                        &output,
                    );
                }
            }
            "SaveReliquaryWearCustomDataReq" => {
                if let Some(request) = message.decode::<SaveReliquaryWearCustomDataReq>() {
                    let result = request
                        .custom_data
                        .as_ref()
                        .ok_or(Retcode::RetFail)
                        .and_then(|data| save_custom(player, data));
                    output.send(
                        player.uid,
                        "SaveReliquaryWearCustomDataRsp",
                        SaveReliquaryWearCustomDataRsp {
                            retcode: result.err().map_or(0, Into::into),
                        },
                    );
                }
            }
            "DeleteReliquaryWearCustomDataReq" => {
                if let Some(request) = message.decode::<DeleteReliquaryWearCustomDataReq>() {
                    if request.index <= 1 && owns_avatar(player, request.avatar_id) {
                        let data = &mut player
                            .item_bin
                            .get_or_insert_with(Default::default)
                            .reliquary_wear_custom_data;
                        data.retain(|d| {
                            !(d.avatar_id == request.avatar_id
                                && d.avatar_element_type == request.avatar_element_type
                                && d.index == request.index)
                        });
                        if request.index == 0 {
                            for data in data.iter_mut().filter(|d| {
                                d.avatar_id == request.avatar_id
                                    && d.avatar_element_type == request.avatar_element_type
                            }) {
                                data.index = 0;
                            }
                        }
                    }
                    wear_custom_notify(
                        player,
                        request.avatar_id,
                        request.avatar_element_type,
                        &output,
                    );
                }
            }
            _ => (),
        }
    }
}

fn owns_avatar(player: &PlayerDataBin, avatar_id: u32) -> bool {
    player
        .avatar_bin
        .as_ref()
        .is_some_and(|b| b.avatar_map.values().any(|a| a.avatar_id == avatar_id))
}

fn wear_custom_notify(
    player: &PlayerDataBin,
    avatar_id: u32,
    element: u32,
    output: &MessageOutput,
) {
    let mut response = GetReliquaryWearCustomDataRsp::default();
    if !owns_avatar(player, avatar_id) {
        response.retcode = Retcode::RetCanNotFindAvatar.into();
    } else {
        response.custom_data_list = player
            .item_bin
            .iter()
            .flat_map(|b| b.reliquary_wear_custom_data.iter())
            .filter(|d| d.avatar_id == avatar_id && d.avatar_element_type == element)
            .map(|d| ReliquaryWearCustomData {
                avatar_id: d.avatar_id,
                avatar_element_type: d.avatar_element_type,
                index: d.index,
                is_all: d.is_all,
                reliquary_set_id_list: d.reliquary_set_id_list.clone(),
                main_prop_type_list: d.main_prop_type_list.clone(),
                append_prop_type_list: d.append_prop_type_list.clone(),
                main_prop_type_map: d
                    .main_prop_type_map
                    .iter()
                    .map(|p| Uint32Pair {
                        key: p.key,
                        value: p.value,
                    })
                    .collect(),
            })
            .collect();
        response.custom_data_list.sort_by_key(|d| d.index);
    }
    output.send(player.uid, "GetReliquaryWearCustomDataRsp", response);
}

fn save_custom(player: &mut PlayerDataBin, data: &ReliquaryWearCustomData) -> InventoryResult<()> {
    if !owns_avatar(player, data.avatar_id) {
        return Err(Retcode::RetCanNotFindAvatar);
    }
    if data.index > 1
        || data.reliquary_set_id_list.len() > 16
        || data.main_prop_type_map.len() > 3
        || data.main_prop_type_list.len() > 64
        || data.append_prop_type_list.len() > 64
    {
        return Err(Retcode::RetFail);
    }
    let sets: HashSet<_> = reliquary_excel_config_collection::get()
        .values()
        .map(|r| r.set_id)
        .collect();
    let main: HashSet<_> = reliquary_main_prop_excel_config_collection::get()
        .values()
        .map(|r| r.prop_type as u32)
        .collect();
    let append: HashSet<_> = reliquary_affix_excel_config_collection::get()
        .values()
        .map(|r| r.prop_type as u32)
        .collect();
    let mut slots = HashSet::new();
    if data.reliquary_set_id_list.iter().any(|s| !sets.contains(s))
        || data.main_prop_type_list.iter().any(|p| !main.contains(p))
        || data
            .append_prop_type_list
            .iter()
            .any(|p| !append.contains(p))
        || data
            .main_prop_type_map
            .iter()
            .any(|p| !(3..=5).contains(&p.key) || !main.contains(&p.value) || !slots.insert(p.key))
    {
        return Err(Retcode::RetFail);
    }
    let saved = ReliquaryWearCustomDataBin {
        avatar_id: data.avatar_id,
        avatar_element_type: data.avatar_element_type,
        index: data.index,
        is_all: data.is_all,
        reliquary_set_id_list: data.reliquary_set_id_list.clone(),
        main_prop_type_list: data.main_prop_type_list.clone(),
        append_prop_type_list: data.append_prop_type_list.clone(),
        main_prop_type_map: data
            .main_prop_type_map
            .iter()
            .map(|p| Uint32PairBin {
                key: p.key,
                value: p.value,
            })
            .collect(),
    };
    let rows = &mut player
        .item_bin
        .get_or_insert_with(Default::default)
        .reliquary_wear_custom_data;
    if let Some(old) = rows.iter_mut().find(|d| {
        d.avatar_id == data.avatar_id
            && d.avatar_element_type == data.avatar_element_type
            && d.index == data.index
    }) {
        *old = saved;
    } else {
        rows.push(saved);
    }
    Ok(())
}

fn set_locked(
    player: &mut PlayerDataBin,
    guids: &[u64],
    locked: bool,
    output: &MessageOutput,
) -> InventoryResult<()> {
    if guids.is_empty() || guids.len() > 2100 {
        return Err(Retcode::RetItemInvalidUseCount);
    }
    let items = player.item_bin.as_mut().ok_or(Retcode::RetItemNotExist)?;
    for guid in guids {
        if !matches!(
            items.get_item(guid).and_then(|i| i.detail.as_ref()),
            Some(item_bin::Detail::Equip(_))
        ) {
            return Err(Retcode::RetItemNotExist);
        }
    }
    for guid in guids {
        if let Some(ItemBin {
            detail: Some(item_bin::Detail::Equip(equip)),
            ..
        }) = items.get_mut_item(guid)
        {
            equip.is_locked = locked;
        }
    }
    // 装备在角色和背包中各存一份，锁定状态保持一致。
    if let Some(avatars) = player.avatar_bin.as_mut() {
        let selected: HashSet<_> = guids.iter().copied().collect();
        for item in avatars
            .avatar_map
            .values_mut()
            .flat_map(|a| a.equip_map.values_mut())
        {
            if selected.contains(&item.guid) {
                if let Some(item_bin::Detail::Equip(e)) = item.detail.as_mut() {
                    e.is_locked = locked;
                }
            }
        }
    }
    notify_transaction(
        output,
        player,
        &InventoryChange {
            changed: guids.to_vec(),
            ..Default::default()
        },
    );
    Ok(())
}

fn set_starred(
    player: &mut PlayerDataBin,
    guid: u64,
    starred: bool,
    output: &MessageOutput,
) -> InventoryResult<()> {
    let item = player
        .item_bin
        .as_mut()
        .and_then(|i| i.get_mut_item(&guid))
        .ok_or(Retcode::RetItemNotExist)?;
    let Some(item_bin::Detail::Equip(equip)) = item.detail.as_mut() else {
        return Err(Retcode::RetItemInvalidTarget);
    };
    let Some(equip_bin::Detail::Reliquary(relic)) = equip.detail.as_mut() else {
        return Err(Retcode::RetItemInvalidTarget);
    };
    relic.is_relic_starred = starred;
    if let Some(avatars) = player.avatar_bin.as_mut() {
        for item in avatars
            .avatar_map
            .values_mut()
            .flat_map(|a| a.equip_map.values_mut())
            .filter(|i| i.guid == guid)
        {
            if let Some(item_bin::Detail::Equip(equip)) = item.detail.as_mut() {
                if let Some(equip_bin::Detail::Reliquary(relic)) = equip.detail.as_mut() {
                    relic.is_relic_starred = starred;
                }
            }
        }
    }
    notify_transaction(
        output,
        player,
        &InventoryChange {
            changed: vec![guid],
            ..Default::default()
        },
    );
    Ok(())
}

fn decompose(
    player: &mut PlayerDataBin,
    request: &ReliquaryDecomposeReq,
    output: &MessageOutput,
) -> InventoryResult<ReliquaryDecomposeRsp> {
    let config = production::ProductionConfig::get();
    let recipe = config
        .decompose
        .get(&request.config_id)
        .ok_or(Retcode::RetNotFoundConfig)?;
    if request.target_count == 0
        || request
            .target_count
            .checked_mul(recipe.need_reliquary_count)
            != Some(request.guid_list.len() as u32)
        || request.guid_list.len() > recipe.max_reliquary_num as usize
    {
        return Err(Retcode::RetReliquaryDecomposeParamError);
    }
    let items = player.item_bin.as_ref().ok_or(Retcode::RetItemNotExist)?;
    for guid in &request.guid_list {
        let item = items.get_item(guid).ok_or(Retcode::RetItemNotExist)?;
        let data = reliquary_excel_config_collection::get()
            .get(&item.item_id)
            .ok_or(Retcode::RetItemInvalidTarget)?;
        if data.rank_level != recipe.need_reliquary_rank_level {
            return Err(Retcode::RetReliquaryDecomposeParamError);
        }
    }
    let display = config
        .reward_preview_items
        .get(&recipe.reward_preview_id)
        .and_then(|r| r.first())
        .ok_or(Retcode::RetNotFoundConfig)?;
    let set = config
        .display_sets
        .get(display)
        .ok_or(Retcode::RetNotFoundConfig)?;
    let mut choices: Vec<_> = reliquary_excel_config_collection::get()
        .values()
        .filter(|r| {
            r.set_id == *set
                && r.rank_level == recipe.need_reliquary_rank_level
                && !r.dropable
                && (3..=4).contains(&r.append_prop_num)
        })
        .map(|r| r.id)
        .collect();
    choices.sort_unstable();
    let mut rewards = Amounts::new();
    let mut rng = rand::thread_rng();
    for _ in 0..request.target_count {
        add_amount(
            &mut rewards,
            *choices.choose(&mut rng).ok_or(Retcode::RetNotFoundConfig)?,
            1,
        )?;
    }
    let change = transact(player, &Amounts::new(), &rewards, &request.guid_list)?;
    notify_transaction(output, player, &change);
    Ok(ReliquaryDecomposeRsp {
        guid_list: change.added,
        ..Default::default()
    })
}

fn transfer_prop(player: &PlayerDataBin, output: &MessageOutput) {
    let exp = player
        .item_bin
        .as_ref()
        .map_or(0, |i| i.reliquary_transfer_exp) as i64;
    output.send(
        player.uid,
        "PlayerPropNotify",
        PlayerPropNotify {
            prop_map: HashMap::from([(
                PROP_RELIQUARY_TEMPORARY_EXP,
                PropValue {
                    r#type: PROP_RELIQUARY_TEMPORARY_EXP,
                    val: exp,
                    value: Some(prop_value::Value::Ival(exp)),
                },
            )]),
        },
    );
}

fn transfer(
    player: &mut PlayerDataBin,
    request: &GetReliquaryTransferReq,
    output: &MessageOutput,
) -> InventoryResult<GetReliquaryTransferRsp> {
    if request.guid_list.is_empty() || request.guid_list.len() > 2100 {
        return Err(Retcode::RetItemInvalidUseCount);
    }
    let items = player.item_bin.as_ref().ok_or(Retcode::RetItemNotExist)?;
    let old_exp = items.reliquary_transfer_exp;
    let mut added = 0u64;
    for guid in &request.guid_list {
        let item = items.get_item(guid).ok_or(Retcode::RetItemNotExist)?;
        let Some(item_bin::Detail::Equip(equip)) = item.detail.as_ref() else {
            return Err(Retcode::RetItemInvalidTarget);
        };
        let Some(equip_bin::Detail::Reliquary(relic)) = equip.detail.as_ref() else {
            return Err(Retcode::RetItemInvalidTarget);
        };
        let config = reliquary_excel_config_collection::get()
            .get(&item.item_id)
            .ok_or(Retcode::RetNotFoundConfig)?;
        if relic.level == 0 || relic.level > config.max_level {
            return Err(Retcode::RetItemInvalidTarget);
        }
        let mut exp = relic.exp as u64;
        for level in 1..relic.level {
            exp += reliquary_level_excel_config_collection::get()
                .get(&((config.rank_level << 8) + level))
                .ok_or(Retcode::RetNotFoundConfig)?
                .exp as u64;
        }
        added = added
            .checked_add(config.base_conv_exp as u64 + exp * 4 / 5)
            .ok_or(Retcode::RetItemExceedLimit)?;
    }
    let add_exp = u32::try_from(added).map_err(|_| Retcode::RetItemExceedLimit)?;
    let mut remaining = old_exp as u64 + added;
    let mut groups = Vec::new();
    for group in &production::ProductionConfig::get().transfer_materials {
        let mut exp = 0u64;
        for material in group {
            let config = material_excel_config_collection::get()
                .get(&material.id)
                .ok_or(Retcode::RetNotFoundConfig)?;
            let value = config
                .item_use
                .first()
                .and_then(|u| u.use_param.first())
                .and_then(|s| s.as_str().parse::<u64>().ok())
                .ok_or(Retcode::RetNotFoundConfig)?;
            exp += value
                .checked_mul(material.count as u64)
                .ok_or(Retcode::RetItemExceedLimit)?;
        }
        if exp == 0 {
            return Err(Retcode::RetNotFoundConfig);
        }
        groups.push((exp, group));
    }
    if groups.is_empty() {
        return Err(Retcode::RetNotFoundConfig);
    }
    groups.sort_by_key(|(exp, _)| std::cmp::Reverse(*exp));
    let mut rewards = Amounts::new();
    for (exp, materials) in groups {
        let count = remaining / exp;
        remaining %= exp;
        for material in materials {
            add_amount(
                &mut rewards,
                material.id,
                u32::try_from(count * material.count as u64)
                    .map_err(|_| Retcode::RetItemExceedLimit)?,
            )?;
        }
    }
    let remain_exp = u32::try_from(remaining).map_err(|_| Retcode::RetItemExceedLimit)?;
    let change = transact(player, &Amounts::new(), &rewards, &request.guid_list)?;
    player.item_bin.as_mut().unwrap().reliquary_transfer_exp = remain_exp;
    notify_transaction(output, player, &change);
    transfer_prop(player, output);
    Ok(GetReliquaryTransferRsp {
        old_exp,
        add_exp,
        remain_exp,
        transfer_item_list: rewards
            .into_iter()
            .map(|(item_id, count)| ItemParam { item_id, count })
            .collect(),
        ..Default::default()
    })
}
