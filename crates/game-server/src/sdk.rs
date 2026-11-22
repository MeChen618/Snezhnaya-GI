use anyhow::{Context, Result, bail};
use common::game_server_config::{SdkProvider, SdkSettings};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_tungstenite::tungstenite::{Message, protocol::WebSocketConfig};

const MAX_RESPONSE: usize = 65_536;

#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    Invalid,
    Banned,
    Unavailable,
}

enum Backend {
    AstaPs {
        dispatch_key: String,
        encryption_key: Vec<u8>,
    },
    HoyoSdk(reqwest::Client),
}

pub struct SdkClient {
    url: String,
    backend: Backend,
    timeout: Duration,
    pending: Arc<Semaphore>,
}

impl SdkClient {
    pub fn new(settings: &SdkSettings) -> Result<Self> {
        if !(100..=30_000).contains(&settings.timeout_ms)
            || !(1..=1024).contains(&settings.max_pending)
        {
            bail!("sdk timeout_ms must be 100..30000 and max_pending must be 1..1024");
        }
        let url = reqwest::Url::parse(&settings.verify_url).context("invalid sdk.verify_url")?;
        if !url.username().is_empty() || url.password().is_some() || url.host_str().is_none() {
            bail!("sdk.verify_url must have a host and no embedded credentials");
        }
        let timeout = Duration::from_millis(settings.timeout_ms);
        let backend = match settings.provider {
            SdkProvider::AstaPs => {
                if !matches!(url.scheme(), "ws" | "wss") {
                    bail!("AstaPS requires a ws:// or wss:// dispatch URL");
                }
                let dispatch_key = std::env::var("SNEZHNAYA_SDK_DISPATCH_KEY")
                    .unwrap_or_else(|_| settings.dispatch_key.clone());
                let key = std::env::var("SNEZHNAYA_SDK_ENCRYPTION_KEY")
                    .unwrap_or_else(|_| settings.encryption_key.clone());
                let encryption_key = base64_simd::STANDARD
                    .decode_to_vec(&key)
                    .map_err(|_| anyhow::anyhow!("sdk.encryption_key must be base64"))?;
                if dispatch_key.is_empty()
                    || dispatch_key.len() > 4096
                    || !(16..=4096).contains(&encryption_key.len())
                {
                    bail!(
                        "configure sdk.dispatch_key and sdk.encryption_key from AstaPS server.dispatch, or set SNEZHNAYA_SDK_DISPATCH_KEY and SNEZHNAYA_SDK_ENCRYPTION_KEY"
                    );
                }
                Backend::AstaPs {
                    dispatch_key,
                    encryption_key,
                }
            }
            SdkProvider::HoyoSdk => {
                if !matches!(url.scheme(), "http" | "https") {
                    bail!("hoyo-sdk requires an http:// or https:// verify URL");
                }
                Backend::HoyoSdk(
                    reqwest::Client::builder()
                        .timeout(timeout)
                        .connect_timeout(timeout)
                        .redirect(reqwest::redirect::Policy::none())
                        .pool_max_idle_per_host(8)
                        .build()
                        .context("creating SDK HTTP client")?,
                )
            }
        };
        Ok(Self {
            url: settings.verify_url.clone(),
            backend,
            timeout,
            pending: Arc::new(Semaphore::new(settings.max_pending)),
        })
    }

    pub fn try_reserve(&self) -> Option<OwnedSemaphorePermit> {
        self.pending.clone().try_acquire_owned().ok()
    }

    pub async fn verify(&self, uid: &str, token: &str) -> Result<(), VerifyError> {
        if uid.is_empty()
            || uid.len() > 128
            || uid.chars().any(char::is_control)
            || token.is_empty()
            || token.len() > 4096
            || token.chars().any(char::is_control)
        {
            return Err(VerifyError::Invalid);
        }
        tokio::time::timeout(self.timeout, async {
            match &self.backend {
                Backend::AstaPs {
                    dispatch_key,
                    encryption_key,
                } => {
                    self.verify_astaps(uid, token, dispatch_key, encryption_key)
                        .await
                }
                Backend::HoyoSdk(client) => self.verify_hoyo_sdk(client, uid, token).await,
            }
        })
        .await
        .unwrap_or(Err(VerifyError::Unavailable))
    }

