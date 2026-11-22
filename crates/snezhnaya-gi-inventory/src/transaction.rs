use rand::seq::SliceRandom;
use snezhnaya_gi_data::{excel::common::ItemType, excel::*, prop_type::*};
use snezhnaya_gi_message::output::MessageOutput;
use snezhnaya_gi_proto::{
    normal::*,
    retcode::Retcode,
    server_only::{
        equip_bin, item_bin, EquipBin, ItemBin, MaterialBin, PlayerDataBin, PlayerItemCompBin,
        ReliquaryBin, WeaponBin,
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

fn currency(items: &PlayerItemCompBin, id: u32) -> Option<u32> {
    match id {
        201 => Some(items.hcoin),
        202 => Some(items.scoin),
        203 => Some(items.mcoin),
        204 => Some(items.home_coin),
        _ => None,
    }
}

fn currency_mut(items: &mut PlayerItemCompBin, id: u32) -> Option<&mut u32> {
    match id {
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
    let mut items = player.item_bin.clone().ok_or(Retcode::RetItemNotExist)?;
    let before = items
        .pack_store
        .as_ref()
        .ok_or(Retcode::RetItemNotExist)?
        .item_map
        .clone();
    let mut counter = player.guid_counter;
    let mut change = InventoryChange {
        rewards: rewards.clone(),
        ..Default::default()
    };
    let mut seen = HashSet::new();
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
        if item.owner_guid != 0 {
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
            return Err(if id == 202 {
                Retcode::RetScoinNotEnough
            } else {
                Retcode::RetItemCountNotEnough
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
        if let Some(value) = currency_mut(&mut items, id) {
            *value = value
                .checked_add(count)
                .filter(|v| *v <= i32::MAX as u32)
                .ok_or(Retcode::RetVirtualExceedLimit)?;
            continue;
        }
        if let Some(config) = material_excel_config_collection::get().get(&id) {
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
    player.item_bin = Some(items);
    player.guid_counter = counter;
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
    output.send(
        player.uid,
        "PlayerPropNotify",
        PlayerPropNotify {
            prop_map: currency_properties(items),
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
