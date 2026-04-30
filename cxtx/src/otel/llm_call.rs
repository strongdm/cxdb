//! `finalize_llm_call` — the single emit site routed to by every provider
//! finalize block.
//!
//! The function is the only place spans and metrics are stamped for an
//! LLM call. Providers construct `CallContext`, parse `UsageOutcome`, and
//! call `finalize_llm_call(ctx, outcome, response_model)` exactly once.

use std::time::Instant;

use cxdb_otel::gen_ai::{emit_calls, emit_token_usage, emit_usage_missing, Attrs};
use opentelemetry::global;
use opentelemetry::trace::{Span, SpanKind, Status, TraceContextExt, Tracer};
use opentelemetry::{Array, KeyValue, StringValue, Value};

use crate::otel::buckets::{derive_and_validate, InvalidReason};
use crate::otel::call_context::CallContext;
use crate::otel::finish_reasons::{map_anthropic, map_openai_chat, map_openai_responses};
use crate::provider::usage::UsageOutcome;

/// Sentinel used when a parsed response model is unavailable. Caller sets
/// `gen_ai.response.model = gen_ai.request.model` and stamps
/// `llm.response_model_source="request_fallback"` on the span.
const RESPONSE_MODEL_FALLBACK: &str = "request_fallback";

/// Finalize an LLM call — emit one `chat <model>` span (with explicit
/// `start_time`/`end_time`), zero-or-more `gen_ai.client.token.usage`
/// histogram samples, plus either a `gen_ai.calls` counter increment or a
/// `gen_ai.usage_missing` counter increment per the variant dispatch
/// (see sprint doc §"Design Decisions Locked").
pub fn finalize_llm_call(
    ctx: &CallContext,
    outcome: &UsageOutcome,
    response_model: Option<&str>,
) {
    let t_end = Instant::now();
    let duration_ms = t_end.saturating_duration_since(ctx.t_start).as_secs_f64() * 1000.0;

    let resolved_model = response_model
        .map(|s| s.to_string())
        .unwrap_or_else(|| ctx.request_model.clone());
    let response_model_source = if response_model.is_some() {
        "response"
    } else {
        RESPONSE_MODEL_FALLBACK
    };

    // Open the forensic `chat <model>` span directly on the global tracer
    // so we can stamp dotted attributes verbatim (tracing macros mangle
    // dotted keys) and set explicit start/end times.
    let tracer = global::tracer("cxtx");
    let span_name = format!("chat {resolved_model}");
    let mut span = tracer
        .span_builder(span_name.clone())
        .with_kind(SpanKind::Client)
        .with_start_time(instant_to_system_time(ctx.t_start))
        .start(&tracer);

    span.set_attribute(KeyValue::new("gen_ai.system", ctx.provider_system));
    span.set_attribute(KeyValue::new(
        "gen_ai.request.model",
        ctx.request_model.clone(),
    ));
    span.set_attribute(KeyValue::new(
        "gen_ai.response.model",
        resolved_model.clone(),
    ));
    span.set_attribute(KeyValue::new(
        "gen_ai.request.is_stream",
        ctx.is_stream,
    ));
    span.set_attribute(KeyValue::new(
        "llm.response_model_source",
        response_model_source,
    ));
    span.set_attribute(KeyValue::new("llm.tier", "standard"));
    span.set_attribute(KeyValue::new("llm.duration_ms", duration_ms));
    span.set_attribute(KeyValue::new(
        "app.client_tag",
        ctx.attribution.client_tag.clone(),
    ));
    span.set_attribute(KeyValue::new(
        "app.wrapper_command",
        ctx.attribution.wrapper_command.clone(),
    ));
    span.set_attribute(KeyValue::new(
        "app.wrapper_version",
        ctx.attribution.wrapper_version.clone(),
    ));
    span.set_attribute(KeyValue::new(
        "app.provider_kind",
        ctx.attribution.provider_kind.clone(),
    ));
    span.set_attribute(KeyValue::new(
        "app.session_id",
        ctx.attribution.session_id.clone(),
    ));
    if let Some(user) = ctx.attribution.user.as_deref() {
        span.set_attribute(KeyValue::new("app.user", user.to_string()));
    }
    // Decision: `app.tenant` stamped on the span when
    // attribution carries a tenant; omitted entirely when `None`.
    if let Some(tenant) = ctx.attribution.tenant.as_deref() {
        span.set_attribute(KeyValue::new("app.tenant", tenant.to_string()));
    }

    // Build shared attribute set used by metric emit sites.
    let mut common_attrs = Attrs::new()
        .with("gen_ai.system", ctx.provider_system)
        .with("gen_ai.response.model", resolved_model.clone())
        .with("app.client_tag", ctx.attribution.client_tag.clone())
        .with("llm.tier", "standard");
    // Tenant: tenant added to the metric attribute set when present.
    // Absent tenant → no `app.tenant` label on any histogram /
    // counter datapoint.
    if let Some(tenant) = ctx.attribution.tenant.as_deref() {
        common_attrs = common_attrs.with("app.tenant", tenant.to_string());
    }

    match outcome {
        UsageOutcome::Reported(raw) => {
            // Canonical finish reasons first — if validation fails we
            // still want them on the span.
            let finish = canonical_finish(ctx.provider_system, &raw.finish_reasons_raw);
            set_string_array(&mut span, "gen_ai.response.finish_reasons", &finish);

            // Span-only token attributes (DD LLM Observability reads
            // these; metric uses the derived buckets).
            span.set_attribute(KeyValue::new(
                "gen_ai.usage.input_tokens",
                raw.input_tokens as i64,
            ));
            span.set_attribute(KeyValue::new(
                "gen_ai.usage.output_tokens",
                raw.output_tokens as i64,
            ));
            span.set_attribute(KeyValue::new(
                "gen_ai.usage.cached_tokens",
                raw.cached_tokens as i64,
            ));
            span.set_attribute(KeyValue::new(
                "gen_ai.usage.reasoning_tokens",
                raw.reasoning_tokens as i64,
            ));

            match derive_and_validate(raw) {
                Ok(buckets) => {
                    // Happy path: emit histogram samples + counter.
                    emit_token_usage(&buckets, &common_attrs);
                    emit_calls(&common_attrs);
                }
                Err(reason) => {
                    let reason_tag = match &reason {
                        InvalidReason::Other(s) => s.clone(),
                        other => other.as_str().to_string(),
                    };
                    span.set_attribute(KeyValue::new(
                        "llm.usage_invalid_reason",
                        reason_tag.clone(),
                    ));
                    let attrs = common_attrs.clone().with("reason", "invalid");
                    emit_usage_missing(&attrs);
                }
            }
        }
        UsageOutcome::NotReported { partial } => {
            // Preserve real finish reasons when possible.
            let finish = canonical_finish(ctx.provider_system, &partial.finish_reasons_raw);
            if !finish.is_empty() {
                set_string_array(&mut span, "gen_ai.response.finish_reasons", &finish);
            }
            span.set_attribute(KeyValue::new("llm.usage_missing", true));
            let attrs = common_attrs.clone().with("reason", "not_reported");
            emit_usage_missing(&attrs);
        }
        UsageOutcome::Error { class, .. } => {
            let error_tag = format!("{class:?}");
            span.set_attribute(KeyValue::new("llm.usage_missing", true));
            span.set_attribute(KeyValue::new("error.type", error_tag.clone()));
            set_string_array(&mut span, "gen_ai.response.finish_reasons", &["error".to_string()]);
            span.set_status(Status::error(error_tag.clone()));
            let attrs = common_attrs
                .clone()
                .with("reason", "error")
                .with("error.type", error_tag);
            emit_usage_missing(&attrs);
        }
    }

    span.end_with_timestamp(instant_to_system_time(t_end));
    // Span drops here; explicit end above makes duration deterministic.
    let _ = opentelemetry::Context::current();
    drop(span);
}

