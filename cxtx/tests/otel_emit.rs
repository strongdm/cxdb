//! Phase 2 emit-pipeline tests for Sprint 017.
//!
//! These tests install the global OTEL tracer + meter providers against
//! in-memory exporters, invoke `cxtx::otel::llm_call::finalize_llm_call`
//! through representative inputs, and assert span attributes + metric
//! samples + cardinality constraints.
//!
//! Global-provider state is process-wide; tests serialize via a static
//! Mutex so assertions aren't contaminated by each other.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use cxtx::otel::call_context::{AppAttribution, CallContext};
use cxtx::otel::llm_call::finalize_llm_call;
use cxtx::provider::usage::{ErrorClass, RawUsage, UsageOutcome};
use opentelemetry::global;
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use opentelemetry_sdk::metrics::reader::MetricReader;
use opentelemetry_sdk::metrics::{
    InstrumentKind, ManualReader, MetricResult, Pipeline, SdkMeterProvider, Temporality,
};
use opentelemetry_sdk::testing::trace::InMemorySpanExporter;
use opentelemetry_sdk::trace::TracerProvider;
use std::sync::{Arc, Weak};

fn serial_lock() -> &'static Mutex<TestHarness> {
    static HARNESS: OnceLock<Mutex<TestHarness>> = OnceLock::new();
    HARNESS.get_or_init(|| Mutex::new(TestHarness::new()))
}

/// Newtype wrapping `Arc<ManualReader>` so multiple handles can share the
/// same reader state — the SdkMeterProvider consumes one copy; the test
/// harness holds another for on-demand drain via `collect()`.
#[derive(Debug, Clone)]
struct SharedManualReader(Arc<ManualReader>);

impl SharedManualReader {
    fn new() -> Self {
        Self(Arc::new(
            ManualReader::builder()
                .with_temporality(Temporality::Delta)
                .build(),
        ))
    }
}

impl MetricReader for SharedManualReader {
    fn register_pipeline(&self, p: Weak<Pipeline>) {
        self.0.register_pipeline(p);
    }
    fn collect(&self, rm: &mut ResourceMetrics) -> MetricResult<()> {
        self.0.collect(rm)
    }
    fn force_flush(&self) -> MetricResult<()> {
        self.0.force_flush()
    }
    fn shutdown(&self) -> MetricResult<()> {
        self.0.shutdown()
    }
    fn temporality(&self, kind: InstrumentKind) -> Temporality {
        self.0.temporality(kind)
    }
}

/// Owns the in-memory span exporter + a shared ManualReader for metrics,
/// plus the provider handles installed globally for these tests.
struct TestHarness {
    span_exporter: InMemorySpanExporter,
    reader: SharedManualReader,
    #[allow(dead_code)]
    tracer_provider: TracerProvider,
    #[allow(dead_code)]
    meter_provider: SdkMeterProvider,
}

impl TestHarness {
    fn new() -> Self {
        let span_exporter = InMemorySpanExporter::default();
        let tracer_provider = TracerProvider::builder()
            .with_simple_exporter(span_exporter.clone())
            .build();
        global::set_tracer_provider(tracer_provider.clone());

        let reader = SharedManualReader::new();
        let meter_provider = SdkMeterProvider::builder()
            .with_reader(reader.clone())
            .build();
        global::set_meter_provider(meter_provider.clone());

        Self {
            span_exporter,
            reader,
            tracer_provider,
            meter_provider,
        }
    }

    fn reset(&self) {
        // Drain pending delta + clear the span exporter so each test
        // starts with an empty slate.
        let _ = self.drain_metrics();
        self.span_exporter.reset();
    }

    fn drain_spans(&self) -> Vec<opentelemetry_sdk::export::trace::SpanData> {
        self.span_exporter.get_finished_spans().unwrap_or_default()
    }

    fn drain_metrics(&self) -> Vec<ResourceMetrics> {
        let mut rm = ResourceMetrics {
            resource: Default::default(),
            scope_metrics: Vec::new(),
        };
        let _ = self.reader.collect(&mut rm);
        vec![rm]
    }
}

fn test_attribution() -> AppAttribution {
    AppAttribution {
        client_tag: "cxtx/claude".to_string(),
        wrapper_command: "claude".to_string(),
        wrapper_version: "0.1.0".to_string(),
        provider_kind: "anthropic".to_string(),
        session_id: "sess-abc".to_string(),
        user: Some("alice".to_string()),
        tenant: None,
    }
}

fn test_ctx(is_stream: bool) -> CallContext {
    CallContext::new(
        Instant::now(),
        "claude-sonnet-4-6",
        "anthropic",
        test_attribution(),
        is_stream,
    )
}

