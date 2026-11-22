use crate::codec::{align_json, collect_masks, internal_value, transform, ScalarMasks};
use crate::Protobuf;
use anyhow::{anyhow, bail, Context, Result};
use notify::{RecursiveMode, Watcher};
use protobuf::reflect::{FileDescriptor, MessageDescriptor};
use serde::{de, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct VersionConfig {
    pub ty_value: u32,
}

impl Default for VersionConfig {
    fn default() -> Self {
        Self { ty_value: 24 }
    }
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct ReplaceValueConfig {
    pub n1: u64,
    pub n2: u64,
    pub n3: u64,
    pub n4: u64,
    pub n5: u64,
    pub const_in: u64,
    pub const_out: u64,
}

#[derive(Debug)]
pub struct VersionProtocol {
    pub version_config: VersionConfig,
    pub replace_value_map: HashMap<String, ReplaceValueConfig>,
    pub cmd_id_map: HashMap<String, u16>,
    pub cmd_id_map_re: HashMap<u16, String>,
    pub file_descriptor: FileDescriptor,
    pub command_rows: usize,
    messages: HashMap<String, MessageDescriptor>,
    masks: ScalarMasks,
}

pub static MULTI_VERSION_PROTOCOL: std::sync::OnceLock<
    Arc<dashmap::DashMap<String, VersionProtocol>>,
> = std::sync::OnceLock::new();

fn read_optional<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T> {
    if path.is_file() {
        serde_json::from_slice(&std::fs::read(path)?).with_context(|| path.display().to_string())
    } else {
        Ok(T::default())
    }
}

fn command_priority(name: &str) -> u8 {
    if name.ends_with("Req") {
        2
    } else if name.chars().any(|c| c.is_ascii_lowercase()) {
        1
    } else {
        0
    }
}

fn load_version(directory: &Path) -> Result<VersionProtocol> {
    let path = directory.join("all.proto");
    let parsed = protobuf_parse::Parser::new()
        .pure()
        .include(directory)
        .input(&path)
        .parse_and_typecheck()
        .with_context(|| path.display().to_string())?;
    // 导入的 option 和 descriptor 也必须参与依赖解析。
    let descriptors = FileDescriptor::new_dynamic_fds(parsed.file_descriptors, &[])?;
    let descriptor = descriptors
        .into_iter()
        .find(|f| f.proto().name() == "all.proto")
        .ok_or_else(|| anyhow!("missing all.proto descriptor"))?;
    let mut messages = HashMap::new();
    let mut masks = HashMap::new();
    for message in descriptor.messages() {
        collect_masks(&message, &mut masks)?;
        messages
            .entry(message.name().trim_start_matches('_').to_owned())
            .or_insert_with(|| message.clone());
        messages.insert(message.name().to_owned(), message);
    }
    let csv = directory.join("cmdid.csv");
    let commands: Vec<(String, u16)> = if csv.is_file() {
        let text = std::fs::read_to_string(&csv)?;
        text.lines()
            .skip(1)
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let columns: Vec<_> = line.trim().split(',').collect();
                if columns.len() != 3 {
                    bail!("invalid CmdID row: {line}");
                }
                Ok((columns[2].to_owned(), columns[0].parse::<u16>()?))
            })
            .collect::<Result<_>>()?
    } else {
        let values: BTreeMap<String, u16> =
            serde_json::from_slice(&std::fs::read(directory.join("protocol.json"))?)?;
        values.into_iter().collect()
    };
    let mut cmd_id_map = HashMap::new();
    let mut cmd_id_map_re: HashMap<u16, String> = HashMap::new();
    for (name, id) in &commands {
        if !messages.contains_key(name) {
            bail!("CmdID {id} references missing message {name}");
        }
        let canonical = name.trim_start_matches('_').to_owned();
        cmd_id_map.insert(name.clone(), *id);
        cmd_id_map.entry(canonical.clone()).or_insert(*id);
        // 同号的混淆类型不能盖掉已确认的客户端请求。
        match cmd_id_map_re.get(id) {
            Some(old) if command_priority(old) >= command_priority(&canonical) => {}
            _ => {
                cmd_id_map_re.insert(*id, canonical);
            }
        }
    }
    let version_config: VersionConfig = read_optional(&directory.join("version_config.json"))?;
    if !(1..32).contains(&version_config.ty_value) {
        bail!("invalid entity type shift");
    }
    if !cmd_id_map.contains_key("GetPlayerTokenReq") || !cmd_id_map.contains_key("PlayerLoginReq") {
        bail!("protocol is missing login commands");
    }
    tracing::info!(version = %directory.display(), messages = descriptor.messages().count(), commands = commands.len(), masks = masks.len(), "protocol loaded");
    Ok(VersionProtocol {
        version_config,
        replace_value_map: read_optional(&directory.join("replace_value.json"))?,
        cmd_id_map,
        cmd_id_map_re,
        file_descriptor: descriptor,
        command_rows: commands.len(),
        messages,
        masks,
    })
}

