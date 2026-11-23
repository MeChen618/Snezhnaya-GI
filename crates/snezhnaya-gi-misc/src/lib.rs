use bevy_app::prelude::*;
mod activity;
mod daily_task;
mod ley_line;
mod mail;
mod progression;
use bevy_ecs::prelude::*;

use common::player_cache::{cache_get_client_time, cache_get_is_pause, cache_set_is_pause};
use common::time_util;
use snezhnaya_gi_event::scene::*;
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::normal::{
    ChangeGameTimeReq, ChangeGameTimeRsp, ClientSetGameTimeReq, ClientSetGameTimeRsp,
    PlayerGameTimeNotify, PlayerSetPauseReq, PlayerSetPauseRsp, PlayerTimeNotify, ServerTimeNotify,
};
use snezhnaya_gi_proto::retcode::Retcode;
pub struct MiscPlugin;

impl Plugin for MiscPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(SceneTime::default())
            .add_systems(
                PreStartup,
                (
                    progression::login,
                    activity::login,
                    mail::login,
                    daily_task::login,
                )
                    .chain(),
            )
            .init_resource::<ley_line::Challenges>()
            .init_resource::<daily_task::DailyWakeup>()
            .add_systems(
                Update,
                (
                    mail::handle,
                    daily_task::handle,
                    progression::handle,
                    activity::handle,
                    ley_line::handle,
                    ley_line::apply_monster_scaling,
                )
                    .chain(),
            )
            .add_systems(
                Last,
                (
                    ley_line::update,
                    ley_line::apply_monster_scaling,
                    daily_task::update,
                    daily_task::observe,
                    progression::observe,
                )
                    .chain()
                    .after(snezhnaya_gi_entity::EntitySystemSet::HandleEntityDeath)
                    .before(snezhnaya_gi_entity::EntitySystemSet::HandleEntityCleanup),
            )
            .add_systems(Startup, init_scene_time)
            .add_systems(PreUpdate, set_pause)
            .add_systems(PreUpdate, client_set_game_time)
            .add_systems(
                First,
                (
                    update_scene_clock,
                    sync_scene_time_on_scene_init_finish,
                    sync_scene_time_on_enter_scene_done,
                )
                    .chain(),
            );
    }
}

#[derive(Resource, Default)]
pub struct SceneTime {
    pub scene_time: u64,
    started_at: Option<std::time::Instant>,
    pub game_time: u32,
}

impl SceneTime {
    pub fn elapsed_ms(&self) -> u64 {
        self.started_at
            .map_or(0, |start| start.elapsed().as_millis() as u64)
    }
}
fn update_scene_clock(mut time: ResMut<SceneTime>) {
    time.started_at.get_or_insert_with(std::time::Instant::now);
    time.scene_time = time.elapsed_ms();
}
pub fn next_wakeup(world: &World) -> Option<std::time::Instant> {
    [ley_line::next_wakeup(world), daily_task::next_wakeup(world)]
        .into_iter()
        .flatten()
        .min()
}

pub fn init_scene_time(mut time: ResMut<SceneTime>) {
    time.game_time = (7.31 * 60.0) as u32;
}

pub fn set_pause(
    mut events: MessageReader<ClientMessageEvent>,
    message_output: Res<MessageOutput>,
) {
    for message in events.read() {
        match message.message_name() {
            "PlayerSetPauseReq" => {
                if let Some(request) = message.decode::<PlayerSetPauseReq>() {
                    let uid = message.sender_uid();
                    cache_set_is_pause(uid, request.is_paused);
                    message_output.send(uid, "PlayerSetPauseRsp", PlayerSetPauseRsp { retcode: 0 });
                }
            }
            &_ => {}
        }
    }
}

