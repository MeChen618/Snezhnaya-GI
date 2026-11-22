use crate::transaction::*;
use rand::seq::SliceRandom;
use snezhnaya_gi_data::excel::*;
use snezhnaya_gi_message::output::MessageOutput;
use snezhnaya_gi_proto::{
    normal::*,
    retcode::Retcode,
    server_only::{equip_bin, item_bin, PlayerDataBin},
};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct TransmuterConfig {
    widget: u32,
    elixir: u32,
    purchase_props: u32,
    purchase_guarantee: u32,
    purchase_costs: Vec<u32>,
    purchase_limit: u32,
    offering_rank: u32,
    offering_min_level: u32,
    offering_max_count: u32,
    offering_max_progress: u32,
    offering_limit: u32,
    offering_levels: Vec<u32>,
    offering_materials: HashMap<u32, f64>,
    dust: u32,
    dust_props: u32,
    dust_level: u32,
    dust_rank: u32,
    dust_min_props: u32,
    dust_costs: HashMap<u32, u32>,
    dust_purchased_costs: HashMap<u32, u32>,
    dust_guarantees: HashMap<u32, u32>,
    dust_tiers: Vec<(u32, u32)>,
}

fn transmuter_config() -> InventoryResult<&'static TransmuterConfig> {
    static CONFIG: std::sync::OnceLock<Option<TransmuterConfig>> = std::sync::OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let parse = || -> Option<TransmuterConfig> {
                let rows = &production::ProductionConfig::get().constants;
                let p = rows.get("CONST_VALUE_COMPOUND_SPE_RELIQUARY_PARAMS_1")?;
                let o = rows.get("CONST_VALUE_COMPOUND_SPE_RELIQUARY_PARAMS_2")?;
                let m = rows.get("CONST_VALUE_COMPOUND_SPE_RELIQUARY_PARAMS_3")?;
                let d = rows.get("CONST_VALUE_RELIQUARY_DUST_PARAM_1")?;
                let t = rows.get("CONST_VALUE_RELIQUARY_DUST_PARAM_2")?;
                let l = rows.get("CONST_VALUE_PURCHASE_RELIQUARY_PARAM")?;
                fn number(values: &[String], i: usize) -> Option<u32> {
                    values.get(i)?.parse().ok()
                }
                fn list(value: &str) -> Option<Vec<u32>> {
                    value.split(';').map(|v| v.parse().ok()).collect()
                }
                fn pairs(value: &str) -> Option<HashMap<u32, u32>> {
                    value
                        .split(';')
                        .map(|v| {
                            let (a, b) = v.split_once(',')?;
                            Some((a.parse().ok()?, b.parse().ok()?))
                        })
                        .collect()
                }
                let limits = list(d.get(1)?)?;
                let mut c = TransmuterConfig {
                    widget: number(p, 0)?,
                    purchase_props: number(p, 1)?,
                    purchase_guarantee: number(p, 2)?,
                    elixir: number(p, 3)?,
                    purchase_costs: list(p.get(4)?)?,
                    purchase_limit: number(l, 0)?,
                    offering_rank: number(o, 0)?,
                    offering_min_level: number(o, 1)?,
                    offering_max_count: number(o, 2)?,
                    offering_max_progress: number(o, 3)?,
                    offering_limit: number(o, 4)?,
                    offering_levels: list(o.get(5)?)?,
                    dust_props: number(d, 0)?,
                    dust_level: *limits.first()?,
                    dust_rank: *limits.get(1)?,
                    dust_min_props: *limits.get(2)?,
                    dust: number(d, 2)?,
                    dust_costs: pairs(d.get(3)?)?,
                    dust_purchased_costs: pairs(d.get(4)?)?,
                    dust_guarantees: pairs(d.get(5)?)?,
                    dust_tiers: pairs(t.first()?)?.into_iter().collect(),
                    ..Default::default()
                };
                for value in m.get(1)?.split(';') {
                    let (id, amount) = value.split_once(',')?;
                    let progress: f64 = amount.parse().ok()?;
                    if !progress.is_finite() || progress <= 0.0 {
                        return None;
                    }
                    c.offering_materials.insert(id.parse().ok()?, progress);
                }
                c.dust_tiers.sort_unstable();
                if c.purchase_costs.len() != 5
                    || c.purchase_props != 2
                    || c.dust_props != 2
                    || c.offering_max_progress == 0
                    || c.widget == 0
                    || c.elixir == 0
                    || c.dust_guarantees.get(&1).copied().unwrap_or(0) == 0
                {
                    return None;
                }
                Some(c)
            };
            let config = parse();
            if config.is_none() {
                tracing::warn!("invalid reliquary transmuter config; requests will be rejected");
            }
            config
        })
        .as_ref()
        .ok_or(Retcode::RetNotFoundConfig)
}