    async fn verify_hoyo_sdk(
        &self,
        client: &reqwest::Client,
        uid: &str,
        token: &str,
    ) -> Result<(), VerifyError> {
        let mut response = client
            .post(&self.url)
            .json(&json!({ "uid": uid, "token": token }))
            .send()
            .await
            .map_err(|_| VerifyError::Unavailable)?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|len| len > MAX_RESPONSE as u64)
        {
            return Err(VerifyError::Unavailable);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| VerifyError::Unavailable)?
        {
            if body.len() + chunk.len() > MAX_RESPONSE {
                return Err(VerifyError::Unavailable);
            }
            body.extend_from_slice(&chunk);
        }
        #[derive(Deserialize)]
        struct Response {
            retcode: i32,
            data: Option<Data>,
        }
        #[derive(Deserialize)]
        struct Data {
            account: Account,
        }
        #[derive(Deserialize)]
        struct Account {
            uid: String,
        }
        let response: Response =
            serde_json::from_slice(&body).map_err(|_| VerifyError::Unavailable)?;
        if response.retcode == 0 && response.data.is_some_and(|data| data.account.uid == uid) {
            Ok(())
        } else {
            Err(VerifyError::Invalid)
        }
    }

    async fn verify_astaps(
        &self,
        uid: &str,
        token: &str,
        dispatch_key: &str,
        encryption_key: &[u8],
    ) -> Result<(), VerifyError> {
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_RESPONSE))
            .max_frame_size(Some(MAX_RESPONSE));
        let (mut socket, _) =
            tokio_tungstenite::connect_async_with_config(&self.url, Some(config), true)
                .await
                .map_err(|_| VerifyError::Unavailable)?;
        // 每次校验独立连接；AstaPS 的回包没有请求编号，不能共用回调。
        for (id, message) in [
            (1, json!(dispatch_key)),
            (2, json!({ "uid": uid, "token": token })),
        ] {
            socket
                .send(Message::Binary(
                    dispatch_packet(id, &message, encryption_key)?.into(),
                ))
                .await
                .map_err(|_| VerifyError::Unavailable)?;
        }
        for _ in 0..16 {
            match socket
                .next()
                .await
                .ok_or(VerifyError::Unavailable)?
                .map_err(|_| VerifyError::Unavailable)?
            {
                Message::Binary(body) => {
                    let mut body = body.to_vec();
                    xor(&mut body, encryption_key);
                    let packet: Value =
                        serde_json::from_slice(&body).map_err(|_| VerifyError::Unavailable)?;
                    if packet.get("packetId").and_then(Value::as_u64) != Some(3) {
                        continue;
                    }
                    let data = match &packet["message"] {
                        Value::String(message) => serde_json::from_str::<Value>(message)
                            .map_err(|_| VerifyError::Unavailable)?,
                        Value::Object(_) => packet["message"].clone(),
                        _ => return Err(VerifyError::Unavailable),
                    };
                    return validate_astaps_account(&data, uid);
                }
                Message::Ping(_) => socket.flush().await.map_err(|_| VerifyError::Unavailable)?,
                Message::Close(_) => return Err(VerifyError::Unavailable),
                _ => {}
            }
        }
        Err(VerifyError::Unavailable)
    }
}

fn dispatch_packet(id: u32, message: &Value, key: &[u8]) -> Result<Vec<u8>, VerifyError> {
    let message = serde_json::to_string(message).map_err(|_| VerifyError::Unavailable)?;
    let mut body = serde_json::to_vec(&json!({ "packetId": id, "message": message }))
        .map_err(|_| VerifyError::Unavailable)?;
    xor(&mut body, key);
    Ok(body)
}

fn xor(body: &mut [u8], key: &[u8]) {
    for (index, byte) in body.iter_mut().enumerate() {
        *byte ^= key[index % key.len()];
    }
}

fn validate_astaps_account(data: &Value, uid: &str) -> Result<(), VerifyError> {
    if data["valid"].as_bool() != Some(true) || data["account"]["id"].as_str() != Some(uid) {
        return Err(VerifyError::Invalid);
    }
    let account = &data["account"];
    let ban_end = account["banEndTime"].as_i64().unwrap_or(0);
    if account["isBanned"].as_bool().unwrap_or(false)
        && (ban_end <= 0 || ban_end > common::time_util::unix_timestamp() as i64)
    {
        return Err(VerifyError::Banned);
    }
    Ok(())
}
