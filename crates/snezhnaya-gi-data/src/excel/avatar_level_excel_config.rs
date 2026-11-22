use std::collections::HashMap;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AvatarLevelExcelConfig {
    pub level: u32,
    pub exp: u32,
}

pub trait AvatarLevelExcelConfigKeyed<K> {
    fn load(excel_bin_output_path: &str) -> HashMap<K, AvatarLevelExcelConfig>;
}

impl AvatarLevelExcelConfigKeyed<u32> for AvatarLevelExcelConfig {
    fn load(excel_bin_output_path: &str) -> HashMap<u32, AvatarLevelExcelConfig> {
        let path = format!("{excel_bin_output_path}/AvatarLevelExcelConfigData.json");
        let rows: Vec<Self> = serde_json::from_slice(&std::fs::read(&path).expect(&path))
            .expect("invalid avatar level configuration");
        rows.into_iter().map(|row| (row.level, row)).collect()
    }
}