fn span_attrs(span: &opentelemetry_sdk::export::trace::SpanData) -> Vec<(String, String)> {
    span.attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), format!("{:?}", kv.value)))
        .collect()
}

fn attr_value(
    span: &opentelemetry_sdk::export::trace::SpanData,
    key: &str,
) -> Option<String> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| match &kv.value {
            opentelemetry::Value::String(s) => s.as_str().to_string(),
            other => format!("{:?}", other),
        })
}

fn metric_samples<'a>(
    metrics: &'a [opentelemetry_sdk::metrics::data::ResourceMetrics],
    metric_name: &str,
) -> Vec<&'a opentelemetry_sdk::metrics::data::Metric> {
    metrics
        .iter()
        .flat_map(|rm| rm.scope_metrics.iter())
        .flat_map(|sm| sm.metrics.iter())
        .filter(|m| m.name == metric_name)
        .collect()
}

fn histogram_sample_attr_keys(
    metric: &opentelemetry_sdk::metrics::data::Metric,
) -> Vec<Vec<String>> {
    if let Some(h) = metric.data.as_any().downcast_ref::<opentelemetry_sdk::metrics::data::Histogram<u64>>() {
        h.data_points
            .iter()
            .map(|dp| {
                dp.attributes
                    .iter()
                    .map(|kv| kv.key.as_str().to_string())
                    .collect::<Vec<_>>()
            })
            .collect()
    } else {
        Vec::new()
    }
}

fn counter_sample_attr_keys(
    metric: &opentelemetry_sdk::metrics::data::Metric,
) -> Vec<Vec<String>> {
    if let Some(c) = metric.data.as_any().downcast_ref::<opentelemetry_sdk::metrics::data::Sum<u64>>() {
        c.data_points
            .iter()
            .map(|dp| {
                dp.attributes
                    .iter()
                    .map(|kv| kv.key.as_str().to_string())
                    .collect::<Vec<_>>()
            })
            .collect()
    } else {
        Vec::new()
    }
}

fn counter_sum(metric: &opentelemetry_sdk::metrics::data::Metric) -> u64 {
    if let Some(c) = metric.data.as_any().downcast_ref::<opentelemetry_sdk::metrics::data::Sum<u64>>() {
        c.data_points.iter().map(|dp| dp.value).sum()
    } else {
        0
    }
}

fn histogram_sample_count(metric: &opentelemetry_sdk::metrics::data::Metric) -> u64 {
    if let Some(h) = metric.data.as_any().downcast_ref::<opentelemetry_sdk::metrics::data::Histogram<u64>>() {
        h.data_points.iter().map(|dp| dp.count).sum()
    } else {
        0
    }
}

