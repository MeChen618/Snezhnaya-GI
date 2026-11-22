use crate::transaction::*;
use bevy_ecs::prelude::*;
use common::time_util::unix_timestamp;
use snezhnaya_gi_data::{excel::production::*, prop_type::PROP_PLAYER_FORGE_POINT};
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::{
    normal::*,
    retcode::Retcode,
    server_only::{CompoundOutputBin, ForgeQueueBin, PlayerDataBin},
};
use std::collections::HashMap;

const DAILY_FORGE_POINTS: u32 = 300_000;

fn now() -> u32 {
    unix_timestamp() as u32
}
fn ids<T>(map: &HashMap<u32, T>) -> Vec<u32> {
    let mut keys: Vec<_> = map.keys().copied().collect();
    keys.sort_unstable();
    keys
}
fn params(items: &Amounts) -> Vec<ItemParam> {
    items
        .iter()
        .map(|(&item_id, &count)| ItemParam { item_id, count })
        .collect()
}
fn hints(items: &Amounts) -> Vec<ItemHint> {
    items
        .iter()
        .map(|(&item_id, &count)| ItemHint {
            item_id,
            count,
            ..Default::default()
        })
        .collect()
}
fn count_valid(count: u32) -> InventoryResult<()> {
    if count == 0 || count > 9999 {
        Err(Retcode::RetItemInvalidUseCount)
    } else {
        Ok(())
    }
}
fn costs(materials: &[ItemAmount], mora: u32, count: u32) -> InventoryResult<Amounts> {
    let mut costs = scale_items(materials, count)?;
    add_amount(
        &mut costs,
        202,
        mora.checked_mul(count).ok_or(Retcode::RetItemExceedLimit)?,
    )?;
    Ok(costs)
}
fn execute(
    player: &mut PlayerDataBin,
    costs: &Amounts,
    rewards: &Amounts,
    output: &MessageOutput,
) -> InventoryResult<()> {
    let change = transact(player, costs, rewards, &[])?;
    notify_transaction(output, player, &change);
    Ok(())
}
fn forge_limit(player: &PlayerDataBin) -> u32 {
    match player.basic_bin.as_ref().map_or(1, |b| b.level) {
        0..5 => 1,
        5..10 => 2,
        10..15 => 3,
        _ => 4,
    }
}
fn refresh_forge(player: &mut PlayerDataBin, timestamp: u32) {
    let forge = player.forge_bin.get_or_insert_with(Default::default);
    // 国服每日四点对应 UTC 前一天二十点。
    if forge.last_refresh_time == 0
        || (timestamp as u64 + 14_400) / 86_400
            != (forge.last_refresh_time as u64 + 14_400) / 86_400
    {
        forge.forge_point = DAILY_FORGE_POINTS;
        forge.last_refresh_time = timestamp;
    }
}
fn finished(queue: &ForgeQueueBin, timestamp: u32) -> u32 {
    (timestamp.saturating_sub(queue.start_time) / queue.single_time_cost.max(1))
        .min(queue.forge_count)
}
fn forge_data(player: &PlayerDataBin, timestamp: u32) -> HashMap<u32, ForgeQueueData> {
    player
        .forge_bin
        .iter()
        .flat_map(|b| b.forge_queue_map.iter())
        .map(|(&id, q)| {
            let completed = finished(q, timestamp);
            (
                id,
                ForgeQueueData {
                    queue_id: id,
                    forge_id: q.forge_id,
                    avatar_id: q.avatar_id,
                    finish_count: completed.saturating_sub(q.taken_count),
                    unfinish_count: q.forge_count - completed,
                    next_finish_timestamp: if completed < q.forge_count {
                        q.start_time
                            .saturating_add((completed + 1).saturating_mul(q.single_time_cost))
                    } else {
                        0
                    },
                    total_finish_timestamp: q
                        .start_time
                        .saturating_add(q.forge_count.saturating_mul(q.single_time_cost)),
                },
            )
        })
        .collect()
}
fn forge_notify(player: &PlayerDataBin, output: &MessageOutput, removed: Vec<u32>) {
    output.send(
        player.uid,
        "ForgeQueueDataNotify",
        ForgeQueueDataNotify {
            forge_queue_map: forge_data(player, now()),
            removed_forge_queue_list: removed,
        },
    );
    if let Some(forge) = &player.forge_bin {
        output.send(
            player.uid,
            "PlayerPropNotify",
            PlayerPropNotify {
                prop_map: HashMap::from([(
                    PROP_PLAYER_FORGE_POINT,
                    PropValue {
                        r#type: PROP_PLAYER_FORGE_POINT,
                        val: forge.forge_point as i64,
                        value: Some(prop_value::Value::Ival(forge.forge_point as i64)),
                    },
                )]),
            },
        );
    }
}
fn compound_data(player: &PlayerDataBin, timestamp: u32) -> Vec<CompoundQueueData> {
    let Some(cook) = &player.cook_bin else {
        return vec![];
    };
    ids(&cook.compound_que_bin_map)
        .into_iter()
        .filter_map(|id| {
            let queue = cook.compound_que_bin_map.get(&id)?;
            let output_count = queue
                .output_list
                .iter()
                .filter(|o| o.output_time <= timestamp)
                .count() as u32;
            Some(CompoundQueueData {
                compound_id: id,
                output_count,
                wait_count: queue.output_list.len() as u32 - output_count,
                output_time: queue
                    .output_list
                    .iter()
                    .filter(|o| o.output_time > timestamp)
                    .map(|o| o.output_time)
                    .min()
                    .unwrap_or_default(),
            })
        })
        .collect()
}
fn compound_notify(player: &PlayerDataBin, output: &MessageOutput) {
    output.send(
        player.uid,
        "CompoundDataNotify",
        CompoundDataNotify {
            unlock_compound_list: ids(&ProductionConfig::get().compound),
            compound_que_data_list: compound_data(player, now()),
        },
    );
}

