use bevy_ecs::prelude::*;
use snezhnaya_gi_data::{config, excel::*, prop_type::*};
use snezhnaya_gi_entity::{ability::Ability, avatar::*, common::*};
use snezhnaya_gi_inventory::transaction::{
    add_amount, notify_transaction, transact, Amounts, InventoryResult,
};
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::{
    normal::*,
    retcode::Retcode,
    server_only::{avatar_bin, AvatarBin, PlayerDataBin},
};

fn avatar(player: &PlayerDataBin, guid: u64) -> InventoryResult<AvatarBin> {
    let avatar = player
        .avatar_bin
        .as_ref()
        .and_then(|bin| bin.avatar_map.get(&guid))
        .ok_or(Retcode::RetCanNotFindAvatar)?;
    if !matches!(avatar.detail, Some(avatar_bin::Detail::FormalAvatar(_))) {
        return Err(Retcode::RetAvatarCanNotUse);
    }
    if !avatar_excel_config_collection::get().contains_key(&avatar.avatar_id) {
        return Err(Retcode::RetNotFoundConfig);
    }
    if !avatar.depot_map.contains_key(&avatar.skill_depot_id)
        || !avatar_skill_depot_excel_config_collection::get().contains_key(&avatar.skill_depot_id)
    {
        return Err(Retcode::RetSkillDepotNotFound);
    }
    Ok(avatar.clone())
}

fn promotion(avatar: &AvatarBin, level: u32) -> InventoryResult<&'static AvatarPromoteExcelConfig> {
    let row = avatar_excel_config_collection::get()
        .get(&avatar.avatar_id)
        .ok_or(Retcode::RetNotFoundConfig)?;
    avatar_promote_excel_config_collection::get()
        .get(&((row.avatar_promote_id << 8) | level))
        .ok_or(Retcode::RetAvatarOnMaxBreakLevel)
}

// 先验证角色和消耗，背包事务成功后再提交角色副本。
fn commit(
    player: &mut PlayerDataBin,
    avatar: AvatarBin,
    costs: &Amounts,
    output: &MessageOutput,
) -> InventoryResult<()> {
    let change = transact(player, costs, &Amounts::new(), &[])?;
    player
        .avatar_bin
        .as_mut()
        .unwrap()
        .avatar_map
        .insert(avatar.guid, avatar);
    notify_transaction(output, player, &change);
    Ok(())
}

fn upgrade(
    player: &mut PlayerDataBin,
    request: &AvatarUpgradeReq,
    output: &MessageOutput,
) -> InventoryResult<AvatarUpgradeRsp> {
    let mut avatar = avatar(player, request.avatar_guid)?;
    let limit = promotion(&avatar, avatar.promote_level)?.unlock_max_level;
    if avatar.level == 0 || avatar.level >= limit {
        return Err(Retcode::RetAvatarLimitLevelError);
    }
    if request.item_param_list.is_empty() || request.item_param_list.len() > 100 {
        return Err(Retcode::RetItemInvalidUseCount);
    }
    let mut costs = Amounts::new();
    let mut experience = 0u32;
    for item in &request.item_param_list {
        if item.count == 0 {
            return Err(Retcode::RetItemInvalidUseCount);
        }
        let material = material_excel_config_collection::get()
            .get(&item.item_id)
            .filter(|row| {
                row.material_type == snezhnaya_gi_data::excel::common::MaterialType::ExpFruit
            })
            .ok_or(Retcode::RetItemInvalidTarget)?;
        let exp = material
            .item_use
            .iter()
            .find(|op| op.use_op == ItemUseOp::AddExp)
            .and_then(|op| op.use_param.first())
            .and_then(|value| value.as_str().parse::<u32>().ok())
            .filter(|value| *value > 0)
            .ok_or(Retcode::RetNotFoundConfig)?;
        experience = exp
            .checked_mul(item.count)
            .and_then(|value| experience.checked_add(value))
            .ok_or(Retcode::RetItemExceedLimit)?;
        add_amount(&mut costs, item.item_id, item.count)?;
    }
    add_amount(&mut costs, 202, experience / 5)?;
    let old_level = avatar.level;
    let Some(avatar_bin::Detail::FormalAvatar(formal)) = avatar.detail.as_mut() else {
        unreachable!()
    };
    let mut total = u64::from(formal.exp) + u64::from(experience);
    while avatar.level < limit {
        let required = avatar_level_excel_config_collection::get()
            .get(&avatar.level)
            .map(|row| row.exp)
            .filter(|exp| *exp > 0)
            .ok_or(Retcode::RetNotFoundConfig)?;
        if total < u64::from(required) {
            break;
        }
        total -= u64::from(required);
        avatar.level += 1;
    }
    formal.exp = if avatar.level >= limit {
        0
    } else {
        u32::try_from(total).map_err(|_| Retcode::RetItemExceedLimit)?
    };
    let response = AvatarUpgradeRsp {
        avatar_guid: avatar.guid,
        old_level,
        cur_level: avatar.level,
        retcode: 0,
    };
    commit(player, avatar, &costs, output)?;
    Ok(response)
}