/// P2-T1: happy-path emission — one span + N histogram samples + 1 counter.
#[tokio::test(flavor = "multi_thread")]
async fn p2_t1_happy_path_emits_span_histogram_and_calls() {
    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    let ctx = test_ctx(true);
    let outcome = UsageOutcome::Reported(RawUsage {
        input_tokens: 100,
        output_tokens: 50,
        cached_tokens: 20,
        reasoning_tokens: 10,
        finish_reasons_raw: vec!["end_turn".to_string()],
        ..RawUsage::default()
    });
    finalize_llm_call(&ctx, &outcome, Some("claude-sonnet-4-6"));

    let spans = harness.drain_spans();
    assert_eq!(spans.len(), 1, "expected exactly one span");
    let span = &spans[0];
    assert_eq!(span.name.as_ref(), "chat claude-sonnet-4-6");
    assert_eq!(attr_value(span, "gen_ai.system").as_deref(), Some("anthropic"));
    assert_eq!(
        attr_value(span, "gen_ai.request.model").as_deref(),
        Some("claude-sonnet-4-6")
    );
    assert_eq!(
        attr_value(span, "gen_ai.response.model").as_deref(),
        Some("claude-sonnet-4-6")
    );
    assert_eq!(
        attr_value(span, "app.client_tag").as_deref(),
        Some("cxtx/claude")
    );
    // PII-gated user attribute on span (dropped from metrics by the
    // cardinality view test, P2-T7).
    assert_eq!(attr_value(span, "app.user").as_deref(), Some("alice"));
    assert_eq!(
        attr_value(span, "llm.response_model_source").as_deref(),
        Some("response")
    );
    // finish reasons array includes 'stop' (end_turn → stop).
    let fr = span_attrs(span)
        .into_iter()
        .find(|(k, _)| k == "gen_ai.response.finish_reasons")
        .unwrap()
        .1;
    assert!(fr.contains("stop"), "finish_reasons should carry 'stop', got {fr}");

    let metrics = harness.drain_metrics();
    let histos = metric_samples(&metrics, "gen_ai.client.token.usage");
    assert_eq!(histos.len(), 1);
    // 4 buckets (input=80, cached=20, output=40, reasoning=10) per derive_and_validate happy path.
    assert_eq!(histogram_sample_count(histos[0]), 4);

    let calls = metric_samples(&metrics, "gen_ai.calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(counter_sum(calls[0]), 1);

    // No usage_missing increments on a happy call.
    let miss = metric_samples(&metrics, "gen_ai.usage_missing");
    assert!(miss.is_empty() || counter_sum(miss[0]) == 0);
}

/// P2-T2: not-reported path — zero histogram, zero calls, one
/// `usage_missing{reason=not_reported}`.
#[tokio::test(flavor = "multi_thread")]
async fn p2_t2_not_reported_path() {
    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    let ctx = test_ctx(true);
    let outcome = UsageOutcome::NotReported {
        partial: RawUsage {
            finish_reasons_raw: vec!["stop".to_string()],
            ..RawUsage::default()
        },
    };
    finalize_llm_call(&ctx, &outcome, Some("gpt-4o-mini"));

    let metrics = harness.drain_metrics();

    // Histogram zero-sample (may not exist or has zero data points).
    let histos = metric_samples(&metrics, "gen_ai.client.token.usage");
    assert!(histos.is_empty() || histogram_sample_count(histos[0]) == 0);

    // gen_ai.calls zero.
    let calls = metric_samples(&metrics, "gen_ai.calls");
    assert!(calls.is_empty() || counter_sum(calls[0]) == 0);

    // usage_missing incremented with reason=not_reported.
    let miss = metric_samples(&metrics, "gen_ai.usage_missing");
    assert_eq!(miss.len(), 1);
    assert_eq!(counter_sum(miss[0]), 1);
}

/// P2-T3: error path — zero histogram, zero calls, one
/// `usage_missing{reason=error, error.type=<class>}`, span
/// `finish_reasons=["error"]`.
#[tokio::test(flavor = "multi_thread")]
async fn p2_t3_error_path() {
    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    let ctx = test_ctx(true);
    let outcome = UsageOutcome::Error {
        class: ErrorClass::Upstream5xx,
        detail: "internal server error".to_string(),
    };
    finalize_llm_call(&ctx, &outcome, None);

    let spans = harness.drain_spans();
    assert_eq!(spans.len(), 1);
    let fr = span_attrs(&spans[0])
        .into_iter()
        .find(|(k, _)| k == "gen_ai.response.finish_reasons")
        .unwrap()
        .1;
    assert!(fr.contains("error"));

    let metrics = harness.drain_metrics();
    let calls = metric_samples(&metrics, "gen_ai.calls");
    assert!(calls.is_empty() || counter_sum(calls[0]) == 0);

    let miss = metric_samples(&metrics, "gen_ai.usage_missing");
    assert_eq!(miss.len(), 1);
    assert_eq!(counter_sum(miss[0]), 1);
    // The emit attrs for this increment MUST include reason=error and error.type=<class>.
    let keys = counter_sample_attr_keys(miss[0]);
    assert!(keys.iter().any(|k| k.iter().any(|s| s == "reason")));
    assert!(keys.iter().any(|k| k.iter().any(|s| s == "error.type")));
}

/// P2-T4: cache-breakdown-mismatch → `usage_missing{reason=invalid}`,
/// span `llm.usage_invalid_reason="cache_breakdown_mismatch"`.
#[tokio::test(flavor = "multi_thread")]
async fn p2_t4_cache_breakdown_mismatch() {
    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    let ctx = test_ctx(false);
    let outcome = UsageOutcome::Reported(RawUsage {
        input_tokens: 50,
        output_tokens: 10,
        cache_creation_total: 100, // aggregate claims 100
        cache_creation_5m: 25,     // but parts sum to 40
        cache_creation_1h: 15,
        finish_reasons_raw: vec!["end_turn".to_string()],
        ..RawUsage::default()
    });
    finalize_llm_call(&ctx, &outcome, Some("claude-sonnet-4-6"));

    let spans = harness.drain_spans();
    assert_eq!(spans.len(), 1);
    assert_eq!(
        attr_value(&spans[0], "llm.usage_invalid_reason").as_deref(),
        Some("cache_breakdown_mismatch")
    );

    let metrics = harness.drain_metrics();
    let calls = metric_samples(&metrics, "gen_ai.calls");
    assert!(calls.is_empty() || counter_sum(calls[0]) == 0);
    let histos = metric_samples(&metrics, "gen_ai.client.token.usage");
    assert!(histos.is_empty() || histogram_sample_count(histos[0]) == 0);
    let miss = metric_samples(&metrics, "gen_ai.usage_missing");
    assert_eq!(counter_sum(miss[0]), 1);
}

/// P2-T5: all-zero happy path — zero histogram samples, one
/// `gen_ai.calls` increment.
#[tokio::test(flavor = "multi_thread")]
async fn p2_t5_all_zero_reports_as_call_without_histogram() {
    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    let ctx = test_ctx(false);
    let outcome = UsageOutcome::Reported(RawUsage {
        finish_reasons_raw: vec!["end_turn".to_string()],
        ..RawUsage::default()
    });
    finalize_llm_call(&ctx, &outcome, Some("claude-haiku-4"));

    let metrics = harness.drain_metrics();
    let calls = metric_samples(&metrics, "gen_ai.calls");
    assert_eq!(counter_sum(calls[0]), 1);
    let histos = metric_samples(&metrics, "gen_ai.client.token.usage");
    assert!(histos.is_empty() || histogram_sample_count(histos[0]) == 0);
}

/// P2-T6: response-model fallback — `response_model=None` stamps
/// `llm.response_model_source="request_fallback"` and
/// `gen_ai.response.model = <request model>`.
#[tokio::test(flavor = "multi_thread")]
async fn p2_t6_response_model_fallback() {
    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    let ctx = test_ctx(true);
    let outcome = UsageOutcome::Error {
        class: ErrorClass::ConnectionDrop,
        detail: "reset".to_string(),
    };
    finalize_llm_call(&ctx, &outcome, None);

    let spans = harness.drain_spans();
    assert_eq!(spans.len(), 1);
    assert_eq!(
        attr_value(&spans[0], "gen_ai.response.model").as_deref(),
        Some("claude-sonnet-4-6")
    );
    assert_eq!(
        attr_value(&spans[0], "llm.response_model_source").as_deref(),
        Some("request_fallback")
    );
}

/// P3-T1: replay dedup regression — feeding the same semantic assistant
/// turn twice via `SessionRuntime` with DIFFERENT `CallContext.t_start`
/// values dedups to ONE stored turn. Sprint 017 design decision #1: the
/// CallContext is not part of `HistoryItem` and therefore plays no role
/// in replay normalization.
#[tokio::test(flavor = "multi_thread")]
async fn p3_t1_replay_dedup_ignores_call_context() {
    use cxtx::provider::ProviderKind;
    use cxtx::session::SessionRuntime;
    use cxtx::turns::{ArtifactRefs, HistoryItem};

    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    let session =
        SessionRuntime::new(ProviderKind::Claude, Vec::new(), Default::default()).unwrap();

    // First turn: user asks, assistant replies with a usage outcome
    // produced under a `CallContext` stamped at T1.
    let _ = session.observe_request_history(
        "exchange-0001",
        vec![HistoryItem::UserInput {
            text: "hello".to_string(),
            files: Vec::new(),
        }],
        &ArtifactRefs::default(),
    );
    let ctx_t1 = CallContext::new(
        Instant::now(),
        "claude-sonnet-4-6",
        "anthropic",
        test_attribution(),
        /* is_stream */ true,
    );
    finalize_llm_call(
        &ctx_t1,
        &UsageOutcome::Reported(RawUsage {
            input_tokens: 10,
            output_tokens: 4,
            finish_reasons_raw: vec!["end_turn".to_string()],
            ..RawUsage::default()
        }),
        Some("claude-sonnet-4-6"),
    );
    let _appended = session.append_history_item(
        "exchange-0001",
        HistoryItem::AssistantTurn {
            text: "hi".to_string(),
            tool_calls: Vec::new(),
            model: Some("claude-sonnet-4-6".to_string()),
            finish_reason: Some("end_turn".to_string()),
            usage: Some(UsageOutcome::Reported(RawUsage {
                input_tokens: 10,
                output_tokens: 4,
                finish_reasons_raw: vec!["end_turn".to_string()],
                ..RawUsage::default()
            })),
        },
    );

    // Replay the identical semantic turn through a SECOND exchange with
    // a different CallContext.t_start (a few ms later). The normalization
    // must absorb the replay because CallContext never participates.
    std::thread::sleep(std::time::Duration::from_millis(3));
    let ctx_t2 = CallContext::new(
        Instant::now(),
        "claude-sonnet-4-6",
        "anthropic",
        test_attribution(),
        true,
    );
    finalize_llm_call(
        &ctx_t2,
        &UsageOutcome::Reported(RawUsage {
            input_tokens: 10,
            output_tokens: 4,
            finish_reasons_raw: vec!["end_turn".to_string()],
            ..RawUsage::default()
        }),
        Some("claude-sonnet-4-6"),
    );
    let replay = session.observe_request_history(
        "exchange-0002",
        vec![
            HistoryItem::UserInput {
                text: "hello".to_string(),
                files: Vec::new(),
            },
            HistoryItem::AssistantTurn {
                text: "hi".to_string(),
                tool_calls: Vec::new(),
                model: None,
                finish_reason: None,
                usage: None,
            },
        ],
        &ArtifactRefs::default(),
    );
    assert!(
        replay.is_empty(),
        "CallContext.t_start varies BUT semantic history matches — replay must produce zero new turns, got {:?}",
        replay.iter().map(|t| &t.item.item_type).collect::<Vec<_>>()
    );
}

/// P3-T2: Anthropic finalize emit via `finalize_llm_call` from an
/// Anthropic stream terminal `message_delta` SSE event — one `chat
/// claude-<model>` span + happy-path histogram + one `gen_ai.calls`
/// increment.
#[tokio::test(flavor = "multi_thread")]
async fn p3_t2_anthropic_finalize_emits() {
    use cxtx::provider::usage::anthropic_sse_message_delta_outcome;
    use serde_json::json;

    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    let event = json!({
        "type": "message_delta",
        "delta": {"stop_reason": "end_turn"},
        "usage": {"input_tokens": 80, "output_tokens": 30, "cache_read_input_tokens": 20}
    });
    let outcome = anthropic_sse_message_delta_outcome(&event);

    let ctx = CallContext::new(
        Instant::now(),
        "claude-sonnet-4-6",
        "anthropic",
        test_attribution(),
        true,
    );
    finalize_llm_call(&ctx, &outcome, Some("claude-sonnet-4-6"));

    let spans = harness.drain_spans();
    assert_eq!(spans.len(), 1);
    assert!(spans[0].name.as_ref().starts_with("chat "));
    let metrics = harness.drain_metrics();
    let calls = metric_samples(&metrics, "gen_ai.calls");
    assert_eq!(counter_sum(calls[0]), 1);
    let histos = metric_samples(&metrics, "gen_ai.client.token.usage");
    // input = 80 - 20 = 60; cached = 20; output = 30; total 3 samples.
    assert_eq!(histogram_sample_count(histos[0]), 3);
}

/// P3-T3: OpenAI ChatCompletions happy path (caller set
/// `stream_options.include_usage=true`).
#[tokio::test(flavor = "multi_thread")]
async fn p3_t3_openai_chat_happy_include_usage() {
    use cxtx::provider::usage::openai_chat_terminal_chunk_outcome;
    use serde_json::json;

    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    let chunk = json!({
        "choices": [],
        "usage": {
            "prompt_tokens": 100,
            "completion_tokens": 40,
            "prompt_tokens_details": {"cached_tokens": 20}
        }
    });
    let outcome = openai_chat_terminal_chunk_outcome(&chunk, vec!["stop".to_string()]);

    let ctx = CallContext::new(
        Instant::now(),
        "gpt-4o",
        "openai",
        AppAttribution {
            client_tag: "cxtx/codex".to_string(),
            wrapper_command: "codex".to_string(),
            wrapper_version: "0.1.0".to_string(),
            provider_kind: "openai".to_string(),
            session_id: "sess-openai".to_string(),
            user: None,
            tenant: None,
        },
        true,
    );
    finalize_llm_call(&ctx, &outcome, Some("gpt-4o"));

    let metrics = harness.drain_metrics();
    let calls = metric_samples(&metrics, "gen_ai.calls");
    assert_eq!(counter_sum(calls[0]), 1);
}

/// P3-T4: OpenAI ChatCompletions without `include_usage` → one
/// `usage_missing{reason=not_reported}` increment.
#[tokio::test(flavor = "multi_thread")]
async fn p3_t4_openai_chat_not_reported() {
    use cxtx::provider::usage::openai_chat_terminal_chunk_outcome;
    use serde_json::json;

    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    // Terminal chunk WITHOUT `usage` — mirrors the no-`include_usage`
    // stream-options case.
    let chunk = json!({
        "choices": [{"index": 0, "finish_reason": "stop"}]
    });
    let outcome = openai_chat_terminal_chunk_outcome(&chunk, Vec::new());

    let ctx = CallContext::new(
        Instant::now(),
        "gpt-4o",
        "openai",
        AppAttribution {
            client_tag: "cxtx/codex".to_string(),
            wrapper_command: "codex".to_string(),
            wrapper_version: "0.1.0".to_string(),
            provider_kind: "openai".to_string(),
            session_id: "sess-openai".to_string(),
            user: None,
            tenant: None,
        },
        true,
    );
    finalize_llm_call(&ctx, &outcome, Some("gpt-4o"));

    let metrics = harness.drain_metrics();
    let miss = metric_samples(&metrics, "gen_ai.usage_missing");
    assert_eq!(counter_sum(miss[0]), 1);
    let calls = metric_samples(&metrics, "gen_ai.calls");
    assert!(calls.is_empty() || counter_sum(calls[0]) == 0);
}

/// P3-T5: OpenAI Responses finalize with a tool-use output — span
/// `finish_reasons=["tool_use"]`.
#[tokio::test(flavor = "multi_thread")]
async fn p3_t5_openai_responses_tool_use() {
    use cxtx::provider::usage::openai_responses_completed_outcome;
    use serde_json::json;

    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    let event = json!({
        "type": "response.completed",
        "response": {
            "model": "gpt-5.4",
            "status": "completed",
            "output": [
                {"type": "function_call", "call_id": "call_1", "name": "lookup"}
            ],
            "usage": {
                "input_tokens": 50,
                "output_tokens": 10
            }
        }
    });
    let outcome = openai_responses_completed_outcome(&event);

    let ctx = CallContext::new(
        Instant::now(),
        "gpt-5.4",
        "openai",
        AppAttribution {
            client_tag: "cxtx/codex".to_string(),
            wrapper_command: "codex".to_string(),
            wrapper_version: "0.1.0".to_string(),
            provider_kind: "openai".to_string(),
            session_id: "sess-responses".to_string(),
            user: None,
            tenant: None,
        },
        true,
    );
    finalize_llm_call(&ctx, &outcome, Some("gpt-5.4"));

    let spans = harness.drain_spans();
    assert_eq!(spans.len(), 1);
    let fr_value = span_attrs(&spans[0])
        .into_iter()
        .find(|(k, _)| k == "gen_ai.response.finish_reasons")
        .map(|(_, v)| v)
        .unwrap();
    assert!(
        fr_value.contains("tool_use"),
        "expected finish reasons to include tool_use; got {fr_value}"
    );
}

/// P3-T6: WS breadcrumb — drive a mock WS exchange through
/// `WebsocketCapture`; assert ONE `usage_missing{reason=not_reported,
/// gen_ai.system=openai}` increment + ZERO spans emitted by the WS path.
///
/// The Codex WS path uses OpenAI Responses — this uses the existing
/// `WebsocketCapture` happy-path flow that already has coverage in
/// `cxtx/src/proxy.rs::tests::websocket_capture_turns_real_prompt_into_history_and_answer`.
#[tokio::test(flavor = "multi_thread")]
async fn p3_t6_ws_breadcrumb_emits_one_usage_missing_and_no_span() {
    use cxtx::provider::ProviderKind;
    use cxtx::proxy::WebsocketCapture;
    use cxtx::session::SessionRuntime;
    use cxtx::turns::ArtifactRefs;

    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    let session =
        SessionRuntime::new(ProviderKind::Codex, Vec::new(), Default::default()).unwrap();
    let mut capture = WebsocketCapture::new(
        ProviderKind::Codex,
        "exchange-0001".to_string(),
        Some("req_123".to_string()),
        ArtifactRefs::default(),
    );

    // Downstream opens the real exchange (response.create). Upstream
    // finishes with response.completed; that's where the WS breadcrumb
    // emit lives.
    let _ = capture.observe_downstream_text_for_test(
        &session,
        r#"{
            "type":"response.create",
            "model":"gpt-5.4",
            "input":[
                {"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}
            ]
        }"#,
    );
    let _ = capture.observe_upstream_text_for_test(
        &session,
        r#"{
            "type":"response.completed",
            "response":{
                "model":"gpt-5.4",
                "status":"completed",
                "output":[
                    {"type":"message","role":"assistant","content":[{"type":"output_text","text":"yo"}]}
                ]
            }
        }"#,
    );

    let spans = harness.drain_spans();
    assert!(
        spans.is_empty(),
        "WS breadcrumb path must NOT emit a chat <model> span, got {spans:?}"
    );

    let metrics = harness.drain_metrics();
    let miss = metric_samples(&metrics, "gen_ai.usage_missing");
    assert_eq!(counter_sum(miss[0]), 1);
    // No histogram/calls samples should leak from the WS path.
    let calls = metric_samples(&metrics, "gen_ai.calls");
    assert!(calls.is_empty() || counter_sum(calls[0]) == 0);
    let histos = metric_samples(&metrics, "gen_ai.client.token.usage");
    assert!(histos.is_empty() || histogram_sample_count(histos[0]) == 0);
}

