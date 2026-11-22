use super::common::{AddProp, IdCountConfig};
use std::collections::HashMap;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
#[derive(Default)]
pub struct WeaponPromoteExcelConfig {
    pub weapon_promote_id: u32,
    pub promote_level: u32,
    pub required_player_level: u32,
    pub coin_cost: u32,
    pub unlock_max_level: u32,
    pub cost_items: Vec<IdCountConfig>,
    pub add_props: Vec<AddProp>,
}

pub trait WeaponPromoteExcelConfigKeyed<K> {
    fn load(excel_bin_output_path: &str) -> HashMap<K, WeaponPromoteExcelConfig>;
}

impl WeaponPromoteExcelConfigKeyed<u32> for WeaponPromoteExcelConfig {
    fn load(excel_bin_output_path: &str) -> HashMap<u32, WeaponPromoteExcelConfig> {
        let path = format!("{excel_bin_output_path}/WeaponPromoteExcelConfigData.json");
        let rows: Vec<Self> = serde_json::from_slice(&std::fs::read(&path).expect(&path))
            .expect("invalid weapon promotion configuration");
        rows.into_iter()
            .map(|row| ((row.weapon_promote_id << 8) | row.promote_level, row))
            .collect()
    }
}
