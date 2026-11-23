use bevy_ecs::prelude::*;
use snezhnaya_gi_data::{
    custom,
    excel::{expedition::*, production::ProductionConfig},
};
use snezhnaya_gi_inventory::transaction::{self as inventory, Amounts};
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::{record_progress, Players};
use snezhnaya_gi_proto::{
    normal as n,
    retcode::Retcode,
    server_only::{self as b, avatar_bin},
};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

#[derive(Resource, Default)]
pub(super) struct ExpeditionWakeup(Option<Instant>);

pub fn next_wakeup(world: &World) -> Option<Instant> {
    world.get_resource::<ExpeditionWakeup>()?.0
}
fn now() -> u32 {
    common::time_util::unix_timestamp().min(u64::from(u32::MAX)) as u32
}
fn expedition(avatar: &b::AvatarBin) -> Option<&b::AvatarExpeditionBin> {
    let Some(avatar_bin::Detail::FormalAvatar(formal)) = &avatar.detail else {
        return None;
    };
    formal
        .expedition_data
        .as_ref()
        .filter(|e| matches!(e.state, 1..=3))
}
fn state_mut(avatar: &mut b::AvatarBin) -> Option<&mut Option<b::AvatarExpeditionBin>> {
    let Some(avatar_bin::Detail::FormalAvatar(formal)) = &mut avatar.detail else {
        return None;
    };
    Some(&mut formal.expedition_data)
}
fn finish_time(e: &b::AvatarExpeditionBin) -> u64 {
    // 时长采用开始派遣时保存的天赋加成。
    let ratio = if e.shorten_ratio.is_finite() {
        e.shorten_ratio.clamp(0.0, 0.99)
    } else {
        0.0
    };
    u64::from(e.start_time)
        + (f64::from(e.hour_time) * 3600.0 * (1.0 - f64::from(ratio))).ceil() as u64
}
fn info(player: &b::PlayerDataBin) -> HashMap<u64, n::AvatarExpeditionInfo> {
    player
        .avatar_bin
        .iter()
        .flat_map(|a| &a.avatar_map)
        .filter_map(|(&guid, a)| {
            let e = expedition(a)?;
            Some((
                guid,
                n::AvatarExpeditionInfo {
                    state: e.state as i32,
                    exp_id: e.exp_id,
                    hour_time: e.hour_time,
                    start_time: e.start_time,
                    shorten_ratio: e.shorten_ratio,
                },
            ))
        })
        .collect()
}
fn limit(player: &b::PlayerDataBin) -> usize {
    let initial = ProductionConfig::get()
        .constants
        .get("CONST_VALUE_EXPEDITION_INIT_LIMIT")
        .and_then(|v| v.first())
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(2);
    let level = player.basic_bin.as_ref().map_or(1, |b| b.level);
    ExpeditionConfig::get()
        .limit_additions
        .range(..=level)
        .fold(initial, |n, (_, add)| n.saturating_add(*add)) as usize
}
fn open(player: &b::PlayerDataBin, config: &Expedition) -> bool {
    config
        .open_condition_vec
        .iter()
        .all(|c| match c.kind.as_str() {
            "" => c.para == 0 || player.basic_bin.as_ref().is_some_and(|b| b.level >= c.para),
            "EXP_OPEN_COND_POINT" => player
                .scene_bin
                .as_ref()
                .and_then(|s| s.world.as_ref())
                .and_then(|w| w.scene_map.get(&c.para2))
                .is_none_or(|s| s.unlocked_point_list.contains(&c.para)),
            "EXP_OPEN_COND_QUEST" => player.quest_bin.as_ref().is_some_and(|q| {
                q.quest_bin
                    .as_ref()
                    .and_then(|q| q.quest_map.get(&c.para))
                    .is_some_and(|q| q.state == 3)
                    || q.parent_quest_bin
                        .as_ref()
                        .and_then(|q| q.parent_quest_map.get(&c.para))
                        .is_some_and(|q| q.total_finish_count > 0)
            }),
            _ => false,
        })
}
fn all(player: &b::PlayerDataBin, out: &MessageOutput) {
    out.send(
        player.uid,
        "AvatarExpeditionAllDataRsp",
        n::AvatarExpeditionAllDataRsp {
            expedition_info_map: info(player),
            expedition_count_limit: limit(player) as u32,
            open_expedition_list: ExpeditionConfig::get()
                .expeditions
                .values()
                .filter(|e| open(player, e))
                .map(|e| e.id)
                .collect(),
            ..Default::default()
        },
    );
}
fn notify(player: &b::PlayerDataBin, out: &MessageOutput) {
    out.send(
        player.uid,
        "AvatarExpeditionDataNotify",
        n::AvatarExpeditionDataNotify {
            expedition_info_map: info(player),
        },
    );
}
fn refresh(player: &mut b::PlayerDataBin, time: u32) -> bool {
    let mut changed = false;
    for a in player
        .avatar_bin
        .iter_mut()
        .flat_map(|a| a.avatar_map.values_mut())
    {
        if let Some(Some(e)) = state_mut(a) {
            if e.state == 1 && finish_time(e) <= u64::from(time) {
                e.state = 2;
                changed = true;
            }
        }
    }
    changed
}
pub(super) fn login(mut players: ResMut<Players>, out: Res<MessageOutput>) {
    for player in players.values_mut() {
        refresh(player, now());
        notify(player, &out);
    }
}
pub(super) fn update(
    mut players: ResMut<Players>,
    out: Res<MessageOutput>,
    mut wakeup: ResMut<ExpeditionWakeup>,
) {
    let time = now();
    let mut deadline = None;
    for player in players.values_mut() {
        if refresh(player, time) {
            notify(player, &out);
        }
        for e in player
            .avatar_bin
            .iter()
            .flat_map(|a| a.avatar_map.values())
            .filter_map(expedition)
            .filter(|e| e.state == 1)
        {
            let end = finish_time(e);
            deadline = Some(deadline.map_or(end, |old: u64| old.min(end)));
        }
    }
    wakeup.0 = deadline.and_then(|end| {
        Instant::now().checked_add(Duration::from_secs(end.saturating_sub(u64::from(time))))
    });
}
fn start(
    player: &mut b::PlayerDataBin,
    entries: &[n::AvatarExpeditionBasicInfo],
) -> Result<Vec<n::AvatarExpeditionBasicInfo>, Retcode> {
    if entries.is_empty() || entries.len() > limit(player) {
        return Err(Retcode::RetAvatarExpeditionCountLimit);
    }
    let avatars = player
        .avatar_bin
        .as_ref()
        .ok_or(Retcode::RetCanNotFindAvatar)?;
    let active: Vec<_> = avatars.avatar_map.values().filter_map(expedition).collect();
    if active.len() + entries.len() > limit(player) {
        return Err(Retcode::RetAvatarExpeditionCountLimit);
    }
    let mut used: HashSet<_> = active.iter().map(|e| e.exp_id).collect();
    let mut guids = HashSet::new();
    let mut result = Vec::new();
    let mut bonuses = HashMap::new();
    for entry in entries {
        if !guids.insert(entry.avatar_guid) {
            return Err(Retcode::RetDuplicateAvatar);
        }
        let avatar = avatars
            .avatar_map
            .get(&entry.avatar_guid)
            .ok_or(Retcode::RetCanNotFindAvatar)?;
        if !matches!(avatar.detail, Some(avatar_bin::Detail::FormalAvatar(_))) {
            return Err(Retcode::RetAvatarExpeditionTrialForbid);
        }
        if matches!(avatar.avatar_id, 10000005 | 10000007) {
            return Err(Retcode::RetAvatarExpeditionMainForbid);
        }
        if avatar.life_state != 1 {
            return Err(Retcode::RetAvatarExpeditionAvatarDie);
        }
        if expedition(avatar).is_some() || !used.insert(entry.exp_id) {
            return Err(Retcode::RetTeamAvatarInExpedition);
        }
        let config = ExpeditionConfig::get()
            .expeditions
            .get(&entry.exp_id)
            .ok_or(Retcode::RetNotFoundConfig)?;
        if !open(player, config) {
            return Err(Retcode::RetOpenStateNotOpen);
        }
        if !config
            .time_reward_vec
            .iter()
            .any(|r| r.htime == entry.hour_time)
        {
            return Err(Retcode::RetNotFoundConfig);
        }
        let bonus = ExpeditionConfig::get().bonuses(
            avatar.skill_depot_id,
            avatar.promote_level,
            entry.exp_id,
            entry.hour_time,
        );
        bonuses.insert(entry.avatar_guid, bonus);
        result.push(n::AvatarExpeditionBasicInfo {
            shorten_ratio: bonus.shorten_ratio,
            ..*entry
        });
    }
    let time = now();
    let avatars = player.avatar_bin.as_mut().unwrap();
    for entry in &result {
        let bonus = bonuses[&entry.avatar_guid];
        *state_mut(avatars.avatar_map.get_mut(&entry.avatar_guid).unwrap()).unwrap() =
            Some(b::AvatarExpeditionBin {
                state: 1,
                exp_id: entry.exp_id,
                hour_time: entry.hour_time,
                start_time: time,
                shorten_ratio: bonus.shorten_ratio,
                extra_output_ratio: Some(bonus.extra_ratio),
                extra_output_probability: bonus.extra_probability,
                ..Default::default()
            });
    }
    avatars.total_expedition_num = avatars
        .total_expedition_num
        .saturating_add(result.len() as u32);
    record_progress(player, "expedition_start", 0, result.len() as u32);
    Ok(result)
}
fn recall(player: &mut b::PlayerDataBin, guids: &[u64]) -> Result<(), Retcode> {
    if guids.is_empty() {
        return Err(Retcode::RetFail);
    }
    let avatars = player
        .avatar_bin
        .as_ref()
        .ok_or(Retcode::RetCanNotFindAvatar)?;
    if guids.len() > avatars.avatar_map.len() {
        return Err(Retcode::RetFail);
    }
    let mut seen = HashSet::new();
    for guid in guids {
        if !seen.insert(*guid) {
            return Err(Retcode::RetDuplicateAvatar);
        }
        let a = avatars
            .avatar_map
            .get(guid)
            .ok_or(Retcode::RetCanNotFindAvatar)?;
        if expedition(a).is_none() {
            return Err(Retcode::RetFail);
        }
    }
    for guid in guids {
        *state_mut(
            player
                .avatar_bin
                .as_mut()
                .unwrap()
                .avatar_map
                .get_mut(guid)
                .unwrap(),
        )
        .unwrap() = None;
    }
    Ok(())
}
fn claim(
    player: &mut b::PlayerDataBin,
    request: &n::AvatarExpeditionGetRewardReq,
    out: &MessageOutput,
) -> Result<Vec<n::AvatarExpeditionRewardInfo>, Retcode> {
    let mut entries: Vec<_> = player
        .avatar_bin
        .iter()
        .flat_map(|a| &a.avatar_map)
        .filter_map(|(&guid, a)| expedition(a).map(|e| (guid, e)))
        .filter(|(guid, e)| e.state == 2 && (request.is_claim_all || *guid == request.avatar_guid))
        .collect();
    entries.sort_by_key(|(guid, _)| *guid);
    if entries.is_empty() {
        return Err(Retcode::RetAvatarExpeditionNoAvatarCanTakeReward);
    }
    let mut rewards = Amounts::new();
    let mut result = Vec::new();
    for (guid, e) in entries {
        let recipe = ExpeditionConfig::get()
            .expeditions
            .get(&e.exp_id)
            .and_then(|r| r.time_reward_vec.iter().find(|r| r.htime == e.hour_time))
            .ok_or(Retcode::RetNotFoundConfig)?;
        let drops = if custom::drop_table_excel_config_collection::get()
            .contains_key(&recipe.reward_drop_id)
            || custom::drop_sub_table_excel_config_collection::get()
                .contains_key(&recipe.reward_drop_id)
        {
            custom::resolve_drop(recipe.reward_drop_id, 1)
        } else {
            // 部分地点没有掉落表，按对应奖励预览的数量区间结算。
            recipe.roll_preview()
        };
        let mut rolled = Amounts::new();
        for (id, count) in drops {
            inventory::add_amount(&mut rolled, id, count)?;
            inventory::add_amount(&mut rewards, id, count)?;
        }
        if rolled.is_empty() {
            return Err(Retcode::RetNotFoundConfig);
        }
        let bonus = if let Some(ratio) = e.extra_output_ratio {
            ExpeditionBonus {
                extra_ratio: ratio,
                extra_probability: e.extra_output_probability,
                ..Default::default()
            }
        } else {
            let a = &player.avatar_bin.as_ref().unwrap().avatar_map[&guid];
            ExpeditionConfig::get().bonuses(
                a.skill_depot_id,
                a.promote_level,
                e.exp_id,
                e.hour_time,
            )
        };
        let mut extra = Amounts::new();
        for (id, count) in bonus.roll_extra(
            &rolled
                .iter()
                .map(|(&id, &count)| (id, count))
                .collect::<Vec<_>>(),
        ) {
            inventory::add_amount(&mut extra, id, count)?;
            inventory::add_amount(&mut rewards, id, count)?;
        }
        result.push(n::AvatarExpeditionRewardInfo {
            basic_info: Some(n::AvatarExpeditionBasicInfo {
                avatar_guid: guid,
                exp_id: e.exp_id,
                hour_time: e.hour_time,
                shorten_ratio: e.shorten_ratio,
            }),
            item_list: rolled
                .into_iter()
                .map(|(item_id, count)| n::ItemParam { item_id, count })
                .collect(),
            extra_item_list: extra
                .into_iter()
                .map(|(item_id, count)| n::ItemParam { item_id, count })
                .collect(),
            ..Default::default()
        });
    }
    // 先整批入包，背包满或奖励配置异常时保留所有派遣。
    let change = inventory::transact(player, &Amounts::new(), &rewards, &[])?;
    for reward in &result {
        let guid = reward.basic_info.as_ref().unwrap().avatar_guid;
        *state_mut(
            player
                .avatar_bin
                .as_mut()
                .unwrap()
                .avatar_map
                .get_mut(&guid)
                .unwrap(),
        )
        .unwrap() = None;
    }
    record_progress(player, "expedition_complete", 0, result.len() as u32);
    inventory::notify_transaction(out, player, &change);
    Ok(result)
}
pub(super) fn handle(
    mut events: MessageReader<ClientMessageEvent>,
    mut players: ResMut<Players>,
    out: Res<MessageOutput>,
) {
    for message in events.read() {
        if !matches!(
            message.message_name(),
            "AvatarExpeditionAllDataReq"
                | "AvatarExpeditionStartReq"
                | "AvatarExpeditionCallBackReq"
                | "AvatarExpeditionGetRewardReq"
        ) {
            continue;
        }
        let Some(player) = players.get_mut(message.sender_uid()) else {
            continue;
        };
        if refresh(player, now()) {
            notify(player, &out);
        }
        match message.message_name() {
            "AvatarExpeditionAllDataReq" => all(player, &out),
            "AvatarExpeditionStartReq" => {
                let Some(req) = message.decode::<n::AvatarExpeditionStartReq>() else {
                    continue;
                };
                let result = start(player, &req.basic_info_list);
                let retcode = result.as_ref().err().copied().unwrap_or(Retcode::RetSucc) as i32;
                out.send(
                    player.uid,
                    "AvatarExpeditionStartRsp",
                    n::AvatarExpeditionStartRsp {
                        retcode,
                        expedition_basic_info_list: result.unwrap_or_default(),
                        expedition_info_map: info(player),
                        ..Default::default()
                    },
                );
                if retcode == 0 {
                    notify(player, &out);
                }
            }
            "AvatarExpeditionCallBackReq" => {
                let Some(req) = message.decode::<n::AvatarExpeditionCallBackReq>() else {
                    continue;
                };
                let retcode = recall(player, &req.avatar_guid)
                    .err()
                    .unwrap_or(Retcode::RetSucc) as i32;
                out.send(
                    player.uid,
                    "AvatarExpeditionCallBackRsp",
                    n::AvatarExpeditionCallBackRsp {
                        retcode,
                        expedition_info_map: info(player),
                    },
                );
                if retcode == 0 {
                    notify(player, &out);
                }
            }
            "AvatarExpeditionGetRewardReq" => {
                let Some(req) = message.decode::<n::AvatarExpeditionGetRewardReq>() else {
                    continue;
                };
                let result = claim(player, &req, &out);
                let retcode = result.as_ref().err().copied().unwrap_or(Retcode::RetSucc) as i32;
                out.send(
                    player.uid,
                    "AvatarExpeditionGetRewardRsp",
                    n::AvatarExpeditionGetRewardRsp {
                        retcode,
                        expedition_reward_list: result.unwrap_or_default(),
                        expedition_info_map: info(player),
                        ..Default::default()
                    },
                );
                if retcode == 0 {
                    notify(player, &out);
                }
            }
            _ => (),
        }
    }
}