fn transmuter_state(
    player: &mut PlayerDataBin,
) -> &mut snezhnaya_gi_proto::server_only::ReliquaryTransmuterBin {
    const BEGIN: u32 = 1_785_528_000;
    const PERIOD: u32 = 42 * 86400;
    let now = ::common::time_util::unix_timestamp() as u32;
    let begin = BEGIN + now.saturating_sub(BEGIN) / PERIOD * PERIOD;
    let state = player
        .item_bin
        .get_or_insert_with(Default::default)
        .reliquary_transmuter
        .get_or_insert_with(Default::default);
    if state.schedule_id != begin {
        state.schedule_id = begin;
        state.end_time = begin.saturating_add(PERIOD - 1);
        state.offering_count = 0;
        state.purchase_count_map.clear();
    }
    state
}

fn transmuter_ready(
    player: &mut PlayerDataBin,
    schedule: u32,
) -> InventoryResult<&'static TransmuterConfig> {
    let config = transmuter_config()?;
    if !player
        .item_bin
        .as_ref()
        .is_some_and(|i| item_count(i, config.widget) > 0)
    {
        return Err(Retcode::RetItemNotExist);
    }
    if schedule != 0 && schedule != transmuter_state(player).schedule_id {
        return Err(Retcode::RetReliquaryDecomposeParamError);
    }
    Ok(config)
}

pub(super) fn transmuter_notify(player: &mut PlayerDataBin, output: &MessageOutput) {
    if transmuter_config().is_err() {
        return;
    }
    let uid = player.uid;
    let state = transmuter_state(player);
    let mut records: Vec<_> = state
        .purchase_count_map
        .iter()
        .map(
            |(&reliquary_set_id, &purchase_count)| ReliquaryPurchaseRecord {
                reliquary_set_id,
                purchase_count,
            },
        )
        .collect();
    records.sort_by_key(|r| r.reliquary_set_id);
    output.send(
        uid,
        "GetReliquaryOfferPurchaseInfoNotify",
        GetReliquaryOfferPurchaseInfoNotify {
            schedule_id: state.schedule_id,
            end_time: state.end_time,
            offering_count: state.offering_count,
            offering_progress: state.offering_progress.floor() as u32,
            offering_progress_extra: state.offering_progress.fract() as f32,
            purchase_record_list: records,
        },
    );
    output.send(
        uid,
        "ReliquaryReforgeDataNotify",
        ReliquaryReforgeDataNotify {
            cur_progress: state.dust_progress,
            reforge_record_list: state
                .pending_reforge
                .iter()
                .map(|p| ReliquaryReforgeRecord {
                    guid: p.guid,
                    append_prop_type_list: p.append_prop_type_list.clone(),
                    append_prop_id_list: p.append_prop_id_list.clone(),
                })
                .collect(),
        },
    );
}

