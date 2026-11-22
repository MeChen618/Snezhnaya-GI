use crate::{production, transaction::*};
use bevy_ecs::prelude::*;
use common::time_util::unix_timestamp;
use snezhnaya_gi_data::excel::{
    common::ItemType, material_excel_config_collection, production::ProductionConfig, ItemUseOp,
    ItemUseTarget,
};
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::{normal::*, retcode::Retcode, server_only::*};
use std::time::{Duration, Instant};

#[derive(Resource, Default)]
pub(super) struct ResinWakeup(Option<Instant>);

pub fn next_wakeup(world: &World) -> Option<Instant> {
    world.get_resource::<ResinWakeup>()?.0
}

pub(super) fn timestamp() -> u32 {
    unix_timestamp().min(u64::from(u32::MAX)) as u32
}

fn day(timestamp: u32) -> u64 {
    (u64::from(timestamp) + 14_400) / 86_400
}

// 恢复时间随存档保存，离线期间也按同一进度结算。
pub(super) fn refresh_resin(items: &mut PlayerItemCompBin, now: u32) {
    items.resin_record = Some(recover_record(items.resin_record, now));
}

fn recover_record(record: Option<AutoRecoverItemBin>, now: u32) -> AutoRecoverItemBin {
    let config = &ProductionConfig::get().resin;
    let mut record = record.unwrap_or(AutoRecoverItemBin {
        value: config.recover_limit,
        ..Default::default()
    });
    if record.bought_num > 0 && day(now) > day(record.last_bought_time) {
        record.bought_num = 0;
    }
    if record.value >= config.recover_limit {
        record.next_refresh_time = 0;
    } else if record.next_refresh_time == 0 {
        record.next_refresh_time = now.saturating_add(config.recover_interval);
    } else if record.next_refresh_time <= now {
        let count = (now - record.next_refresh_time) / config.recover_interval + 1;
        record.value = record.value.saturating_add(count).min(config.recover_limit);
        record.next_refresh_time = if record.value >= config.recover_limit {
            0
        } else {
            record
                .next_refresh_time
                .saturating_add(count.saturating_mul(config.recover_interval))
        };
    }
    record
}

pub(super) fn notify_resin(player: &PlayerDataBin, output: &MessageOutput) {
    let Some(record) = player
        .item_bin
        .as_ref()
        .and_then(|i| i.resin_record.as_ref())
    else {
        return;
    };
    output.send(
        player.uid,
        "ResinChangeNotify",
        ResinChangeNotify {
            cur_value: record.value,
            cur_buy_count: record.bought_num,
            next_add_timestamp: record.next_refresh_time,
        },
    );
}

pub(super) fn login(mut players: ResMut<Players>, output: Res<MessageOutput>) {
    let now = timestamp();
    for player in players.values_mut() {
        if let Some(items) = player.item_bin.as_mut() {
            refresh_resin(items, now);
            notify_resin(player, &output);
        }
    }
}

pub(super) fn tick(
    mut players: ResMut<Players>,
    output: Res<MessageOutput>,
    mut wake: ResMut<ResinWakeup>,
) {
    let now = timestamp();
    let mut next = None;
    for player in players.values_mut() {
        let Some(items) = player.item_bin.as_mut() else {
            continue;
        };
        let before = items.resin_record;
        refresh_resin(items, now);
        let record = items.resin_record.as_ref().unwrap();
        for deadline in [
            (record.next_refresh_time > 0).then_some(u64::from(record.next_refresh_time)),
            (record.bought_num > 0).then(|| (day(record.last_bought_time) + 1) * 86_400 - 14_400),
        ]
        .into_iter()
        .flatten()
        {
            next = Some(next.map_or(deadline, |value: u64| value.min(deadline)));
        }
        if items.resin_record != before {
            notify_resin(player, &output);
            output.send(
                player.uid,
                "PlayerPropNotify",
                PlayerPropNotify {
                    prop_map: currency_properties(player.item_bin.as_ref().unwrap()),
                },
            );
        }
    }
    wake.0 = next.map(|deadline| {
        Instant::now() + Duration::from_secs(deadline.saturating_sub(u64::from(now)))
    });
}

pub(super) fn handle(
    mut events: MessageReader<ClientMessageEvent>,
    mut players: ResMut<Players>,
    output: Res<MessageOutput>,
) {
    for message in events.read() {
        let Some(player) = players.get_mut(message.sender_uid()) else {
            continue;
        };
        match message.message_name() {
            "McoinExchangeHcoinReq" => {
                let Some(req) = message.decode::<McoinExchangeHcoinReq>() else {
                    continue;
                };
                let result = exchange(player, &req, &output);
                output.send(
                    player.uid,
                    "McoinExchangeHcoinRsp",
                    McoinExchangeHcoinRsp {
                        retcode: result.err().unwrap_or(Retcode::RetSucc).into(),
                        hcoin: req.hcoin,
                        mcoin_cost: req.mcoin_cost,
                    },
                );
            }
            "BuyResinReq" => {
                let result = buy_resin(player, &output);
                output.send(
                    player.uid,
                    "BuyResinRsp",
                    BuyResinRsp {
                        retcode: result.err().unwrap_or(Retcode::RetSucc).into(),
                        cur_value: player
                            .item_bin
                            .as_ref()
                            .map_or(0, |items| item_count(items, 106)),
                    },
                );
            }
            "UseItemReq" => {
                let Some(req) = message.decode::<UseItemReq>() else {
                    continue;
                };
                let response = use_item(player, &req, &output);
                output.send(
                    player.uid,
                    "UseItemRsp",
                    response.unwrap_or_else(|code| UseItemRsp {
                        retcode: code.into(),
                        guid: req.guid,
                        target_guid: req.target_guid,
                        option_idx: req.option_idx,
                        ..Default::default()
                    }),
                );
            }
            _ => (),
        }
    }
}

