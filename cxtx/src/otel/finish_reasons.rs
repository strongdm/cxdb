//! Provider-native → canonical finish-reason mapping.
//!
//! Implements the 14-row table in `OTEL_SPEC.md` §"Finish-reason mapping"
//! across Anthropic, OpenAI ChatCompletions, and OpenAI Responses.
//!
//! Design constraints:
//! - Unknown values are passed through verbatim (never coerced to `error`);
//!   this future-proofs new provider codes without a release train.
//! - `n>1` handling: canonical values are emitted in `choices[].index` order;
//!   the full vec collapses to a single element ONLY when every choice
//!   mapped to the same canonical value (so homogeneous responses stay
//!   single-element).

/// Status extracted from an OpenAI Responses `response.completed` (or
/// `response.failed`) event. See `OTEL_SPEC.md` §"Finish-reason mapping"
/// rows for OpenAI Responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponsesStatus {
    Completed { has_tool_use: bool },
    Incomplete { reason: String },
    Failed { code: String },
}

/// Map an Anthropic native `stop_reason` to the canonical set. Empty input
/// returns an empty string — callers usually filter those out at the emit
/// site.
pub fn map_anthropic(raw: &str) -> String {
    match raw {
        "end_turn" | "stop_sequence" => "stop".to_string(),
        "max_tokens" => "length".to_string(),
        "tool_use" => "tool_use".to_string(),
        // Passthrough — preserves unknown values verbatim instead of
        // coercing to `error`.
        other => other.to_string(),
    }
}

/// Map OpenAI ChatCompletions `choices[].finish_reason` values to the
/// canonical set. Input is a slice of per-choice raw strings in
/// `choices[].index` order; output preserves that order.
///
/// When every mapped choice collapses to the same canonical value the
/// returned vec is length-1 — matching the spec's "de-duplicate only if
/// every choice mapped to the same canonical value" rule.
pub fn map_openai_chat(choices: &[&str]) -> Vec<String> {
    if choices.is_empty() {
        return Vec::new();
    }
    let mapped: Vec<String> = choices.iter().map(|raw| map_openai_chat_one(raw)).collect();
    if mapped.iter().all(|v| v == &mapped[0]) {
        vec![mapped[0].clone()]
    } else {
        mapped
    }
}

fn map_openai_chat_one(raw: &str) -> String {
    match raw {
        "stop" => "stop".to_string(),
        "length" => "length".to_string(),
        "tool_calls" | "function_call" => "tool_use".to_string(),
        "content_filter" => "content_filter".to_string(),
        other => other.to_string(),
    }
}

/// Map an OpenAI Responses terminal event status to the canonical set.
///
/// Returns a `(finish_reasons, error_type)` tuple. `error_type` is
/// populated ONLY for `Failed { code }` — the caller stamps it as
/// `error.type` on the span.
pub fn map_openai_responses(status: ResponsesStatus) -> (Vec<String>, Option<String>) {
    match status {
        ResponsesStatus::Completed { has_tool_use } => {
            if has_tool_use {
                (vec!["tool_use".to_string()], None)
            } else {
                (vec!["stop".to_string()], None)
            }
        }
        ResponsesStatus::Incomplete { reason } => match reason.as_str() {
            "max_output_tokens" => (vec!["length".to_string()], None),
            "content_filter" => (vec!["content_filter".to_string()], None),
            // Passthrough — unknown `incomplete` reasons stay as themselves.
            other => (vec![other.to_string()], None),
        },
        ResponsesStatus::Failed { code } => {
            let err = if code.is_empty() { None } else { Some(code) };
            (vec!["error".to_string()], err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P1-T1: Anthropic mapping — 5 rows + passthrough.
    #[test]
    fn anthropic_maps_five_rows_and_passthrough() {
        assert_eq!(map_anthropic("end_turn"), "stop");
        assert_eq!(map_anthropic("stop_sequence"), "stop");
        assert_eq!(map_anthropic("max_tokens"), "length");
        assert_eq!(map_anthropic("tool_use"), "tool_use");
        // Passthrough — future-proof against new Anthropic codes.
        assert_eq!(map_anthropic("new_weird_reason"), "new_weird_reason");
    }

    /// P1-T2: OpenAI ChatCompletions mapping — 4 rows per choice.
    #[test]
    fn openai_chat_maps_four_rows() {
        assert_eq!(map_openai_chat(&["stop"]), vec!["stop"]);
        assert_eq!(map_openai_chat(&["length"]), vec!["length"]);
        assert_eq!(map_openai_chat(&["tool_calls"]), vec!["tool_use"]);
        assert_eq!(map_openai_chat(&["function_call"]), vec!["tool_use"]);
        assert_eq!(
            map_openai_chat(&["content_filter"]),
            vec!["content_filter"]
        );
    }

    /// P1-T3: OpenAI Responses mapping — 6 scenarios.
    #[test]
    fn openai_responses_maps_six_scenarios() {
        assert_eq!(
            map_openai_responses(ResponsesStatus::Completed {
                has_tool_use: false,
            }),
            (vec!["stop".to_string()], None),
        );
        assert_eq!(
            map_openai_responses(ResponsesStatus::Completed {
                has_tool_use: true,
            }),
            (vec!["tool_use".to_string()], None),
        );
        assert_eq!(
            map_openai_responses(ResponsesStatus::Incomplete {
                reason: "max_output_tokens".to_string(),
            }),
            (vec!["length".to_string()], None),
        );
        assert_eq!(
            map_openai_responses(ResponsesStatus::Incomplete {
                reason: "content_filter".to_string(),
            }),
            (vec!["content_filter".to_string()], None),
        );
        assert_eq!(
            map_openai_responses(ResponsesStatus::Incomplete {
                reason: "policy_violation".to_string(),
            }),
            (vec!["policy_violation".to_string()], None),
        );
        assert_eq!(
            map_openai_responses(ResponsesStatus::Failed {
                code: "rate_limited".to_string(),
            }),
            (vec!["error".to_string()], Some("rate_limited".to_string())),
        );
    }

    /// P1-T4: `n>1` mixed collapse rules.
    #[test]
    fn openai_chat_n_gt_one_mixes_and_collapses() {
        // Mixed: preserved in choice order.
        assert_eq!(
            map_openai_chat(&["stop", "length"]),
            vec!["stop", "length"],
        );
        // Homogeneous: collapses to length-1.
        assert_eq!(
            map_openai_chat(&["stop", "stop"]),
            vec!["stop"],
        );
        // Homogeneous (3): collapses.
        assert_eq!(
            map_openai_chat(&["length", "length", "length"]),
            vec!["length"],
        );
        // Mixed after mapping (tool_calls + length → tool_use + length).
        assert_eq!(
            map_openai_chat(&["tool_calls", "length"]),
            vec!["tool_use", "length"],
        );
    }
}
