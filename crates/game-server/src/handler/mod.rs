mod get_player_token;
mod gm_util;

// use std::net::{IpAddr, Ipv4Addr};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};

use crate::handler::gm_util::execute_gm_cmd;
use crate::{AppState, net::Connection, util};
use anyhow::Result;
use common::player_cache::{cache_set_client_time, cache_set_player_client_data_version};
use snezhnaya_gi_encryption::xor::MhyXorpad;
use snezhnaya_gi_message::output::ClientOutput;
use snezhnaya_gi_proto::normal::{
    GetPlayerTokenReq, GetPlayerTokenRsp, PingReq, PingRsp, PlayerLoginReq, UnionCmdNotify,
};
use snezhnaya_gi_proto::{
    Protobuf,
    packet_head::PacketHead,
    raw_packet::{RawPacket, make_raw_packet},
    retcode::Retcode,
};
use tokio::sync::mpsc;

enum InputItem {
    NewConnection(Connection),
    DropConnection(u32),
    Packet(u32, Box<[u8]>),
    Shutdown(tokio::sync::oneshot::Sender<()>),
    TokenVerified(
        u32,
        Arc<Session>,
        Result<get_player_token::VerifiedToken, i32>,
    ),
}

#[derive(Clone)]
pub struct PacketHandler(mpsc::UnboundedSender<InputItem>);

pub struct Session {
    pub connection: Connection,
    pub account_uid: OnceLock<String>,
    pub player_uid: OnceLock<u32>,
    pub xor_pad: OnceLock<MhyXorpad>,
    pub version: OnceLock<String>,
    pub login_started: AtomicBool,
    token_pending: AtomicBool,
}

impl PacketHandler {
    pub async fn shutdown(&self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self.0.send(InputItem::Shutdown(tx)).is_ok() {
            let _ = rx.await;
        }
    }
    pub fn new(state: &'static AppState) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move { packet_handler_loop(rx, state).await });

        Self(tx)
    }

    pub fn add_connection(&self, conn: Connection) {
        let _ = self.0.send(InputItem::NewConnection(conn));
    }

    pub fn remove_connection(&self, id: u32) {
        let _ = self.0.send(InputItem::DropConnection(id));
    }

    pub fn enqueue(&self, conn_id: u32, buf: Box<[u8]>) {
        let _ = self.0.send(InputItem::Packet(conn_id, buf));
    }
}

async fn packet_handler_loop(mut rx: mpsc::UnboundedReceiver<InputItem>, state: &'static AppState) {
    let mut token_tasks = tokio::task::JoinSet::new();
    loop {
        let item = tokio::select! {
            item = rx.recv() => match item { Some(item) => item, None => break },
            completed = token_tasks.join_next(), if !token_tasks.is_empty() => match completed {
                Some(Ok(item)) => item,
                Some(Err(error)) => { tracing::error!(%error, "SDK verification task failed"); continue; }
                None => continue,
            },
        };
        match item {
            InputItem::Shutdown(done) => {
                token_tasks.abort_all();
                while token_tasks.join_next().await.is_some() {}
                for session in state.sessions.iter() {
                    if let Some(uid) = session.player_uid.get() {
                        state.logic_simulator.offline(*uid);
                    }
                }
                state.sessions.clear();
                let _ = done.send(());
                break;
            }
            InputItem::TokenVerified(id, session, result) => {
                let active = state
                    .sessions
                    .get(&id)
                    .is_some_and(|current| Arc::ptr_eq(current.value(), &session));
                if !active {
                    continue;
                }
                session.token_pending.store(false, Ordering::Release);
                let response = match result {
                    Ok(verified) => get_player_token::complete(&session, verified),
                    Err(retcode) => GetPlayerTokenRsp {
                        retcode,
                        ..Default::default()
                    },
                };
                if response.retcode == 0 {
                    snezhnaya_gi_message::PLAYER_VERSION
                        .get()
                        .unwrap()
                        .insert(response.uid, session.version.get().unwrap().clone());
                }
                if let Err(error) = send_token_response(state, &session, response).await {
                    tracing::warn!(%error, "could not send token response");
                }
            }
            InputItem::NewConnection(conn) => {
                state.sessions.insert(
                    conn.conv,
                    Arc::new(Session {
                        connection: conn,
                        account_uid: OnceLock::new(),
                        player_uid: OnceLock::new(),
                        xor_pad: OnceLock::new(),
                        version: Default::default(),
                        login_started: AtomicBool::new(false),
                        token_pending: AtomicBool::new(false),
                    }),
                );
            }
            InputItem::DropConnection(id) => {
                if let Some(session) = state.sessions.get(&id) {
                    match session.player_uid.get() {
                        Some(uid) => state.logic_simulator.offline(*uid),
                        None => {}
                    }
                }
                state.sessions.remove(&id);
            }
            InputItem::Packet(id, buf) => {
                if let Some(session) = state
                    .sessions
                    .get(&id)
                    .map(|session| Arc::clone(session.value()))
                {
                    if let Err(err) =
                        handle_packet(state, &session, buf, id, &mut token_tasks).await
                    {
                        tracing::warn!("handle_packet(connection_id: {id}) failed, error: {err}");
                    }
                }
            }
        }
    }
}