pub(super) fn offering(
    player: &mut PlayerDataBin,
    request: &GetReliquaryOfferingReq,
    output: &MessageOutput,
) -> InventoryResult<GetReliquaryOfferingRsp> {
    let config = transmuter_ready(player, request.schedule_id)?;
    let state = transmuter_state(player).clone();
    if request.schedule_id != state.schedule_id
        || state.offering_count >= config.offering_limit
        || request.reliquary_guid_list.len() > config.offering_max_count as usize
        || request.item_list.len() > config.offering_materials.len()
    {
        return Err(Retcode::RetReliquaryDecomposeParamError);
    }
    let mut progress = 0.0;
    let items = player.item_bin.as_ref().unwrap();
    for guid in &request.reliquary_guid_list {
        let item = items.get_item(guid).ok_or(Retcode::RetItemNotExist)?;
        let Some(item_bin::Detail::Equip(e)) = item.detail.as_ref() else {
            return Err(Retcode::RetItemInvalidTarget);
        };
        let Some(equip_bin::Detail::Reliquary(relic)) = e.detail.as_ref() else {
            return Err(Retcode::RetItemInvalidTarget);
        };
        let row = reliquary_excel_config_collection::get()
            .get(&item.item_id)
            .ok_or(Retcode::RetNotFoundConfig)?;
        if row.rank_level != config.offering_rank || relic.level < config.offering_min_level {
            return Err(Retcode::RetReliquaryDecomposeParamError);
        }
        progress += *config
            .offering_levels
            .get(relic.level.saturating_sub(1) as usize)
            .ok_or(Retcode::RetReliquaryDecomposeParamError)? as f64;
    }
    let mut costs = Amounts::new();
    for item in &request.item_list {
        if item.count == 0 || costs.contains_key(&item.item_id) {
            return Err(Retcode::RetItemInvalidUseCount);
        }
        progress += item.count as f64
            * config
                .offering_materials
                .get(&item.item_id)
                .ok_or(Retcode::RetItemInvalidTarget)?;
        add_amount(&mut costs, item.item_id, item.count)?;
    }
    let total = state.offering_progress + progress;
    let earned = (total / config.offering_max_progress as f64).floor();
    if progress <= 0.0
        || !total.is_finite()
        || earned > (config.offering_limit - state.offering_count) as f64
    {
        return Err(Retcode::RetReliquaryDecomposeParamError);
    }
    let rewards = if earned > 0.0 {
        Amounts::from([(config.elixir, earned as u32)])
    } else {
        Amounts::new()
    };
    let change = transact(player, &costs, &rewards, &request.reliquary_guid_list)?;
    let state = transmuter_state(player);
    state.offering_progress = total - earned * config.offering_max_progress as f64;
    state.offering_count += earned as u32;
    notify_transaction(output, player, &change);
    transmuter_notify(player, output);
    Ok(GetReliquaryOfferingRsp {
        schedule_id: request.schedule_id,
        item_list: request.item_list.clone(),
        reliquary_guid_list: request.reliquary_guid_list.clone(),
        ..Default::default()
    })
}

fn roll_affix(depot: u32, prop: u32, upgrade: bool) -> InventoryResult<u32> {
    let candidates: Vec<_> = reliquary_affix_excel_config_collection::get()
        .values()
        .filter(|p| p.depot_id == depot && p.prop_type as u32 == prop)
        .collect();
    candidates
        .choose_weighted(&mut rand::thread_rng(), |p| {
            if upgrade {
                p.upgrade_weight
            } else {
                p.weight
            }
        })
        .map(|p| p.id)
        .map_err(|_| Retcode::RetNotFoundConfig)
}

