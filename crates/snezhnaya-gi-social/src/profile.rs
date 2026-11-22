use bevy_ecs::prelude::*;
use common::player_cache::update_player_cache;
use snezhnaya_gi_data::excel::{
    common::MaterialType, material_excel_config_collection, production::ProductionConfig,
};
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::{
    normal::*,
    retcode::Retcode,
    server_only::{self, PlayerDataBin},
};
use std::collections::HashSet;

type ProfileResult<T> = Result<T, Retcode>;

fn constant(name: &str, index: usize, default: u32) -> u32 {
    ProductionConfig::get()
        .constants
        .get(name)
        .and_then(|values| values.get(index))
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}
fn name_card_exists(id: u32) -> bool {
    material_excel_config_collection::get()
        .get(&id)
        .is_some_and(|c| c.material_type == MaterialType::Namecard)
}
fn cache_profile(player: &PlayerDataBin) {
    update_player_cache(player.uid, |cache| {
        if let Some(basic) = &player.basic_bin {
            cache.nick_name.clone_from(&basic.nickname);
        }
        if let Some(social) = &player.social_bin {
            cache.name_card_id = social.name_card_id;
        }
    });
}

pub(super) fn login(mut players: ResMut<Players>) {
    let default_card = constant("CONST_VALUE_DEFAULT_NAME_CARD_ID", 0, 210001);
    for player in players.values_mut() {
        let social = player.social_bin.get_or_insert_with(Default::default);
        if name_card_exists(default_card) && !social.unlock_name_card_list.contains(&default_card) {
            social.unlock_name_card_list.push(default_card);
        }
        if !social.unlock_name_card_list.contains(&social.name_card_id)
            || !name_card_exists(social.name_card_id)
        {
            social.name_card_id = default_card;
        }
        cache_profile(player);
    }
}

pub(super) fn handle(
    mut events: MessageReader<ClientMessageEvent>,
    mut players: ResMut<Players>,
    output: Res<MessageOutput>,
) {
    for message in events.read() {
        let uid = message.sender_uid();
        if message.message_name() == "GetPlayerSocialDetailReq" {
            let Some(req) = message.decode::<GetPlayerSocialDetailReq>() else {
                continue;
            };
            let detail = players
                .get(req.uid)
                .and_then(|player| detail(player, uid, req.param));
            output.send(
                uid,
                "GetPlayerSocialDetailRsp",
                GetPlayerSocialDetailRsp {
                    retcode: if detail.is_some() {
                        Retcode::RetSucc
                    } else {
                        Retcode::RetUidNotExist
                    }
                    .into(),
                    detail_data: detail,
                    param: req.param,
                },
            );
            continue;
        }
        let Some(player) = players.get_mut(uid) else {
            continue;
        };
        macro_rules! respond {
            ($request:ty, $response:ident, $name:literal, $body:expr) => {
                if let Some(req) = message.decode::<$request>() {
                    let response =
                        ($body)(player, &req).unwrap_or_else(|code: Retcode| $response {
                            retcode: code.into(),
                            ..Default::default()
                        });
                    output.send(uid, $name, response);
                }
            };
        }
        match message.message_name() {
            "SetPlayerNameReq" => respond!(
                SetPlayerNameReq,
                SetPlayerNameRsp,
                "SetPlayerNameRsp",
                |player: &mut PlayerDataBin, req: &SetPlayerNameReq| {
                    let response = set_name(player, req)?;
                    output.send_json(
                        uid,
                        "PlayerNicknameNotify",
                        serde_json::json!({ "nickname": response.nick_name }),
                    );
                    Ok(response)
                }
            ),
            "SetPlayerSignatureReq" => respond!(
                SetPlayerSignatureReq,
                SetPlayerSignatureRsp,
                "SetPlayerSignatureRsp",
                set_signature
            ),
            "SetPlayerBirthdayReq" => respond!(
                SetPlayerBirthdayReq,
                SetPlayerBirthdayRsp,
                "SetPlayerBirthdayRsp",
                set_birthday
            ),
            "SetNameCardReq" => {
                respond!(SetNameCardReq, SetNameCardRsp, "SetNameCardRsp", set_card)
            }
            "UpdatePlayerShowAvatarListReq" => respond!(
                UpdatePlayerShowAvatarListReq,
                UpdatePlayerShowAvatarListRsp,
                "UpdatePlayerShowAvatarListRsp",
                set_avatars
            ),
            "UpdatePlayerShowNameCardListReq" => respond!(
                UpdatePlayerShowNameCardListReq,
                UpdatePlayerShowNameCardListRsp,
                "UpdatePlayerShowNameCardListRsp",
                set_cards
            ),
            "GetAllUnlockNameCardReq" => output.send(
                uid,
                "GetAllUnlockNameCardRsp",
                GetAllUnlockNameCardRsp {
                    name_card_list: player
                        .social_bin
                        .as_ref()
                        .map(|s| {
                            s.unlock_name_card_list
                                .iter()
                                .copied()
                                .filter(|id| name_card_exists(*id))
                                .collect()
                        })
                        .unwrap_or_default(),
                    ..Default::default()
                },
            ),
            _ => (),
        }
    }
}

