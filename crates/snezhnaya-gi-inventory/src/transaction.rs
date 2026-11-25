use rand::seq::SliceRandom;
use snezhnaya_gi_data::{
    excel::common::{ItemType, MaterialType},
    excel::*,
    prop_type::*,
};
use snezhnaya_gi_message::output::MessageOutput;
use snezhnaya_gi_proto::{
    normal::*,
    retcode::Retcode,
    server_only::{
        avatar_bin, equip_bin, item_bin, AvatarFetterBin, EquipBin, ItemBin, MaterialBin,
        PlayerDataBin, PlayerItemCompBin, ReliquaryBin, WeaponBin,
    },
};
use std::collections::{BTreeMap, HashMap, HashSet};

pub type InventoryResult<T> = Result<T, Retcode>;
pub type Amounts = BTreeMap<u32, u32>;

#[derive(Default)]
pub struct InventoryChange {
    pub changed: Vec<u64>,
    pub removed: Vec<u64>,
    pub added: Vec<u64>,
    pub rewards: Amounts,
    pub resin_changed: bool,
    pub attendance_changed: bool,
    pub unlocked_name_cards: Vec<u32>,
    pub fetter_changed: Vec<u64>,
}

pub fn add_amount(items: &mut Amounts, id: u32, count: u32) -> InventoryResult<()> {
    if id == 0 || count == 0 {
        return Ok(());
    }
    let total = items
        .get(&id)
        .copied()
        .unwrap_or_default()
        .checked_add(count)
        .ok_or(Retcode::RetItemExceedLimit)?;
    items.insert(id, total);
    Ok(())
}

pub fn scale_items(items: &[production::ItemAmount], count: u32) -> InventoryResult<Amounts> {
    let mut result = Amounts::new();
    for item in items {
        add_amount(
            &mut result,
            item.id,
            item.count
                .checked_mul(count)
                .ok_or(Retcode::RetItemExceedLimit)?,
        )?;
    }
    Ok(result)
}

pub fn reward_items(ids: impl IntoIterator<Item = u32>) -> InventoryResult<Amounts> {
    let mut items = Amounts::new();
    for id in ids {
        let row = progression::ProgressionConfig::get()
            .rewards
            .get(&id)
            .ok_or(Retcode::RetItemInvalidTarget)?;
        for item in &row.reward_item_list {
            add_amount(&mut items, item.item_id, item.item_count)?;
        }
    }
    Ok(items)
}

fn currency(items: &PlayerItemCompBin, id: u32) -> Option<u32> {
    match id {
        106 => Some(items.resin_record.as_ref().map_or(0, |r| r.value)),
        201 => Some(items.hcoin),
        202 => Some(items.scoin),
        203 => Some(items.mcoin),
        204 => Some(items.home_coin),
        _ => None,
    }
}

fn currency_mut(items: &mut PlayerItemCompBin, id: u32) -> Option<&mut u32> {
    match id {
        106 => Some(
            &mut items
                .resin_record
                .get_or_insert_with(Default::default)
                .value,
        ),
        201 => Some(&mut items.hcoin),
        202 => Some(&mut items.scoin),
        203 => Some(&mut items.mcoin),
        204 => Some(&mut items.home_coin),
        _ => None,
    }
}

pub fn item_count(items: &PlayerItemCompBin, id: u32) -> u32 {
    currency(items, id).unwrap_or_else(|| {
        items
            .iter()
            .filter_map(|(_, item)| {
                if item.item_id != id {
                    return None;
                }
                match &item.detail {
                    Some(item_bin::Detail::Material(m)) => Some(m.count),
                    _ => None,
                }
            })
            .fold(0, u32::saturating_add)
    })
}

// 先在背包副本上完成校验；任何一步失败都不改原存档。
pub fn transact(
    player: &mut PlayerDataBin,
    costs: &Amounts,
    rewards: &Amounts,
    remove: &[u64],
) -> InventoryResult<InventoryChange> {
    transact_inner(
        player,
        costs,
        rewards,
        remove,
        &[],
        crate::consumables::timestamp(),
    )
}

pub(super) fn transact_at(
    player: &mut PlayerDataBin,
    costs: &Amounts,
    rewards: &Amounts,
    now: u32,
) -> InventoryResult<InventoryChange> {
    transact_inner(player, costs, rewards, &[], &[], now)
}

pub(super) fn consume_item(
    player: &mut PlayerDataBin,
    guid: u64,
    count: u32,
    rewards: &Amounts,
) -> InventoryResult<InventoryChange> {
    transact_items(player, &[(guid, count)], &[], rewards)
}