pub fn login(mut players: ResMut<Players>, output: Res<MessageOutput>) {
    let config = ProductionConfig::get();
    for player in players.values_mut() {
        player
            .combine_bin
            .get_or_insert_with(Default::default)
            .unlock_combine_id_list = ids(&config.combine);
        player
            .forge_bin
            .get_or_insert_with(Default::default)
            .unlock_forge_id_list = ids(&config.forge);
        let cook = player.cook_bin.get_or_insert_with(Default::default);
        cook.unlock_compound_list = ids(&config.compound);
        for recipe in config.cook.values().filter(|r| r.is_default_unlocked) {
            cook.recipe_data_map.entry(recipe.id).or_default();
        }
        refresh_forge(player, now());
        output.send(
            player.uid,
            "CombineDataNotify",
            CombineDataNotify {
                combine_id_list: ids(&config.combine),
            },
        );
        output.send(
            player.uid,
            "ForgeDataNotify",
            ForgeDataNotify {
                forge_id_list: ids(&config.forge),
                forge_queue_map: forge_data(player, now()),
                max_queue_num: forge_limit(player),
            },
        );
        let cook = player.cook_bin.as_ref().unwrap();
        output.send(
            player.uid,
            "CookDataNotify",
            CookDataNotify {
                grade: cook.grade,
                recipe_data_list: ids(&cook.recipe_data_map)
                    .into_iter()
                    .map(|id| CookRecipeData {
                        recipe_id: id,
                        proficiency: cook.recipe_data_map[&id].proficiency,
                    })
                    .collect(),
            },
        );
        compound_notify(player, &output);
        forge_notify(player, &output, vec![]);
    }
}

