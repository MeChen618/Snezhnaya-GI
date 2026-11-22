use anyhow::{anyhow, bail, Context, Result};
use heck::ToSnakeCase;
use protobuf::reflect::{
    FieldDescriptor, MessageDescriptor, ReflectFieldRef, ReflectValueBox, RuntimeFieldType,
    RuntimeType,
};
use protobuf::{MessageDyn, UnknownValueRef};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

#[derive(Clone, PartialEq, prost::Message)]
struct ScalarTransform {
    #[prost(uint32, tag = "1")]
    bit_width: u32,
    #[prost(bool, tag = "2")]
    is_masked: bool,
    #[prost(string, tag = "3")]
    value_mask: String,
    #[prost(string, tag = "4")]
    value_unmask: String,
    #[prost(string, tag = "5")]
    evidence_rva: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ScalarMask {
    bits: u32,
    encode: [(u8, u64); 2],
    decode: [(u8, u64); 2],
}

impl ScalarMask {
    fn parse(expression: &str, bits: u32) -> Result<[(u8, u64); 2]> {
        static PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| {
            regex::Regex::new(
                r"^uint(32|64)\(\(\(value ([+\-^]) 0x([0-9A-Fa-f]+)\) ([+\-^]) 0x([0-9A-Fa-f]+)\)\)$",
            )
            .unwrap()
        });
        let caps = PATTERN
            .captures(expression)
            .ok_or_else(|| anyhow!("unsupported scalar transform: {expression}"))?;
        if caps[1].parse::<u32>()? != bits {
            bail!("scalar transform bit width mismatch");
        }
        Ok([
            (caps[2].as_bytes()[0], u64::from_str_radix(&caps[3], 16)?),
            (caps[4].as_bytes()[0], u64::from_str_radix(&caps[5], 16)?),
        ])
    }

    fn from_option(option: ScalarTransform) -> Result<Self> {
        let mask = Self {
            bits: option.bit_width,
            encode: Self::parse(&option.value_mask, option.bit_width)?,
            decode: Self::parse(&option.value_unmask, option.bit_width)?,
        };
        for value in [0, 1, u64::from(u32::MAX), u64::MAX] {
            if mask.apply(mask.apply(value, true), false) != mask.truncate(value) {
                bail!("scalar transform and inverse do not agree");
            }
        }
        Ok(mask)
    }

    fn truncate(&self, value: u64) -> u64 {
        if self.bits == 32 {
            u64::from(value as u32)
        } else {
            value
        }
    }

    fn apply(&self, mut value: u64, outgoing: bool) -> u64 {
        for (operation, constant) in if outgoing { self.encode } else { self.decode } {
            value = match operation {
                b'+' => value.wrapping_add(constant),
                b'-' => value.wrapping_sub(constant),
                b'^' => value ^ constant,
                _ => unreachable!(),
            };
        }
        self.truncate(value)
    }
}

pub(crate) type ScalarMasks = HashMap<String, ScalarMask>;

pub(crate) fn collect_masks(message: &MessageDescriptor, masks: &mut ScalarMasks) -> Result<()> {
    for field in message.fields() {
        let Some(options) = field.proto().options.as_ref() else {
            continue;
        };
        let Some(UnknownValueRef::LengthDelimited(bytes)) =
            options.special_fields.unknown_fields().get(51072)
        else {
            continue;
        };
        let option = <ScalarTransform as prost::Message>::decode(bytes)?;
        if option.is_masked {
            if !matches!(
                field.runtime_field_type(),
                RuntimeFieldType::Singular(
                    RuntimeType::U32 | RuntimeType::U64 | RuntimeType::I32 | RuntimeType::I64
                )
            ) {
                bail!("unsupported masked field: {field}");
            }
            masks.insert(
                field.full_name(),
                ScalarMask::from_option(option).with_context(|| field.full_name())?,
            );
        }
    }
    for nested in message.nested_messages() {
        collect_masks(&nested, masks)?;
    }
    Ok(())
}