pub(super) fn transact_items(
    player: &mut PlayerDataBin,
    materials: &[(u64, u32)],
    remove: &[u64],
    rewards: &Amounts,
) -> InventoryResult<InventoryChange> {
    transact_inner(
        player,
        &Amounts::new(),
        rewards,
        remove,
        materials,
        crate::consumables::timestamp(),
    )
}

fn transact_inner(
    player: &mut PlayerDataBin,
    costs: &Amounts,
    rewards: &Amounts,
    remove: &[u64],
    materials: &[(u64, u32)],
    now: u32,
) -> InventoryResult<InventoryChange> {
    let mut items = player.item_bin.clone().ok_or(Retcode::RetItemNotExist)?;
    let resin_before = items.resin_record;
    crate::consumables::refresh_resin(&mut items, now);
    let before = items
        .pack_store
        .as_ref()
        .ok_or(Retcode::RetItemNotExist)?
        .item_map
        .clone();
    let mut counter = player.guid_counter;
    let mut experience = None;
    let mut fetter_updates = BTreeMap::new();
    let mut change = InventoryChange {
        rewards: rewards.clone(),
        ..Default::default()
    };
    let mut seen = HashSet::new();
    let mut total_costs = costs.clone();
    // 按 GUID 扣指定的材料叠，批量销毁和单次使用共用一次提交。
    for &(guid, count) in materials {
        if count == 0 || !seen.insert(guid) {
            return Err(Retcode::RetItemInvalidUseCount);
        }
        let item = items.get_mut_item(&guid).ok_or(Retcode::RetItemNotExist)?;
        if item.owner_guid != 0 {
            return Err(Retcode::RetItemInvalidTarget);
        }
        let Some(item_bin::Detail::Material(material)) = &mut item.detail else {
            return Err(Retcode::RetItemNotUsable);
        };
        if count > material.count {
            return Err(Retcode::RetItemCountNotEnough);
        }
        add_amount(&mut total_costs, item.item_id, count)?;
        material.count -= count;
        if material.count == 0 {
            items.remove_item(&guid);
        }
    }
    for guid in remove {
        if items
            .reliquary_transmuter
            .as_ref()
            .and_then(|s| s.pending_reforge.as_ref())
            .is_some_and(|p| p.guid == *guid)
        {
            return Err(Retcode::RetEquipIsLocked);
        }
        if !seen.insert(*guid) {
            return Err(Retcode::RetItemInvalidUseCount);
        }
        let item = items.get_item(guid).ok_or(Retcode::RetItemNotExist)?;
        if item.owner_guid != 0
            || player.avatar_bin.as_ref().is_some_and(|a| {
                a.avatar_map
                    .values()
                    .any(|avatar| avatar.equip_map.values().any(|equip| equip.guid == *guid))
            })
        {
            return Err(Retcode::RetEquipHasBeenWeared);
        }
        match &item.detail {
            Some(item_bin::Detail::Equip(equip)) if equip.is_locked => {
                return Err(Retcode::RetEquipIsLocked)
            }
            Some(item_bin::Detail::Equip(_)) => (),
            _ => return Err(Retcode::RetItemInvalidTarget),
        }
        items.remove_item(guid);
    }
    for (&id, &count) in costs {
        if count == 0 {
            continue;
        }
        if item_count(&items, id) < count {
            return Err(match id {
                106 => Retcode::RetResinNotEnough,
                201 => Retcode::RetHcoinNotEnough,
                202 => Retcode::RetScoinNotEnough,
                203 => Retcode::RetMcoinNotEnough,
                _ => Retcode::RetItemCountNotEnough,
            });
        }
        if let Some(value) = currency_mut(&mut items, id) {
            *value -= count;
            continue;
        }
        let mut left = count;
        let mut guids: Vec<_> = items
            .iter()
            .filter(|(_, item)| item.item_id == id)
            .map(|(guid, _)| *guid)
            .collect();
        guids.sort_unstable();
        for guid in guids {
            if let Some(ItemBin {
                detail: Some(item_bin::Detail::Material(material)),
                ..
            }) = items.get_mut_item(&guid)
            {
                let taken = left.min(material.count);
                material.count -= taken;
                left -= taken;
                if material.count == 0 {
                    items.remove_item(&guid);
                }
                if left == 0 {
                    break;
                }
            }
        }
    }
    for (&id, &count) in rewards {
        if count == 0 {
            continue;
        }
        if id == 105 {
            let avatars = player
                .avatar_bin
                .as_ref()
                .ok_or(Retcode::RetCanNotFindAvatar)?;
            let table = &progression::ProgressionConfig::get().fetter_level_exp;
            let max = *table.keys().max().ok_or(Retcode::RetNotFoundConfig)?;
            for guid in avatars
                .cur_avatar_guid_list
                .iter()
                .copied()
                .collect::<HashSet<_>>()
            {
                let Some(avatar) = avatars.avatar_map.get(&guid) else {
                    continue;
                };
                if matches!(avatar.avatar_id, 10000005 | 10000007) {
                    continue;
                }
                let Some(avatar_bin::Detail::FormalAvatar(formal)) = &avatar.detail else {
                    continue;
                };
                let mut fetter = formal.fetter.clone().unwrap_or(AvatarFetterBin {
                    level: 1,
                    ..Default::default()
                });
                if fetter.level >= max {
                    continue;
                }
                fetter.level = fetter.level.max(1);
                let mut exp = u64::from(fetter.exp) + u64::from(count);
                while fetter.level < max {
                    let need = *table.get(&fetter.level).ok_or(Retcode::RetNotFoundConfig)?;
                    if need == 0 || exp < u64::from(need) {
                        break;
                    }
                    exp -= u64::from(need);
                    fetter.level += 1;
                }
                fetter.exp = if fetter.level >= max {
                    0
                } else {
                    u32::try_from(exp).map_err(|_| Retcode::RetItemExceedLimit)?
                };
                fetter_updates.insert(guid, fetter);
            }
            continue;
        }
        if id == 102 {
            let basic = player
                .basic_bin
                .as_ref()
                .ok_or(Retcode::RetItemInvalidTarget)?;
            let table = &progression::ProgressionConfig::get().player_level_exp;
            let max_level = *table.keys().max().ok_or(Retcode::RetItemInvalidTarget)?;
            let mut level = basic.level.max(1);
            let mut exp = u64::from(basic.exp) + u64::from(count);
            while level < max_level {
                let cost = *table.get(&level).ok_or(Retcode::RetItemInvalidTarget)?;
                if cost == 0 || exp < u64::from(cost) {
                    break;
                }
                exp -= u64::from(cost);
                level += 1;
            }
            experience = Some((
                level,
                if level >= max_level {
                    0
                } else {
                    exp.try_into().map_err(|_| Retcode::RetItemExceedLimit)?
                },
            ));
            continue;
        }
        if let Some(value) = currency_mut(&mut items, id) {
            let limit = if id == 106 {
                production::ProductionConfig::get().resin.max_value
            } else {
                i32::MAX as u32
            };
            *value = value
                .checked_add(count)
                .filter(|v| *v <= limit)
                .ok_or(if id == 106 {
                    Retcode::RetResinExceedLimit
                } else {
                    Retcode::RetVirtualExceedLimit
                })?;
            continue;
        }
        if let Some(config) = material_excel_config_collection::get().get(&id) {
            if config.item_type == ItemType::MATERIAL
                && config.use_on_gain
                && config.material_type == MaterialType::Namecard
                && config
                    .item_use
                    .iter()
                    .any(|op| op.use_op == ItemUseOp::GainNameCard)
                && config
                    .item_use
                    .iter()
                    .all(|op| matches!(op.use_op, ItemUseOp::GainNameCard | ItemUseOp::None))
            {
                if !player
                    .social_bin
                    .as_ref()
                    .is_some_and(|s| s.unlock_name_card_list.contains(&id))
                {
                    change.unlocked_name_cards.push(id);
                }
                continue;
            }
            if config.item_type != ItemType::MATERIAL || config.use_on_gain {
                return Err(Retcode::RetNotSupportItem);
            }
            let total = item_count(&items, id)
                .checked_add(count)
                .ok_or(Retcode::RetMaterialExceedLimit)?;
            if config.stack_limit != 0 && total > config.stack_limit {
                return Err(Retcode::RetMaterialExceedLimit);
            }
            if let Some(guid) = items.has_material(id) {
                if let Some(ItemBin {
                    detail: Some(item_bin::Detail::Material(material)),
                    ..
                }) = items.get_mut_item(&guid)
                {
                    material.count = total;
                }
            } else {
                let guid = allocate_guid(player.uid, &mut counter)?;
                items.add_item(
                    guid,
                    ItemBin {
                        item_id: id,
                        item_type: ItemType::MATERIAL as u32,
                        guid,
                        detail: Some(item_bin::Detail::Material(MaterialBin {
                            count,
                            ..Default::default()
                        })),
                        ..Default::default()
                    },
                );
                change.added.push(guid);
            }
        } else {
            if count > 2100 {
                return Err(Retcode::RetEquipExceedLimit);
            }
            for _ in 0..count {
                let guid = allocate_guid(player.uid, &mut counter)?;
                let item = create_equip(id, guid)?;
                items.add_item(guid, item);
                change.added.push(guid);
            }
        }
    }
    let mut weapons = 0;
    let mut relics = 0;
    let mut materials = 0;
    for (_, item) in items.iter() {
        match item.item_type {
            3 => relics += 1,
            4 => weapons += 1,
            _ => materials += 1,
        }
    }
    if relics > 2100 || weapons > 2000 || materials > 30000 {
        return Err(Retcode::RetPackExceedMaxWeight);
    }
    for (guid, item) in items.iter() {
        if before.get(guid) != Some(item) {
            change.changed.push(*guid);
        }
    }
    change.removed = before
        .keys()
        .filter(|guid| items.get_item(guid).is_none())
        .copied()
        .collect();
    crate::consumables::refresh_resin(&mut items, now);
    change.resin_changed = resin_before != items.resin_record;
    player.item_bin = Some(items);
    player.guid_counter = counter;
    if !change.unlocked_name_cards.is_empty() {
        player
            .social_bin
            .get_or_insert_with(Default::default)
            .unlock_name_card_list
            .extend(change.unlocked_name_cards.iter().copied());
    }
    if let Some((level, exp)) = experience {
        let basic = player.basic_bin.as_mut().unwrap();
        basic.level = level;
        basic.exp = exp;
    }
    for (guid, fetter) in fetter_updates {
        let avatar = player
            .avatar_bin
            .as_mut()
            .unwrap()
            .avatar_map
            .get_mut(&guid)
            .unwrap();
        if let Some(avatar_bin::Detail::FormalAvatar(formal)) = &mut avatar.detail {
            formal.fetter = Some(fetter);
            change.fetter_changed.push(guid);
        }
    }
    for (&id, &count) in &total_costs {
        snezhnaya_gi_persistence::record_progress(player, "cost", id, count);
    }
    if let Some(&resin) = total_costs.get(&106) {
        change.attendance_changed = crate::attendance::spend_resin(player, resin, now);
    }
    for (&id, &count) in rewards {
        snezhnaya_gi_persistence::record_progress(player, "obtain", id, count);
    }
    Ok(change)
}

