use crossbeam_queue::SegQueue;
use snezhnaya_gi_message::output::ClientOutput;
use snezhnaya_gi_proto::packet_head::PacketHead;
use snezhnaya_gi_proto::server_only::PlayerDataBin;
use std::sync::{LazyLock, OnceLock};

pub static LOGIC_COMMAND_QUEUE: LazyLock<SegQueue<(u32, LogicCommand)>> =
    LazyLock::new(|| SegQueue::new());
pub(crate) static SIMULATION_THREAD: OnceLock<std::thread::Thread> = OnceLock::new();

pub enum LogicCommand {
    CreateWorld {
        player_information: PlayerDataBin,
        output: ClientOutput,
    },
    ClientInput {
        head: PacketHead,
        cmd_id: u16,
        data: Box<[u8]>,
        immediate_mode: bool,
    },
    WorldUpdate(),
    Offline(),
    Shutdown(tokio::sync::oneshot::Sender<()>),
}

impl LogicCommand {
    pub fn push(self, uid: u32) {
        LOGIC_COMMAND_QUEUE.push((uid, self));
        if let Some(thread) = SIMULATION_THREAD.get() {
            thread.unpark();
        }
    }
}
