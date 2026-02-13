// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

//! Rust CXDB client library with Go parity.
//!
//! Exposes a synchronous TCP/TLS client, reconnecting wrapper, fstree snapshots,
//! SSE subscription helpers, and canonical conversation types plus msgpack helpers.

pub mod client;
pub mod context;
pub mod encoding;
pub mod error;
pub mod events;
pub mod follow;
pub mod fs;
pub mod protocol;
pub mod reconnect;
mod sse_decode;
pub mod subscribe;
pub mod telemetry;
pub mod turn;

pub mod fstree;
pub mod types;

#[cfg(test)]
mod test_util;
pub use crate::client::{
    dial, dial_tls, with_client_tag, with_dial_timeout, with_request_timeout, Client, ClientOption,
    RequestContext,
};
pub use crate::context::ContextHead;
pub use crate::encoding::{decode_msgpack, decode_msgpack_into, encode_msgpack};
pub use crate::error::{is_server_error, Error, Result, ServerError};
pub use crate::events::{
    decode_client_connected, decode_client_disconnected, decode_context_created,
    decode_context_metadata_updated, decode_turn_appended, ClientConnectedEvent,
    ClientDisconnectedEvent, ContextCreatedEvent, ContextMetadataUpdatedEvent, TurnAppendedEvent,
};
pub use crate::follow::{
    follow_turns, with_follow_buffer, with_max_seen_per_context, FollowError, FollowOption,
    FollowTurn, TurnClient,
};
pub use crate::fs::{AttachFsRequest, AttachFsResult, PutBlobRequest, PutBlobResult};
pub use crate::reconnect::{
    dial_reconnecting, dial_tls_reconnecting, DialFunc, ReconnectOption, ReconnectingClient,
};
pub use crate::subscribe::{
    subscribe_events, with_error_buffer, with_event_buffer, with_headers, with_http_client,
    with_max_event_bytes, with_subscribe_max_retry_delay, with_subscribe_retry_delay, Event,
    SubscribeError, SubscribeOption,
};
pub use crate::turn::{AppendRequest, AppendResult, GetLastOptions, TurnRecord};

// Re-export shared constants for parity with Go names.
#[allow(non_upper_case_globals)]
pub const EncodingMsgpack: u32 = protocol::ENCODING_MSGPACK;
#[allow(non_upper_case_globals)]
pub const CompressionNone: u32 = protocol::COMPRESSION_NONE;
#[allow(non_upper_case_globals)]
pub const CompressionZstd: u32 = protocol::COMPRESSION_ZSTD;

#[allow(non_upper_case_globals)]
pub const DefaultDialTimeout: std::time::Duration = protocol::DEFAULT_DIAL_TIMEOUT;
#[allow(non_upper_case_globals)]
pub const DefaultRequestTimeout: std::time::Duration = protocol::DEFAULT_REQUEST_TIMEOUT;

#[allow(non_upper_case_globals)]
pub const DefaultMaxRetries: usize = reconnect::DEFAULT_MAX_RETRIES;
#[allow(non_upper_case_globals)]
pub const DefaultRetryDelay: std::time::Duration = reconnect::DEFAULT_RETRY_DELAY;
#[allow(non_upper_case_globals)]
pub const DefaultMaxRetryDelay: std::time::Duration = reconnect::DEFAULT_MAX_RETRY_DELAY;
#[allow(non_upper_case_globals)]
pub const DefaultQueueSize: usize = reconnect::DEFAULT_QUEUE_SIZE;

#[allow(non_snake_case)]
pub fn SubscribeEvents(
    ctx: &RequestContext,
    url: &str,
    opts: impl IntoIterator<Item = SubscribeOption>,
) -> (
    crossbeam_channel::Receiver<Event>,
    crossbeam_channel::Receiver<SubscribeError>,
) {
    subscribe_events(ctx, url, opts)
}

#[allow(non_snake_case)]
pub fn FollowTurns(
    ctx: &RequestContext,
    events: crossbeam_channel::Receiver<Event>,
    client: std::sync::Arc<dyn TurnClient>,
    opts: impl IntoIterator<Item = FollowOption>,
) -> (
    crossbeam_channel::Receiver<FollowTurn>,
    crossbeam_channel::Receiver<FollowError>,
) {
    follow_turns(ctx, events, client, opts)
}

#[allow(non_snake_case)]
pub fn WithHTTPClient(agent: ureq::Agent) -> SubscribeOption {
    with_http_client(agent)
}

#[allow(non_snake_case)]
pub fn WithHeaders<I, K, V>(headers: I) -> SubscribeOption
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    with_headers(headers)
}

#[allow(non_snake_case)]
pub fn WithMaxEventBytes(n: usize) -> SubscribeOption {
    with_max_event_bytes(n)
}

#[allow(non_snake_case)]
pub fn WithEventBuffer(n: usize) -> SubscribeOption {
    with_event_buffer(n)
}

#[allow(non_snake_case)]
pub fn WithErrorBuffer(n: usize) -> SubscribeOption {
    with_error_buffer(n)
}

#[allow(non_snake_case)]
pub fn WithSubscribeRetryDelay(delay: std::time::Duration) -> SubscribeOption {
    with_subscribe_retry_delay(delay)
}

#[allow(non_snake_case)]
pub fn WithSubscribeMaxRetryDelay(delay: std::time::Duration) -> SubscribeOption {
    with_subscribe_max_retry_delay(delay)
}

#[allow(non_snake_case)]
pub fn WithFollowBuffer(size: usize) -> FollowOption {
    with_follow_buffer(size)
}

#[allow(non_snake_case)]
pub fn WithMaxSeenPerContext(limit: usize) -> FollowOption {
    with_max_seen_per_context(limit)
}

/// Go-parity alias for client options.
pub type Option = ClientOption;

#[allow(non_snake_case)]
pub fn IsServerError(err: &Error, code: u32) -> bool {
    is_server_error(err, code)
}

#[allow(non_snake_case)]
pub fn WithDialTimeout(timeout: std::time::Duration) -> ClientOption {
    with_dial_timeout(timeout)
}

#[allow(non_snake_case)]
pub fn WithRequestTimeout(timeout: std::time::Duration) -> ClientOption {
    with_request_timeout(timeout)
}

#[allow(non_snake_case)]
pub fn WithClientTag(tag: impl Into<String>) -> ClientOption {
    with_client_tag(tag)
}

#[allow(non_snake_case)]
pub fn Dial(addr: &str, opts: impl IntoIterator<Item = ClientOption>) -> Result<Client> {
    dial(addr, opts)
}

#[allow(non_snake_case)]
pub fn DialTLS(addr: &str, opts: impl IntoIterator<Item = ClientOption>) -> Result<Client> {
    dial_tls(addr, opts)
}

#[allow(non_snake_case)]
pub fn DialReconnecting(
    addr: &str,
    reconnect_opts: impl IntoIterator<Item = ReconnectOption>,
    opts: impl IntoIterator<Item = ClientOption>,
) -> Result<ReconnectingClient> {
    dial_reconnecting(addr, reconnect_opts, opts)
}

#[allow(non_snake_case)]
pub fn DialTLSReconnecting(
    addr: &str,
    reconnect_opts: impl IntoIterator<Item = ReconnectOption>,
    opts: impl IntoIterator<Item = ClientOption>,
) -> Result<ReconnectingClient> {
    dial_tls_reconnecting(addr, reconnect_opts, opts)
}
