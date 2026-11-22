use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use common::time_util;
use snezhnaya_gi_data::excel;
use snezhnaya_gi_entity::int_prop_map;
use snezhnaya_gi_message::output::MessageOutput;
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::normal::*;

pub struct PlayerDataSyncPlugin;

impl Plugin for PlayerDataSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            (
                sync_player_data,
                sync_player_store,
                sync_open_state_map,
                sync_avatar_data,
                sync_quest_list,
            )
                .chain(),
        );
    }
}

pub fn sync_player_data(players: Res<Players>, message_output: Res<MessageOutput>) {
    for uid in players.keys() {
        let Some(player_info) = players.get(*uid) else {
            continue;
        };
        let Some(ref player_basic_bin) = player_info.basic_bin else {
            continue;
        };
        let empty_items = Default::default();
        let items = player_info.item_bin.as_ref().unwrap_or(&empty_items);
        message_output.send(
            *uid,
            "PlayerDataNotify",
            PlayerDataNotify {
                nick_name: player_basic_bin.nickname.clone(),
                prop_map: int_prop_map! {
                    PROP_PLAYER_WORLD_LEVEL: 9;
                    PROP_IS_SPRING_AUTO_USE: 1;
                    PROP_SPRING_AUTO_USE_PERCENT: 50;
                    PROP_IS_FLYABLE: 1;
                    PROP_IS_GAME_TIME_LOCKED: player_basic_bin.is_game_time_locked as i64;
                    PROP_IS_TRANSFERABLE: 1;
                    PROP_MAX_STAMINA: 24000;
                    PROP_CUR_PERSIST_STAMINA: 24000;
                    PROP_PLAYER_LEVEL: player_basic_bin.level;
                    PROP_PLAYER_EXP: player_basic_bin.exp;
                    PROP_PLAYER_MP_SETTING_TYPE :1;
                    PROP_IS_MP_MODE_AVAILABLE :1;
                    PROP_PLAYER_RESIN:items.resin_record.as_ref().map_or(0, |r| r.value);
                    PROP_IS_DIVEABLE :1;
                    PROP_CUR_PHLOGISTON :10000;
                    PROP_PLAYER_HCOIN :items.hcoin;
                    PROP_PLAYER_SCOIN :items.scoin;
                    PROP_PLAYER_MCOIN :items.mcoin;
                    PROP_PLAYER_HOME_COIN :items.home_coin;
                },
                server_time: time_util::unix_timestamp_ms(),
                is_first_login_today: false,
                region_id: 0,
            },
        )
    }
}

pub fn sync_player_store(players: Res<Players>, message_output: Res<MessageOutput>) {
    for uid in players.keys() {
        let Some(player_info) = players.get(*uid) else {
            continue;
        };
        let Some(ref player_item_bin) = player_info.item_bin else {
            continue;
        };

        message_output.send(
            *uid,
            "PlayerStoreNotify",
            PlayerStoreNotify {
                store_type: StoreType::StorePack.into(),
                weight_limit: 30_000,
                item_list: player_item_bin
                    .iter()
                    .filter_map(|(_guid, item)| item.to_normal_proto())
                    .collect(),
            },
        );
    }
}

pub fn sync_avatar_data(players: Res<Players>, message_output: Res<MessageOutput>) {
    for uid in players.keys() {
        let Some(player_info) = players.get(*uid) else {
            continue;
        };

        let Some(ref player_avatar_bin) = player_info.avatar_bin else {
            continue;
        };

        message_output.send(
            *uid,
            "AvatarDataNotify",
            AvatarDataNotify {
                choose_avatar_guid: player_avatar_bin.choose_avatar_guid,
                avatar_list: player_avatar_bin
                    .avatar_map
                    .values()
                    .filter_map(snezhnaya_gi_avatar::util::client_avatar_info)
                    .collect(),
                avatar_team_map: player_avatar_bin
                    .team_map
                    .iter()
                    .map(|(idx, team)| {
                        (
                            *idx,
                            AvatarTeam {
                                team_name: team.team_name.clone(),
                                avatar_guid_list: team.avatar_guid_list.clone(),
                            },
                        )
                    })
                    .collect(),
                cur_avatar_team_id: player_avatar_bin.cur_team_id,
                owned_flycloak_list: player_avatar_bin
                    .owned_flycloak_list
                    .iter()
                    .copied()
                    .collect(),
                owned_costume_list: player_avatar_bin
                    .owned_costume_list
                    .iter()
                    .copied()
                    .collect(),
                owned_trace_effect_list: player_avatar_bin
                    .owned_trace_effect_list
                    .iter()
                    .copied()
                    .collect(),
                ..Default::default()
            },
        );
    }
}

pub fn sync_open_state_map(players: Res<Players>, message_output: Res<MessageOutput>) {
    let open_state_config_collection_clone =
        std::sync::Arc::clone(excel::open_state_config_collection::get());

    for uid in players.keys() {
        message_output.send(
            *uid,
            "OpenStateUpdateNotify",
            OpenStateUpdateNotify {
                open_state_map: open_state_config_collection_clone
                    .values()
                    .map(|c| (c.id, 1))
                    .collect(),
            },
        );
    }
}

pub fn sync_quest_list(players: Res<Players>, message_output: Res<MessageOutput>) {
    for uid in players.keys() {
        let Some(player_info) = players.get(*uid) else {
            continue;
        };

        let Some(ref player_quest_bin) = player_info.quest_bin else {
            continue;
        };
        let Some(ref quest_bin) = player_quest_bin.quest_bin else {
            continue;
        };

        message_output.send(
            *uid,
            "QuestListNotify",
            QuestListNotify {
                quest_list: quest_bin
                    .quest_map
                    .iter()
                    .map(|(_sub_quest_id, quest_item)| Quest {
                        quest_id: quest_item.quest_id,
                        parent_quest_id: quest_item.parent_quest_id,
                        state: quest_item.state,
                        start_time: quest_item.start_time,
                        accept_time: quest_item.accept_time,
                        start_game_time: quest_item.start_game_time,
                        finish_progress_list: quest_item.finish_progress_list.clone(),
                        fail_progress_list: quest_item.fail_progress_list.clone(),
                        ..Default::default()
                    })
                    .collect(),
            },
        );

        if let Some(ref parent_quest_bin) = player_quest_bin.parent_quest_bin {
            let parent_quest_list: Vec<ParentQuest> = parent_quest_bin
                .parent_quest_map
                .iter()
                .filter(|(_, pq)| pq.state > 0)
                .map(|(_, pq)| pq.to_normal_proto())
                .collect();

            if !parent_quest_list.is_empty() {
                message_output.send(
                    *uid,
                    "FinishedParentQuestNotify",
                    FinishedParentQuestNotify { parent_quest_list },
                );
            }
        }

        if !player_quest_bin.quest_global_var_list.is_empty() {
            message_output.send(
                *uid,
                "QuestGlobalVarNotify",
                QuestGlobalVarNotify {
                    var_list: player_quest_bin
                        .quest_global_var_list
                        .iter()
                        .map(|v| QuestGlobalVar {
                            key: v.key,
                            value: v.value,
                        })
                        .collect(),
                },
            );
        }
    }
}