pub(crate) fn guaranteed_upgrade(
    relic: &snezhnaya_gi_proto::server_only::ReliquaryBin,
    item: &ReliquaryExcelConfig,
) -> InventoryResult<Option<snezhnaya_gi_data::prop_type::FightPropType>> {
    if relic.purchased_append_prop_id_list.is_empty() || relic.append_prop_id_list.len() < 4 {
        return Ok(None);
    }
    let config = transmuter_config()?;
    let affixes = reliquary_affix_excel_config_collection::get();
    let mut chosen = Vec::new();
    for id in &relic.purchased_append_prop_id_list {
        let prop = affixes
            .get(id)
            .filter(|a| a.depot_id == item.append_prop_depot_id)
            .ok_or(Retcode::RetNotFoundConfig)?
            .prop_type;
        if !chosen.contains(&prop) {
            chosen.push(prop);
        }
    }
    if chosen.len() != config.purchase_props as usize {
        return Err(Retcode::RetItemInvalidTarget);
    }
    let hits = relic.append_prop_id_list[4..]
        .iter()
        .filter(|id| {
            affixes
                .get(id)
                .is_some_and(|a| chosen.contains(&a.prop_type))
        })
        .count() as u32;
    let remaining = item
        .add_prop_levels
        .iter()
        .filter(|level| **level >= relic.level)
        .count() as u32;
    if hits < config.purchase_guarantee && remaining <= config.purchase_guarantee - hits {
        return Ok(chosen.choose(&mut rand::thread_rng()).copied());
    }
    Ok(None)
}

pub(super) fn purchase(
    player: &mut PlayerDataBin,
    request: &GetReliquaryPurchaseReq,
    output: &MessageOutput,
) -> InventoryResult<GetReliquaryPurchaseRsp> {
    let config = transmuter_ready(player, request.schedule_id)?;
    let row = production::ProductionConfig::get()
        .purchase
        .get(&request.config_id)
        .ok_or(Retcode::RetNotFoundConfig)?;
    let state = transmuter_state(player);
    if request.schedule_id != state.schedule_id
        || !(1..=5).contains(&request.reliquary_type)
        || request.append_prop_type_list.len() != config.purchase_props as usize
        || state
            .purchase_count_map
            .get(&row.offer_set_id)
            .copied()
            .unwrap_or(0)
            >= config.purchase_limit
    {
        return Err(Retcode::RetReliquaryDecomposeParamError);
    }
    let ids = row.items(request.reliquary_type);
    if ids.len() != row.weights.len() {
        return Err(Retcode::RetNotFoundConfig);
    }
    let item_id = *ids
        .choose_weighted(&mut rand::thread_rng(), |id| {
            row.weights[ids.iter().position(|i| i == id).unwrap()]
        })
        .map_err(|_| Retcode::RetNotFoundConfig)?;
    let item = reliquary_excel_config_collection::get()
        .get(&item_id)
        .ok_or(Retcode::RetNotFoundConfig)?;
    if item.equip_type as u32 != request.reliquary_type
        || item.set_id != row.offer_set_id
        || item.append_prop_num < config.purchase_props
        || item.append_prop_num > 4
    {
        return Err(Retcode::RetNotFoundConfig);
    }
    let main = reliquary_main_prop_excel_config_collection::get()
        .values()
        .find(|p| {
            p.prop_depot_id == item.main_prop_depot_id
                && p.prop_type as u32 == request.main_prop_type
                && p.weight > 0
        })
        .ok_or(Retcode::RetItemInvalidTarget)?;
    let mut types = HashSet::from([request.main_prop_type]);
    let mut affixes = Vec::new();
    for &prop in &request.append_prop_type_list {
        if !types.insert(prop) {
            return Err(Retcode::RetItemInvalidTarget);
        }
        affixes.push(roll_affix(item.append_prop_depot_id, prop, false)?);
    }
    let chosen = affixes.clone();
    for _ in affixes.len()..item.append_prop_num as usize {
        let choices: Vec<_> = reliquary_affix_excel_config_collection::get()
            .values()
            .filter(|p| {
                p.depot_id == item.append_prop_depot_id
                    && p.weight > 0
                    && !types.contains(&(p.prop_type as u32))
            })
            .collect();
        let affix = choices
            .choose_weighted(&mut rand::thread_rng(), |p| p.weight)
            .map_err(|_| Retcode::RetNotFoundConfig)?;
        affixes.push(affix.id);
        types.insert(affix.prop_type as u32);
    }
    let cost = config.purchase_costs[request.reliquary_type as usize - 1];
    if cost == 0 {
        return Err(Retcode::RetNotFoundConfig);
    }
    let change = transact(
        player,
        &Amounts::from([(config.elixir, cost)]),
        &Amounts::from([(item_id, 1)]),
        &[],
    )?;
    let new_item = player
        .item_bin
        .as_mut()
        .unwrap()
        .get_mut_item(&change.added[0])
        .unwrap();
    if let Some(item_bin::Detail::Equip(e)) = new_item.detail.as_mut() {
        if let Some(equip_bin::Detail::Reliquary(relic)) = e.detail.as_mut() {
            relic.main_prop_id = main.id;
            relic.append_prop_id_list = affixes;
            relic.purchased_append_prop_id_list = chosen;
        }
    }
    *transmuter_state(player)
        .purchase_count_map
        .entry(row.offer_set_id)
        .or_default() += 1;
    notify_transaction(output, player, &change);
    transmuter_notify(player, output);
    Ok(GetReliquaryPurchaseRsp {
        schedule_id: request.schedule_id,
        config_id: request.config_id,
        ..Default::default()
    })
}