pub fn handle(
    mut messages: MessageReader<ClientMessageEvent>,
    mut players: ResMut<Players>,
    output: Res<MessageOutput>,
) {
    for message in messages.read() {
        let uid = message.sender_uid();
        let Some(player) = players.get_mut(uid) else {
            continue;
        };
        macro_rules! respond {
            ($request:ty, $response:ident, $name:literal, $body:expr) => {
                if let Some(request) = message.decode::<$request>() {
                    let result: InventoryResult<$response> = ($body)(player, &request, &output);
                    let response = match result {
                        Ok(response) => response,
                        Err(code) => $response {
                            retcode: code.into(),
                            ..Default::default()
                        },
                    };
                    output.send(uid, $name, response);
                }
            };
        }
        match message.message_name() {
            "CombineReq" => respond!(CombineReq, CombineRsp, "CombineRsp", combine),
            "ForgeGetQueueDataReq" => {
                refresh_forge(player, now());
                output.send(
                    uid,
                    "ForgeGetQueueDataRsp",
                    ForgeGetQueueDataRsp {
                        forge_queue_map: forge_data(player, now()),
                        max_queue_num: forge_limit(player),
                        ..Default::default()
                    },
                );
            }
            "ForgeStartReq" => respond!(ForgeStartReq, ForgeStartRsp, "ForgeStartRsp", forge_start),
            "ForgeQueueManipulateReq" => respond!(
                ForgeQueueManipulateReq,
                ForgeQueueManipulateRsp,
                "ForgeQueueManipulateRsp",
                forge_manipulate
            ),
            "TakeAllForgeOutputReq" => {
                let response = match forge_take_all(player, &output) {
                    Ok(response) => response,
                    Err(code) => TakeAllForgeOutputRsp {
                        retcode: code.into(),
                        ..Default::default()
                    },
                };
                output.send(uid, "TakeAllForgeOutputRsp", response);
            }
            "PlayerCookArgsReq" => respond!(
                PlayerCookArgsReq,
                PlayerCookArgsRsp,
                "PlayerCookArgsRsp",
                cook_args
            ),
            "PlayerCookReq" => respond!(PlayerCookReq, PlayerCookRsp, "PlayerCookRsp", cook),
            "UseItemReq" => respond!(UseItemReq, UseItemRsp, "UseItemRsp", use_recipe),
            "GetCompoundDataReq" => output.send(
                uid,
                "GetCompoundDataRsp",
                GetCompoundDataRsp {
                    unlock_compound_list: ids(&ProductionConfig::get().compound),
                    compound_que_data_list: compound_data(player, now()),
                    ..Default::default()
                },
            ),
            "PlayerCompoundMaterialReq" => respond!(
                PlayerCompoundMaterialReq,
                PlayerCompoundMaterialRsp,
                "PlayerCompoundMaterialRsp",
                compound_start
            ),
            "TakeCompoundOutputReq" => respond!(
                TakeCompoundOutputReq,
                TakeCompoundOutputRsp,
                "TakeCompoundOutputRsp",
                compound_take
            ),
            _ => (),
        }
    }
}