fn promote(
    player: &mut PlayerDataBin,
    request: &AvatarPromoteReq,
    output: &MessageOutput,
) -> InventoryResult<AvatarPromoteRsp> {
    let mut avatar = avatar(player, request.guid)?;
    let current = promotion(&avatar, avatar.promote_level)?;
    let next_level = avatar
        .promote_level
        .checked_add(1)
        .ok_or(Retcode::RetAvatarOnMaxBreakLevel)?;
    let next = promotion(&avatar, next_level)?;
    if avatar.level < current.unlock_max_level {
        return Err(Retcode::RetAvatarLevelLessThan);
    }
    if player.basic_bin.as_ref().map_or(0, |bin| bin.level) < next.required_player_level {
        return Err(Retcode::RetPlayerLevelLessThan);
    }
    let depot = avatar_skill_depot_excel_config_collection::get()
        .get(&avatar.skill_depot_id)
        .ok_or(Retcode::RetSkillDepotNotFound)?;
    let skills = avatar
        .depot_map
        .get_mut(&avatar.skill_depot_id)
        .ok_or(Retcode::RetSkillDepotNotFound)?;
    for skill in &depot.inherent_proud_skill_opens {
        let id = skill.proud_skill_group_id * 100 + 1;
        if skill.proud_skill_group_id != 0
            && skill.need_avatar_promote_level <= next_level
            && !skills.inherent_proud_skill_list.contains(&id)
        {
            if !proud_skill_excel_config_collection::get().contains_key(&id) {
                return Err(Retcode::RetNotFoundConfig);
            }
            skills.inherent_proud_skill_list.push(id);
        }
    }
    let mut costs = Amounts::new();
    add_amount(&mut costs, 202, next.scoin_cost)?;
    for item in &next.cost_items {
        add_amount(&mut costs, item.id, item.count)?;
    }
    avatar.promote_level = next_level;
    commit(player, avatar, &costs, output)?;
    Ok(AvatarPromoteRsp {
        guid: request.guid,
        retcode: 0,
    })
}

fn upgrade_skill(
    player: &mut PlayerDataBin,
    request: &AvatarSkillUpgradeReq,
    output: &MessageOutput,
) -> InventoryResult<AvatarSkillUpgradeRsp> {
    let mut avatar = avatar(player, request.avatar_guid)?;
    let depot = avatar_skill_depot_excel_config_collection::get()
        .get(&avatar.skill_depot_id)
        .ok_or(Retcode::RetSkillDepotNotFound)?;
    if !depot.skills.contains(&request.avatar_skill_id)
        && depot.energy_skill != request.avatar_skill_id
    {
        return Err(Retcode::RetItemInvalidTarget);
    }
    let skills = avatar
        .depot_map
        .get_mut(&avatar.skill_depot_id)
        .ok_or(Retcode::RetSkillDepotNotFound)?;
    let old_level = *skills
        .skill_level_map
        .get(&request.avatar_skill_id)
        .ok_or(Retcode::RetPrevProudSkillNotGet)?;
    if old_level != request.old_level {
        return Err(Retcode::RetPrevProudSkillNotGet);
    }
    let next_level = old_level
        .checked_add(1)
        .ok_or(Retcode::RetProudSkillMaxLevel)?;
    let skill = avatar_skill_excel_config_collection::get()
        .get(&request.avatar_skill_id)
        .filter(|skill| skill.proud_skill_group_id != 0)
        .ok_or(Retcode::RetNotFoundConfig)?;
    let row = proud_skill_excel_config_collection::get()
        .get(&(skill.proud_skill_group_id * 100 + next_level))
        .filter(|row| {
            row.level == next_level
                && (!row.cost_items.iter().all(|item| item.count == 0) || row.coin_cost > 0)
        })
        .ok_or(Retcode::RetProudSkillMaxLevel)?;
    if avatar.promote_level < row.break_level {
        return Err(Retcode::RetAvatarBreakLevelLessThan);
    }
    let mut costs = Amounts::new();
    add_amount(&mut costs, 202, row.coin_cost)?;
    for item in &row.cost_items {
        add_amount(&mut costs, item.id, item.count)?;
    }
    skills
        .skill_level_map
        .insert(request.avatar_skill_id, next_level);
    let response = AvatarSkillUpgradeRsp {
        avatar_guid: avatar.guid,
        avatar_skill_id: request.avatar_skill_id,
        old_level,
        cur_level: next_level,
        retcode: 0,
    };
    commit(player, avatar, &costs, output)?;
    Ok(response)
}

