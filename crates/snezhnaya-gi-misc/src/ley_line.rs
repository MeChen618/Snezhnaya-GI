use super::{activity, progression::now, SceneTime};
use bevy_ecs::{prelude::*, system::SystemParam};
use serde_json::{json, Value};
use snezhnaya_gi_data::excel::{
    self,
    ley_line_challenge::{Arena, LeyLineConfig, Monster},
    progression::{ChallengeLevel, ProgressionConfig},
};
use snezhnaya_gi_data::prop_type::FightPropType as F;
use snezhnaya_gi_entity::{
    common::*,
    gadget::spawn_gadget_entity,
    monster::{spawn_monster_entity, AffixList, MonsterEquipment, MonsterSpawnInfo},
    transform::Transform,
};
use snezhnaya_gi_event::{ability::ServerInvokeEvent, lua::MonsterKillEvent, scene::*};
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::{dy_parser, normal as n, retcode::Retcode, server_only as b};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

const ACTIVITY: u32 = 5269;
#[derive(Clone, Copy)]
struct Actor {
    id: u32,
    entity: Entity,
}
struct Session {
    owner: u32,
    schedule: u32,
    difficulty: u32,
    arena: Arena,
    round: usize,
    teams: b::LeyLineChallengeTeamsBin,
    original: Vec<u64>,
    original_guid: u64,
    stage: u32,
    ready: bool,
    start_pending: bool,
    started: Instant,
    stage_time: u64,
    scene_start: u32,
    transaction: String,
    key: Option<Actor>,
    boss: Option<Actor>,
    summons: HashMap<u32, Actor>,
    actions: HashSet<[u32; 4]>,
    costs: [u32; 3],
}
impl Session {
    fn completed(&self) -> bool {
        self.costs.iter().all(|t| *t > 0)
    }
    fn limit(&self) -> u32 {
        if self.difficulty < 4 {
            600
        } else {
            120
        }
    }
    fn level(&self) -> &'static ChallengeLevel {
        let p = ProgressionConfig::get();
        let id = p.challenge_seasons[&self.schedule]
            .level_id_list
            .iter()
            .find(|id| p.challenge_levels[*id].difficulty == self.difficulty)
            .unwrap();
        &p.challenge_levels[id]
    }
    fn monster(&self) -> &'static Monster {
        &LeyLineConfig::get().monsters[&self.level().level_config_list[self.round]]
    }
}
#[derive(Resource, Default)]
pub struct Challenges {
    session: Option<Session>,
    wake: Option<Instant>,
}
pub fn next_wakeup(world: &World) -> Option<Instant> {
    let state = world.get_resource::<Challenges>()?;
    [
        state.wake,
        state
            .session
            .as_ref()
            .filter(|s| s.stage == 2)
            .map(|s| s.started + Duration::from_secs(u64::from(s.limit()))),
    ]
    .into_iter()
    .flatten()
    .min()
}
#[derive(Component)]
pub(super) struct Scaling {
    hp: f32,
    attack: f32,
}
#[derive(Component)]
pub(super) struct Scaled;
pub fn apply_monster_scaling(
    mut commands: Commands,
    mut monsters: Query<(Entity, &Scaling, &mut FightProperties), Without<Scaled>>,
) {
    for (e, s, mut props) in &mut monsters {
        let hp = props.get_property(F::FIGHT_PROP_MAX_HP) * s.hp;
        let atk = props.get_property(F::FIGHT_PROP_CUR_ATTACK) * s.attack;
        props.set_property(F::FIGHT_PROP_MAX_HP, hp);
        props.set_property(F::FIGHT_PROP_CUR_HP, hp);
        props.set_property(F::FIGHT_PROP_CUR_ATTACK, atk);
        commands.entity(e).insert(Scaled);
    }
}

