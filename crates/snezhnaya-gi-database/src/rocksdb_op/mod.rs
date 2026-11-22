use crate::{data::UserUidRow, DbConnection, DbError};

use std::sync::{LazyLock, Mutex};

static UID_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub fn insert_or_update_player_data(
    conn: &DbConnection,
    uid: i32,
    data: impl AsRef<[u8]>,
) -> Result<(), DbError> {
    let data = data.as_ref();
    let key = format!("player_data:{}", uid);
    if data.is_empty() {
        return Err(DbError::InvalidData("empty player save".into()));
    }
    let mut batch = rocksdb::WriteBatch::default();
    if let Some(previous) = conn.0.get(&key)? {
        batch.put(format!("player_data_backup:{}", uid), previous);
    }
    batch.put(key, data);
    let mut options = rocksdb::WriteOptions::default();
    options.set_sync(true);
    conn.0.write_opt(batch, &options)?;
    Ok(())
}

pub fn select_player_data_by_uid(
    conn: &DbConnection,
    uid: i32,
) -> Result<Option<Vec<u8>>, DbError> {
    let key = format!("player_data:{}", uid);
    match conn.0.get(key)? {
        Some(value) => Ok(Some(value)),
        None => Ok(None),
    }
}

pub fn select_user_uid_by_account_uid(
    conn: &DbConnection,
    account_uid: &str,
) -> Result<Option<UserUidRow>, DbError> {
    let key = format!("user_uid:{}", account_uid);
    match conn.0.get(key)? {
        Some(value) => {
            let user_uid: UserUidRow = serde_json::from_slice(&value)?;
            Ok(Some(user_uid))
        }
        None => Ok(None),
    }
}

pub fn insert_user_uid(conn: &DbConnection, account_uid: &str) -> Result<UserUidRow, DbError> {
    let key = format!("user_uid:{}", account_uid);

    let _guard = UID_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(existing) = select_user_uid_by_account_uid(conn, account_uid)? {
        return Ok(existing);
    }
    let uid = match conn.0.get("uid_counter")? {
        Some(value) => std::str::from_utf8(&value)
            .ok()
            .and_then(|s| s.parse::<i32>().ok())
            .filter(|uid| *uid >= 10001)
            .and_then(|uid| uid.checked_add(1))
            .ok_or_else(|| DbError::InvalidData("uid_counter is corrupt or exhausted".into()))?,
        None => 10001,
    };

    let user_uid = UserUidRow {
        account_uid: account_uid.to_string(),
        uid,
    };

    let value = serde_json::to_vec(&user_uid)?;

    crate::batch_write(conn, |batch| {
        batch.put("uid_counter", uid.to_string().as_bytes());
        batch.put(key, value);
    })?;

    Ok(user_uid)
}
