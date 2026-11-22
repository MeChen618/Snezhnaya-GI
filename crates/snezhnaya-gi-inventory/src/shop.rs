use crate::transaction::*;
use bevy_ecs::prelude::*;
use common::time_util::unix_timestamp;
use snezhnaya_gi_data::excel::{self, common::ItemType, shop::*};
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::{
    normal::*,
    retcode::Retcode,
    server_only::{PlayerDataBin, ShopGoodsRecordBin, ShopRecordBin},
};

fn shop(player: &PlayerDataBin, id: u32, now: u32) -> InventoryResult<&'static ShopDefinition> {
    let row = ShopConfig::get()
        .shops
        .get(&id)
        .ok_or(Retcode::RetShopNotOpen)?;
    let closed = shop_open_state(&row.open_state_type).is_some_and(|state| {
        player
            .basic_bin
            .as_ref()
            .and_then(|b| b.open_state_map.get(&state))
            == Some(&0)
    });
    if id >= 100000 || closed || !in_time(&row.begin_time, &row.end_time, now) {
        return Err(Retcode::RetShopNotOpen);
    }
    Ok(row)
}
fn precondition(player: &PlayerDataBin, goods: &Goods) -> bool {
    match goods.precondition.as_str() {
        "" => true,
        "SHOP_PRECONDITION_QUEST_FINISH" | "SHOP_PRECONDITION_QUEST_FINISH_ANY" => {
            let ids = goods
                .precondition_param_list
                .iter()
                .flat_map(|s| s.split([',', ';', '|']))
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().parse::<u32>())
                .collect::<Result<Vec<_>, _>>();
            let Ok(ids) = ids else {
                return false;
            };
            !ids.is_empty()
                && ids.iter().any(|id| {
                    player
                        .quest_bin
                        .as_ref()
                        .and_then(|b| b.quest_bin.as_ref())
                        .and_then(|b| b.quest_map.get(id))
                        .is_some_and(|q| q.state == 3)
                })
        }
        // 家园、活动和七圣召唤的条件要由各自系统提供。
        _ => false,
    }
}
fn supported_item(id: u32) -> bool {
    excel::material_excel_config_collection::get()
        .get(&id)
        .is_some_and(|m| m.item_type == ItemType::MATERIAL && !m.use_on_gain)
        || excel::weapon_excel_config_collection::get().contains_key(&id)
        || excel::reliquary_excel_config_collection::get().contains_key(&id)
}
fn available(player: &PlayerDataBin, goods: &Goods, now: u32) -> bool {
    goods.goods_type == 1
        && goods.item_count > 0
        && goods.choose_one_group_id == 0
        && in_time(&goods.begin_time, &goods.end_time, now)
        && precondition(player, goods)
        && ShopConfig::get()
            .item_id(goods, now)
            .is_some_and(supported_item)
}
fn limit(goods: &Goods) -> u32 {
    if goods.is_buy_once {
        goods.buy_limit.max(1)
    } else {
        goods.buy_limit
    }
}
fn record<'a>(player: &'a PlayerDataBin, goods: &Goods) -> Option<&'a ShopGoodsRecordBin> {
    let shop = player
        .shop_bin
        .as_ref()?
        .shop_record_list
        .iter()
        .find(|s| s.shop_type == goods.shop_type)?;
    if goods.is_buy_once {
        if let Some(r) = shop
            .history_goods_record_list
            .iter()
            .find(|r| r.goods_id == goods.goods_id)
        {
            return Some(r);
        }
    }
    shop.goods_record_list
        .iter()
        .find(|r| r.goods_id == goods.goods_id)
}
fn counts(player: &PlayerDataBin, goods: &Goods, now: u32) -> (u32, u32, u32) {
    if let Some(r) = record(player, goods) {
        if goods.is_buy_once {
            return (r.bought_num, 0, r.last_buy_time);
        }
        let next =
            next_refresh(&goods.refresh_type, goods.refresh_param, r.last_buy_time).unwrap_or(0);
        if next == 0 || now < next {
            return (r.bought_num, next, r.last_buy_time);
        }
    }
    (
        0,
        if goods.is_buy_once {
            0
        } else {
            next_refresh(&goods.refresh_type, goods.refresh_param, now).unwrap_or(0)
        },
        now,
    )
}
fn price(player: &PlayerDataBin, row: &ShopDefinition, goods: &Goods) -> InventoryResult<Amounts> {
    let reputation = player
        .reputation_bin
        .as_ref()
        .and_then(|b| {
            b.city_reputation_list
                .iter()
                .find(|r| r.city_id == row.city_id)
        })
        .map_or(0, |r| r.level);
    let discount = row.city_discount_level > 0
        && reputation >= row.city_discount_level
        && (1..=100).contains(&row.scoin_discount_rate);
    let mora = if discount {
        (u64::from(goods.cost_scoin) * u64::from(row.scoin_discount_rate) / 100) as u32
    } else {
        goods.cost_scoin
    };
    // costItems 已是实际售价，不再乘 discountRate。
    let mut costs = scale_items(&goods.cost_items, 1)?;
    for (id, count) in [
        (201, goods.cost_hcoin),
        (202, mora),
        (203, goods.cost_mcoin),
    ] {
        add_amount(&mut costs, id, count)?;
    }
    Ok(costs)
}
fn packet(
    player: &PlayerDataBin,
    row: &ShopDefinition,
    goods: &Goods,
    now: u32,
) -> InventoryResult<ShopGoods> {
    let costs = price(player, row, goods)?;
    Ok(ShopGoods {
        goods_id: goods.goods_id,
        goods_item: Some(ItemParam {
            item_id: ShopConfig::get()
                .item_id(goods, now)
                .ok_or(Retcode::RetGoodsNotExist)?,
            count: goods.item_count,
        }),
        cost_item_list: costs
            .iter()
            .filter(|(id, _)| !matches!(**id, 201..=203))
            .map(|(&item_id, &count)| ItemParam { item_id, count })
            .collect(),
        hcoin: costs.get(&201).copied().unwrap_or(0),
        scoin: costs.get(&202).copied().unwrap_or(0),
        mcoin: costs.get(&203).copied().unwrap_or(0),
        buy_limit: limit(goods),
        bought_num: counts(player, goods, now).0,
        next_refresh_time: counts(player, goods, now).1,
        min_level: goods.min_player_level,
        max_level: goods.max_player_level,
        begin_time: excel::progression::cn_timestamp(&goods.begin_time).unwrap_or(0),
        end_time: excel::progression::cn_timestamp(&goods.end_time).unwrap_or(0),
        ..Default::default()
    })
}
fn goods_list(player: &PlayerDataBin, row: &ShopDefinition, id: u32, now: u32) -> Vec<ShopGoods> {
    let level = player.basic_bin.as_ref().map_or(1, |b| b.level);
    let mut rows: Vec<_> = ShopConfig::get()
        .goods_for_shop(id)
        .filter(|g| {
            g.shop_type == id
                && level >= g.min_show_level.max(g.min_player_level)
                && available(player, g, now)
        })
        .collect();
    rows.sort_by_key(|g| (g.sort_level, g.goods_id));
    rows.into_iter()
        .filter_map(|g| packet(player, row, g, now).ok())
        .collect()
}
fn buy(
    player: &mut PlayerDataBin,
    req: &BuyGoodsReq,
    now: u32,
    output: &MessageOutput,
) -> InventoryResult<ShopGoods> {
    let row = shop(player, req.shop_type, now)?;
    let supplied = req.goods.as_ref().ok_or(Retcode::RetShopContentNotMatch)?;
    let goods = ShopConfig::get()
        .goods
        .get(&supplied.goods_id)
        .filter(|g| g.shop_type == req.shop_type)
        .ok_or(Retcode::RetGoodsNotExist)?;
    if req.buy_count == 0 || req.buy_count > 9999 {
        return Err(Retcode::RetGoodsBuyNumError);
    }
    if !in_time(&goods.begin_time, &goods.end_time, now) {
        return Err(Retcode::RetGoodsNotInTime);
    }
    if !available(player, goods, now) {
        return Err(Retcode::RetGoodsPreconditionNotSatisfied);
    }
    let level = player.basic_bin.as_ref().map_or(1, |b| b.level);
    if level < goods.min_player_level
        || (goods.max_player_level != 0 && level > goods.max_player_level)
    {
        return Err(Retcode::RetItemNeedPlayerLevel);
    }
    let item_id = ShopConfig::get()
        .item_id(goods, now)
        .ok_or(Retcode::RetGoodsNotExist)?;
    if supplied
        .goods_item
        .as_ref()
        .is_some_and(|i| i.item_id != item_id || i.count != goods.item_count)
    {
        return Err(Retcode::RetShopContentNotMatch);
    }
    let (bought, _, start) = counts(player, goods, now);
    let total = bought
        .checked_add(req.buy_count)
        .ok_or(Retcode::RetGoodsBuyNumError)?;
    if limit(goods) != 0 && total > limit(goods) {
        return Err(Retcode::RetGoodsBuyNumNotEnough);
    }
    let costs = price(player, row, goods)?
        .into_iter()
        .map(|(id, n)| {
            n.checked_mul(req.buy_count)
                .map(|n| (id, n))
                .ok_or(Retcode::RetGoodsBuyNumError)
        })
        .collect::<InventoryResult<Amounts>>()?;
    let rewards = Amounts::from([(
        item_id,
        goods
            .item_count
            .checked_mul(req.buy_count)
            .ok_or(Retcode::RetGoodsBuyNumError)?,
    )]);
    let change = transact(player, &costs, &rewards, &[])?;
    let shops = &mut player
        .shop_bin
        .get_or_insert_with(Default::default)
        .shop_record_list;
    let index = shops
        .iter()
        .position(|s| s.shop_type == req.shop_type)
        .unwrap_or_else(|| {
            shops.push(ShopRecordBin {
                shop_type: req.shop_type,
                ..Default::default()
            });
            shops.len() - 1
        });
    let records = if goods.is_buy_once {
        &mut shops[index].history_goods_record_list
    } else {
        &mut shops[index].goods_record_list
    };
    let saved = ShopGoodsRecordBin {
        goods_id: goods.goods_id,
        bought_num: total,
        last_buy_time: start,
    };
    if let Some(r) = records.iter_mut().find(|r| r.goods_id == goods.goods_id) {
        *r = saved;
    } else {
        records.push(saved);
    }
    notify_transaction(output, player, &change);
    packet(player, row, goods, now)
}

