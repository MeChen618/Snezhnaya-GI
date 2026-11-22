// disable lints on generated code
#![allow(unused, warnings)]

include!("../gen/normal/_.rs");

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TakeAllForgeOutputRsp {
    #[prost(int32, tag = "1")]
    pub retcode: i32,
    #[prost(message, repeated, tag = "2")]
    pub output_item_list: Vec<ItemHint>,
    #[prost(message, repeated, tag = "3")]
    pub extra_output_item_list: Vec<ItemHint>,
    #[prost(message, repeated, tag = "4")]
    pub return_item_list: Vec<ItemHint>,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ReliquaryFilterStateNotify {
    #[prost(message, repeated, tag = "1")]
    pub reliquary_filter_state_map: Vec<Uint32Pair>,
}

pub type ReliquaryFilterStateSaveNotify = ReliquaryFilterStateNotify;

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SetEquipLockStateBatchReq {
    #[prost(bool, tag = "1")]
    pub is_locked: bool,
    #[prost(uint64, repeated, tag = "2")]
    #[serde(with = "crate::u64_repeated_string")]
    pub equip_guid_list: Vec<u64>,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SetEquipLockStateBatchRsp {
    #[prost(int32, tag = "1")]
    pub retcode: i32,
    #[prost(bool, tag = "2")]
    pub is_locked: bool,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GetReliquaryTransferReq {
    #[prost(uint64, repeated, tag = "3")]
    #[serde(with = "crate::u64_repeated_string")]
    pub guid_list: Vec<u64>,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GetReliquaryTransferRsp {
    #[prost(uint32, tag = "4")]
    pub old_exp: u32,
    #[prost(int32, tag = "7")]
    pub retcode: i32,
    #[prost(message, repeated, tag = "8")]
    pub transfer_item_list: Vec<ItemParam>,
    #[prost(uint32, tag = "9")]
    pub add_exp: u32,
    #[prost(uint32, tag = "15")]
    pub remain_exp: u32,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GetReliquaryWearCustomDataReq {
    #[prost(uint32, tag = "5")]
    pub avatar_id: u32,
    #[prost(uint32, tag = "10")]
    pub avatar_element_type: u32,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ReliquaryWearCustomData {
    #[prost(uint32, repeated, tag = "1")]
    pub main_prop_type_list: Vec<u32>,
    #[prost(uint32, tag = "3")]
    pub avatar_element_type: u32,
    #[prost(uint32, tag = "4")]
    pub index: u32,
    #[prost(uint32, repeated, tag = "6")]
    pub reliquary_set_id_list: Vec<u32>,
    #[prost(bool, tag = "8")]
    pub is_all: bool,
    #[prost(message, repeated, tag = "10")]
    pub main_prop_type_map: Vec<Uint32Pair>,
    #[prost(uint32, tag = "11")]
    pub avatar_id: u32,
    #[prost(uint32, repeated, tag = "14")]
    pub append_prop_type_list: Vec<u32>,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GetReliquaryWearCustomDataRsp {
    #[prost(int32, tag = "5")]
    pub retcode: i32,
    #[prost(message, repeated, tag = "7")]
    pub custom_data_list: Vec<ReliquaryWearCustomData>,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SaveReliquaryWearCustomDataReq {
    #[prost(message, optional, tag = "1")]
    pub custom_data: Option<ReliquaryWearCustomData>,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SaveReliquaryWearCustomDataRsp {
    #[prost(int32, tag = "13")]
    pub retcode: i32,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct DeleteReliquaryWearCustomDataReq {
    #[prost(uint32, tag = "7")]
    pub index: u32,
    #[prost(uint32, tag = "9")]
    pub avatar_id: u32,
    #[prost(uint32, tag = "14")]
    pub avatar_element_type: u32,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GetReliquaryOfferingReq {
    #[prost(uint64, repeated, tag = "8")]
    #[serde(with = "crate::u64_repeated_string")]
    pub reliquary_guid_list: Vec<u64>,
    #[prost(uint32, tag = "10")]
    pub schedule_id: u32,
    #[prost(message, repeated, tag = "15")]
    pub item_list: Vec<ItemParam>,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GetReliquaryOfferingRsp {
    #[prost(uint32, tag = "4")]
    pub schedule_id: u32,
    #[prost(message, repeated, tag = "7")]
    pub item_list: Vec<ItemParam>,
    #[prost(uint64, repeated, tag = "14")]
    #[serde(with = "crate::u64_repeated_string")]
    pub reliquary_guid_list: Vec<u64>,
    #[prost(int32, tag = "15")]
    pub retcode: i32,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GetReliquaryPurchaseReq {
    #[prost(uint32, tag = "1")]
    pub config_id: u32,
    #[prost(uint32, repeated, tag = "4")]
    pub append_prop_type_list: Vec<u32>,
    #[prost(uint32, tag = "8")]
    pub main_prop_type: u32,
    #[prost(uint32, tag = "11")]
    pub reliquary_type: u32,
    #[prost(uint32, tag = "13")]
    pub schedule_id: u32,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GetReliquaryPurchaseRsp {
    #[prost(uint32, tag = "4")]
    pub schedule_id: u32,
    #[prost(uint32, tag = "5")]
    pub config_id: u32,
    #[prost(int32, tag = "11")]
    pub retcode: i32,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ReliquaryPurchaseRecord {
    #[prost(uint32, tag = "9")]
    pub purchase_count: u32,
    #[prost(uint32, tag = "15")]
    pub reliquary_set_id: u32,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GetReliquaryOfferPurchaseInfoNotify {
    #[prost(uint32, tag = "3")]
    pub offering_progress: u32,
    #[prost(float, tag = "4")]
    pub offering_progress_extra: f32,
    #[prost(uint32, tag = "5")]
    pub offering_count: u32,
    #[prost(uint32, tag = "6")]
    pub end_time: u32,
    #[prost(message, repeated, tag = "12")]
    pub purchase_record_list: Vec<ReliquaryPurchaseRecord>,
    #[prost(uint32, tag = "14")]
    pub schedule_id: u32,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GetReliquaryDustReq {
    #[prost(uint32, repeated, tag = "10")]
    pub append_prop_type_list: Vec<u32>,
    #[prost(uint64, tag = "14")]
    #[serde(with = "crate::u64_string")]
    pub guid: u64,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GetReliquaryDustRsp {
    #[prost(int32, tag = "3")]
    pub retcode: i32,
    #[prost(uint64, tag = "5")]
    #[serde(with = "crate::u64_string")]
    pub guid: u64,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GetReliquaryDustSelectReq {
    #[prost(bool, tag = "4")]
    pub is_use_new: bool,
    #[prost(uint64, tag = "8")]
    #[serde(with = "crate::u64_string")]
    pub guid: u64,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GetReliquaryDustSelectRsp {
    #[prost(int32, tag = "15")]
    pub retcode: i32,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ReliquaryReforgeRecord {
    #[prost(uint64, tag = "5")]
    #[serde(with = "crate::u64_string")]
    pub guid: u64,
    #[prost(uint32, repeated, tag = "7")]
    pub append_prop_type_list: Vec<u32>,
    #[prost(uint32, repeated, tag = "10")]
    pub append_prop_id_list: Vec<u32>,
}

#[derive(Clone, PartialEq, ::prost::Message, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ReliquaryReforgeDataNotify {
    #[prost(uint32, tag = "1")]
    pub cur_progress: u32,
    #[prost(message, repeated, tag = "4")]
    pub reforge_record_list: Vec<ReliquaryReforgeRecord>,
}
