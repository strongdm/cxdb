// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use serde::Deserialize;

use crate::sse_decode::{SseInt64, SseUint32, SseUint64};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextCreatedEvent {
    pub context_id: u64,
    pub session_id: String,
    pub client_tag: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMetadataUpdatedEvent {
    pub context_id: u64,
    pub has_provenance: bool,
    pub client_tag: String,
    pub title: String,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnAppendedEvent {
    pub context_id: u64,
    pub turn_id: u64,
    pub parent_turn_id: u64,
    pub depth: u32,
    pub declared_type_id: String,
    pub declared_type_version: u32,
    pub has_declared_type_id: bool,
    pub has_declared_type_ver: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConnectedEvent {
    pub session_id: String,
    pub client_tag: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientDisconnectedEvent {
    pub session_id: String,
    pub client_tag: String,
    pub contexts: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ContextCreatedPayload {
    #[serde(default)]
    context_id: SseUint64,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    client_tag: String,
    #[serde(default)]
    created_at: SseInt64,
}

#[derive(Debug, Deserialize)]
struct ContextMetadataUpdatedPayload {
    #[serde(default)]
    context_id: SseUint64,
    #[serde(default)]
    has_provenance: bool,
    #[serde(default)]
    client_tag: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    labels: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct TurnAppendedPayload {
    #[serde(default)]
    context_id: SseUint64,
    #[serde(default)]
    turn_id: SseUint64,
    #[serde(default)]
    parent_turn_id: SseUint64,
    #[serde(default)]
    depth: SseUint32,
    #[serde(default)]
    declared_type_id: Option<String>,
    #[serde(default)]
    declared_type_version: Option<SseUint32>,
}

#[derive(Debug, Deserialize)]
struct ClientConnectedPayload {
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    client_tag: String,
}

#[derive(Debug, Deserialize)]
struct ClientDisconnectedPayload {
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    client_tag: String,
    #[serde(default)]
    contexts: Vec<String>,
}

pub fn decode_context_created(data: &[u8]) -> Result<ContextCreatedEvent, serde_json::Error> {
    let payload: ContextCreatedPayload = serde_json::from_slice(data)?;
    Ok(ContextCreatedEvent {
        context_id: payload.context_id.value,
        session_id: payload.session_id,
        client_tag: payload.client_tag,
        created_at: payload.created_at.value,
    })
}

pub fn decode_context_metadata_updated(
    data: &[u8],
) -> Result<ContextMetadataUpdatedEvent, serde_json::Error> {
    let payload: ContextMetadataUpdatedPayload = serde_json::from_slice(data)?;
    Ok(ContextMetadataUpdatedEvent {
        context_id: payload.context_id.value,
        has_provenance: payload.has_provenance,
        client_tag: payload.client_tag,
        title: payload.title,
        labels: payload.labels,
    })
}

pub fn decode_turn_appended(data: &[u8]) -> Result<TurnAppendedEvent, serde_json::Error> {
    let payload: TurnAppendedPayload = serde_json::from_slice(data)?;
    let declared_type_id = payload.declared_type_id.unwrap_or_default();
    let has_declared_type_id = !declared_type_id.is_empty();
    let (declared_type_version, has_declared_type_ver) = match payload.declared_type_version {
        Some(ver) => (ver.value, true),
        None => (0, false),
    };
    Ok(TurnAppendedEvent {
        context_id: payload.context_id.value,
        turn_id: payload.turn_id.value,
        parent_turn_id: payload.parent_turn_id.value,
        depth: payload.depth.value,
        declared_type_id,
        declared_type_version,
        has_declared_type_id,
        has_declared_type_ver,
    })
}

pub fn decode_client_connected(data: &[u8]) -> Result<ClientConnectedEvent, serde_json::Error> {
    let payload: ClientConnectedPayload = serde_json::from_slice(data)?;
    Ok(ClientConnectedEvent {
        session_id: payload.session_id,
        client_tag: payload.client_tag,
    })
}

pub fn decode_client_disconnected(
    data: &[u8],
) -> Result<ClientDisconnectedEvent, serde_json::Error> {
    let payload: ClientDisconnectedPayload = serde_json::from_slice(data)?;
    Ok(ClientDisconnectedEvent {
        session_id: payload.session_id,
        client_tag: payload.client_tag,
        contexts: payload.contexts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_context_created_fields() {
        let input = br#"{"context_id":"42","session_id":"sess-abc","client_tag":"ai-staff","created_at":1739481600000}"#;
        let ev = decode_context_created(input).expect("decode context_created");
        assert_eq!(ev.context_id, 42);
        assert_eq!(ev.session_id, "sess-abc");
        assert_eq!(ev.client_tag, "ai-staff");
        assert_eq!(ev.created_at, 1739481600000);
    }

    #[test]
    fn decode_turn_appended_optional_fields() {
        let input = br#"{"context_id":7,"turn_id":"9","parent_turn_id":"8","depth":10}"#;
        let ev = decode_turn_appended(input).expect("decode turn_appended");
        assert_eq!(ev.context_id, 7);
        assert_eq!(ev.turn_id, 9);
        assert_eq!(ev.parent_turn_id, 8);
        assert_eq!(ev.depth, 10);
        assert!(!ev.has_declared_type_id);
        assert!(!ev.has_declared_type_ver);
    }
}