fn unlock_talent(
    player: &mut PlayerDataBin,
    request: &UnlockAvatarTalentReq,
    output: &MessageOutput,
) -> InventoryResult<UnlockAvatarTalentRsp> {
    let mut avatar = avatar(player, request.avatar_guid)?;
    let row = avatar_skill_depot_excel_config_collection::get()
        .get(&avatar.skill_depot_id)
        .ok_or(Retcode::RetSkillDepotNotFound)?;
    let skills = avatar
        .depot_map
        .get_mut(&avatar.skill_depot_id)
        .ok_or(Retcode::RetSkillDepotNotFound)?;
    if !row.talents.contains(&request.talent_id) {
        return Err(Retcode::RetItemInvalidTarget);
    }
    if skills.talent_id_list.contains(&request.talent_id) {
        return Err(Retcode::RetTalentAlreayUnlocked);
    }
    if row
        .talents
        .iter()
        .find(|id| !skills.talent_id_list.contains(id))
        != Some(&request.talent_id)
    {
        return Err(Retcode::RetPrevTalentNotUnlocked);
    }
    let talent = avatar_talent_excel_config_collection::get()
        .get(&request.talent_id)
        .ok_or(Retcode::RetNotFoundConfig)?;
    if talent.cost_item_id() == 0 || talent.main_cost_item_count == 0 {
        return Err(Retcode::RetNotFoundConfig);
    }
    let mut costs = Amounts::new();
    add_amount(
        &mut costs,
        talent.cost_item_id(),
        talent.main_cost_item_count,
    )?;
    skills.talent_id_list.push(request.talent_id);
    skills.core_proud_skill_level = skills.talent_id_list.len() as u32;
    commit(player, avatar, &costs, output)?;
    Ok(UnlockAvatarTalentRsp {
        avatar_guid: request.avatar_guid,
        talent_id: request.talent_id,
        retcode: 0,
    })
}

