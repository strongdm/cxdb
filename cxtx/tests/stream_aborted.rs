//! Regression tests for upstream stream-abort classification.
//!
//! When the proxy's SSE read loop hits a transport error mid-stream, the
//! exchange state must be tagged so `finalize_stream` emits OTEL
//! `Error(StreamAborted)` and the stored assistant turn's `usage_status`
//! reflects the same class. Without this, the original 2xx upstream
//! response status routes the call through the happy path and OTEL
//! receives `NotReported` — losing the abort signal that the
//! cost-attribution KPI depends on for unhappy paths.

use std::collections::BTreeMap;

use cxtx::provider::openai::SseFrame;
use cxtx::provider::ProviderKind;
use cxtx::session::SessionRuntime;
use cxtx::turns::ArtifactRefs;

fn openai_request_body() -> &'static [u8] {
    br#"{"model":"gpt-5","stream":true,"messages":[{"role":"user","content":"hi"}]}"#
}

fn anthropic_request_body() -> &'static [u8] {
    br#"{"model":"claude-sonnet-4-6","stream":true,"messages":[{"role":"user","content":"hi"}]}"#
}

#[test]
fn openai_partial_stream_abort_stamps_error_stream_aborted() {
    let session =
        SessionRuntime::new(ProviderKind::Codex, Vec::new(), BTreeMap::new()).unwrap();

    let mut prepared = ProviderKind::Codex.prepare_exchange(
        &session,
        "ex-openai-aborted".to_string(),
        openai_request_body(),
        &ArtifactRefs::default(),
    );

    // Some text streamed before the upstream connection died.
    prepared.state.absorb_sse_frame(&SseFrame {
        event: None,
        data: r#"{"choices":[{"delta":{"content":"par"}}]}"#.to_string(),
        raw: String::new(),
    });

    prepared
        .state
        .mark_stream_aborted("connection reset by peer".to_string());

    // Original upstream status was 2xx — the abort happens AFTER headers
    // are received. The fix's job is to make sure status=200 doesn't
    // route this through the happy path.
    let turns = prepared.state.finalize_stream(
        &session,
        200,
        None,
        &ArtifactRefs::default(),
        None,
    );

    assert_eq!(turns.len(), 1, "partial content must yield one assistant turn");
    let metrics = turns[0]
        .item
        .turn
        .as_ref()
        .and_then(|t| t.metrics.as_ref())
        .expect("metrics stamped on partial assistant turn");
    assert_eq!(
        metrics.usage_status.as_deref(),
        Some("error:StreamAborted"),
        "stream_aborted state must classify the stored turn as error:StreamAborted, \
         got {:?}",
        metrics.usage_status
    );
}

#[test]
fn openai_empty_stream_abort_yields_no_assistant_turn() {
    let session =
        SessionRuntime::new(ProviderKind::Codex, Vec::new(), BTreeMap::new()).unwrap();

    let mut prepared = ProviderKind::Codex.prepare_exchange(
        &session,
        "ex-openai-empty-aborted".to_string(),
        openai_request_body(),
        &ArtifactRefs::default(),
    );
    prepared
        .state
        .mark_stream_aborted("eof before any frame".to_string());

    let turns = prepared.state.finalize_stream(
        &session,
        200,
        None,
        &ArtifactRefs::default(),
        None,
    );

    // The proxy emits the user-visible `stream_transport_error` system
    // turn separately; finalize_stream's only job on an aborted, empty
    // exchange is to push the OTEL classification, not double-record.
    assert!(
        turns.is_empty(),
        "empty content + abort must not synthesize a placeholder assistant turn"
    );
}

#[test]
fn anthropic_partial_stream_abort_stamps_error_stream_aborted() {
    let session =
        SessionRuntime::new(ProviderKind::Claude, Vec::new(), BTreeMap::new()).unwrap();

    let mut prepared = ProviderKind::Claude.prepare_exchange(
        &session,
        "ex-anthropic-aborted".to_string(),
        anthropic_request_body(),
        &ArtifactRefs::default(),
    );

    prepared.state.absorb_sse_frame(&SseFrame {
        event: Some("message_start".to_string()),
        data: r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-sonnet-4-6","content":[]}}"#.to_string(),
        raw: String::new(),
    });
    prepared.state.absorb_sse_frame(&SseFrame {
        event: Some("content_block_start".to_string()),
        data: r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#.to_string(),
        raw: String::new(),
    });
    prepared.state.absorb_sse_frame(&SseFrame {
        event: Some("content_block_delta".to_string()),
        data: r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"par"}}"#.to_string(),
        raw: String::new(),
    });

    prepared
        .state
        .mark_stream_aborted("connection reset by peer".to_string());

    let turns = prepared.state.finalize_stream(
        &session,
        200,
        None,
        &ArtifactRefs::default(),
        None,
    );

    assert_eq!(turns.len(), 1, "partial content must yield one assistant turn");
    let metrics = turns[0]
        .item
        .turn
        .as_ref()
        .and_then(|t| t.metrics.as_ref())
        .expect("metrics stamped on partial assistant turn");
    assert_eq!(
        metrics.usage_status.as_deref(),
        Some("error:StreamAborted"),
        "stream_aborted state must classify the stored turn as error:StreamAborted, \
         got {:?}",
        metrics.usage_status
    );
}

#[test]
fn anthropic_empty_stream_abort_yields_no_assistant_turn() {
    let session =
        SessionRuntime::new(ProviderKind::Claude, Vec::new(), BTreeMap::new()).unwrap();

    let mut prepared = ProviderKind::Claude.prepare_exchange(
        &session,
        "ex-anthropic-empty-aborted".to_string(),
        anthropic_request_body(),
        &ArtifactRefs::default(),
    );
    prepared
        .state
        .mark_stream_aborted("eof before any frame".to_string());

    let turns = prepared.state.finalize_stream(
        &session,
        200,
        None,
        &ArtifactRefs::default(),
        None,
    );

    assert!(
        turns.is_empty(),
        "empty content + abort must not synthesize a placeholder assistant turn"
    );
}
