//! Typed provider-usage parse state shared by the Anthropic and OpenAI
//! finalize paths.
//!
//! Sprint 016 scope: parse the `usage` object (and its cousins) into a
//! structurally faithful, provider-neutral shape, AND record the
//! parse-status outcome so Sprint 017 can distinguish happy-path from
//! `not_reported` / `error` without re-parsing raw payloads.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Canonical usage numbers extracted from a provider response. Fields are
/// `u64` because provider-reported token counts are non-negative by
/// definition; the stored `TurnMetrics` uses `i64` for compatibility with
/// the rest of the cxdb schema — the conversion happens at the persistence
/// boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub reasoning_tokens: u64,
    /// Aggregate cache-creation token count (Anthropic). 0 when absent.
    pub cache_creation_total: u64,
    /// Anthropic per-TTL cache-creation breakdown. 0 when absent.
    pub cache_creation_5m: u64,
    /// Anthropic per-TTL cache-creation breakdown. 0 when absent.
    pub cache_creation_1h: u64,
    /// Raw finish-reason strings in provider-native shape; multi-choice
    /// responses (OpenAI `n>1`) preserve one entry per choice in
    /// `choices[].index` order. Sprint 017 maps these to the canonical
    /// set.
    pub finish_reasons_raw: Vec<String>,
}

/// Classifier for the `UsageOutcome::Error` variant. `Debug` output
/// becomes the Sprint 017 span tag / metric `reason` / `error.type`
/// value (per the sprint brief, Error → `format!("error:{class:?}")`),
/// so variant names are effectively the contract surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorClass {
    Upstream4xx,
    Upstream5xx,
    StreamAborted,
    ConnectionDrop,
    MalformedJson,
    Other(String),
}

/// Result of the finalize-time usage parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageOutcome {
    /// Clean happy path — usage object parsed from the terminal event or
    /// response body.
    Reported(RawUsage),
    /// Stream / response terminated cleanly but reported no usage object
    /// (e.g., OpenAI ChatCompletions SSE without `stream_options.include_usage`).
    /// `partial` retains whatever finish reasons were visible for Sprint 017.
    NotReported { partial: RawUsage },
    /// Upstream error, aborted stream, malformed JSON, etc. `detail`
    /// carries a free-form operator-facing message.
    Error {
        class: ErrorClass,
        detail: String,
    },
}

impl UsageOutcome {
    /// Helper for Phase 3 stamping: `Reported → None`,
    /// `NotReported → Some("not_reported")`, `Error{class} → Some("error:{class:?}")`.
    pub fn status_tag(&self) -> Option<String> {
        match self {
            UsageOutcome::Reported(_) => None,
            UsageOutcome::NotReported { .. } => Some("not_reported".to_string()),
            UsageOutcome::Error { class, .. } => Some(format!("error:{class:?}")),
        }
    }

    /// Convenience accessor — returns the `RawUsage` that should drive the
    /// stored `TurnMetrics` (zero usage on `Error`).
    pub fn raw_for_metrics(&self) -> RawUsage {
        match self {
            UsageOutcome::Reported(u) => u.clone(),
            UsageOutcome::NotReported { partial } => partial.clone(),
            UsageOutcome::Error { .. } => RawUsage::default(),
        }
    }
}

// ---- Anthropic extraction -------------------------------------------------

/// Extract Anthropic `usage` object fields into `RawUsage`. `stop_reason`
/// (when present) is appended to `finish_reasons_raw`.
pub fn anthropic_usage_from_value(
    usage: &Value,
    stop_reason: Option<&str>,
) -> RawUsage {
    let mut raw = RawUsage::default();
    if let Some(n) = usage.get("input_tokens").and_then(Value::as_u64) {
        raw.input_tokens = n;
    }
    if let Some(n) = usage.get("output_tokens").and_then(Value::as_u64) {
        raw.output_tokens = n;
    }
    if let Some(n) = usage.get("cache_read_input_tokens").and_then(Value::as_u64) {
        raw.cached_tokens = n;
    }
    if let Some(n) = usage.get("cache_creation_input_tokens").and_then(Value::as_u64) {
        raw.cache_creation_total = n;
    }
    if let Some(cache_creation) = usage.get("cache_creation") {
        if let Some(n) = cache_creation
            .get("ephemeral_5m_input_tokens")
            .and_then(Value::as_u64)
        {
            raw.cache_creation_5m = n;
        }
        if let Some(n) = cache_creation
            .get("ephemeral_1h_input_tokens")
            .and_then(Value::as_u64)
        {
            raw.cache_creation_1h = n;
        }
    }
    // Reasoning/thinking tokens — see phase0/anthropic-reasoning-token-field.md.
    // Not reported by Anthropic on current public models; field stays 0 until
    // a canonical key lands. We probe a couple of plausible future keys so the
    // parser is forward-compatible without guessing wrong.
    if let Some(n) = usage.get("thinking_tokens").and_then(Value::as_u64) {
        raw.reasoning_tokens = n;
    } else if let Some(n) = usage.get("thinking_output_tokens").and_then(Value::as_u64) {
        raw.reasoning_tokens = n;
    }

    if let Some(reason) = stop_reason.filter(|s| !s.is_empty()) {
        raw.finish_reasons_raw.push(reason.to_string());
    }
    raw
}