fn allocate_guid(uid: u32, counter: &mut u32) -> InventoryResult<u64> {
    *counter = counter.checked_add(1).ok_or(Retcode::RetItemExceedLimit)?;
    Ok(((uid as u64) << 32) | *counter as u64)
}

fn create_equip(id: u32, guid: u64) -> InventoryResult<ItemBin> {
    let (item_type, detail) = if let Some(config) = weapon_excel_config_collection::get().get(&id) {
        (
            ItemType::WEAPON,
            equip_bin::Detail::Weapon(WeaponBin {
                level: 1,
                affix_map: config
                    .skill_affix
                    .iter()
                    .filter(|id| **id != 0)
                    .map(|id| (*id, 0))
                    .collect(),
                ..Default::default()
            }),
        )
    } else if let Some(config) = reliquary_excel_config_collection::get().get(&id) {
        let mut rng = rand::thread_rng();
        let props: Vec<_> = reliquary_main_prop_excel_config_collection::get()
            .values()
            .filter(|p| p.prop_depot_id == config.main_prop_depot_id && p.weight > 0)
            .collect();
        let main = props
            .choose_weighted(&mut rng, |p| p.weight)
            .map_err(|_| Retcode::RetNotFoundConfig)?;
        let mut affixes = Vec::new();
        let mut types = HashSet::from([main.prop_type]);
        for _ in 0..config.append_prop_num.min(4) {
            let choices: Vec<_> = reliquary_affix_excel_config_collection::get()
                .values()
                .filter(|p| {
                    p.depot_id == config.append_prop_depot_id
                        && p.weight > 0
                        && !types.contains(&p.prop_type)
                })
                .collect();
            let affix = choices
                .choose_weighted(&mut rng, |p| p.weight)
                .map_err(|_| Retcode::RetNotFoundConfig)?;
            types.insert(affix.prop_type);
            affixes.push(affix.id);
        }
        (
            ItemType::RELIQUARY,
            equip_bin::Detail::Reliquary(ReliquaryBin {
                level: 1,
                main_prop_id: main.id,
                append_prop_id_list: affixes,
                ..Default::default()
            }),
        )
    } else {
        return Err(Retcode::RetNotFoundConfig);
    };
    Ok(ItemBin {
        item_id: id,
        guid,
        item_type: item_type as u32,
        detail: Some(item_bin::Detail::Equip(EquipBin {
            detail: Some(detail),
            is_locked: false,
        })),
        ..Default::default()
    })
}

