use super::{dungeon_challenge_config_collection, ChallengeType};
use rand::{seq::SliceRandom, SeedableRng};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashSet},
    io,
    path::Path,
    sync::OnceLock,
};

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DailyTask {
    #[serde(rename = "ID")]
    pub id: u32,
    pub city_id: u32,
    #[serde(rename = "type")]
    pub kind: String,
    pub finish_type: String,
    pub finish_progress: u32,
    pub finish_param1: u32,
    pub task_reward_id: u32,
    pub new_group_vec: Vec<u32>,
    pub quest_id: u32,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyReward {
    pub drop_id: u32,
    pub preview_reward_id: u32,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyLevel {
    #[serde(rename = "ID")]
    pub id: u32,
    pub min_player_level: u32,
    pub max_player_level: u32,
    pub score_drop_id: u32,
    pub score_preview_reward_id: u32,
}

pub struct DailyTaskConfig {
    pub tasks: BTreeMap<u32, DailyTask>,
    pub levels: BTreeMap<u32, DailyLevel>,
    pub rewards: BTreeMap<u32, Vec<DailyReward>>,
    pub cities: BTreeMap<u32, Vec<u32>>,
    pub groups: HashSet<u32>,
}
static CONFIG: OnceLock<DailyTaskConfig> = OnceLock::new();

impl DailyTaskConfig {
    pub fn get() -> &'static Self {
        CONFIG.get().expect("daily task tables loaded")
    }
    pub fn level(&self, level: u32) -> Option<&DailyLevel> {
        self.levels
            .values()
            .find(|r| r.min_player_level <= level && level <= r.max_player_level)
    }
    pub fn reward(&self, task: &DailyTask, level: u32) -> Option<DailyReward> {
        self.rewards
            .get(&task.task_reward_id)?
            .get(self.level(level)?.id.checked_sub(1)? as usize)
            .copied()
    }
    pub fn draw(&self, preferred: u32, uid: u32, day: u64) -> Option<(u32, Vec<u32>)> {
        let mut rng = rand::rngs::SmallRng::seed_from_u64((u64::from(uid) << 32) ^ day);
        let cities: Vec<_> = self.cities.keys().copied().collect();
        let city = if preferred == 0 {
            *cities.choose(&mut rng)?
        } else if self.cities.contains_key(&preferred) {
            preferred
        } else {
            1
        };
        let mut pool = self.cities.get(&city)?.clone();
        pool.shuffle(&mut rng);
        let mut used = HashSet::new();
        let mut tasks = Vec::new();
        for id in pool {
            let task = &self.tasks[&id];
            if task.new_group_vec.iter().any(|g| used.contains(g)) {
                continue;
            }
            used.extend(task.new_group_vec.iter().copied());
            tasks.push(id);
            if tasks.len() == 4 {
                return Some((city, tasks));
            }
        }
        None
    }
    pub fn load(root: &str) -> io::Result<()> {
        fn read<T: serde::de::DeserializeOwned>(root: &str, name: &str) -> io::Result<Vec<T>> {
            serde_json::from_slice(&std::fs::read(
                Path::new(root).join(format!("{name}.json")),
            )?)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{name}: {e}")))
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Rewards {
            #[serde(rename = "ID")]
            id: u32,
            drop_vec: Vec<DailyReward>,
        }
        let tasks: BTreeMap<_, _> = read::<DailyTask>(root, "DailyTaskExcelConfigData")?
            .into_iter()
            .map(|r| (r.id, r))
            .collect();
        let scene = Path::new(root)
            .parent()
            .unwrap_or(Path::new("assets"))
            .join("lua/scene/3");
        let mut cities: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        let mut groups = HashSet::new();
        for task in tasks.values().filter(|t| t.kind == "DAILY_TASK_SCENE") {
            groups.extend(task.new_group_vec.iter().copied());
            let supported = match task.finish_type.as_str() {
                "DAILY_FINISH_MONSTER_NUM" | "DAILY_FINISH_GADGET_ID_NUM" => true,
                "DAILY_FINISH_CHALLENGE" => dungeon_challenge_config_collection::get()
                    .get(&task.finish_param1)
                    .is_some_and(|c| {
                        matches!(
                            c.challenge_type,
                            ChallengeType::ChallengeKillCount
                                | ChallengeType::ChallengeKillCountInTime
                                | ChallengeType::ChallengeKillCountFast
                                | ChallengeType::ChallengeSurvive
                                | ChallengeType::ChallengeSurviveInTime
                                | ChallengeType::ChallengeTimeFly
                                | ChallengeType::ChallengeTriggerInTime
                                | ChallengeType::ChallengeKillCountGuardHp
                        )
                    }),
                _ => false,
            };
            if supported
                && task.finish_progress > 0
                && !task.new_group_vec.is_empty()
                && task
                    .new_group_vec
                    .iter()
                    .all(|g| scene.join(format!("scene3_group{g}.lua")).is_file())
            {
                cities.entry(task.city_id).or_default().push(task.id);
            }
        }
        cities.retain(|_, tasks| tasks.len() >= 4);
        let _ = CONFIG.set(Self {
            tasks,
            cities,
            groups,
            levels: read::<DailyLevel>(root, "DailyTaskLevelExcelConfigData")?
                .into_iter()
                .map(|r| (r.id, r))
                .collect(),
            rewards: read::<Rewards>(root, "DailyTaskRewardExcelConfigData")?
                .into_iter()
                .map(|r| (r.id, r.drop_vec))
                .collect(),
        });
        Ok(())
    }
}
