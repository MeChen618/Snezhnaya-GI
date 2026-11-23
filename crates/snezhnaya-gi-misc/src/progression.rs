use bevy_ecs::prelude::*;
use snezhnaya_gi_data::excel::{
    self,
    progression::{ProgressionConfig, Trigger},
};
use snezhnaya_gi_event::{
    lua::MonsterKillEvent, quest::QuestContentProgressEvent, scene::WorldOwnerUID,
};
use snezhnaya_gi_inventory::transaction::{self as inventory, Amounts};
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::{progress_counter, record_progress, Players};
use snezhnaya_gi_proto::{
    normal::*,
    retcode::Retcode,
    server_only::{self as b, equip_bin, item_bin},
};
use std::collections::{HashMap, HashSet};

use snezhnaya_gi_data::scene::scene_point_config::get_scene_point_config_collection as excel_points;

const PERIOD: u32 = 42 * 86400;
pub(super) fn now() -> u32 {
    common::time_util::unix_timestamp() as u32
}
fn day(time: u32) -> u32 {
    time.saturating_add(4 * 3600) / 86400
}
fn week(time: u32) -> u32 {
    (day(time) + 3) / 7
}
fn period(time: u32) -> (u32, u32) {
    let anchor = 1785528000;
    let start = if time >= anchor {
        anchor + (time - anchor) / PERIOD * PERIOD
    } else {
        time - time % PERIOD
    };
    (start, start + PERIOD - 1)
}
fn max_level() -> u32 {
    ProgressionConfig::get()
        .pass_level_points
        .keys()
        .copied()
        .max()
        .unwrap_or(50)
}

pub(super) fn counter_key(trigger: &Trigger) -> Option<(&'static str, u32)> {
    Some(match trigger.trigger_type.as_str() {
        "TRIGGER_LOGIN" => ("login", 0),
        "TRIGGER_GACHA_NUM" => ("gacha", trigger.param(0)),
        "TRIGGER_MONSTER_DIE" => ("kill", trigger.param(0)),
        "TRIGGER_FINISH_DUNGEON" => ("dungeon", trigger.param(0)),
        "TRIGGER_COST_MATERIAL" => ("cost", trigger.param(0)),
        "TRIGGER_OBTAIN_MATERIAL_NUM" => ("obtain", trigger.param(0)),
        "TRIGGER_CONSUME_RESIN" => ("cost", 106),
        "TRIGGER_DO_COOK" => ("cook", 0),
        "TRIGGER_DO_FORGE" => ("forge", 0),
        "TRIGGER_FORGE_WEAPON" => ("forge_weapon", 0),
        "TRIGGER_DO_COMBINE" | "TRIGGER_COMBINE_ITEM" => ("combine", 0),
        "TRIGGER_DO_COMPOUND" => ("compound", 0),
        "TRIGGER_START_AVATAR_EXPEDITION" => ("expedition_start", 0),
        "TRIGGER_AVATAR_EXPEDITION" => ("expedition_complete", 0),
        "TRIGGER_DAILY_TASK" => ("daily_task", 0),
        "TRIGGER_OPEN_CHEST_WITH_GADGET_ID" => ("chest", trigger.param(0)),
        _ => return None,
    })
}
pub(super) fn counter(player: &b::PlayerDataBin, trigger: &Trigger) -> u64 {
    let Some((kind, param)) = counter_key(trigger) else {
        return 0;
    };
    let ids: Vec<u32> = trigger
        .param_list
        .first()
        .into_iter()
        .flat_map(|s| s.split(';'))
        .filter_map(|s| s.parse().ok())
        .collect();
    if ids.len() > 1 {
        ids.into_iter()
            .map(|id| progress_counter(player, kind, id))
            .fold(0, u64::saturating_add)
    } else {
        progress_counter(player, kind, param)
    }
}

fn quest_done(player: &b::PlayerDataBin, id: u32) -> bool {
    player
        .quest_bin
        .as_ref()
        .and_then(|q| q.quest_bin.as_ref())
        .and_then(|q| q.quest_map.get(&id))
        .is_some_and(|q| q.state == 3)
}

