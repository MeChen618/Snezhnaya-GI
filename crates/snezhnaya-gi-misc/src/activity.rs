use super::progression::{now, reward_items, value};
use bevy_ecs::prelude::*;
use common::game_server_config::ActivitySettings;
use serde_json::{json, Value};
use snezhnaya_gi_data::{
    excel::progression::{ChallengeSeason, ProgressionConfig},
    GAME_SERVER_CONFIG,
};
use snezhnaya_gi_inventory::transaction::{self as inventory, Amounts};
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::{
    retcode::Retcode,
    server_only::{self as b},
};
use std::collections::HashSet;

const LEYLINE: u32 = 5269;
fn number(value: &Value, key: &str) -> u32 {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
}
pub(super) fn active_season(time: u32) -> Option<&'static ChallengeSeason> {
    let cfg = ProgressionConfig::get();
    cfg.challenge_seasons.values().rev().find(|s| {
        s.begin().is_some_and(|b| b <= time)
            && s.end(cfg.challenge_duration).is_some_and(|e| time < e)
    })
}
fn schedules() -> Vec<ActivitySettings> {
    let cfg = ProgressionConfig::get();
    let mut ids = HashSet::new();
    let mut result: Vec<_> = GAME_SERVER_CONFIG
        .activities
        .iter()
        .filter(|a| {
            a.activity_id != LEYLINE
                && a.activity_type != 0
                && a.schedule_id != 0
                && a.begin_time < a.end_time
                && cfg.activities.contains_key(&a.activity_id)
                && ids.insert(a.activity_id)
        })
        .cloned()
        .collect();
    if let Some(s) = active_season(now()).or_else(|| {
        cfg.challenge_seasons
            .values()
            .rev()
            .find(|s| s.begin().is_some_and(|b| b <= now()))
    }) {
        if let (Some(begin), Some(end)) = (s.begin(), s.end(cfg.challenge_duration)) {
            result.push(ActivitySettings {
                activity_id: LEYLINE,
                activity_type: 5701,
                schedule_id: s.schedule_id,
                begin_time: begin,
                end_time: end,
            });
        }
    }
    result.sort_by_key(|a| a.activity_id);
    result
}
fn is_open(a: &ActivitySettings) -> bool {
    a.begin_time <= now() && now() < a.end_time
}
fn watcher_ids(a: &ActivitySettings) -> Vec<u32> {
    let cfg = ProgressionConfig::get();
    let mut ids = cfg
        .activities
        .get(&a.activity_id)
        .map_or(vec![], |a| a.watcher_id.clone());
    if a.activity_id == LEYLINE {
        if let Some(s) = cfg.challenge_seasons.get(&a.schedule_id) {
            ids.push(s.reward_watcher_id);
            for id in &s.level_id_list {
                if let Some(level) = cfg.challenge_levels.get(id) {
                    ids.extend(&level.watcher_list);
                }
            }
        }
    }
    ids.sort_unstable();
    ids.dedup();
    ids.retain(|id| cfg.activity_watchers.contains_key(id));
    ids
}
fn ensure(player: &mut b::PlayerDataBin, a: &ActivitySettings) {
    let reset = player
        .activity_bin
        .as_ref()
        .and_then(|b| b.activity_bin_map.get(&a.activity_id))
        .is_none_or(|s| s.schedule_id != a.schedule_id);
    if reset {
        let bases = watcher_ids(a)
            .into_iter()
            .filter_map(|id| {
                let def = ProgressionConfig::get().activity_watchers.get(&id)?;
                Some((id, super::progression::counter(player, &def.trigger_config)))
            })
            .collect();
        let state = player
            .activity_bin
            .get_or_insert_with(Default::default)
            .activity_bin_map
            .entry(a.activity_id)
            .or_default();
        let mut ley = state.ley_line_challenge.take();
        if let Some(ley) = ley.as_mut() {
            ley.teams.clear();
        }
        *state = b::ActivityBin {
            schedule_id: a.schedule_id,
            watcher_counter_base_map: bases,
            ley_line_challenge: ley,
            ..Default::default()
        };
    }
}
fn refresh(player: &mut b::PlayerDataBin, a: &ActivitySettings) {
    ensure(player, a);
    if !is_open(a) {
        return;
    }
    let values: Vec<_> = watcher_ids(a)
        .into_iter()
        .filter_map(|id| {
            let def = ProgressionConfig::get().activity_watchers.get(&id)?;
            let current = value(player, &def.trigger_config);
            let baseline = player
                .activity_bin
                .as_ref()?
                .activity_bin_map
                .get(&a.activity_id)?
                .watcher_counter_base_map
                .get(&id)
                .copied()
                .unwrap_or(0);
            let progress = if super::progression::counter_key(&def.trigger_config).is_some() {
                super::progression::counter(player, &def.trigger_config)
                    .saturating_sub(baseline)
                    .min(u64::from(def.progress)) as u32
            } else {
                current.min(def.progress)
            };
            Some((id, progress, def.progress))
        })
        .collect();
    let state = player
        .activity_bin
        .as_mut()
        .unwrap()
        .activity_bin_map
        .get_mut(&a.activity_id)
        .unwrap();
    for (id, progress, total) in values {
        let entry = state.watcher_progress_map.entry(id).or_default();
        *entry = (*entry).max(progress);
        if total > 0 && *entry >= total && !state.finished_watcher_id_list.contains(&id) {
            state.finished_watcher_id_list.push(id);
        }
    }
}
pub(super) fn ley_state(player: &b::PlayerDataBin) -> Option<&b::LeyLineChallengeBin> {
    player
        .activity_bin
        .as_ref()?
        .activity_bin_map
        .get(&LEYLINE)?
        .ley_line_challenge
        .as_ref()
}
pub(super) fn ley_state_mut(player: &mut b::PlayerDataBin) -> &mut b::LeyLineChallengeBin {
    player
        .activity_bin
        .get_or_insert_with(Default::default)
        .activity_bin_map
        .entry(LEYLINE)
        .or_default()
        .ley_line_challenge
        .get_or_insert_with(Default::default)
}
fn selected(player: &b::PlayerDataBin) -> u32 {
    let choices = &ProgressionConfig::get().challenge_reliquaries;
    ley_state(player)
        .map(|s| s.reliquary_id)
        .filter(|id| choices.contains(id))
        .or_else(|| choices.iter().min().copied())
        .unwrap_or(0)
}
pub(super) fn ley_data(player: &b::PlayerDataBin) -> Value {
    json!({"DNIAFBOHIPC":super::ley_line::max_completed(player),"GLKEFHEKCPO":super::ley_line::max_completed(player),"FODFAOBLHEA":ley_state(player).into_iter().flat_map(|p|p.records.iter()).flat_map(|(&schedule,r)|r.records.keys().filter_map(move|&difficulty|super::ley_line::record_packet(player,schedule,difficulty))).collect::<Vec<_>>(),"activity_schedule_id":active_season(now()).map_or(0,|s|s.schedule_id),"reliquary_id":selected(player),"favorite_reliquary_id_list":ley_state(player).map_or(vec![],|s|s.favorite_reliquary_ids.clone())})
}
fn info(player: &b::PlayerDataBin, a: &ActivitySettings) -> Value {
    let state = &player.activity_bin.as_ref().unwrap().activity_bin_map[&a.activity_id];
    let watchers:Vec<_>=watcher_ids(a).into_iter().map(|id|{let def=&ProgressionConfig::get().activity_watchers[&id];json!({"watcher_id":id,"cur_progress":state.watcher_progress_map.get(&id).copied().unwrap_or(0),"total_progress":def.progress,"is_taken_reward":state.taken_watcher_id_list.contains(&id)})}).collect();
    let mut data = json!({"activity_id":a.activity_id,"activity_type":a.activity_type,"schedule_id":a.schedule_id,"begin_time":a.begin_time,"end_time":a.end_time,"first_day_start_time":a.begin_time,"is_finished":!is_open(a),"is_play_open_anim":state.is_play_open_anim,"watcher_info_list":watchers});
    if a.activity_id == LEYLINE {
        let season = &ProgressionConfig::get().challenge_seasons[&a.schedule_id];
        data["first_day_start_time"] = json!(season.first_day().unwrap_or(a.begin_time));
        let selections:Vec<_>=ley_state(player).into_iter().flat_map(|s|s.teams.iter()).flat_map(|(&difficulty,teams)|teams.team_list.iter().enumerate().map(move|(round,team)|json!({"difficulty":difficulty,"CIPECILEKHK":round+1,"avatar_id_list":team.avatar_ids}))).collect();
        data["ley_line_challenge_detail_info"] = json!({"HGEEJFIBCLH":ley_data(player),"OFBPKKNJJFN":true,"NDBOPNDMGNM":true,"HNBNPOEHDAA":cn_end(season).to_string(),"difficulty_info_list":(1..=6).map(|d|json!({"difficulty":d})).collect::<Vec<_>>(),"LDCNHAOBINH":selections});
        data["ley_line_challenge_detail_info"]["difficulty_info_list"]=json!((1..=6).map(|d|json!({"difficulty":d,"DNIJJMFOJLI":ley_state(player).and_then(|l|l.records.get(&a.schedule_id)).and_then(|r|r.records.get(&d)).map_or(vec![],|r|r.round_time_ms.iter().map(|ms|ms.div_ceil(1000)).collect())})).collect::<Vec<_>>());
    }
    data
}
fn cn_end(s: &ChallengeSeason) -> u64 {
    u64::from(
        snezhnaya_gi_data::excel::progression::cn_timestamp(&s.schedule_end_time).unwrap_or(0),
    )
}
pub(super) fn notify(player: &mut b::PlayerDataBin, output: &MessageOutput) {
    for a in schedules() {
        refresh(player, &a);
        output.send_json(
            player.uid,
            "ActivityInfoNotify",
            json!({"activity_info":info(player,&a)}),
        );
    }
}
pub fn login(mut players: ResMut<Players>, output: Res<MessageOutput>) {
    let schedules = schedules();
    for player in players.values_mut() {
        for a in &schedules {
            refresh(player, a);
        }
        output.send_json(player.uid,"ActivityScheduleInfoNotify",json!({"activity_schedule_list":schedules.iter().map(|a|json!({"activity_id":a.activity_id,"schedule_id":a.schedule_id,"begin_time":a.begin_time,"end_time":a.end_time,"is_open":is_open(a)})).collect::<Vec<_>>() }));
        output.send_json(
            player.uid,
            "LeyLineChallengeDataNotify",
            json!({"data":ley_data(player)}),
        );
    }
}
fn claim(
    player: &mut b::PlayerDataBin,
    activity: u32,
    watcher: u32,
    output: &MessageOutput,
) -> Result<(), Retcode> {
    let a = schedules()
        .into_iter()
        .find(|a| a.activity_id == activity && is_open(a))
        .ok_or(Retcode::RetActivityClose)?;
    refresh(player, &a);
    if !watcher_ids(&a).contains(&watcher) {
        return Err(Retcode::RetFail);
    }
    let state = &player.activity_bin.as_ref().unwrap().activity_bin_map[&activity];
    if state.taken_watcher_id_list.contains(&watcher) {
        return Err(Retcode::RetActivityWatcherRewardTaken);
    }
    if !state.finished_watcher_id_list.contains(&watcher) {
        return Err(Retcode::RetActivityWatcherRewardNotFinished);
    }
    let def = &ProgressionConfig::get().activity_watchers[&watcher];
    if def.reward_id == 0 {
        return Err(Retcode::RetNotFoundConfig);
    }
    let rewards = reward_items([def.reward_id])?;
    let change = inventory::transact(player, &Amounts::new(), &rewards, &[])?;
    player
        .activity_bin
        .as_mut()
        .unwrap()
        .activity_bin_map
        .get_mut(&activity)
        .unwrap()
        .taken_watcher_id_list
        .push(watcher);
    inventory::notify_transaction(output, player, &change);
    Ok(())
}
fn save_teams(player: &mut b::PlayerDataBin, request: &Value) -> Result<(), Retcode> {
    let difficulty = number(request, "difficulty");
    if active_season(now()).is_none() {
        return Err(Retcode::RetActivityClose);
    }
    if !(1..=6).contains(&difficulty) {
        return Err(Retcode::RetFail);
    }
    let saved = parse_teams(player, request)?;
    ley_state_mut(player).teams.insert(difficulty, saved);
    Ok(())
}
pub(super) fn parse_teams(
    player: &b::PlayerDataBin,
    request: &Value,
) -> Result<b::LeyLineChallengeTeamsBin, Retcode> {
    let teams = request
        .get("team_list")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if teams.len() > 3 {
        return Err(Retcode::RetFail);
    }
    let mut saved = vec![];
    for team in teams {
        let mut used = HashSet::new();
        let avatars = team
            .get("avatar_list")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if avatars.len() > 4 {
            return Err(Retcode::RetFail);
        }
        let mut ids = vec![];
        for avatar in avatars {
            let id = number(avatar, "avatar_id");
            if avatar.get("is_trial_avatar").and_then(Value::as_bool) == Some(true)
                || !used.insert(id)
                || !player
                    .avatar_bin
                    .as_ref()
                    .is_some_and(|b| b.avatar_map.values().any(|a| a.avatar_id == id))
            {
                return Err(Retcode::RetCanNotFindAvatar);
            }
            ids.push(id);
        }
        saved.push(b::LeyLineChallengeTeamBin { avatar_ids: ids });
    }
    saved.resize_with(3, Default::default);
    Ok(b::LeyLineChallengeTeamsBin { team_list: saved })
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
            "GetActivityInfoReq"
                | "GetActivityScheduleReq"
                | "ActivityTakeWatcherRewardReq"
                | "ActivityPlayOpenAnimNotify"
                | "LeyLineChallengeGetDataReq"
                | "LeyLineChallengeGetReliquaryListReq"
                | "LeyLineChallengeSelectReliquaryReq"
                | "LeyLineChallengeSetFavoriteReliquaryReq"
                | "LeyLineChallengeSetTeamReq"
                | "LeyLineChallengeGetPopularAvatarReq"
        ) {
            continue;
        }
        let Some(player) = players.get_mut(message.sender_uid()) else {
            continue;
        };
        let Some(request) = message.decode_json() else {
            continue;
        };
        match name {
            "GetActivityInfoReq" => {
                let ids: HashSet<_> = request
                    .get("activity_id_list")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_u64)
                    .collect();
                let mut infos = vec![];
                for a in schedules() {
                    if ids.contains(&u64::from(a.activity_id)) {
                        refresh(player, &a);
                        infos.push(info(player, &a));
                    }
                }
                output.send_json(
                    player.uid,
                    "GetActivityInfoRsp",
                    json!({"activity_info_list":infos}),
                );
            }
            "ActivityTakeWatcherRewardReq" => {
                let activity = number(&request, "activity_id");
                let watcher = number(&request, "watcher_id");
                let result = claim(player, activity, watcher, &output);
                output.send_json(player.uid,"ActivityTakeWatcherRewardRsp",json!({"activity_id":activity,"watcher_id":watcher,"retcode":result.err().map(i32::from).unwrap_or(0)}));
                notify(player, &output);
            }
            "ActivityPlayOpenAnimNotify" => {
                let id = number(&request, "activity_id");
                if let Some(a) = schedules().iter().find(|a| a.activity_id == id) {
                    ensure(player, a);
                    player
                        .activity_bin
                        .as_mut()
                        .unwrap()
                        .activity_bin_map
                        .get_mut(&id)
                        .unwrap()
                        .is_play_open_anim = true;
                }
            }
            "LeyLineChallengeGetDataReq" => output.send_json(
                player.uid,
                "LeyLineChallengeGetDataRsp",
                json!({"data":ley_data(player)}),
            ),
            "LeyLineChallengeGetReliquaryListReq" => output.send_json(
                player.uid,
                "LeyLineChallengeGetReliquaryListRsp",
                json!({"reliquary_id_list":ProgressionConfig::get().challenge_reliquaries}),
            ),
            "LeyLineChallengeSelectReliquaryReq" => {
                let id = number(&request, "reliquary_id");
                let code = if ProgressionConfig::get().challenge_reliquaries.contains(&id) {
                    ley_state_mut(player).reliquary_id = id;
                    0
                } else {
                    Retcode::RetFail.into()
                };
                output.send_json(
                    player.uid,
                    "LeyLineChallengeSelectReliquaryRsp",
                    json!({"retcode":code}),
                );
                output.send_json(
                    player.uid,
                    "LeyLineChallengeDataUpdateNotify",
                    json!({"data":ley_data(player)}),
                );
            }
            "LeyLineChallengeSetFavoriteReliquaryReq" => {
                {
                    let ids = request
                        .get("reliquary_id_list")
                        .and_then(Value::as_array)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    let ids: Option<Vec<u32>> = ids
                        .iter()
                        .map(|v| v.as_u64().and_then(|n| u32::try_from(n).ok()))
                        .collect();
                    if let Some(mut ids) = ids {
                        if ids.len() <= ProgressionConfig::get().challenge_reliquaries.len()
                            && ids
                                .iter()
                                .all(|i| ProgressionConfig::get().challenge_reliquaries.contains(i))
                        {
                            ids.sort_unstable();
                            ids.dedup();
                            ley_state_mut(player).favorite_reliquary_ids = ids;
                        }
                    }
                }
                output.send_json(
                    player.uid,
                    "LeyLineChallengeDataUpdateNotify",
                    json!({"data":ley_data(player)}),
                );
            }
            "LeyLineChallengeSetTeamReq" => {
                let result = save_teams(player, &request);
                output.send_json(
                    player.uid,
                    "LeyLineChallengeSetTeamRsp",
                    json!({"retcode":result.err().map(i32::from).unwrap_or(0)}),
                );
            }
            "LeyLineChallengeGetPopularAvatarReq" => output.send_json(
                player.uid,
                "LeyLineChallengeGetPopularAvatarRsp",
                json!({"GLKEFHEKCPO":number(&request,"glkefhekcpo")}),
            ),
            _ => {}
        }
    }
}
