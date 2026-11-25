use bevy_ecs::prelude::*;
use snezhnaya_gi_data::excel::{
    common::QuestState,
    progression::{AchievementConfig, ProgressionConfig, Trigger},
};
use snezhnaya_gi_inventory::transaction::{self as inventory, Amounts, InventoryResult};
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::{
    normal::{achievement::Status, *},
    retcode::Retcode,
    server_only::{AchievementBin, PlayerDataBin},
};
use std::collections::{HashMap, HashSet};

use super::progression;

fn states(player: &PlayerDataBin) -> impl Iterator<Item = &AchievementBin> {
    player
        .achievement_bin
        .iter()
        .flat_map(|bin| &bin.achievement_list)
}

fn status(entry: Option<&AchievementBin>) -> Status {
    match entry.map(|entry| entry.status) {
        Some(raw) if raw == Status::Finished as u32 => Status::Finished,
        Some(raw) if raw == Status::RewardTaken as u32 => Status::RewardTaken,
        _ => Status::Unfinished,
    }
}

fn packet(config: &AchievementConfig, entry: Option<&AchievementBin>) -> Achievement {
    let status = status(entry);
    Achievement {
        id: config.id,
        status: status as i32,
        finish_timestamp: entry.map_or(0, |entry| entry.finish_timestamp),
        total_progress: config.progress,
        cur_progress: if status == Status::Unfinished {
            entry.map_or(0, |entry| entry.cur_progress.min(config.progress))
        } else {
            config.progress
        },
    }
}

fn notify_updates(output: &MessageOutput, uid: u32, updates: Vec<Achievement>) {
    for batch in updates.chunks(128) {
        output.send(
            uid,
            "AchievementUpdateNotify",
            AchievementUpdateNotify {
                achievement_list: batch.to_vec(),
            },
        );
    }
}

fn update_progress<'a>(
    player: &mut PlayerDataBin,
    changes: impl IntoIterator<Item = (&'a AchievementConfig, u32)>,
    add: bool,
    now: u32,
) -> Vec<Achievement> {
    let mut changes = changes.into_iter().peekable();
    if changes.peek().is_none() {
        return Vec::new();
    }
    let entries = &mut player
        .achievement_bin
        .get_or_insert_with(Default::default)
        .achievement_list;
    let mut indices: HashMap<_, _> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.id, index))
        .collect();
    let mut updates = Vec::new();
    for (config, value) in changes {
        let index = *indices.entry(config.id).or_insert_with(|| {
            entries.push(AchievementBin {
                id: config.id,
                ..Default::default()
            });
            entries.len() - 1
        });
        let entry = &mut entries[index];
        if status(Some(entry)) != Status::Unfinished {
            continue;
        }

        let progress = if add {
            entry.cur_progress.saturating_add(value)
        } else {
            entry.cur_progress.max(value)
        }
        .min(config.progress);
        if progress == entry.cur_progress && progress < config.progress {
            continue;
        }

        entry.cur_progress = progress;
        entry.status = Status::Unfinished as u32;
        if progress == config.progress {
            entry.status = Status::Finished as u32;
            entry.finish_timestamp = now;
        }
        updates.push(packet(config, Some(entry)));
    }
    updates
}

// Only use counters whose parameters correspond to the state the server actually tracks.
fn server_trigger(trigger: &Trigger) -> bool {
    if trigger
        .param_list
        .iter()
        .skip(1)
        .any(|p| !p.trim().is_empty())
    {
        return false;
    }
    is_quest_trigger(trigger)
        || matches!(
            trigger.trigger_type.as_str(),
            "TRIGGER_UNLOCK_RECIPE"
                | "TRIGGER_AVATAR_PROMOTE"
                | "TRIGGER_WEAPON_PROMOTE"
                | "TRIGGER_OBTAIN_MATERIAL_NUM"
        )
}

fn is_quest_trigger(trigger: &Trigger) -> bool {
    matches!(
        trigger.trigger_type.as_str(),
        "TRIGGER_FINISH_QUEST_AND"
            | "TRIGGER_FINISH_QUEST_OR"
            | "TRIGGER_FINISH_PARENT_QUEST_AND"
            | "TRIGGER_FINISH_PARENT_QUEST_OR"
    )
}