pub(super) fn reforge(
    player: &mut PlayerDataBin,
    request: &GetReliquaryDustReq,
    output: &MessageOutput,
) -> InventoryResult<GetReliquaryDustRsp> {
    use rand::Rng;
    let config = transmuter_ready(player, 0)?;
    let state = transmuter_state(player).clone();
    if state.pending_reforge.is_some()
        || request.append_prop_type_list.len() != config.dust_props as usize
    {
        return Err(Retcode::RetReliquaryDecomposeParamError);
    }
    let item = player
        .item_bin
        .as_ref()
        .and_then(|i| i.get_item(&request.guid))
        .ok_or(Retcode::RetItemNotExist)?;
    let Some(item_bin::Detail::Equip(equip)) = item.detail.as_ref() else {
        return Err(Retcode::RetItemInvalidTarget);
    };
    let Some(equip_bin::Detail::Reliquary(relic)) = equip.detail.as_ref() else {
        return Err(Retcode::RetItemInvalidTarget);
    };
    let row = reliquary_excel_config_collection::get()
        .get(&item.item_id)
        .ok_or(Retcode::RetNotFoundConfig)?;
    let expected = row.append_prop_num as usize
        + row
            .add_prop_levels
            .iter()
            .filter(|level| **level <= relic.level)
            .count();
    if row.rank_level != config.dust_rank
        || relic.level != config.dust_level
        || row.append_prop_num < config.dust_min_props
        || relic.append_prop_id_list.len() != expected
        || expected < 4
    {
        return Err(Retcode::RetReliquaryDecomposeParamError);
    }
    let cost_map = if relic.purchased_append_prop_id_list.is_empty() {
        &config.dust_costs
    } else {
        &config.dust_purchased_costs
    };
    let cost = *cost_map
        .get(&(row.equip_type as u32))
        .filter(|v| **v > 0)
        .ok_or(Retcode::RetNotFoundConfig)?;
    let next = state
        .dust_progress
        .checked_add(cost)
        .ok_or(Retcode::RetItemExceedLimit)?;
    let tier = config
        .dust_tiers
        .iter()
        .filter(|(progress, _)| state.dust_progress < *progress && next >= *progress)
        .last()
        .map_or(1, |(_, tier)| *tier);
    let guaranteed = *config
        .dust_guarantees
        .get(&tier)
        .ok_or(Retcode::RetNotFoundConfig)?;
    let upgrades = expected - 4;
    if guaranteed > upgrades as u32 {
        return Err(Retcode::RetNotFoundConfig);
    }
    let mut types = Vec::new();
    for id in &relic.append_prop_id_list[..4] {
        let affix = reliquary_affix_excel_config_collection::get()
            .get(id)
            .ok_or(Retcode::RetNotFoundConfig)?;
        if affix.depot_id != row.append_prop_depot_id || types.contains(&(affix.prop_type as u32)) {
            return Err(Retcode::RetItemInvalidTarget);
        }
        types.push(affix.prop_type as u32);
    }
    let chosen: HashSet<_> = request.append_prop_type_list.iter().copied().collect();
    if chosen.len() != config.dust_props as usize || chosen.iter().any(|p| !types.contains(p)) {
        return Err(Retcode::RetItemInvalidTarget);
    }
    let mut candidate: Vec<_> = types
        .iter()
        .map(|p| roll_affix(row.append_prop_depot_id, *p, false))
        .collect::<InventoryResult<_>>()?;
    let mut rng = rand::thread_rng();
    let upgrade_types = (0..10000)
        .find_map(|_| {
            let drawn: Vec<_> = (0..upgrades)
                .map(|_| types[rng.gen_range(0..types.len())])
                .collect();
            (drawn.iter().filter(|p| chosen.contains(p)).count() >= guaranteed as usize)
                .then_some(drawn)
        })
        .ok_or(Retcode::RetFail)?;
    for prop in upgrade_types {
        candidate.push(roll_affix(row.append_prop_depot_id, prop, true)?);
    }
    let change = transact(
        player,
        &Amounts::from([(config.dust, cost)]),
        &Amounts::new(),
        &[],
    )?;
    let state = transmuter_state(player);
    state.dust_progress =
        config.dust_tiers.last().map_or(
            next,
            |(cycle, _)| if *cycle > 0 { next % cycle } else { next },
        );
    state.pending_reforge = Some(snezhnaya_gi_proto::server_only::ReliquaryReforgeBin {
        guid: request.guid,
        append_prop_id_list: candidate,
        append_prop_type_list: request.append_prop_type_list.clone(),
    });
    notify_transaction(output, player, &change);
    transmuter_notify(player, output);
    Ok(GetReliquaryDustRsp {
        guid: request.guid,
        ..Default::default()
    })
}