fn use_recipe(
    player: &mut PlayerDataBin,
    request: &UseItemReq,
    output: &MessageOutput,
) -> InventoryResult<UseItemRsp> {
    use snezhnaya_gi_data::excel::{material_excel_config_collection, ItemUseOp};
    if request.count != 1 {
        return Err(Retcode::RetItemInvalidUseCount);
    }
    let item = player
        .item_bin
        .as_ref()
        .and_then(|b| b.get_item(&request.guid))
        .ok_or(Retcode::RetItemNotExist)?;
    let item_id = item.item_id;
    let config = material_excel_config_collection::get()
        .get(&item_id)
        .ok_or(Retcode::RetNotFoundConfig)?;
    let op = config
        .item_use
        .iter()
        .find(|op| {
            matches!(
                op.use_op,
                ItemUseOp::UnlockCookRecipe | ItemUseOp::UnlockCombine | ItemUseOp::UnlockForge
            )
        })
        .ok_or(Retcode::RetItemNotUsable)?;
    let id = op
        .use_param
        .first()
        .and_then(|s| s.as_str().parse::<u32>().ok())
        .ok_or(Retcode::RetNotFoundConfig)?;
    let recipes = ProductionConfig::get();
    match op.use_op {
        ItemUseOp::UnlockCookRecipe => {
            if !recipes.cook.contains_key(&id) {
                return Err(Retcode::RetRecipeNotExist);
            }
            if player
                .cook_bin
                .as_ref()
                .is_some_and(|b| b.recipe_data_map.contains_key(&id))
            {
                return Err(Retcode::RetRecipeUnlocked);
            }
        }
        ItemUseOp::UnlockForge if !recipes.forge.contains_key(&id) => {
            return Err(Retcode::RetNotFoundConfig)
        }
        ItemUseOp::UnlockCombine if !recipes.combine.contains_key(&id) => {
            return Err(Retcode::RetNotFoundConfig)
        }
        ItemUseOp::UnlockForge | ItemUseOp::UnlockCombine => {
            return Err(Retcode::RetRecipeUnlocked)
        }
        _ => return Err(Retcode::RetItemNotUsable),
    }
    execute(
        player,
        &Amounts::from([(item_id, 1)]),
        &Amounts::new(),
        output,
    )?;
    player
        .cook_bin
        .get_or_insert_with(Default::default)
        .recipe_data_map
        .entry(id)
        .or_default();
    output.send(
        player.uid,
        "CookRecipeDataNotify",
        CookRecipeDataNotify {
            recipe_data: Some(CookRecipeData {
                recipe_id: id,
                proficiency: 0,
            }),
        },
    );
    Ok(UseItemRsp {
        guid: request.guid,
        item_id,
        target_guid: request.target_guid,
        ..Default::default()
    })
}

fn combine(
    player: &mut PlayerDataBin,
    request: &CombineReq,
    output: &MessageOutput,
) -> InventoryResult<CombineRsp> {
    count_valid(request.combine_count)?;
    let recipe = ProductionConfig::get()
        .combine
        .get(&request.combine_id)
        .ok_or(Retcode::RetNotFoundConfig)?;
    if player.basic_bin.as_ref().map_or(1, |b| b.level) < recipe.player_level {
        return Err(Retcode::RetPlayerLevelLessThan);
    }
    if request.avatar_guid != 0
        && !player
            .avatar_bin
            .as_ref()
            .is_some_and(|b| b.avatar_map.contains_key(&request.avatar_guid))
    {
        return Err(Retcode::RetCanNotFindAvatar);
    }
    let costs = costs(
        &recipe.material_items,
        recipe.scoin_cost,
        request.combine_count,
    )?;
    let rewards = scale_items(
        &[ItemAmount {
            id: recipe.result_item_id,
            count: recipe.result_item_count,
        }],
        request.combine_count,
    )?;
    if rewards.is_empty() {
        return Err(Retcode::RetNotFoundConfig);
    }
    execute(player, &costs, &rewards, output)?;
    // 客户端会再乘合成次数，这里只填单次产出。
    Ok(CombineRsp {
        combine_id: request.combine_id,
        combine_count: request.combine_count,
        avatar_guid: request.avatar_guid,
        result_item_list: vec![ItemParam {
            item_id: recipe.result_item_id,
            count: recipe.result_item_count,
        }],
        cost_item_list: params(&costs),
        ..Default::default()
    })
}