pub fn init() -> Result<()> {
    let registry = MULTI_VERSION_PROTOCOL.get_or_init(|| Arc::new(dashmap::DashMap::new()));
    let root = Path::new("assets/proto");
    for entry in root.read_dir().context("missing assets/proto")? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && entry.path().join("all.proto").is_file() {
            let version = load_version(&entry.path())?;
            registry.insert(entry.file_name().to_string_lossy().into_owned(), version);
        }
    }
    if registry.is_empty() {
        bail!("no usable client protocols found");
    }
    std::thread::Builder::new()
        .name("protocol-watch".into())
        .spawn(|| {
            if let Err(error) = watch_protocols() {
                tracing::error!(%error, "protocol watcher stopped");
            }
        })?;
    Ok(())
}

fn watch_protocols() -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx)?;
    watcher.watch(Path::new("assets/proto"), RecursiveMode::Recursive)?;
    while let Ok(event) = rx.recv() {
        let event = event?;
        if !(event.kind.is_modify() || event.kind.is_create()) {
            continue;
        }
        std::thread::sleep(Duration::from_millis(300));
        let mut directories = std::collections::HashSet::new();
        for path in event.paths {
            if let Some(parent) = path.parent().filter(|p| p.join("all.proto").is_file()) {
                directories.insert(parent.to_path_buf());
            }
        }
        for directory in directories {
            match load_version(&directory) {
                Ok(version) => {
                    MULTI_VERSION_PROTOCOL.get().unwrap().insert(
                        directory
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                        version,
                    );
                }
                Err(error) => {
                    tracing::error!(%error, path = %directory.display(), "protocol reload rejected; previous version retained")
                }
            }
        }
    }
    Ok(())
}

pub fn get_version(first_cmd_id: u16) -> String {
    let mut version = "".to_string();
    MULTI_VERSION_PROTOCOL.get().unwrap().iter().for_each(|r| {
        let Some(message_name) = r.cmd_id_map_re.get(&first_cmd_id) else {
            return;
        };
        if message_name == "GetPlayerTokenReq" {
            version = r.key().clone();
        }
    });

    version
}

pub fn get_ty_value_by_version(version: &str) -> u32 {
    match MULTI_VERSION_PROTOCOL.get().unwrap().get(version) {
        None => {
            tracing::warn!("version not found for {}", version);
            24
        }
        Some(data) => data.version_config.ty_value,
    }
}

pub fn get_name_by_cmd_id_version(version: &str, cmd_id: u16) -> Option<String> {
    match MULTI_VERSION_PROTOCOL.get().unwrap().get(version) {
        None => {
            tracing::warn!("version not found for {}", version);
            None
        }
        Some(data) => match data.cmd_id_map_re.get(&cmd_id) {
            None => {
                tracing::error!("failed to get name version:{} cmd_id:{}", version, cmd_id,);
                None
            }
            Some(message_name) => Some(message_name.clone()),
        },
    }
}

pub fn get_cmd_id_by_name_version(version: &str, message_name: &str) -> Option<u16> {
    match MULTI_VERSION_PROTOCOL.get().unwrap().get(version) {
        None => {
            tracing::warn!("version not found for {}", version);
            None
        }
        Some(data) => match data.cmd_id_map.get(message_name) {
            None => {
                tracing::error!(
                    "failed to get cmd_id version:{} message_name:{}",
                    version,
                    message_name,
                );
                None
            }
            Some(cmd_id) => Some(*cmd_id),
        },
    }
}

