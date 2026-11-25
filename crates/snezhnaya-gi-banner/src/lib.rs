mod gacha;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use rand::{rngs::SmallRng, SeedableRng};
use snezhnaya_gi_data::{
    custom::{gacha_banner_collection, GachaBanner},
    excel,
};
use snezhnaya_gi_inventory::transaction::{self as inventory, Amounts, InventoryChange};
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::{record_progress, Players};
use snezhnaya_gi_proto::{
    normal::*,
    retcode::Retcode,
    server_only::{self as bin, avatar_bin},
};
use std::collections::HashSet;

pub struct BannerPlugin;
impl Plugin for BannerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, on_banner_notify);
    }
}

fn initialize(player: &mut bin::PlayerDataBin) {
    let state = player.gacha_bin.get_or_insert_with(Default::default);
    for (&id, _) in gacha_banner_collection::get().iter() {
        state.gacha_map.entry(id).or_default();
    }
    let day = ((common::time_util::unix_timestamp() + 4 * 3600) / 86400) as u32;
    for state in state.gacha_map.values_mut() {
        if state.last_day != day {
            state.last_day = day;
            state.daily_count = 0;
        }
        if state.item4_is_up_list.len() > 1 {
            state.item4_is_up_list = state.item4_is_up_list.last().copied().into_iter().collect();
        }
        if state.item5_is_up_list.len() > 1 {
            state.item5_is_up_list = state.item5_is_up_list.last().copied().into_iter().collect();
        }
    }
}

fn info(banner: &GachaBanner, state: &bin::GachaBin) -> GachaInfo {
    let limit = banner.gacha_times_limit.unwrap_or(u32::MAX);
    let now = common::time_util::unix_timestamp() as u32;
    GachaInfo {
        gacha_type: banner.gacha_type,
        schedule_id: banner.schedule_id,
        begin_time: now.saturating_sub(30 * 86400),
        end_time: now.saturating_add(30 * 86400),
        cost_item_id: banner.cost_item_id,
        cost_item_num: 1,
        ten_cost_item_id: banner.cost_item_id,
        ten_cost_item_num: banner.cost_item_amount_10.unwrap_or(10),
        gacha_prefab_path: banner.prefab_path.to_string(),
        gacha_preview_prefab_path: banner
            .preview_prefab_path
            .map(|p| p.to_string())
            .unwrap_or_else(|| format!("UI_Tab_{}", banner.prefab_path)),
        title_textmap: banner.title_path.to_string(),
        gacha_sort_id: banner.sort_id,
        gacha_times_limit: limit,
        left_gacha_times: limit.saturating_sub(state.total_count),
        cur_schedule_daily_gacha_times: state.daily_count,
        wish_item_id: state.wish_item_id,
        wish_progress: state.wish_progress,
        wish_max_progress: if matches!(banner.gacha_type, 302 | 500) {
            1
        } else {
            0
        },
        display_up5_item_list: banner.rate_up_items_5.clone(),
        display_up4_item_list: banner.rate_up_items_4.clone(),
        display_chronicle_5_item_list: if matches!(banner.gacha_type, 302 | 500) {
            banner.rate_up_items_5.clone()
        } else {
            vec![]
        },
        gacha_up_info_list: [(1, &banner.rate_up_items_5), (2, &banner.rate_up_items_4)]
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(kind, v)| GachaUpInfo {
                item_parent_type: kind,
                item_id_list: v.clone(),
            })
            .collect(),
        ..Default::default()
    }
}