/// P2-T7: cardinality-view — the three `gen_ai.*` metrics MUST NOT carry
/// `app.session_id`, `app.user`, `app.wrapper_version` on any emitted
/// sample. (The emit helpers in `cxdb_otel::gen_ai` don't stamp these
/// attributes on metrics at all; this test locks in that invariant.)
#[tokio::test(flavor = "multi_thread")]
async fn p2_t7_cardinality_view_drops_pii_and_version() {
    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    // Cover the three code paths through finalize_llm_call so every
    // metric in the family gets an increment.
    finalize_llm_call(
        &test_ctx(true),
        &UsageOutcome::Reported(RawUsage {
            input_tokens: 10,
            output_tokens: 4,
            finish_reasons_raw: vec!["end_turn".to_string()],
            ..RawUsage::default()
        }),
        Some("claude-sonnet-4-6"),
    );
    finalize_llm_call(
        &test_ctx(true),
        &UsageOutcome::NotReported {
            partial: RawUsage::default(),
        },
        Some("gpt-4o"),
    );
    finalize_llm_call(
        &test_ctx(false),
        &UsageOutcome::Error {
            class: ErrorClass::StreamAborted,
            detail: "client disconnect".to_string(),
        },
        None,
    );

    let metrics = harness.drain_metrics();
    for metric_name in [
        "gen_ai.client.token.usage",
        "gen_ai.calls",
        "gen_ai.usage_missing",
    ] {
        let samples = metric_samples(&metrics, metric_name);
        for metric in samples {
            let per_dp_keys = if metric_name == "gen_ai.client.token.usage" {
                histogram_sample_attr_keys(metric)
            } else {
                counter_sample_attr_keys(metric)
            };
            for keys in per_dp_keys {
                for forbidden in ["app.session_id", "app.user", "app.wrapper_version"] {
                    assert!(
                        !keys.iter().any(|k| k == forbidden),
                        "metric {metric_name} emitted with forbidden attribute {forbidden}: {keys:?}"
                    );
                }
            }
        }
    }
}