pub fn encode_to_vec_by_name_version<T>(
    version: &str,
    message_name: &str,
    value: &T,
) -> Option<Vec<u8>>
where
    T: Sized + Serialize + Protobuf,
{
    if version == "base" {
        return Some(value.encode_to_vec());
    }
    encode_json_by_name_version(version, message_name, &serde_json::to_value(value).ok()?)
}

/// 使用本版描述符发送新增协议，字段编号和掩码仍由协议配置负责。
pub fn encode_json_by_name_version(
    version: &str,
    message_name: &str,
    value: &serde_json::Value,
) -> Option<Vec<u8>> {
    let registry = MULTI_VERSION_PROTOCOL.get()?;
    let data = registry.get(version)?;
    let result = (|| -> Result<Vec<u8>> {
        let descriptor = data
            .messages
            .get(message_name)
            .ok_or_else(|| anyhow!("message descriptor not found"))?;
        let json = align_json(descriptor, value.clone(), true);
        let mut message = protobuf_json_mapping::parse_dyn_from_str_with_options(
            descriptor,
            &serde_json::to_string(&json)?,
            &protobuf_json_mapping::ParseOptions {
                ignore_unknown_fields: true,
                ..Default::default()
            },
        )?;
        transform(message.as_mut(), &data.masks, true)?;
        Ok(message.write_to_bytes_dyn()?)
    })();
    result
        .map_err(|error| tracing::error!(%error, version, message_name, "protocol encode failed"))
        .ok()
}

pub fn decode_from_vec_by_name_version<T>(
    version: &str,
    message_name: &str,
    body: &[u8],
) -> Option<T>
where
    T: Default + Serialize + Protobuf + for<'a> de::Deserialize<'a>,
{
    if version == "base" {
        return T::decode(body).ok();
    }
    decode_value_by_name_version(version, message_name, body)?
        .deserialize_into()
        .ok()
}

pub fn decode_json_by_name_version(
    version: &str,
    message_name: &str,
    body: &[u8],
) -> Option<serde_json::Value> {
    // JSON 对象的键只能是字符串，不能直接反序列化带数字键的协议 map。
    serde_json::to_value(decode_value_by_name_version(version, message_name, body)?).ok()
}

fn decode_value_by_name_version(
    version: &str,
    message_name: &str,
    body: &[u8],
) -> Option<serde_value::Value> {
    let registry = MULTI_VERSION_PROTOCOL.get()?;
    let data = registry.get(version)?;
    let result = (|| -> Result<serde_value::Value> {
        let descriptor = data
            .messages
            .get(message_name)
            .ok_or_else(|| anyhow!("message descriptor not found"))?;
        let mut message = descriptor.new_instance();
        message.merge_from_bytes_dyn(body)?;
        transform(message.as_mut(), &data.masks, false)?;
        let text = protobuf_json_mapping::print_to_string_with_options(
            message.as_ref(),
            &protobuf_json_mapping::PrintOptions {
                proto_field_name: true,
                enum_values_int: true,
                ..Default::default()
            },
        )?;
        let aligned = align_json(descriptor, serde_json::from_str(&text)?, false);
        internal_value(descriptor, aligned)
    })();
    result
        .map_err(|error| tracing::error!(%error, version, message_name, "protocol decode failed"))
        .ok()
}

pub fn encode_empty_by_name_version(version: &str, message_name: &str) -> Option<Vec<u8>> {
    if version == "base" {
        return Some(Vec::new());
    }
    let registry = MULTI_VERSION_PROTOCOL.get()?;
    let data = registry.get(version)?;
    let mut message = data.messages.get(message_name)?.new_instance();
    transform(message.as_mut(), &data.masks, true).ok()?;
    message.write_to_bytes_dyn().ok()
}