pub(super) fn reforge_select(
    player: &mut PlayerDataBin,
    request: &GetReliquaryDustSelectReq,
    output: &MessageOutput,
) -> InventoryResult<GetReliquaryDustSelectRsp> {
    let pending = transmuter_state(player)
        .pending_reforge
        .clone()
        .filter(|p| p.guid == request.guid)
        .ok_or(Retcode::RetItemNotExist)?;
    let item = player
        .item_bin
        .as_mut()
        .and_then(|i| i.get_mut_item(&request.guid))
        .ok_or(Retcode::RetItemNotExist)?;
    if request.is_use_new {
        let Some(item_bin::Detail::Equip(e)) = item.detail.as_mut() else {
            return Err(Retcode::RetItemInvalidTarget);
        };
        let Some(equip_bin::Detail::Reliquary(relic)) = e.detail.as_mut() else {
            return Err(Retcode::RetItemInvalidTarget);
        };
        relic.append_prop_id_list = pending.append_prop_id_list;
        relic.definite_append_prop_id_list.clear();
        let item = item.clone();
        if let Some(avatars) = player.avatar_bin.as_mut() {
            if let Some(avatar) = avatars.avatar_map.get_mut(&item.owner_guid) {
                let slot = reliquary_excel_config_collection::get()
                    .get(&item.item_id)
                    .ok_or(Retcode::RetNotFoundConfig)?
                    .equip_type as u32;
                avatar.equip_map.insert(slot, item);
            }
        }
        notify_transaction(
            output,
            player,
            &InventoryChange {
                changed: vec![request.guid],
                ..Default::default()
            },
        );
    }
    transmuter_state(player).pending_reforge = None;
    transmuter_notify(player, output);
    Ok(GetReliquaryDustSelectRsp::default())
}