// ---------------------------------------------------------------------------
// Sprint 021 — `app.tenant` on cxtx emit sites
// ---------------------------------------------------------------------------

/// Sprint 021 P4-T1: when `AppAttribution.tenant` is `Some(...)`,
/// `finalize_llm_call` stamps `app.tenant` on the `chat <model>` span
/// AND on every `gen_ai.*` metric datapoint.
#[tokio::test(flavor = "multi_thread")]
async fn tenant_stamped_on_span_and_metrics() {
    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    let mut attribution = test_attribution();
    attribution.tenant = Some("tenant-alpha".to_string());
    let ctx = CallContext::new(
        Instant::now(),
        "claude-sonnet-4-6",
        "anthropic",
        attribution,
        true,
    );
    let outcome = UsageOutcome::Reported(RawUsage {
        input_tokens: 100,
        output_tokens: 50,
        finish_reasons_raw: vec!["end_turn".to_string()],
        ..RawUsage::default()
    });
    finalize_llm_call(&ctx, &outcome, Some("claude-sonnet-4-6"));

    let spans = harness.drain_spans();
    let span = spans.iter().find(|s| s.name == "chat claude-sonnet-4-6").expect("span");
    assert_eq!(
        attr_value(span, "app.tenant").as_deref(),
        Some("tenant-alpha"),
        "span attrs: {:?}",
        span_attrs(span)
    );

    let metrics = harness.drain_metrics();
    for metric_name in ["gen_ai.client.token.usage", "gen_ai.calls"] {
        let samples = metric_samples(&metrics, metric_name);
        assert!(
            !samples.is_empty(),
            "expected at least one {metric_name} sample"
        );
        for metric in samples {
            let per_dp_keys = if metric_name == "gen_ai.client.token.usage" {
                histogram_sample_attr_keys(metric)
            } else {
                counter_sample_attr_keys(metric)
            };
            for keys in per_dp_keys {
                assert!(
                    keys.iter().any(|k| k == "app.tenant"),
                    "metric {metric_name} missing app.tenant in {keys:?}"
                );
            }
        }
    }
}

