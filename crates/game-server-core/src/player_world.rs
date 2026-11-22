use std::collections::HashMap;

use crate::player_data_sync::PlayerDataSyncPlugin;
use bevy_app::prelude::*;
use common::player_cache::cache_get_player_client_data_version;
use snezhnaya_gi_ability::AbilityPlugin;
use snezhnaya_gi_avatar::AvatarPlugin;
use snezhnaya_gi_banner::BannerPlugin;
use snezhnaya_gi_combat::CombatPlugin;
use snezhnaya_gi_command::CommandPlugin;
use snezhnaya_gi_data::{GAME_SERVER_CONFIG, REGION_LIST};
use snezhnaya_gi_entity::EntityPlugin;
use snezhnaya_gi_environment::EnvironmentPlugin;
use snezhnaya_gi_event::scene::{WorldOwnerUID, WorldVersionConfig};
use snezhnaya_gi_event::EventRegistryPlugin;
use snezhnaya_gi_inventory::InventoryPlugin;
use snezhnaya_gi_luashell::{LuaShellPlugin, LuaShellSettings};
use snezhnaya_gi_map::MapPlugin;
use snezhnaya_gi_message::{
    event::ClientMessageEvent,
    get_player_version,
    output::{ClientOutput, MessageOutput},
};
use snezhnaya_gi_misc::MiscPlugin;
use snezhnaya_gi_pathfinding::PathfindingPlugin;
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::dy_parser::get_ty_value_by_version;
use snezhnaya_gi_proto::normal::{PlayerLoginRsp, ResVersionConfig};
use snezhnaya_gi_proto::server_only::PlayerDataBin;
use snezhnaya_gi_proto::Protobuf;
use snezhnaya_gi_quest::QuestPlugin;
use snezhnaya_gi_scene::ScenePlugin;
use snezhnaya_gi_script::ScriptPlugin;
use snezhnaya_gi_social::SocialPlugin;

pub struct PlayerWorld(App);

impl Drop for PlayerWorld {
    fn drop(&mut self) {
        if let Some(queue) = self
            .0
            .world()
            .get_resource::<snezhnaya_gi_script::ScriptCommandQueue>()
        {
            // 队列里的 Lua 表可能反向持有虚拟机，离线时先断开引用。
            while queue.0.pop().is_some() {}
        }
    }
}

impl PlayerWorld {
    pub fn new(player_information: PlayerDataBin, output: ClientOutput) -> Self {
        let uid = player_information.uid;

        let message_out = MessageOutput::new(HashMap::from([(uid, output.clone())]));
        let players = Players::from(HashMap::from([(uid, player_information)]));

        let mut app = App::new();
        app.insert_resource(message_out)
            .insert_resource(players)
            .add_message::<ClientMessageEvent>();

        app.add_plugins(EventRegistryPlugin);

        app.world_mut()
            .get_resource_mut::<WorldOwnerUID>()
            .unwrap()
            .0 = uid;

        let version = get_player_version!(&uid);

        app.world_mut()
            .get_resource_mut::<WorldVersionConfig>()
            .unwrap()
            .protocol_version = version.clone();

        app.world_mut()
            .get_resource_mut::<WorldVersionConfig>()
            .unwrap()
            .ty_value = get_ty_value_by_version(version.as_str());

        app.add_plugins(PlayerDataSyncPlugin)
            .add_plugins(EntityPlugin)
            .add_plugins(ScenePlugin)
            .add_plugins(AvatarPlugin)
            .add_plugins(BannerPlugin)
            .add_plugins(InventoryPlugin)
            .add_plugins(EnvironmentPlugin)
            .add_plugins(PathfindingPlugin)
            .add_plugins(CombatPlugin)
            .add_plugins(MiscPlugin)
            .add_plugins(CommandPlugin)
            .add_plugins(MapPlugin)
            .add_plugins(LuaShellPlugin)
            .add_plugins(ScriptPlugin);

        if GAME_SERVER_CONFIG.plugin.ability {
            app.add_plugins(AbilityPlugin);
        }

        if GAME_SERVER_CONFIG.plugin.social {
            app.add_plugins(SocialPlugin);
        }

        if GAME_SERVER_CONFIG.plugin.quest {
            app.add_plugins(QuestPlugin);
        }

        app.insert_resource(LuaShellSettings {
            startup_payloads: vec![include_bytes!("../../../assets/luashell/wm.bin")
                .to_vec()
                .into_boxed_slice()],
        });

        app.finish();
        app.cleanup();
        app.update();

        let binding = get_player_version!(&uid);
        let version = binding.as_str();
        let client_data_version = cache_get_player_client_data_version(uid).unwrap_or_default();

        let mut cur_hot_fix_data = None;

        REGION_LIST.get().unwrap().iter().for_each(|region| {
            region.hot_fix_data.iter().for_each(|(_, hot_fix_data)| {
                if hot_fix_data.client_data_version == client_data_version {
                    cur_hot_fix_data = Some(hot_fix_data.clone());
                }
            })
        });

        match cur_hot_fix_data {
            None => {
                output.push_none(
                    snezhnaya_gi_proto::packet_head::PacketHead::default(),
                    version,
                    "PlayerLoginRsp",
                );
            }
            Some(cur_hot_fix_data) => {
                output.push(
                    snezhnaya_gi_proto::packet_head::PacketHead::default(),
                    version,
                    "PlayerLoginRsp",
                    PlayerLoginRsp {
                        client_md5: cur_hot_fix_data.client_data_md5.clone(),
                        client_silence_md5: cur_hot_fix_data.client_silence_data_md5.clone(),
                        client_data_version: cur_hot_fix_data.client_data_version.clone(),
                        client_silence_data_version: cur_hot_fix_data
                            .client_silence_data_version
                            .clone(),
                        client_version_suffix: cur_hot_fix_data.client_version_suffix.clone(),
                        client_silence_version_suffix: cur_hot_fix_data
                            .client_silence_version_suffix
                            .clone(),

                        res_version_config: Some(ResVersionConfig {
                            version: cur_hot_fix_data.res_version_config.version.clone(),
                            md5: cur_hot_fix_data.res_version_config.md5.clone(),
                            release_total_size: cur_hot_fix_data
                                .res_version_config
                                .release_total_size
                                .clone(),
                            version_suffix: cur_hot_fix_data
                                .res_version_config
                                .version_suffix
                                .clone(),
                            branch: cur_hot_fix_data.res_version_config.branch.clone(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                );
            }
        }

        tracing::debug!("created world for player: {uid}");

        Self(app)
    }

    pub fn add_packet(
        &mut self,
        head: snezhnaya_gi_proto::packet_head::PacketHead,
        cmd_id: u16,
        data: Box<[u8]>,
        message_name: String,
    ) {
        self.0
            .world_mut()
            .write_message(ClientMessageEvent::new(head, cmd_id, data, message_name));
    }

    pub fn update(&mut self) {
        self.0.update();
    }

    pub fn serialize_player_information(&mut self, uid: u32) -> Vec<u8> {
        let Some(players) = self.0.world_mut().get_resource::<Players>() else {
            return vec![];
        };
        match players.get(uid) {
            None => {
                vec![]
            }
            Some(player) => snezhnaya_gi_persistence::player_snapshot(player).encode_to_vec(),
        }
    }

    pub fn should_save(&mut self, uid: u32) -> bool {
        let Some(players) = self.0.world_mut().get_resource::<Players>() else {
            return false;
        };
        let Some(player_info) = players.get(uid) else {
            return false;
        };
        player_info.scene_bin.is_some()
    }
}