fn transform_value(
    value: &mut ReflectValueBox,
    mask: Option<&ScalarMask>,
    masks: &ScalarMasks,
    outgoing: bool,
) -> Result<()> {
    match value {
        ReflectValueBox::Message(message) => transform(message.as_mut(), masks, outgoing)?,
        ReflectValueBox::U32(v) if mask.is_some() => {
            *v = mask.unwrap().apply(u64::from(*v), outgoing) as u32;
        }
        ReflectValueBox::U64(v) if mask.is_some() => *v = mask.unwrap().apply(*v, outgoing),
        ReflectValueBox::I32(v) if mask.is_some() => {
            *v = mask.unwrap().apply(*v as u32 as u64, outgoing) as u32 as i32;
        }
        ReflectValueBox::I64(v) if mask.is_some() => {
            *v = mask.unwrap().apply(*v as u64, outgoing) as i64;
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn transform(
    message: &mut dyn MessageDyn,
    masks: &ScalarMasks,
    outgoing: bool,
) -> Result<()> {
    for field in message.descriptor_dyn().fields() {
        let mask = masks.get(&field.full_name());
        match field.get_reflect(message) {
            ReflectFieldRef::Optional(value) => {
                // 掩码后的逻辑零也可能非零，需要写入协议。
                let value = value
                    .value()
                    .or_else(|| mask.map(|_| field.get_singular_field_or_default(message)));
                if let Some(value) = value {
                    let mut value = value.to_box();
                    transform_value(&mut value, mask, masks, outgoing)?;
                    field.set_singular_field(message, value);
                }
            }
            ReflectFieldRef::Repeated(values) => {
                let mut values: Vec<_> = values.into_iter().map(|v| v.to_box()).collect();
                for (index, value) in values.iter_mut().enumerate() {
                    transform_value(value, None, masks, outgoing)?;
                    field.mut_repeated(message).set(index, value.clone());
                }
            }
            ReflectFieldRef::Map(values) => {
                let mut values: Vec<_> = values
                    .into_iter()
                    .map(|(k, v)| (k.to_box(), v.to_box()))
                    .collect();
                for (key, value) in &mut values {
                    transform_value(value, None, masks, outgoing)?;
                    field.mut_map(message).insert(key.clone(), value.clone());
                }
            }
        }
    }
    Ok(())
}

fn canonical(name: &str) -> String {
    name.trim_start_matches('_').to_snake_case()
}

fn internal_field_name(message: &MessageDescriptor, field: &FieldDescriptor) -> String {
    match (message.name().trim_start_matches('_'), field.name()) {
        ("MapLayerInfo", "_unlock_map_layer_list") => "unlocked_map_layer_id_list".into(),
        ("MapLayerInfo", "_unlock_map_layer_group_list") => "unlocked_layer_group_list".into(),
        _ => canonical(field.name()),
    }
}

fn align_value(field: &FieldDescriptor, value: Value, outgoing: bool) -> Value {
    match field.runtime_field_type() {
        RuntimeFieldType::Singular(RuntimeType::I64) if !outgoing => signed_integer(value),
        RuntimeFieldType::Repeated(RuntimeType::I64) if !outgoing => match value {
            Value::Array(values) => Value::Array(values.into_iter().map(signed_integer).collect()),
            other => other,
        },
        RuntimeFieldType::Map(_, RuntimeType::I64) if !outgoing => match value {
            Value::Object(values) => Value::Object(
                values
                    .into_iter()
                    .map(|(key, value)| (key, signed_integer(value)))
                    .collect(),
            ),
            other => other,
        },
        RuntimeFieldType::Singular(RuntimeType::Message(message)) => {
            align_json(&message, value, outgoing)
        }
        RuntimeFieldType::Repeated(RuntimeType::Message(message)) => match value {
            Value::Array(values) => Value::Array(
                values
                    .into_iter()
                    .map(|v| align_json(&message, v, outgoing))
                    .collect(),
            ),
            other => other,
        },
        RuntimeFieldType::Map(_, RuntimeType::Message(message)) => match value {
            Value::Object(values) => Value::Object(
                values
                    .into_iter()
                    .map(|(k, v)| (k, align_json(&message, v, outgoing)))
                    .collect(),
            ),
            other => other,
        },
        _ => value,
    }
}

fn signed_integer(value: Value) -> Value {
    // protobuf JSON 将 int64 写成字符串，旧内部模型使用有符号整数。
    match value {
        Value::String(text) => match text.parse::<i64>() {
            Ok(number) => Value::from(number),
            Err(_) => Value::String(text),
        },
        other => other,
    }
}

pub(crate) fn align_json(message: &MessageDescriptor, value: Value, outgoing: bool) -> Value {
    let Value::Object(mut object) = value else {
        return value;
    };
    let mut result = Map::new();
    for field in message.fields() {
        let key = if outgoing {
            if object.contains_key(field.name()) {
                Some(field.name().to_owned())
            } else {
                let matches: Vec<_> = object
                    .keys()
                    .filter(|key| canonical(key) == internal_field_name(message, &field))
                    .cloned()
                    .collect();
                if matches.len() == 1 {
                    matches.into_iter().next()
                } else {
                    None
                }
            }
        } else {
            Some(field.name().to_owned())
        };
        if let Some(value) = key.and_then(|key| object.remove(&key)) {
            result.insert(
                if outgoing {
                    field.name().to_owned()
                } else {
                    internal_field_name(message, &field)
                },
                align_value(&field, value, outgoing),
            );
        }
    }
    Value::Object(result)
}

pub(crate) fn internal_value(
    message: &MessageDescriptor,
    value: Value,
) -> Result<serde_value::Value> {
    let Value::Object(mut object) = value else {
        return Ok(serde_value::to_value(value)?);
    };
    let mut output = BTreeMap::new();
    for field in message.fields() {
        let name = internal_field_name(message, &field);
        let Some(value) = object.remove(&name) else {
            continue;
        };
        let value = match field.runtime_field_type() {
            RuntimeFieldType::Singular(RuntimeType::Message(child)) => {
                internal_value(&child, value)?
            }
            RuntimeFieldType::Repeated(RuntimeType::Message(child)) => {
                let Value::Array(values) = value else {
                    bail!("expected message list: {field}");
                };
                serde_value::Value::Seq(
                    values
                        .into_iter()
                        .map(|value| internal_value(&child, value))
                        .collect::<Result<_>>()?,
                )
            }
            RuntimeFieldType::Map(key_type, value_type) => {
                let Value::Object(values) = value else {
                    bail!("expected map: {field}");
                };
                let mut map = BTreeMap::new();
                for (key, value) in values {
                    // oneof flatten 会先缓存键类型，数字键必须保留为整数。
                    let key = match key_type {
                        RuntimeType::U32 => serde_value::Value::U32(key.parse()?),
                        RuntimeType::I32 => serde_value::Value::I32(key.parse()?),
                        RuntimeType::I64 => serde_value::Value::I64(key.parse()?),
                        RuntimeType::Bool => serde_value::Value::Bool(key.parse()?),
                        // 旧模型的 uint64 键由字符串辅助器读取。
                        _ => serde_value::Value::String(key),
                    };
                    let value = match &value_type {
                        RuntimeType::Message(child) => internal_value(child, value)?,
                        _ => serde_value::to_value(value)?,
                    };
                    map.insert(key, value);
                }
                serde_value::Value::Map(map)
            }
            _ => serde_value::to_value(value)?,
        };
        output.insert(serde_value::Value::String(name), value);
    }
    Ok(serde_value::Value::Map(output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transforms_wrap_and_reject_unsupported_expressions() {
        let mask = ScalarMask::from_option(ScalarTransform {
            bit_width: 32,
            is_masked: true,
            value_mask: "uint32(((value ^ 0x4902) - 0xB3B6))".into(),
            value_unmask: "uint32(((value + 0xB3B6) ^ 0x4902))".into(),
            ..Default::default()
        })
        .unwrap();
        for value in [0_u32, 1, 123456, u32::MAX] {
            assert_eq!(
                mask.apply(value as u64, true),
                (value ^ 0x4902).wrapping_sub(0xB3B6) as u64
            );
            assert_eq!(
                mask.apply(mask.apply(value as u64, true), false),
                value as u64
            );
        }
        assert!(ScalarMask::parse("uint32(value)", 32).is_err());
        assert!(ScalarMask::parse("uint64(((value ^ 0x1) + 0x2))", 32).is_err());
    }
}
