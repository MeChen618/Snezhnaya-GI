use bevy_app::prelude::*;
use bevy_ecs::schedule::IntoScheduleConfigs;

mod appearance;
mod equip;
mod gm;
mod progression;
pub mod util;

pub struct AvatarPlugin;

impl Plugin for AvatarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, appearance::handle_appearance_change_request)
            .add_systems(
                Update,
                (
                    progression::handle_rewards,
                    progression::handle,
                    equip::apply_equip_change_to_avatar_entity,
                )
                    .chain(),
            )
            .add_systems(Update, gm::avatar_command_handler)
            .add_systems(Update, gm::buff_command_handler);
    }
}
