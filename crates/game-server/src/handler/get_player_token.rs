use super::Session;
use crate::AppState;
use common::language::Language;
use common::player_cache::{
    PlayerStatusType, cache_set_language, cache_set_online_status, is_player_online,
};
use rand::RngCore;
use snezhnaya_gi_encryption::xor::{MhyXorpad, XorpadGenerationMethod};
use snezhnaya_gi_proto::normal::{GetPlayerTokenReq, GetPlayerTokenRsp};
use snezhnaya_gi_proto::retcode::Retcode;

pub struct VerifiedToken {
    request: GetPlayerTokenReq,
    uid: u32,
    xor_pad: MhyXorpad,
    response: GetPlayerTokenRsp,
}

pub async fn verify(state: &AppState, request: GetPlayerTokenReq) -> Result<VerifiedToken, i32> {
    if request.client_rand_key.is_empty() || request.client_rand_key.len() > 8192 {
        return Err(Retcode::RetTokenParamError.into());
    }
    let mut response = GetPlayerTokenRsp::default();
    let xor_pad = gen_session_key(state, &request, &mut response)
        .ok_or(i32::from(Retcode::RetTokenParamError))?;
    state
        .sdk
        .verify(&request.account_uid, &request.account_token)
        .await
        .map_err(|error| match error {
            crate::sdk::VerifyError::Invalid => i32::from(Retcode::RetAccountVeirfyError),
            crate::sdk::VerifyError::Banned => i32::from(Retcode::RetAccountFreeze),
            crate::sdk::VerifyError::Unavailable => {
                tracing::warn!(
                    "SDK verification unavailable; check the service address and dispatch keys"
                );
                i32::from(Retcode::RetSystemBusy)
            }
        })?;
    // 校验成功后才分配角色 UID，错误令牌不会创建账号映射。
    let uid = state
        .db_handle
        .fetch_user_uid(request.account_uid.clone())
        .await
        .map_err(|error| {
            tracing::error!(%error, "could not allocate player UID");
            i32::from(Retcode::RetSystemBusy)
        })?;
    Ok(VerifiedToken {
        request,
        uid,
        xor_pad,
        response,
    })
}

pub fn complete(session: &Session, verified: VerifiedToken) -> GetPlayerTokenRsp {
    let VerifiedToken {
        request: req,
        uid,
        xor_pad,
        response,
    } = verified;
    let language = Language::from(req.lang);

    let rsp = GetPlayerTokenRsp {
        retcode: Retcode::RetFail.into(),
        ..response
    };

    //if online
    if is_player_online(uid) {
        let mut msg = "Your account is logged in on another device. Please wait.".to_string();

        if language == Language::Chs || language == Language::Cht {
            msg = "已在其他设备登录，等待".to_string();
        }

        return GetPlayerTokenRsp {
            retcode: Retcode::RetRepeatLogin.into(),
            msg,
            uid,
            ..rsp
        };
    }

    if let Some(account_uid) = session.account_uid.get() {
        tracing::debug!("repeated GetPlayerTokenReq (account_uid: {account_uid})");
        return rsp;
    }

    let _ = session.xor_pad.set(xor_pad);
    let _ = session.account_uid.set(req.account_uid);
    let _ = session.player_uid.set(uid);
    cache_set_online_status(uid, PlayerStatusType::PlayerStatusOnline);
    cache_set_language(uid, language);

    GetPlayerTokenRsp {
        retcode: Retcode::RetSucc.into(),
        uid,
        ..rsp
    }
}

fn gen_session_key(
    state: &AppState,
    req: &GetPlayerTokenReq,
    rsp: &mut GetPlayerTokenRsp,
) -> Option<MhyXorpad> {
    if !state
        .region_config
        .allowed_key_id_list
        .contains(&req.key_id)
    {
        tracing::debug!(
            "client key id ({}) is not allowed by region config",
            req.key_id
        );
        return None;
    }

    let Some(keys) = state.key_pair_map.get(&req.key_id) else {
        tracing::error!(
            "key id {} is allowed by region but doesn't exist in encryption config",
            req.key_id
        );
        return None;
    };

    let Ok(client_rand_key) = base64_simd::STANDARD.decode_to_vec(&req.client_rand_key) else {
        tracing::debug!("failed to decode client_rand_key as base64");
        return None;
    };

    let Some(client_rand_key) = keys.server_decrypt(&client_rand_key) else {
        tracing::debug!(
            "failed to decrypt client_rand_key using key_id: {}",
            req.key_id
        );
        return None;
    };

    let client_rand_key = u64::from_be_bytes(
        client_rand_key
            .try_into()
            .inspect_err(|_| tracing::debug!("client_rand_key is not uint64"))
            .ok()?,
    );

    let server_rand_key = rand::thread_rng().next_u64();

    rsp.server_rand_key = base64_simd::STANDARD
        .encode_to_string(&keys.client_encrypt(&server_rand_key.to_be_bytes()));
    rsp.sign = base64_simd::STANDARD.encode_to_string(&keys.sign(&server_rand_key.to_be_bytes()));

    Some(MhyXorpad::new::<byteorder::BE>(
        client_rand_key ^ server_rand_key,
        XorpadGenerationMethod::ReseedWithSkip,
    ))
}