// ---- OpenAI ChatCompletions extraction -----------------------------------

/// Extract ChatCompletions-style `usage` (SSE terminal chunk or JSON body)
/// into `RawUsage`. Caller supplies the finish-reason vector from
/// `choices[].finish_reason`.
pub fn openai_chat_usage_from_value(
    usage: &Value,
    finish_reasons: Vec<String>,
) -> RawUsage {
    let mut raw = RawUsage::default();
    if let Some(n) = usage.get("prompt_tokens").and_then(Value::as_u64) {
        raw.input_tokens = n;
    }
    if let Some(n) = usage.get("completion_tokens").and_then(Value::as_u64) {
        raw.output_tokens = n;
    }
    if let Some(details) = usage.get("prompt_tokens_details") {
        if let Some(n) = details.get("cached_tokens").and_then(Value::as_u64) {
            raw.cached_tokens = n;
        }
    }
    if let Some(details) = usage.get("completion_tokens_details") {
        if let Some(n) = details.get("reasoning_tokens").and_then(Value::as_u64) {
            raw.reasoning_tokens = n;
        }
    }
    raw.finish_reasons_raw = finish_reasons;
    raw
}

// ---- OpenAI Responses extraction ------------------------------------------

/// Extract Responses-API `usage` object (on `response.completed` event or
/// JSON body). Caller supplies the raw single-element finish-reason vec.
pub fn openai_responses_usage_from_value(
    usage: &Value,
    finish_reasons: Vec<String>,
) -> RawUsage {
    let mut raw = RawUsage::default();
    if let Some(n) = usage.get("input_tokens").and_then(Value::as_u64) {
        raw.input_tokens = n;
    }
    if let Some(n) = usage.get("output_tokens").and_then(Value::as_u64) {
        raw.output_tokens = n;
    }
    if let Some(details) = usage.get("input_tokens_details") {
        if let Some(n) = details.get("cached_tokens").and_then(Value::as_u64) {
            raw.cached_tokens = n;
        }
    }
    if let Some(details) = usage.get("output_tokens_details") {
        if let Some(n) = details.get("reasoning_tokens").and_then(Value::as_u64) {
            raw.reasoning_tokens = n;
        }
    }
    raw.finish_reasons_raw = finish_reasons;
    raw
}

/// Classify an HTTP status code that came back before / during a stream.
pub fn classify_http_status(status: u16) -> ErrorClass {
    match status {
        400..=499 => ErrorClass::Upstream4xx,
        500..=599 => ErrorClass::Upstream5xx,
        _ => ErrorClass::Other(format!("http_{status}")),
    }
}

// ---- Fixture-oriented entry points ---------------------------------------
//
// These helpers take a parsed JSON `Value` representing a terminal SSE event
// or a complete non-streaming response body, and produce a `UsageOutcome`.
// They are the top-level parse API exercised by the 16-cell matrix tests.

/// Parse the Anthropic SSE `message_delta` event body into a `UsageOutcome`.
///
/// Input: the full event JSON (the value after `data:`), which has shape
/// `{ "type": "message_delta", "delta": {...}, "usage": {...} }`.
pub fn anthropic_sse_message_delta_outcome(event: &Value) -> UsageOutcome {
    let stop = event
        .get("delta")
        .and_then(|d| d.get("stop_reason"))
        .and_then(Value::as_str);
    match event.get("usage") {
        Some(usage) => UsageOutcome::Reported(anthropic_usage_from_value(usage, stop)),
        None => UsageOutcome::NotReported {
            partial: RawUsage {
                finish_reasons_raw: stop.into_iter().map(|s| s.to_string()).collect(),
                ..RawUsage::default()
            },
        },
    }
}

