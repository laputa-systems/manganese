#[derive(Clone, Copy, PartialEq, Eq, Hash, ::prost::Message)]
pub struct PeriodicSyncOptions {
    #[prost(int64, optional, tag="1")]
    pub min_interval: ::core::option::Option<i64>,
}
#[derive(Clone, PartialEq, Eq, Hash, ::prost::Message)]
pub struct BackgroundSyncRegistrationProto {
    #[prost(bytes="vec", required, tag="2")]
    pub tag: ::prost::alloc::vec::Vec<u8>,
    #[prost(int32, required, tag="7")]
    pub num_attempts: i32,
    #[prost(int64, required, tag="8")]
    pub delay_until: i64,
    #[prost(message, optional, tag="9")]
    pub periodic_sync_options: ::core::option::Option<PeriodicSyncOptions>,
    #[prost(int32, optional, tag="10")]
    pub max_attempts: ::core::option::Option<i32>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct BackgroundSyncRegistrationsProto {
    #[prost(message, repeated, tag="1")]
    pub registration: ::prost::alloc::vec::Vec<BackgroundSyncRegistrationProto>,
    #[prost(bytes="vec", required, tag="3")]
    pub origin: ::prost::alloc::vec::Vec<u8>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum SyncNetworkState {
    NetworkStateAny = 0,
    NetworkStateAvoidCellular = 1,
    NetworkStateOnline = 2,
}
impl SyncNetworkState {
    pub fn as_str_name(&self) -> &'static str {
        match self {
            Self::NetworkStateAny => "NETWORK_STATE_ANY",
            Self::NetworkStateAvoidCellular => "NETWORK_STATE_AVOID_CELLULAR",
            Self::NetworkStateOnline => "NETWORK_STATE_ONLINE",
        }
    }
    pub fn from_str_name(value: &str) -> ::core::option::Option<Self> {
        match value {
            "NETWORK_STATE_ANY" => Some(Self::NetworkStateAny),
            "NETWORK_STATE_AVOID_CELLULAR" => Some(Self::NetworkStateAvoidCellular),
            "NETWORK_STATE_ONLINE" => Some(Self::NetworkStateOnline),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, ::prost::Message)]
pub struct ImageSizeProto {
    #[prost(uint64, optional, tag="1")]
    pub width: ::core::option::Option<u64>,
    #[prost(uint64, optional, tag="2")]
    pub height: ::core::option::Option<u64>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct StoredPaymentInstrumentImageObject {
    #[prost(bytes="vec", optional, tag="1")]
    pub src: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes="vec", optional, tag="2")]
    pub r#type: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(message, repeated, tag="3")]
    pub sizes: ::prost::alloc::vec::Vec<ImageSizeProto>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct StoredPaymentInstrumentProto {
    #[prost(int64, optional, tag="1")]
    pub registration_id: ::core::option::Option<i64>,
    #[prost(bytes="vec", optional, tag="2")]
    pub instrument_key: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes="vec", optional, tag="3")]
    pub name: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes="vec", optional, tag="4")]
    pub method: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes="vec", optional, tag="5")]
    pub stringified_capabilities: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(message, repeated, tag="6")]
    pub icons: ::prost::alloc::vec::Vec<StoredPaymentInstrumentImageObject>,
    #[prost(bytes="vec", optional, tag="7")]
    pub decoded_instrument_icon: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(int32, repeated, packed="false", tag="8")]
    pub supported_card_networks: ::prost::alloc::vec::Vec<i32>,
}
#[derive(Clone, PartialEq, Eq, Hash, ::prost::Message)]
pub struct StoredRelatedApplicationProto {
    #[prost(bytes="vec", optional, tag="1")]
    pub platform: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes="vec", optional, tag="2")]
    pub id: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, ::prost::Message)]
pub struct SupportedDelegationsProto {
    #[prost(bool, optional, tag="1")]
    pub shipping_address: ::core::option::Option<bool>,
    #[prost(bool, optional, tag="2")]
    pub payer_name: ::core::option::Option<bool>,
    #[prost(bool, optional, tag="3")]
    pub payer_phone: ::core::option::Option<bool>,
    #[prost(bool, optional, tag="4")]
    pub payer_email: ::core::option::Option<bool>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct StoredPaymentAppProto {
    #[prost(int64, optional, tag="1")]
    pub registration_id: ::core::option::Option<i64>,
    #[prost(bytes="vec", optional, tag="2")]
    pub scope: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes="vec", optional, tag="3")]
    pub name: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes="vec", optional, tag="4")]
    pub icon: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bool, optional, tag="5")]
    pub prefer_related_applications: ::core::option::Option<bool>,
    #[prost(message, repeated, tag="6")]
    pub related_applications: ::prost::alloc::vec::Vec<StoredRelatedApplicationProto>,
    #[prost(bytes="vec", optional, tag="7")]
    pub user_hint: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(message, optional, tag="8")]
    pub supported_delegations: ::core::option::Option<SupportedDelegationsProto>,
}