pub fn replace_out_u32(version: &str, field_name: &str, ov: u32) -> u32 {
    match MULTI_VERSION_PROTOCOL.get().unwrap().get(version) {
        None => {
            tracing::warn!("version not found for {}", version);
            ov
        }
        Some(data) => match data.replace_value_map.get(field_name) {
            None => ov,
            Some(config) => {
                let result;
                if config.const_out != 0 {
                    result = config.const_out as u32;
                } else {
                    result = (ov
                        .wrapping_add(config.n1 as u32)
                        .wrapping_sub(config.n2 as u32)
                        ^ (config.n3 as u32))
                        .wrapping_add(config.n4 as u32)
                        .wrapping_sub(config.n5 as u32);
                }
                tracing::debug!(
                    "replace_out_u32 version:{} field_name:{} value1:{} value2:{}",
                    version,
                    field_name,
                    ov,
                    result
                );
                result
            }
        },
    }
}

pub fn replace_in_u32(version: &str, field_name: &str, ov: u32) -> u32 {
    match MULTI_VERSION_PROTOCOL.get().unwrap().get(version) {
        None => {
            tracing::warn!("version not found for {}", version);
            ov
        }
        Some(data) => match data.replace_value_map.get(field_name) {
            None => ov,
            Some(config) => {
                let result;
                if config.const_in != 0 {
                    result = config.const_in as u32;
                } else {
                    result = (ov
                        .wrapping_add(config.n5 as u32)
                        .wrapping_sub(config.n4 as u32)
                        ^ (config.n3 as u32))
                        .wrapping_add(config.n2 as u32)
                        .wrapping_sub(config.n1 as u32);
                }
                tracing::debug!(
                    "replace_in_u32 version:{} field_name:{} value1:{} value2:{}",
                    version,
                    field_name,
                    ov,
                    result
                );
                result
            }
        },
    }
}

pub fn replace_out_i32(version: &str, field_name: &str, ov: i32) -> i32 {
    match MULTI_VERSION_PROTOCOL.get().unwrap().get(version) {
        None => {
            tracing::warn!("version not found for {}", version);
            ov
        }
        Some(data) => match data.replace_value_map.get(field_name) {
            None => ov,
            Some(config) => {
                let result;
                if config.const_out != 0 {
                    result = config.const_out as i32;
                } else {
                    result = (ov
                        .wrapping_add(config.n1 as i32)
                        .wrapping_sub(config.n2 as i32)
                        ^ (config.n3 as i32))
                        .wrapping_add(config.n4 as i32)
                        .wrapping_sub(config.n5 as i32);
                }
                tracing::debug!(
                    "replace_out_i32 version:{} field_name:{} value1:{} value2:{}",
                    version,
                    field_name,
                    ov,
                    result
                );
                result
            }
        },
    }
}

pub fn replace_in_i32(version: &str, field_name: &str, ov: i32) -> i32 {
    match MULTI_VERSION_PROTOCOL.get().unwrap().get(version) {
        None => {
            tracing::warn!("version not found for {}", version);
            ov
        }
        Some(data) => match data.replace_value_map.get(field_name) {
            None => ov,
            Some(config) => {
                let result;
                if config.const_in != 0 {
                    result = config.const_in as i32;
                } else {
                    result = (ov
                        .wrapping_add(config.n5 as i32)
                        .wrapping_sub(config.n4 as i32)
                        ^ (config.n3 as i32))
                        .wrapping_add(config.n2 as i32)
                        .wrapping_sub(config.n1 as i32);
                }
                tracing::debug!(
                    "replace_in_i32 version:{} field_name:{} value1:{} value2:{}",
                    version,
                    field_name,
                    ov,
                    result
                );
                result
            }
        },
    }
}

pub fn replace_out_u64(version: &str, field_name: &str, ov: u64) -> u64 {
    match MULTI_VERSION_PROTOCOL.get().unwrap().get(version) {
        None => {
            tracing::warn!("version not found for {}", version);
            ov
        }
        Some(data) => match data.replace_value_map.get(field_name) {
            None => ov,
            Some(config) => {
                let result;
                if config.const_out != 0 {
                    result = config.const_out as u64;
                } else {
                    result = (ov
                        .wrapping_add(config.n1 as u64)
                        .wrapping_sub(config.n2 as u64)
                        ^ (config.n3 as u64))
                        .wrapping_add(config.n4 as u64)
                        .wrapping_sub(config.n5 as u64);
                }
                tracing::debug!(
                    "replace_out_u64 version:{} field_name:{} value1:{} value2:{}",
                    version,
                    field_name,
                    ov,
                    result
                );
                result
            }
        },
    }
}

