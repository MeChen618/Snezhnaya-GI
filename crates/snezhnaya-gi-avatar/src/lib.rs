use bevy_app::prelude::*;
use bevy_ecs::schedule::IntoScheduleConfigs;

mod appearance;
mod equip;
mod expedition;
mod gm;
mod progression;
pub mod util;

pub struct AvatarPlugin;

impl Plugin for AvatarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<expedition::ExpeditionWakeup>()
            .add_systems(PreStartup, expedition::login)
            .add_systems(PreUpdate, appearance::handle_appearance_change_request)
            .add_systems(
                Update,
                (
                    progression::handle_rewards,
                    progression::handle,
                    expedition::handle,
                    expedition::update,
                    equip::apply_equip_change_to_avatar_entity,
                )
                    .chain(),
            )
            .add_systems(Update, gm::avatar_command_handler)
            .add_systems(Update, gm::buff_command_handler);
    }
}

pub fn next_wakeup(world: &bevy_ecs::world::World) -> Option<std::time::Instant> {
    expedition::next_wakeup(world)
}
