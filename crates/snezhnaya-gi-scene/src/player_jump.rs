use bevy_ecs::prelude::*;
use common::player_cache::cache_set_scene_level;
use snezhnaya_gi_data::quest::quest_config::QuestContent;
use snezhnaya_gi_entity::avatar::{AvatarQueryReadOnly, CurrentPlayerAvatarMarker};
use snezhnaya_gi_entity::common::Visible;
use snezhnaya_gi_entity::transform::Transform;
use snezhnaya_gi_event::entity::EntityDisappearEvent;
use snezhnaya_gi_event::quest::QuestContentProgressEvent;
use snezhnaya_gi_event::scene::*;
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::normal::VisionType;
use snezhnaya_gi_proto::server_only::{PlayerDungeonCompBin, VectorBin};
use std::sync::Arc;

pub fn player_jump(
    mut events: MessageReader<ScenePlayerJumpEvent>,
    mut players: ResMut<Players>,
    mut enter_events: MessageWriter<BeginEnterSceneEvent>,
    mut commands: Commands,
    player_avatar_entities: Query<(Entity, AvatarQueryReadOnly)>,
    mut disappear_events: MessageWriter<EntityDisappearEvent>,
) {
    for ScenePlayerJumpEvent(uid, scene_id, enter_reason, destination) in events.read() {
        let Some(player_info) = players.get_mut(*uid) else {
            continue;
        };
        if player_info
            .scene_bin
            .as_ref()
            .is_some_and(|s| s.my_cur_scene_id != *scene_id)
        {
            snezhnaya_gi_persistence::restore_dungeon_team(player_info);
        }
        let Some(ref mut player_scene_bin) = player_info.scene_bin else {
            continue;
        };
        let Some(ref player_avatar_bin) = player_info.avatar_bin else {
            continue;
        };

        let enter_type;
        if *scene_id == player_scene_bin.my_cur_scene_id {
            enter_type = snezhnaya_gi_proto::normal::EnterType::EnterGoto;
        } else {
            player_info.dungeon_bin = None;
            enter_type = snezhnaya_gi_proto::normal::EnterType::EnterJump;
        }

        let destination = VectorBin::from((destination.0, destination.1, destination.2));

        // Move directly in the same scene.
        if (*enter_reason == EnterReason::Gm
            || *enter_reason == EnterReason::Lua
            || *enter_reason == EnterReason::LuaSkipUi)
            && player_scene_bin.my_cur_scene_id == *scene_id
        {
            player_scene_bin.my_cur_scene_id = *scene_id;
            player_scene_bin.my_cur_scene_pos = destination.into();

            for (avatar_entity, avatar_data) in player_avatar_entities.iter().filter(|(_, data)| {
                data.owner_player_uid.0 == *uid && data.guid.0 == player_avatar_bin.cur_avatar_guid
            }) {
                commands
                    .entity(avatar_entity)
                    .remove::<CurrentPlayerAvatarMarker>()
                    .remove::<Visible>();

                disappear_events.write(EntityDisappearEvent(
                    avatar_data.entity_id.0,
                    VisionType::VisionMiss.into(),
                ));

                commands
                    .entity(avatar_entity)
                    .insert(CurrentPlayerAvatarMarker)
                    .insert(Visible)
                    .insert(Transform {
                        position: player_scene_bin.my_cur_scene_pos.unwrap_or_default().into(),
                        rotation: player_scene_bin.my_cur_scene_rot.unwrap_or_default().into(),
                    });
            }
            continue;
        };

        player_scene_bin.my_cur_scene_id = *scene_id;
        player_scene_bin.my_cur_scene_pos = destination.into();
        enter_events.write(BeginEnterSceneEvent {
            uid: *uid,
            scene_id: *scene_id,
            dungeon_id: 0,
            enter_type,
            enter_reason: *enter_reason,
            position: destination,
        });
    }
}