pub(super) fn value(player: &b::PlayerDataBin, trigger: &Trigger) -> u32 {
    if counter_key(trigger).is_some() {
        return counter(player, trigger).min(u32::MAX as u64) as u32;
    }
    let avatars = player.avatar_bin.as_ref();
    let p = trigger.param(0);
    let avatar_count = |test: &dyn Fn(&b::AvatarBin) -> bool| {
        avatars.map_or(0, |a| {
            a.avatar_map.values().filter(|a| test(a)).count() as u32
        })
    };
    let items = || {
        player
            .item_bin
            .as_ref()
            .into_iter()
            .flat_map(|i| i.iter().map(|(_, item)| item))
    };
    match trigger.trigger_type.as_str() {
        "TRIGGER_UNLOCK_AREA" => player
            .scene_bin
            .as_ref()
            .and_then(|s| s.world.as_ref())
            .map_or(0, |w| {
                w.scene_map
                    .values()
                    .filter_map(|s| s.area_comp_bin.as_ref())
                    .any(|a| a.unlocked_area_list.contains(&p)) as u32
            }),
        "TRIGGER_UNLOCK_TRANS_POINT" | "TRIGGER_UNLOCK_GATE_TEMPLE" => {
            let points = excel_points();
            player
                .scene_bin
                .as_ref()
                .and_then(|s| s.world.as_ref())
                .map_or(0, |w| {
                    w.scene_map
                        .iter()
                        .map(|(id, scene)| {
                            scene
                                .unlocked_point_list
                                .iter()
                                .filter(|point| {
                                    let row = points.get(id).and_then(|s| s.points.get(point));
                                    row.is_some_and(|r| {
                                        if trigger.trigger_type == "TRIGGER_UNLOCK_GATE_TEMPLE" {
                                            r.point_type.as_str().contains("DungeonEntry")
                                        } else {
                                            r.point_type.as_str().contains("TransPoint")
                                        }
                                    })
                                })
                                .count() as u32
                        })
                        .sum()
                })
        }
        "TRIGGER_CITY_LEVEL_UP" => player
            .scene_bin
            .as_ref()
            .and_then(|s| s.world.as_ref())
            .map_or(0, |w| {
                w.scene_map
                    .values()
                    .filter_map(|s| s.area_comp_bin.as_ref())
                    .filter_map(|a| a.city_map.get(&p))
                    .any(|c| c.level >= trigger.param(1)) as u32
            }),
        "TRIGGER_UPGRADE_TALENT" => avatars.map_or(0, |a| {
            a.avatar_map
                .values()
                .map(|a| {
                    a.depot_map
                        .values()
                        .map(|d| {
                            d.skill_level_map
                                .values()
                                .map(|level| level.saturating_sub(1))
                                .sum::<u32>()
                        })
                        .sum::<u32>()
                })
                .sum()
        }),
        "TRIGGER_WEAR_RELIQUARY" => avatar_count(&|a| {
            a.equip_map
                .iter()
                .filter(|(slot, _)| matches!(**slot, 1..=5))
                .count()
                >= p as usize
        }),
        "TRIGGER_AVATAR_UPGRADE" => avatar_count(&|a| a.level >= p),
        "TRIGGER_AVATAR_PROMOTE" => avatar_count(&|a| a.promote_level >= p),
        "TRIGGER_AVATAR_PROMOTE_EXCLUDING_PLAYER" => {
            avatar_count(&|a| a.promote_level >= p && !matches!(a.avatar_id, 10000005 | 10000007))
        }
        "TRIGGER_WEAPON_UPGRADE"
        | "TRIGGER_WEAPON_PROMOTE"
        | "TRIGGER_SPECIFIED_WEAPON_UPGRADE" => items()
            .filter(|i| {
                let Some(item_bin::Detail::Equip(e)) = &i.detail else {
                    return false;
                };
                let Some(equip_bin::Detail::Weapon(w)) = &e.detail else {
                    return false;
                };
                if trigger.trigger_type == "TRIGGER_SPECIFIED_WEAPON_UPGRADE" {
                    i.item_id == p && w.level >= trigger.param(1)
                } else if trigger.trigger_type == "TRIGGER_WEAPON_PROMOTE" {
                    w.promote_level >= p
                } else {
                    w.level >= p
                }
            })
            .count() as u32,
        "TRIGGER_RELIQUARY_UPGRADE" | "TRIGGER_RELIQUARY_UPGRADE_EQUAL_RANK_LEVEL" => items()
            .filter(|i| {
                let Some(item_bin::Detail::Equip(e)) = &i.detail else {
                    return false;
                };
                let Some(equip_bin::Detail::Reliquary(r)) = &e.detail else {
                    return false;
                };
                r.level.saturating_sub(1) >= p
                    && (trigger.param(1) == 0
                        || excel::reliquary_excel_config_collection::get()
                            .get(&i.item_id)
                            .is_some_and(|row| row.rank_level >= trigger.param(1)))
            })
            .count()
            as u32,
        "TRIGGER_FINISH_CHAPTER" => ProgressionConfig::get()
            .chapter_end_quests
            .get(&p)
            .is_some_and(|id| quest_done(player, *id)) as u32,
        "TRIGGER_FINISH_QUEST_AND" | "TRIGGER_FINISH_QUEST_OR" => {
            let ids: Vec<_> = trigger
                .param_list
                .iter()
                .flat_map(|s| s.split(';'))
                .filter_map(|s| s.parse::<u32>().ok())
                .filter(|id| *id != 0)
                .collect();
            (!ids.is_empty()
                && if trigger.trigger_type == "TRIGGER_FINISH_QUEST_AND" {
                    ids.iter().all(|id| quest_done(player, *id))
                } else {
                    ids.iter().any(|id| quest_done(player, *id))
                }) as u32
        }
        "TRIGGER_UNLOCK_RECIPE" => player
            .cook_bin
            .as_ref()
            .map_or(0, |c| c.recipe_data_map.len() as u32),
        _ => 0,
    }
}