/// Sprint 021 P4-T2: when `AppAttribution.tenant` is `None`, the
/// attribute is OMITTED from span + all metrics.
#[tokio::test(flavor = "multi_thread")]
async fn tenant_omitted_when_none() {
    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    let ctx = test_ctx(true);
    let outcome = UsageOutcome::Reported(RawUsage {
        input_tokens: 42,
        output_tokens: 7,
        finish_reasons_raw: vec!["end_turn".to_string()],
        ..RawUsage::default()
    });
    finalize_llm_call(&ctx, &outcome, Some("claude-sonnet-4-6"));

    let spans = harness.drain_spans();
    let span = spans
        .iter()
        .find(|s| s.name == "chat claude-sonnet-4-6")
        .expect("span");
    assert!(
        attr_value(span, "app.tenant").is_none(),
        "app.tenant MUST NOT be present on span when attribution.tenant=None; attrs: {:?}",
        span_attrs(span)
    );

    let metrics = harness.drain_metrics();
    for metric_name in [
        "gen_ai.client.token.usage",
        "gen_ai.calls",
        "gen_ai.usage_missing",
    ] {
        for metric in metric_samples(&metrics, metric_name) {
            let per_dp_keys = if metric_name == "gen_ai.client.token.usage" {
                histogram_sample_attr_keys(metric)
            } else {
                counter_sample_attr_keys(metric)
            };
            for keys in per_dp_keys {
                assert!(
                    !keys.iter().any(|k| k == "app.tenant"),
                    "metric {metric_name} unexpectedly carries app.tenant when attribution.tenant=None: {keys:?}"
                );
            }
        }
    }
}

