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

const TASK_COUNT: usize = 4;
const DEFAULT_CITY: u32 = 1;

fn select_tasks(tasks: &BTreeMap<u32, DailyTask>, pool: &[u32]) -> Option<Vec<u32>> {
    fn fill(tasks: &BTreeMap<u32, DailyTask>, pool: &[u32], selected: &mut Vec<u32>) -> bool {
        let needed = TASK_COUNT - selected.len();
        if needed == 0 {
            return true;
        }
        if pool.len() < needed {
            return false;
        }
        for (index, &id) in pool.iter().take(pool.len() - needed + 1).enumerate() {
            let groups = &tasks[&id].new_group_vec;
            if selected.iter().any(|other| {
                tasks[other]
                    .new_group_vec
                    .iter()
                    .any(|group| groups.contains(group))
            }) {
                continue;
            }
            selected.push(id);
            if fill(tasks, &pool[index + 1..], selected) {
                return true;
            }
            selected.pop();
        }
        false
    }

    let mut selected = Vec::with_capacity(TASK_COUNT);
    fill(tasks, pool, &mut selected).then_some(selected)
}

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
        let city = if preferred == 0 {
            let cities: Vec<_> = self.cities.keys().copied().collect();
            *cities.choose(&mut rng)?
        } else if self.cities.contains_key(&preferred) {
            preferred
        } else if self.cities.contains_key(&DEFAULT_CITY) {
            DEFAULT_CITY
        } else {
            *self.cities.keys().next()?
        };
        let mut pool = self.cities.get(&city)?.clone();
        pool.shuffle(&mut rng);
        Some((city, select_tasks(&self.tasks, &pool)?))
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
        cities.retain(|_, pool| select_tasks(&tasks, pool).is_some());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_falls_back_and_finds_four_compatible_tasks() {
        let tasks = [vec![1, 2, 3, 4], vec![1], vec![2], vec![3], vec![4]]
            .into_iter()
            .enumerate()
            .map(|(id, new_group_vec)| {
                (
                    id as u32,
                    DailyTask {
                        new_group_vec,
                        ..Default::default()
                    },
                )
            })
            .collect();
        assert!(select_tasks(&tasks, &[0, 1, 2, 3]).is_none());
        let config = DailyTaskConfig {
            tasks,
            cities: BTreeMap::from([(2, vec![0, 1, 2, 3, 4])]),
            levels: BTreeMap::new(),
            rewards: BTreeMap::new(),
            groups: HashSet::new(),
        };
        for day in 1..=16 {
            let (city, mut tasks) = config.draw(DEFAULT_CITY, 42, day).unwrap();
            tasks.sort_unstable();
            assert_eq!((city, tasks), (2, vec![1, 2, 3, 4]));
        }
    }
}