pub fn client_set_game_time(
    mut events: MessageReader<ClientMessageEvent>,
    players: Res<Players>,
    owner: Res<WorldOwnerUID>,
    mut time: ResMut<SceneTime>,
    message_output: Res<MessageOutput>,
) {
    for message in events.read() {
        let uid = message.sender_uid();
        let retcode = if uid != owner.0 {
            i32::from(Retcode::RetMpNotInMyWorld)
        } else if players
            .get(uid)
            .and_then(|p| p.basic_bin.as_ref())
            .is_none_or(|basic| basic.is_game_time_locked)
        {
            i32::from(Retcode::RetPlayerTimeLocked)
        } else {
            0
        };
        match message.message_name() {
            "ClientSetGameTimeReq" => {
                let Some(request) = message.decode::<ClientSetGameTimeReq>() else {
                    continue;
                };
                if retcode == 0 {
                    time.game_time = request.game_time;
                    message_output.send_to_all(
                        "PlayerGameTimeNotify",
                        PlayerGameTimeNotify {
                            uid,
                            is_home: false,
                            game_time: time.game_time,
                        },
                    );
                }
                message_output.send(
                    uid,
                    "ClientSetGameTimeRsp",
                    ClientSetGameTimeRsp {
                        retcode,
                        game_time: time.game_time,
                        client_game_time: request.client_game_time,
                        ..Default::default()
                    },
                );
            }
            "ChangeGameTimeReq" => {
                let Some(request) = message.decode::<ChangeGameTimeReq>() else {
                    continue;
                };
                let mut rsp = ChangeGameTimeRsp {
                    retcode,
                    cur_game_time: time.game_time,
                    extra_days: request.extra_days,
                };
                if retcode == 0 {
                    // 游戏时间以分钟累计，跨天也不回退。
                    let delta = (request.game_time % 1440 + 1440 - time.game_time % 1440) % 1440;
                    let next = u64::from(time.game_time)
                        + u64::from(request.extra_days) * 1440
                        + u64::from(delta);
                    if let Ok(next) = u32::try_from(next) {
                        time.game_time = next;
                        rsp.cur_game_time = next;
                        message_output.send_to_all(
                            "PlayerGameTimeNotify",
                            PlayerGameTimeNotify {
                                uid,
                                is_home: false,
                                game_time: next,
                            },
                        );
                    } else {
                        rsp.retcode = Retcode::RetFail.into();
                    }
                }
                message_output.send(uid, "ChangeGameTimeRsp", rsp);
            }
            _ => (),
        }
    }
}

pub fn sync_scene_time_on_scene_init_finish(
    mut events: MessageReader<SceneInitFinishEvent>,
    message_output: Res<MessageOutput>,
    players: Res<Players>,
    time: Res<SceneTime>,
) {
    use snezhnaya_gi_proto::normal::{PlayerGameTimeNotify, SceneTimeNotify};

    for SceneInitFinishEvent(uid) in events.read() {
        let Some(player_info) = players.get(*uid) else {
            continue;
        };

        message_output.send(
            *uid,
            "ServerTimeNotify",
            ServerTimeNotify {
                server_time: time_util::unix_timestamp_ms(),
            },
        );

        let scene_id = if let Some(ref player_scene_bin) = player_info.scene_bin {
            player_scene_bin.my_cur_scene_id
        } else {
            0
        };
        message_output.send(
            *uid,
            "SceneTimeNotify",
            SceneTimeNotify {
                is_paused: cache_get_is_pause(*uid).unwrap_or_default(),
                scene_id,
                scene_time: time.scene_time,
            },
        );

        message_output.send(
            *uid,
            "PlayerGameTimeNotify",
            PlayerGameTimeNotify {
                is_home: false,
                uid: *uid,
                game_time: time.game_time,
            },
        )
    }
}

pub fn sync_scene_time_on_enter_scene_done(
    mut events: MessageReader<EnterSceneDoneEvent>,
    message_output: Res<MessageOutput>,
) {
    for EnterSceneDoneEvent(uid) in events.read() {
        message_output.send(
            *uid,
            "PlayerTimeNotify",
            PlayerTimeNotify {
                is_paused: cache_get_is_pause(*uid).unwrap_or_default(),
                server_time: time_util::unix_timestamp_ms(),
                player_time: cache_get_client_time(*uid).unwrap_or_default() as u64,
            },
        );
    }
}