fn quest_progress(player: &PlayerDataBin, trigger: &Trigger) -> u32 {
    let Some(quests) = &player.quest_bin else {
        return 0;
    };
    let Some(params) = trigger.param_list.first() else {
        return 0;
    };
    let Ok(ids) = params
        .split([',', ';'])
        .map(|id| id.trim().parse::<u32>())
        .collect::<Result<Vec<_>, _>>()
    else {
        return 0;
    };

    let parent = trigger.trigger_type.starts_with("TRIGGER_FINISH_PARENT_");
    let finished = |id: &u32| {
        let finished_state = QuestState::Finished as u32;
        if parent {
            quests
                .parent_quest_bin
                .as_ref()
                .and_then(|bin| bin.parent_quest_map.get(id))
                .is_some_and(|entry| entry.state == finished_state || entry.total_finish_count > 0)
        } else {
            quests
                .quest_bin
                .as_ref()
                .and_then(|bin| bin.quest_map.get(id))
                .is_some_and(|entry| entry.state == finished_state)
        }
    };
    (if trigger.trigger_type.ends_with("_AND") {
        ids.iter().all(finished)
    } else {
        ids.iter().any(finished)
    }) as u32
}

fn refresh(player: &mut PlayerDataBin, include_quests: bool) -> Vec<Achievement> {
    let changes: Vec<_> = ProgressionConfig::get()
        .achievements
        .values()
        .filter_map(|config| {
            let trigger = &config.trigger_config;
            if !server_trigger(trigger) {
                return None;
            }
            let value = if is_quest_trigger(trigger) {
                if !include_quests {
                    return None;
                }
                quest_progress(player, trigger)
            } else {
                progression::value(player, trigger)
            };
            (value > 0).then_some((config, value))
        })
        .collect();
    update_progress(player, changes, false, progression::now())
}

pub(super) fn observe(player: &mut PlayerDataBin, output: &MessageOutput, quest_changed: bool) {
    let updates = refresh(player, quest_changed);
    notify_updates(output, player.uid, updates);
}

pub(super) fn login(mut players: ResMut<Players>, output: Res<MessageOutput>) {
    for player in players.values_mut() {
        refresh(player, true);
        let saved: HashMap<_, _> = states(player).map(|entry| (entry.id, entry)).collect();
        let achievement_list: Vec<_> = ProgressionConfig::get()
            .achievements
            .values()
            .map(|config| packet(config, saved.get(&config.id).copied()))
            .collect();
        let watcher_list = achievement_list
            .iter()
            .filter(|entry| entry.status == Status::Unfinished as i32)
            .map(|entry| entry.id)
            .collect();
        output.send(
            player.uid,
            "AchievementAllDataNotify",
            AchievementAllDataNotify {
                achievement_list,
                reward_taken_goal_id_list: player
                    .achievement_bin
                    .as_ref()
                    .map_or_else(Vec::new, |bin| bin.reward_taken_goal_id_list.clone()),
            },
        );
        output.send(
            player.uid,
            "WatcherAllDataNotify",
            WatcherAllDataNotify { watcher_list },
        );
    }
}

fn params_match(trigger: &Trigger, params: &[u32]) -> bool {
    trigger
        .param_list
        .iter()
        .enumerate()
        .all(|(index, condition)| {
            condition.trim().is_empty()
                || params.get(index).is_some_and(|param| {
                    condition
                        .split(';')
                        .any(|choice| choice.trim().parse() == Ok(*param))
                })
        })
}