#[derive(SystemParam)]
pub struct Context<'w, 's> {
    commands: Commands<'w, 's>,
    players: ResMut<'w, Players>,
    output: Res<'w, MessageOutput>,
    counter: ResMut<'w, EntityCounter>,
    version: Res<'w, WorldVersionConfig>,
    owner: Res<'w, WorldOwnerUID>,
    clock: Res<'w, SceneTime>,
    enter: MessageWriter<'w, ScenePlayerEnterDungeonEvent>,
    team: MessageWriter<'w, PlayerAvatarTeamChanged>,
    actors: Query<
        'w,
        's,
        (
            &'static ProtocolEntityID,
            &'static Transform,
            Option<&'static MonsterEquipment>,
        ),
    >,
    avatars: Query<
        'w,
        's,
        (
            &'static OwnerPlayerUID,
            &'static Guid,
            &'static mut FightProperties,
            &'static mut LifeState,
        ),
    >,
}
fn number(v: &Value, k: &str) -> u32 {
    v.get(k)
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0)
}
fn millis() -> u64 {
    common::time_util::unix_timestamp_ms()
}
fn round_record(
    owner: u32,
    round: usize,
    milliseconds: u32,
    team: Option<&b::LeyLineChallengeTeamBin>,
) -> Value {
    let avatars: Vec<_> = team
        .into_iter()
        .flat_map(|t| &t.avatar_ids)
        .map(|id| json!({"avatar_id":id}))
        .collect();
    json!({
        "CIPECILEKHK": round + 1,
        "is_finished": milliseconds > 0,
        "cost_time": milliseconds.div_ceil(1000),
        "team_info_list": [{"uid":owner,"avatar_info_list":avatars}]
    })
}
fn records(s: &Session) -> Vec<Value> {
    s.costs
        .iter()
        .enumerate()
        .map(|(i, &cost)| round_record(s.owner, i, cost, s.teams.team_list.get(i)))
        .collect()
}
fn gallery(s: &Session) -> Value {
    let battle = s.stage == 2;
    json!({
        "gallery_id": if battle {s.arena.battle_gallery} else {s.arena.prepare_gallery},
        "owner_uid": s.owner,
        "player_count": 1,
        "stage": if battle {2} else {1},
        "start_time": s.scene_start,
        "end_time": if battle {s.scene_start + s.limit()} else {0}
    })
}
fn stage(s: &Session, out: &MessageOutput) {
    out.send_json(
        s.owner,
        "LeyLineChallengeStageNotify",
        json!({
            "owner_uid": s.owner,
            "difficulty": s.difficulty,
            "stage": s.stage,
            "cur_round_num": s.round + 1,
            "stage_begin_time": s.stage_time.to_string(),
            "transaction": s.transaction,
            "record_list": records(s),
            "gallery_info": gallery(s),
            "player_list": [{"uid":s.owner}]
        }),
    );
}
pub(super) fn max_completed(player: &b::PlayerDataBin) -> u32 {
    activity::ley_state(player)
        .into_iter()
        .flat_map(|p| p.records.values())
        .flat_map(|s| s.records.iter())
        .filter(|(_, r)| r.round_time_ms.len() == 3 && r.round_time_ms.iter().all(|t| *t > 0))
        .map(|(&difficulty, _)| difficulty)
        .max()
        .unwrap_or(0)
}
pub(super) fn record_packet(
    player: &b::PlayerDataBin,
    schedule: u32,
    difficulty: u32,
) -> Option<Value> {
    let r = activity::ley_state(player)?
        .records
        .get(&schedule)?
        .records
        .get(&difficulty)?;
    let rounds: Vec<_> = r
        .round_time_ms
        .iter()
        .enumerate()
        .map(|(i, &ms)| round_record(player.uid, i, ms, r.teams.get(i)))
        .collect();
    Some(json!({
        "schedule_id": schedule,
        "difficulty": difficulty,
        "timestamp": r.timestamp.to_string(),
        "player_info_list": [{"uid":player.uid}],
        "MJCPCDPFJHB": rounds
    }))
}
impl Context<'_, '_> {
    fn remove(&mut self, actor: Option<Actor>) {
        if let Some(a) = actor {
            if let Some((_, _, Some(equip))) = self.actors.iter().find(|(id, _, _)| id.0 == a.id) {
                for e in &equip.0 {
                    if let Ok(mut e) = self.commands.get_entity(*e) {
                        e.insert(ToBeRemovedMarker);
                    }
                }
            }
            if let Ok(mut e) = self.commands.get_entity(a.entity) {
                e.insert(ToBeRemovedMarker);
            }
            self.output.send_to_all(
                "SceneEntityDisappearNotify",
                n::SceneEntityDisappearNotify {
                    entity_list: vec![a.id],
                    disappear_type: n::VisionType::VisionRemove.into(),
                    ..Default::default()
                },
            );
        }
    }
    fn clear_battle(&mut self, s: &mut Session) {
        self.remove(s.boss.take());
        self.remove(s.key.take());
        for (_, a) in s.summons.drain() {
            self.remove(Some(a));
        }
        s.actions.clear();
    }
    fn team(&mut self, uid: u32, guids: Vec<u64>, current: u64, heal: bool) {
        if let Some(p) = self.players.get_mut(uid) {
            if let Some(a) = p.avatar_bin.as_mut() {
                a.cur_avatar_guid_list = guids.clone();
                a.cur_avatar_guid = current;
            }
        }
        if heal {
            for (owner, guid, mut props, mut life) in &mut self.avatars {
                if owner.0 == uid && guids.contains(&guid.0) {
                    let hp = props.get_property(F::FIGHT_PROP_MAX_HP);
                    props.set_property(F::FIGHT_PROP_CUR_HP, hp);
                    for (current, maximum) in [
                        (F::FIGHT_PROP_CUR_FIRE_ENERGY, F::FIGHT_PROP_MAX_FIRE_ENERGY),
                        (
                            F::FIGHT_PROP_CUR_WATER_ENERGY,
                            F::FIGHT_PROP_MAX_WATER_ENERGY,
                        ),
                        (
                            F::FIGHT_PROP_CUR_GRASS_ENERGY,
                            F::FIGHT_PROP_MAX_GRASS_ENERGY,
                        ),
                        (F::FIGHT_PROP_CUR_ELEC_ENERGY, F::FIGHT_PROP_MAX_ELEC_ENERGY),
                        (F::FIGHT_PROP_CUR_WIND_ENERGY, F::FIGHT_PROP_MAX_WIND_ENERGY),
                        (F::FIGHT_PROP_CUR_ICE_ENERGY, F::FIGHT_PROP_MAX_ICE_ENERGY),
                        (F::FIGHT_PROP_CUR_ROCK_ENERGY, F::FIGHT_PROP_MAX_ROCK_ENERGY),
                        (
                            F::FIGHT_PROP_CUR_SPECIAL_ENERGY,
                            F::FIGHT_PROP_MAX_SPECIAL_ENERGY,
                        ),
                    ] {
                        let energy = props.get_property(maximum);
                        props.set_property(current, energy);
                    }
                    *life = LifeState::Alive;
                }
            }
        }
        self.team.write(PlayerAvatarTeamChanged {
            uid,
            avatar_team_guid_list: guids,
            cur_avatar_guid: current,
        });
    }
    fn selected_team(&mut self, s: &Session) -> Result<(), Retcode> {
        let p = self.players.get(s.owner).ok_or(Retcode::RetFail)?;
        let avatars = p.avatar_bin.as_ref().ok_or(Retcode::RetFail)?;
        let ids = &s.teams.team_list[s.round].avatar_ids;
        if ids.is_empty() {
            return Err(Retcode::RetCanNotFindAvatar);
        }
        let mut guids = vec![];
        for id in ids {
            guids.push(
                avatars
                    .avatar_map
                    .values()
                    .find(|a| a.avatar_id == *id)
                    .ok_or(Retcode::RetCanNotFindAvatar)?
                    .guid,
            );
        }
        let current = guids[0];
        self.team(s.owner, guids, current, true);
        Ok(())
    }
    fn restore(&mut self, s: &mut Session) {
        self.clear_battle(s);
        // 场景初始化会按恢复后的队伍生成实体，不再另发一次换队事件。
        if let Some(p) = self.players.get_mut(s.owner) {
            if let Some(a) = p.avatar_bin.as_mut() {
                a.cur_avatar_guid_list = s.original.clone();
                a.cur_avatar_guid = s.original_guid;
            }
            snezhnaya_gi_persistence::normalize_active_team(p);
        }
    }
    fn reenter(&mut self, s: &mut Session) {
        self.clear_battle(s);
        s.ready = false;
        s.start_pending = true;
        self.enter
            .write(ScenePlayerEnterDungeonEvent(s.owner, s.arena.dungeon));
    }
    fn spawn_key(&mut self, s: &mut Session) {
        if s.key.is_some() || !s.ready || s.completed() || s.stage != 1 {
            return;
        }
        let position = s.monster().pos.clone();
        if let Some((id, entity, _, _)) = spawn_gadget_entity(
            self.version.protocol_version.clone(),
            &mut self.commands,
            &mut self.counter,
            b::VectorBin {
                x: position.x,
                y: position.y + 0.403,
                z: position.z,
            },
            Default::default(),
            73051002,
            1,
            true,
            Some(n::scene_gadget_info::Content::Worktop(n::WorktopInfo {
                option_list: vec![7],
                ..Default::default()
            })),
            None,
            0,
            0,
        ) {
            self.commands
                .entity(entity)
                .insert((GroupId(s.arena.group), ConfigId(1001), Visible));
            s.key = Some(Actor { id, entity });
        }
    }
    fn start(&mut self, s: &mut Session) -> Result<(), Retcode> {
        if s.stage == 2 {
            return Ok(());
        }
        if !s.ready || s.start_pending || s.completed() || s.costs[s.round] != 0 {
            return Err(Retcode::RetFail);
        }
        self.selected_team(s)?;
        let m = s.monster();
        let level = s.level().revise_level + 1;
        let (id, entity, _, _) = spawn_monster_entity(
            self.version.protocol_version.clone(),
            &mut self.commands,
            &mut self.counter,
            b::VectorBin {
                x: m.pos.x,
                y: m.pos.y,
                z: m.pos.z,
            },
            b::VectorBin {
                x: m.rot.x,
                y: m.rot.y,
                z: m.rot.z,
            },
            m.monster_id,
            level,
            m.pose_id,
            0,
            0,
            None,
            0,
        )
        .ok_or(Retcode::RetNotFoundConfig)?;
        self.commands.entity(entity).insert((
            GroupId(s.arena.group),
            ConfigId(1003),
            Visible,
            AffixList(m.affix_ids.clone()),
            Scaling {
                hp: m.hp_multiplier,
                attack: m.attack_multiplier,
            },
            MonsterSpawnInfo {
                ai_config_id: m.ai_config_id,
                ..Default::default()
            },
        ));
        s.boss = Some(Actor { id, entity });
        s.stage = 2;
        s.started = Instant::now();
        s.stage_time = millis();
        s.scene_start = (self.clock.elapsed_ms() / 1000) as u32;
        stage(s, &self.output);
        self.output.send_json(s.owner,"GalleryStartNotify",json!({"gallery_id":s.arena.battle_gallery,"owner_uid":s.owner,"player_count":1,"start_time":s.scene_start,"end_time":s.scene_start+s.limit()}));
        self.output.send_json(
            s.owner,
            "MonsterForceAlertNotify",
            json!({"monster_entity_id":id}),
        );
        self.remove(s.key.take());
        Ok(())
    }
    fn finish(&mut self, s: &mut Session, success: bool, reason: u32) {
        if s.stage != 2 {
            return;
        }
        let cost = s
            .started
            .elapsed()
            .as_millis()
            .max(1)
            .min(u128::from(u32::MAX)) as u32;
        if success {
            s.costs[s.round] = cost;
        }
        self.output.send_json(
            s.owner,
            "GalleryStopNotify",
            json!({"gallery_id":s.arena.battle_gallery,"stop_reason":reason}),
        );
        self.clear_battle(s);
        s.stage = 1;
        s.started = Instant::now();
        s.stage_time = millis();
        let mut is_new = false;
        if s.completed() {
            let p = self.players.get_mut(s.owner).unwrap();
            let state = activity::ley_state_mut(p)
                .records
                .entry(s.schedule)
                .or_default();
            let replace = state.records.get(&s.difficulty).is_none_or(|r| {
                r.round_time_ms.len() != 3
                    || s.costs.iter().map(|v| u64::from(*v)).sum::<u64>()
                        < r.round_time_ms.iter().map(|v| u64::from(*v)).sum::<u64>()
            });
            if replace {
                is_new = true;
                state.records.insert(
                    s.difficulty,
                    b::LeyLineChallengeRecordBin {
                        round_time_ms: s.costs.to_vec(),
                        teams: s.teams.team_list.clone(),
                        timestamp: millis(),
                    },
                );
            }
            let progress = p
                .activity_bin
                .as_mut()
                .unwrap()
                .activity_bin_map
                .get_mut(&ACTIVITY)
                .unwrap();
            for id in &s.level().watcher_list {
                let Some(def) = ProgressionConfig::get().activity_watchers.get(id) else {
                    continue;
                };
                let elite = s.level().elite_times.iter().copied().max().unwrap_or(0);
                if *id == 1526907
                    && s.costs.iter().map(|v| u64::from(*v)).sum::<u64>() > u64::from(elite) * 1000
                {
                    continue;
                }
                progress.watcher_progress_map.insert(*id, def.progress);
                if !progress.finished_watcher_id_list.contains(id) {
                    progress.finished_watcher_id_list.push(*id);
                }
            }
            activity::notify(p, &self.output);
            self.output.send_json(
                s.owner,
                "LeyLineChallengeDataUpdateNotify",
                json!({"data":activity::ley_data(p)}),
            );
        }
        self.output.send_json(s.owner,"LeyLineChallengeSettleNotify",json!({"difficulty":s.difficulty,"cur_round_num":s.round+1,"is_succ":success,"is_new_record":is_new,"current_record":records(s)[s.round].clone(),"record_list":records(s)}));
        stage(s, &self.output);
        self.spawn_key(s);
    }
    fn create_session(&mut self, uid: u32, req: &Value) -> Result<Session, Retcode> {
        if uid != self.owner.0 || self.players.keys().count() != 1 {
            return Err(Retcode::RetFail);
        }
        let season = activity::active_season(now()).ok_or(Retcode::RetActivityClose)?;
        let difficulty = number(req, "difficulty");
        let cfg = ProgressionConfig::get();
        let level = season
            .level_id_list
            .iter()
            .filter_map(|id| cfg.challenge_levels.get(id))
            .find(|l| l.difficulty == difficulty)
            .ok_or(Retcode::RetNotFoundConfig)?;
        let p = self.players.get_mut(uid).ok_or(Retcode::RetFail)?;
        if max_completed(p)
            < LeyLineConfig::get()
                .required_difficulty
                .get(&difficulty)
                .copied()
                .unwrap_or(u32::MAX)
        {
            return Err(Retcode::RetFail);
        }
        if level.level_config_list.len() != 3
            || level
                .level_config_list
                .iter()
                .any(|id| !LeyLineConfig::get().monsters.contains_key(id))
        {
            return Err(Retcode::RetNotFoundConfig);
        }
        let arena = LeyLineConfig::get()
            .arenas
            .get(&1)
            .cloned()
            .ok_or(Retcode::RetNotFoundConfig)?;
        if !excel::dungeon_excel_config_collection::get().contains_key(&arena.dungeon)
            || !std::path::Path::new(&format!(
                "assets/lua/scene/{}/scene{}.lua",
                arena.scene, arena.scene
            ))
            .is_file()
        {
            return Err(Retcode::RetNotFoundConfig);
        }
        let teams = if req
            .get("team_list")
            .and_then(Value::as_array)
            .is_some_and(|v| !v.is_empty())
        {
            activity::parse_teams(p, req)?
        } else {
            activity::ley_state(p)
                .and_then(|r| r.teams.get(&difficulty))
                .cloned()
                .unwrap_or_else(|| b::LeyLineChallengeTeamsBin {
                    team_list: vec![Default::default(); 3],
                })
        };
        activity::ley_state_mut(p)
            .teams
            .insert(difficulty, teams.clone());
        let avatars = p.avatar_bin.as_ref().ok_or(Retcode::RetFail)?;
        let s = Session {
            owner: uid,
            schedule: season.schedule_id,
            difficulty,
            arena,
            round: 0,
            teams,
            original: avatars.cur_avatar_guid_list.clone(),
            original_guid: avatars.cur_avatar_guid,
            stage: 1,
            ready: false,
            start_pending: false,
            started: Instant::now(),
            stage_time: millis(),
            scene_start: 0,
            transaction: format!("{}-{}-{}", season.schedule_id, uid, millis()),
            key: None,
            boss: None,
            summons: HashMap::new(),
            actions: HashSet::new(),
            costs: [0; 3],
        };
        self.enter
            .write(ScenePlayerEnterDungeonEvent(uid, s.arena.dungeon));
        Ok(s)
    }
    fn summon(&mut self, s: &mut Session, invoke: &n::AbilityInvokeEntry) {
        if s.stage != 2
            || s.boss.is_none_or(|b| b.id != invoke.entity_id)
            || invoke.argument_type() != n::AbilityInvokeArgument::AbilityActionSummon
        {
            return;
        }
        let Some(template) = s.monster().summon.as_ref() else {
            return;
        };
        let Some(head) = invoke.head else {
            return;
        };
        let Some(&index) = template.local_ids.get(&(head.local_id as u32)) else {
            return;
        };
        let key = [
            head.instanced_ability_id,
            head.instanced_modifier_id,
            head.local_id as u32,
            invoke.event_id,
        ];
        if s.summons.len() >= 3 || s.actions.len() >= 4096 || s.actions.contains(&key) {
            return;
        }
        let Some(req) = dy_parser::decode_from_vec_by_name_version::<n::AbilityActionSummon>(
            &self.version.protocol_version,
            "AbilityActionSummon",
            &invoke.ability_data,
        ) else {
            return;
        };
        let (Some(pos), Some(rot)) = (req.pos, req.rot) else {
            return;
        };
        if ![pos.x, pos.y, pos.z, rot.x, rot.y, rot.z]
            .into_iter()
            .all(f32::is_finite)
        {
            return;
        }
        let Some((_, transform, _)) = self
            .actors
            .iter()
            .find(|(id, _, _)| id.0 == invoke.entity_id)
        else {
            return;
        };
        if (pos.x - transform.position.x).abs() > 60.
            || (pos.y - transform.position.y).abs() > 30.
            || (pos.z - transform.position.z).abs() > 60.
        {
            return;
        }
        if let Some((id, entity, _, _)) = spawn_monster_entity(
            self.version.protocol_version.clone(),
            &mut self.commands,
            &mut self.counter,
            pos.into(),
            rot.into(),
            template.monster_id,
            s.level().revise_level + 1,
            0,
            0,
            0,
            None,
            0,
        ) {
            let value = n::AbilityScalarValueEntry {
                key: Some(n::AbilityString {
                    r#type: Some(n::ability_string::Type::Hash(template.sgv_hash)),
                }),
                float_value: index as f32,
            };
            self.commands.entity(entity).insert((
                GroupId(s.arena.group),
                ConfigId(2000 + index),
                Visible,
                AffixList(template.affix_ids.clone()),
                Scaling {
                    hp: template.hp_multiplier,
                    attack: 1.,
                },
                MonsterSpawnInfo {
                    owner_entity_id: invoke.entity_id,
                    summoned_tag: template.monster_id,
                    values: vec![value],
                    ..Default::default()
                },
            ));
            s.summons.insert(id, Actor { id, entity });
            s.actions.insert(key);
            self.summon_tags(s);
        }
    }
    fn summon_tags(&self, s: &Session) {
        let Some(boss) = s.boss else {
            return;
        };
        let tag = s.monster().summon.as_ref().map_or(0, |m| m.monster_id);
        let tags: HashMap<u32, u32> = if s.summons.is_empty() {
            HashMap::new()
        } else {
            HashMap::from([(tag, s.summons.len() as u32)])
        };
        self.output.send(
            s.owner,
            "MonsterSummonTagNotify",
            n::MonsterSummonTagNotify {
                monster_entity_id: boss.id,
                summon_tag_map: tags,
            },
        );
    }
}