fn initialize(player: &mut b::PlayerDataBin, time: u32) {
    let cfg = ProgressionConfig::get();
    let Some(schedule) = cfg.current_pass() else {
        return;
    };
    let (begin, end) = period(time);
    let new_day = player
        .battle_pass_bin
        .as_ref()
        .map_or(true, |p| day(p.last_refresh_time) != day(time));
    if new_day || progress_counter(player, "login", 0) == 0 {
        record_progress(player, "login", 0, 1);
    }
    let previous = player
        .battle_pass_bin
        .as_ref()
        .map_or(0, |p| p.last_refresh_time);
    let previous_week = player
        .battle_pass_bin
        .as_ref()
        .map_or(0, |p| p.weekly_last_refresh_time);
    let reset = player.battle_pass_bin.as_ref().map_or(true, |p| {
        p.cur_schedule_id != schedule.id
            || !p
                .schedule_bin_list
                .iter()
                .any(|s| s.schedule_id == schedule.id && s.begin_time == begin)
    });
    let definitions: Vec<_> = cfg.pass_missions(schedule.id).collect();
    let baselines: HashMap<_, _> = definitions
        .iter()
        .map(|m| (m.id, counter(player, &m.trigger_config)))
        .collect();
    let bp = player.battle_pass_bin.get_or_insert_with(Default::default);
    bp.cur_schedule_id = schedule.id;
    if reset {
        bp.schedule_bin_list
            .retain(|s| s.schedule_id != schedule.id);
        bp.schedule_bin_list.push(b::BattlePassScheduleBin {
            schedule_id: schedule.id,
            unlock_status: 1,
            begin_time: begin,
            end_time: end,
            reward_plan: schedule.level_reward_index_id,
            ..Default::default()
        });
        bp.cur_battle_pass_mission_bin_list.clear();
    }
    let current = bp
        .schedule_bin_list
        .iter_mut()
        .find(|s| s.schedule_id == schedule.id)
        .unwrap();
    if current.unlock_status == 0 {
        current.unlock_status = 1;
    }
    if current.reward_plan == 0 {
        current.reward_plan = schedule.level_reward_index_id;
    }
    if week(previous_week) != week(time) {
        current.cur_cycle_points = 0;
    }
    for def in definitions {
        let daily =
            def.refresh_type.is_empty() || def.refresh_type == "BATTLE_PASS_MISSION_REFRESH_DAILY";
        let weekly = matches!(
            def.refresh_type.as_str(),
            "BATTLE_PASS_MISSION_REFRESH_CYCLE"
                | "BATTLE_PASS_MISSION_REFRESH_CYCLE_CROSS_SCHEDULE"
        );
        let refresh =
            (daily && day(previous) != day(time)) || (weekly && week(previous_week) != week(time));
        let state = if let Some(index) = bp
            .cur_battle_pass_mission_bin_list
            .iter()
            .position(|m| m.mission_id == def.id)
        {
            &mut bp.cur_battle_pass_mission_bin_list[index]
        } else {
            bp.cur_battle_pass_mission_bin_list
                .push(b::BattlePassMissionBin {
                    mission_id: def.id,
                    ..Default::default()
                });
            bp.cur_battle_pass_mission_bin_list.last_mut().unwrap()
        };
        if refresh || state.mission_status == 0 {
            state.mission_status = 1;
            state.progress = 0;
            state.counter_base = baselines[&def.id];
            if def.trigger_config.trigger_type == "TRIGGER_LOGIN" {
                state.counter_base = state.counter_base.saturating_sub(1);
            }
        }
    }
    bp.last_refresh_time = time;
    bp.weekly_last_refresh_time = time;
    let investigation = player
        .investigation_bin
        .get_or_insert_with(Default::default);
    for &id in cfg.investigations.keys() {
        if !investigation
            .investigation_list
            .iter()
            .any(|r| r.investigation_id == id)
        {
            investigation.investigation_list.push(b::InvestigationBin {
                investigation_id: id,
                ..Default::default()
            });
        }
    }
}

fn refresh(player: &mut b::PlayerDataBin, time: u32) -> bool {
    initialize(player, time);
    let cfg = ProgressionConfig::get();
    let mut changed = false;
    let mission_values: HashMap<_, _> = cfg
        .current_pass()
        .into_iter()
        .flat_map(|s| cfg.pass_missions(s.id))
        .map(|m| {
            let progress = if counter_key(&m.trigger_config).is_some() {
                counter(player, &m.trigger_config)
                    .saturating_sub(
                        player
                            .battle_pass_bin
                            .as_ref()
                            .unwrap()
                            .cur_battle_pass_mission_bin_list
                            .iter()
                            .find(|s| s.mission_id == m.id)
                            .map_or(0, |s| s.counter_base),
                    )
                    .min(u32::MAX as u64) as u32
            } else {
                value(player, &m.trigger_config)
            };
            (m.id, progress.min(m.progress))
        })
        .collect();
    for mission in &mut player
        .battle_pass_bin
        .as_mut()
        .unwrap()
        .cur_battle_pass_mission_bin_list
    {
        if mission.mission_status == 3 {
            continue;
        }
        let Some(&progress) = mission_values.get(&mission.mission_id) else {
            continue;
        };
        let target = cfg.pass_missions[&mission.mission_id].progress;
        let progress = progress.max(mission.progress);
        let status = if progress >= target { 2 } else { 1 };
        if mission.progress != progress || mission.mission_status != status {
            changed = true;
            mission.progress = progress;
            mission.mission_status = status;
        }
    }
    let target_values: Vec<_> = cfg
        .targets
        .values()
        .map(|t| {
            (
                t.id,
                value(player, &t.trigger_config).min(t.progress),
                t.progress,
            )
        })
        .collect();
    let state = player.investigation_bin.as_mut().unwrap();
    for (id, progress, total) in target_values {
        let previous = state.target_progress_map.get(&id).copied().unwrap_or(0);
        if progress > previous {
            state.target_progress_map.insert(id, progress);
            changed = true;
        }
        if total > 0
            && progress.max(previous) >= total
            && !state.finished_target_id_list.contains(&id)
        {
            state.finished_target_id_list.push(id);
            changed = true;
        }
    }
    for chapter in &mut state.investigation_list {
        chapter.progress = cfg
            .targets
            .values()
            .filter(|t| {
                t.investigation_id == chapter.investigation_id
                    && state.reward_taken_target_id_list.contains(&t.id)
            })
            .count() as u32;
    }
    changed
}