fn on_banner_notify(
    mut events: MessageReader<ClientMessageEvent>,
    output: Res<MessageOutput>,
    mut players: ResMut<Players>,
) {
    for message in events.read() {
        let name = message.message_name();
        if !matches!(name, "GetGachaInfoReq" | "DoGachaReq" | "GachaWishReq") {
            continue;
        }
        let uid = message.sender_uid();
        let Some(player) = players.get_mut(uid) else {
            continue;
        };
        initialize(player);
        match name {
            "GetGachaInfoReq" => {
                let mut banners: Vec<_> = gacha_banner_collection::get().values().collect();
                banners.sort_by_key(|b| (b.sort_id, b.schedule_id));
                output.send(
                    uid,
                    "GetGachaInfoRsp",
                    GetGachaInfoRsp {
                        gacha_info_list: banners
                            .into_iter()
                            .filter(|b| gacha::GachaPool::new(b).is_ok())
                            .map(|b| {
                                info(
                                    b,
                                    &player.gacha_bin.as_ref().unwrap().gacha_map[&b.schedule_id],
                                )
                            })
                            .collect(),
                        ..Default::default()
                    },
                );
            }
            "DoGachaReq" => {
                if let Some(req) = message.decode::<DoGachaReq>() {
                    match draw(player, &req) {
                        Ok((rsp, change, new_avatars)) => {
                            inventory::notify_transaction(&output, player, &change);
                            for guid in new_avatars {
                                if let Some(avatar) = player
                                    .avatar_bin
                                    .as_ref()
                                    .and_then(|a| a.avatar_map.get(&guid))
                                    .and_then(snezhnaya_gi_avatar::util::client_avatar_info)
                                {
                                    output.send(
                                        uid,
                                        "AvatarAddNotify",
                                        AvatarAddNotify {
                                            avatar: Some(avatar),
                                            is_in_team: false,
                                        },
                                    );
                                }
                            }
                            output.send(uid, "DoGachaRsp", rsp);
                        }
                        Err(error) => output.send(
                            uid,
                            "DoGachaRsp",
                            DoGachaRsp {
                                retcode: error.into(),
                                gacha_type: req.gacha_type,
                                gacha_schedule_id: req.gacha_schedule_id,
                                ..Default::default()
                            },
                        ),
                    }
                }
            }
            "GachaWishReq" => {
                if let Some(req) = message.decode::<GachaWishReq>() {
                    let result = wish(player, &req);
                    let state = player
                        .gacha_bin
                        .as_ref()
                        .unwrap()
                        .gacha_map
                        .get(&req.gacha_schedule_id);
                    output.send(
                        uid,
                        "GachaWishRsp",
                        GachaWishRsp {
                            retcode: result.err().map(i32::from).unwrap_or(0),
                            gacha_type: req.gacha_type,
                            gacha_schedule_id: req.gacha_schedule_id,
                            wish_item_id: state.map_or(0, |s| s.wish_item_id),
                            wish_progress: state.map_or(0, |s| s.wish_progress),
                            wish_max_progress: 1,
                        },
                    );
                }
            }
            _ => {}
        }
    }
}

fn wish(player: &mut bin::PlayerDataBin, req: &GachaWishReq) -> Result<(), Retcode> {
    let banner = gacha_banner_collection::get()
        .get(&req.gacha_schedule_id)
        .ok_or(Retcode::RetGachaScheduleNotMatch)?;
    if req.gacha_type != banner.gacha_type
        || !matches!(banner.gacha_type, 302 | 500)
        || (req.item_id != 0 && !banner.rate_up_items_5.contains(&req.item_id))
    {
        return Err(Retcode::RetGachaWishInvalidItem);
    }
    let state = player
        .gacha_bin
        .as_mut()
        .unwrap()
        .gacha_map
        .get_mut(&banner.schedule_id)
        .unwrap();
    if state.wish_item_id != req.item_id {
        state.wish_item_id = req.item_id;
        state.wish_progress = 0;
    }
    Ok(())
}