pub fn handle(
    mut ctx: Context,
    mut state: ResMut<Challenges>,
    mut messages: MessageReader<ClientMessageEvent>,
) {
    for message in messages.read() {
        let name = message.message_name();
        if !matches!(
            name,
            "LeyLineChallengeEnterReq"
                | "LeyLineChallengeSetRoundTeamReq"
                | "LeyLineChallengeSetTeamReq"
                | "LeyLineChallengeStartReq"
                | "LeyLineChallengeRetryReq"
                | "LeyLineChallengeTerminateReq"
                | "LeyLineChallengeTakeRewardReq"
                | "SelectWorktopOptionReq"
                | "GetAllSceneGalleryInfoReq"
                | "PlayerQuitDungeonReq"
        ) {
            continue;
        }
        let uid = message.sender_uid();
        let Some(req) = message.decode_json() else {
            continue;
        };
        if name == "LeyLineChallengeEnterReq" {
            if uid != ctx.owner.0 {
                ctx.output.send_json(
                    uid,
                    "LeyLineChallengeEnterRsp",
                    json!({"retcode":i32::from(Retcode::RetFail)}),
                );
                continue;
            }
            if let Some(s) = state.session.as_ref() {
                if !s.completed() && (s.stage == 2 || s.difficulty == number(&req, "difficulty")) {
                    stage(s, &ctx.output);
                    ctx.output
                        .send_json(uid, "LeyLineChallengeEnterRsp", json!({}));
                    continue;
                }
            }
            if let Some(mut old) = state.session.take() {
                ctx.restore(&mut old);
            }
            match ctx.create_session(uid, &req) {
                Ok(s) => {
                    state.session = Some(s);
                    state.wake = Some(Instant::now() + Duration::from_millis(10));
                    ctx.output
                        .send_json(uid, "LeyLineChallengeEnterRsp", json!({}));
                }
                Err(code) => ctx.output.send_json(
                    uid,
                    "LeyLineChallengeEnterRsp",
                    json!({"retcode":i32::from(code)}),
                ),
            }
            continue;
        }
        if name == "GetAllSceneGalleryInfoReq" {
            ctx.output.send_json(uid,"GetAllSceneGalleryInfoRsp",json!({"gallery_info_list":state.session.as_ref().filter(|s|s.ready).map(|s|vec![gallery(s)]).unwrap_or_default()}));
            continue;
        }
        if name == "LeyLineChallengeTakeRewardReq" {
            ctx.output.send_json(
                uid,
                "LeyLineChallengeTakeRewardRsp",
                json!({"retcode":i32::from(Retcode::RetNotSupportItem)}),
            );
            continue;
        }
        let Some(mut s) = state.session.take() else {
            if name.starts_with("LeyLineChallenge") && name != "LeyLineChallengeSetTeamReq" {
                ctx.output.send_json(
                    uid,
                    &name.replace("Req", "Rsp"),
                    json!({"retcode":i32::from(Retcode::RetFail)}),
                );
            }
            continue;
        };
        if s.owner != uid {
            state.session = Some(s);
            continue;
        }
        let result = match name {
            "LeyLineChallengeSetTeamReq" => {
                if s.stage != 2 {
                    if let Some(p) = ctx.players.get(uid) {
                        if let Some(t) =
                            activity::ley_state(p).and_then(|r| r.teams.get(&s.difficulty))
                        {
                            s.teams = t.clone();
                        }
                    }
                }
                Ok(())
            }
            "LeyLineChallengeStartReq" => {
                if s.stage == 1
                    && req
                        .get("team_list")
                        .and_then(Value::as_array)
                        .is_some_and(|v| !v.is_empty())
                {
                    match ctx.players.get(uid).map(|p| activity::parse_teams(p, &req)) {
                        Some(Ok(teams)) => {
                            s.teams = teams;
                            ctx.start(&mut s)
                        }
                        _ => Err(Retcode::RetCanNotFindAvatar),
                    }
                } else {
                    ctx.start(&mut s)
                }
            }
            "SelectWorktopOptionReq" => {
                if number(&req, "option_id") == 7
                    && s.key
                        .is_some_and(|k| k.id == number(&req, "gadget_entity_id"))
                {
                    ctx.start(&mut s)
                } else {
                    Ok(())
                }
            }
            "LeyLineChallengeRetryReq" => {
                if s.completed() || s.start_pending || s.costs[s.round] != 0 {
                    Err(Retcode::RetFail)
                } else {
                    if s.stage == 2 {
                        ctx.finish(&mut s, false, 2);
                    }
                    ctx.reenter(&mut s);
                    Ok(())
                }
            }
            "LeyLineChallengeTerminateReq" => {
                if s.stage == 2 {
                    ctx.finish(&mut s, false, 2);
                    Ok(())
                } else {
                    Err(Retcode::RetFail)
                }
            }
            "LeyLineChallengeSetRoundTeamReq" => {
                let round = number(&req, "round_id");
                if !(1..=3).contains(&round)
                    || s.stage == 2
                    || s.start_pending
                    || s.costs[(round.saturating_sub(1)) as usize] != 0
                {
                    Err(Retcode::RetFail)
                } else {
                    let index = (round - 1) as usize;
                    let mut input = json!({"team_list":s.teams.team_list.iter().map(|t|json!({"avatar_list":t.avatar_ids.iter().map(|id|json!({"avatar_id":id})).collect::<Vec<_>>()})).collect::<Vec<_>>()});
                    if let Some(team) = req.get("team") {
                        input["team_list"][index] = team.clone();
                    }
                    let candidate = ctx
                        .players
                        .get(uid)
                        .and_then(|p| activity::parse_teams(p, &input).ok());
                    if let Some(teams) =
                        candidate.filter(|t| !t.team_list[index].avatar_ids.is_empty())
                    {
                        let changed = s.round != index;
                        s.teams = teams;
                        s.round = index;
                        if let Some(p) = ctx.players.get_mut(uid) {
                            activity::ley_state_mut(p)
                                .teams
                                .insert(s.difficulty, s.teams.clone());
                        }
                        if changed {
                            // 换轮先重入场地，等客户端加载完成再生成 Boss。
                            ctx.reenter(&mut s);
                            Ok(())
                        } else {
                            ctx.start(&mut s)
                        }
                    } else {
                        Err(Retcode::RetCanNotFindAvatar)
                    }
                }
            }
            "PlayerQuitDungeonReq" => {
                if req.get("is_quit_immediately").and_then(Value::as_bool).unwrap_or(false) {
                    ctx.restore(&mut s);
                    continue;
                }
                Ok(())
            }
            _ => Ok(()),
        };
        if name.starts_with("LeyLineChallenge") && name != "LeyLineChallengeSetTeamReq" {
            ctx.output.send_json(
                uid,
                &name.replace("Req", "Rsp"),
                json!({"retcode":result.err().map(i32::from).unwrap_or(0)}),
            );
        }
        state.wake = Some(Instant::now() + Duration::from_millis(10));
        state.session = Some(s);
    }
}