fn handle_watcher(
    player: &mut PlayerDataBin,
    message: &ClientMessageEvent,
    output: &MessageOutput,
) {
    let configs = &ProgressionConfig::get().achievements;
    let updates = if message.message_name() == "WatcherEventTypeNotify" {
        let Some(event) = message.decode::<WatcherEventTypeNotify>() else {
            return;
        };
        let Some(trigger) = watcher_trigger_name(event.watcher_trigger_type) else {
            return;
        };
        let changes = configs.values().filter_map(|config| {
            let condition = &config.trigger_config;
            (condition.trigger_type == trigger
                && !server_trigger(condition)
                && params_match(condition, &event.param_list))
            .then_some((config, event.add_progress.max(1)))
        });
        update_progress(player, changes, true, progression::now())
    } else {
        let decoded = if message.message_name() == "WatcherEventStageNotify" {
            message
                .decode::<WatcherEventStageNotify>()
                .map(|event| (event.watcher_id, event.add_progress))
        } else {
            message
                .decode::<WatcherEventNotify>()
                .map(|event| (event.watcher_id, event.add_progress))
        };
        let Some((id, amount)) = decoded else {
            return;
        };
        let Some(config) = configs
            .get(&id)
            .filter(|c| !server_trigger(&c.trigger_config))
        else {
            return;
        };
        update_progress(player, [(config, amount.max(1))], true, progression::now())
    };
    notify_updates(output, player.uid, updates);
}

fn claim(
    player: &mut PlayerDataBin,
    ids: &[u32],
    goal: bool,
    output: &MessageOutput,
) -> InventoryResult<Vec<ItemParam>> {
    if ids.is_empty() || ids.iter().collect::<HashSet<_>>().len() != ids.len() {
        return Err(Retcode::RetFail);
    }
    let config = ProgressionConfig::get();
    let bin = player.achievement_bin.as_ref().ok_or(Retcode::RetFail)?;
    let saved: HashMap<_, _> = states(player).map(|entry| (entry.id, entry)).collect();

    let mut rewards = Vec::with_capacity(ids.len());
    for &id in ids {
        if goal {
            let reward = *config
                .achievement_goal_rewards
                .get(&id)
                .ok_or(Retcode::RetFail)?;
            let mut members = config
                .achievements
                .values()
                .filter(|c| c.goal_id == id)
                .peekable();
            if bin.reward_taken_goal_id_list.contains(&id)
                || members.peek().is_none()
                || members.any(|c| status(saved.get(&c.id).copied()) == Status::Unfinished)
            {
                return Err(Retcode::RetFail);
            }
            rewards.push(reward);
        } else {
            let row = config.achievements.get(&id).ok_or(Retcode::RetFail)?;
            if status(saved.get(&id).copied()) != Status::Finished {
                return Err(Retcode::RetFail);
            }
            rewards.push(row.finish_reward_id);
        }
    }
    let rewards = inventory::reward_items(rewards)?;
    if rewards.is_empty() {
        return Err(Retcode::RetFail);
    }
    let change = inventory::transact(player, &Amounts::new(), &rewards, &[])?;
    let bin = player.achievement_bin.as_mut().unwrap();
    if goal {
        bin.reward_taken_goal_id_list.extend_from_slice(ids);
    } else {
        let claimed: HashSet<_> = ids.iter().copied().collect();
        let mut updates = Vec::with_capacity(ids.len());
        for entry in &mut bin.achievement_list {
            if claimed.contains(&entry.id) {
                entry.status = Status::RewardTaken as u32;
                updates.push(packet(&config.achievements[&entry.id], Some(entry)));
            }
        }
        notify_updates(output, player.uid, updates);
    }
    inventory::notify_transaction(output, player, &change);
    Ok(rewards
        .into_iter()
        .map(|(item_id, count)| ItemParam { item_id, count })
        .collect())
}

pub(super) fn handle(
    mut events: MessageReader<ClientMessageEvent>,
    mut players: ResMut<Players>,
    output: Res<MessageOutput>,
) {
    for message in events.read() {
        let Some(player) = players.get_mut(message.sender_uid()) else {
            continue;
        };
        match message.message_name() {
            "WatcherEventNotify" | "WatcherEventStageNotify" | "WatcherEventTypeNotify" => {
                handle_watcher(player, message, &output);
            }
            "TakeAchievementRewardReq" => {
                let Some(request) = message.decode::<TakeAchievementRewardReq>() else {
                    continue;
                };
                let response = match claim(player, &request.id_list, false, &output) {
                    Ok(item_list) => TakeAchievementRewardRsp {
                        id_list: request.id_list,
                        item_list,
                        retcode: 0,
                    },
                    Err(code) => TakeAchievementRewardRsp {
                        retcode: code.into(),
                        ..Default::default()
                    },
                };
                output.send(player.uid, "TakeAchievementRewardRsp", response);
            }
            "TakeAchievementGoalRewardReq" => {
                let Some(request) = message.decode::<TakeAchievementGoalRewardReq>() else {
                    continue;
                };
                let response = match claim(player, &request.id_list, true, &output) {
                    Ok(item_list) => TakeAchievementGoalRewardRsp {
                        id_list: request.id_list,
                        item_list,
                        retcode: 0,
                    },
                    Err(code) => TakeAchievementGoalRewardRsp {
                        retcode: code.into(),
                        ..Default::default()
                    },
                };
                output.send(player.uid, "TakeAchievementGoalRewardRsp", response);
            }
            _ => {}
        }
    }
}

