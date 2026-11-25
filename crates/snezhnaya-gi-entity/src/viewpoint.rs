use crate::{
    avatar::CurrentPlayerAvatarMarker,
    common::{ConfigId, EntityById, GroupId, OwnerPlayerUID, ToBeRemovedMarker, Visible},
    gadget::{GadgetID, Interactive},
    transform::Transform,
};
use bevy_ecs::{prelude::*, system::SystemParam};
use common::player_cache::cache_get_is_tp;
use snezhnaya_gi_data::excel::{
    codex::CodexConfig, common::EntityType, gadget_excel_config_collection,
};
use snezhnaya_gi_event::entity::ViewpointDiscoverEvent;
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::{
    normal::{CodexType, GadgetInteractReq, GadgetInteractRsp, InterOpType, InteractType},
    retcode::Retcode,
    server_only::PlayerDataBin,
};

// 客户端触发距离为 3 米，服务端给同步中的位置留出余量。
const MAX_INTERACT_DISTANCE_SQUARED: f32 = 8.0 * 8.0;

pub(super) fn is_viewpoint(gadget_id: u32) -> bool {
    gadget_excel_config_collection::get()
        .get(&gadget_id)
        .is_some_and(|config| config.r#type == EntityType::ViewPoint)
}

pub(super) fn codex_id(
    player: &PlayerDataBin,
    group_id: u32,
    config_id: u32,
    gadget_id: u32,
) -> Option<u32> {
    CodexConfig::get().viewpoint(
        player.scene_bin.as_ref()?.my_cur_scene_id,
        group_id,
        config_id,
        gadget_id,
    )
}

pub(super) fn discovered(player: &PlayerDataBin, id: u32) -> bool {
    player.codex_bin.as_ref().is_some_and(|codex| {
        codex.type_data_list.iter().any(|entry| {
            entry.r#type == CodexType::CodexView as u32 && entry.codex_id_list.contains(&id)
        })
    })
}

#[derive(SystemParam)]
pub struct ViewpointInteraction<'w, 's> {
    players: Res<'w, Players>,
    gadgets: Query<
        'w,
        's,
        (
            &'static GadgetID,
            Option<&'static GroupId>,
            Option<&'static ConfigId>,
            &'static Transform,
            &'static Interactive,
            Has<Visible>,
            Has<ToBeRemovedMarker>,
        ),
    >,
    avatars: Query<
        'w,
        's,
        (&'static OwnerPlayerUID, &'static Transform),
        (
            With<CurrentPlayerAvatarMarker>,
            With<Visible>,
            Without<ToBeRemovedMarker>,
        ),
    >,
    discoveries: MessageWriter<'w, ViewpointDiscoverEvent>,
}

impl ViewpointInteraction<'_, '_> {
    pub(super) fn interact(
        &mut self,
        index: &EntityById,
        uid: u32,
        request: &GadgetInteractReq,
    ) -> Option<GadgetInteractRsp> {
        let gadget = index
            .0
            .get(&request.gadget_entity_id)
            .and_then(|&entity| self.gadgets.get(entity).ok());
        if !is_viewpoint(request.gadget_id) && !gadget.is_some_and(|(id, ..)| is_viewpoint(id.0)) {
            return None;
        }

        let mut response = GadgetInteractRsp {
            gadget_id: request.gadget_id,
            gadget_entity_id: request.gadget_entity_id,
            op_type: request.op_type,
            interact_type: InteractType::InteractView as i32,
            ..Default::default()
        };
        let result = (|| {
            let Some((id, group, config, transform, interactive, visible, removed)) = gadget else {
                return Err(Retcode::RetGadgetNotExist);
            };
            if id.0 != request.gadget_id {
                return Err(Retcode::RetGadgetInteractParamError);
            }
            if !interactive.0 || !visible || removed {
                return Err(Retcode::RetGadgetNotInteractive);
            }
            let op = InterOpType::try_from(request.op_type)
                .map_err(|_| Retcode::RetGadgetInteractParamError)?;
            let player = self.players.get(uid).ok_or(Retcode::RetAvatarNotOnScene)?;
            if cache_get_is_tp(uid).unwrap_or(false) {
                return Err(Retcode::RetPlayerIsEnterScene);
            }
            let id = group
                .zip(config)
                .and_then(|(group, config)| codex_id(player, group.0, config.0, id.0))
                .ok_or(Retcode::RetSceneGroupNotMatch)?;
            let Some((_, avatar)) = self.avatars.iter().find(|(owner, _)| owner.0 == uid) else {
                return Err(Retcode::RetAvatarNotOnScene);
            };
            let offset_x = avatar.position.x - transform.position.x;
            let offset_y = avatar.position.y - transform.position.y;
            let offset_z = avatar.position.z - transform.position.z;
            let distance = offset_x * offset_x + offset_y * offset_y + offset_z * offset_z;
            if !distance.is_finite() || distance > MAX_INTERACT_DISTANCE_SQUARED {
                return Err(Retcode::RetDistanceLong);
            }
            if op == InterOpType::InterOpFinish && !discovered(player, id) {
                self.discoveries.write(ViewpointDiscoverEvent {
                    player_uid: uid,
                    codex_id: id,
                    gadget_entity_id: request.gadget_entity_id,
                });
            }
            Ok(())
        })();
        if let Err(code) = result {
            response.retcode = code.into();
        }
        Some(response)
    }
}
