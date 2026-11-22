use serde::Deserialize;
use std::{collections::HashMap, io, path::Path, sync::OnceLock};

#[derive(Clone, Copy, Default, Debug, Deserialize)]
#[serde(default)]
pub struct ItemAmount {
    pub id: u32,
    pub count: u32,
}

#[derive(Clone, Default, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CombineRecipe {
    pub combine_id: u32,
    pub player_level: u32,
    pub scoin_cost: u32,
    pub material_items: Vec<ItemAmount>,
    pub result_item_id: u32,
    pub result_item_count: u32,
}

#[derive(Clone, Default, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ForgeRecipe {
    pub id: u32,
    pub player_level: u32,
    pub scoin_cost: u32,
    pub material_items: Vec<ItemAmount>,
    pub result_item_id: u32,
    pub result_item_count: u32,
    pub forge_time: u32,
    pub forge_point: u32,
    pub queue_num: u32,
}

#[derive(Clone, Default, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CookRecipe {
    pub id: u32,
    pub max_proficiency: u32,
    pub is_default_unlocked: bool,
    pub input_vec: Vec<ItemAmount>,
    pub quality_output_vec: Vec<ItemAmount>,
}

#[derive(Clone, Default, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompoundRecipe {
    pub id: u32,
    #[serde(rename = "groupID")]
    pub group_id: u32,
    pub queue_size: u32,
    pub cost_time: u32,
    pub input_vec: Vec<ItemAmount>,
    pub output_vec: Vec<ItemAmount>,
}

#[derive(Clone, Default, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReliquaryDecomposeRecipe {
    pub id: u32,
    pub reward_preview_id: u32,
    pub need_reliquary_count: u32,
    pub need_reliquary_rank_level: u32,
    pub max_reliquary_num: u32,
}

#[derive(Clone, Default, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReliquaryPurchaseRecipe {
    pub id: u32,
    pub offer_set_id: u32,
    pub flower_ids: Vec<u32>,
    pub feather_ids: Vec<u32>,
    pub timepiece_ids: Vec<u32>,
    pub goblet_ids: Vec<u32>,
    pub circlet_ids: Vec<u32>,
    pub weights: Vec<u32>,
}

impl ReliquaryPurchaseRecipe {
    pub fn items(&self, slot: u32) -> &[u32] {
        match slot {
            1 => &self.flower_ids,
            2 => &self.feather_ids,
            3 => &self.timepiece_ids,
            4 => &self.goblet_ids,
            5 => &self.circlet_ids,
            _ => &[],
        }
    }
}

#[derive(Default)]
pub struct ProductionConfig {
    pub combine: HashMap<u32, CombineRecipe>,
    pub forge: HashMap<u32, ForgeRecipe>,
    pub cook: HashMap<u32, CookRecipe>,
    pub compound: HashMap<u32, CompoundRecipe>,
    pub decompose: HashMap<u32, ReliquaryDecomposeRecipe>,
    pub reward_preview_items: HashMap<u32, Vec<u32>>,
    pub display_sets: HashMap<u32, u32>,
    pub transfer_materials: Vec<Vec<ItemAmount>>,
    pub purchase: HashMap<u32, ReliquaryPurchaseRecipe>,
    pub constants: HashMap<String, Vec<String>>,
}

static CONFIG: OnceLock<ProductionConfig> = OnceLock::new();

impl ProductionConfig {
    pub fn get() -> &'static Self {
        CONFIG
            .get()
            .expect("production config must be loaded at startup")
    }

    pub fn load(path: &str) -> io::Result<()> {
        fn read<T: serde::de::DeserializeOwned>(path: &str, name: &str) -> io::Result<Vec<T>> {
            serde_json::from_slice(&std::fs::read(
                Path::new(path).join(format!("{name}.json")),
            )?)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{name}: {error}")))
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Preview {
            #[serde(default)]
            id: u32,
            preview_items: Vec<PreviewItem>,
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct PreviewItem {
            id: u32,
        }
        #[derive(Deserialize)]
        #[serde(default)]
        struct Display {
            id: u32,
            param: u32,
        }
        impl Default for Display {
            fn default() -> Self {
                Self { id: 0, param: 0 }
            }
        }
        #[derive(Deserialize)]
        struct Constant {
            #[serde(default)]
            name: String,
            #[serde(default)]
            value: Vec<String>,
        }
        let constants: HashMap<_, _> = read::<Constant>(path, "ConstValueExcelConfigData")?
            .into_iter()
            .map(|r| (r.name, r.value))
            .collect();
        let transfer_materials = constants
            .get("CONST_VALUE_RELIQUARY_TRANSFER_MATERIALS_V2")
            .or_else(|| constants.get("CONST_VALUE_RELIQUARY_TRANSFER_MATERIALS"))
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.split(',')
                    .map(|part| {
                        let (id, count) = part.split_once(':').unwrap_or((part, "1"));
                        Ok(ItemAmount {
                            id: id.parse().map_err(io::Error::other)?,
                            count: count.parse().map_err(io::Error::other)?,
                        })
                    })
                    .collect::<io::Result<Vec<_>>>()
            })
            .collect::<io::Result<Vec<_>>>()?;
        let config = Self {
            combine: read::<CombineRecipe>(path, "CombineExcelConfigData")?
                .into_iter()
                .map(|r| (r.combine_id, r))
                .collect(),
            forge: read::<ForgeRecipe>(path, "ForgeExcelConfigData")?
                .into_iter()
                .map(|r| (r.id, r))
                .collect(),
            cook: read::<CookRecipe>(path, "CookRecipeExcelConfigData")?
                .into_iter()
                .map(|r| (r.id, r))
                .collect(),
            compound: read::<CompoundRecipe>(path, "CompoundExcelConfigData")?
                .into_iter()
                .map(|r| (r.id, r))
                .collect(),
            decompose: read::<ReliquaryDecomposeRecipe>(path, "ReliquaryDecomposeExcelConfigData")?
                .into_iter()
                .map(|r| (r.id, r))
                .collect(),
            reward_preview_items: read::<Preview>(path, "RewardPreviewExcelConfigData")?
                .into_iter()
                .map(|r| {
                    (
                        r.id,
                        r.preview_items
                            .into_iter()
                            .map(|i| i.id)
                            .filter(|id| *id != 0)
                            .collect(),
                    )
                })
                .collect(),
            display_sets: read::<Display>(path, "DisplayItemExcelConfigData")?
                .into_iter()
                .map(|r| (r.id, r.param))
                .collect(),
            transfer_materials,
            purchase: read::<ReliquaryPurchaseRecipe>(
                path,
                "ReliquaryOfferPurchaseExcelConfigData",
            )?
            .into_iter()
            .map(|r| (r.id, r))
            .collect(),
            constants,
        };
        let _ = CONFIG.set(config);
        Ok(())
    }
}
