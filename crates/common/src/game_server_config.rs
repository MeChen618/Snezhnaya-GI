use crate::database_config::DatabaseSettings;
use crate::TomlConfig;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct GameServerConfig {
    #[serde(skip)]
    pub language: u32,
    pub network: NetworkSettings,
    pub plugin: PluginSettings,
    pub database: DatabaseSettings,
    #[serde(default)]
    pub sdk: SdkSettings,
    #[serde(default)]
    pub activities: Vec<ActivitySettings>,
    pub cur_region_name: String,
    pub region_list_path: String,
    pub encryption_config_path: String,
}

#[derive(Deserialize)]
pub struct NetworkSettings {
    pub gate_host: String,
}

#[derive(Deserialize, Clone, Copy, Default)]
pub enum SdkProvider {
    #[default]
    #[serde(rename = "astaps")]
    AstaPs,
    #[serde(rename = "hoyo_sdk")]
    HoyoSdk,
}

#[derive(Deserialize)]
#[serde(default)]
pub struct SdkSettings {
    pub provider: SdkProvider,
    pub verify_url: String,
    pub dispatch_key: String,
    pub encryption_key: String,
    pub timeout_ms: u64,
    pub max_pending: usize,
}

impl Default for SdkSettings {
    fn default() -> Self {
        Self {
            provider: SdkProvider::AstaPs,
            verify_url: "ws://127.0.0.1:1111".to_string(),
            dispatch_key: String::new(),
            encryption_key: String::new(),
            timeout_ms: 3000,
            max_pending: 64,
        }
    }
}

#[derive(Deserialize)]
pub struct PluginSettings {
    pub packet_log: bool,
    pub ability: bool,
    pub ability_log: bool,
    pub social: bool,
    pub quest: bool,
}

impl TomlConfig for GameServerConfig {
    const DEFAULT_TOML: &str = include_str!("../game-server.default.toml");
}

#[derive(Clone, Deserialize)]
pub struct ActivitySettings {
    pub activity_id: u32,
    pub activity_type: u32,
    pub schedule_id: u32,
    pub begin_time: u32,
    pub end_time: u32,
}