pub fn player_jump_by_point(
    mut events: MessageReader<ScenePlayerJumpByPointEvent>,
    mut players: ResMut<Players>,
    mut enter_events: MessageWriter<BeginEnterSceneEvent>,
) {
    for ScenePlayerJumpByPointEvent(uid, scene_id, point_id) in events.read() {
        let Some((dest, point)) =
            snezhnaya_gi_data::scene::scene_point_config::resolve_point(*scene_id, *point_id)
        else {
            continue;
        };
        let Some(player) = players.get_mut(*uid) else {
            continue;
        };
        let previous = player.scene_bin.as_ref().map_or(0, |s| s.my_cur_scene_id);
        let quitting = player
            .dungeon_bin
            .as_ref()
            .is_some_and(|d| d.cur_dungeon_id != 0);
        snezhnaya_gi_persistence::restore_dungeon_team(player);
        player.dungeon_bin = None;
        let position = VectorBin::from((point.tran_pos.x, point.tran_pos.y, point.tran_pos.z));
        let Some(scene) = player.scene_bin.as_mut() else {
            continue;
        };
        scene.my_prev_scene_id = previous;
        scene.my_cur_scene_id = dest;
        scene.my_cur_scene_pos = Some(position);
        scene.my_cur_scene_rot =
            Some((point.tran_rot.x, point.tran_rot.y, point.tran_rot.z).into());
        enter_events.write(BeginEnterSceneEvent {
            uid: *uid,
            scene_id: dest,
            dungeon_id: 0,
            enter_type: if previous == dest {
                snezhnaya_gi_proto::normal::EnterType::EnterGoto
            } else {
                snezhnaya_gi_proto::normal::EnterType::EnterJump
            },
            enter_reason: if quitting {
                EnterReason::DungeonQuit
            } else {
                EnterReason::TransPoint
            },
            position,
        });
    }
}

pub fn player_enter_dungeon(
    mut events: MessageReader<ScenePlayerEnterDungeonEvent>,
    mut players: ResMut<Players>,
    mut enter_events: MessageWriter<BeginEnterSceneEvent>,
    mut quest_content_events: MessageWriter<QuestContentProgressEvent>,
    world_owner_uid: Res<WorldOwnerUID>,
) {
    let dungeon_excel_config_collection_clone =
        Arc::clone(snezhnaya_gi_data::excel::dungeon_excel_config_collection::get());

    let scene_config_collection_clone =
        snezhnaya_gi_data::scene::script_cache::get_scene_config_collection();

    for ScenePlayerEnterDungeonEvent(uid, dungeon_id) in events.read() {
        match dungeon_excel_config_collection_clone.get(&dungeon_id) {
            None => {}
            Some(dungeon_config) => {
                let tran_scene_id = dungeon_config.scene_id;

                let Some(scene_config) = scene_config_collection_clone.get(&tran_scene_id) else {
                    continue;
                };

                let Some(player_info) = players.get_mut(*uid) else {
                    continue;
                };

                let destination = VectorBin::from((
                    scene_config.scene_config.born_pos.x,
                    scene_config.scene_config.born_pos.y,
                    scene_config.scene_config.born_pos.z,
                ));

                if let Some(ref mut player_scene_bin) = player_info.scene_bin {
                    match &player_info.dungeon_bin {
                        None => {
                            player_info.dungeon_bin = Some(PlayerDungeonCompBin {
                                cur_dungeon_id: *dungeon_id,
                                is_has_quit_target_pos: true,
                                quit_scene_id: player_scene_bin.my_cur_scene_id,
                                quit_pos: player_scene_bin.my_cur_scene_pos,
                                quit_rot: player_scene_bin.my_cur_scene_rot,
                                ..Default::default()
                            });
                        }
                        Some(pre_data) => {
                            player_info.dungeon_bin = Some(PlayerDungeonCompBin {
                                cur_dungeon_id: *dungeon_id,
                                is_has_quit_target_pos: true,
                                quit_scene_id: pre_data.quit_scene_id,
                                quit_pos: pre_data.quit_pos,
                                quit_rot: pre_data.quit_rot,
                                is_ley_line_challenge: pre_data.is_ley_line_challenge,
                                original_avatar_guids: pre_data.original_avatar_guids.clone(),
                                original_avatar_guid: pre_data.original_avatar_guid,
                                ..Default::default()
                            });
                        }
                    }

                    player_scene_bin.my_cur_scene_id = tran_scene_id;
                    player_scene_bin.my_cur_scene_pos = destination.into();
                }
                cache_set_scene_level(world_owner_uid.0, tran_scene_id, dungeon_config.show_level);
                enter_events.write(BeginEnterSceneEvent {
                    uid: *uid,
                    scene_id: tran_scene_id,
                    dungeon_id: *dungeon_id,
                    enter_type: snezhnaya_gi_proto::normal::EnterType::EnterDungeon,
                    enter_reason: EnterReason::DungeonEnter,
                    position: destination,
                });
                quest_content_events.write(QuestContentProgressEvent {
                    player_uid: *uid,
                    content_type: QuestContent::EnterDungeon,
                    param: *dungeon_id,
                    param2: 0,
                    param3: 0,
                    add_progress: 1,
                });
            }
        }
    }
}
