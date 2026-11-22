use crate::command::LOGIC_COMMAND_QUEUE;
use crate::{command::LogicCommand, player_world::PlayerWorld};
use common::logging::TRACE_LOG_PACKET;
use common::player_cache::remove_player_cache;
use snezhnaya_gi_data::GAME_SERVER_CONFIG;
use snezhnaya_gi_data::scene::group_entity_state_cache::get_group_entity_state_cache;
use snezhnaya_gi_message::get_player_version;
use snezhnaya_gi_message::output::ClientOutput;
use snezhnaya_gi_proto::packet_head::PacketHead;
use snezhnaya_gi_proto::server_only::PlayerDataBin;
use std::{
    collections::{BTreeSet, HashMap},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct LogicSimulator;

impl LogicSimulator {
    pub fn spawn(save_data_tx: tokio::sync::mpsc::Sender<(u32, Vec<u8>)>) -> Self {
        thread::spawn(|| simulation_loop(save_data_tx));
        Self
    }

    pub fn create_world(&self, uid: u32, player_information: PlayerDataBin, output: ClientOutput) {
        LogicCommand::CreateWorld {
            player_information,
            output,
        }
        .push(uid);
    }

    pub fn add_client_packet(
        &self,
        uid: u32,
        head: PacketHead,
        cmd_id: u16,
        data: Box<[u8]>,
        immediate_mode: bool,
    ) {
        LogicCommand::ClientInput {
            head,
            cmd_id,
            data,
            immediate_mode,
        }
        .push(uid);
    }

    pub fn update_world(&self, uid: u32) {
        LogicCommand::WorldUpdate().push(uid);
    }

    pub fn offline(&self, uid: u32) {
        LogicCommand::Offline().push(uid);
    }

    pub async fn shutdown(&self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        LogicCommand::Shutdown(tx).push(0);
        let _ = rx.await;
    }
}

fn simulation_loop(save_data_tx: tokio::sync::mpsc::Sender<(u32, Vec<u8>)>) {
    let _ = crate::command::SIMULATION_THREAD.set(thread::current());
    // client_player_uid -> world_owner_uid
    let mut player_uid_map: HashMap<u32, u32> = HashMap::new();
    let mut player_world_map: HashMap<u32, PlayerWorld> = HashMap::new();
    let mut player_save_time_map: HashMap<u32, Instant> = HashMap::new();
    let mut deadlines: BTreeSet<(Instant, u32)> = BTreeSet::new();
    let mut scheduled: HashMap<u32, Instant> = HashMap::new();

    loop {
        for _ in 0..256 {
            let Some((uid, command)) = LOGIC_COMMAND_QUEUE.pop() else {
                break;
            };
            use LogicCommand::*;
            match command {
                Shutdown(done) => {
                    for (&uid, &owner) in &player_uid_map {
                        if let Some(world) = player_world_map.get_mut(&owner) {
                            let _ = save_data_tx
                                .blocking_send((uid, world.serialize_player_information(uid)));
                        }
                        remove_player_cache(uid);
                        if let Some(versions) = snezhnaya_gi_message::PLAYER_VERSION.get() {
                            versions.remove(&uid);
                        }
                    }
                    player_world_map.clear();
                    let _ = done.send(());
                    return;
                }
                CreateWorld {
                    player_information,
                    output,
                } => {
                    player_save_time_map.insert(player_information.uid, Instant::now());
                    player_uid_map.insert(player_information.uid, player_information.uid);
                    player_world_map.insert(
                        player_information.uid,
                        PlayerWorld::new(player_information, output),
                    );
                }
                ClientInput {
                    head,
                    cmd_id,
                    data,
                    immediate_mode,
                } => {
                    let uid = head.user_id;
                    if head.is_gm_packet {
                        if let Some(world_owner_uid) = player_uid_map.get(&uid) {
                            if let Some(world) = player_world_map.get_mut(world_owner_uid) {
                                world.add_packet(head, cmd_id, data, "GmTalkByMuipReq".to_string());
                            }
                        }
                    } else {
                        let binding = get_player_version!(&uid);
                        let version = binding.as_str();
                        match snezhnaya_gi_proto::dy_parser::get_name_by_cmd_id_version(
                            version, cmd_id,
                        ) {
                            None => {
                                tracing::warn!(
                                    "UNKNOWN version:{} cmd_id:{} message_name:UNKNOWN",
                                    version,
                                    cmd_id
                                );
                            }
                            Some(message_name) => {
                                if message_name.to_uppercase() == message_name {
                                    tracing::warn!(
                                        "UNKNOWN version:{} cmd_id:{} message_name:{}",
                                        version,
                                        cmd_id,
                                        message_name
                                    );
                                } else {
                                    if TRACE_LOG_PACKET.contains(&&*message_name) {
                                        tracing::trace!(
                                            "version:{} cmd_id: {} message_name:{}",
                                            version,
                                            cmd_id,
                                            message_name,
                                        );
                                        if GAME_SERVER_CONFIG.plugin.packet_log {
                                            tracing::trace!("recv:[{}]", hex::encode(&data));
                                        }
                                    } else {
                                        tracing::debug!(
                                            "version:{} cmd_id: {} message_name:{}",
                                            version,
                                            cmd_id,
                                            message_name,
                                        );
                                        if GAME_SERVER_CONFIG.plugin.packet_log {
                                            tracing::debug!("recv:[{}]", hex::encode(&data));
                                        }
                                    }

                                    if let Some(world_owner_uid) = player_uid_map.get(&uid) {
                                        if let Some(world) =
                                            player_world_map.get_mut(world_owner_uid)
                                        {
                                            world.add_packet(head, cmd_id, data, message_name);
                                            if immediate_mode {
                                                world.update();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                WorldUpdate() => {
                    if let Some(world_owner_uid) = player_uid_map.get(&uid) {
                        if let Some(world) = player_world_map.get_mut(world_owner_uid) {
                            world.update();
                        }
                    }
                }
                Offline() => {
                    if let Some(&world_owner_uid) = player_uid_map.get(&uid) {
                        if let Some(world) = player_world_map.get_mut(&world_owner_uid) {
                            if world.should_save(uid) {
                                let _ = save_data_tx
                                    .blocking_send((uid, world.serialize_player_information(uid)));
                            }
                        }
                        player_uid_map.remove(&uid);
                        player_world_map.remove(&world_owner_uid);
                        player_save_time_map.remove(&uid);
                    }
                    get_group_entity_state_cache().clear_user_cache(uid);
                    remove_player_cache(uid);
                    if let Some(versions) = snezhnaya_gi_message::PLAYER_VERSION.get() {
                        versions.remove(&uid);
                    }
                    tracing::info!("Player {} offline", uid);
                }
            }
            if let Some(&owner) = player_uid_map.get(&uid) {
                if let Some(world) = player_world_map.get_mut(&owner) {
                    if let Some(last_save) = player_save_time_map.get_mut(&owner) {
                        save_world_if_due(owner, world, last_save, &save_data_tx);
                        schedule_world(owner, world, *last_save, &mut scheduled, &mut deadlines);
                    }
                }
            } else if let Some(deadline) = scheduled.remove(&uid) {
                deadlines.remove(&(deadline, uid));
            }
        }

        let now = Instant::now();
        while let Some(&(deadline, owner)) = deadlines.first() {
            if deadline > now {
                break;
            }
            deadlines.pop_first();
            scheduled.remove(&owner);
            if let Some(world) = player_world_map.get_mut(&owner) {
                world.update();
                if let Some(last_save) = player_save_time_map.get_mut(&owner) {
                    save_world_if_due(owner, world, last_save, &save_data_tx);
                    schedule_world(owner, world, *last_save, &mut scheduled, &mut deadlines);
                }
            }
        }

        if !LOGIC_COMMAND_QUEUE.is_empty() {
            continue;
        }

        // 只更新到期的世界，空闲玩家不跟着循环扫描。
        match deadlines.first() {
            Some((deadline, _)) => {
                thread::park_timeout(deadline.saturating_duration_since(Instant::now()))
            }
            None => thread::park(),
        }
    }
}

fn schedule_world(
    owner: u32,
    world: &PlayerWorld,
    last_save: Instant,
    scheduled: &mut HashMap<u32, Instant>,
    deadlines: &mut BTreeSet<(Instant, u32)>,
) {
    if let Some(previous) = scheduled.remove(&owner) {
        deadlines.remove(&(previous, owner));
    }
    let deadline = world
        .next_wakeup()
        .map_or(last_save + Duration::from_secs(60), |timer| {
            timer.min(last_save + Duration::from_secs(60))
        });
    // 零延时脚本也让出一轮，避免自建定时器饿死其他玩家。
    let deadline = deadline.max(Instant::now() + Duration::from_millis(1));
    deadlines.insert((deadline, owner));
    scheduled.insert(owner, deadline);
}

fn save_world_if_due(
    uid: u32,
    world: &mut PlayerWorld,
    last_save: &mut Instant,
    save_data: &tokio::sync::mpsc::Sender<(u32, Vec<u8>)>,
) {
    if last_save.elapsed() >= Duration::from_secs(60) {
        if world.should_save(uid) {
            let _ = save_data.blocking_send((uid, world.serialize_player_information(uid)));
        }
        *last_save = Instant::now();
    }
}
