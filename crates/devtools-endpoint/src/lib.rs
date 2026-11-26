//! The DevTools HTTP and WebSocket endpoint and the golden-box plumbing that exercises it.

pub mod canonical;
pub mod cbor;
pub mod cdp;
pub mod compression;
pub mod connection;
pub mod dispatchable;
pub mod endpoint;
pub mod handler;
pub mod json;
pub mod json_parser;
pub mod launch;
pub mod read_buffer;
pub mod registry;
pub mod rename;
pub mod request;
pub mod response;
pub mod status;
pub mod url;
pub mod websocket;
pub mod wire;