pub fn replace_in_u64(version: &str, field_name: &str, ov: u64) -> u64 {
    match MULTI_VERSION_PROTOCOL.get().unwrap().get(version) {
        None => {
            tracing::warn!("version not found for {}", version);
            ov
        }
        Some(data) => match data.replace_value_map.get(field_name) {
            None => ov,
            Some(config) => {
                let result;
                if config.const_in != 0 {
                    result = config.const_in as u64;
                } else {
                    result = (ov
                        .wrapping_add(config.n5 as u64)
                        .wrapping_sub(config.n4 as u64)
                        ^ (config.n3 as u64))
                        .wrapping_add(config.n2 as u64)
                        .wrapping_sub(config.n1 as u64);
                }
                tracing::debug!(
                    "replace_in_u64 version:{} field_name:{} value1:{} value2:{}",
                    version,
                    field_name,
                    ov,
                    result
                );
                result
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normal::*;
    use protobuf::reflect::{ReflectValueBox, ReflectValueRef};
    use std::sync::Once;

    fn profile() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/proto/7.2.0");
            let protocol = load_version(&path).unwrap();
            assert_eq!(protocol.command_rows, 4012);
            assert_eq!(protocol.masks.len(), 30);
            assert_eq!(protocol.version_config.ty_value, 27);
            MULTI_VERSION_PROTOCOL
                .get_or_init(|| Arc::new(dashmap::DashMap::new()))
                .insert("7.2.0".into(), protocol);
        });
    }

    fn descriptor(name: &str) -> MessageDescriptor {
        profile();
        MULTI_VERSION_PROTOCOL
            .get()
            .unwrap()
            .get("7.2.0")
            .unwrap()
            .messages[name]
            .clone()
    }

    #[test]
    fn full_profile_keeps_login_and_duplicate_request_commands() {
        profile();
        assert_eq!(get_version(24604), "7.2.0");
        assert_eq!(
            get_name_by_cmd_id_version("7.2.0", 1017).as_deref(),
            Some("EnterSceneDoneReq")
        );
        assert_eq!(
            get_cmd_id_by_name_version("7.2.0", "NCJGOJMOPHB"),
            Some(1017)
        );
        assert_eq!(
            get_cmd_id_by_name_version("7.2.0", "GetPlayerTokenRsp"),
            Some(27808)
        );
        assert_eq!((1_u32 << get_ty_value_by_version("7.2.0")) | 1, 0x08000001);
    }

    #[test]
    fn scene_response_uses_native_masks_and_preserves_zero() {
        profile();
        for token in [0_u32, 1, 12345678, u32::MAX] {
            let body = encode_to_vec_by_name_version(
                "7.2.0",
                "EnterSceneDoneRsp",
                &EnterSceneDoneRsp {
                    enter_scene_token: token,
                    retcode: 0,
                },
            )
            .unwrap();
            let description = descriptor("EnterSceneDoneRsp");
            let mut wire = description.new_instance();
            wire.merge_from_bytes_dyn(&body).unwrap();
            assert_eq!(
                description
                    .field_by_name("enter_scene_token")
                    .unwrap()
                    .get_singular_field_or_default(wire.as_ref()),
                ReflectValueRef::U32((token ^ 0xBF71).wrapping_sub(0x860B))
            );
            assert_eq!(
                description
                    .field_by_name("retcode")
                    .unwrap()
                    .get_singular_field_or_default(wire.as_ref()),
                ReflectValueRef::I32((0x6D20_u32.wrapping_sub(0xBEAC)) as i32)
            );
            let decoded: EnterSceneDoneRsp =
                decode_from_vec_by_name_version("7.2.0", "EnterSceneDoneRsp", &body).unwrap();
            assert_eq!((decoded.enter_scene_token, decoded.retcode), (token, 0));
        }
        let empty = encode_empty_by_name_version("7.2.0", "EnterSceneDoneRsp").unwrap();
        assert!(!empty.is_empty());
        let decoded: EnterSceneDoneRsp =
            decode_from_vec_by_name_version("7.2.0", "EnterSceneDoneRsp", &empty).unwrap();
        assert_eq!((decoded.enter_scene_token, decoded.retcode), (0, 0));
    }

    #[test]
    fn incoming_scene_token_is_unmasked_from_the_client_field() {
        profile();
        let description = descriptor("EnterSceneReadyReq");
        let mut wire = description.new_instance();
        let token = 0x10203040_u32;
        description
            .field_by_name("enter_scene_token")
            .unwrap()
            .set_singular_field(
                wire.as_mut(),
                ReflectValueBox::U32((token ^ 0xE8C3).wrapping_add(0x4C01)),
            );
        let decoded: EnterSceneReadyReq = decode_from_vec_by_name_version(
            "7.2.0",
            "EnterSceneReadyReq",
            &wire.write_to_bytes_dyn().unwrap(),
        )
        .unwrap();
        assert_eq!(decoded.enter_scene_token, token);
    }

    #[test]
    fn nested_map_layer_names_keep_all_three_lists() {
        profile();
        let original = PlayerWorldSceneInfo {
            scene_id: 3,
            map_layer_info: Some(MapLayerInfo {
                unlocked_map_layer_id_list: vec![301, 302],
                unlocked_layer_group_list: vec![3001],
                unlocked_map_layer_floor_id_list: vec![300101],
            }),
            ..Default::default()
        };
        let body =
            encode_to_vec_by_name_version("7.2.0", "PlayerWorldSceneInfo", &original).unwrap();
        let decoded: PlayerWorldSceneInfo =
            decode_from_vec_by_name_version("7.2.0", "PlayerWorldSceneInfo", &body).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn avatar_guids_keep_all_64_bits() {
        profile();
        let original = AvatarEnterSceneInfo {
            avatar_guid: u64::MAX,
            weapon_guid: u64::MAX - 1,
            ..Default::default()
        };
        let body =
            encode_to_vec_by_name_version("7.2.0", "AvatarEnterSceneInfo", &original).unwrap();
        let decoded: AvatarEnterSceneInfo =
            decode_from_vec_by_name_version("7.2.0", "AvatarEnterSceneInfo", &body).unwrap();
        assert_eq!(decoded.avatar_guid, original.avatar_guid);
        assert_eq!(decoded.weapon_guid, original.weapon_guid);
    }

    #[test]
    fn avatar_properties_keep_signed_64_bit_values_and_oneofs() {
        profile();
        let original = crate::normal::AvatarInfo {
            guid: u64::MAX,
            prop_map: HashMap::from([(
                4001,
                crate::normal::PropValue {
                    r#type: 4001,
                    val: i64::MIN,
                    value: Some(crate::normal::prop_value::Value::Ival(i64::MAX)),
                },
            )]),
            ..Default::default()
        };
        let body = encode_to_vec_by_name_version("7.2.0", "AvatarInfo", &original).unwrap();
        let decoded: crate::normal::AvatarInfo =
            decode_from_vec_by_name_version("7.2.0", "AvatarInfo", &body).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn avatar_entity_oneof_keeps_nested_integer_key_maps() {
        use crate::normal::{scene_entity_info, SceneAvatarInfo, SceneEntityInfo, SceneWeaponInfo};
        profile();
        let original = SceneEntityInfo {
            entity_type: 1,
            entity_id: 0x08000007,
            entity: Some(scene_entity_info::Entity::Avatar(SceneAvatarInfo {
                uid: 10001,
                avatar_id: 10000046,
                guid: u64::MAX,
                skill_level_map: HashMap::from([(101, 10)]),
                weapon: Some(SceneWeaponInfo {
                    guid: u64::MAX - 1,
                    affix_map: HashMap::from([(7, 4)]),
                    ..Default::default()
                }),
                ..Default::default()
            })),
            ..Default::default()
        };
        let body = encode_to_vec_by_name_version("7.2.0", "SceneEntityInfo", &original).unwrap();
        let decoded: SceneEntityInfo =
            decode_from_vec_by_name_version("7.2.0", "SceneEntityInfo", &body).unwrap();
        assert_eq!(decoded, original);
    }
}
