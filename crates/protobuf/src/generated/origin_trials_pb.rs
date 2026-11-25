#[derive(Clone, PartialEq, Eq, Hash, ::prost::Message)]
pub struct TrialTokenDbEntry {
    #[prost(bytes="vec", optional, tag="1")]
    pub trial_name: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(uint64, optional, tag="2")]
    pub token_expiry: ::core::option::Option<u64>,
    #[prost(uint32, optional, tag="3")]
    pub usage_restriction: ::core::option::Option<u32>,
    #[prost(bytes="vec", optional, tag="4")]
    pub token_signature: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes="vec", repeated, tag="5")]
    pub partition_sites: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    #[prost(bool, optional, tag="6")]
    pub match_subdomains: ::core::option::Option<bool>,
}
#[derive(Clone, PartialEq, Eq, Hash, ::prost::Message)]
pub struct OriginMessage {
    #[prost(bytes="vec", optional, tag="1")]
    pub scheme: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes="vec", optional, tag="2")]
    pub host: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(uint32, optional, tag="3")]
    pub port: ::core::option::Option<u32>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TrialTokenDbEntries {
    #[prost(message, optional, tag="1")]
    pub origin: ::core::option::Option<OriginMessage>,
    #[prost(message, repeated, tag="2")]
    pub tokens: ::prost::alloc::vec::Vec<TrialTokenDbEntry>,
}
