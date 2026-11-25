use common::time_util::cn_game_day;
use serde_json::json;
use snezhnaya_gi_data::excel::{common::QuestState, daily_task::DailyTaskConfig};
use snezhnaya_gi_message::output::MessageOutput;
use snezhnaya_gi_proto::server_only::PlayerDataBin;

pub const MAX_REWARDS: u32 = 4;
pub const REWARD_GROUP: u32 = 2;

const POINTS_PER_REWARD: u32 = 2000;
const RESIN_PER_REWARD: u32 = 30;
// 7.2 的长期历练点重置时间，用于离线跨期存档。
pub const ACCUMULATED_RESET_TIME: u32 = 1_820_962_800;

pub fn unlocked(player: &PlayerDataBin) -> bool {
    player.basic_bin.as_ref().is_some_and(|basic| {
        basic.level >= 24
            || [4104, 4105].iter().any(|id| {
                basic
                    .open_state_map
                    .get(id)
                    .is_some_and(|&state| state != 0)
            })
    }) || player
        .quest_bin
        .as_ref()
        .and_then(|quests| quests.quest_bin.as_ref())
        .and_then(|quests| quests.quest_map.get(&202102))
        .is_some_and(|quest| quest.state == QuestState::Finished as u32)
}

pub fn refresh(player: &mut PlayerDataBin, time: u32) -> bool {
    if !unlocked(player) {
        return false;
    }
    let state = player
        .daily_attendance_bin
        .get_or_insert_with(Default::default);
    let mut changed = false;
    let day = cn_game_day(time);
    if day > state.game_day {
        state.game_day = day;
        state.current_points = 0;
        state.claimed_count = 0;
        state.resin_remainder = 0;
        changed = true;
    }
    if time >= ACCUMULATED_RESET_TIME && state.last_accumulated_reset < ACCUMULATED_RESET_TIME {
        state.accumulated_points = 0;
        state.last_accumulated_reset = ACCUMULATED_RESET_TIME;
        changed = true;
    }
    changed
}

fn commission_count(player: &PlayerDataBin, time: u32) -> u32 {
    player
        .daily_task_bin
        .as_ref()
        .filter(|state| state.last_time != 0 && cn_game_day(state.last_time) == cn_game_day(time))
        .map_or(0, |state| state.finished_num.min(MAX_REWARDS))
}

pub fn finished_count(player: &PlayerDataBin, time: u32) -> u32 {
    let claimed = player
        .daily_attendance_bin
        .as_ref()
        .filter(|state| state.game_day == cn_game_day(time))
        .map_or(0, |state| state.claimed_count);
    commission_count(player, time)
        .saturating_add(claimed)
        .min(MAX_REWARDS)
}

pub fn claimable(player: &PlayerDataBin, time: u32) -> u32 {
    let Some(state) = &player.daily_attendance_bin else {
        return 0;
    };
    if state.game_day != cn_game_day(time) {
        return 0;
    }
    (state.current_points / POINTS_PER_REWARD)
        .min(MAX_REWARDS - commission_count(player, time))
        .saturating_sub(state.claimed_count)
}

pub(crate) fn spend_resin(player: &mut PlayerDataBin, amount: u32, time: u32) -> bool {
    if amount == 0 || !unlocked(player) {
        return false;
    }
    refresh(player, time);
    let target = (MAX_REWARDS - commission_count(player, time)) * POINTS_PER_REWARD;
    let state = player.daily_attendance_bin.as_mut().unwrap();
    if state.game_day != cn_game_day(time) {
        return false;
    }

    state.resin_remainder = state.resin_remainder.saturating_add(amount);
    let count = (target.saturating_sub(state.current_points) / POINTS_PER_REWARD)
        .min(state.resin_remainder / RESIN_PER_REWARD)
        .min(state.accumulated_points / POINTS_PER_REWARD);
    state.resin_remainder -= count * RESIN_PER_REWARD;
    state.accumulated_points -= count * POINTS_PER_REWARD;
    state.current_points += count * POINTS_PER_REWARD;
    true
}

pub fn notify(player: &PlayerDataBin, output: &MessageOutput) {
    let Some(state) = &player.daily_attendance_bin else {
        return;
    };
    let level = player.basic_bin.as_ref().map_or(0, |basic| basic.level);
    let reward_id = DailyTaskConfig::get()
        .reward_at_level(REWARD_GROUP, level)
        .map_or(0, |reward| reward.preview_reward_id);
    let next_reset = if state.last_accumulated_reset < ACCUMULATED_RESET_TIME {
        ACCUMULATED_RESET_TIME
    } else {
        0
    };
    output.send_json(
        player.uid,
        "DailyAttendanceDataNotify",
        json!({
            "daily_attendance_reward_id": reward_id,
            "daily_attendance_accumulated_points": state.accumulated_points,
            "daily_attendance_accumulated_reset_time": next_reset,
            "daily_attendance_total_claimed_rewards": state.claimed_count,
            "daily_attendance_max_points": MAX_REWARDS * POINTS_PER_REWARD,
            "daily_attendance_cur_points": state.current_points,
            "daily_attendance_resin_remainder": state.resin_remainder,
        }),
    );
}