fn pass_state(player: &b::PlayerDataBin) -> Option<&b::BattlePassScheduleBin> {
    let bp = player.battle_pass_bin.as_ref()?;
    bp.schedule_bin_list
        .iter()
        .find(|s| s.schedule_id == bp.cur_schedule_id)
}
fn pass_state_mut(player: &mut b::PlayerDataBin) -> Option<&mut b::BattlePassScheduleBin> {
    let bp = player.battle_pass_bin.as_mut()?;
    let id = bp.cur_schedule_id;
    bp.schedule_bin_list
        .iter_mut()
        .find(|s| s.schedule_id == id)
}
fn mission_packet(state: &b::BattlePassMissionBin) -> Option<BattlePassMission> {
    let def = ProgressionConfig::get()
        .pass_missions
        .get(&state.mission_id)?;
    Some(BattlePassMission {
        mission_id: def.id,
        mission_status: state.mission_status as i32,
        cur_progress: state.progress,
        total_progress: def.progress,
        reward_battle_pass_point: def.add_point,
        mission_type: if def.refresh_type.is_empty()
            || def.refresh_type == "BATTLE_PASS_MISSION_REFRESH_DAILY"
        {
            0
        } else if def.refresh_type.contains("CYCLE") {
            1
        } else {
            2
        },
    })
}
fn schedule_packet(state: &b::BattlePassScheduleBin) -> BattlePassSchedule {
    let end = (((week(now()) + 1) * 7 - 3) * 86400).saturating_sub(4 * 3600) - 1;
    BattlePassSchedule {
        schedule_id: state.schedule_id,
        product_info: Some(BattlePassProduct {
            normal_product_id: ProgressionConfig::get()
                .pass_product_ids
                .get(&201)
                .cloned()
                .unwrap_or_default(),
            extra_product_id: ProgressionConfig::get()
                .pass_product_ids
                .get(&202)
                .cloned()
                .unwrap_or_default(),
            upgrade_product_id: ProgressionConfig::get()
                .pass_product_ids
                .get(&203)
                .cloned()
                .unwrap_or_default(),
            ..Default::default()
        }),
        level: state.level,
        point: state.point,
        unlock_status: state.unlock_status as i32,
        begin_time: state.begin_time,
        end_time: state.end_time,
        is_viewed: state.is_viewed,
        cur_cycle_points: state.cur_cycle_points,
        cur_cycle: Some(BattlePassCycle {
            begin_time: end.saturating_sub(7 * 86400 - 1),
            end_time: end,
            cycle_idx: week(now()).saturating_sub(week(state.begin_time)) + 1,
        }),
        reward_taken_list: state
            .reward_taken_list
            .iter()
            .map(|r| BattlePassRewardTag {
                level: r.level,
                reward_id: r.reward_id,
                unlock_status: r.unlock_status as i32,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}
fn schedule_json(state: &b::BattlePassScheduleBin) -> serde_json::Value {
    let mut packet =
        serde_json::to_value(schedule_packet(state)).expect("battle pass serialization");
    packet["reward_plan_option_list"] = serde_json::json!((1..=5)
        .map(|slot| serde_json::json!({
            "IMMIBAOCIEH": slot, "battle_pass_plan":state.reward_plan
        }))
        .collect::<Vec<_>>());
    packet
}
fn notify_pass(player: &b::PlayerDataBin, output: &MessageOutput) {
    let state = pass_state(player);
    let missions: Vec<_> = player
        .battle_pass_bin
        .as_ref()
        .into_iter()
        .flat_map(|p| p.cur_battle_pass_mission_bin_list.iter())
        .filter_map(mission_packet)
        .collect();
    output.send_json(player.uid,"BattlePassAllDataNotify",serde_json::json!({
        "have_cur_schedule":state.is_some(),"cur_schedule":state.map(schedule_json),
        "battle_pass_plan":state.map_or(1,|s|s.reward_plan),"is_viewed":state.is_some_and(|s|s.is_viewed),"mission_list":missions
    }));
}
fn notify_schedule(player: &b::PlayerDataBin, output: &MessageOutput) {
    let state = pass_state(player);
    output.send_json(player.uid,"BattlePassCurScheduleUpdateNotify",serde_json::json!({
        "have_cur_schedule":state.is_some(),"cur_schedule":state.map(schedule_json),
        "battle_pass_plan":state.map_or(1,|s|s.reward_plan),"is_viewed":state.is_some_and(|s|s.is_viewed)
    }));
}
fn target_packet(
    player: &b::PlayerDataBin,
    target: &excel::progression::Target,
) -> InvestigationTarget {
    let state = player.investigation_bin.as_ref().unwrap();
    InvestigationTarget {
        quest_id: target.id,
        investigation_id: target.investigation_id,
        total_progress: target.progress,
        progress: state
            .target_progress_map
            .get(&target.id)
            .copied()
            .unwrap_or(0),
        state: if state.reward_taken_target_id_list.contains(&target.id) {
            3
        } else if state.finished_target_id_list.contains(&target.id) {
            2
        } else {
            1
        },
    }
}
fn chapter_packet(_player: &b::PlayerDataBin, chapter: &b::InvestigationBin) -> Investigation {
    let total = ProgressionConfig::get()
        .targets
        .values()
        .filter(|t| t.investigation_id == chapter.investigation_id)
        .count() as u32;
    Investigation {
        id: chapter.investigation_id,
        total_progress: total,
        progress: chapter.progress,
        state: if chapter.is_taken_reward {
            3
        } else if total > 0 && chapter.progress >= total {
            2
        } else {
            1
        },
    }
}
fn notify_investigation(player: &b::PlayerDataBin, output: &MessageOutput) {
    output.send(
        player.uid,
        "PlayerInvestigationAllInfoNotify",
        PlayerInvestigationAllInfoNotify {
            investigation_list: player
                .investigation_bin
                .as_ref()
                .unwrap()
                .investigation_list
                .iter()
                .map(|r| chapter_packet(player, r))
                .collect(),
            investigation_target_list: ProgressionConfig::get()
                .targets
                .values()
                .filter(|t| {
                    ProgressionConfig::get()
                        .investigations
                        .get(&t.investigation_id)
                        .is_some_and(|i| {
                            i.unlock_level <= player.basic_bin.as_ref().map_or(1, |b| b.level)
                        })
                })
                .map(|t| target_packet(player, t))
                .collect(),
        },
    );
}

pub fn reward_items(ids: impl IntoIterator<Item = u32>) -> Result<Amounts, Retcode> {
    inventory::reward_items(ids)
}
fn add_points(state: &mut b::BattlePassScheduleBin, mut points: u32) {
    let table = &ProgressionConfig::get().pass_level_points;
    let limit = max_level();
    while points > 0 && state.level < limit {
        let need = table.get(&state.level).copied().unwrap_or(1000).max(1);
        let add = points.min(need.saturating_sub(state.point));
        state.point += add;
        points -= add;
        if state.point >= need {
            state.level += 1;
            state.point = 0;
        }
    }
}

fn notify_level_rewards(player: &b::PlayerDataBin, output: &MessageOutput) {
    output.send(
        player.uid,
        "PlayerLevelRewardUpdateNotify",
        PlayerLevelRewardUpdateNotify {
            level_list: player
                .basic_bin
                .as_ref()
                .map(|b| b.reward_taken_level_list.clone())
                .unwrap_or_default(),
        },
    );
}

fn claim_level_reward(
    player: &mut b::PlayerDataBin,
    level: u32,
    output: &MessageOutput,
) -> Result<u32, Retcode> {
    let reward = *ProgressionConfig::get()
        .player_level_rewards
        .get(&level)
        .ok_or(Retcode::RetItemInvalidTarget)?;
    let basic = player.basic_bin.as_ref().ok_or(Retcode::RetFail)?;
    if level > basic.level {
        return Err(Retcode::RetPlayerLevelLessThan);
    }
    if basic.reward_taken_level_list.contains(&level) {
        return Err(Retcode::RetRewardHasTaken);
    }
    let items = reward_items([reward])?;
    let change = inventory::transact(player, &Amounts::new(), &items, &[])?;
    // 发奖成功后再记领取状态，背包不足时仍可重试。
    player
        .basic_bin
        .as_mut()
        .unwrap()
        .reward_taken_level_list
        .push(level);
    inventory::notify_transaction(output, player, &change);
    Ok(reward)
}

pub fn login(mut players: ResMut<Players>, output: Res<MessageOutput>) {
    let time = now();
    for player in players.values_mut() {
        refresh(player, time);
        notify_pass(player, &output);
        notify_investigation(player, &output);
        notify_level_rewards(player, &output);
    }
}

pub fn handle(
    mut events: MessageReader<ClientMessageEvent>,
    mut players: ResMut<Players>,
    output: Res<MessageOutput>,
) {
    for message in events.read() {
        let name = message.message_name();
        if !matches!(
            name,
            "TakeBattlePassMissionPointReq"
                | "TakeBattlePassRewardReq"
                | "BuyBattlePassLevelReq"
                | "GetBattlePassProductReq"
                | "SetBattlePassViewedReq"
                | "_BattlePassSetRewardPlanReq"
                | "BattlePassSetRewardPlanReq"
                | "TakeInvestigationTargetRewardReq"
                | "TakeInvestigationRewardReq"
                | "InvestigationReadQuestDailyNotify"
                | "TakePlayerLevelRewardReq"
        ) {
            continue;
        }
        let Some(player) = players.get_mut(message.sender_uid()) else {
            continue;
        };
        refresh(player, now());
        match name {
            "TakePlayerLevelRewardReq" => {
                if let Some(req) = message.decode::<TakePlayerLevelRewardReq>() {
                    let result = claim_level_reward(player, req.level, &output);
                    output.send(
                        player.uid,
                        "TakePlayerLevelRewardRsp",
                        TakePlayerLevelRewardRsp {
                            level: req.level,
                            reward_id: result.as_ref().copied().unwrap_or_default(),
                            retcode: result.err().map(i32::from).unwrap_or_default(),
                        },
                    );
                    notify_level_rewards(player, &output);
                }
            }
            "GetBattlePassProductReq" => {
                let play_type = message
                    .decode_json()
                    .and_then(|r| {
                        r.get("battle_pass_product_play_type")
                            .and_then(serde_json::Value::as_u64)
                    })
                    .unwrap_or(0) as u32;
                let config = 200u32.checked_add(play_type);
                let catalog = ProgressionConfig::get();
                let product = config.and_then(|id| catalog.pass_product_ids.get(&id));
                let tier = config.and_then(|id| catalog.pass_product_tiers.get(&id));
                output.send(
                    player.uid,
                    "GetBattlePassProductRsp",
                    GetBattlePassProductRsp {
                        battle_pass_product_play_type: play_type,
                        cur_schedule_id: pass_state(player).map_or(0, |s| s.schedule_id),
                        product_id: product.cloned().unwrap_or_default(),
                        price_tier: tier.cloned().unwrap_or_default(),
                        retcode: if (1..=5).contains(&play_type)
                            && product.is_some()
                            && tier.is_some()
                        {
                            0
                        } else {
                            Retcode::RetShopContentNotMatch.into()
                        },
                    },
                );
            }
            "SetBattlePassViewedReq" => {
                if let Some(state) = pass_state_mut(player) {
                    state.is_viewed = true;
                }
                notify_pass(player, &output);
            }
            "_BattlePassSetRewardPlanReq" | "BattlePassSetRewardPlanReq" => {
                if let Some(req) = message.decode::<BattlePassSetRewardPlanReq>() {
                    let mut error = 0;
                    let state = pass_state_mut(player).unwrap();
                    if !ProgressionConfig::get()
                        .pass_rewards
                        .contains_key(&(req.battle_pass_plan, 1))
                    {
                        error = Retcode::RetFail.into();
                    } else if state.reward_plan != req.battle_pass_plan
                        && !state.reward_taken_list.is_empty()
                    {
                        error = Retcode::RetRewardHasTaken.into();
                    } else {
                        state.reward_plan = req.battle_pass_plan;
                    }
                    let plan = state.reward_plan;
                    output.send(
                        player.uid,
                        "_BattlePassSetRewardPlanRsp",
                        BattlePassSetRewardPlanRsp {
                            retcode: error,
                            battle_pass_plan: plan,
                        },
                    );
                    notify_pass(player, &output);
                }
            }
            "BuyBattlePassLevelReq" => {
                if let Some(req) = message.decode::<BuyBattlePassLevelReq>() {
                    // 回包编号未确认时不扣款，避免购买后一直等不到反馈。
                    if snezhnaya_gi_proto::dy_parser::get_cmd_id_by_name_version(
                        &message.version(),
                        "BuyBattlePassLevelRsp",
                    )
                    .is_none()
                    {
                        tracing::warn!(uid = player.uid, "纪行购买回包尚未映射，未扣款");
                        notify_schedule(player, &output);
                        continue;
                    }
                    let result = buy_levels(player, req.buy_level, &output);
                    output.send(
                        player.uid,
                        "BuyBattlePassLevelRsp",
                        BuyBattlePassLevelRsp {
                            retcode: result.err().map(i32::from).unwrap_or(0),
                            buy_level: req.buy_level,
                        },
                    );
                    notify_pass(player, &output);
                }
            }
            "TakeBattlePassMissionPointReq" => {
                if let Some(req) = message.decode::<TakeBattlePassMissionPointReq>() {
                    let result = claim_points(player, &req.mission_id_list);
                    output.send(
                        player.uid,
                        "TakeBattlePassMissionPointRsp",
                        TakeBattlePassMissionPointRsp {
                            retcode: result.as_ref().err().cloned().map(i32::from).unwrap_or(0),
                            mission_id_list: if result.is_ok() {
                                req.mission_id_list
                            } else {
                                vec![]
                            },
                        },
                    );
                    notify_pass(player, &output);
                }
            }
            "TakeBattlePassRewardReq" => {
                if let Some(req) = message.decode::<TakeBattlePassRewardReq>() {
                    let result = claim_pass_reward(player, &req.take_option_list, &output);
                    let good = result.is_ok();
                    output.send(
                        player.uid,
                        "TakeBattlePassRewardRsp",
                        TakeBattlePassRewardRsp {
                            retcode: result.as_ref().err().cloned().map(i32::from).unwrap_or(0),
                            take_option_list: if good { req.take_option_list } else { vec![] },
                            item_list: result.unwrap_or_default(),
                        },
                    );
                    notify_pass(player, &output);
                }
            }
            "TakeInvestigationTargetRewardReq" => {
                if let Some(req) = message.decode::<TakeInvestigationTargetRewardReq>() {
                    let result = claim_target(player, req.quest_id, &output);
                    output.send(
                        player.uid,
                        "TakeInvestigationTargetRewardRsp",
                        TakeInvestigationTargetRewardRsp {
                            retcode: result.err().map(i32::from).unwrap_or(0),
                            quest_id: req.quest_id,
                        },
                    );
                    refresh(player, now());
                    notify_investigation(player, &output);
                }
            }
            "TakeInvestigationRewardReq" => {
                if let Some(req) = message.decode::<TakeInvestigationRewardReq>() {
                    let result = claim_chapter(player, req.id, &output);
                    output.send(
                        player.uid,
                        "TakeInvestigationRewardRsp",
                        TakeInvestigationRewardRsp {
                            retcode: result.err().map(i32::from).unwrap_or(0),
                            id: req.id,
                        },
                    );
                    notify_investigation(player, &output);
                }
            }
            "InvestigationReadQuestDailyNotify" => {
                player
                    .investigation_bin
                    .as_mut()
                    .unwrap()
                    .is_today_quest_daily_notify_ack = true;
            }
            _ => {}
        }
    }
}

fn buy_levels(
    player: &mut b::PlayerDataBin,
    count: u32,
    output: &MessageOutput,
) -> Result<(), Retcode> {
    let state = pass_state(player).ok_or(Retcode::RetBattlePassNoSchedule)?;
    if count == 0 || state.level.saturating_add(count) > max_level() {
        return Err(Retcode::RetBattlePassLevelOverflow);
    }
    let price = ProgressionConfig::get().pass_schedules[&state.schedule_id]
        .buy_level_cost_coin_num
        .checked_mul(count)
        .ok_or(Retcode::RetFail)?;
    let change = inventory::transact(player, &Amounts::from([(201, price)]), &Amounts::new(), &[])?;
    let state = pass_state_mut(player).unwrap();
    state.level += count;
    inventory::notify_transaction(output, player, &change);
    Ok(())
}
fn claim_points(player: &mut b::PlayerDataBin, ids: &[u32]) -> Result<(), Retcode> {
    let cfg = ProgressionConfig::get();
    let bp = player
        .battle_pass_bin
        .as_ref()
        .ok_or(Retcode::RetBattlePassNoSchedule)?;
    let state = pass_state(player).unwrap();
    let mut seen = HashSet::new();
    let mut points = 0u32;
    let mut limited = 0u32;
    if ids.is_empty() {
        return Err(Retcode::RetFail);
    }
    for id in ids {
        if !seen.insert(*id) {
            return Err(Retcode::RetFail);
        }
        let def = cfg.pass_missions.get(id).ok_or(Retcode::RetFail)?;
        if !bp
            .cur_battle_pass_mission_bin_list
            .iter()
            .any(|m| m.mission_id == *id && m.mission_status == 2)
        {
            return Err(Retcode::RetFail);
        }
        let weekly = def.refresh_type.is_empty()
            || def.refresh_type == "BATTLE_PASS_MISSION_REFRESH_DAILY"
            || def.refresh_type.contains("CYCLE");
        let sum = if weekly { &mut limited } else { &mut points };
        *sum = sum.checked_add(def.add_point).ok_or(Retcode::RetFail)?;
    }
    let cap = cfg.pass_schedules[&state.schedule_id].cycle_point_upper_limit;
    if cap != 0 {
        limited = limited.min(cap.saturating_sub(state.cur_cycle_points));
    }
    points = points.checked_add(limited).ok_or(Retcode::RetFail)?;
    if points == 0 {
        return Err(Retcode::RetFail);
    }
    let state = pass_state_mut(player).unwrap();
    state.cur_cycle_points = state.cur_cycle_points.saturating_add(limited);
    add_points(state, points);
    for mission in &mut player
        .battle_pass_bin
        .as_mut()
        .unwrap()
        .cur_battle_pass_mission_bin_list
    {
        if seen.contains(&mission.mission_id) {
            mission.mission_status = 3;
        }
    }
    Ok(())
}
fn claim_pass_reward(
    player: &mut b::PlayerDataBin,
    options: &[BattlePassRewardTakeOption],
    output: &MessageOutput,
) -> Result<Vec<ItemParam>, Retcode> {
    if options.is_empty() {
        return Err(Retcode::RetFail);
    }
    let state = pass_state(player).ok_or(Retcode::RetBattlePassNoSchedule)?;
    let mut tags = Vec::new();
    let mut seen = HashSet::new();
    for option in options {
        let tag = option.tag.as_ref().ok_or(Retcode::RetFail)?;
        if tag.level == 0
            || tag.level > state.level
            || !matches!(tag.unlock_status, 1 | 2)
            || tag.unlock_status == 2 && state.unlock_status != 2
            || option.battle_pass_plan != 0 && option.battle_pass_plan != state.reward_plan
        {
            return Err(Retcode::RetFail);
        }
        let rewards = ProgressionConfig::get()
            .pass_rewards
            .get(&(state.reward_plan, tag.level))
            .ok_or(Retcode::RetFail)?;
        let allowed = if tag.unlock_status == 1 {
            &rewards.free_reward_id_list
        } else {
            &rewards.paid_reward_id_list
        };
        if !allowed.contains(&tag.reward_id) {
            return Err(Retcode::RetFail);
        }
        let key = (tag.level, tag.unlock_status, tag.reward_id);
        if !seen.insert(key)
            || state
                .reward_taken_list
                .iter()
                .any(|r| (r.level, r.unlock_status as i32, r.reward_id) == key)
        {
            return Err(Retcode::RetRewardHasTaken);
        }
        tags.push(b::BattlePassRewardTagBin {
            level: tag.level,
            unlock_status: tag.unlock_status as u32,
            reward_id: tag.reward_id,
        });
    }
    let rewards = reward_items(tags.iter().map(|r| r.reward_id))?;
    let change = inventory::transact(player, &Amounts::new(), &rewards, &[])?;
    pass_state_mut(player)
        .unwrap()
        .reward_taken_list
        .extend(tags);
    inventory::notify_transaction(output, player, &change);
    Ok(rewards
        .into_iter()
        .map(|(item_id, count)| ItemParam { item_id, count })
        .collect())
}
fn claim_target(
    player: &mut b::PlayerDataBin,
    id: u32,
    output: &MessageOutput,
) -> Result<(), Retcode> {
    let def = ProgressionConfig::get()
        .targets
        .get(&id)
        .ok_or(Retcode::RetFail)?;
    if ProgressionConfig::get()
        .investigations
        .get(&def.investigation_id)
        .is_none_or(|i| i.unlock_level > player.basic_bin.as_ref().map_or(1, |b| b.level))
    {
        return Err(Retcode::RetPlayerLevelLessThan);
    }
    let state = player.investigation_bin.as_ref().unwrap();
    if state.reward_taken_target_id_list.contains(&id) {
        return Err(Retcode::RetRewardHasTaken);
    }
    if !state.finished_target_id_list.contains(&id) {
        return Err(Retcode::RetFail);
    }
    let rewards = reward_items([def.reward_id])?;
    let change = inventory::transact(player, &Amounts::new(), &rewards, &[])?;
    player
        .investigation_bin
        .as_mut()
        .unwrap()
        .reward_taken_target_id_list
        .push(id);
    inventory::notify_transaction(output, player, &change);
    Ok(())
}
fn claim_chapter(
    player: &mut b::PlayerDataBin,
    id: u32,
    output: &MessageOutput,
) -> Result<(), Retcode> {
    let def = ProgressionConfig::get()
        .investigations
        .get(&id)
        .ok_or(Retcode::RetFail)?;
    if def.unlock_level > player.basic_bin.as_ref().map_or(1, |b| b.level) {
        return Err(Retcode::RetPlayerLevelLessThan);
    }
    let state = player
        .investigation_bin
        .as_ref()
        .unwrap()
        .investigation_list
        .iter()
        .find(|r| r.investigation_id == id)
        .ok_or(Retcode::RetFail)?;
    if state.is_taken_reward {
        return Err(Retcode::RetRewardHasTaken);
    }
    if chapter_packet(player, state).state != 2 {
        return Err(Retcode::RetFail);
    }
    let rewards = reward_items([def.reward_id])?;
    let change = inventory::transact(player, &Amounts::new(), &rewards, &[])?;
    player
        .investigation_bin
        .as_mut()
        .unwrap()
        .investigation_list
        .iter_mut()
        .find(|r| r.investigation_id == id)
        .unwrap()
        .is_taken_reward = true;
    inventory::notify_transaction(output, player, &change);
    Ok(())
}

pub fn observe(
    owner: Res<WorldOwnerUID>,
    mut players: ResMut<Players>,
    output: Res<MessageOutput>,
    mut kills: MessageReader<MonsterKillEvent>,
    mut quests: MessageReader<QuestContentProgressEvent>,
    mut requests: MessageReader<ClientMessageEvent>,
    mut versions: Local<HashMap<u32, (u64, u32)>>,
) {
    let mut dirty = HashSet::new();
    for event in kills.read() {
        if let Some(player) = players.get_mut(owner.0) {
            record_progress(player, "kill", 0, 1);
            record_progress(player, "kill", event.monster_id, 1);
            dirty.insert(player.uid);
        }
    }
    for event in quests.read() {
        dirty.insert(event.player_uid);
    }
    for request in requests.read() {
        if matches!(
            request.message_name(),
            "AvatarUpgradeReq"
                | "AvatarPromoteReq"
                | "WeaponUpgradeReq"
                | "WeaponPromoteReq"
                | "ReliquaryUpgradeReq"
                | "WearEquipReq"
                | "AvatarSkillUpgradeReq"
                | "SceneTransToPointReq"
        ) {
            dirty.insert(request.sender_uid());
        }
    }
    for player in players.values_mut() {
        let version = (progress_counter(player, "revision", 0), day(now()));
        let previous = versions.insert(player.uid, version);
        if previous != Some(version) || dirty.contains(&player.uid) {
            if refresh(player, now()) {
                notify_pass(player, &output);
                notify_investigation(player, &output);
            }
            super::activity::notify(player, &output);
        }
    }
}