// Trigger IDs used by active 7.2 achievements, from the existing AstaPS watcher enum.
fn watcher_trigger_name(id: u32) -> Option<&'static str> {
    Some(match id {
        1 => "TRIGGER_COMBAT_CONFIG_COMMON",
        10 => "TRIGGER_PAIMON_ANGRY_VOICE_EASTER_EGG",
        104 => "TRIGGER_UNLOCK_GATE_TEMPLE",
        105 => "TRIGGER_UNLOCK_AREA",
        106 => "TRIGGER_UNLOCK_TRANS_POINT",
        107 => "TRIGGER_OPEN_CHEST_WITH_GADGET_ID",
        108 => "TRIGGER_CITY_LEVEL_UP",
        111 => "TRIGGER_GROUP_NOTIFY",
        112 => "TRIGGER_ELEMENT_TYPE_CHANGE",
        113 => "TRIGGER_GADGET_INTERACTABLE",
        114 => "TRIGGER_COLLECT_SET_OF_READINGS",
        115 => "TRIGGER_TELEPORT_WITH_CERTAIN_PORTAL",
        118 => "TRIGGER_BATTLE_FOR_MONSTER_DIE_OR",
        119 => "TRIGGER_BATTLE_FOR_MONSTER_DIE_AND",
        120 => "TRIGGER_OPEN_WORLD_CHEST",
        122 => "TRIGGER_UNLOCK_SCENE_POINT",
        123 => "TRIGGER_INTERACT_GADGET_WITH_INTERACT_ID",
        204 => "TRIGGER_AVATAR_PROMOTE",
        206 => "TRIGGER_WEAPON_PROMOTE",
        210 => "TRIGGER_UNLOCK_RECIPE",
        212 => "TRIGGER_OBTAIN_MATERIAL_NUM",
        213 => "TRIGGER_OBTAIN_RELIQUARY_NUM",
        216 => "TRIGGER_FETTER_LEVEL_AVATAR_NUM",
        217 => "TRIGGER_SKILLED_AT_RECIPE",
        218 => "TRIGGER_RELIQUARY_UPGRADE_EQUAL_RANK_LEVEL",
        221 => "TRIGGER_UNLOCK_SPECIFIC_RECIPE_OR",
        229 => "TRIGGER_UNLOCK_SPECIFIC_ANIMAL_CODEX",
        230 => "TRIGGER_OBTAIN_ITEM_NUM",
        231 => "TRIGGER_CAPTURE_ANIMAL",
        304 => "TRIGGER_FINISH_TOWER_LEVEL",
        309 => "TRIGGER_OPEN_BLOSSOM_CHEST",
        311 => "TRIGGER_DONE_TOWER_GADGET_UNHURT",
        312 => "TRIGGER_DONE_TOWER_STARS",
        313 => "TRIGGER_DONE_TOWER_UNHURT",
        314 => "TRIGGER_STEAL_FOOD_TIMES",
        315 => "TRIGGER_DONE_DUNGEON_WITH_SAME_ELEMENT_AVATARS",
        332 => "TRIGGER_GROUP_VARIABLE_SET_VALUE_TO",
        336 => "TRIGGER_ENTER_VEHICLE",
        337 => "TRIGGER_VEHICLE_DURATION",
        338 => "TRIGGER_VEHICLE_FRIENDS",
        339 => "TRIGGER_VEHICLE_KILLED_BY_MONSTER",
        340 => "TRIGGER_VEHICLE_DASH",
        401 => "TRIGGER_DO_COOK",
        405 => "TRIGGER_BUY_SHOP_GOODS",
        406 => "TRIGGER_FORGE_WEAPON",
        424 => "TRIGGER_MP_DUNGEON_TIMES",
        425 => "TRIGGER_MP_KILL_MONSTER_NUM",
        437 => "TRIGGER_COMBINE_ITEM",
        438 => "TRIGGER_FINISH_CHALLENGE_IN_DURATION",
        440 => "TRIGGER_MP_KILL_MONSTER_ID_NUM",
        504 => "TRIGGER_USE_ITEM",
        700 => "TRIGGER_FINISH_QUEST_AND",
        701 => "TRIGGER_FINISH_QUEST_OR",
        702 => "TRIGGER_DAILY_TASK_VAR_EQUAL",
        703 => "TRIGGER_QUEST_GLOBAL_VAR_EQUAL",
        704 => "TRIGGER_TALK_NUM",
        705 => "TRIGGER_FINISH_PARENT_QUEST_AND",
        706 => "TRIGGER_FINISH_PARENT_QUEST_OR",
        800 => "TRIGGER_ELEMENT_REACTION_TIMELIMIT_NUM",
        801 => "TRIGGER_ELEMENT_REACTION_TIMELIMIT_KILL_NUM",
        803 => "TRIGGER_ABILITY_STATE_PASS_TIME",
        804 => "TRIGGER_MAX_CRITICAL_DAMAGE",
        805 => "TRIGGER_FULL_SATIATION_TEAM_AVATAR_NUM",
        806 => "TRIGGER_KILLED_BY_CERTAIN_MONSTER",
        809 => "TRIGGER_USE_ENERGY_SKILL_NUM_TIMELIMIT",
        810 => "TRIGGER_SHIELD_SOURCE_NUM",
        811 => "TRIGGER_CUR_AVATAR_HURT_BY_SPECIFIC_ABILITY",
        812 => "TRIGGER_KILLED_BY_SPECIFIC_ABILITY",
        900 => "TRIGGER_MAX_DASH_TIME",
        901 => "TRIGGER_MAX_FLY_TIME",
        902 => "TRIGGER_MAX_FLY_MAP_DISTANCE",
        903 => "TRIGGER_SIT_DOWN_IN_POINT",
        930 => "TRIGGER_CITY_REPUTATION_LEVEL",
        931 => "TRIGGER_CITY_REPUTATION_FINISH_REQUEST",
        932 => "TRIGGER_HUNTING_FINISH_NUM",
        933 => "TRIGGER_HUNTING_FAIL_NUM",
        934 => "TRIGGER_OFFERING_LEVEL",
        936 => "TRIGGER_MIRACLE_RING_TAKE_REWARD",
        942 => "TRIGGER_MAIN_COOP_SAVE_POINT_AND",
        944 => "TRIGGER_MAIN_COOP_VAR_EQUAL",
        1002 => "TRIGGER_UNLOCK_FURNITURE_COUNT",
        1003 => "TRIGGER_FURNITURE_MAKE",
        1004 => "TRIGGER_HOME_LEVEL",
        1005 => "TRIGGER_HOME_COIN",
        1009 => "TRIGGER_ARRANGEMENT_FURNITURE_COUNT",
        1010 => "TRIGGER_ENTER_SELF_HOME",
        1011 => "TRIGGER_HOME_MODULE_COMFORT_VALUE",
        1016 => "TRIGGER_HOME_AVATAR_REWARD_EVENT_ALL_COUNT",
        1017 => "TRIGGER_HOME_AVATAR_TALK_FINISH_ALL_COUNT",
        1019 => "TRIGGER_HOME_AVATAR_IN_COUNT",
        1021 => "TRIGGER_ARRANGEMENT_FURNITURE",
        1023 => "TRIGGER_HOME_FIELD_GATHER_COUNT",
        1024 => "TRIGGER_HOME_UNLOCK_BGM_COUNT",
        1100 => "TRIGGER_FISHING_SUCC_NUM",
        1101 => "TRIGGER_FISHING_KEEP_BONUS",
        1103 => "TRIGGER_FISHING_FAIL_NUM",
        1104 => "TRIGGER_SHOCK_FISH_NUM",
        _ => return None,
    })
}
