use super::progression::ProgressionConfig;
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path, sync::OnceLock};

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct Position {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Position {
    pub fn valid(&self) -> bool {
        [self.x, self.y, self.z].into_iter().all(f32::is_finite)
    }
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summon {
    pub monster_id: u32,
    pub affix_ids: Vec<u32>,
    pub hp_multiplier: f32,
    pub sgv_hash: u32,
    pub local_ids: BTreeMap<u32, u32>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Monster {
    pub level_config_id: u32,
    pub monster_id: u32,
    pub pose_id: u32,
    pub ai_config_id: u32,
    pub affix_ids: Vec<u32>,
    pub hp_multiplier: f32,
    pub attack_multiplier: f32,
    pub pos: Position,
    pub rot: Position,
    pub verified_difficulty: u32,
    #[serde(default)]
    pub summon: Option<Summon>,
}
#[derive(Clone)]
pub struct Arena {
    pub dungeon: u32,
    pub scene: u32,
    pub group: u32,
    pub prepare_gallery: u32,
    pub battle_gallery: u32,
}
#[derive(Default)]
pub struct LeyLineConfig {
    pub monsters: BTreeMap<u32, Monster>,
    pub arenas: BTreeMap<u32, Arena>,
    pub required_difficulty: BTreeMap<u32, u32>,
}
static DATA: OnceLock<LeyLineConfig> = OnceLock::new();
impl LeyLineConfig {
    pub fn get() -> &'static Self {
        DATA.get().expect("LeyLine tables loaded")
    }
    pub fn load(root: &str) -> std::io::Result<()> {
        fn rows<T: serde::de::DeserializeOwned>(root: &str, name: &str) -> std::io::Result<Vec<T>> {
            serde_json::from_slice(&std::fs::read(
                Path::new(root).join(format!("{name}.json")),
            )?)
            .map_err(std::io::Error::other)
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Dungeon {
            id: u32,
            dungeon_id: u32,
            #[serde(rename = "HLKOKPADLEH")]
            prepare: u32,
            #[serde(rename = "LGGKCOFNJAK")]
            battle: u32,
        }
        #[derive(Deserialize, Default)]
        #[serde(default, rename_all = "camelCase")]
        struct Gallery {
            id: u32,
            scene_id: u32,
            control_group_id: u32,
        }
        #[derive(Deserialize, Default)]
        #[serde(default, rename_all = "camelCase")]
        struct Difficulty {
            id: u32,
            required_difficulty_completion_to_unlock: u32,
        }
        let mut result = Self::default();
        let galleries: BTreeMap<_, _> = rows::<Gallery>(root, "GalleryExcelConfigData")?
            .into_iter()
            .map(|r| (r.id, r))
            .collect();
        for r in rows::<Dungeon>(root, "LeyLineDungeonExcelConfigData")? {
            if let (Some(prepare), Some(battle)) =
                (galleries.get(&r.prepare), galleries.get(&r.battle))
            {
                if prepare.scene_id == battle.scene_id
                    && prepare.control_group_id == battle.control_group_id
                    && super::dungeon_excel_config_collection::get()
                        .get(&r.dungeon_id)
                        .is_some_and(|d| d.scene_id == prepare.scene_id)
                {
                    result.arenas.insert(
                        r.id,
                        Arena {
                            dungeon: r.dungeon_id,
                            scene: prepare.scene_id,
                            group: prepare.control_group_id,
                            prepare_gallery: r.prepare,
                            battle_gallery: r.battle,
                        },
                    );
                }
            }
        }
        result.required_difficulty = rows::<Difficulty>(root, "LeyLineDifficultyExcelConfigData")?
            .into_iter()
            .map(|r| (r.id, r.required_difficulty_completion_to_unlock))
            .collect();
        let path = Path::new(root)
            .parent()
            .unwrap_or(Path::new("assets"))
            .join("Server/LeyLineChallenge.json");
        if path.is_file() {
            let monsters: Vec<Monster> =
                serde_json::from_slice(&std::fs::read(path)?).map_err(std::io::Error::other)?;
            for m in monsters {
                if m.level_config_id == 0
                    || !m.pos.valid()
                    || !m.rot.valid()
                    || !m.hp_multiplier.is_finite()
                    || m.hp_multiplier <= 0.
                    || !m.attack_multiplier.is_finite()
                    || m.attack_multiplier <= 0.
                    || !(1..=6).contains(&m.verified_difficulty)
                    || !super::monster_excel_config_collection::get().contains_key(&m.monster_id)
                    || result.monsters.contains_key(&m.level_config_id)
                {
                    return Err(std::io::Error::other("invalid LeyLine monster binding"));
                }
                if let Some(s) = &m.summon {
                    if !super::monster_excel_config_collection::get().contains_key(&s.monster_id)
                        || !s.hp_multiplier.is_finite()
                        || s.hp_multiplier <= 0.
                        || s.local_ids.is_empty()
                        || s.local_ids.values().any(|v| !(1..=3).contains(v))
                    {
                        return Err(std::io::Error::other("invalid LeyLine summon binding"));
                    }
                }
                result.monsters.insert(m.level_config_id, m);
            }
        }
        for season in ProgressionConfig::get().challenge_seasons.values() {
            if season.level_id_list.len() != 6 {
                return Err(std::io::Error::other(
                    "LeyLine season must contain six difficulties",
                ));
            }
        }
        let _ = DATA.set(result);
        Ok(())
    }
}