fn forge_start(
    player: &mut PlayerDataBin,
    request: &ForgeStartReq,
    output: &MessageOutput,
) -> InventoryResult<ForgeStartRsp> {
    count_valid(request.forge_count)?;
    let config = ProductionConfig::get()
        .forge
        .get(&request.forge_id)
        .ok_or(Retcode::RetNotFoundConfig)?;
    if player.basic_bin.as_ref().map_or(1, |b| b.level) < config.player_level {
        return Err(Retcode::RetPlayerLevelLessThan);
    }
    if request.forge_count > config.queue_num || config.forge_time == 0 {
        return Err(Retcode::RetForgeQueueCapacity);
    }
    if request.avatar_id != 0
        && !player.avatar_bin.as_ref().is_some_and(|b| {
            b.avatar_map
                .values()
                .any(|a| a.avatar_id == request.avatar_id)
        })
    {
        return Err(Retcode::RetCanNotFindAvatar);
    }
    let timestamp = now();
    refresh_forge(player, timestamp);
    let limit = forge_limit(player);
    let forge = player.forge_bin.as_ref().unwrap();
    let queue_id = (1..=limit)
        .find(|id| !forge.forge_queue_map.contains_key(id))
        .ok_or(Retcode::RetForgeQueueFull)?;
    let points = config
        .forge_point
        .checked_mul(request.forge_count)
        .ok_or(Retcode::RetForgePointNotEnough)?;
    if forge.forge_point < points {
        return Err(Retcode::RetForgePointNotEnough);
    }
    timestamp
        .checked_add(
            config
                .forge_time
                .checked_mul(request.forge_count)
                .ok_or(Retcode::RetForgeQueueCapacity)?,
        )
        .ok_or(Retcode::RetForgeQueueCapacity)?;
    let costs = costs(
        &config.material_items,
        config.scoin_cost,
        request.forge_count,
    )?;
    execute(player, &costs, &Amounts::new(), output)?;
    let forge = player.forge_bin.as_mut().unwrap();
    forge.forge_point -= points;
    forge.forge_queue_map.insert(
        queue_id,
        ForgeQueueBin {
            queue_id,
            forge_id: request.forge_id,
            avatar_id: request.avatar_id,
            forge_count: request.forge_count,
            start_time: timestamp,
            single_time_cost: config.forge_time,
            ..Default::default()
        },
    );
    forge_notify(player, output, vec![]);
    Ok(ForgeStartRsp::default())
}

fn forge_manipulate(
    player: &mut PlayerDataBin,
    request: &ForgeQueueManipulateReq,
    output: &MessageOutput,
) -> InventoryResult<ForgeQueueManipulateRsp> {
    let queue = player
        .forge_bin
        .as_ref()
        .and_then(|b| b.forge_queue_map.get(&request.forge_queue_id))
        .cloned()
        .ok_or(Retcode::RetForgeQueueNotFound)?;
    let config = ProductionConfig::get()
        .forge
        .get(&queue.forge_id)
        .ok_or(Retcode::RetNotFoundConfig)?;
    let completed = finished(&queue, now());
    let ready = completed.saturating_sub(queue.taken_count);
    let mut response = ForgeQueueManipulateRsp {
        manipulate_type: request.manipulate_type,
        ..Default::default()
    };
    let mut removed = vec![];
    match request.manipulate_type {
        0 => {
            if ready == 0 {
                return Err(Retcode::RetForgeQueueEmpty);
            }
            let rewards = scale_items(
                &[ItemAmount {
                    id: config.result_item_id,
                    count: config.result_item_count,
                }],
                ready,
            )?;
            execute(player, &Amounts::new(), &rewards, output)?;
            let forge = player.forge_bin.as_mut().unwrap();
            if completed == queue.forge_count {
                forge.forge_queue_map.remove(&request.forge_queue_id);
                removed.push(request.forge_queue_id);
            } else {
                forge
                    .forge_queue_map
                    .get_mut(&request.forge_queue_id)
                    .unwrap()
                    .taken_count = completed;
            }
            response.output_item_list = hints(&rewards);
        }
        1 => {
            if ready != 0 {
                return Err(Retcode::RetForgeQueueFull);
            }
            let unfinished = queue.forge_count - completed;
            let refund = costs(&config.material_items, config.scoin_cost, unfinished)?;
            execute(player, &Amounts::new(), &refund, output)?;
            let forge = player.forge_bin.as_mut().unwrap();
            // 跨日取消不能把昨天的额度退到今天。
            if (queue.start_time as u64 + 14_400) / 86_400 == (now() as u64 + 14_400) / 86_400 {
                forge.forge_point = forge
                    .forge_point
                    .saturating_add(config.forge_point.saturating_mul(unfinished))
                    .min(DAILY_FORGE_POINTS);
            }
            forge.forge_queue_map.remove(&request.forge_queue_id);
            removed.push(request.forge_queue_id);
            response.return_item_list = hints(&refund);
        }
        _ => return Err(Retcode::RetFail),
    }
    forge_notify(player, output, removed);
    Ok(response)
}

