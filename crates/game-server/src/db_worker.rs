use crate::player_info_util;
use snezhnaya_gi_database::{rocksdb_op, DbConnection, DbError};
use snezhnaya_gi_proto::server_only::PlayerDataBin;
use snezhnaya_gi_proto::Protobuf;
use tokio::{
    select,
    sync::{mpsc, oneshot},
};

enum DbOperation {
    Fetch(u32, oneshot::Sender<Option<PlayerDataBin>>),
    FetchUserUid(String, oneshot::Sender<Result<u32, DbError>>),
    Shutdown(oneshot::Sender<Result<(), DbError>>),
}

pub struct DbWorkerHandle(mpsc::Sender<DbOperation>);

impl DbWorkerHandle {
    pub async fn shutdown(&self) -> Result<(), DbError> {
        let (tx, rx) = oneshot::channel();
        self.0
            .send(DbOperation::Shutdown(tx))
            .await
            .map_err(|_| DbError::Unavailable)?;
        rx.await.unwrap_or(Err(DbError::Unavailable))
    }
    pub async fn fetch(&self, uid: u32) -> Option<PlayerDataBin> {
        let (tx, rx) = oneshot::channel();
        let _ = self.0.send(DbOperation::Fetch(uid, tx)).await;

        rx.await.ok().flatten()
    }

    pub async fn fetch_user_uid(&self, account_uid: String) -> Result<u32, DbError> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .0
            .send(DbOperation::FetchUserUid(account_uid, tx))
            .await;

        rx.await.unwrap_or(Err(DbError::Unavailable))
    }
}

pub fn start(connection: DbConnection) -> (DbWorkerHandle, mpsc::Sender<(u32, Vec<u8>)>) {
    let (op_tx, op_rx) = mpsc::channel(32);
    let (save_data_tx, save_data_rx) = mpsc::channel(32);

    tokio::task::spawn_blocking(move || {
        // RocksDB 的同步读写留在专用线程，避免堵住网络运行时。
        tokio::runtime::Handle::current().block_on(db_work_loop(connection, op_rx, save_data_rx));
    });

    (DbWorkerHandle(op_tx), save_data_tx)
}

async fn db_work_loop(
    connection: DbConnection,
    mut op_rx: mpsc::Receiver<DbOperation>,
    mut save_data_rx: mpsc::Receiver<(u32, Vec<u8>)>,
) {
    let mut operations_open = true;
    let mut saves_open = true;
    let mut pending = std::collections::HashMap::new();
    let mut retry = tokio::time::interval(std::time::Duration::from_secs(5));
    while operations_open || saves_open {
        select! {
            _ = retry.tick() => {
                retry_saves(&connection, &mut pending);
            },
            op = op_rx.recv(), if operations_open => {
                match op {
                    Some(DbOperation::Fetch(uid, tx)) => {
                        let row = pending.get(&uid).cloned().map(|data| Ok(Some(data))).unwrap_or_else(|| rocksdb_op::select_player_data_by_uid(&connection, uid as i32));
                        let result = match row
                        {
                            Ok(Some(row)) => PlayerDataBin::decode(&*row).inspect_err(|err| {
                                tracing::error!(%err, uid, "player save is unreadable; original data retained");
                            }).ok().filter(|data| {
                                let valid = data.uid == uid && data.basic_bin.is_some() && data.item_bin.is_some() && data.avatar_bin.is_some();
                                if !valid { tracing::error!(uid, "player save failed identity/component validation; original data retained"); }
                                valid
                            }),
                            Ok(None) => Some(player_info_util::create_default_player_information(
                                uid,
                                String::from("SnezhnayaGI"),
                            )),
                            Err(_) => None,
                        };
                        let _ = tx.send(result);
                    }
                    Some(DbOperation::FetchUserUid(account_uid, tx)) => {
                        let result = rocksdb_op::insert_user_uid(&connection, &account_uid).map(|row| row.uid as u32);
                        let _ = tx.send(result);
                    }
                    Some(DbOperation::Shutdown(tx)) => {
                        while let Ok((uid, data)) = save_data_rx.try_recv() {
                            save_player(&connection, &mut pending, uid, data);
                        }
                        retry_saves(&connection, &mut pending);
                        let result = if pending.is_empty() { snezhnaya_gi_database::flush(&connection) }
                            else { Err(DbError::InvalidData(format!("{} player saves could not be written", pending.len()))) };
                        let _ = tx.send(result);
                        break;
                    }
                    None => operations_open = false,
                }
            },
            save_data = save_data_rx.recv(), if saves_open => {
                if let Some((uid, data)) = save_data {
                    save_player(&connection, &mut pending, uid, data);
                } else {
                    saves_open = false;
                }
            }
        }
    }
}

fn save_player(
    connection: &DbConnection,
    pending: &mut std::collections::HashMap<u32, Vec<u8>>,
    uid: u32,
    data: Vec<u8>,
) {
    match rocksdb_op::insert_or_update_player_data(connection, uid as i32, &data) {
        Ok(()) => {
            pending.remove(&uid);
        }
        Err(error) => {
            tracing::error!(%error, uid, "save failed; retaining latest snapshot for retry");
            pending.insert(uid, data);
        }
    }
}

fn retry_saves(connection: &DbConnection, pending: &mut std::collections::HashMap<u32, Vec<u8>>) {
    pending.retain(|uid, data| {
        rocksdb_op::insert_or_update_player_data(connection, *uid as i32, &*data).is_err()
    });
}
