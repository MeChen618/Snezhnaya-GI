use rand::{rngs::SmallRng, Rng};
use snezhnaya_gi_data::custom::{BannerWeight, GachaBanner};
use snezhnaya_gi_data::excel::{self, common::MaterialType, AvatarUseType, ItemUseOp};
use snezhnaya_gi_proto::{retcode::Retcode, server_only::GachaBin};

pub struct GachaPool {
    up4: Vec<u32>,
    up5: Vec<u32>,
    normal3: Vec<u32>,
    normal4: Vec<u32>,
    normal5: Vec<u32>,
}

pub struct Roll {
    pub item_id: u32,
    pub rank: u32,
    pub is_up: bool,
}

pub fn avatar_id(item_id: u32) -> Option<u32> {
    let material = excel::material_excel_config_collection::get().get(&item_id)?;
    if material.material_type != MaterialType::Avatar {
        return None;
    }
    material
        .item_use
        .iter()
        .find(|action| action.use_op == ItemUseOp::GainAvatar)?
        .use_param
        .first()?
        .as_str()
        .parse()
        .ok()
}

impl GachaPool {
    pub fn new(banner: &GachaBanner) -> Result<Self, Retcode> {
        let materials = excel::material_excel_config_collection::get();
        let weapons = excel::weapon_excel_config_collection::get();
        let avatars = excel::avatar_excel_config_collection::get();
        let default = |rank| {
            let mut items: Vec<_> = weapons
                .values()
                .filter(|v| v.rank_level == rank)
                .map(|v| v.id)
                .collect();
            if banner.gacha_type != 302 && rank != 3 {
                items.extend(
                    materials
                        .values()
                        .filter(|v| {
                            v.rank_level == rank
                                && avatar_id(v.id)
                                    .and_then(|id| avatars.get(&id))
                                    .is_some_and(|a| a.use_type == Some(AvatarUseType::Formal))
                        })
                        .map(|v| v.id),
                );
            }
            items.sort_unstable();
            items
        };
        let normal4 = if banner.gacha_type == 302 {
            &banner.fallback_items_4_pool_2
        } else {
            &banner.fallback_items_4_pool_1
        };
        let normal5 = if banner.gacha_type == 302 {
            &banner.fallback_items_5_pool_2
        } else {
            &banner.fallback_items_5_pool_1
        };
        let pool = Self {
            up4: banner.rate_up_items_4.clone(),
            up5: banner.rate_up_items_5.clone(),
            normal3: banner
                .fallback_items_3
                .clone()
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| default(3)),
            normal4: normal4
                .clone()
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| default(4)),
            normal5: normal5
                .clone()
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| default(5)),
        };
        for (rank, items) in [
            (3, &pool.normal3),
            (4, &pool.normal4),
            (5, &pool.normal5),
            (4, &pool.up4),
            (5, &pool.up5),
        ] {
            if items.iter().any(|id| {
                weapons.get(id).map(|w| w.rank_level) != Some(rank)
                    && !materials.get(id).is_some_and(|m| {
                        m.rank_level == rank
                            && avatar_id(*id)
                                .and_then(|id| avatars.get(&id))
                                .is_some_and(|a| a.use_type == Some(AvatarUseType::Formal))
                    })
            }) {
                return Err(Retcode::RetGachaInavailable);
            }
        }
        if pool.normal3.is_empty() || pool.normal4.is_empty() || pool.normal5.is_empty() {
            return Err(Retcode::RetGachaInavailable);
        }
        if matches!(banner.gacha_type, 301 | 302 | 500) && pool.up5.is_empty() {
            return Err(Retcode::RetGachaInavailable);
        }
        Ok(pool)
    }

    pub fn roll(&self, rng: &mut SmallRng, banner: &GachaBanner, state: &GachaBin) -> Roll {
        let weapon = banner.gacha_type == 302;
        let rank = if chance(
            rng,
            state.fail_5_count,
            banner.weights_5.as_deref(),
            if weapon { 70 } else { 60 },
            if weapon { 63 } else { 74 },
            if weapon { 80 } else { 90 },
        ) {
            5
        } else if chance(
            rng,
            state.fail_4_count,
            banner.weights_4.as_deref(),
            if weapon { 600 } else { 510 },
            10,
            10,
        ) {
            4
        } else {
            3
        };
        let (up, normal, guarantee, rate) = match rank {
            5 => (
                &self.up5,
                &self.normal5,
                state.item5_is_up_list.last() == Some(&false),
                banner
                    .event_chance_5
                    .unwrap_or(if weapon { 75 } else { 50 }),
            ),
            4 => (
                &self.up4,
                &self.normal4,
                state.item4_is_up_list.last() == Some(&false),
                banner
                    .event_chance_4
                    .unwrap_or(if weapon { 75 } else { 50 }),
            ),
            _ => {
                return Roll {
                    item_id: self.normal3[rng.gen_range(0..self.normal3.len())],
                    rank: 3,
                    is_up: false,
                }
            }
        };
        if rank == 5
            && state.wish_item_id != 0
            && state.wish_progress >= 1
            && up.contains(&state.wish_item_id)
        {
            return Roll {
                item_id: state.wish_item_id,
                rank,
                is_up: true,
            };
        }
        let is_up = !up.is_empty() && (guarantee || rng.gen_range(0..100) < rate.min(100));
        let items = if is_up { up } else { normal };
        let item_id = items[rng.gen_range(0..items.len())];
        Roll {
            item_id,
            rank,
            is_up: !up.is_empty() && up.contains(&item_id),
        }
    }
}

fn chance(
    rng: &mut SmallRng,
    pulls: u32,
    weights: Option<&[BannerWeight]>,
    base: u32,
    soft: u32,
    hard: u32,
) -> bool {
    let weight = if let Some(weights) = weights {
        weights
            .iter()
            .filter(|row| row.pulls <= pulls)
            .max_by_key(|row| row.pulls)
            .map_or(0, |row| row.weight)
    } else if pulls >= hard {
        10000
    } else if pulls >= soft {
        base.saturating_add((pulls - soft + 1).saturating_mul(if hard == 80 { 700 } else { 600 }))
    } else {
        base
    };
    rng.gen_range(0..10000) < weight.min(10000)
}