fn exchange(
    player: &mut PlayerDataBin,
    req: &McoinExchangeHcoinReq,
    output: &MessageOutput,
) -> InventoryResult<()> {
    if req.hcoin == 0 || req.hcoin != req.mcoin_cost {
        return Err(Retcode::RetItemInvalidUseCount);
    }
    let change = transact(
        player,
        &Amounts::from([(203, req.mcoin_cost)]),
        &Amounts::from([(201, req.hcoin)]),
        &[],
    )?;
    notify_transaction(output, player, &change);
    Ok(())
}

fn buy_resin(player: &mut PlayerDataBin, output: &MessageOutput) -> InventoryResult<()> {
    let now = timestamp();
    let items = player.item_bin.as_ref().ok_or(Retcode::RetItemNotExist)?;
    let record = recover_record(items.resin_record, now);
    let config = &ProductionConfig::get().resin;
    let price = *config
        .buy_costs
        .get(record.bought_num as usize)
        .ok_or(Retcode::RetResinBoughtCountExceeded)?;
    // 报价和扣款使用同一时刻，跨四点时不会套用两天的价格。
    let change = transact_at(
        player,
        &Amounts::from([(201, price)]),
        &Amounts::from([(106, config.buy_value)]),
        now,
    )?;
    let record = player
        .item_bin
        .as_mut()
        .unwrap()
        .resin_record
        .as_mut()
        .unwrap();
    record.bought_num += 1;
    record.last_bought_time = now;
    notify_transaction(output, player, &change);
    Ok(())
}

fn use_item(
    player: &mut PlayerDataBin,
    req: &UseItemReq,
    output: &MessageOutput,
) -> InventoryResult<UseItemRsp> {
    if req.count == 0 {
        return Err(Retcode::RetItemInvalidUseCount);
    }
    if req.option_idx != 0
        || !req.option_idx_list.is_empty()
        || req.target_guid != 0
        || req.is_enter_mp_dungeon_team
    {
        return Err(Retcode::RetItemInvalidTarget);
    }
    let item = player
        .item_bin
        .as_ref()
        .and_then(|i| i.get_item(&req.guid))
        .ok_or(Retcode::RetItemNotExist)?;
    let id = item.item_id;
    let Some(item_bin::Detail::Material(material)) = &item.detail else {
        return Err(Retcode::RetItemNotUsable);
    };
    if material.count < req.count {
        return Err(Retcode::RetItemCountNotEnough);
    }
    let config = material_excel_config_collection::get()
        .get(&id)
        .ok_or(Retcode::RetNotFoundConfig)?;
    if config.item_type != ItemType::MATERIAL
        || config.use_on_gain
        || config.use_target != ItemUseTarget::None
    {
        return Err(Retcode::RetItemNotUsable);
    }
    if req.count > config.max_use_count.max(1) {
        return Err(Retcode::RetItemInvalidUseCount);
    }
    let operations: Vec<_> = config
        .item_use
        .iter()
        .filter(|op| op.use_op != ItemUseOp::None)
        .collect();
    if operations.len() == 1
        && matches!(
            operations[0].use_op,
            ItemUseOp::UnlockCookRecipe | ItemUseOp::UnlockCombine | ItemUseOp::UnlockForge
        )
    {
        return production::use_recipe(player, req, output);
    }
    if operations.is_empty() || operations.iter().any(|op| op.use_op != ItemUseOp::AddItem) {
        return Err(Retcode::RetItemNotUsable);
    }
    let mut rewards = Amounts::new();
    for op in operations {
        let parse = |index: usize| {
            op.use_param
                .get(index)
                .and_then(|s| s.as_str().parse::<u32>().ok())
                .filter(|v| *v > 0)
                .ok_or(Retcode::RetNotFoundConfig)
        };
        add_amount(
            &mut rewards,
            parse(0)?,
            parse(1)?
                .checked_mul(req.count)
                .ok_or(Retcode::RetItemExceedLimit)?,
        )?;
    }
    let change = consume_item(player, req.guid, req.count, &rewards)?;
    notify_transaction(output, player, &change);
    Ok(UseItemRsp {
        guid: req.guid,
        target_guid: req.target_guid,
        item_id: id,
        ..Default::default()
    })
}
