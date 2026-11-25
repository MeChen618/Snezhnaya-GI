use super::monster_excel_config_collection;
use serde::Deserialize;
use std::{collections::HashMap, io, sync::OnceLock};

pub struct CodexConfig {
    pub weapons: HashMap<u32, u32>,
    pub materials: HashMap<u32, u32>,
    pub books: HashMap<u32, u32>,
    pub quests: HashMap<u32, u32>,
    viewpoints: HashMap<(u32, u32, u32, u32), u32>,
    pub monster_codex_ids: HashMap<u32, u32>,
    pub monster_variants: HashMap<u32, Vec<u32>>,
    pub reliquary_parts: HashMap<u32, Vec<u32>>,
    pub reliquary_sets_by_part: HashMap<u32, Vec<u32>>,
}

static CONFIG: OnceLock<CodexConfig> = OnceLock::new();

pub fn reliquary_part(item_id: u32) -> u32 {
    item_id / 10 * 10
}

impl CodexConfig {
    pub fn get() -> &'static Self {
        CONFIG.get().expect("codex tables loaded")
    }

    pub fn viewpoint(&self, scene: u32, group: u32, config: u32, gadget: u32) -> Option<u32> {
        self.viewpoints
            .get(&(scene, group, config, gadget))
            .copied()
    }

    pub(super) fn load(root: &str) -> io::Result<()> {
        fn rows<T: serde::de::DeserializeOwned>(root: &str, name: &str) -> io::Result<Vec<T>> {
            serde_json::from_slice(&std::fs::read(format!(
                "{root}/{name}CodexExcelConfigData.json"
            ))?)
            .map_err(|error| {
                io::Error::new(io::ErrorKind::InvalidData, format!("{name}Codex: {error}"))
            })
        }

        #[derive(Default, Deserialize)]
        #[serde(default, rename_all = "camelCase")]
        struct Entry {
            #[serde(rename = "Id")]
            id: u32,
            weapon_id: u32,
            material_id: u32,
            parent_quest_id: u32,
            is_disuse: bool,
        }
        let entries = |name, source_id: fn(Entry) -> u32| -> io::Result<HashMap<u32, u32>> {
            Ok(rows::<Entry>(root, name)?
                .into_iter()
                .filter(|row| !row.is_disuse && row.id != 0)
                .filter_map(|row| {
                    let id = row.id;
                    let source = source_id(row);
                    (source != 0).then_some((source, id))
                })
                .collect())
        };

        #[derive(Default, Deserialize)]
        #[serde(default, rename_all = "camelCase")]
        struct Animal {
            #[serde(rename = "Id")]
            id: u32,
            describe_id: u32,
            count_type: String,
        }
        let animals: HashMap<_, _> = rows::<Animal>(root, "Animal")?
            .into_iter()
            .filter(|row| row.count_type == "CODEX_COUNT_TYPE_KILL")
            .map(|row| (row.describe_id, row.id))
            .collect();
        let mut monster_codex_ids = HashMap::new();
        let mut monster_variants: HashMap<u32, Vec<u32>> = HashMap::new();
        for (&id, monster) in monster_excel_config_collection::get().iter() {
            if let Some(&codex_id) = animals.get(&monster.describe_id) {
                monster_codex_ids.insert(id, codex_id);
                monster_variants.entry(codex_id).or_default().push(id);
            }
        }

        #[derive(Default, Deserialize)]
        #[serde(default, rename_all = "camelCase")]
        struct Reliquary {
            #[serde(rename = "Id")]
            id: u32,
            cap_id: u32,
            cup_id: u32,
            flower_id: u32,
            leather_id: u32,
            sand_id: u32,
        }
        let mut reliquary_parts = HashMap::new();
        let mut reliquary_sets_by_part: HashMap<u32, Vec<u32>> = HashMap::new();
        for row in rows::<Reliquary>(root, "Reliquary")? {
            let mut parts: Vec<_> = [
                row.cap_id,
                row.cup_id,
                row.flower_id,
                row.leather_id,
                row.sand_id,
            ]
            .into_iter()
            .filter(|&id| id != 0)
            .map(reliquary_part)
            .collect();
            parts.sort_unstable();
            parts.dedup();
            if row.id != 0 && !parts.is_empty() {
                for &part in &parts {
                    reliquary_sets_by_part.entry(part).or_default().push(row.id);
                }
                reliquary_parts.insert(row.id, parts);
            }
        }

        #[derive(Default, Deserialize)]
        #[serde(default, rename_all = "camelCase")]
        struct Viewpoint {
            #[serde(rename = "Id")]
            id: u32,
            scene_id: u32,
            group_id: u32,
            config_id: u32,
            gadget_id: u32,
            is_disuse: bool,
        }
        let viewpoints = rows::<Viewpoint>(root, "View")?
            .into_iter()
            .filter(|row| !row.is_disuse && row.id != 0)
            .map(|row| {
                (
                    (row.scene_id, row.group_id, row.config_id, row.gadget_id),
                    row.id,
                )
            })
            .collect();
        let _ = CONFIG.set(Self {
            weapons: entries("Weapon", |row| row.weapon_id)?,
            materials: entries("Material", |row| row.material_id)?,
            books: entries("Books", |row| row.material_id)?,
            quests: entries("Quest", |row| row.parent_quest_id)?,
            viewpoints,
            monster_codex_ids,
            monster_variants,
            reliquary_parts,
            reliquary_sets_by_part,
        });
        Ok(())
    }
}