fn set_string_array<S: AsRef<str>>(span: &mut impl Span, key: &'static str, values: &[S]) {
    let kv_values: Vec<StringValue> = values
        .iter()
        .map(|v| StringValue::from(v.as_ref().to_string()))
        .collect();
    span.set_attribute(KeyValue::new(key, Value::Array(Array::from(kv_values))));
}

/// Convert a monotonic `Instant` into the `SystemTime` that the OTEL
/// tracer expects for explicit start/end stamping.
fn instant_to_system_time(instant: Instant) -> std::time::SystemTime {
    let now_instant = Instant::now();
    let now_sys = std::time::SystemTime::now();
    if instant >= now_instant {
        now_sys + instant.saturating_duration_since(now_instant)
    } else {
        now_sys - now_instant.saturating_duration_since(instant)
    }
}

/// Map a provider-native `finish_reasons_raw` vec to the canonical set.
/// Uses the existing `map_*` helpers from `finish_reasons.rs`.
fn canonical_finish(provider_system: &str, raws: &[String]) -> Vec<String> {
    if raws.is_empty() {
        return Vec::new();
    }
    match provider_system {
        "anthropic" => raws.iter().map(|s| map_anthropic(s)).collect(),
        "openai" => {
            // Disambiguate ChatCompletions (any `stop`/`length`/`tool_calls`/
            // `content_filter` raw value) vs Responses (`completed`,
            // `incomplete:...`, `failed:...`, etc.). The parser tags
            // Responses with either `completed`, `tool_use`,
            // `incomplete:<reason>`, or `failed:<code>`; everything else
            // comes from ChatCompletions.
            if raws.iter().any(|s| {
                s == "completed"
                    || s == "tool_use"
                    || s == "failed"
                    || s == "incomplete"
                    || s.starts_with("incomplete:")
                    || s.starts_with("failed:")
            }) {
                use crate::otel::finish_reasons::ResponsesStatus;
                let mut out: Vec<String> = Vec::new();
                for raw in raws {
                    let status = if raw == "completed" {
                        ResponsesStatus::Completed { has_tool_use: false }
                    } else if raw == "tool_use" {
                        ResponsesStatus::Completed { has_tool_use: true }
                    } else if let Some(rest) = raw.strip_prefix("incomplete:") {
                        ResponsesStatus::Incomplete {
                            reason: rest.to_string(),
                        }
                    } else if let Some(rest) = raw.strip_prefix("failed:") {
                        ResponsesStatus::Failed {
                            code: rest.to_string(),
                        }
                    } else if raw == "failed" {
                        ResponsesStatus::Failed { code: String::new() }
                    } else if raw == "incomplete" {
                        ResponsesStatus::Incomplete {
                            reason: String::new(),
                        }
                    } else {
                        out.push(raw.clone());
                        continue;
                    };
                    let (mut mapped, _) = map_openai_responses(status);
                    out.append(&mut mapped);
                }
                out
            } else {
                let refs: Vec<&str> = raws.iter().map(String::as_str).collect();
                map_openai_chat(&refs)
            }
        }
        _ => raws.to_vec(),
    }
}

/// Drop-guarded wrapper used to mark the current span (when any) with
/// error context. Exposed for future symmetry — today the emit site
/// uses `set_status` directly.
#[allow(dead_code)]
fn attach_error_to_current_span(error_tag: &str) {
    let cx = opentelemetry::Context::current();
    let span = cx.span();
    span.set_status(Status::error(error_tag.to_string()));
}
