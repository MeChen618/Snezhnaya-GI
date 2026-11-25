use bevy_app::prelude::*;
use bevy_ecs::schedule::IntoScheduleConfigs;

pub mod attendance;
mod consumables;
mod equip;
mod gm;
mod item;
mod production;
mod reliquary;
mod shop;
pub mod transaction;

pub struct InventoryPlugin;

impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<consumables::ResinWakeup>();
        app.add_systems(PostUpdate, consumables::tick);
        app.add_systems(
            PreStartup,
            (production::login, reliquary::login, consumables::login).chain(),
        )
        .add_systems(PreUpdate, equip::change_avatar_equip)
        .add_systems(
            Update,
            (
                consumables::handle,
                production::handle,
                reliquary::handle,
                shop::handle,
                item::handle_inventory_request,
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

pub use consumables::next_wakeup;
