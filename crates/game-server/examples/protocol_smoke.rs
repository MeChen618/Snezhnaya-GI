//! 用本地测试账号验证真实 UDP/KCP 登录和进场链路。
use anyhow::{anyhow, bail, Context, Result};
use common::data::EncryptionConfig;
use serde::{de::DeserializeOwned, Serialize};
use snezhnaya_gi_encryption::{
    rsa::RsaKeyPair,
    xor::{MhyXorpad, XorpadGenerationMethod},
};
use snezhnaya_gi_proto::{
    dy_parser as protocol,
    normal::*,
    packet_head::PacketHead,
    raw_packet::{make_raw_packet, RawPacket},
    Protobuf,
};
use std::{
    collections::{HashMap, VecDeque},
    io::{self, Write},
    net::UdpSocket,
    time::{Duration, Instant},
};

const VERSION: &str = "7.2.0";
struct Output(UdpSocket);
impl Write for Output {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.send(data)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct Client {
    socket: UdpSocket,
    kcp: kcp::Kcp<Output>,
    started: Instant,
    pad: MhyXorpad,
    inbox: VecDeque<(String, Vec<u8>)>,
    conv: u32,
    uid: u32,
    sequence: u32,
}
impl Drop for Client {
    fn drop(&mut self) {
        let mut packet = [0u8; 20];
        packet[..4].copy_from_slice(&0x194_u32.to_be_bytes());
        packet[4..8].copy_from_slice(&self.conv.to_be_bytes());
        packet[8..12].copy_from_slice(&self.kcp.token().to_be_bytes());
        packet[16..].copy_from_slice(&0x19419494_u32.to_be_bytes());
        let _ = self.socket.send(&packet);
    }
}
impl Client {
    fn connect(address: &str) -> Result<Self> {
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        socket.connect(address)?;
        socket.set_read_timeout(Some(Duration::from_secs(3)))?;
        let mut packet = [0u8; 20];
        packet[..4].copy_from_slice(&0xff_u32.to_be_bytes());
        packet[16..].copy_from_slice(&u32::MAX.to_be_bytes());
        socket.send(&packet)?;
        let length = socket.recv(&mut packet)?;
        anyhow::ensure!(
            length == 20 && packet[..4] == 0x145_u32.to_be_bytes(),
            "UDP handshake rejected"
        );
        let conv = u32::from_be_bytes(packet[4..8].try_into()?);
        let token = u32::from_be_bytes(packet[8..12].try_into()?);
        socket.set_read_timeout(Some(Duration::from_millis(10)))?;
        let kcp = kcp::Kcp::new(conv, token, 0, false, Output(socket.try_clone()?));
        Ok(Self {
            socket,
            kcp,
            started: Instant::now(),
            pad: MhyXorpad::from_ec2b(&std::fs::read("assets/region/client_secret_key.ec2b")?)?,
            inbox: VecDeque::new(),
            conv,
            uid: 0,
            sequence: 0,
        })
    }
    fn send<T: Serialize + Protobuf>(&mut self, name: &str, value: &T) -> Result<()> {
        let body = protocol::encode_to_vec_by_name_version(VERSION, name, value)
            .context("encoding request")?;
        self.sequence += 1;
        let head = PacketHead {
            client_sequence_id: self.sequence,
            user_id: self.uid,
            user_session_id: self.conv,
            ..Default::default()
        };
        let mut packet = make_raw_packet(
            protocol::get_cmd_id_by_name_version(VERSION, name).context("request CmdID")?,
            head,
            &body,
        );
        self.pad.xor(&mut packet);
        self.kcp.send(&packet)?;
        self.kcp.update(self.started.elapsed().as_millis() as u32)?;
        self.kcp.flush()?;
        Ok(())
    }
    fn queue(&mut self, cmd: u16, body: Vec<u8>) -> Result<()> {
        let name = protocol::get_name_by_cmd_id_version(VERSION, cmd).context("response CmdID")?;
        if name == "UnionCmdNotify" {
            let union: UnionCmdNotify =
                protocol::decode_from_vec_by_name_version(VERSION, &name, &body)
                    .context("decoding union")?;
            for sub in union.cmd_list {
                self.queue(sub.message_id as u16, sub.body)?;
            }
        } else {
            self.inbox.push_back((name, body));
        }
        Ok(())
    }
    fn wait<T: Default + Serialize + DeserializeOwned + Protobuf>(
        &mut self,
        name: &str,
    ) -> Result<T> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut datagram = [0u8; 65536];
        loop {
            if let Some(index) = self.inbox.iter().position(|(n, _)| n == name) {
                let (_, body) = self.inbox.remove(index).unwrap();
                return protocol::decode_from_vec_by_name_version(VERSION, name, &body)
                    .with_context(|| format!("decoding {name}"));
            }
            if Instant::now() > deadline {
                bail!(
                    "timeout waiting for {name}; received {:?}",
                    self.inbox.iter().map(|p| &p.0).collect::<Vec<_>>()
                );
            }
            self.kcp.update(self.started.elapsed().as_millis() as u32)?;
            match self.socket.recv(&mut datagram) {
                Ok(n) => {
                    self.kcp.input(&datagram[..n])?;
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    ()
                }
                Err(e) => return Err(e.into()),
            }
            while let Ok(size) = self.kcp.peeksize() {
                let mut bytes = vec![0; size];
                self.kcp.recv(&mut bytes)?;
                self.pad.xor(&mut bytes);
                let packet = RawPacket::new(&bytes)?;
                self.queue(packet.cmd_id(), packet.body().to_vec())?;
            }
        }
    }
}
fn main() -> Result<()> {
    common::logging::init();
    protocol::init()?;
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:21002".into());
    let mut client = Client::connect(&address)?;
    let configs: HashMap<u32, EncryptionConfig> =
        serde_json::from_slice(&std::fs::read("assets/region/encryption_config.json")?)?;
    let keys = RsaKeyPair::from_encryption_config(configs.get(&4).context("key 4 missing")?);
    let client_seed = 0x1020_3040_5060_7080_u64;
    let account_uid = std::env::var("SMOKE_ACCOUNT_UID").unwrap_or_else(|_| "720000001".into());
    client.send(
        "GetPlayerTokenReq",
        &GetPlayerTokenReq {
            account_uid,
            account_token: "local-smoke".into(),
            key_id: 4,
            platform_type: 3,
            lang: 2,
            client_rand_key: base64_simd::STANDARD
                .encode_to_string(&keys.server_encrypt(&client_seed.to_be_bytes())),
            ..Default::default()
        },
    )?;
    let token: GetPlayerTokenRsp = client.wait("GetPlayerTokenRsp")?;
    anyhow::ensure!(
        token.retcode == 0 && token.uid > 0,
        "token failed: {}",
        token.retcode
    );
    let server_seed = keys
        .client_decrypt(
            &base64_simd::STANDARD
                .decode_to_vec(&token.server_rand_key)
                .map_err(|e| anyhow!("{e}"))?,
        )
        .context("RSA seed")?;
    client.pad = MhyXorpad::new::<byteorder::BE>(
        client_seed
            ^ u64::from_be_bytes(
                server_seed
                    .try_into()
                    .map_err(|_| anyhow!("invalid seed size"))?,
            ),
        XorpadGenerationMethod::ReseedWithSkip,
    );
    client.uid = token.uid;
    client.send(
        "PlayerLoginReq",
        &PlayerLoginReq {
            client_version: "CNRELAndroid7.2.0_R49008628_S48722240_D49008628".into(),
            client_data_version: 49008628,
            ..Default::default()
        },
    )?;
    let login: PlayerLoginRsp = client.wait("PlayerLoginRsp")?;
    anyhow::ensure!(login.retcode == 0, "login failed: {}", login.retcode);
    let avatars: AvatarDataNotify = client.wait("AvatarDataNotify")?;
    anyhow::ensure!(!avatars.avatar_list.is_empty(), "avatar data is empty");
    let scene: PlayerEnterSceneNotify = client.wait("PlayerEnterSceneNotify")?;
    let enter_scene_token = scene.enter_scene_token;
    client.send(
        "EnterSceneReadyReq",
        &EnterSceneReadyReq { enter_scene_token },
    )?;
    let ready: EnterSceneReadyRsp = client.wait("EnterSceneReadyRsp")?;
    anyhow::ensure!(
        ready.retcode == 0 && ready.enter_scene_token == enter_scene_token,
        "scene ready failed"
    );
    client.send(
        "SceneInitFinishReq",
        &SceneInitFinishReq { enter_scene_token },
    )?;
    let init: SceneInitFinishRsp = client.wait("SceneInitFinishRsp")?;
    anyhow::ensure!(
        init.retcode == 0 && init.enter_scene_token == enter_scene_token,
        "scene init failed"
    );
    client.send(
        "EnterSceneDoneReq",
        &EnterSceneDoneReq { enter_scene_token },
    )?;
    let done: EnterSceneDoneRsp = client.wait("EnterSceneDoneRsp")?;
    anyhow::ensure!(
        done.retcode == 0 && done.enter_scene_token == enter_scene_token,
        "scene done failed"
    );
    client.send(
        "PostEnterSceneReq",
        &PostEnterSceneReq {
            enter_scene_token,
            ..Default::default()
        },
    )?;
    let post: PostEnterSceneRsp = client.wait("PostEnterSceneRsp")?;
    anyhow::ensure!(post.retcode == 0, "post scene failed");
    loop {
        let appeared: SceneEntityAppearNotify = client.wait("SceneEntityAppearNotify")?;
        if let Some(avatar) = appeared
            .entity_list
            .iter()
            .find(|entity| entity.entity_type == ProtEntityType::ProtEntityAvatar as i32)
        {
            anyhow::ensure!(
                avatar.entity_id >> 27 == 1
                    && matches!(avatar.entity, Some(scene_entity_info::Entity::Avatar(_))),
                "invalid avatar entity: id={:#010x}, type={}, avatar_content={}",
                avatar.entity_id,
                avatar.entity_type,
                matches!(avatar.entity, Some(scene_entity_info::Entity::Avatar(_)))
            );
            break;
        }
    }
    client.send(
        "GetScenePointReq",
        &GetScenePointReq {
            scene_id: 3,
            ..Default::default()
        },
    )?;
    let points: GetScenePointRsp = client.wait("GetScenePointRsp")?;
    anyhow::ensure!(
        points.retcode == 0 && points.scene_id == 3 && !points.unlocked_point_list.is_empty(),
        "scene points failed"
    );
    client.send(
        "PingReq",
        &PingReq {
            seq: 42,
            client_time: 1234,
            ..Default::default()
        },
    )?;
    let ping: PingRsp = client.wait("PingRsp")?;
    anyhow::ensure!(ping.retcode == 0 && ping.seq == 42, "ping failed");
    println!(
        "PASS: 7.2.0 UDP/KCP, RSA/session XOR, login, avatars, scene entry/entity, scene 3 points and ping; uid={}",
        token.uid
    );
    Ok(())
}