/// Sprint 021: empty-string tenant at the metadata layer flows through
/// the flattening seam as `None`, so emit sites see None and omit.
#[tokio::test(flavor = "multi_thread")]
async fn tenant_empty_metadata_string_stays_absent_on_span() {
    use cxdb::types::ContextMetadata as ClientContextMetadata;
    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    let metadata = ClientContextMetadata {
        client_tag: "cxtx/claude".to_string(),
        title: String::new(),
        labels: Vec::new(),
        custom: std::collections::HashMap::new(),
        tenant: Some(String::new()),
        provenance: None,
    };
    let attribution = AppAttribution::from_metadata(&metadata);
    assert_eq!(attribution.tenant, None);

    let ctx = CallContext::new(
        Instant::now(),
        "claude-sonnet-4-6",
        "anthropic",
        attribution,
        false,
    );
    let outcome = UsageOutcome::Reported(RawUsage {
        input_tokens: 1,
        output_tokens: 1,
        finish_reasons_raw: vec!["end_turn".to_string()],
        ..RawUsage::default()
    });
    finalize_llm_call(&ctx, &outcome, Some("claude-sonnet-4-6"));

    let spans = harness.drain_spans();
    let span = spans
        .iter()
        .find(|s| s.name == "chat claude-sonnet-4-6")
        .expect("span");
    assert!(attr_value(span, "app.tenant").is_none());
}