pub fn handle(
    mut requests: MessageReader<ClientMessageEvent>,
    mut players: ResMut<Players>,
    output: Res<MessageOutput>,
    mut commands: Commands,
    avatars: Query<(
        Entity,
        &Guid,
        &OwnerPlayerUID,
        &ProtocolEntityID,
        &FightProperties,
    )>,
) {
    for message in requests.read() {
        let uid = message.sender_uid();
        if !matches!(
            message.message_name(),
            "AvatarUpgradeReq"
                | "AvatarPromoteReq"
                | "AvatarSkillUpgradeReq"
                | "UnlockAvatarTalentReq"
        ) {
            continue;
        }
        let Some(player) = players.get_mut(uid) else {
            continue;
        };
        macro_rules! respond {
            ($request:ty, $response:ident, $name:literal, $function:ident, $guid:ident) => {{
                let Some(request) = message.decode::<$request>() else {
                    continue;
                };
                let guid = request.$guid;
                let response = match $function(player, &request, &output) {
                    Ok(response) => response,
                    Err(code) => {
                        output.send(
                            uid,
                            $name,
                            $response {
                                $guid: guid,
                                retcode: code.into(),
                                ..Default::default()
                            },
                        );
                        continue;
                    }
                };
                output.send(uid, $name, response);
                guid
            }};
        }
        let guid = match message.message_name() {
            "AvatarUpgradeReq" => respond!(
                AvatarUpgradeReq,
                AvatarUpgradeRsp,
                "AvatarUpgradeRsp",
                upgrade,
                avatar_guid
            ),
            "AvatarPromoteReq" => respond!(
                AvatarPromoteReq,
                AvatarPromoteRsp,
                "AvatarPromoteRsp",
                promote,
                guid
            ),
            "AvatarSkillUpgradeReq" => respond!(
                AvatarSkillUpgradeReq,
                AvatarSkillUpgradeRsp,
                "AvatarSkillUpgradeRsp",
                upgrade_skill,
                avatar_guid
            ),
            "UnlockAvatarTalentReq" => respond!(
                UnlockAvatarTalentReq,
                UnlockAvatarTalentRsp,
                "UnlockAvatarTalentRsp",
                unlock_talent,
                avatar_guid
            ),
            _ => unreachable!(),
        };
        let avatar = &player.avatar_bin.as_ref().unwrap().avatar_map[&guid];
        let skills = &avatar.depot_map[&avatar.skill_depot_id];
        let found = avatars
            .iter()
            .find(|(_, id, owner, ..)| id.0 == guid && owner.0 == uid);
        let entity_id = found.map_or(0, |item| item.3 .0);
        if let Some(fight_props) = refresh_avatar_fight_props(avatar, found.map(|item| item.4)) {
            if let Some((entity, ..)) = found {
                let mut open_configs = config::process_talent_ids(
                    &skills.talent_id_list,
                    avatar_talent_excel_config_collection::get(),
                );
                open_configs.extend(config::process_inherent_proud_skills(
                    &skills.inherent_proud_skill_list,
                    proud_skill_excel_config_collection::get(),
                ));
                commands.entity(entity).insert((
                    fight_props,
                    Level(avatar.level),
                    AvatarPromoteLevel(avatar.promote_level),
                    SkillLevelMap(skills.skill_level_map.clone()),
                    TalentIdList(skills.talent_id_list.clone()),
                    InherentProudSkillList(skills.inherent_proud_skill_list.clone()),
                    CoreProudSkillLevel(skills.core_proud_skill_level),
                    Ability::new_for_avatar(avatar.avatar_id, open_configs),
                ));
            } else {
                output.send(
                    uid,
                    "AvatarFightPropUpdateNotify",
                    AvatarFightPropUpdateNotify {
                        avatar_guid: guid,
                        fight_prop_map: fight_props
                            .0
                            .into_iter()
                            .map(|(ty, value)| (ty as u32, value))
                            .collect(),
                    },
                );
            }
        }
        let exp = match avatar.detail.as_ref() {
            Some(avatar_bin::Detail::FormalAvatar(formal)) => formal.exp,
            _ => 0,
        };
        output.send(
            uid,
            "AvatarPropNotify",
            AvatarPropNotify {
                avatar_guid: guid,
                prop_map: [
                    (PROP_LEVEL, i64::from(avatar.level)),
                    (PROP_EXP, i64::from(exp)),
                    (PROP_BREAK_LEVEL, i64::from(avatar.promote_level)),
                ]
                .into(),
            },
        );
        output.send(
            uid,
            "AvatarSkillDepotChangeNotify",
            AvatarSkillDepotChangeNotify {
                avatar_guid: guid,
                skill_depot_id: avatar.skill_depot_id,
                entity_id,
                skill_level_map: skills.skill_level_map.clone(),
                proud_skill_extra_level_map: config::proud_skill_extra_levels(
                    avatar.skill_depot_id,
                    &skills.talent_id_list,
                ),
                talent_id_list: skills.talent_id_list.clone(),
                proud_skill_list: skills.inherent_proud_skill_list.clone(),
                core_proud_skill_level: skills.core_proud_skill_level,
                ..Default::default()
            },
        );
        if message.message_name() == "AvatarSkillUpgradeReq" {
            let request = message.decode::<AvatarSkillUpgradeReq>().unwrap();
            output.send(
                uid,
                "AvatarSkillChangeNotify",
                AvatarSkillChangeNotify {
                    avatar_guid: guid,
                    entity_id,
                    skill_depot_id: avatar.skill_depot_id,
                    avatar_skill_id: request.avatar_skill_id,
                    old_level: request.old_level,
                    cur_level: skills.skill_level_map[&request.avatar_skill_id],
                },
            );
        } else if message.message_name() == "UnlockAvatarTalentReq" {
            let request = message.decode::<UnlockAvatarTalentReq>().unwrap();
            output.send(
                uid,
                "AvatarUnlockTalentNotify",
                AvatarUnlockTalentNotify {
                    avatar_guid: guid,
                    entity_id,
                    skill_depot_id: avatar.skill_depot_id,
                    talent_id: request.talent_id,
                },
            );
        }
    }
}

