use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Trigger {
    pub trigger_type: String,
    pub param_list: Vec<String>,
}

impl Trigger {
    pub fn param(&self, index: usize) -> u32 {
        self.param_list
            .get(index)
            .and_then(|p| p.parse().ok())
            .unwrap_or(0)
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Mission {
    pub id: u32,
    pub schedule_id: u32,
    pub add_point: u32,
    pub progress: u32,
    pub refresh_type: String,
    pub trigger_config: Trigger,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PassSchedule {
    pub id: u32,
    pub level_reward_index_id: u32,
    pub extra_paid_reward_id: u32,
    pub buy_level_cost_coin_num: u32,
    pub cycle_point_upper_limit: u32,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PassReward {
    pub index_id: u32,
    pub level: u32,
    pub free_reward_id_list: Vec<u32>,
    pub paid_reward_id_list: Vec<u32>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RewardItem {
    pub item_id: u32,
    pub item_count: u32,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Reward {
    pub reward_id: u32,
    pub reward_item_list: Vec<RewardItem>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Investigation {
    pub id: u32,
    pub reward_id: u32,
    pub unlock_level: u32,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Target {
    pub id: u32,
    pub quest_id: u32,
    pub investigation_id: u32,
    pub reward_id: u32,
    pub progress: u32,
    pub trigger_config: Trigger,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ActivityWatcher {
    pub id: u32,
    #[serde(rename = "rewardID")]
    pub reward_id: u32,
    pub progress: u32,
    pub trigger_config: Trigger,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Activity {
    pub activity_id: u32,
    pub watcher_id: Vec<u32>,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChallengeSeason {
    pub schedule_id: u32,
    pub schedule_start_time: String,
    pub schedule_end_time: String,
    pub level_id_list: Vec<u32>,
    pub reward_watcher_id: u32,
    #[serde(rename = "PABCIOMCPMD")]
    pub burst_days: u32,
}
impl ChallengeSeason {
    pub fn begin(&self) -> Option<u32> {
        cn_timestamp(&self.schedule_start_time)
    }
    pub fn first_day(&self) -> Option<u32> {
        let begin = self.begin()?;
        Some((begin + 8 * 3600) / 86400 * 86400 - 4 * 3600)
    }
    pub fn end(&self, duration: u32) -> Option<u32> {
        Some(
            (cn_timestamp(&self.schedule_end_time)?.checked_add(1)?).min(
                self.first_day()?
                    .checked_add(duration.checked_mul(86400)?)?,
            ),
        )
    }
}
#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChallengeLevel {
    pub revise_level: u32,
    #[serde(rename = "PFCPNCCOAPB")]
    pub elite_times: Vec<u32>,
    pub id: u32,
    pub watcher_list: Vec<u32>,
    pub level_config_list: Vec<u32>,
    #[serde(rename = "MMKEJFCFMBN")]
    pub difficulty: u32,
}
// 资源日期使用 UTC+8，边界不依赖宿主机时区。
pub fn cn_timestamp(text: &str) -> Option<u32> {
    let v: Vec<u32> = text
        .split(['-', ' ', ':'])
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    if v.len() != 6 {
        return None;
    }
    let (y, m, d, h, min, sec) = (v[0], v[1], v[2], v[3], v[4], v[5]);
    if !(1970..=2106).contains(&y) || !(1..=12).contains(&m) || h > 23 || min > 59 || sec > 59 {
        return None;
    }
    let leap = |y: u32| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = [
        31,
        28 + u32::from(leap(y)),
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if d == 0 || d > days[m as usize - 1] {
        return None;
    }
    let before: u64 = (1970..y)
        .map(|y| if leap(y) { 366u64 } else { 365 })
        .sum::<u64>()
        + days[..m as usize - 1]
            .iter()
            .map(|d| u64::from(*d))
            .sum::<u64>()
        + u64::from(d - 1);
    u32::try_from((before * 86400 + u64::from(h * 3600 + min * 60 + sec)).checked_sub(8 * 3600)?)
        .ok()
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HandbookDungeon {
    pub id: u32,
    pub scene_id: u32,
    pub dungeon_entry_id: u32,
    pub is_show_in_adv_handbook: bool,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HandbookMonster {
    pub id: u32,
    pub city_id: u32,
    pub group_id_list: Vec<u32>,
    pub monster_id_list: Vec<u32>,
    pub monster_category: String,
}
pub struct ProgressionConfig {
    pub handbook_dungeons: BTreeMap<u32, HandbookDungeon>,
    pub handbook_monsters: BTreeMap<u32, HandbookMonster>,
    pub pass_product_ids: BTreeMap<u32, String>,
    pub pass_product_tiers: BTreeMap<u32, String>,
    pub activities: BTreeMap<u32, Activity>,
    pub challenge_seasons: BTreeMap<u32, ChallengeSeason>,
    pub challenge_levels: BTreeMap<u32, ChallengeLevel>,
    pub challenge_reliquaries: Vec<u32>,
    pub challenge_duration: u32,
    pub pass_schedules: BTreeMap<u32, PassSchedule>,
    pub pass_missions: BTreeMap<u32, Mission>,
    pub pass_rewards: BTreeMap<(u32, u32), PassReward>,
    pub pass_level_points: BTreeMap<u32, u32>,
    pub rewards: BTreeMap<u32, Reward>,
    pub investigations: BTreeMap<u32, Investigation>,
    pub targets: BTreeMap<u32, Target>,
    pub activity_watchers: BTreeMap<u32, ActivityWatcher>,
    pub player_level_exp: BTreeMap<u32, u32>,
    pub chapter_end_quests: BTreeMap<u32, u32>,
}

static CONFIG: OnceLock<ProgressionConfig> = OnceLock::new();

impl ProgressionConfig {
    pub fn get() -> &'static Self {
        CONFIG.get().expect("progression tables loaded")
    }

    pub fn load(root: &str) -> std::io::Result<()> {
        fn rows<T: DeserializeOwned>(root: &str, name: &str) -> std::io::Result<Vec<T>> {
            let data = std::fs::read(format!("{root}/{name}.json"))?;
            serde_json::from_slice(&data).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{name}: {e}"))
            })
        }
        #[derive(Deserialize, Default)]
        #[serde(default, rename_all = "camelCase")]
        struct Level {
            level: u32,
            level_up_need_point: u32,
        }
        #[derive(Deserialize, Default)]
        #[serde(default, rename_all = "camelCase")]
        struct PlayerLevel {
            level: u32,
            exp: u32,
        }
        #[derive(Deserialize, Default)]
        #[serde(default, rename_all = "camelCase")]
        struct Chapter {
            id: u32,
            end_quest_id: u32,
        }
        #[derive(Deserialize)]
        struct Entry {
            id: u32,
            #[serde(default)]
            duration: u32,
        }
        #[derive(Deserialize)]
        struct Reliquary {
            id: u32,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Product {
            config_id: u32,
            product_id: String,
            #[serde(default)]
            is_internal: bool,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct ProductPlay {
            config_id: u32,
            price_tier: String,
        }
        let data = Self {
            pass_product_tiers: rows::<ProductPlay>(root, "ProductPlayDetailConfigData")?
                .into_iter()
                .map(|r| (r.config_id, r.price_tier))
                .collect(),
            handbook_dungeons: rows::<HandbookDungeon>(root, "DungeonEntryExcelConfigData")?
                .into_iter()
                .map(|r| (r.id, r))
                .collect(),
            handbook_monsters: rows::<HandbookMonster>(root, "InvestigationMonsterConfigData")?
                .into_iter()
                .map(|r| (r.id, r))
                .collect(),
            pass_product_ids: rows::<Product>(root, "ProductIdConfigData")?
                .into_iter()
                .filter(|r| {
                    r.is_internal
                        && (r.product_id.starts_with("ys_chn_")
                            || r.product_id.starts_with("yschnbp"))
                })
                .map(|r| (r.config_id, r.product_id))
                .collect(),
            activities: rows::<Activity>(root, "NewActivityExcelConfigData")?
                .into_iter()
                .map(|r| (r.activity_id, r))
                .collect(),
            challenge_seasons: rows::<ChallengeSeason>(root, "LeyLineChallengeExcelConfigData")?
                .into_iter()
                .map(|r| (r.schedule_id, r))
                .collect(),
            challenge_levels: rows::<ChallengeLevel>(root, "LeyLineLevelExcelConfigData")?
                .into_iter()
                .map(|r| (r.id, r))
                .collect(),
            challenge_reliquaries: rows::<Reliquary>(root, "LeyLineReliquaryExcelConfigData")?
                .into_iter()
                .map(|r| r.id)
                .collect(),
            challenge_duration: rows::<Entry>(root, "NewActivityEntryConfigData")?
                .into_iter()
                .find(|r| r.id == 5269)
                .map_or(0, |r| r.duration),
            pass_schedules: rows::<PassSchedule>(root, "BattlePassScheduleExcelConfigData")?
                .into_iter()
                .map(|r| (r.id, r))
                .collect(),
            pass_missions: rows::<Mission>(root, "BattlePassMissionExcelConfigData")?
                .into_iter()
                .map(|r| (r.id, r))
                .collect(),
            pass_rewards: rows::<PassReward>(root, "BattlePassRewardExcelConfigData")?
                .into_iter()
                .map(|r| ((r.index_id, r.level), r))
                .collect(),
            pass_level_points: rows::<Level>(root, "BattlePassLevelExcelConfigData")?
                .into_iter()
                .map(|r| (r.level, r.level_up_need_point))
                .collect(),
            rewards: rows::<Reward>(root, "RewardExcelConfigData")?
                .into_iter()
                .map(|r| (r.reward_id, r))
                .collect(),
            investigations: rows::<Investigation>(root, "InvestigationConfigData")?
                .into_iter()
                .map(|r| (r.id, r))
                .collect(),
            targets: rows::<Target>(root, "InvestigationTargetConfigData")?
                .into_iter()
                .map(|r| (r.id, r))
                .collect(),
            activity_watchers: rows::<ActivityWatcher>(root, "NewActivityWatcherConfigData")?
                .into_iter()
                .map(|r| (r.id, r))
                .collect(),
            player_level_exp: rows::<PlayerLevel>(root, "PlayerLevelExcelConfigData")?
                .into_iter()
                .map(|r| (r.level, r.exp))
                .collect(),
            chapter_end_quests: rows::<Chapter>(root, "ChapterExcelConfigData")?
                .into_iter()
                .map(|r| (r.id, r.end_quest_id))
                .collect(),
        };
        let _ = CONFIG.set(data);
        Ok(())
    }

    pub fn current_pass(&self) -> Option<&PassSchedule> {
        self.pass_schedules
            .values()
            .rev()
            .find(|row| row.id < 9000 && row.level_reward_index_id != 0)
    }

    pub fn pass_missions(&self, schedule: u32) -> impl Iterator<Item = &Mission> {
        self.pass_missions.values().filter(move |row| {
            row.progress > 0
                && (row.schedule_id == 0
                    || row.schedule_id == schedule
                    || row.refresh_type == "BATTLE_PASS_MISSION_REFRESH_CYCLE_CROSS_SCHEDULE")
        })
    }
}