fn forge_take_all(
    player: &mut PlayerDataBin,
    output: &MessageOutput,
) -> InventoryResult<TakeAllForgeOutputRsp> {
    let timestamp = now();
    let mut rewards = Amounts::new();
    let mut updates = Vec::new();
    for (&id, queue) in player
        .forge_bin
        .iter()
        .flat_map(|b| b.forge_queue_map.iter())
    {
        let completed = finished(queue, timestamp);
        let ready = completed.saturating_sub(queue.taken_count);
        if ready == 0 {
            continue;
        }
        let config = ProductionConfig::get()
            .forge
            .get(&queue.forge_id)
            .ok_or(Retcode::RetNotFoundConfig)?;
        add_amount(
            &mut rewards,
            config.result_item_id,
            config
                .result_item_count
                .checked_mul(ready)
                .ok_or(Retcode::RetItemExceedLimit)?,
        )?;
        updates.push((id, completed, completed == queue.forge_count));
    }
    if updates.is_empty() {
        return Err(Retcode::RetForgeQueueEmpty);
    }
    execute(player, &Amounts::new(), &rewards, output)?;
    let forge = player.forge_bin.as_mut().unwrap();
    let mut removed = vec![];
    for (id, completed, done) in updates {
        if done {
            forge.forge_queue_map.remove(&id);
            removed.push(id);
        } else {
            forge.forge_queue_map.get_mut(&id).unwrap().taken_count = completed;
        }
    }
    forge_notify(player, output, removed);
    Ok(TakeAllForgeOutputRsp {
        output_item_list: hints(&rewards),
        ..Default::default()
    })
}

fn cook_args(
    player: &mut PlayerDataBin,
    request: &PlayerCookArgsReq,
    _: &MessageOutput,
) -> InventoryResult<PlayerCookArgsRsp> {
    if !ProductionConfig::get()
        .cook
        .contains_key(&request.recipe_id)
    {
        return Err(Retcode::RetRecipeNotExist);
    }
    if !player
        .cook_bin
        .as_ref()
        .is_some_and(|b| b.recipe_data_map.contains_key(&request.recipe_id))
    {
        return Err(Retcode::RetRecipeLocked);
    }
    Ok(PlayerCookArgsRsp {
        qte_range_ratio: 1.0,
        ..Default::default()
    })
}

fn cook(
    player: &mut PlayerDataBin,
    request: &PlayerCookReq,
    output: &MessageOutput,
) -> InventoryResult<PlayerCookRsp> {
    count_valid(request.cook_count)?;
    let recipe = ProductionConfig::get()
        .cook
        .get(&request.recipe_id)
        .ok_or(Retcode::RetRecipeNotExist)?;
    let proficiency = player
        .cook_bin
        .as_ref()
        .and_then(|b| b.recipe_data_map.get(&request.recipe_id))
        .ok_or(Retcode::RetRecipeLocked)?
        .proficiency;
    if request.assist_avatar != 0
        && !player.avatar_bin.as_ref().is_some_and(|b| {
            b.avatar_map
                .values()
                .any(|a| a.avatar_id == request.assist_avatar)
        })
    {
        return Err(Retcode::RetCanNotFindAvatar);
    }
    let quality = if request.qte_quality == 0 {
        3
    } else {
        request.qte_quality
    };
    if !(1..=3).contains(&quality) {
        return Err(Retcode::RetFail);
    }
    let result = recipe
        .quality_output_vec
        .get(quality as usize - 1)
        .ok_or(Retcode::RetNotFoundConfig)?;
    let rewards = scale_items(&[*result], request.cook_count)?;
    execute(
        player,
        &scale_items(&recipe.input_vec, request.cook_count)?,
        &rewards,
        output,
    )?;
    let proficiency = if quality == 3 {
        proficiency
            .saturating_add(request.cook_count)
            .min(recipe.max_proficiency)
    } else {
        proficiency
    };
    player
        .cook_bin
        .as_mut()
        .unwrap()
        .recipe_data_map
        .get_mut(&request.recipe_id)
        .unwrap()
        .proficiency = proficiency;
    Ok(PlayerCookRsp {
        recipe_data: Some(CookRecipeData {
            recipe_id: request.recipe_id,
            proficiency,
        }),
        item_list: params(&rewards),
        qte_quality: quality,
        cook_count: request.cook_count,
        ..Default::default()
    })
}

