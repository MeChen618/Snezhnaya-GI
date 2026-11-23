use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use snezhnaya_gi_data::excel::{SceneTagConfig, SceneTagConfigKeyed};
use snezhnaya_gi_data::scene::scene_point_config::ScenePointConfig;
use snezhnaya_gi_event::scene::*;
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::{record_progress, Players};
use snezhnaya_gi_proto::retcode::Retcode;
use snezhnaya_gi_proto::server_only::{CityBin, PlayerDataBin, SceneAreaCompBin, SceneBin};
use std::sync::Arc;

mod handbook;
mod on_map;

pub struct MapPlugin;

impl Plugin for MapPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreStartup, initialize_maps)
            .add_systems(Startup,on_map::login_marks)
            .add_systems(Update, handbook::handle)
            .add_systems(PreUpdate, data_request_processor)
            .add_systems(PreUpdate, on_map::message_on_map)
            .add_systems(PreUpdate, sync_scene_info_list_on_scene_init)
            .add_systems(PreUpdate, sync_group_unlimit_point_list_on_post_enter_scene);
    }
}

fn scene_state<'a>(
    player: &'a mut PlayerDataBin,
    scene_id: u32,
    config: &ScenePointConfig,
) -> &'a mut SceneBin {
    let scenes = &mut player
        .scene_bin
        .get_or_insert_with(Default::default)
        .world
        .get_or_insert_with(Default::default)
        .scene_map;
    if let std::collections::hash_map::Entry::Vacant(entry) = scenes.entry(scene_id) {
        // 沿用默认全解锁策略，已有存档不覆盖。
        let mut points: Vec<_> = config.points.keys().copied().collect();
        points.sort_unstable();
        let mut areas: Vec<_> = (1..=1000)
            .chain(
                config
                    .points
                    .values()
                    .map(|p| p.area_id)
                    .filter(|id| *id != 0),
            )
            .collect();
        areas.sort_unstable();
        areas.dedup();
        entry.insert(SceneBin {
            unlocked_point_list: points,
            area_comp_bin: Some(SceneAreaCompBin {
                unlocked_area_list: areas,
                city_map: if scene_id == 3 {
                    (1..=5)
                        .map(|id| {
                            (
                                id,
                                CityBin {
                                    level: 10,
                                    crystal_num: 10,
                                },
                            )
                        })
                        .collect()
                } else {
                    Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        });
        record_progress(player, "map", scene_id, 1);
    }
    player
        .scene_bin
        .as_mut()
        .unwrap()
        .world
        .as_mut()
        .unwrap()
        .scene_map
        .get_mut(&scene_id)
        .unwrap()
}
fn initialize_maps(mut players: ResMut<Players>) {
    let configs = snezhnaya_gi_data::scene::scene_point_config::get_scene_point_config_collection();
    if let Some(config) = configs.get(&3) {
        for player in players.values_mut() {
            scene_state(player, 3, config);
        }
    }
}

fn data_request_processor(
    mut events: MessageReader<ClientMessageEvent>,
    mut players: ResMut<Players>,
    message_output: Res<MessageOutput>,
) {
    use snezhnaya_gi_proto::normal::*;

    let scene_point_config_collection_clone =
        snezhnaya_gi_data::scene::scene_point_config::get_scene_point_config_collection();

    let daily_dungeon_config_collection_clone =
        Arc::clone(snezhnaya_gi_data::excel::daily_dungeon_config_collection::get());

    for message in events.read() {
        match message.message_name() {
            "GetSceneAreaReq" => {
                if let Some(request) = message.decode::<GetSceneAreaReq>() {
                    let Some(player) = players.get_mut(message.sender_uid()) else {
                        continue;
                    };
                    let Some(config) = scene_point_config_collection_clone.get(&request.scene_id)
                    else {
                        message_output.send(
                            message.sender_uid(),
                            "GetSceneAreaRsp",
                            GetSceneAreaRsp {
                                retcode: Retcode::RetNotFoundConfig.into(),
                                scene_id: request.scene_id,
                                ..Default::default()
                            },
                        );
                        continue;
                    };
                    let state = scene_state(player, request.scene_id, config);
                    let area = state.area_comp_bin.as_ref();
                    message_output.send(
                        message.sender_uid(),
                        "GetSceneAreaRsp",
                        GetSceneAreaRsp {
                            scene_id: request.scene_id,
                            area_id_list: area.map_or(vec![], |a| a.unlocked_area_list.clone()),
                            city_info_list: area
                                .into_iter()
                                .flat_map(|a| a.city_map.iter())
                                .map(|(&city_id, c)| CityInfo {
                                    city_id,
                                    level: c.level,
                                    crystal_num: c.crystal_num,
                                })
                                .collect(),
                            ..Default::default()
                        },
                    );
                }
            }
            "GetScenePointReq" => {
                if let Some(request) = message.decode::<GetScenePointReq>() {
                    match scene_point_config_collection_clone.get(&request.scene_id) {
                        None => {
                            message_output.send(
                                message.sender_uid(),
                                "GetScenePointRsp",
                                GetScenePointRsp {
                                    retcode: Retcode::RetNotFoundConfig.into(),
                                    scene_id: request.scene_id,
                                    ..Default::default()
                                },
                            );
                        }
                        Some(scene_config) => {
                            let owner = if request.belong_uid == 0 {
                                message.sender_uid()
                            } else {
                                request.belong_uid
                            };
                            let Some(player) = players.get_mut(owner) else {
                                message_output.send(
                                    message.sender_uid(),
                                    "GetScenePointRsp",
                                    GetScenePointRsp {
                                        retcode: Retcode::RetFail.into(),
                                        scene_id: request.scene_id,
                                        belong_uid: owner,
                                        ..Default::default()
                                    },
                                );
                                continue;
                            };
                            let state = scene_state(player, request.scene_id, scene_config);
                            message_output.send(
                                message.sender_uid(),
                                "GetScenePointRsp",
                                GetScenePointRsp {
                                    is_relogin: request.is_relogin,
                                    belong_uid: owner,
                                    scene_id: request.scene_id,
                                    unlock_area_list: state
                                        .area_comp_bin
                                        .as_ref()
                                        .map_or(vec![], |a| a.unlocked_area_list.clone()),
                                    unlocked_point_list: state.unlocked_point_list.clone(),
                                    unhide_point_list: state.unlocked_point_list.clone(),
                                    ..Default::default()
                                },
                            )
                        }
                    }
                }
            }
            "DungeonEntryInfoReq" => {
                if let Some(request) = message.decode::<DungeonEntryInfoReq>() {
                    match scene_point_config_collection_clone.get(&request.scene_id) {
                        None => {}
                        Some(scene_config) => match scene_config.points.get(&request.point_id) {
                            None => {}
                            Some(point_config) => {
                                let mut dungeon_entry_list: Vec<DungeonEntryInfo> = vec![];
                                point_config.dungeon_ids.iter().for_each(|dungeon_id| {
                                    dungeon_entry_list.push(DungeonEntryInfo {
                                        dungeon_id: *dungeon_id,
                                        ..Default::default()
                                    });
                                });
                                point_config
                                    .dungeon_random_list
                                    .iter()
                                    .for_each(|random_id| {
                                        match daily_dungeon_config_collection_clone.get(random_id) {
                                            None => {}
                                            Some(daily_dungeon_config) => {
                                                daily_dungeon_config.sunday.iter().for_each(
                                                    |dungeon_id| {
                                                        dungeon_entry_list.push(DungeonEntryInfo {
                                                            dungeon_id: *dungeon_id,
                                                            ..Default::default()
                                                        });
                                                    },
                                                );
                                            }
                                        }
                                    });
                                message_output.send(
                                    message.sender_uid(),
                                    "DungeonEntryInfoRsp",
                                    DungeonEntryInfoRsp {
                                        point_id: request.point_id,
                                        dungeon_entry_list,
                                        ..Default::default()
                                    },
                                );
                            }
                        },
                    }
                }
            }
            &_ => {}
        }
    }
}

pub fn sync_scene_info_list_on_scene_init(
    mut events: MessageReader<SceneInitFinishEvent>,
    message_output: Res<MessageOutput>,
) {
    use snezhnaya_gi_proto::normal::*;

    let scene_tag_entries_clone = Arc::clone(SceneTagConfig::get_scene_tag_entries());

    let mut info_list: Vec<PlayerWorldSceneInfo> = vec![];

    [3, 5, 6, 7, 11, 101, 103].iter().for_each(|scene_id| {
        match scene_tag_entries_clone.get(scene_id) {
            None => {}
            Some(scene_tag_list) => {
                let mut scene_tag_id_list: Vec<u32> = vec![];
                scene_tag_list.iter().for_each(|item| {
                    scene_tag_id_list.push(item.id);
                });
                if *scene_id == 3 {
                    info_list.push(PlayerWorldSceneInfo {
                        is_locked: false,
                        scene_id: 3,
                        map_layer_info: Some(MapLayerInfo {
                            unlocked_map_layer_floor_id_list:
                                snezhnaya_gi_data::excel::get_map_layer_floor_config_collection()
                                    .to_vec(),
                            unlocked_layer_group_list:
                                snezhnaya_gi_data::excel::get_map_layer_group_config_collection()
                                    .to_vec(),
                            unlocked_map_layer_id_list:
                                snezhnaya_gi_data::excel::get_map_layer_config_collection().to_vec(),
                        }),
                        scene_tag_id_list,
                        ..Default::default()
                    });
                } else if *scene_id == 101 {
                    info_list.push(PlayerWorldSceneInfo {
                        is_locked: false,
                        scene_id: 101,
                        map_layer_info: Some(MapLayerInfo {
                            unlocked_map_layer_id_list: vec![
                                1018000101, 1018000102, 1018000103, 1018000106, 1018000107,
                                1018000108, 1018000109, 1018000110, 1018000111, 1018000112,
                            ],
                            unlocked_layer_group_list: vec![10180001],
                            unlocked_map_layer_floor_id_list: vec![
                                1018000101, 1018000102, 1018000110, 1018000111, 1018000112,
                            ],
                        }),
                        scene_tag_id_list,
                        ..Default::default()
                    });
                } else if *scene_id == 103 {
                    info_list.push(PlayerWorldSceneInfo {
                        is_locked: false,
                        scene_id: 103,
                        map_layer_info: Some(MapLayerInfo {
                            unlocked_map_layer_id_list: vec![
                                101, 102, 103, 201, 301, 302, 401, 501, 502, 503, 601, 602, 603,
                                604, 605, 606, 608, 609, 610, 611, 612, 613, 702, 901, 902,
                            ],
                            unlocked_layer_group_list: vec![1, 2, 3, 6, 9],
                            unlocked_map_layer_floor_id_list: vec![
                                101, 102, 103, 201, 301, 302, 401, 501, 502, 503, 602, 605, 608,
                                610, 702, 901, 902,
                            ],
                        }),
                        scene_tag_id_list,
                        ..Default::default()
                    });
                } else {
                    info_list.push(PlayerWorldSceneInfo {
                        is_locked: false,
                        scene_id: *scene_id,
                        map_layer_info: None,
                        scene_tag_id_list,
                        ..Default::default()
                    });
                }
            }
        }
    });

    for SceneInitFinishEvent(uid) in events.read() {
        message_output.send(
            *uid,
            "PlayerWorldSceneInfoListNotify",
            PlayerWorldSceneInfoListNotify {
                unlocked_area_id_list: vec![4, 5, 6, 11, 14, 19, 22],
                info_list: info_list.clone(),
            },
        );
    }
}

pub fn sync_group_unlimit_point_list_on_post_enter_scene(
    mut reader: MessageReader<PostEnterSceneEvent>,
    players: Res<Players>,
    out: Res<MessageOutput>,
) {
    let scene_point_config_collection_clone =
        snezhnaya_gi_data::scene::scene_point_config::get_scene_point_config_collection();

    for PostEnterSceneEvent(uid) in reader.read() {
        let Some(player_info) = players.get(*uid) else {
            continue;
        };
        let scene_id = if let Some(ref player_scene_bin) = player_info.scene_bin {
            player_scene_bin.my_cur_scene_id
        } else {
            continue;
        };
        match scene_point_config_collection_clone.get(&scene_id) {
            None => {}
            Some(scene_config) => {
                scene_config
                    .points
                    .iter()
                    .filter(|(_point_id, point_data)| {
                        point_data.r#point_type == "DungeonEntry" && point_data.group_limit
                    })
                    .for_each(|(point_id, _point_data)| {
                        out.send(
                            *uid,
                            "UnfreezeGroupLimitNotify",
                            snezhnaya_gi_proto::normal::UnfreezeGroupLimitNotify {
                                point_id: *point_id,
                                scene_id,
                            },
                        );
                    });
            }
        }
    }
}
