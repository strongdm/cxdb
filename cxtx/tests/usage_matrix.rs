//! Phase 2 & Phase 4 fixture-based parser matrix.
//!
//! Each subdirectory under `cxtx/tests/fixtures/usage/` is one row of the
//! 16-cell provider-matrix documented in Sprint 016. This test iterates
//! every subdirectory, dispatches on the `kind` field in `expected.json`,
//! feeds the input through the corresponding `cxtx::provider::usage` entry
//! point, and asserts exact `UsageOutcome` equality.

use std::path::{Path, PathBuf};

use cxtx::provider::usage::{
    anthropic_json_body_outcome, anthropic_sse_message_delta_outcome, ErrorClass,
    openai_chat_json_body_outcome, openai_chat_terminal_chunk_outcome,
    openai_responses_completed_outcome, openai_responses_json_body_outcome, UsageOutcome,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct ExpectedFile {
    kind: String,
    outcome: UsageOutcome,
}

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("usage")
}

fn read_json(path: &Path) -> Value {
    let body = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("parse {} as JSON: {e}", path.display()))
}

fn read_expected(dir: &Path) -> ExpectedFile {
    let path = dir.join("expected.json");
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("parse {} as ExpectedFile: {e}", path.display()))
}

fn run_fixture(dir: &Path) {
    let expected = read_expected(dir);

    let actual = match expected.kind.as_str() {
        "anthropic_sse_message_delta" => {
            let event = read_json(&dir.join("event.json"));
            anthropic_sse_message_delta_outcome(&event)
        }
        "anthropic_json_body" => {
            let body = read_json(&dir.join("body.json"));
            anthropic_json_body_outcome(&body)
        }
        "openai_chat_terminal_chunk" => {
            let terminal = read_json(&dir.join("terminal_chunk.json"));
            let acc: Vec<String> = serde_json::from_str(
                &std::fs::read_to_string(dir.join("accumulated_finish_reasons.json"))
                    .expect("accumulated_finish_reasons.json"),
            )
            .expect("parse accumulated_finish_reasons.json");
            openai_chat_terminal_chunk_outcome(&terminal, acc)
        }
        "openai_responses_completed" => {
            let event = read_json(&dir.join("event.json"));
            openai_responses_completed_outcome(&event)
        }
        "openai_chat_json_body" => {
            let body = read_json(&dir.join("body.json"));
            openai_chat_json_body_outcome(&body)
        }
        "openai_responses_json_body" => {
            let body = read_json(&dir.join("body.json"));
            openai_responses_json_body_outcome(&body)
        }
        "synthetic_aborted" => {
            // For the aborted-stream case the parser's contract is: the
            // finalize path MUST produce `UsageOutcome::Error { class:
            // StreamAborted, .. }`. Fixture has no input event because
            // aborted streams produce no terminal event by definition —
            // we synthesize the expected outcome to exercise the
            // `stream_aborted_outcome` helper and assert it matches.
            cxtx::provider::anthropic::stream_aborted_outcome(
                "connection dropped before message_delta",
            )
        }
        other => panic!(
            "unknown fixture kind `{other}` in {}",
            dir.join("expected.json").display()
        ),
    };

    assert_eq!(
        actual,
        expected.outcome,
        "fixture {} mismatch\n  expected: {:?}\n  actual:   {:?}",
        dir.display(),
        expected.outcome,
        actual
    );
}

#[test]
fn anthropic_sse_happy() {
    run_fixture(&fixtures_root().join("anthropic_sse_happy"));
}

#[test]
fn anthropic_sse_with_5m_cache_write() {
    run_fixture(&fixtures_root().join("anthropic_sse_with_5m_cache_write"));
}

#[test]
fn anthropic_sse_with_1h_cache_write() {
    run_fixture(&fixtures_root().join("anthropic_sse_with_1h_cache_write"));
}

#[test]
fn anthropic_sse_with_breakdown_matching() {
    run_fixture(&fixtures_root().join("anthropic_sse_with_breakdown_matching"));
}

#[test]
fn anthropic_sse_with_breakdown_mismatch() {
    run_fixture(&fixtures_root().join("anthropic_sse_with_breakdown_mismatch"));
}

#[test]
fn anthropic_sse_aggregate_only() {
    run_fixture(&fixtures_root().join("anthropic_sse_aggregate_only"));
}

#[test]
fn anthropic_json_happy() {
    run_fixture(&fixtures_root().join("anthropic_json_happy"));
}

#[test]
fn anthropic_stream_aborted() {
    run_fixture(&fixtures_root().join("anthropic_stream_aborted"));
}

#[test]
fn openai_sse_chatcompletions_with_usage() {
    run_fixture(&fixtures_root().join("openai_sse_chatcompletions_with_usage"));
}

#[test]
fn openai_sse_chatcompletions_no_usage() {
    run_fixture(&fixtures_root().join("openai_sse_chatcompletions_no_usage"));
}

#[test]
fn openai_sse_chatcompletions_n2() {
    run_fixture(&fixtures_root().join("openai_sse_chatcompletions_n2"));
}

#[test]
fn openai_sse_responses_completed() {
    run_fixture(&fixtures_root().join("openai_sse_responses_completed"));
}

#[test]
fn openai_sse_responses_tool_use() {
    run_fixture(&fixtures_root().join("openai_sse_responses_tool_use"));
}

#[test]
fn openai_sse_responses_incomplete_length() {
    run_fixture(&fixtures_root().join("openai_sse_responses_incomplete_length"));
}

#[test]
fn openai_sse_responses_failed() {
    run_fixture(&fixtures_root().join("openai_sse_responses_failed"));
}

#[test]
fn openai_json_chatcompletions_happy() {
    run_fixture(&fixtures_root().join("openai_json_chatcompletions_happy"));
}

#[test]
fn openai_json_responses_happy() {
    run_fixture(&fixtures_root().join("openai_json_responses_happy"));
}

/// Sanity: classify_http_status maps to the right ErrorClass variants.
#[test]
fn http_status_classification() {
    use cxtx::provider::usage::classify_http_status;
    assert_eq!(classify_http_status(429), ErrorClass::Upstream4xx);
    assert_eq!(classify_http_status(503), ErrorClass::Upstream5xx);
}
