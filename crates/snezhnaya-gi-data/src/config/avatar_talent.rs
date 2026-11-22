pub use super::talent_types::TalentConfig;
use crate::config::TalentAction;
use crate::excel::{AvatarTalentExcelConfig, ProudSkillExcelConfig};
use common::string_util::InternString;
use std::collections::hash_map::Iter;
use std::{
    collections::HashMap,
    fs::{self, ReadDir},
    sync::OnceLock,
};

static AVATAR_TALENT_CONFIG_MAP: OnceLock<HashMap<InternString, Vec<TalentAction>>> =
    OnceLock::new();

fn load_avatar_talent_configs(talent_config_dir: ReadDir) -> std::io::Result<()> {
    let mut map = HashMap::new();
    for entry in talent_config_dir {
        let entry = entry?;
        let json = std::fs::read(entry.path())?;
        let config: TalentConfig = serde_json::from_slice(&*json)?;
        map.extend(config.talents);
    }

    let _ = AVATAR_TALENT_CONFIG_MAP.set(map);
    Ok(())
}

pub fn load_avatar_talent_configs_from_bin(bin_output_path: &str) -> std::io::Result<()> {
    load_avatar_talent_configs(fs::read_dir(format!(
        "{bin_output_path}/Talent/AvatarTalents/"
    ))?)?;

    Ok(())
}

pub fn get_avatar_talent_config(name: &InternString) -> Option<&Vec<TalentAction>> {
    AVATAR_TALENT_CONFIG_MAP.get().unwrap().get(name)
}

pub fn iter_avatar_talent_config_map() -> Iter<'static, InternString, Vec<TalentAction>> {
    AVATAR_TALENT_CONFIG_MAP.get().unwrap().iter()
}
pub fn process_talent_ids(
    talent_id_list: &[u32],
    avatar_talent_collection: &std::sync::Arc<HashMap<u32, AvatarTalentExcelConfig>>,
) -> Vec<InternString> {
    let mut open_configs = Vec::new();
    for talent_id in talent_id_list {
        if let Some(talent_config) = avatar_talent_collection.get(talent_id) {
            open_configs.push(talent_config.open_config);
        }
    }
    open_configs
}

pub fn process_inherent_proud_skills(
    inherent_proud_skill_list: &[u32],
    proud_skill_collection: &std::sync::Arc<HashMap<u32, ProudSkillExcelConfig>>,
) -> Vec<InternString> {
    let mut open_configs = Vec::new();
    for proud_skill_id in inherent_proud_skill_list {
        if let Some(proud_skill_config) = proud_skill_collection.get(proud_skill_id) {
            open_configs.push(proud_skill_config.open_config);
        }
    }
    open_configs
}

pub fn proud_skill_extra_levels(skill_depot_id: u32, talent_ids: &[u32]) -> HashMap<u32, u32> {
    use crate::excel::{
        avatar_skill_depot_excel_config_collection as depots,
        avatar_skill_excel_config_collection as skills,
        avatar_talent_excel_config_collection as talents,
    };
    let mut levels = HashMap::<u32, u32>::new();
    let Some(depot) = depots::get().get(&skill_depot_id) else {
        return levels;
    };
    for id in talent_ids {
        let Some(actions) = talents::get()
            .get(id)
            .and_then(|talent| get_avatar_talent_config(&talent.open_config))
        else {
            continue;
        };
        for action in actions {
            let TalentAction::AddTalentExtraLevel {
                extra_level,
                talent_type,
                talent_index,
            } = action
            else {
                continue;
            };
            if talent_type.as_str() != "AvatarSkill" {
                continue;
            }
            let skill_id = match talent_index {
                1 | 2 => depot.skills.get((*talent_index - 1) as usize).copied(),
                9 => Some(depot.energy_skill),
                _ => None,
            };
            let Some(skill) = skill_id
                .and_then(|id| skills::get().get(&id))
                .filter(|skill| skill.proud_skill_group_id != 0)
            else {
                continue;
            };
            let level = levels.entry(skill.proud_skill_group_id).or_default();
            *level = level.saturating_add(*extra_level);
        }
    }
    levels
}