pub fn handle(
    mut events: MessageReader<ClientMessageEvent>,
    mut players: ResMut<Players>,
    output: Res<MessageOutput>,
) {
    let now = unix_timestamp() as u32;
    for message in events.read() {
        if !matches!(
            message.message_name(),
            "GetShopReq" | "BuyGoodsReq" | "GetShopmallDataReq"
        ) {
            continue;
        }
        let Some(player) = players.get_mut(message.sender_uid()) else {
            continue;
        };
        match message.message_name() {
            "GetShopReq" => {
                if let Some(req) = message.decode::<GetShopReq>() {
                    let result = shop(player, req.shop_type, now).map(|row| {
                        let goods_list = goods_list(player, row, req.shop_type, now);
                        let next = goods_list
                            .iter()
                            .map(|g| g.next_refresh_time)
                            .filter(|n| *n > now)
                            .chain(next_refresh(&row.refresh_type, row.refresh_param, now))
                            .min()
                            .unwrap_or(0);
                        Shop {
                            shop_type: req.shop_type,
                            goods_list,
                            next_refresh_time: next,
                            city_id: row.city_id,
                            ..Default::default()
                        }
                    });
                    output.send_json(player.uid, "GetShopRsp", match result {
                    Ok(shop) => serde_json::json!({"shop_type": req.shop_type, "shop": shop}),
                    Err(e) => serde_json::json!({"shop_type": req.shop_type, "retcode": i32::from(e)}),
                });
                }
            }
            "BuyGoodsReq" => {
                if let Some(req) = message.decode::<BuyGoodsReq>() {
                    let result = buy(player, &req, now, &output);
                    let (retcode, goods_list) = match result {
                        Ok(goods) => (0, vec![goods]),
                        Err(e) => (i32::from(e), vec![]),
                    };
                    output.send(
                        player.uid,
                        "BuyGoodsRsp",
                        BuyGoodsRsp {
                            shop_type: req.shop_type,
                            buy_count: req.buy_count,
                            retcode,
                            goods_list,
                            ..Default::default()
                        },
                    );
                }
            }
            "GetShopmallDataReq" => {
                if let Some(req) = message.decode_json() {
                    let shop_type_list = ShopConfig::get()
                        .entrances
                        .iter()
                        .copied()
                        .filter(|id| {
                            shop(player, *id, now)
                                .is_ok_and(|row| !goods_list(player, row, *id, now).is_empty())
                        })
                        .collect();
                    output.send(
                        player.uid,
                        "GetShopmallDataRsp",
                        GetShopmallDataRsp {
                            param: req
                                .get("param")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(0) as u32,
                            shop_type_list,
                            retcode: 0,
                        },
                    );
                }
            }
            _ => {}
        }
    }
}