pub fn update(
    mut ctx: Context,
    mut state: ResMut<Challenges>,
    mut ready: MessageReader<EnterSceneDoneEvent>,
    mut kills: MessageReader<MonsterKillEvent>,
    mut invokes: MessageReader<ServerInvokeEvent>,
) {
    if state.wake.is_some_and(|t| t <= Instant::now()) {
        state.wake = None;
    }
    let Some(mut s) = state.session.take() else {
        ready.clear();
        kills.clear();
        invokes.clear();
        return;
    };
    let scene = ctx
        .players
        .get(s.owner)
        .and_then(|p| p.scene_bin.as_ref())
        .map_or(0, |p| p.my_cur_scene_id);
    if s.ready && scene != s.arena.scene {
        ctx.restore(&mut s);
        ready.clear();
        kills.clear();
        invokes.clear();
        return;
    }
    for EnterSceneDoneEvent(uid) in ready.read() {
        if *uid != s.owner || scene != s.arena.scene {
            continue;
        }
        s.ready = true;
        if let Some(p) = ctx.players.get_mut(s.owner) {
            if let Some(dungeon) = p.dungeon_bin.as_mut() {
                dungeon.is_ley_line_challenge = true;
                dungeon.original_avatar_guids = s.original.clone();
                dungeon.original_avatar_guid = s.original_guid;
            }
        }
        if s.start_pending {
            s.start_pending = false;
            let _ = ctx.start(&mut s);
        } else {
            ctx.spawn_key(&mut s);
        }
        stage(&s, &ctx.output);
        state.wake = Some(Instant::now() + Duration::from_millis(10));
    }
    for event in kills.read() {
        if s.summons.remove(&event.entity_id).is_some() {
            ctx.summon_tags(&s);
            continue;
        }
        if s.stage == 2
            && s.boss.is_some_and(|b| b.id == event.entity_id)
            && event.group_id == s.arena.group
            && event.config_id == 1003
        {
            let success = s.started.elapsed() < Duration::from_secs(u64::from(s.limit()));
            ctx.finish(&mut s, success, if success { 3 } else { 1 });
            state.wake = Some(Instant::now() + Duration::from_millis(10));
        }
    }
    for ServerInvokeEvent(invoke) in invokes.read() {
        let previous = s.summons.len();
        ctx.summon(&mut s, invoke);
        if previous != s.summons.len() {
            state.wake = Some(Instant::now() + Duration::from_millis(10));
        }
    }
    if s.stage == 2 {
        let guids = ctx
            .players
            .get(s.owner)
            .and_then(|p| p.avatar_bin.as_ref())
            .map_or(vec![], |a| a.cur_avatar_guid_list.clone());
        let mut count = 0;
        let mut alive = 0;
        for (owner, guid, _, life) in ctx.avatars.iter_mut() {
            if owner.0 == s.owner && guids.contains(&guid.0) {
                count += 1;
                if *life == LifeState::Alive {
                    alive += 1;
                }
            }
        }
        if count > 0 && alive == 0 {
            ctx.finish(&mut s, false, 8);
            state.wake = Some(Instant::now() + Duration::from_millis(10));
        }
    }
    if s.stage == 2
        && (s.started.elapsed() >= Duration::from_secs(u64::from(s.limit()))
            || activity::active_season(now()).is_none_or(|v| v.schedule_id != s.schedule))
    {
        ctx.finish(&mut s, false, 1);
        state.wake = Some(Instant::now() + Duration::from_millis(10));
    }
    state.session = Some(s);
}