pub fn notify_transaction(
    output: &MessageOutput,
    player: &PlayerDataBin,
    change: &InventoryChange,
) {
    let Some(items) = &player.item_bin else {
        return;
    };
    if !change.changed.is_empty() {
        output.send(
            player.uid,
            "StoreItemChangeNotify",
            StoreItemChangeNotify {
                store_type: StoreType::StorePack as i32,
                item_list: change
                    .changed
                    .iter()
                    .filter_map(|guid| items.get_item(guid)?.to_normal_proto())
                    .collect(),
                ..Default::default()
            },
        );
    }
    if !change.removed.is_empty() {
        output.send(
            player.uid,
            "StoreItemDelNotify",
            StoreItemDelNotify {
                store_type: StoreType::StorePack as i32,
                guid_list: change.removed.clone(),
            },
        );
    }
    for &name_card_id in &change.unlocked_name_cards {
        output.send(
            player.uid,
            "UnlockNameCardNotify",
            UnlockNameCardNotify { name_card_id },
        );
    }
    if !change.fetter_changed.is_empty() {
        let mut map = HashMap::new();
        for guid in &change.fetter_changed {
            let Some(avatar) = player
                .avatar_bin
                .as_ref()
                .and_then(|a| a.avatar_map.get(guid))
            else {
                continue;
            };
            let Some(avatar_bin::Detail::FormalAvatar(formal)) = &avatar.detail else {
                continue;
            };
            let Some(fetter) = &formal.fetter else {
                continue;
            };
            map.insert(
                *guid,
                AvatarFetterInfo {
                    exp_level: fetter.level,
                    exp_number: fetter.exp,
                    rewarded_fetter_level_list: fetter.rewarded_fetter_level_list.clone(),
                    fetter_list: FetterDataConfig::get_fetter_data_entries()
                        .get(&avatar.avatar_id)
                        .into_iter()
                        .flatten()
                        .map(|f| FetterData {
                            fetter_id: f.fetter_id,
                            fetter_state: 3,
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                },
            );
        }
        output.send(
            player.uid,
            "AvatarFetterDataNotify",
            AvatarFetterDataNotify {
                fetter_info_map: map,
            },
        );
    }
    if change.resin_changed {
        crate::consumables::notify_resin(player, output);
    }
    if change.attendance_changed {
        crate::attendance::notify(player, output);
    }
    let mut properties = currency_properties(items);
    if let Some(basic) = &player.basic_bin {
        for (id, value) in [
            (PROP_PLAYER_LEVEL, basic.level),
            (PROP_PLAYER_EXP, basic.exp),
        ] {
            properties.insert(
                id,
                PropValue {
                    r#type: id,
                    val: i64::from(value),
                    value: Some(prop_value::Value::Ival(i64::from(value))),
                },
            );
        }
    }
    output.send(
        player.uid,
        "PlayerPropNotify",
        PlayerPropNotify {
            prop_map: properties,
        },
    );
    if !change.rewards.is_empty() {
        output.send(
            player.uid,
            "ItemAddHintNotify",
            ItemAddHintNotify {
                item_list: change
                    .rewards
                    .iter()
                    .map(|(&item_id, &count)| ItemHint {
                        item_id,
                        count,
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            },
        );
    }
}

pub fn currency_properties(items: &PlayerItemCompBin) -> HashMap<u32, PropValue> {
    [
        (
            PROP_PLAYER_RESIN,
            items.resin_record.as_ref().map_or(0, |r| r.value),
        ),
        (PROP_PLAYER_HCOIN, items.hcoin),
        (PROP_PLAYER_SCOIN, items.scoin),
        (PROP_PLAYER_MCOIN, items.mcoin),
        (PROP_PLAYER_HOME_COIN, items.home_coin),
    ]
    .into_iter()
    .map(|(prop, value)| {
        (
            prop,
            PropValue {
                r#type: prop,
                val: value as i64,
                value: Some(prop_value::Value::Ival(value as i64)),
            },
        )
    })
    .collect()
}
