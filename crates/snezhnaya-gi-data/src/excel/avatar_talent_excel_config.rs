use super::common::AddProp;
use common::string_util::InternString;
use std::collections::HashMap;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvatarTalentExcelConfig {
    pub talent_id: u32,
    pub add_props: Vec<AddProp>,
    #[serde(default)]
    pub main_cost_item_id: u32,
    #[serde(default, rename = "KCHDANFBCGD")]
    cost_material_id: u32,
    #[serde(default)]
    pub main_cost_item_count: u32,
    #[serde(default)]
    pub open_config: InternString,
    pub param_list: Vec<f32>,
    pub desc_text_map_hash: u64,
    pub name_text_map_hash: u64,
}

impl AvatarTalentExcelConfig {
    pub fn cost_item_id(&self) -> u32 {
        // 新资源同时保留两列，命座材料以实际消耗列为准。
        if self.cost_material_id != 0 {
            self.cost_material_id
        } else {
            self.main_cost_item_id
        }
    }
}

pub trait AvatarTalentExcelConfigKeyed<K> {
    fn key(&self) -> K;

    fn load(excel_bin_output_path: &str) -> HashMap<K, AvatarTalentExcelConfig>;
}

impl AvatarTalentExcelConfigKeyed<u32> for AvatarTalentExcelConfig {
    fn key(&self) -> u32 {
        self.talent_id
    }

    fn load(excel_bin_output_path: &str) -> HashMap<u32, AvatarTalentExcelConfig> {
        let json = std::fs::read(&format!(
            "{excel_bin_output_path}/AvatarTalentExcelConfigData.json"
        ))
        .unwrap();
        let list: Vec<AvatarTalentExcelConfig> = serde_json::from_slice(&*json).unwrap();
        let data = list.iter().map(|item| (item.key(), item.clone())).collect();
        data
    }
}