/// Sprint 021 P4-T3: the WebSocket `finalize_pending` breadcrumb stamps
/// `app.tenant` when the SessionRuntime's ContextMetadata carries a
/// tenant (CXTX_TENANT env var is the standard ingress). When tenant
/// is absent, the breadcrumb omits the attribute.
#[tokio::test(flavor = "multi_thread")]
async fn ws_breadcrumb_tenant_propagation() {
    use cxtx::provider::ProviderKind;
    use cxtx::proxy::WebsocketCapture;
    use cxtx::session::SessionRuntime;
    use cxtx::turns::ArtifactRefs;

    let harness = serial_lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    // CXTX_TENANT=tenant-ws threads tenant into the session metadata.
    std::env::set_var("CXTX_TENANT", "tenant-ws");
    let session = SessionRuntime::new(
        ProviderKind::Codex,
        Vec::new(),
        std::collections::BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(session.metadata().tenant.as_deref(), Some("tenant-ws"));

    let mut capture = WebsocketCapture::new(
        ProviderKind::Codex,
        "exchange-ws".to_string(),
        Some("req_ws".to_string()),
        ArtifactRefs::default(),
    );
    let _ = capture.observe_downstream_text_for_test(
        &session,
        r#"{"type":"response.create","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}]}"#,
    );
    let _ = capture.observe_upstream_text_for_test(
        &session,
        r#"{"type":"response.completed","response":{"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"yo"}]}]}}"#,
    );

    let metrics = harness.drain_metrics();
    let samples = metric_samples(&metrics, "gen_ai.usage_missing");
    assert!(
        !samples.is_empty(),
        "expected at least one usage_missing breadcrumb sample"
    );
    let mut found_tenant = false;
    for metric in samples {
        for keys in counter_sample_attr_keys(metric) {
            if keys.iter().any(|k| k == "app.tenant") {
                found_tenant = true;
            }
        }
    }
    assert!(
        found_tenant,
        "WS breadcrumb usage_missing MUST carry app.tenant when session metadata has one"
    );
    std::env::remove_var("CXTX_TENANT");
}