/// Parse a complete Anthropic non-streaming JSON response body into a
/// `UsageOutcome`.
pub fn anthropic_json_body_outcome(body: &Value) -> UsageOutcome {
    let stop = body.get("stop_reason").and_then(Value::as_str);
    match body.get("usage") {
        Some(usage) => UsageOutcome::Reported(anthropic_usage_from_value(usage, stop)),
        None => UsageOutcome::NotReported {
            partial: RawUsage {
                finish_reasons_raw: stop.into_iter().map(|s| s.to_string()).collect(),
                ..RawUsage::default()
            },
        },
    }
}

/// Parse the terminal non-`[DONE]` ChatCompletions SSE chunk. If the chunk
/// carries `usage`, the outcome is `Reported`; otherwise `NotReported` with
/// finish reasons preserved from `choices[].finish_reason` (caller supplies
/// the observed reasons collected from the stream).
pub fn openai_chat_terminal_chunk_outcome(
    terminal_chunk: &Value,
    accumulated_finish_reasons: Vec<String>,
) -> UsageOutcome {
    // Merge finish reasons visible on the terminal chunk itself with any the
    // caller accumulated earlier. Deduplicate by index position — we just
    // pick whichever set is larger.
    let mut finish = accumulated_finish_reasons;
    if let Some(choices) = terminal_chunk.get("choices").and_then(Value::as_array) {
        if !choices.is_empty() {
            let mut indexed: Vec<(u64, String)> = Vec::new();
            for choice in choices {
                let idx = choice.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                    indexed.push((idx, reason.to_string()));
                }
            }
            indexed.sort_by_key(|(idx, _)| *idx);
            let latest: Vec<String> = indexed.into_iter().map(|(_, s)| s).collect();
            if latest.len() >= finish.len() {
                finish = latest;
            }
        }
    }

    match terminal_chunk.get("usage") {
        Some(usage) => UsageOutcome::Reported(openai_chat_usage_from_value(usage, finish)),
        None => UsageOutcome::NotReported {
            partial: RawUsage {
                finish_reasons_raw: finish,
                ..RawUsage::default()
            },
        },
    }
}

/// Parse a Responses-API `response.completed` event into a `UsageOutcome`.
///
/// Input: the full event JSON — `{ "type": "response.completed", "response": {...} }`.
pub fn openai_responses_completed_outcome(event: &Value) -> UsageOutcome {
    let response = event.get("response").unwrap_or(event);
    let finish = responses_raw_finish_reason_inner(response);
    match response.get("usage") {
        Some(usage) => UsageOutcome::Reported(openai_responses_usage_from_value(
            usage,
            vec![finish],
        )),
        None => UsageOutcome::NotReported {
            partial: RawUsage {
                finish_reasons_raw: if finish.is_empty() {
                    Vec::new()
                } else {
                    vec![finish]
                },
                ..RawUsage::default()
            },
        },
    }
}

/// Parse a complete non-streaming ChatCompletions JSON body into an
/// outcome.
pub fn openai_chat_json_body_outcome(body: &Value) -> UsageOutcome {
    let finish = body
        .get("choices")
        .and_then(Value::as_array)
        .map(|choices| {
            let mut indexed: Vec<(u64, String)> = choices
                .iter()
                .filter_map(|c| {
                    let idx = c.get("index").and_then(Value::as_u64).unwrap_or(0);
                    c.get("finish_reason")
                        .and_then(Value::as_str)
                        .map(|s| (idx, s.to_string()))
                })
                .collect();
            indexed.sort_by_key(|(idx, _)| *idx);
            indexed.into_iter().map(|(_, s)| s).collect::<Vec<_>>()
        })
        .unwrap_or_default();
    match body.get("usage") {
        Some(usage) => UsageOutcome::Reported(openai_chat_usage_from_value(usage, finish)),
        None => UsageOutcome::NotReported {
            partial: RawUsage {
                finish_reasons_raw: finish,
                ..RawUsage::default()
            },
        },
    }
}

