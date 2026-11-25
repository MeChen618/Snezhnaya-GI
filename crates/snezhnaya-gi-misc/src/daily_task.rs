use bevy_ecs::prelude::*;
use common::time_util::cn_game_day as day;
use serde_json::{json, Value};
use snezhnaya_gi_data::{
    custom,
    excel::{daily_task::*, production::ProductionConfig},
    scene::EventType,
};
use snezhnaya_gi_entity::{common::ProtocolEntityID, gadget::GadgetID};
use snezhnaya_gi_event::{
    lua::{ChallengeFinishEvent, DailyTaskGroupsRefreshEvent, LuaTriggerEvent, MonsterKillEvent},
    quest::{DailyTaskFinishEvent, QuestAcceptCondEvent, QuestAcceptEvent},
    scene::WorldOwnerUID,
};
use snezhnaya_gi_inventory::{
    attendance,
    transaction::{self as inventory, Amounts},
};
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::{record_progress, Players};
use snezhnaya_gi_proto::{retcode::Retcode, server_only as b};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

const TASK_COUNT: u32 = attendance::MAX_REWARDS;
const UNLOCK_LEVEL: u32 = 12;

#[derive(Resource, Default)]
pub(super) struct DailyWakeup(Option<Instant>);
pub fn next_wakeup(world: &World) -> Option<Instant> {
    world.get_resource::<DailyWakeup>()?.0
}
fn now() -> u32 {
    common::time_util::unix_timestamp().min(u64::from(u32::MAX)) as u32
}
fn unlocked(player: &b::PlayerDataBin) -> bool {
    player
        .basic_bin
        .as_ref()
        .is_some_and(|b| b.level >= UNLOCK_LEVEL)
}
fn task_info(task: &b::DailyTaskInfoBin) -> Value {
    let total = DailyTaskConfig::get()
        .tasks
        .get(&task.daily_task_id)
        .map_or(1, |r| r.finish_progress.max(1));
    json!({"daily_task_id":task.daily_task_id,"progress":task.progress,"finish_progress":total,"is_finished":task.is_finished,"reward_id":task.preview_reward_id})
}
fn score_level(player: &b::PlayerDataBin, state: &b::PlayerDailyTaskCompBin) -> u32 {
    if state.score_player_level > 0 {
        state.score_player_level
    } else {
        player.basic_bin.as_ref().map_or(0, |basic| basic.level)
    }
}
fn notify(player: &b::PlayerDataBin, out: &MessageOutput) {
    if !unlocked(player) {
        return;
    }
    let Some(state) = &player.daily_task_bin else {
        return;
    };
    let reward = DailyTaskConfig::get()
        .level(score_level(player, state))
        .map_or(0, |r| r.score_preview_reward_id);
    let finished = attendance::finished_count(player, now());
    out.send_json(player.uid, "DailyTaskDataNotify", json!({
        "finished_num":finished,"is_taken_score_reward":state.is_taken_score_reward,"score_reward_id":reward,
    }));
    if state.last_time != 0 && !state.is_taken_score_reward && finished >= TASK_COUNT {
        out.send_json(
            player.uid,
            "DailyTaskScoreRewardNotify",
            json!({"reward_id":reward}),
        );
    }
}
fn notify_cities(uid: u32, out: &MessageOutput) {
    let cities: Vec<_> = DailyTaskConfig::get().cities.keys().copied().collect();
    out.send_json(
        uid,
        "DailyTaskUnlockedCitiesNotify",
        json!({"unlocked_city_list": cities}),
    );
}
fn notify_world(players: &Players, owner: u32, out: &MessageOutput) {
    let Some(state) = players.get(owner).and_then(|p| p.daily_task_bin.as_ref()) else {
        return;
    };
    let packet = json!({"filter_city_id":state.filter_city_id,"task_list":state.task_list.iter().map(task_info).collect::<Vec<_>>(),"finished_daily_task_num":state.task_list.iter().filter(|t|t.is_finished).count()});
    for uid in players.keys() {
        out.send_json(*uid, "WorldOwnerDailyTaskNotify", packet.clone());
    }
}
fn groups(cfg: &DailyTaskConfig, tasks: &[b::DailyTaskInfoBin]) -> Vec<u32> {
    tasks
        .iter()
        .filter_map(|t| cfg.tasks.get(&t.daily_task_id))
        .flat_map(|t| t.new_group_vec.iter().copied())
        .collect()
}
fn refresh(player: &mut b::PlayerDataBin, time: u32, cfg: &DailyTaskConfig) -> Option<Vec<u32>> {
    if !unlocked(player) {
        return None;
    }
    let state = player
        .daily_task_bin
        .get_or_insert_with(|| b::PlayerDailyTaskCompBin {
            filter_city_id: 1,
            ..Default::default()
        });
    if state.last_time != 0 && day(state.last_time) == day(time) && !state.task_list.is_empty() {
        return None;
    }
    let level = player.basic_bin.as_ref()?.level;
    let assignment = cfg
        .draw(state.filter_city_id, player.uid, u64::from(day(time)))
        .and_then(|(city, ids)| {
            let tasks = ids
                .into_iter()
                .map(|id| {
                    let reward = cfg.reward(&cfg.tasks[&id], level)?;
                    Some(b::DailyTaskInfoBin {
                        daily_task_id: id,
                        drop_id: reward.drop_id,
                        preview_reward_id: reward.preview_reward_id,
                        ..Default::default()
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some((city, tasks))
        });
    if assignment.is_none() && state.last_time == 0 && state.task_list.is_empty() {
        return None;
    }
    let assigned = assignment.is_some();
    let (city, tasks) = assignment.unwrap_or_default();
    let mut changed = groups(cfg, &state.task_list);
    changed.extend(groups(cfg, &tasks));
    changed.sort_unstable();
    changed.dedup();
    // 到期的待发奖励保留，背包满和重登都不会吞掉已完成的委托奖励。
    state.pending_task_rewards.extend(
        std::mem::take(&mut state.task_list)
            .into_iter()
            .filter(|t| !t.pending_reward_map.is_empty()),
    );
    state.task_list = tasks;
    state.last_time = if assigned { time } else { 0 };
    state.score_player_level = if assigned { level } else { 0 };
    state.is_taken_score_reward = false;
    state.finished_num = 0;
    state.random_city_id = city;
    if assigned {
        if state.filter_city_id != 0 {
            state.filter_city_id = city;
        }
        state.refresh_times = state.refresh_times.saturating_add(1);
    }
    Some(changed)
}
fn refresh_all(
    players: &mut Players,
    owner: u32,
    time: u32,
    out: &MessageOutput,
    changes: &mut MessageWriter<DailyTaskGroupsRefreshEvent>,
) {
    let mut world_changed = false;
    for player in players.values_mut() {
        if attendance::refresh(player, time) {
            attendance::notify(player, out);
        }
        if let Some(group_ids) = refresh(player, time, DailyTaskConfig::get()) {
            if player.uid == owner {
                changes.write(DailyTaskGroupsRefreshEvent {
                    owner_uid: owner,
                    group_ids,
                });
                world_changed = true;
            }
            notify_cities(player.uid, out);
            notify(player, out);
        }
    }
    if world_changed {
        notify_world(players, owner, out);
    }
}
pub(super) fn login(
    mut players: ResMut<Players>,
    owner: Res<WorldOwnerUID>,
    out: Res<MessageOutput>,
    mut accepts: MessageWriter<QuestAcceptEvent>,
) {
    let time = now();
    for player in players.values_mut() {
        if attendance::unlocked(player) {
            attendance::refresh(player, time);
            attendance::notify(player, &out);
        }
        if unlocked(player) {
            refresh(player, time, DailyTaskConfig::get());
            if let Some(state) = &player.daily_task_bin {
                let task_vars: Vec<_> = state
                    .task_var_map
                    .iter()
                    .map(|(&key, values)| json!({"key":key,"value_list":values.int32_list}))
                    .collect();
                out.send_json(
                    player.uid,
                    "TaskVarNotify",
                    json!({"task_var_list": task_vars}),
                );
                for task in state.task_list.iter().filter(|t| !t.is_finished) {
                    if let Some(config) = DailyTaskConfig::get()
                        .tasks
                        .get(&task.daily_task_id)
                        .filter(|t| t.quest_id != 0)
                    {
                        if !player
                            .quest_bin
                            .as_ref()
                            .and_then(|q| q.quest_bin.as_ref())
                            .is_some_and(|q| q.quest_map.contains_key(&config.quest_id))
                        {
                            accepts.write(QuestAcceptEvent(player.uid, config.quest_id));
                        }
                    }
                }
            }
            notify_cities(player.uid, &out);
            notify(player, &out);
        }
    }
    notify_world(&players, owner.0, &out);
}
pub(super) fn update(
    mut players: ResMut<Players>,
    owner: Res<WorldOwnerUID>,
    out: Res<MessageOutput>,
    mut changes: MessageWriter<DailyTaskGroupsRefreshEvent>,
    mut wake: ResMut<DailyWakeup>,
) {
    let time = now();
    refresh_all(&mut players, owner.0, time, &out, &mut changes);
    wake.0 = players.values_mut().any(|p| unlocked(p) || attendance::unlocked(p)).then(|| {
        let mut next = (u64::from(day(time)) + 1) * 86400 - 14400;
        if time < attendance::ACCUMULATED_RESET_TIME {
            next = next.min(u64::from(attendance::ACCUMULATED_RESET_TIME));
        }
        Instant::now() + Duration::from_secs(next.saturating_sub(u64::from(time)))
    });
}
fn roll(drop_id: u32) -> Result<Amounts, Retcode> {
    if !custom::drop_table_excel_config_collection::get().contains_key(&drop_id)
        && !custom::drop_sub_table_excel_config_collection::get().contains_key(&drop_id)
    {
        return Err(Retcode::RetNotFoundConfig);
    }
    let mut items = Amounts::new();
    for (id, count) in custom::resolve_drop(drop_id, 1) {
        inventory::add_amount(&mut items, id, count)?;
    }
    if items.is_empty() {
        return Err(Retcode::RetNotFoundConfig);
    }
    Ok(items)
}
fn boost_fetter(items: &mut Amounts, coop: bool) -> Result<(), Retcode> {
    if coop {
        let ratio = ProductionConfig::get()
            .constants
            .get("CONST_VALUE_MULTI_PLAY_FETTER_EXP_RATIO")
            .and_then(|r| r.first())
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(2);
        if let Some(count) = items.get_mut(&105) {
            *count = count
                .checked_mul(ratio)
                .ok_or(Retcode::RetItemExceedLimit)?;
        }
    }
    Ok(())
}
fn claim_score(
    player: &mut b::PlayerDataBin,
    coop: bool,
    out: &MessageOutput,
) -> Result<Amounts, Retcode> {
    let state = player
        .daily_task_bin
        .as_ref()
        .ok_or(Retcode::RetDailyTaskNotFinish)?;
    if !unlocked(player) {
        return Err(Retcode::RetPlayerLevelLessThan);
    }
    if state.last_time == 0 || attendance::finished_count(player, now()) < TASK_COUNT {
        return Err(Retcode::RetDailyTaskNotFinish);
    }
    if state.is_taken_score_reward {
        return Err(Retcode::RetRewardHasTaken);
    }
    let level = DailyTaskConfig::get()
        .level(score_level(player, state))
        .ok_or(Retcode::RetNotFoundConfig)?;
    let mut rewards = roll(level.score_drop_id)?;
    boost_fetter(&mut rewards, coop)?;
    let change = inventory::transact(player, &Amounts::new(), &rewards, &[])?;
    player
        .daily_task_bin
        .as_mut()
        .unwrap()
        .is_taken_score_reward = true;
    record_progress(player, "daily_score", 0, 1);
    inventory::notify_transaction(out, player, &change);
    Ok(rewards)
}

fn claim_attendance(
    player: &mut b::PlayerDataBin,
    time: u32,
    out: &MessageOutput,
) -> Result<(u32, Amounts), Retcode> {
    if !attendance::unlocked(player) {
        return Err(Retcode::RetDailyAttendanceNotOpen);
    }
    let count = attendance::claimable(player, time);
    if count == 0 {
        return Err(Retcode::RetDailyAttendanceRewardMax);
    }
    let level = player.basic_bin.as_ref().map_or(0, |basic| basic.level);
    let reward = DailyTaskConfig::get()
        .reward_at_level(attendance::REWARD_GROUP, level)
        .ok_or(Retcode::RetNotFoundConfig)?;
    let mut rewards = Amounts::new();
    for _ in 0..count {
        for (id, amount) in roll(reward.drop_id)? {
            inventory::add_amount(&mut rewards, id, amount)?;
        }
    }
    let change = inventory::transact(player, &Amounts::new(), &rewards, &[])?;
    player.daily_attendance_bin.as_mut().unwrap().claimed_count += count;
    record_progress(player, "daily_task", 0, count);
    inventory::notify_transaction(out, player, &change);
    Ok((count, rewards))
}

pub(super) fn handle(
    mut messages: MessageReader<ClientMessageEvent>,
    mut players: ResMut<Players>,
    owner: Res<WorldOwnerUID>,
    out: Res<MessageOutput>,
    mut changes: MessageWriter<DailyTaskGroupsRefreshEvent>,
    mut quest_conditions: MessageWriter<QuestAcceptCondEvent>,
) {
    for message in messages.read() {
        let name = message.message_name();
        if !matches!(
            name,
            "DailyTaskFilterCityReq" | "TakeDailyTaskScoreRewardReq" | "DailyAttendanceTakeRewardReq"
        ) {
            continue;
        }
        let Some(req) = message.decode_json() else {
            continue;
        };
        let time = now();
        refresh_all(&mut players, owner.0, time, &out, &mut changes);
        let coop = players.keys().len() > 1;
        let Some(player) = players.get_mut(message.sender_uid()) else {
            continue;
        };
        if name == "DailyAttendanceTakeRewardReq" {
            let result = claim_attendance(player, time, &out);
            let code = result.as_ref().err().copied().unwrap_or(Retcode::RetSucc);
            if result.is_ok() {
                attendance::notify(player, &out);
                notify(player, &out);
                if attendance::finished_count(player, time) == TASK_COUNT {
                    quest_conditions.write(QuestAcceptCondEvent {
                        player_uid: player.uid,
                        cond_type: snezhnaya_gi_data::quest::quest_config::QuestCond::DailyTaskRewardCanGet,
                        param: 0,
                    });
                }
            }
            let (count, items) = result.unwrap_or_default();
            out.send_json(
                player.uid,
                "DailyAttendanceTakeRewardRsp",
                json!({"retcode":i32::from(code),"claimed_daily_task_count":count,
                    "item_list":items.into_iter().map(|(item_id,count)|json!({"item_id":item_id,"count":count})).collect::<Vec<_>>() }),
            );
        } else if name == "DailyTaskFilterCityReq" {
            let city = req.get("city_id").and_then(Value::as_u64).unwrap_or(0) as u32;
            let code = if !unlocked(player) {
                Retcode::RetPlayerLevelLessThan
            } else if city != 0 && !DailyTaskConfig::get().cities.contains_key(&city) {
                Retcode::RetDailyTaskFilterCityNotOpen
            } else {
                player.daily_task_bin.as_mut().unwrap().filter_city_id = city;
                Retcode::RetSucc
            };
            out.send_json(
                player.uid,
                "DailyTaskFilterCityRsp",
                json!({"retcode":i32::from(code),"city_id":city}),
            );
        } else {
            let attendance = req
                .get("is_claim_daily_attendance")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let result = claim_score(player, coop, &out);
            if result.is_ok() {
                quest_conditions.write(QuestAcceptCondEvent {
                    player_uid: player.uid,
                    cond_type:
                        snezhnaya_gi_data::quest::quest_config::QuestCond::DailyTaskRewardReceived,
                    param: 0,
                });
            }
            let code = result.as_ref().err().copied().unwrap_or(Retcode::RetSucc);
            out.send_json(player.uid,"TakeDailyTaskScoreRewardRsp",json!({"retcode":i32::from(code),"is_claim_daily_attendance":attendance,
                "item_list":result.unwrap_or_default().into_iter().map(|(item_id,count)|json!({"item_id":item_id,"count":count})).collect::<Vec<_>>() }));
            notify(player, &out);
        }
    }
}

fn finish_task(players: &mut Players, owner: u32, index: usize) -> Result<(), Retcode> {
    let cfg = DailyTaskConfig::get();
    let task_id = players
        .get(owner)
        .and_then(|p| p.daily_task_bin.as_ref())
        .and_then(|s| s.task_list.get(index))
        .ok_or(Retcode::RetFail)?
        .daily_task_id;
    let task = cfg.tasks.get(&task_id).ok_or(Retcode::RetNotFoundConfig)?;
    let coop = players.keys().len() > 1;
    let time = now();
    let mut pending = HashMap::new();
    for player in players
        .values_mut()
        .filter(|p| unlocked(p) && p.scene_bin.as_ref().is_some_and(|s| s.my_cur_scene_id == 3))
    {
        let Some(state) = &player.daily_task_bin else {
            continue;
        };
        if state.last_time == 0 || attendance::finished_count(player, time) >= TASK_COUNT {
            continue;
        }
        let reward = cfg
            .reward(task, state.score_player_level)
            .ok_or(Retcode::RetNotFoundConfig)?;
        let mut items = roll(reward.drop_id)?;
        boost_fetter(&mut items, coop)?;
        pending.insert(
            player.uid,
            b::DailyTaskRewardBin {
                item_map: items.into_iter().collect(),
            },
        );
    }
    let host = players.get_mut(owner).unwrap();
    let state = host.daily_task_bin.as_mut().unwrap();
    state.task_list[index].is_finished = true;
    state.task_list[index].pending_reward_map = pending;
    let uids: Vec<_> = state.task_list[index]
        .pending_reward_map
        .keys()
        .copied()
        .collect();
    for uid in uids {
        let player = players.get_mut(uid).unwrap();
        let state = player.daily_task_bin.as_mut().unwrap();
        state.finished_num = state.finished_num.saturating_add(1).min(TASK_COUNT);
        state.total_reward_num = state.total_reward_num.saturating_add(1);
        record_progress(player, "daily_task", 0, 1);
    }
    Ok(())
}
fn deliver_pending(players: &mut Players, owner: u32, out: &MessageOutput) {
    let Some(state) = players.get(owner).and_then(|p| p.daily_task_bin.as_ref()) else {
        return;
    };
    let mut rewards = Vec::new();
    for (old, list) in [
        (false, &state.task_list),
        (true, &state.pending_task_rewards),
    ] {
        for (index, task) in list.iter().enumerate() {
            for (&uid, reward) in &task.pending_reward_map {
                rewards.push((old, index, uid, reward.clone()));
            }
        }
    }
    for (old, index, uid, reward) in rewards {
        let Some(player) = players.get_mut(uid) else {
            continue;
        };
        let items: Amounts = reward.item_map.into_iter().collect();
        let Ok(change) = inventory::transact(player, &Amounts::new(), &items, &[]) else {
            continue;
        };
        inventory::notify_transaction(out, player, &change);
        let state = players
            .get_mut(owner)
            .unwrap()
            .daily_task_bin
            .as_mut()
            .unwrap();
        let task = if old {
            &mut state.pending_task_rewards[index]
        } else {
            &mut state.task_list[index]
        };
        task.pending_reward_map.remove(&uid);
    }
    if let Some(state) = players
        .get_mut(owner)
        .and_then(|p| p.daily_task_bin.as_mut())
    {
        state
            .pending_task_rewards
            .retain(|t| !t.pending_reward_map.is_empty());
    }
}

#[derive(Default)]
pub(super) struct SeenEvents {
    day: u32,
    entities: HashSet<u32>,
    challenges: HashSet<(u32, u32)>,
}
pub(super) fn observe(
    mut players: ResMut<Players>,
    owner: Res<WorldOwnerUID>,
    out: Res<MessageOutput>,
    mut monsters: MessageReader<MonsterKillEvent>,
    mut gadgets: MessageReader<LuaTriggerEvent>,
    mut challenges: MessageReader<ChallengeFinishEvent>,
    mut quest_finishes: MessageReader<DailyTaskFinishEvent>,
    mut quest_conditions: MessageWriter<QuestAcceptCondEvent>,
    entities: Query<(&ProtocolEntityID, &GadgetID)>,
    mut seen: Local<SeenEvents>,
) {
    if seen.day != day(now()) {
        *seen = SeenEvents {
            day: day(now()),
            ..Default::default()
        };
    }
    let mut events = Vec::new();
    for event in quest_finishes.read().filter(|e| e.player_uid == owner.0) {
        if let Some(state) = players
            .get_mut(owner.0)
            .and_then(|p| p.daily_task_bin.as_mut())
        {
            for task in state
                .task_list
                .iter_mut()
                .filter(|t| t.daily_task_id == event.daily_task_id && !t.is_finished)
            {
                task.progress = DailyTaskConfig::get()
                    .tasks
                    .get(&task.daily_task_id)
                    .map_or(1, |r| r.finish_progress.max(1));
            }
        }
    }
    for event in monsters.read() {
        events.push((
            event.group_id,
            "DAILY_FINISH_MONSTER_NUM",
            0,
            event.entity_id,
            None,
        ));
    }
    for event in gadgets
        .read()
        .filter(|e| e.event_type == EventType::EventAnyGadgetDie)
    {
        let gadget = entities
            .iter()
            .find(|(id, _)| id.0 == event.evt.source_eid)
            .map(|(_, id)| id.0);
        if let Some(id) = gadget {
            events.push((
                event.group_id,
                "DAILY_FINISH_GADGET_ID_NUM",
                id,
                event.evt.source_eid,
                None,
            ));
        }
    }
    for event in challenges.read().filter(|e| e.is_success) {
        events.push((
            event.group_id,
            "DAILY_FINISH_CHALLENGE",
            event.challenge_id,
            0,
            Some(event.challenge_index),
        ));
    }
    let in_scene = players
        .get(owner.0)
        .and_then(|p| p.scene_bin.as_ref())
        .is_some_and(|s| s.my_cur_scene_id == 3);
    let mut changed = false;
    if in_scene {
        for (group, kind, param, entity, challenge) in events {
            let Some(state) = players.get(owner.0).and_then(|p| p.daily_task_bin.as_ref()) else {
                break;
            };
            let indices: Vec<_> = state
                .task_list
                .iter()
                .enumerate()
                .filter(|(_, t)| !t.is_finished)
                .filter_map(|(i, t)| {
                    DailyTaskConfig::get()
                        .tasks
                        .get(&t.daily_task_id)
                        .map(|c| (i, c))
                })
                .filter(|(_, c)| {
                    c.finish_type == kind
                        && c.new_group_vec.contains(&group)
                        && (kind == "DAILY_FINISH_MONSTER_NUM" || c.finish_param1 == param)
                })
                .map(|(i, _)| i)
                .collect();
            if indices.is_empty() {
                continue;
            }
            if let Some(index) = challenge {
                if !seen.challenges.insert((group, index)) {
                    continue;
                }
            } else if !seen.entities.insert(entity) {
                continue;
            }
            for index in indices {
                let state = players
                    .get_mut(owner.0)
                    .unwrap()
                    .daily_task_bin
                    .as_mut()
                    .unwrap();
                let task = &mut state.task_list[index];
                let total = DailyTaskConfig::get().tasks[&task.daily_task_id]
                    .finish_progress
                    .max(1);
                task.progress = task.progress.saturating_add(1).min(total);
                changed = true;
            }
        }
        let ready: Vec<_> = players
            .get(owner.0)
            .and_then(|p| p.daily_task_bin.as_ref())
            .into_iter()
            .flat_map(|s| s.task_list.iter().enumerate())
            .filter(|(_, t)| {
                !t.is_finished
                    && DailyTaskConfig::get()
                        .tasks
                        .get(&t.daily_task_id)
                        .is_some_and(|c| t.progress >= c.finish_progress.max(1))
            })
            .map(|(i, _)| i)
            .collect();
        for index in ready {
            if finish_task(&mut players, owner.0, index).is_ok() {
                changed = true;
            }
        }
    }
    deliver_pending(&mut players, owner.0, &out);
    if changed {
        if let Some(state) = players.get(owner.0).and_then(|p| p.daily_task_bin.as_ref()) {
            for task in &state.task_list {
                for uid in players.keys() {
                    out.send_json(
                        *uid,
                        "DailyTaskProgressNotify",
                        json!({"info":task_info(task)}),
                    );
                }
            }
        }
        for player in players.values_mut() {
            notify(player, &out);
            if player
                .daily_task_bin
                .as_ref()
                .is_some_and(|s| {
                    attendance::finished_count(player, now()) >= TASK_COUNT
                        && !s.is_taken_score_reward
                })
            {
                quest_conditions.write(QuestAcceptCondEvent {
                    player_uid: player.uid,
                    cond_type:
                        snezhnaya_gi_data::quest::quest_config::QuestCond::DailyTaskRewardCanGet,
                    param: 0,
                });
            }
        }
        notify_world(&players, owner.0, &out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_new_day_assignment_expires_old_tasks_and_keeps_pending_rewards() {
        let reset_time = 86_400 - 14_400;
        let reward = b::DailyTaskRewardBin {
            item_map: [(201, 10)].into(),
        };
        let mut player = b::PlayerDataBin {
            uid: 42,
            basic_bin: Some(b::PlayerBasicCompBin {
                level: UNLOCK_LEVEL,
                ..Default::default()
            }),
            daily_task_bin: Some(b::PlayerDailyTaskCompBin {
                last_time: reset_time - 1,
                finished_num: TASK_COUNT,
                score_player_level: UNLOCK_LEVEL,
                task_list: vec![b::DailyTaskInfoBin {
                    daily_task_id: 1,
                    is_finished: true,
                    pending_reward_map: [(42, reward.clone())].into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let config = DailyTaskConfig {
            tasks: [(
                1,
                DailyTask {
                    new_group_vec: vec![100],
                    ..Default::default()
                },
            )]
            .into(),
            cities: Default::default(),
            levels: Default::default(),
            rewards: Default::default(),
            groups: Default::default(),
        };
        assert_eq!(refresh(&mut player, reset_time - 1, &config), None);
        assert_eq!(refresh(&mut player, reset_time, &config), Some(vec![100]));
        assert_eq!(refresh(&mut player, reset_time + 1, &config), None);
        let state = player.daily_task_bin.as_ref().unwrap();
        assert_eq!(state.last_time, 0);
        assert_eq!(state.finished_num, 0);
        assert!(state.task_list.is_empty());
        assert_eq!(state.pending_task_rewards.len(), 1);
        assert_eq!(
            state.pending_task_rewards[0].pending_reward_map[&42],
            reward
        );
        assert!(matches!(
            claim_score(&mut player, false, &MessageOutput::new(HashMap::new())),
            Err(Retcode::RetDailyTaskNotFinish)
        ));
    }
}
