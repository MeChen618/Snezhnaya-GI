use bevy_ecs::prelude::*;
use snezhnaya_gi_data::excel::common::EquipType;
use snezhnaya_gi_data::excel::weapon_excel_config_collection;
use snezhnaya_gi_entity::avatar::AvatarEquipmentWeapon;
use snezhnaya_gi_entity::gadget::GadgetID;
use snezhnaya_gi_entity::{
    common::{
        refresh_avatar_fight_props, EntityCounter, FightProperties, Guid, Level, OwnerPlayerUID,
        ToBeRemovedMarker,
    },
    util::to_protocol_entity_id,
    weapon::{AffixMap, WeaponBundle, WeaponID, WeaponPromoteLevel},
};
use snezhnaya_gi_event::avatar::AvatarEquipChangeEvent;
use snezhnaya_gi_event::scene::WorldVersionConfig;
use snezhnaya_gi_message::output::MessageOutput;
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::normal::{
    AbilitySyncStateInfo, AvatarEquipChangeNotify, EntityRendererChangedInfo, ProtEntityType,
    SceneReliquaryInfo, SceneWeaponInfo,
};
use snezhnaya_gi_proto::server_only::{equip_bin, item_bin};

pub fn apply_equip_change_to_avatar_entity(
    mut events: MessageReader<AvatarEquipChangeEvent>,
    mut commands: Commands,
    avatars: Query<(
        Entity,
        &Guid,
        &OwnerPlayerUID,
        &AvatarEquipmentWeapon,
        &FightProperties,
    )>,
    mut entity_counter: ResMut<EntityCounter>,
    players: Res<Players>,
    message_output: Res<MessageOutput>,
    world_version_config: Res<WorldVersionConfig>,
) {
    let weapon_excel_config_collection_clone =
        std::sync::Arc::clone(weapon_excel_config_collection::get());

    for avatar_equip_change in events.read() {
        let Some(player_info) = players.get(avatar_equip_change.player_uid) else {
            continue;
        };

        let Some(ref player_avatar_bin) = player_info.avatar_bin else {
            continue;
        };
        let Some(avatar_bin) = player_avatar_bin
            .avatar_map
            .get(&avatar_equip_change.avatar_guid)
        else {
            tracing::debug!(
                "avatar guid {} doesn't exist",
                avatar_equip_change.avatar_guid
            );
            continue;
        };

        let found = avatars.iter().find(|(_, guid, owner_uid, ..)| {
            owner_uid.0 == avatar_equip_change.player_uid
                && guid.0 == avatar_equip_change.avatar_guid
        });
        let Some(fight_props) = refresh_avatar_fight_props(avatar_bin, found.map(|x| x.4)) else {
            continue;
        };
        if let Some((entity, ..)) = found {
            commands.entity(entity).insert(fight_props);
        } else {
            // 未上场角色只更新面板，不额外创建场景实体。
            message_output.send(
                avatar_equip_change.player_uid,
                "AvatarFightPropUpdateNotify",
                snezhnaya_gi_proto::normal::AvatarFightPropUpdateNotify {
                    avatar_guid: avatar_bin.guid,
                    fight_prop_map: fight_props
                        .0
                        .into_iter()
                        .map(|(key, value)| (key as u32, value))
                        .collect(),
                },
            );
        }

        let Some(equip_item_bin) = avatar_bin
            .equip_map
            .get(&(avatar_equip_change.equip_type as u32))
        else {
            message_output.send_to_all(
                "AvatarEquipChangeNotify",
                AvatarEquipChangeNotify {
                    avatar_guid: avatar_equip_change.avatar_guid,
                    equip_guid: 0,
                    item_id: 0,
                    equip_type: avatar_equip_change.equip_type as u32,
                    weapon: None,
                    reliquary: None,
                },
            );
            continue;
        };

        let Some(item_bin::Detail::Equip(ref equip_bin)) = equip_item_bin.detail else {
            continue;
        };

        match avatar_equip_change.equip_type {
            EquipType::None => {}
            EquipType::Bracer
            | EquipType::Necklace
            | EquipType::Shoes
            | EquipType::Ring
            | EquipType::Dress => {
                let Some(equip_bin::Detail::Reliquary(ref reliquary)) = equip_bin.detail else {
                    continue;
                };

                message_output.send_to_all(
                    "AvatarEquipChangeNotify",
                    AvatarEquipChangeNotify {
                        avatar_guid: avatar_equip_change.avatar_guid,
                        equip_guid: equip_item_bin.guid,
                        item_id: equip_item_bin.item_id,
                        equip_type: avatar_equip_change.equip_type as u32,
                        weapon: None,
                        reliquary: Some(SceneReliquaryInfo {
                            guid: equip_item_bin.guid,
                            level: reliquary.level,
                            item_id: equip_item_bin.item_id,
                            promote_level: 0,
                        }),
                    },
                )
            }
            EquipType::Weapon => {
                let weapon_id = equip_item_bin.item_id;

                let Some(equip_bin::Detail::Weapon(ref weapon)) = equip_bin.detail else {
                    continue;
                };

                let Some(weapon_config) = weapon_excel_config_collection_clone.get(&weapon_id)
                else {
                    tracing::debug!("weapon config {} doesn't exist", weapon_id);
                    continue;
                };

                let mut entity_id = 0;
                if let Some((avatar_entity, _, _, old_weapon, _)) = found {
                    commands.entity(old_weapon.0).insert(ToBeRemovedMarker);
                    let protocol_entity_id = to_protocol_entity_id(
                        world_version_config.protocol_version.as_str(),
                        ProtEntityType::ProtEntityWeapon,
                        entity_counter.inc(),
                    );
                    entity_id = protocol_entity_id.0;
                    let weapon_entity = commands
                        .spawn(WeaponBundle {
                            weapon_id: WeaponID(weapon_id),
                            entity_id: protocol_entity_id,
                            level: Level(weapon.level),
                            guid: Guid(equip_item_bin.guid),
                            gadget_id: GadgetID(weapon_config.gadget_id),
                            affix_map: AffixMap(weapon.affix_map.clone()),
                            promote_level: WeaponPromoteLevel(weapon.promote_level),
                        })
                        .id();
                    commands
                        .entity(avatar_entity)
                        .insert(AvatarEquipmentWeapon(weapon_entity));
                }

                message_output.send_to_all(
                    "AvatarEquipChangeNotify",
                    AvatarEquipChangeNotify {
                        avatar_guid: avatar_equip_change.avatar_guid,
                        equip_guid: equip_item_bin.guid,
                        item_id: equip_item_bin.item_id,
                        equip_type: avatar_equip_change.equip_type as u32,
                        weapon: Some(SceneWeaponInfo {
                            guid: equip_item_bin.guid,
                            entity_id: entity_id,
                            gadget_id: weapon_config.gadget_id,
                            item_id: equip_item_bin.item_id,
                            level: weapon.level,
                            promote_level: weapon.promote_level,
                            affix_map: weapon.affix_map.clone(),
                            ability_info: Some(AbilitySyncStateInfo::default()),
                            renderer_changed_info: Some(EntityRendererChangedInfo::default()),
                            ..Default::default()
                        }),
                        reliquary: None,
                    },
                );
            }
        }
    }
}
