use crate::common::PlayerSceneStates;
use bevy_ecs::prelude::*;
use common::language::Language;
use common::player_cache::{cache_get_is_notify, cache_get_language, cache_set_is_notify};
use snezhnaya_gi_event::combat::PlayerMoveEvent;
use snezhnaya_gi_event::scene::*;
use snezhnaya_gi_message::output::MessageOutput;
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::dy_parser::{replace_out_i32, replace_out_u32};
use snezhnaya_gi_proto::normal::{AntiAddictNotify, SceneInitFinishRsp};
use snezhnaya_gi_proto::retcode::Retcode;

pub fn on_scene_init_finish(
    mut reader: MessageReader<SceneInitFinishEvent>,
    mut players: ResMut<Players>,
    mut move_events: MessageWriter<PlayerMoveEvent>,
    mut join_team_events: MessageWriter<PlayerJoinTeamEvent>,
) {
    for event in reader.read() {
        let uid = event.0;
        let Some(player_info) = players.get_mut(uid) else {
            continue;
        };
        if player_info.dungeon_bin.is_some() {
            if let Some(ref player_scene_bin) = player_info.scene_bin {
                if let Some(ref pos) = player_scene_bin.my_cur_scene_pos {
                    move_events.write(PlayerMoveEvent(
                        uid,
                        player_scene_bin.my_cur_scene_id,
                        (pos.x, pos.y, pos.z),
                        true,
                    ));
                }
            }
        };

        if !snezhnaya_gi_persistence::normalize_active_team(player_info) {
            tracing::error!(uid, "scene entry has no owned avatar");
            continue;
        }
        let Some(ref mut player_avatar_bin) = player_info.avatar_bin else {
            continue;
        };

        if player_avatar_bin.cur_avatar_guid_list.is_empty() {
            tracing::debug!("cur_avatar_guid_list is_empty");
            continue;
        }

        let appear_avatar_guid = {
            if !player_avatar_bin
                .cur_avatar_guid_list
                .contains(&player_avatar_bin.cur_avatar_guid)
            {
                player_avatar_bin.cur_avatar_guid = player_avatar_bin
                    .cur_avatar_guid_list
                    .first()
                    .copied()
                    .unwrap_or_default();
            }
            player_avatar_bin.cur_avatar_guid
        };

        join_team_events.write(PlayerJoinTeamEvent {
            player_uid: uid,
            avatar_guid_list: player_avatar_bin.cur_avatar_guid_list.clone(),
            appear_avatar_guid,
        });
    }
}

pub fn scene_init_finish_send_rsp(
    mut scene_init_finish_events: MessageReader<SceneInitFinishEvent>,
    player_scene_states: Res<PlayerSceneStates>,
    message_output: Res<MessageOutput>,
    mut lua_shell_events: MessageWriter<snezhnaya_gi_event::luashell::LuaShellEvent>,
    world_version_config: Res<WorldVersionConfig>,
) {
    for event in scene_init_finish_events.read() {
        let uid = event.0;

        let Some(player_scene_state) = player_scene_states.get(&uid) else {
            continue;
        };

        message_output.send(
            uid,
            "SceneInitFinishRsp",
            SceneInitFinishRsp {
                retcode: replace_out_i32(
                    world_version_config.protocol_version.as_str(),
                    "SceneInitFinishRsp.retcode",
                    Retcode::RetSucc.into(),
                ),
                enter_scene_token: replace_out_u32(
                    world_version_config.protocol_version.as_str(),
                    "SceneInitFinishRsp.enter_scene_token",
                    player_scene_state.enter_scene_token(),
                ),
            },
        );

        if !cache_get_is_notify(uid).unwrap_or_default() {
            cache_set_is_notify(uid, true);
            let language = cache_get_language(uid).unwrap_or(Language::Chs);
            let msg = if language == Language::Chs || language == Language::Cht {
                format!(
                    "欢迎来到 {}！\n本项目为免费开源软件，遵循 AGPL-3.0 许可。请勿付费购买。",
                    common::SERVER_DISPLAY_NAME
                )
            } else {
                format!(
                    "Welcome to {}!\nThis is free and open source software under AGPL-3.0. Do not pay for it.",
                    common::SERVER_DISPLAY_NAME
                )
            };
            message_output.send(
                uid,
                "AntiAddictNotify",
                AntiAddictNotify {
                    msg,
                    msg_type: 1,
                    ..Default::default()
                },
            );
        }

        lua_shell_events.write(snezhnaya_gi_event::luashell::LuaShellEvent());
    }
}
