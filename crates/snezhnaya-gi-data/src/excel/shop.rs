use super::{production::ItemAmount, progression::cn_timestamp};
use serde::Deserialize;
use std::{collections::BTreeMap, io, sync::OnceLock};

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ShopDefinition {
    pub shop_type: String,
    pub open_state_type: String,
    pub refresh_type: String,
    pub refresh_param: u32,
    pub city_id: u32,
    pub city_discount_level: u32,
    pub scoin_discount_rate: u32,
    pub begin_time: String,
    pub end_time: String,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Goods {
    pub goods_id: u32,
    pub shop_type: u32,
    #[serde(rename = "DLKNJDPMAKH")]
    pub goods_type: u32,
    pub item_id: u32,
    pub item_count: u32,
    pub rotate_id: u32,
    pub cost_items: Vec<ItemAmount>,
    pub cost_hcoin: u32,
    pub cost_mcoin: u32,
    pub cost_scoin: u32,
    pub buy_limit: u32,
    pub is_buy_once: bool,
    pub choose_one_group_id: u32,
    pub sort_level: u32,
    pub min_player_level: u32,
    pub max_player_level: u32,
    pub min_show_level: u32,
    pub begin_time: String,
    pub end_time: String,
    pub refresh_type: String,
    pub refresh_param: u32,
    pub precondition: String,
    pub precondition_param_list: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rotation {
    rotate_id: u32,
    rotate_order: u32,
    item_id: u32,
}

#[derive(Default)]
pub struct ShopConfig {
    pub shops: BTreeMap<u32, ShopDefinition>,
    pub goods: BTreeMap<u32, Goods>,
    pub entrances: Vec<u32>,
    rotations: BTreeMap<u32, BTreeMap<u32, u32>>,
    by_shop: BTreeMap<u32, Vec<u32>>,
}

static CONFIG: OnceLock<ShopConfig> = OnceLock::new();

impl ShopConfig {
    pub fn get() -> &'static Self {
        CONFIG.get().expect("shop tables loaded")
    }

    pub fn load(root: &str) -> io::Result<()> {
        fn rows<T: serde::de::DeserializeOwned>(root: &str, name: &str) -> io::Result<Vec<T>> {
            serde_json::from_slice(&std::fs::read(format!("{root}/{name}.json"))?)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{name}: {e}")))
        }
        fn invalid() -> io::Error {
            io::Error::new(io::ErrorKind::InvalidData, "invalid shop configuration")
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Entrance {
            shop_type: String,
        }
        let mut data = Self::default();
        for row in rows::<ShopDefinition>(root, "ShopExcelConfigData")? {
            let id = shop_type(&row.shop_type).ok_or_else(invalid)?;
            if !valid_date(&row.begin_time)
                || !valid_date(&row.end_time)
                || !valid_refresh(&row.refresh_type, row.refresh_param)
                || data.shops.insert(id, row).is_some()
            {
                return Err(invalid());
            }
        }
        for row in rows::<Goods>(root, "ShopGoodsExcelConfigData")? {
            data.by_shop
                .entry(row.shop_type)
                .or_default()
                .push(row.goods_id);
            if !valid_date(&row.begin_time)
                || !valid_date(&row.end_time)
                || !valid_refresh(&row.refresh_type, row.refresh_param)
                || data.goods.insert(row.goods_id, row).is_some()
            {
                return Err(invalid());
            }
        }
        for row in rows::<Rotation>(root, "ShopRotateExcelConfigData")? {
            if row.rotate_order == 0
                || row.item_id == 0
                || data
                    .rotations
                    .entry(row.rotate_id)
                    .or_default()
                    .insert(row.rotate_order, row.item_id)
                    .is_some()
            {
                return Err(invalid());
            }
        }
        for row in rows::<Entrance>(root, "ShopmallEntranceExcelConfigData")? {
            let id = shop_type(&row.shop_type).ok_or_else(invalid)?;
            if !data.entrances.contains(&id) {
                data.entrances.push(id);
            }
        }
        let _ = CONFIG.set(data);
        Ok(())
    }

    pub fn item_id(&self, goods: &Goods, now: u32) -> Option<u32> {
        if goods.rotate_id == 0 {
            return (goods.item_id != 0).then_some(goods.item_id);
        }
        let entries = self.rotations.get(&goods.rotate_id)?;
        let start = cn_timestamp(&goods.begin_time)?;
        let elapsed = month_index(now).checked_sub(month_index(start))?;
        // 轮换从商品自己的开售月份起算，不能用通用占位日期。
        let order = elapsed % u32::try_from(entries.len()).ok()? + 1;
        entries.get(&order).copied()
    }

    pub fn goods_for_shop(&self, id: u32) -> impl Iterator<Item = &Goods> {
        self.by_shop
            .get(&id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.goods.get(id))
    }
}

fn valid_date(text: &str) -> bool {
    text.is_empty() || cn_timestamp(text).is_some()
}
fn valid_refresh(kind: &str, count: u32) -> bool {
    kind.is_empty()
        || (matches!(
            kind,
            "SHOP_REFRESH_DAILY" | "SHOP_REFRESH_WEEKLY" | "SHOP_REFRESH_MONTHLY"
        ) && (1..=120).contains(&count))
}
pub fn in_time(begin: &str, end: &str, now: u32) -> bool {
    (begin.is_empty() || cn_timestamp(begin).is_some_and(|v| now >= v))
        && (end.is_empty() || cn_timestamp(end).is_some_and(|v| now <= v))
}
fn leap(year: u32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}
fn month_index(now: u32) -> u32 {
    let mut days = (u64::from(now) + 14400) / 86400;
    let mut year = 1970;
    loop {
        let count = if leap(year) { 366 } else { 365 };
        if days < count {
            break;
        }
        days -= count;
        year += 1;
    }
    let lengths = [
        31,
        28 + u32::from(leap(year)),
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
    let mut month = 0;
    while days >= u64::from(lengths[month]) {
        days -= u64::from(lengths[month]);
        month += 1;
    }
    year * 12 + month as u32
}

pub fn next_refresh(kind: &str, count: u32, now: u32) -> Option<u32> {
    let days = (u64::from(now) + 14400) / 86400;
    let count = u64::from(count.max(1));
    let next = match kind {
        "SHOP_REFRESH_DAILY" => (days + count) * 86400 - 14400,
        "SHOP_REFRESH_WEEKLY" => (days + 7 - (days + 3) % 7 + (count - 1) * 7) * 86400 - 14400,
        "SHOP_REFRESH_MONTHLY" => {
            let month = u64::from(month_index(now)) + count;
            return cn_timestamp(&format!(
                "{:04}-{:02}-01 04:00:00",
                month / 12,
                month % 12 + 1
            ));
        }
        _ => return None,
    };
    u32::try_from(next).ok()
}

// 与客户端 ShopType 一致，shopId 不是协议里的 shop_type。
pub fn shop_type(name: &str) -> Option<u32> {
    match name {
        "SHOP_TYPE_RECOMMEND" => Some(900),
        "SHOP_TYPE_PACKAGE" => Some(902),
        "SHOP_TYPE_MCOIN" => Some(903),
        "SHOP_TYPE_PAIMON" => Some(1001),
        "SHOP_TYPE_CITY" => Some(1002),
        "SHOP_TYPE_BLACKSMITH" => Some(1003),
        "SHOP_TYPE_GROCERY" => Some(1004),
        "SHOP_TYPE_FOOD" => Some(1005),
        "SHOP_TYPE_SEA_LAMP" => Some(1006),
        "SHOP_TYPE_VIRTUAL_SHOP" => Some(1007),
        "SHOP_TYPE_LIYUE_GROCERY" => Some(1008),
        "SHOP_TYPE_LIYUE_SOUVENIR" => Some(1009),
        "SHOP_TYPE_LIYUE_RESTAURANT" => Some(1010),
        "SHOP_TYPE_NPC_Flora" => Some(1011),
        "SHOP_TYPE_NPC_Charles" => Some(1012),
        "SHOP_TYPE_NPC_Shiliu" => Some(1013),
        "SHOP_TYPE_NPC_Schulz" => Some(1014),
        "SHOP_TYPE_NPC_Brook" => Some(1015),
        "SHOP_TYPE_NPC_Hopkins" => Some(1016),
        "SHOP_TYPE_NPC_Draff" => Some(1017),
        "SHOP_TYPE_NPC_Chloris" => Some(1018),
        "SHOP_TYPE_NPC_Licai" => Some(1019),
        "SHOP_TYPE_NPC_Yueshu" => Some(1020),
        "SHOP_TYPE_NPC_Gui" => Some(1021),
        "SHOP_TYPE_NPC_Gao" => Some(1022),
        "SHOP_TYPE_NPC_Sun" => Some(1023),
        "SHOP_TYPE_NPC_Qiming" => Some(1024),
        "SHOP_TYPE_NPC_Zhangshun" => Some(1025),
        "SHOP_TYPE_NPC_Chen" => Some(1026),
        "SHOP_TYPE_NPC_ErNiang" => Some(1027),
        "SHOP_TYPE_NPC_Shitou" => Some(1028),
        "SHOP_TYPE_NPC_Jifang" => Some(1029),
        "SHOP_TYPE_NPC_Zhu" => Some(1030),
        "SHOP_TYPE_NPC_Bai" => Some(1031),
        "SHOP_TYPE_NPC_Kai" => Some(1032),
        "SHOP_TYPE_NPC_Linglang" => Some(1033),
        "SHOP_TYPE_NPC_VerrGoldet" => Some(1034),
        "SHOP_TYPE_NPC_Zhou" => Some(1035),
        "SHOP_TYPE_TASK_Ekaterina" => Some(1036),
        "SHOP_TYPE_ACTIVITY_ASTER" => Some(1037),
        "SHOP_TYPE_TASK_Tartaglia" => Some(1038),
        "SHOP_TYPE_NPC_Harris" => Some(1039),
        "SHOP_TYPE_ACTIVITY_DRAGON_SPINE" => Some(1040),
        "SHOP_TYPE_ACTIVITY_TREASURE_MAP" => Some(1041),
        "SHOP_TYPE_NPC_Yinian" => Some(1042),
        "SHOP_TYPE_ACTIVITY_SEA_LAMP" => Some(1043),
        "SHOP_TYPE_ACTIVITY_FLEUR_FAIR" => Some(1044),
        "SHOP_TYPE_NPC_Changshun" => Some(1045),
        "SHOP_TYPE_NPC_Bolai" => Some(1046),
        "SHOP_TYPE_NPC_Ashanpo" => Some(1047),
        "SHOP_TYPE_HOME" => Some(1048),
        "SHOP_TYPE_HOME_LIMIT" => Some(1049),
        "SHOP_TYPE_NPC_MasterLu" => Some(1050),
        "SHOP_TYPE_NPC_Goth" => Some(1051),
        "SHOP_TYPE_COSTUME" => Some(1052),
        "SHOP_TYPE_NPC_Obata" => Some(1053),
        "SHOP_TYPE_NPC_Qiuyue" => Some(1054),
        "SHOP_TYPE_NPC_Ryouko" => Some(1055),
        "SHOP_TYPE_INAZUMA_GROCERY" => Some(1056),
        "SHOP_TYPE_INAZUMA_SOUVENIR" => Some(1057),
        "SHOP_TYPE_INAZUMA_RESTAURANT" => Some(1058),
        "SHOP_TYPE_NPC_Kuroda" => Some(1059),
        "SHOP_TYPE_NPC_KiminamiAnna" => Some(1060),
        "SHOP_TYPE_NPC_Tomoki" => Some(1061),
        "SHOP_TYPE_NPC_Karpillia" => Some(1062),
        "SHOP_TYPE_BLACKSMITH_INAZUMA" => Some(1063),
        "SHOP_TYPE_FISH" => Some(1064),
        "SHOP_TYPE_FISH_LIYUE" => Some(1065),
        "SHOP_TYPE_FISH_INAZUMA" => Some(1066),
        "SHOP_TYPE_NPC_Kiyoko" => Some(1067),
        "SHOP_TYPE_EXPIRED_WIDGET_MENGDE" => Some(1068),
        "SHOP_TYPE_CAPTURE_ANIMAL_SHOP" => Some(1069),
        "SHOP_TYPE_NPC_YamashiroKenta" => Some(1070),
        "SHOP_TYPE_FISH_SUMERU" => Some(1071),
        "SHOP_TYPE_NPC_Pam" => Some(1072),
        "SHOP_TYPE_NPC_Lambad" => Some(1073),
        "SHOP_TYPE_NPC_Hamawi" => Some(1074),
        "SHOP_TYPE_NPC_Khalid" => Some(1075),
        "SHOP_TYPE_NPC_Ahangar" => Some(1076),
        "SHOP_TYPE_NPC_Afshin" => Some(1077),
        "SHOP_TYPE_NPC_Jut" => Some(1078),
        "SHOP_TYPE_NPC_Butrus" => Some(1079),
        "SHOP_TYPE_NPC_Jahangir" => Some(1080),
        "SHOP_TYPE_NPC_Qiuwei" => Some(1081),
        "SHOP_TYPE_NPC_Babak" => Some(1082),
        "SHOP_TYPE_NPC_Rama" => Some(1083),
        "SHOP_TYPE_NPC_Ashpazi" => Some(1084),
        "SHOP_TYPE_NPC_Aramani" => Some(1085),
        "SHOP_TYPE_NPC_Aravinay" => Some(1086),
        "SHOP_TYPE_NPC_Enteka" => Some(1087),
        "SHOP_TYPE_NPC_Azalai" => Some(1088),
        "SHOP_TYPE_EXPIRED_WIDGET_INAZUMA" => Some(1089),
        "SHOP_TYPE_FISH_FONTAINE" => Some(1090),
        "SHOP_TYPE_NPC_Establet" => Some(1091),
        "SHOP_TYPE_NPC_Mequignon" => Some(1092),
        "SHOP_TYPE_NPC_Boucicaut" => Some(1093),
        "SHOP_TYPE_NPC_Sanguinetti" => Some(1094),
        "SHOP_TYPE_NPC_Hubel" => Some(1095),
        "SHOP_TYPE_NPC_Eugene" => Some(1096),
        "SHOP_TYPE_NPC_Louis" => Some(1097),
        "SHOP_TYPE_NPC_Crassa" => Some(1098),
        "SHOP_TYPE_NPC_Xana" => Some(1099),
        "SHOP_TYPE_NPC_Arouet" => Some(1100),
        "SHOP_TYPE_NPC_Hinterman" => Some(1101),
        "SHOP_TYPE_NPC_Alvard_01" => Some(1102),
        "SHOP_TYPE_NPC_Alvard_02" => Some(1103),
        "SHOP_TYPE_NPC_Wolsey" => Some(1104),
        "SHOP_TYPE_NPC_Pitot" => Some(1105),
        "SHOP_TYPE_NPC_Fonta_make_meka" => Some(1106),
        "SHOP_TYPE_NPC_Pahsiv_02" => Some(1108),
        "SHOP_TYPE_NPC_Lianfang" => Some(1109),
        "SHOP_TYPE_NPC_Fengtai" => Some(1110),
        "SHOP_TYPE_NPC_Zhenyuan" => Some(1111),
        "SHOP_TYPE_NPC_Siyah" => Some(1112),
        "SHOP_TYPE_NPC_Chanca" => Some(1113),
        "SHOP_TYPE_NPC_Alom" => Some(1114),
        "SHOP_TYPE_NPC_Echuah" => Some(1115),
        "SHOP_TYPE_NPC_Cipac" => Some(1116),
        "SHOP_TYPE_NPC_Bounama" => Some(1117),
        "SHOP_TYPE_NPC_Cintli" => Some(1118),
        "SHOP_TYPE_NPC_Imix" => Some(1119),
        "SHOP_TYPE_NPC_Amazolli" => Some(1120),
        "SHOP_TYPE_FISH_NATLAN" => Some(1122),
        "SHOP_TYPE_NPC_Anchieta" => Some(1123),
        "SHOP_TYPE_EXPIRED_WIDGET_FONTAIN" => Some(1124),
        "SHOP_TYPE_NPC_Macuahuitl" => Some(1125),
        "SHOP_TYPE_NPC_Teuciztli" => Some(1126),
        "SHOP_TYPE_NPC_Fournier" => Some(1127),
        "SHOP_TYPE_NPC_Motolinia" => Some(1128),
        "SHOP_TYPE_NPC_Katya" => Some(1130),
        "SHOP_TYPE_NPC_Lyulka" => Some(1131),
        "SHOP_TYPE_NPC_Reko" => Some(1132),
        "SHOP_TYPE_NPC_Limppu" => Some(1133),
        "SHOP_TYPE_NPC_CaiRuo" => Some(1134),
        "SHOP_TYPE_NPC_Thabit" => Some(1135),
        "SHOP_TYPE_NPC_Richeza" => Some(1136),
        "SHOP_TYPE_NPC_Hedvika" => Some(1137),
        "SHOP_TYPE_NPC_Demyan" => Some(1138),
        "SHOP_TYPE_NPC_Tarno" => Some(1139),
        "SHOP_TYPE_NPC_Punchkickduck" => Some(1140),
        "SHOP_TYPE_NPC_Nataba" => Some(1141),
        "SHOP_TYPE_NPC_Kofitse" => Some(1142),
        "SHOP_TYPE_NPC_Kuzmich01" => Some(1143),
        "SHOP_TYPE_NPC_Kuzmich02" => Some(1144),
        "SHOP_TYPE_NPC_Kuzmich03" => Some(1145),
        "SHOP_TYPE_NPC_Josephus" => Some(1146),
        "SHOP_TYPE_NPC_Frith" => Some(1147),
        "SHOP_TYPE_NPC_Benthem" => Some(1148),
        "SHOP_TYPE_NPC_SNOW_FAIRY" => Some(1149),
        "SHOP_TYPE_NPC_Kuzmichev" => Some(1150),
        "SHOP_TYPE_NPC_Buturlina" => Some(1151),
        "SHOP_TYPE_NPC_Zyryanov" => Some(1152),
        "SHOP_TYPE_NPC_Grigory" => Some(1153),
        "SHOP_TYPE_NPC_Moes" => Some(1154),
        "SHOP_TYPE_NPC_Blyushko" => Some(1155),
        "SHOP_TYPE_NPC_Zherebtsov" => Some(1156),
        "SHOP_TYPE_NPC_Yaroslav" => Some(1157),
        "SHOP_TYPE_NPC_Knyazev" => Some(1158),
        "SHOP_TYPE_NPC_Zheleznov" => Some(1159),
        "SHOP_TYPE_NPC_Tolstaya" => Some(1160),
        "SHOP_TYPE_GCG_CARD" => Some(2000),
        "SHOP_TYPE_GCG_CARD_FACE" => Some(2001),
        "SHOP_TYPE_GCG_APPEARANCE" => Some(2002),
        "SHOP_TYPE_GCG_EXCHANGE" => Some(2003),
        "SHOP_TYPE_NPC_WOLF_POSE" => Some(2100),
        "SHOP_TYPE_MUSIC_GAME_BOOK" => Some(2200),
        "SHOP_TYPE_ACTIVITY_CHANNELLER_SLAB" => Some(15001),
        "SHOP_TYPE_ACTIVITY_SUMMER_TIME" => Some(16001),
        "SHOP_TYPE_ACTIVITY_BOUNCE_CONJURING" => Some(16002),
        "SHOP_TYPE_ACTIVITY_BLITZ_RUSH" => Some(20001),
        "SHOP_TYPE_ACTIVITY_CHESS" => Some(20002),
        "SHOP_TYPE_ACTIVITY_ROGUELIKE_DUNGEON" => Some(20003),
        "SHOP_TYPE_ACTIVITY_WINTER_CAMP" => Some(20004),
        "SHOP_TYPE_ACTIVITY_LANTERN_RITE" => Some(20005),
        "SHOP_TYPE_ACTIVITY_ROGUE_DIARY" => Some(27001),
        "SHOP_TYPE_ACTIVITY_SUMMER_TIME_V2" => Some(28001),
        "SHOP_TYPE_ACTIVITY_GRAVEN_INNOCENCE" => Some(30001),
        "SHOP_TYPE_ACTIVITY_VINTAGE_ANGEL_S_SHARE" => Some(31001),
        "SHOP_TYPE_ACTIVITY_FUNGUS_FIGHTER" => Some(32001),
        "SHOP_TYPE_ACTIVITY_EFFIGY_V2" => Some(32002),
        "SHOP_TYPE_ACTIVITY_BRICK_BREAKER" => Some(33001),
        "SHOP_TYPE_ACTIVITY_FLEURFAIR_V2" => Some(35001),
        "SHOP_TYPE_ACTIVITY_JOURNEY" => Some(37001),
        "SHOP_TYPE_ACTIVITY_TOY_BATTLE" => Some(40001),
        "SHOP_TYPE_ACTIVITY_FILMFEST" => Some(43002),
        "SHOP_TYPE_ACTIVITY_ALCHEMY_SIM" => Some(45001),
        "SHOP_TYPE_ACTIVITY_GREAT_FESTIVAL_V2_NPC" => Some(46001),
        "SHOP_TYPE_ACTIVITY_LANTERN_RITE_V6" => Some(63001),
        "SHOP_TYPE_BEYOND_MCOIN" => Some(100000),
        "SHOP_TYPE_BEYOND_COSTUME" => Some(101000),
        "SHOP_TYPE_BEYOND_FACE_PART" => Some(102000),
        "SHOP_TYPE_BEYOND_EXCHANGE" => Some(103000),
        "SHOP_TYPE_BEYOND_RECOMMEND" => Some(104000),
        "SHOP_TYPE_BEYOND_RERUN" => Some(105000),
        _ => None,
    }
}

pub fn shop_open_state(name: &str) -> Option<u32> {
    match name {
        "OPEN_STATE_SHOP_TYPE_RECOMMANDED" => Some(901),
        "OPEN_STATE_SHOP_TYPE_GENESISCRYSTAL" => Some(902),
        "OPEN_STATE_SHOP_TYPE_GIFTPACKAGE" => Some(903),
        "OPEN_STATE_SHOP_TYPE_PAIMON" => Some(1001),
        "OPEN_STATE_SHOP_TYPE_CITY" => Some(1002),
        "OPEN_STATE_SHOP_TYPE_BLACKSMITH" => Some(1003),
        "OPEN_STATE_SHOP_TYPE_GROCERY" => Some(1004),
        "OPEN_STATE_SHOP_TYPE_FOOD" => Some(1005),
        "OPEN_STATE_SHOP_TYPE_SEA_LAMP" => Some(1006),
        "OPEN_STATE_SHOP_TYPE_VIRTUAL_SHOP" => Some(1007),
        "OPEN_STATE_SHOP_TYPE_LIYUE_GROCERY" => Some(1008),
        "OPEN_STATE_SHOP_TYPE_LIYUE_SOUVENIR" => Some(1009),
        "OPEN_STATE_SHOP_TYPE_LIYUE_RESTAURANT" => Some(1010),
        "OPEN_STATE_SHOP_TYPE_INAZUMA_SOUVENIR" => Some(1011),
        "OPEN_STATE_SHOP_TYPE_NPC_TOMOKI" => Some(1012),
        "OPEN_STATE_HOME" => Some(1500),
        _ => None,
    }
}
