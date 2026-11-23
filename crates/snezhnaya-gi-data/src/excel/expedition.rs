use serde::Deserialize;
use std::{collections::BTreeMap, io, path::Path, sync::OnceLock};

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct OpenCondition {
    #[serde(rename = "type")]
    pub kind: String,
    pub para: u32,
    pub para2: u32,
}

#[derive(Clone, Deserialize)]
pub struct TimeReward {
    pub htime: u32,
    pub reward_drop_id: u32,
    #[serde(rename = "rewardPreview")]
    reward_preview_id: u32,
    #[serde(skip)]
    preview_items: Vec<(u32, u32, u32)>,
}

impl TimeReward {
    pub fn roll_preview(&self) -> Vec<(u32, u32)> {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        self.preview_items
            .iter()
            .map(|&(id, min, max)| (id, rng.gen_range(min..=max)))
            .collect()
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Expedition {
    #[serde(rename = "ID")]
    pub id: u32,
    #[serde(rename = "city_id")]
    pub city_id: u32,
    pub open_condition_vec: Vec<OpenCondition>,
    pub time_reward_vec: Vec<TimeReward>,
}

#[derive(Clone, Copy, Default)]
pub struct ExpeditionBonus {
    pub shorten_ratio: f32,
    pub extra_ratio: f32,
    pub extra_probability: u32,
}

impl ExpeditionBonus {
    pub fn roll_extra(&self, items: &[(u32, u32)]) -> Vec<(u32, u32)> {
        use rand::Rng;
        if !self.extra_ratio.is_finite()
            || !(0.0..=1.0).contains(&self.extra_ratio)
            || rand::thread_rng().gen_range(0..10000) >= self.extra_probability.min(10000)
        {
            return vec![];
        }
        items
            .iter()
            .filter_map(|&(id, count)| {
                let extra = (f64::from(count) * f64::from(self.extra_ratio)).floor() as u32;
                (extra != 0).then_some((id, extra))
            })
            .collect()
    }
}

pub struct ExpeditionConfig {
    pub expeditions: BTreeMap<u32, Expedition>,
    pub limit_additions: BTreeMap<u32, u32>,
}

static CONFIG: OnceLock<ExpeditionConfig> = OnceLock::new();

impl ExpeditionConfig {
    pub fn get() -> &'static Self {
        CONFIG
            .get()
            .expect("expedition config must be loaded at startup")
    }

    pub fn bonuses(
        &self,
        depot_id: u32,
        promote: u32,
        expedition_id: u32,
        hours: u32,
    ) -> ExpeditionBonus {
        use super::{
            avatar_skill_depot_excel_config_collection as depots,
            proud_skill_excel_config_collection as skills,
        };
        let mut bonus = ExpeditionBonus::default();
        let Some(depot) = depots::get().get(&depot_id) else {
            return bonus;
        };
        let Some(expedition) = self.expeditions.get(&expedition_id) else {
            return bonus;
        };
        for open in &depot.inherent_proud_skill_opens {
            if open.need_avatar_promote_level > promote {
                continue;
            }
            let Some(skill) = open
                .proud_skill_group_id
                .checked_mul(100)
                .and_then(|n| n.checked_add(1))
                .and_then(|id| skills::get().get(&id))
            else {
                continue;
            };
            let param = |index: usize| skill.life_effect_params.get(index).map(|v| v.as_str());
            if param(0).and_then(|s| s.parse::<u32>().ok()) != Some(expedition.city_id) {
                continue;
            }
            match skill.life_effect_type.as_str() {
                "PROUD_EFFECT_EXPEDITION_SHORTEN_TIME" => {
                    if let Some(ratio) = param(1)
                        .and_then(|s| s.parse::<f32>().ok())
                        .filter(|r| r.is_finite() && (0.0..1.0).contains(r))
                    {
                        bonus.shorten_ratio = bonus.shorten_ratio.max(ratio);
                    }
                }
                "PROUD_EFFECT_EXPEDITION_EXTRA_OUTPUT" => {
                    if param(1).and_then(|s| s.parse::<u32>().ok()) != Some(hours) {
                        continue;
                    }
                    let probability = param(2)
                        .and_then(|s| s.parse::<u32>().ok())
                        .filter(|p| *p <= 10000);
                    let ratio = param(3)
                        .and_then(|s| s.parse::<f32>().ok())
                        .filter(|r| r.is_finite() && (0.0..=1.0).contains(r));
                    if let (Some(probability), Some(ratio)) = (probability, ratio) {
                        if ratio > bonus.extra_ratio {
                            bonus.extra_ratio = ratio;
                            bonus.extra_probability = probability;
                        }
                    }
                }
                _ => (),
            }
        }
        bonus
    }

    pub fn load(root: &str) -> io::Result<()> {
        fn rows<T: serde::de::DeserializeOwned>(root: &str, name: &str) -> io::Result<Vec<T>> {
            serde_json::from_slice(&std::fs::read(
                Path::new(root).join(format!("{name}.json")),
            )?)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{name}: {e}")))
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Level {
            level: u32,
            #[serde(default)]
            expedition_limit_add: u32,
        }
        #[derive(Deserialize)]
        #[serde(default, rename_all = "camelCase")]
        struct Preview {
            id: u32,
            preview_items: Vec<PreviewItem>,
        }
        impl Default for Preview {
            fn default() -> Self {
                Self {
                    id: 0,
                    preview_items: vec![],
                }
            }
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct PreviewItem {
            id: u32,
            count: String,
        }
        let previews: BTreeMap<_, _> = rows::<Preview>(root, "RewardPreviewExcelConfigData")?
            .into_iter()
            .map(|r| (r.id, r.preview_items))
            .collect();
        let mut expeditions = rows::<Expedition>(root, "ExpeditionDataExcelConfigData")?;
        for expedition in &mut expeditions {
            for reward in &mut expedition.time_reward_vec {
                let preview = previews.get(&reward.reward_preview_id).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "missing expedition reward preview",
                    )
                })?;
                for item in preview.iter().filter(|i| i.id != 0) {
                    let (min, max) = item
                        .count
                        .split_once(';')
                        .unwrap_or((&item.count, &item.count));
                    let invalid = || {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("invalid expedition reward range: {}", item.count),
                        )
                    };
                    let min: u32 = min.parse().map_err(|_| invalid())?;
                    let max: u32 = max.parse().map_err(|_| invalid())?;
                    if min == 0 || min > max {
                        return Err(invalid());
                    }
                    reward.preview_items.push((item.id, min, max));
                }
            }
        }
        let config = Self {
            expeditions: expeditions.into_iter().map(|r| (r.id, r)).collect(),
            limit_additions: rows::<Level>(root, "PlayerLevelExcelConfigData")?
                .into_iter()
                .filter(|r| r.expedition_limit_add != 0)
                .map(|r| (r.level, r.expedition_limit_add))
                .collect(),
        };
        let _ = CONFIG.set(config);
        Ok(())
    }
}