pub fn handle_rewards(
    mut events: MessageReader<ClientMessageEvent>,
    mut players: ResMut<Players>,
    out: Res<MessageOutput>,
) {
    for message in events.read() {
        if !matches!(
            message.message_name(),
            "AvatarFetterLevelRewardReq" | "AvatarPromoteGetRewardReq"
        ) {
            continue;
        }
        let Some(request) = message.decode_json() else {
            continue;
        };
        let guid = request
            .get("avatar_guid")
            .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
            .unwrap_or(0);
        let friendship = message.message_name() == "AvatarFetterLevelRewardReq";
        let field = if friendship {
            "fetter_level"
        } else {
            "promote_level"
        };
        let level = request
            .get(field)
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0);
        let Some(player) = players.get_mut(message.sender_uid()) else {
            continue;
        };
        let result = claim_avatar_reward(player, guid, level, friendship, &out);
        let mut response = serde_json::json!({"avatar_guid":guid.to_string(),"retcode":result.as_ref().err().map_or(0,|code|i32::from(*code)),"reward_id":result.as_ref().copied().unwrap_or(0)});
        response[field] = level.into();
        if friendship {
            response["reward_id_list"] =
                serde_json::json!(result.ok().into_iter().collect::<Vec<_>>());
        }
        out.send_json(
            player.uid,
            if friendship {
                "AvatarFetterLevelRewardRsp"
            } else {
                "AvatarPromoteGetRewardRsp"
            },
            response,
        );
    }
}
fn claim_avatar_reward(
    player: &mut PlayerDataBin,
    guid: u64,
    level: u32,
    friendship: bool,
    out: &MessageOutput,
) -> InventoryResult<u32> {
    let mut avatar = player
        .avatar_bin
        .as_ref()
        .and_then(|a| a.avatar_map.get(&guid))
        .cloned()
        .ok_or(Retcode::RetCanNotFindAvatar)?;
    let Some(avatar_bin::Detail::FormalAvatar(formal)) = &mut avatar.detail else {
        return Err(Retcode::RetAvatarCanNotUse);
    };
    let reward = if friendship {
        let fetter = formal
            .fetter
            .as_mut()
            .ok_or(Retcode::RetFetterRewardLevelNotEnough)?;
        if level > fetter.level {
            return Err(Retcode::RetFetterRewardLevelNotEnough);
        }
        if fetter.rewarded_fetter_level_list.contains(&level) {
            return Err(Retcode::RetFetterRewardAlreadyGot);
        }
        let reward =
            character_card_reward(avatar.avatar_id, level).ok_or(Retcode::RetItemInvalidTarget)?;
        fetter.rewarded_fetter_level_list.push(level);
        reward
    } else {
        if level > avatar.promote_level {
            return Err(Retcode::RetAvatarBreakLevelLessThan);
        }
        if formal.taken_promote_reward_level_list.contains(&level) {
            return Err(Retcode::RetRewardHasTaken);
        }
        let config = avatar_excel_config_collection::get()
            .get(&avatar.avatar_id)
            .ok_or(Retcode::RetNotFoundConfig)?;
        let index = config
            .avatar_promote_reward_level_list
            .iter()
            .position(|l| *l == level)
            .ok_or(Retcode::RetItemInvalidTarget)?;
        let reward = *config
            .avatar_promote_reward_id_list
            .get(index)
            .filter(|id| **id != 0)
            .ok_or(Retcode::RetNotFoundConfig)?;
        formal.taken_promote_reward_level_list.push(level);
        reward
    };
    let rewards = snezhnaya_gi_inventory::transaction::reward_items([reward])?;
    let change = transact(player, &Amounts::new(), &rewards, &[])?;
    player
        .avatar_bin
        .as_mut()
        .unwrap()
        .avatar_map
        .insert(guid, avatar);
    notify_transaction(out, player, &change);
    if friendship {
        let avatar = &player.avatar_bin.as_ref().unwrap().avatar_map[&guid];
        let info = crate::util::client_avatar_info(avatar)
            .and_then(|a| a.fetter_info)
            .unwrap_or_else(|| {
                let Some(avatar_bin::Detail::FormalAvatar(f)) = &avatar.detail else {
                    unreachable!()
                };
                let fetter = f.fetter.as_ref().unwrap();
                AvatarFetterInfo {
                    exp_level: fetter.level,
                    exp_number: fetter.exp,
                    rewarded_fetter_level_list: fetter.rewarded_fetter_level_list.clone(),
                    ..Default::default()
                }
            });
        out.send(
            player.uid,
            "AvatarFetterDataNotify",
            AvatarFetterDataNotify {
                fetter_info_map: std::collections::HashMap::from([(guid, info)]),
            },
        );
    }
    Ok(reward)
}