fn draw(
    player: &mut bin::PlayerDataBin,
    req: &DoGachaReq,
) -> Result<(DoGachaRsp, InventoryChange, Vec<u64>), Retcode> {
    if !matches!(req.gacha_times, 1 | 10) {
        return Err(Retcode::RetGachaInvalidTimes);
    }
    let banner = gacha_banner_collection::get()
        .get(&req.gacha_schedule_id)
        .ok_or(Retcode::RetGachaScheduleNotMatch)?;
    if banner.gacha_type != req.gacha_type {
        return Err(Retcode::RetGachaScheduleNotMatch);
    }
    let pool = gacha::GachaPool::new(banner)?;
    let current = &player.gacha_bin.as_ref().unwrap().gacha_map[&banner.schedule_id];
    if banner
        .gacha_times_limit
        .is_some_and(|limit| current.total_count.saturating_add(req.gacha_times) > limit)
    {
        return Err(Retcode::RetGachaTimesLimit);
    }
    if banner.gacha_type == 500 && current.wish_item_id == 0 {
        return Err(Retcode::RetGachaWishIsNeededBeforeGacha);
    }
    let cost = if req.gacha_times == 10 {
        banner.cost_item_amount_10.unwrap_or(10)
    } else {
        1
    };
    if inventory::item_count(
        player.item_bin.as_ref().ok_or(Retcode::RetItemNotExist)?,
        banner.cost_item_id,
    ) < cost
    {
        return Err(Retcode::RetGachaCostItemNotEnough);
    }
    // 发奖、扣款和保底一起提交，任一奖励无效时整次回滚。
    let mut candidate = player.clone();
    let mut state = current.clone();
    let mut rng = SmallRng::from_entropy();
    let mut rewards = Amounts::new();
    let mut cards = Vec::new();
    let mut new_avatars = Vec::new();
    let mut owned: HashSet<_> = candidate
        .item_bin
        .as_ref()
        .unwrap()
        .iter()
        .map(|(_, i)| i.item_id)
        .collect();
    for _ in 0..req.gacha_times {
        state.fail_4_count = state.fail_4_count.saturating_add(1);
        state.fail_5_count = state.fail_5_count.saturating_add(1);
        let roll = pool.roll(&mut rng, banner, &state);
        if roll.rank == 4 {
            state.fail_4_count = 0;
            state.item4_is_up_list = vec![roll.is_up];
        }
        if roll.rank == 5 {
            state.fail_5_count = 0;
            state.item5_is_up_list = vec![roll.is_up];
            state.wish_progress = if state.wish_item_id != 0 && state.wish_item_id != roll.item_id {
                1
            } else {
                0
            };
        }
        let mut card = GachaItem {
            gacha_item: Some(ItemParam {
                item_id: roll.item_id,
                count: 1,
            }),
            ..Default::default()
        };
        let mut glitter = 0;
        let dust = if roll.rank == 3 { 15 } else { 0 };
        if let Some(avatar_id) = gacha::avatar_id(roll.item_id) {
            let config = excel::avatar_excel_config_collection::get()
                .get(&avatar_id)
                .ok_or(Retcode::RetGachaInavailable)?;
            let previous = candidate
                .avatar_bin
                .as_ref()
                .and_then(|a| a.avatar_map.values().find(|a| a.avatar_id == avatar_id))
                .map(|a| a.guid);
            if let Some(guid) = previous {
                let avatar = &candidate.avatar_bin.as_ref().unwrap().avatar_map[&guid];
                let depot = avatar
                    .depot_map
                    .get(&avatar.skill_depot_id)
                    .ok_or(Retcode::RetGachaInavailable)?;
                let material = excel::avatar_skill_depot_excel_config_collection::get()
                    .get(&avatar.skill_depot_id)
                    .and_then(|d| d.talents.first())
                    .and_then(|id| excel::avatar_talent_excel_config_collection::get().get(id))
                    .map(|t| t.cost_item_id())
                    .unwrap_or(0);
                let copies = depot.talent_id_list.len() as u32
                    + inventory::item_count(candidate.item_bin.as_ref().unwrap(), material)
                    + rewards.get(&material).copied().unwrap_or(0);
                glitter = if roll.rank == 5 {
                    if copies >= 6 {
                        25
                    } else {
                        10
                    }
                } else if copies >= 6 {
                    5
                } else {
                    2
                };
                if copies < 6 {
                    if material == 0 {
                        return Err(Retcode::RetGachaInavailable);
                    }
                    inventory::add_amount(&mut rewards, material, 1)?;
                    card.transfer_items.push(GachaTransferItem {
                        item: Some(ItemParam {
                            item_id: material,
                            count: 1,
                        }),
                        is_transfer_item_new: copies == 0,
                    });
                } else if roll.rank == 5
                    && excel::material_excel_config_collection::get().contains_key(&104300)
                {
                    inventory::add_amount(&mut rewards, 104300, 1)?;
                    card.transfer_items.push(GachaTransferItem {
                        item: Some(ItemParam {
                            item_id: 104300,
                            count: 1,
                        }),
                        ..Default::default()
                    });
                }
                if let Some(avatar_bin::Detail::FormalAvatar(formal)) = candidate
                    .avatar_bin
                    .as_mut()
                    .unwrap()
                    .avatar_map
                    .get_mut(&guid)
                    .unwrap()
                    .detail
                    .as_mut()
                {
                    formal.avatar_repeat_count = formal.avatar_repeat_count.saturating_add(1);
                }
            } else {
                let guid = snezhnaya_gi_avatar::util::grant_avatar(&mut candidate, config)
                    .ok_or(Retcode::RetGachaInavailable)?;
                new_avatars.push(guid);
                card.is_gacha_item_new = true;
            }
        } else {
            inventory::add_amount(&mut rewards, roll.item_id, 1)?;
            card.is_gacha_item_new = owned.insert(roll.item_id);
            if roll.rank >= 4 {
                glitter = if roll.rank == 5 { 10 } else { 2 };
            }
        }
        for (id, count) in [(221, glitter), (222, dust)] {
            if count != 0 {
                inventory::add_amount(&mut rewards, id, count)?;
                card.token_item_list.push(ItemParam { item_id: id, count });
            }
        }
        cards.push(card);
    }
    let mut change = inventory::transact(
        &mut candidate,
        &Amounts::from([(banner.cost_item_id, cost)]),
        &rewards,
        &[],
    )?;
    for guid in &new_avatars {
        let items: Vec<_> = candidate.avatar_bin.as_ref().unwrap().avatar_map[guid]
            .equip_map
            .values()
            .map(|item| (item.guid, item.item_id))
            .collect();
        for (guid, item_id) in items {
            change.changed.push(guid);
            change.added.push(guid);
            snezhnaya_gi_inventory::codex::record_item(&mut candidate, item_id, 0, &mut change.codex_updates);
        }
    }
    state.total_count = state
        .total_count
        .checked_add(req.gacha_times)
        .ok_or(Retcode::RetGachaTimesLimit)?;
    state.daily_count = state.daily_count.saturating_add(req.gacha_times);
    let all = candidate.gacha_bin.as_mut().unwrap();
    for (&id, other) in &mut all.gacha_map {
        if id != banner.schedule_id
            && gacha_banner_collection::get()
                .get(&id)
                .is_some_and(|b| b.gacha_type == banner.gacha_type)
        {
            other.fail_4_count = state.fail_4_count;
            other.fail_5_count = state.fail_5_count;
            other.item4_is_up_list = state.item4_is_up_list.clone();
            other.item5_is_up_list = state.item5_is_up_list.clone();
        }
    }
    all.gacha_map.insert(banner.schedule_id, state.clone());
    record_progress(&mut candidate, "gacha", 0, req.gacha_times);
    record_progress(&mut candidate, "gacha", banner.gacha_type, req.gacha_times);
    let rsp = DoGachaRsp {
        gacha_type: banner.gacha_type,
        gacha_schedule_id: banner.schedule_id,
        gacha_times: req.gacha_times,
        gacha_item_list: cards,
        cost_item_id: banner.cost_item_id,
        cost_item_num: 1,
        ten_cost_item_id: banner.cost_item_id,
        ten_cost_item_num: banner.cost_item_amount_10.unwrap_or(10),
        gacha_times_limit: banner.gacha_times_limit.unwrap_or(u32::MAX),
        left_gacha_times: banner
            .gacha_times_limit
            .unwrap_or(u32::MAX)
            .saturating_sub(state.total_count),
        cur_schedule_daily_gacha_times: state.daily_count,
        daily_gacha_times: state.daily_count,
        new_gacha_random: rand::random(),
        wish_item_id: state.wish_item_id,
        wish_progress: state.wish_progress,
        wish_max_progress: if matches!(banner.gacha_type, 302 | 500) {
            1
        } else {
            0
        },
        ..Default::default()
    };
    *player = candidate;
    Ok((rsp, change, new_avatars))
}