fn compound_start(
    player: &mut PlayerDataBin,
    request: &PlayerCompoundMaterialReq,
    output: &MessageOutput,
) -> InventoryResult<PlayerCompoundMaterialRsp> {
    count_valid(request.count)?;
    let config = ProductionConfig::get()
        .compound
        .get(&request.compound_id)
        .ok_or(Retcode::RetNotFoundConfig)?;
    if config.cost_time == 0 {
        return Err(Retcode::RetNotFoundConfig);
    }
    let mut queue = player
        .cook_bin
        .as_ref()
        .and_then(|b| b.compound_que_bin_map.get(&request.compound_id))
        .cloned()
        .unwrap_or_default();
    if queue
        .output_list
        .len()
        .saturating_add(request.count as usize)
        > config.queue_size as usize
    {
        return Err(Retcode::RetCompoundQueueFull);
    }
    let mut timestamp = queue
        .output_list
        .iter()
        .map(|o| o.output_time)
        .max()
        .unwrap_or_default()
        .max(now());
    for _ in 0..request.count {
        timestamp = timestamp
            .checked_add(config.cost_time)
            .ok_or(Retcode::RetCompoundQueueFull)?;
        queue.output_list.push(CompoundOutputBin {
            output_time: timestamp,
        });
    }
    execute(
        player,
        &scale_items(&config.input_vec, request.count)?,
        &Amounts::new(),
        output,
    )?;
    player
        .cook_bin
        .get_or_insert_with(Default::default)
        .compound_que_bin_map
        .insert(request.compound_id, queue);
    Ok(PlayerCompoundMaterialRsp {
        compound_que_data: compound_data(player, now())
            .into_iter()
            .find(|q| q.compound_id == request.compound_id),
        ..Default::default()
    })
}

fn compound_take(
    player: &mut PlayerDataBin,
    request: &TakeCompoundOutputReq,
    output: &MessageOutput,
) -> InventoryResult<TakeCompoundOutputRsp> {
    if request.compound_id == 0 && request.compound_group_id == 0 && !request.is_claim_all {
        return Err(Retcode::RetFail);
    }
    let timestamp = now();
    let mut rewards = Amounts::new();
    let mut updates = vec![];
    for (&id, queue) in player
        .cook_bin
        .iter()
        .flat_map(|b| b.compound_que_bin_map.iter())
    {
        let config = ProductionConfig::get()
            .compound
            .get(&id)
            .ok_or(Retcode::RetNotFoundConfig)?;
        if (request.compound_id != 0 && request.compound_id != id)
            || (request.compound_id == 0
                && request.compound_group_id != 0
                && request.compound_group_id != config.group_id)
        {
            continue;
        }
        let ready = queue
            .output_list
            .iter()
            .filter(|o| o.output_time <= timestamp)
            .count() as u32;
        if ready == 0 {
            continue;
        }
        for (id, count) in scale_items(&config.output_vec, ready)? {
            add_amount(&mut rewards, id, count)?;
        }
        updates.push(id);
    }
    if rewards.is_empty() {
        return Err(Retcode::RetCompoundNotFinish);
    }
    execute(player, &Amounts::new(), &rewards, output)?;
    let cook = player.cook_bin.as_mut().unwrap();
    for id in updates {
        let queue = cook.compound_que_bin_map.get_mut(&id).unwrap();
        queue.output_list.retain(|o| o.output_time > timestamp);
        if queue.output_list.is_empty() {
            cook.compound_que_bin_map.remove(&id);
        }
    }
    compound_notify(player, output);
    Ok(TakeCompoundOutputRsp {
        item_list: params(&rewards),
        ..Default::default()
    })
}