/// Parse a complete non-streaming Responses-API JSON body into an outcome.
pub fn openai_responses_json_body_outcome(body: &Value) -> UsageOutcome {
    // Body may be a bare response object, or `{response: {...}}`.
    let response = body.get("response").unwrap_or(body);
    let finish = responses_raw_finish_reason_inner(response);
    match response.get("usage") {
        Some(usage) => UsageOutcome::Reported(openai_responses_usage_from_value(
            usage,
            vec![finish],
        )),
        None => UsageOutcome::NotReported {
            partial: RawUsage {
                finish_reasons_raw: if finish.is_empty() {
                    Vec::new()
                } else {
                    vec![finish]
                },
                ..RawUsage::default()
            },
        },
    }
}

fn responses_raw_finish_reason_inner(response: &Value) -> String {
    let status = response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match status {
        "completed" => {
            let has_tool_call = response
                .get("output")
                .and_then(Value::as_array)
                .map(|items| {
                    items.iter().any(|item| {
                        item.get("type").and_then(Value::as_str) == Some("function_call")
                    })
                })
                .unwrap_or(false);
            if has_tool_call {
                "tool_use".to_string()
            } else {
                "completed".to_string()
            }
        }
        "incomplete" => response
            .get("incomplete_details")
            .and_then(|d| d.get("reason"))
            .and_then(Value::as_str)
            .map(|r| format!("incomplete:{r}"))
            .unwrap_or_else(|| "incomplete".to_string()),
        "failed" => {
            let code = response
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if code.is_empty() {
                "failed".to_string()
            } else {
                format!("failed:{code}")
            }
        }
        other if !other.is_empty() => other.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_tag_variants() {
        assert_eq!(
            UsageOutcome::Reported(RawUsage::default()).status_tag(),
            None
        );
        assert_eq!(
            UsageOutcome::NotReported {
                partial: RawUsage::default()
            }
            .status_tag(),
            Some("not_reported".to_string())
        );
        assert_eq!(
            UsageOutcome::Error {
                class: ErrorClass::Upstream5xx,
                detail: String::new()
            }
            .status_tag(),
            Some("error:Upstream5xx".to_string())
        );
        assert_eq!(
            UsageOutcome::Error {
                class: ErrorClass::Other("boom".to_string()),
                detail: String::new()
            }
            .status_tag(),
            Some("error:Other(\"boom\")".to_string())
        );
    }

    #[test]
    fn anthropic_usage_picks_up_all_fields() {
        let usage = json!({
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_read_input_tokens": 20,
            "cache_creation_input_tokens": 30,
            "cache_creation": {
                "ephemeral_5m_input_tokens": 10,
                "ephemeral_1h_input_tokens": 20
            }
        });
        let raw = anthropic_usage_from_value(&usage, Some("end_turn"));
        assert_eq!(raw.input_tokens, 100);
        assert_eq!(raw.output_tokens, 50);
        assert_eq!(raw.cached_tokens, 20);
        assert_eq!(raw.cache_creation_total, 30);
        assert_eq!(raw.cache_creation_5m, 10);
        assert_eq!(raw.cache_creation_1h, 20);
        assert_eq!(raw.finish_reasons_raw, vec!["end_turn"]);
    }

    #[test]
    fn openai_chat_usage_captures_cached_and_reasoning() {
        let usage = json!({
            "prompt_tokens": 10,
            "completion_tokens": 7,
            "prompt_tokens_details": {"cached_tokens": 3},
            "completion_tokens_details": {"reasoning_tokens": 2}
        });
        let raw = openai_chat_usage_from_value(&usage, vec!["stop".to_string()]);
        assert_eq!(raw.input_tokens, 10);
        assert_eq!(raw.output_tokens, 7);
        assert_eq!(raw.cached_tokens, 3);
        assert_eq!(raw.reasoning_tokens, 2);
        assert_eq!(raw.finish_reasons_raw, vec!["stop"]);
    }

    #[test]
    fn openai_responses_usage_captures_cached_and_reasoning() {
        let usage = json!({
            "input_tokens": 40,
            "output_tokens": 12,
            "input_tokens_details": {"cached_tokens": 8},
            "output_tokens_details": {"reasoning_tokens": 4}
        });
        let raw = openai_responses_usage_from_value(&usage, vec!["completed".to_string()]);
        assert_eq!(raw.input_tokens, 40);
        assert_eq!(raw.output_tokens, 12);
        assert_eq!(raw.cached_tokens, 8);
        assert_eq!(raw.reasoning_tokens, 4);
        assert_eq!(raw.finish_reasons_raw, vec!["completed"]);
    }
}
