use bevy_app::prelude::*;
use bevy_ecs::schedule::IntoScheduleConfigs;

mod equip;
mod gm;
mod item;
mod production;
mod reliquary;
pub mod transaction;

pub struct InventoryPlugin;

impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreStartup, (production::login, reliquary::login).chain())
            .add_systems(PreUpdate, equip::change_avatar_equip)
            .add_systems(
                Update,
                (
                    production::handle,
                    reliquary::handle,
                    item::item_command_handler,
                    item::item_add_handler,
                    item::update_player_store,
                )
                    .chain(),
            )
            .add_systems(Update, item::item_drop_handler)
            .add_systems(Update, gm::weapon_command_handler);
    }
}
