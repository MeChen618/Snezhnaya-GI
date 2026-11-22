use bevy_ecs::prelude::*;
use serde_json::{json, Value};
use snezhnaya_gi_data::{
    excel::{self, progression::ProgressionConfig},
    scene::{scene_point_config::get_scene_point_config_collection, script_cache::*},
};
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::retcode::Retcode;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::OnceLock,
};

fn number(v: &Value, key: &str) -> u32 {
    v.get(key)
        .and_then(Value::as_u64)
        .and_then(|v| v.try_into().ok())
        .unwrap_or(0)
}
fn dungeon_ids(point: &snezhnaya_gi_data::scene::scene_point_config::ScenePointData) -> Vec<u32> {
    let mut ids = point.dungeon_ids.clone();
    for id in &point.dungeon_random_list {
        if let Some(d) = excel::daily_dungeon_config_collection::get().get(id) {
            ids.extend(&d.sunday);
        }
    }
    ids.retain(|id| excel::dungeon_excel_config_collection::get().contains_key(id));
    ids.sort_unstable();
    ids.dedup();
    ids
}
fn daily(player: &snezhnaya_gi_proto::server_only::PlayerDataBin, scene: u32) -> Value {
    let points = get_scene_point_config_collection();
    let Some(points) = points.get(&scene) else {
        return json!({"retcode":i32::from(Retcode::RetNotFoundConfig)});
    };
    let level = player.basic_bin.as_ref().map_or(1, |b| b.level);
    let entries: Vec<_> = ProgressionConfig::get()
        .handbook_dungeons
        .values()
        .filter(|row| row.scene_id == scene && row.is_show_in_adv_handbook)
        .filter_map(|row| {
            let point = points.points.get(&row.dungeon_entry_id)?;
            let ids = dungeon_ids(point);
            let recommended = ids
                .iter()
                .filter_map(|id| excel::dungeon_excel_config_collection::get().get(id))
                .filter(|d| d.limit_level <= level)
                .max_by_key(|d| (d.limit_level, d.id))
                .map(|d| d.id)
                .or_else(|| ids.first().copied())?;
            let unlocked = player
                .scene_bin
                .as_ref()
                .and_then(|s| s.world.as_ref())
                .and_then(|w| w.scene_map.get(&scene))
                .is_some_and(|s| s.unlocked_point_list.contains(&row.dungeon_entry_id));
            Some(json!({
                "dungeon_entry_config_id": row.id,
                "dungeon_entry_id": row.dungeon_entry_id,
                "recommend_dungeon_id": recommended,
                "recommend_dungeon_entry_info": {"dungeon_id": recommended},
                "BMOKOMEOKCP": ids,
                "is_quick_open": unlocked,
                "FHAEFOBOCPA": true,
                "FKGGIKIPKOB": true
            }))
        })
        .collect();
    json!({"daily_dungeon_info_list":entries})
}
fn monster_details() -> &'static BTreeMap<u32, Value> {
    static DETAILS: OnceLock<BTreeMap<u32, Value>> = OnceLock::new();
    DETAILS.get_or_init(|| {
        let cfg = ProgressionConfig::get();
        let needed: HashSet<_> = cfg.handbook_monsters.values()
            .flat_map(|m| m.group_id_list.iter().copied()).collect();
        let mut index = HashMap::new();
        if let Some(blocks) = SCENE_BLOCK_COLLECTION.get() {
            for ((scene, block), data) in blocks.iter() {
                for group in &data.groups {
                    if needed.contains(&group.id) { index.insert(group.id, (*scene, *block)); }
                }
            }
        }
        let Some(lua) = SCENE_LUA_VM.get() else { return BTreeMap::new(); };
        cfg.handbook_monsters.values().filter_map(|row| {
            for group_id in &row.group_id_list {
                let Some(&(scene, block)) = index.get(group_id) else { continue; };
                // 读取静态配置，不为打开手册生成战斗实体。
                let Some(group) = load_scene_group_from_cache("assets/lua", lua, scene, block, *group_id) else { continue; };
                let Some(monster) = group.monsters.iter().find(|m| row.monster_id_list.contains(&m.monster_id)) else { continue; };
                return Some((row.id, json!({
                    "level": monster.level.unwrap_or(1),
                    "is_alive": true,
                    "resin": if row.monster_category == "Boss" {40} else {0},
                    "pos": {"x": monster.pos.x, "y": monster.pos.y, "z": monster.pos.z},
                    "monster_config": {"monster_id": monster.monster_id, "scene_id": scene, "group_id": group_id}
                })));
            }
            None
        }).collect()
    })
}

pub fn handle(
    mut messages: MessageReader<ClientMessageEvent>,
    players: Res<Players>,
    output: Res<MessageOutput>,
) {
    for message in messages.read() {
        let name = message.message_name();
        if !matches!(
            name,
            "GetDailyDungeonEntryInfoReq"
                | "GetInvestigationMonsterReq"
                | "GetDungeonEntryExploreConditionReq"
        ) {
            continue;
        }
        let Some(player) = players.get(message.sender_uid()) else {
            continue;
        };
        let Some(req) = message.decode_json() else {
            continue;
        };
        match name {
            "GetDailyDungeonEntryInfoReq" => {
                let scene = number(&req, "scene_id");
                output.send_json(
                    player.uid,
                    "GetDailyDungeonEntryInfoRsp",
                    daily(player, if scene == 0 { 3 } else { scene }),
                );
            }
            "GetInvestigationMonsterReq" => {
                let cities: HashSet<_> = req
                    .get("city_id_list")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_u64)
                    .collect();
                let details = monster_details();
                let monsters: Vec<_> = ProgressionConfig::get()
                    .handbook_monsters
                    .values()
                    .filter(|row| cities.is_empty() || cities.contains(&u64::from(row.city_id)))
                    .filter_map(|row| {
                        details.get(&row.id).map(|detail| {
                            json!({
                                "id": row.id, "city_id": row.city_id,
                                "investigation_monster_detail_list": [detail]
                            })
                        })
                    })
                    .collect();
                output.send_json(player.uid, "GetInvestigationMonsterRsp", json!({
                    "is_for_mark": req.get("is_for_mark").and_then(Value::as_bool).unwrap_or(false),
                    "monster_list": monsters
                }));
            }
            _ => {
                let id = number(&req, "dungeon_entry_config_id");
                let config = ProgressionConfig::get().handbook_dungeons.get(&id);
                let valid = config.is_some_and(|row| {
                    row.scene_id == number(&req, "scene_id")
                        && row.dungeon_entry_id == number(&req, "scene_point_id")
                });
                output.send_json(
                    player.uid,
                    "GetDungeonEntryExploreConditionRsp",
                    json!({
                        "retcode": if valid {0} else {i32::from(Retcode::RetNotFoundConfig)},
                        "dungeon_entry_cond": {}
                    }),
                );
            }
        }
    }
}
