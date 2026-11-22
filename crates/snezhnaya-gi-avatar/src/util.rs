use std::collections::HashMap;

use common::time_util;
use snezhnaya_gi_data::config::{process_inherent_proud_skills, process_talent_ids};
use snezhnaya_gi_data::excel::common::{EquipType, ItemType};
use snezhnaya_gi_data::excel::{
    avatar_excel_config_collection, avatar_skill_depot_excel_config_collection,
    avatar_skill_excel_config_collection, avatar_talent_excel_config_collection,
    proud_skill_excel_config_collection, AvatarExcelConfig,
};
use snezhnaya_gi_proto::server_only::*;

pub fn add_avatar_and_weapon(player: &mut PlayerDataBin, avatar: &AvatarExcelConfig) {
    let _ = create_avatar(player, avatar, true);
}

pub fn grant_avatar(player: &mut PlayerDataBin, avatar: &AvatarExcelConfig) -> Option<u64> {
    create_avatar(player, avatar, false)
}

fn create_avatar(
    player: &mut PlayerDataBin,
    avatar: &AvatarExcelConfig,
    trained: bool,
) -> Option<u64> {
    const CHOOSE_AVATAR_ID: u32 = 10000007;
    let initial_level = if trained { 100 } else { 1 };
    let initial_promote = if trained { 6 } else { 0 };
    let initial_talents = if trained { 6 } else { 0 };
    let weapon_level = if trained { 100 } else { 1 };
    let weapon_promote = if trained { 6 } else { 0 };
    const DEFAULT_FLYCLOAK_ID: u32 = 140001;

    if player.avatar_bin.is_none()
        || player
            .item_bin
            .as_ref()
            .and_then(|i| i.pack_store.as_ref())
            .is_none()
        || player.guid_counter.checked_add(2).is_none()
        || !snezhnaya_gi_data::excel::weapon_excel_config_collection::get()
            .contains_key(&avatar.initial_weapon)
    {
        return None;
    }
    let avatar_guid = player.next_guid();
    let weapon_guid = player.next_guid();

    let Some(ref mut avatar_bin) = player.avatar_bin else {
        return None;
    };

    let Some(ref mut item_bin) = player.item_bin else {
        return None;
    };

    let mut skill_level_map = HashMap::new();
    let mut inherent_proud_skill_list = Vec::new();

    let avatar_skill_depot_excel_config_collection_clone =
        std::sync::Arc::clone(avatar_skill_depot_excel_config_collection::get());

    let avatar_skill_excel_config_collection_clone =
        std::sync::Arc::clone(avatar_skill_excel_config_collection::get());

    let avatar_talent_collection_clone =
        std::sync::Arc::clone(avatar_talent_excel_config_collection::get());

    let proud_skill_collection_clone =
        std::sync::Arc::clone(proud_skill_excel_config_collection::get());

    if let Some(skill_depot_config) =
        avatar_skill_depot_excel_config_collection_clone.get(&avatar.skill_depot_id)
    {
        skill_depot_config
            .skills
            .iter()
            .filter(|id| **id != 0)
            .for_each(|id| {
                skill_level_map.insert(*id, if trained { 14 } else { 1 });
            });

        skill_depot_config
            .sub_skills
            .iter()
            .filter(|id| **id != 0)
            .for_each(|id| {
                skill_level_map.insert(*id, if trained { 14 } else { 1 });
            });

        skill_level_map.insert(
            skill_depot_config.energy_skill,
            if trained { 14 } else { 1 },
        );

        skill_depot_config
            .inherent_proud_skill_opens
            .iter()
            .filter(|s| {
                s.proud_skill_group_id != 0 && s.need_avatar_promote_level <= initial_promote
            })
            .for_each(|s| inherent_proud_skill_list.push(s.proud_skill_group_id * 100 + 1));
    }

    let Some(avatar_config) = avatar_excel_config_collection::get().get(&avatar.id) else {
        tracing::debug!("avatar config {} doesn't exist", avatar.id);
        return None;
    };

    let Some(skill_depot_config) =
        avatar_skill_depot_excel_config_collection_clone.get(&avatar_config.skill_depot_id)
    else {
        tracing::debug!(
            "avatar skill depot config {} doesn't exist",
            avatar_config.skill_depot_id
        );
        return None;
    };

    let talent_id_list: Vec<u32> = if initial_talents as usize > skill_depot_config.talents.len() {
        skill_depot_config.talents.clone()
    } else {
        skill_depot_config.talents[0..initial_talents as usize].to_vec()
    };

    let mut open_configs = Vec::new();

    open_configs.extend(process_talent_ids(
        &talent_id_list,
        &avatar_talent_collection_clone,
    ));

    open_configs.extend(process_inherent_proud_skills(
        &inherent_proud_skill_list,
        &proud_skill_collection_clone,
    ));

    let mut skill_extra_charge_map: HashMap<u32, AvatarSkillBin> = HashMap::new();

    for open_config in &open_configs {
        match snezhnaya_gi_data::config::get_avatar_talent_config(&open_config.clone().into()) {
            None => continue,
            Some(talent_action) => {
                for action in talent_action {
                    if let snezhnaya_gi_data::config::TalentAction::ModifySkillPoint {
                        skill_id,
                        point_delta,
                    } = action
                    {
                        if let Some(skill_config) =
                            avatar_skill_excel_config_collection_clone.get(&skill_id)
                        {
                            let max_charge_num = skill_config.max_charge_num;
                            let extra_charge = max_charge_num + point_delta;
                            skill_extra_charge_map.insert(
                                *skill_id,
                                AvatarSkillBin {
                                    max_charge_count: extra_charge,
                                    ..Default::default()
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    if avatar_config.id == CHOOSE_AVATAR_ID {
        avatar_bin.choose_avatar_guid = avatar_guid;
    }

    let mut depot_map: HashMap<u32, AvatarSkillDepotBin> = HashMap::new();
    depot_map.insert(
        avatar_config.skill_depot_id,
        AvatarSkillDepotBin {
            talent_id_list,
            core_proud_skill_level: initial_talents,
            inherent_proud_skill_list,
            skill_level_map,
        },
    );

    let mut equip_map = HashMap::new();
    let weapon_item_bin = ItemBin {
        item_type: ItemType::WEAPON as u32,
        item_id: avatar_config.initial_weapon,
        guid: weapon_guid,
        owner_guid: avatar_guid,
        detail: Some(item_bin::Detail::Equip(EquipBin {
            is_locked: false,
            detail: Some(equip_bin::Detail::Weapon(WeaponBin {
                level: weapon_level,
                exp: 0,
                promote_level: weapon_promote,
                affix_map: HashMap::with_capacity(0),
            })),
        })),
    };

    equip_map.insert(EquipType::Weapon as u32, weapon_item_bin.clone());

    item_bin.add_item(weapon_guid, weapon_item_bin);

    avatar_bin.avatar_map.insert(
        avatar_guid,
        AvatarBin {
            avatar_type: 1,
            detail: Some(avatar_bin::Detail::FormalAvatar(FormalAvatarBin {
                fetter: Some(AvatarFetterBin {
                    level: 1,
                    ..Default::default()
                }),
                ..Default::default()
            })),
            avatar_id: avatar_config.id,
            level: initial_level,
            promote_level: initial_promote,
            skill_map: skill_extra_charge_map,
            depot_map,
            skill_depot_id: avatar_config.skill_depot_id,
            born_time: time_util::unix_timestamp() as u32,
            guid: avatar_guid,
            equip_map,
            cur_hp: avatar_config.hp_base,
            wearing_flycloak_id: DEFAULT_FLYCLOAK_ID,
            costume_id: 0,
            trace_effect_id: 0,
            weapon_skin_id: 0,
            ..Default::default()
        },
    );
    Some(avatar_guid)
}

pub fn client_avatar_info(
    avatar_bin: &AvatarBin,
) -> Option<snezhnaya_gi_proto::normal::AvatarInfo> {
    use snezhnaya_gi_data::excel::{FetterDataConfig, FetterDataConfigKeyed};
    use snezhnaya_gi_entity::{
        common::{create_fight_props_with_equip, LifeState},
        int_prop_map,
    };
    use snezhnaya_gi_proto::normal::*;
    let avatar_excel_config_collection_clone = avatar_excel_config_collection::get();
    let fetter_data_entries_clone = FetterDataConfig::get_fetter_data_entries();

    let Some(skill_depot) = avatar_bin.depot_map.get(&avatar_bin.skill_depot_id) else {
        tracing::debug!(
            "skill_depot bin {} doesn't exist",
            avatar_bin.skill_depot_id
        );
        return None;
    };

    let mut fetter_data_list = vec![];

    if fetter_data_entries_clone.contains_key(&avatar_bin.avatar_id) {
        let Some(temp_fetter_data_list) = fetter_data_entries_clone
            .get(&avatar_bin.avatar_id)
            .cloned()
        else {
            tracing::debug!("fetter config {} doesn't exist", avatar_bin.avatar_id);
            return None;
        };
        fetter_data_list = temp_fetter_data_list;
    }

    let Some(avatar_config) = avatar_excel_config_collection_clone.get(&avatar_bin.avatar_id)
    else {
        tracing::debug!("avatar config {} doesn't exist", avatar_bin.avatar_id);
        return None;
    };

    let fetter = match avatar_bin.detail.as_ref() {
        Some(avatar_bin::Detail::FormalAvatar(formal)) => formal.fetter.as_ref(),
        _ => None,
    };
    Some(AvatarInfo {
        avatar_type: 1,
        avatar_id: avatar_bin.avatar_id,
        guid: avatar_bin.guid,
        equip_guid_list: avatar_bin
            .equip_map
            .iter()
            .map(|(_, item)| item.guid)
            .collect(),
        skill_depot_id: avatar_bin.skill_depot_id,
        talent_id_list: skill_depot.talent_id_list.clone(),
        core_proud_skill_level: skill_depot.core_proud_skill_level,
        born_time: avatar_bin.born_time,
        life_state: (avatar_bin.cur_hp > 0.0)
            .then_some(LifeState::Alive)
            .unwrap_or(LifeState::Dead) as u32,
        wearing_flycloak_id: avatar_bin.wearing_flycloak_id,
        costume_id: avatar_bin.costume_id,
        trace_effect_id: avatar_bin.trace_effect_id,
        fetter_info: Some(AvatarFetterInfo {
            exp_level: fetter.map_or(1, |f| f.level.max(1)),
            exp_number: fetter.map_or(0, |f| f.exp),
            rewarded_fetter_level_list: fetter
                .map_or(vec![], |f| f.rewarded_fetter_level_list.clone()),
            fetter_list: fetter_data_list
                .into_iter()
                .map(|x| FetterData {
                    fetter_state: 3,
                    fetter_id: x.fetter_id,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }),
        skill_level_map: skill_depot.skill_level_map.clone(),
        skill_map: avatar_bin
            .skill_map
            .iter()
            .map(|(k, v)| {
                (
                    *k,
                    AvatarSkillInfo {
                        max_charge_count: v.max_charge_count,
                        ..Default::default()
                    },
                )
            })
            .collect(),
        inherent_proud_skill_list: skill_depot.inherent_proud_skill_list.clone(),
        prop_map: int_prop_map! {
            PROP_LEVEL: avatar_bin.level;
            PROP_BREAK_LEVEL: avatar_bin.promote_level;
        },
        fight_prop_map: create_fight_props_with_equip(avatar_bin, avatar_config)
            .0
            .iter()
            .map(|(ty, val)| (*ty as u32, *val))
            .collect(),
        ..Default::default()
    })
}