fn valid_text(text: &str) -> bool {
    !text
        .chars()
        .any(|c| c.is_control() || matches!(c, '<' | '>'))
}

fn set_name(player: &mut PlayerDataBin, req: &SetPlayerNameReq) -> ProfileResult<SetPlayerNameRsp> {
    if req.nick_name.trim().is_empty() {
        return Err(Retcode::RetNicknameIsEmpty);
    }
    if req.nick_name.encode_utf16().count()
        > constant("CONST_VALUE_NICKNAME_MAX_LENGTH", 0, 14) as usize
    {
        return Err(Retcode::RetNicknameTooLong);
    }
    if req.nick_name.chars().filter(|c| c.is_numeric()).count()
        > constant("CONST_VALUE_NICKNAME_MAX_LENGTH", 1, 6) as usize
    {
        return Err(Retcode::RetNicknameTooManyDigits);
    }
    if !valid_text(&req.nick_name) {
        return Err(Retcode::RetNicknameWordIllegal);
    }
    let basic = player.basic_bin.as_mut().ok_or(Retcode::RetFail)?;
    if basic.nickname == req.nick_name {
        return Err(Retcode::RetNicknameNotChanged);
    }
    basic.nickname.clone_from(&req.nick_name);
    cache_profile(player);
    Ok(SetPlayerNameRsp {
        nick_name: req.nick_name.clone(),
        ..Default::default()
    })
}
fn set_signature(
    player: &mut PlayerDataBin,
    req: &SetPlayerSignatureReq,
) -> ProfileResult<SetPlayerSignatureRsp> {
    if req.signature.encode_utf16().count() > 50 || !valid_text(&req.signature) {
        return Err(Retcode::RetSignatureIllegal);
    }
    let social = player.social_bin.get_or_insert_with(Default::default);
    if social.signature == req.signature {
        return Err(Retcode::RetSignatureNotChanged);
    }
    social.signature.clone_from(&req.signature);
    Ok(SetPlayerSignatureRsp {
        signature: req.signature.clone(),
        ..Default::default()
    })
}
fn valid_birthday(month: u32, day: u32) -> bool {
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => 29,
        _ => 0,
    };
    day > 0 && day <= days
}
fn set_birthday(
    player: &mut PlayerDataBin,
    req: &SetPlayerBirthdayReq,
) -> ProfileResult<SetPlayerBirthdayRsp> {
    let birthday = req.birthday.ok_or(Retcode::RetBirthdayFormatError)?;
    if !valid_birthday(birthday.month, birthday.day) {
        return Err(Retcode::RetBirthdayFormatError);
    }
    let social = player.social_bin.get_or_insert_with(Default::default);
    if social
        .birthday
        .as_ref()
        .is_some_and(|b| valid_birthday(b.month, b.day))
    {
        return Err(Retcode::RetBirthdayCannotBeSetTwice);
    }
    social.birthday = Some(server_only::Birthday {
        month: birthday.month,
        day: birthday.day,
    });
    Ok(SetPlayerBirthdayRsp {
        birthday: Some(birthday),
        ..Default::default()
    })
}
fn set_card(player: &mut PlayerDataBin, req: &SetNameCardReq) -> ProfileResult<SetNameCardRsp> {
    let social = player.social_bin.get_or_insert_with(Default::default);
    if !name_card_exists(req.name_card_id)
        || !social.unlock_name_card_list.contains(&req.name_card_id)
    {
        return Err(Retcode::RetNameCardNotUnlocked);
    }
    social.name_card_id = req.name_card_id;
    cache_profile(player);
    Ok(SetNameCardRsp {
        name_card_id: req.name_card_id,
        ..Default::default()
    })
}
fn unique(ids: &[u32], limit: usize) -> bool {
    ids.len() <= limit
        && ids.iter().all(|id| *id != 0)
        && ids.iter().collect::<HashSet<_>>().len() == ids.len()
}
fn set_avatars(
    player: &mut PlayerDataBin,
    req: &UpdatePlayerShowAvatarListReq,
) -> ProfileResult<UpdatePlayerShowAvatarListRsp> {
    if !unique(&req.show_avatar_id_list, 12)
        || req.show_avatar_id_list.iter().any(|id| {
            !player.avatar_bin.as_ref().is_some_and(|a| {
                a.avatar_map.values().any(|a| {
                    a.avatar_id == *id
                        && matches!(
                            a.detail,
                            Some(server_only::avatar_bin::Detail::FormalAvatar(_))
                        )
                })
            })
        })
    {
        return Err(Retcode::RetShowAvatarInfoNotExist);
    }
    let social = player.social_bin.get_or_insert_with(Default::default);
    social
        .show_avatar_id_list
        .clone_from(&req.show_avatar_id_list);
    social.is_show_avatar = req.is_show_avatar;
    social.is_show_constellation_num = req.is_show_constellation_num;
    Ok(UpdatePlayerShowAvatarListRsp {
        show_avatar_id_list: req.show_avatar_id_list.clone(),
        is_show_avatar: req.is_show_avatar,
        is_show_constellation_num: req.is_show_constellation_num,
        ..Default::default()
    })
}
fn set_cards(
    player: &mut PlayerDataBin,
    req: &UpdatePlayerShowNameCardListReq,
) -> ProfileResult<UpdatePlayerShowNameCardListRsp> {
    let social = player.social_bin.get_or_insert_with(Default::default);
    if !unique(&req.show_name_card_id_list, 9)
        || req
            .show_name_card_id_list
            .iter()
            .any(|id| !name_card_exists(*id) || !social.unlock_name_card_list.contains(id))
    {
        return Err(Retcode::RetNameCardNotUnlocked);
    }
    social
        .show_name_card_id_list
        .clone_from(&req.show_name_card_id_list);
    Ok(UpdatePlayerShowNameCardListRsp {
        show_name_card_id_list: req.show_name_card_id_list.clone(),
        ..Default::default()
    })
}
fn detail(player: &PlayerDataBin, viewer: u32, param: u32) -> Option<SocialDetail> {
    let basic = player.basic_bin.as_ref()?;
    let social = player.social_bin.as_ref()?;
    let avatars = player.avatar_bin.as_ref();
    let show_avatar_info_list = if social.is_show_avatar || viewer == player.uid {
        social
            .show_avatar_id_list
            .iter()
            .filter_map(|id| {
                let avatar = avatars?.avatar_map.values().find(|a| a.avatar_id == *id)?;
                Some(SocialShowAvatarInfo {
                    avatar_id: *id,
                    level: avatar.level,
                    costume_id: avatar.costume_id,
                    talent_level: if social.is_show_constellation_num {
                        avatar
                            .depot_map
                            .get(&avatar.skill_depot_id)
                            .map_or(0, |d| d.talent_id_list.len() as u32)
                    } else {
                        0
                    },
                    ..Default::default()
                })
            })
            .collect()
    } else {
        vec![]
    };
    Some(SocialDetail {
        uid: player.uid,
        nickname: basic.nickname.clone(),
        level: basic.level,
        avatar_id: avatars
            .and_then(|a| a.avatar_map.get(&a.cur_avatar_guid))
            .map_or(0, |a| a.avatar_id),
        signature: social.signature.clone(),
        birthday: social.birthday.map(|b| Birthday {
            month: b.month,
            day: b.day,
        }),
        world_level: 9,
        online_state: FriendOnlineState::FriendOnline as i32,
        is_mp_mode_available: true,
        name_card_id: social.name_card_id,
        is_show_avatar: social.is_show_avatar,
        is_show_constellation_num: social.is_show_constellation_num,
        show_avatar_info_list,
        show_name_card_id_list: social.show_name_card_id_list.clone(),
        profile_picture: Some(ProfilePicture {
            profile_picture_id: basic.profile_picture_id,
            profile_frame_id: basic.profile_frame_id,
            ..Default::default()
        }),
        param,
        ..Default::default()
    })
}
