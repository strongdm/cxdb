//! Phase 3 integration tests: drive captured fixtures through the
//! `SessionRuntime` and assert the stored `ConversationItem.metrics`
//! carries real numbers with the right `usage_status`.
//!
//! Also covers Phase 3 round-trip sanity for the additive
//! `TurnMetrics.usage_status` field.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use cxdb::types::TurnMetrics;
use cxtx::provider::usage::{
    anthropic_sse_message_delta_outcome, openai_chat_terminal_chunk_outcome,
};
use cxtx::provider::ProviderKind;
use cxtx::session::SessionRuntime;
use cxtx::turns::{ArtifactRefs, HistoryItem};
use serde_json::Value;

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("usage")
}

fn read_json(path: &std::path::Path) -> Value {
    let body = fs::read_to_string(path).expect("read fixture");
    serde_json::from_str(&body).expect("parse fixture JSON")
}

/// P3-T1: a happy-path Anthropic SSE fixture flows through to a stored
/// assistant turn with real token counts and `usage_status == None`.
#[test]
fn anthropic_happy_flow_stamps_real_tokens() {
    let event = read_json(&fixtures_root().join("anthropic_sse_happy/event.json"));
    let outcome = anthropic_sse_message_delta_outcome(&event);

    let session =
        SessionRuntime::new(ProviderKind::Claude, Vec::new(), BTreeMap::new()).unwrap();

    // Seed the request prefix.
    let _ = session.observe_request_history(
        "exchange-0001",
        vec![HistoryItem::UserInput {
            text: "hello".to_string(),
            files: Vec::new(),
        }],
        &ArtifactRefs::default(),
    );

    let turn = session.append_history_item(
        "exchange-0001",
        HistoryItem::AssistantTurn {
            text: "ok".to_string(),
            tool_calls: Vec::new(),
            model: Some("claude-placeholder".to_string()),
            finish_reason: Some("end_turn".to_string()),
            usage: Some(outcome),
        },
    );

    let metrics = turn
        .item
        .turn
        .as_ref()
        .and_then(|t| t.metrics.as_ref())
        .expect("metrics stamped on assistant turn");
    assert!(
        metrics.input_tokens > 0,
        "expected positive input_tokens, got {metrics:?}"
    );
    assert_eq!(metrics.input_tokens, 120);
    assert_eq!(metrics.output_tokens, 45);
    assert_eq!(metrics.total_tokens, 165);
    assert_eq!(metrics.usage_status, None, "Reported → None");
    assert_eq!(metrics.model, "claude-placeholder");
}

/// P3-T2: OpenAI ChatCompletions without `include_usage` → stored turn
/// carries zero tokens and `usage_status == Some("not_reported")`.
#[test]
fn openai_chat_no_usage_stamps_not_reported() {
    let terminal = read_json(
        &fixtures_root().join("openai_sse_chatcompletions_no_usage/terminal_chunk.json"),
    );
    let outcome = openai_chat_terminal_chunk_outcome(&terminal, Vec::new());

    let session =
        SessionRuntime::new(ProviderKind::Codex, Vec::new(), BTreeMap::new()).unwrap();
    let _ = session.observe_request_history(
        "exchange-0001",
        vec![HistoryItem::UserInput {
            text: "hi".to_string(),
            files: Vec::new(),
        }],
        &ArtifactRefs::default(),
    );
    let turn = session.append_history_item(
        "exchange-0001",
        HistoryItem::AssistantTurn {
            text: "ok".to_string(),
            tool_calls: Vec::new(),
            model: Some("gpt-placeholder".to_string()),
            finish_reason: Some("stop".to_string()),
            usage: Some(outcome),
        },
    );

    let metrics = turn
        .item
        .turn
        .as_ref()
        .and_then(|t| t.metrics.as_ref())
        .expect("metrics stamped on assistant turn");
    assert_eq!(metrics.input_tokens, 0);
    assert_eq!(metrics.output_tokens, 0);
    assert_eq!(metrics.usage_status.as_deref(), Some("not_reported"));
}

/// P3-T4: TurnMetrics round-trip — encode a post-sprint `TurnMetrics`
/// with `usage_status: Some("not_reported")` to msgpack, decode via the
/// stable serde shape, confirm the new field is preserved and existing
/// fields are untouched.
#[test]
fn turn_metrics_roundtrip_preserves_usage_status() {
    let metrics = TurnMetrics {
        input_tokens: 42,
        output_tokens: 7,
        total_tokens: 49,
        cached_tokens: Some(5),
        reasoning_tokens: Some(1),
        duration_ms: Some(123),
        model: "placeholder".to_string(),
        usage_status: Some("not_reported".to_string()),
    };

    let bytes = rmp_serde::to_vec_named(&metrics).expect("encode");
    let decoded: TurnMetrics = rmp_serde::from_slice(&bytes).expect("decode");
    assert_eq!(decoded, metrics);

    // Also verify the field is numerically renamed to "8" (per the additive
    // field tag assigned in Phase 2). Walk the msgpack with rmpv to make
    // sure the bytes carry key "8" → "not_reported".
    let raw: rmpv::Value = rmp_serde::from_slice(&bytes).expect("rmpv decode");
    let map = match &raw {
        rmpv::Value::Map(m) => m,
        other => panic!("expected Map, got {other:?}"),
    };
    let has_field_8 = map.iter().any(|(k, v)| {
        k.as_str() == Some("8")
            && matches!(v, rmpv::Value::String(s) if s.as_str() == Some("not_reported"))
    });
    assert!(has_field_8, "msgpack field '8' missing: {:?}", map);
}

/// Round-trip sanity: a TurnMetrics with usage_status=None skips the
/// field entirely, preserving backward compatibility with pre-sprint
/// readers.
#[test]
fn turn_metrics_roundtrip_skips_none_usage_status() {
    let metrics = TurnMetrics {
        input_tokens: 10,
        output_tokens: 1,
        total_tokens: 11,
        cached_tokens: None,
        reasoning_tokens: None,
        duration_ms: None,
        model: "placeholder".to_string(),
        usage_status: None,
    };

    let bytes = rmp_serde::to_vec_named(&metrics).expect("encode");
    let raw: rmpv::Value = rmp_serde::from_slice(&bytes).expect("rmpv decode");
    let map = match &raw {
        rmpv::Value::Map(m) => m,
        other => panic!("expected Map, got {other:?}"),
    };
    let has_field_8 = map.iter().any(|(k, _)| k.as_str() == Some("8"));
    assert!(
        !has_field_8,
        "field 8 should be skipped when usage_status=None, got {:?}",
        map
    );
}
