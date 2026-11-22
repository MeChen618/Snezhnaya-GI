use crate::net::udp_server::ConnectionManager;
use anyhow::{Context, Result};
use common::data::{EncryptionConfig, RegionConfig};
use common::logging;
use common::player_cache::init_player_cache;
use dashmap::DashMap;
use db_worker::DbWorkerHandle;
use game_server_core::LogicSimulator;
use net::UdpServer;
use snezhnaya_gi_data::ability::load_ability_configs_from_bin;
use snezhnaya_gi_data::config::load_avatar_talent_configs_from_bin;
use snezhnaya_gi_data::custom::{CombinedDrop, GadgetMapping};
use snezhnaya_gi_data::quest::quest_config::load_quest_configs_from_bin;
use snezhnaya_gi_data::scene::scene_point_config::load_scene_point_configs_from_bin;
use snezhnaya_gi_data::scene::script_cache::{init_scene_static_templates, load_lua_vm};
use snezhnaya_gi_data::{
    config::load_avatar_configs_from_bin, config::load_gadget_configs_from_bin, custom, excel,
    GAME_SERVER_CONFIG, REGION_LIST,
};
use snezhnaya_gi_encryption::{rsa::RsaKeyPair, xor::MhyXorpad};
use std::collections::HashMap;
use std::fs;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};
use tokio::net::UdpSocket;

mod db_worker;
mod player_info_util;

mod handler;
mod net;
mod util;

struct AppState {
    pub socket: Arc<UdpSocket>,
    pub conn_mgr: Arc<ConnectionManager>,
    pub db_handle: DbWorkerHandle,
    pub logic_simulator: LogicSimulator,
    region_config: RegionConfig,
    sessions: DashMap<u32, Arc<handler::Session>>,
    key_pair_map: HashMap<u32, RsaKeyPair>,
    initial_xor_pad: Option<MhyXorpad>,
    stop_flag: AtomicBool,
}

#[tokio::main]
async fn main() -> Result<()> {
    logging::init();
    logging::set_ability_log(GAME_SERVER_CONFIG.plugin.ability_log);

    snezhnaya_gi_proto::dy_parser::init()?;

    init_player_cache();

    snezhnaya_gi_message::PLAYER_VERSION
        .set(Arc::new(DashMap::new()))
        .expect("TODO: panic message");

    static STATE: OnceLock<AppState> = OnceLock::new();

    load_resources()?;
    if std::env::args().any(|argument| argument == "--check-resources") {
        tracing::info!("7.2 resources and protocols validated");
        return Ok(());
    }

    let db_connection = snezhnaya_gi_database::connect_to(&GAME_SERVER_CONFIG.database)?;
    let (db_handle, save_data_tx) = db_worker::start(db_connection);

    let region_list: Vec<RegionConfig> = serde_json::from_str(
        &common::string_util::read_utf8_no_bom(&GAME_SERVER_CONFIG.region_list_path)?,
    )?;

    let _ = REGION_LIST.set(region_list.clone());

    let key_pair_map = serde_json::from_str::<HashMap<u32, EncryptionConfig>>(
        &common::string_util::read_utf8_no_bom(&GAME_SERVER_CONFIG.encryption_config_path)?,
    )?
    .into_iter()
    .map(|(id, conf)| (id, RsaKeyPair::from_encryption_config(&conf)))
    .collect();

    let cur_region = region_list
        .into_iter()
        .find(|r| r.name == GAME_SERVER_CONFIG.cur_region_name)
        .expect("cur_region not found in region list");

    let initial_xor_pad = if let Some(secret_key_path) = cur_region.secret_key_path.as_ref() {
        Some(MhyXorpad::from_ec2b(&fs::read(secret_key_path)?)?)
    } else {
        None
    };

    let socket = Arc::new(
        UdpSocket::bind(GAME_SERVER_CONFIG.network.gate_host.clone())
            .await
            .with_context(|| {
                format!("binding game UDP {}", GAME_SERVER_CONFIG.network.gate_host)
            })?,
    );
    tracing::info!(address = %GAME_SERVER_CONFIG.network.gate_host, "7.2 game server listening");

    let state = STATE.get_or_init(move || AppState {
        socket: Arc::clone(&socket),
        conn_mgr: Arc::new(ConnectionManager::default()),
        db_handle,
        logic_simulator: LogicSimulator::spawn(save_data_tx),
        region_config: cur_region,
        sessions: DashMap::new(),
        initial_xor_pad,
        key_pair_map,
        stop_flag: AtomicBool::new(false),
    });

    let udp_server = UdpServer::bind(state).await?;
    udp_server.serve(state).await;

    Ok(())
}

fn load_resources() -> Result<()> {
    excel::load_all("assets/ExcelBinOutput").context("loading ExcelBinOutput")?;
    custom::load_all("assets/custom").context("loading server tables")?;
    CombinedDrop::load("assets/custom");
    GadgetMapping::load("assets/custom");
    if GAME_SERVER_CONFIG.plugin.ability {
        load_ability_configs_from_bin("assets/BinOutput")
            .context("loading ability configurations")?;
    }
    load_avatar_configs_from_bin("assets/BinOutput").context("loading avatar configurations")?;
    load_gadget_configs_from_bin("assets/BinOutput").context("loading gadget configurations")?;
    load_avatar_talent_configs_from_bin("assets/BinOutput")
        .context("loading talent configurations")?;
    load_quest_configs_from_bin("assets/BinOutput");
    load_scene_point_configs_from_bin("assets/BinOutput");
    load_lua_vm("assets/lua/common");
    init_scene_static_templates("assets/lua/scene");
    let avatars = excel::avatar_excel_config_collection::get().len();
    let points = snezhnaya_gi_data::scene::scene_point_config::get_scene_point_config_collection();
    let world_points = points.get(&3).map(|scene| scene.points.len()).unwrap_or(0);
    let scenes = snezhnaya_gi_data::scene::script_cache::get_scene_config_collection();
    if avatars == 0 || world_points == 0 || !scenes.contains_key(&3) {
        return Err(anyhow::anyhow!(
            "required avatar, scene 3 points or scene 3 Lua data is empty"
        ));
    }
    tracing::info!(
        avatars,
        world_points,
        scenes = scenes.len(),
        "required resources loaded"
    );
    Ok(())
}