async fn handle_packet(
    state: &'static AppState,
    session: &Arc<Session>,
    mut data: Box<[u8]>,
    conv: u32,
    token_tasks: &mut tokio::task::JoinSet<InputItem>,
) -> Result<()> {
    // if session.connection.source_addr.ip() != IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)) {
    //     return Ok(())
    // }
    util::xor_packet(
        session.xor_pad.get(),
        state.initial_xor_pad.as_ref(),
        &mut data,
    );

    let packet = RawPacket::new(&data)?;

    let cmd_id = packet.cmd_id();
    let head = packet.head();
    if head.is_gm_packet && cmd_id == 1 {
        tracing::debug!(
            "gm_packet source_addr ip:{} port:{}",
            session.connection.source_addr.ip(),
            session.connection.source_addr.port()
        );

        let body = packet.body();
        match snezhnaya_gi_proto::gm::GmTalkByMuipReq::decode(body) {
            Ok(req) => {
                let rsp = execute_gm_cmd(state, req, head, body).await;
                let head = PacketHead {
                    is_gm_packet: true,
                    ..Default::default()
                };
                let mut data = make_raw_packet(2, head, &rsp.encode_to_vec());
                util::xor_packet(
                    session.xor_pad.get(),
                    state.initial_xor_pad.as_ref(),
                    &mut data,
                );

                session.connection.send(data).await;
            }
            Err(_) => {}
        }

        return Ok(());
    }

    if session.version.get().is_none() {
        let version = snezhnaya_gi_proto::dy_parser::get_version(cmd_id);
        session.version.set(version).expect("TODO: panic message");
    }
    if session.version.get().unwrap().is_empty() {
        return Err(anyhow::anyhow!("version mismatch"));
    }
    let version = session.version.get().unwrap().as_str();
    let body = packet.body();

    match snezhnaya_gi_proto::dy_parser::get_name_by_cmd_id_version(version, cmd_id) {
        Some(message_name) => {
            match message_name.as_str() {
                "GetPlayerTokenReq" => {
                    if session.account_uid.get().is_some() {
                        return send_token_response(
                            state,
                            session,
                            GetPlayerTokenRsp {
                                retcode: Retcode::RetGetTokenSessionHasUid.into(),
                                ..Default::default()
                            },
                        )
                        .await;
                    }
                    if session.token_pending.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    let Some(request) =
                        snezhnaya_gi_proto::dy_parser::decode_from_vec_by_name_version::<
                            GetPlayerTokenReq,
                        >(version, "GetPlayerTokenReq", body)
                    else {
                        return send_token_response(
                            state,
                            session,
                            GetPlayerTokenRsp {
                                retcode: Retcode::RetTokenParamError.into(),
                                ..Default::default()
                            },
                        )
                        .await;
                    };
                    let Some(permit) = state.sdk.try_reserve() else {
                        return send_token_response(
                            state,
                            session,
                            GetPlayerTokenRsp {
                                retcode: Retcode::RetSystemBusy.into(),
                                ..Default::default()
                            },
                        )
                        .await;
                    };
                    session.token_pending.store(true, Ordering::Release);
                    let session = session.clone();
                    token_tasks.spawn(async move {
                        let _permit = permit;
                        let result = get_player_token::verify(state, request).await;
                        InputItem::TokenVerified(conv, session, result)
                    });
                }
                "PlayerLoginReq" => {
                    tracing::debug!("PlayerLoginReq received packet: {}", hex::encode(&body));

                    match snezhnaya_gi_proto::dy_parser::decode_from_vec_by_name_version::<
                        PlayerLoginReq,
                    >(version, "PlayerLoginReq", body)
                    {
                        None => {}
                        Some(request) => {
                            let Some(&user_id) = session.player_uid.get() else {
                                return Err(anyhow::anyhow!("login before token exchange"));
                            };
                            if session.login_started.swap(true, Ordering::AcqRel) {
                                return Ok(());
                            }
                            let user_session_id = session.connection.conv;
                            if request.client_version.contains("_") {
                                cache_set_player_client_data_version(
                                    user_id,
                                    request.client_data_version,
                                );
                            }

                            tracing::debug!(
                                "received player login request, session id: {}, player uid: {}",
                                user_session_id,
                                user_id
                            );
                            player_login(state, user_id, user_session_id, conv).await;
                        }
                    }
                }
                "PingReq" => {
                    tracing::trace!("PingReq received packet: {}", hex::encode(&data));

                    match snezhnaya_gi_proto::dy_parser::decode_from_vec_by_name_version::<PingReq>(
                        version, "PingReq", body,
                    ) {
                        None => {
                            tracing::error!("version:{} message_name:{} error", version, "PingReq");
                        }
                        Some(ping_req) => {
                            match session.player_uid.get() {
                                None => {}
                                Some(uid) => {
                                    cache_set_client_time(*uid, ping_req.client_time);
                                }
                            }

                            let ping_rsp = &PingRsp {
                                retcode: Retcode::RetSucc.into(),
                                client_time: ping_req.client_time,
                                seq: ping_req.seq,
                            };

                            match snezhnaya_gi_proto::dy_parser::get_cmd_id_by_name_version(
                                version, "PingRsp",
                            ) {
                                None => {
                                    tracing::error!("version:{} message_name:{} error", version, "PingRsp");
                                }
                                Some(cmd_id) => {
                                    match snezhnaya_gi_proto::dy_parser::encode_to_vec_by_name_version::<
                                        PingRsp,
                                    >(
                                        version, "PingRsp", &ping_rsp,
                                    ) {
                                        None => {
                                            tracing::error!(
                                                "version:{} message_name:{} error",
                                                version, "PingRsp"
                                            );
                                        }
                                        Some(body) => {
                                            let mut data = make_raw_packet(
                                                cmd_id,
                                                PacketHead::default(),
                                                &body,
                                            );
                                            util::xor_packet(
                                                session.xor_pad.get(),
                                                state.initial_xor_pad.as_ref(),
                                                &mut data,
                                            );

                                            session.connection.send(data).await;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                "UnionCmdNotify" => {
                    tracing::trace!("UnionCmdNotify received packet: ...");

                    match snezhnaya_gi_proto::dy_parser::decode_from_vec_by_name_version::<
                        UnionCmdNotify,
                    >(version, "UnionCmdNotify", body)
                    {
                        None => {
                            tracing::error!(
                                "version:{} message_name:{} error",
                                version,
                                "UnionCmdNotify"
                            );
                        }
                        Some(union_cmd) => {
                            let Some(&user_id) = session.player_uid.get() else {
                                return Err(anyhow::anyhow!("union packet before token exchange"));
                            };
                            let user_session_id = session.connection.conv;
                            let head = PacketHead {
                                user_id,
                                user_session_id,
                                ..Default::default()
                            };

                            for sub_cmd in union_cmd.cmd_list {
                                tracing::trace!(
                                    "received union subcommand with cmd_id: {}",
                                    sub_cmd.message_id
                                );

                                state.logic_simulator.add_client_packet(
                                    user_id,
                                    head.clone(),
                                    sub_cmd.message_id as u16,
                                    sub_cmd.body.into(),
                                    false,
                                );
                            }

                            state.logic_simulator.update_world(user_id);
                        }
                    }
                }
                _ => match session.player_uid.get() {
                    None => {}
                    Some(user_id) => {
                        let head = PacketHead {
                            user_id: *user_id,
                            user_session_id: session.connection.conv,
                            ..Default::default()
                        };
                        state.logic_simulator.add_client_packet(
                            *user_id,
                            head,
                            cmd_id,
                            packet.body().into(),
                            true,
                        );
                    }
                },
            }
        }
        _ => {}
    }

    Ok(())
}

async fn player_login(state: &'static AppState, user_id: u32, user_session_id: u32, conv: u32) {
    let (tx, rx) = mpsc::channel(32);
    tokio::spawn(packet_sink(state, user_id, user_session_id, rx, conv));

    let Some(player_data) = state.db_handle.fetch(user_id).await else {
        tracing::error!("failed to get player data, uid: {user_id}");
        let version = snezhnaya_gi_message::get_player_version!(&user_id);
        let response = snezhnaya_gi_proto::normal::PlayerLoginRsp {
            retcode: Retcode::RetLoginDbFail.into(),
            ..Default::default()
        };
        if let (Some(id), Some(body)) = (
            snezhnaya_gi_proto::dy_parser::get_cmd_id_by_name_version(&version, "PlayerLoginRsp"),
            snezhnaya_gi_proto::dy_parser::encode_to_vec_by_name_version(
                &version,
                "PlayerLoginRsp",
                &response,
            ),
        ) {
            let _ = tx
                .send((id, PacketHead::default(), body.into_boxed_slice()))
                .await;
        }
        if let Some(session) = state.sessions.get(&conv) {
            session.login_started.store(false, Ordering::Release);
        }
        return;
    };

    state
        .logic_simulator
        .create_world(user_id, player_data, ClientOutput::new(tx));
}

async fn packet_sink(
    state: &'static AppState,
    user_id: u32,
    user_session_id: u32,
    mut rx: mpsc::Receiver<(u16, PacketHead, Box<[u8]>)>,
    conv: u32,
) {
    while let Some((cmd_id, head, body)) = rx.recv().await {
        match state
            .sessions
            .get(&conv)
            .map(|session| Arc::clone(session.value()))
        {
            None => break,
            Some(session) => {
                let mut data = make_raw_packet(
                    cmd_id,
                    PacketHead {
                        user_id,
                        user_session_id,
                        ..head
                    },
                    &body,
                );
                util::xor_packet(
                    session.xor_pad.get(),
                    state.initial_xor_pad.as_ref(),
                    &mut data,
                );

                session.connection.send(data).await;
            }
        }
    }
}

async fn send_token_response(
    state: &AppState,
    session: &Session,
    response: GetPlayerTokenRsp,
) -> Result<()> {
    let version = session
        .version
        .get()
        .ok_or_else(|| anyhow::anyhow!("token response has no protocol version"))?;
    let id =
        snezhnaya_gi_proto::dy_parser::get_cmd_id_by_name_version(version, "GetPlayerTokenRsp")
            .ok_or_else(|| anyhow::anyhow!("GetPlayerTokenRsp is not registered"))?;
    let body = snezhnaya_gi_proto::dy_parser::encode_to_vec_by_name_version(
        version,
        "GetPlayerTokenRsp",
        &response,
    )
    .ok_or_else(|| anyhow::anyhow!("could not encode GetPlayerTokenRsp"))?;
    let mut packet = make_raw_packet(id, PacketHead::default(), &body);
    util::xor_packet(None, state.initial_xor_pad.as_ref(), &mut packet);
    session.connection.send(packet).await;
    Ok(())
}
