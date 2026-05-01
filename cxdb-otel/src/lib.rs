//! Narrow shared OpenTelemetry bootstrap for cxdb binaries.
//!
//! Exposes an env-gated `init()` that is fully inert when
//! `OTEL_EXPORTER_OTLP_ENDPOINT` is unset or empty: no subscriber installed,
//! no tracing callsites lit up, no exporter spun up. When the endpoint is
//! set, the standard OTLP/gRPC pipeline (traces + metrics) is constructed,
//! the W3C `TraceContext` propagator is installed globally, and a
//! `tracing_subscriber` registry is wired so downstream `tracing::*!` calls
//! reach the OTEL layer.
//!
//! Binaries call `init(&cfg, &rt_handle)` once and hold the returned
//! `OtelGuard` for the program lifetime. Drop order is: guard first, then the
//! Tokio runtime that owns the exporter's background tasks.

use std::time::Duration;
use std::str::FromStr;

use opentelemetry::global;
use opentelemetry::trace::{
    SpanContext, TraceContextExt, TraceFlags, TraceId, TraceState, TracerProvider as _,
};
use opentelemetry::{Context, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, Temporality};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::TracerProvider;
use opentelemetry_sdk::{runtime, Resource};
use thiserror::Error;
use tokio::runtime::Handle;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

pub mod gen_ai;
pub mod http;
pub mod test_util;

pub const BINARY_TRACE_CONTEXT_V1: u32 = 0x0001;
pub const BINARY_FLAG_EXTENDED_HEADER: u16 = 1 << 15;
pub const BINARY_TRACE_TRACESTATE_LIMIT: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceContextTrailer {
    pub trace_flags: u8,
    pub trace_id: [u8; 16],
    pub parent_span_id: [u8; 8],
    pub tracestate: Vec<u8>,
}

pub fn context_to_trailer(ctx: &Context) -> Option<TraceContextTrailer> {
    let span = ctx.span();
    let span_context = span.span_context();
    if !span_context.is_valid() {
        return None;
    }

    let trace_id = span_context.trace_id().to_bytes();
    let parent_span_id = span_context.span_id().to_bytes();
    let mut tracestate = span_context.trace_state().header().into_bytes();
    if tracestate.len() > BINARY_TRACE_TRACESTATE_LIMIT {
        tracestate.clear();
    }

    Some(TraceContextTrailer {
        trace_flags: (span_context.trace_flags() & TraceFlags::SAMPLED).to_u8(),
        trace_id,
        parent_span_id,
        tracestate,
    })
}

pub fn trailer_to_context(trailer: &TraceContextTrailer) -> Context {
    let trace_state = std::str::from_utf8(&trailer.tracestate)
        .ok()
        .and_then(|raw| TraceState::from_str(raw).ok())
        .unwrap_or_default();

    let span_context = SpanContext::new(
        TraceId::from_bytes(trailer.trace_id),
        opentelemetry::trace::SpanId::from_bytes(trailer.parent_span_id),
        TraceFlags::new(trailer.trace_flags) & TraceFlags::SAMPLED,
        true,
        trace_state,
    );

    Context::new().with_remote_span_context(span_context)
}

/// Parsed environment configuration for OTEL bootstrap.
///
/// Only fields the bootstrap actually consumes live here. The OTLP/gRPC
/// exporter and the trace SDK auto-read several other env vars on their
/// own — `OTEL_EXPORTER_OTLP_HEADERS` (auth headers, picked up by
/// `opentelemetry_otlp::TonicExporterBuilder` when `with_metadata` is not
/// called), and `OTEL_TRACES_SAMPLER` / `OTEL_TRACES_SAMPLER_ARG`
/// (consumed by `opentelemetry_sdk::trace::Config::default()`). Don't
/// re-add fields here unless we explicitly thread them into a builder
/// API; otherwise the API misleads operators.
#[derive(Debug, Clone, Default)]
pub struct OtelConfig {
    pub endpoint: Option<String>,
    pub service_name: Option<String>,
    pub resource_attributes: Option<String>,
    pub metric_export_interval_ms: Option<u64>,
    pub temporality_preference: Option<String>,
}

impl OtelConfig {
    /// Read configuration from the process environment.
    pub fn from_env() -> Self {
        fn read(name: &str) -> Option<String> {
            std::env::var(name)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        }

        Self {
            endpoint: read("OTEL_EXPORTER_OTLP_ENDPOINT"),
            service_name: read("OTEL_SERVICE_NAME"),
            resource_attributes: read("OTEL_RESOURCE_ATTRIBUTES"),
            metric_export_interval_ms: read("OTEL_METRIC_EXPORT_INTERVAL")
                .and_then(|v| v.parse().ok()),
            temporality_preference: read("OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE"),
        }
    }

    /// Whether the OTEL endpoint is configured and non-empty.
    pub fn is_enabled(&self) -> bool {
        self.endpoint.as_deref().map(str::trim).is_some_and(|v| !v.is_empty())
    }
}

/// Failures during OTEL initialization. Surfaced to the binary so they crash
/// loudly rather than degrading silently.
#[derive(Debug, Error)]
pub enum InitError {
    #[error("a tracing subscriber is already installed")]
    SubscriberAlreadyInstalled,
    #[error("failed to build OTLP tracer: {0}")]
    Tracer(String),
    #[error("failed to build OTLP meter: {0}")]
    Meter(String),
}

/// Handle returned by `init`. Keep alive for the program lifetime; drop it
/// before the runtime so the exporter's background flush can complete on the
/// same Tokio runtime.
pub struct OtelGuard {
    inner: Option<GuardInner>,
}

struct GuardInner {
    rt_handle: Handle,
    tracer_provider: TracerProvider,
    meter_provider: SdkMeterProvider,
}

impl OtelGuard {
    /// An inert guard — used by the disabled path so every binary has a
    /// uniform return shape.
    pub fn no_op() -> Self {
        Self { inner: None }
    }

    /// Whether this guard owns a live exporter.
    pub fn is_active(&self) -> bool {
        self.inner.is_some()
    }
}

impl Drop for OtelGuard {
    fn drop(&mut self) {
        let Some(inner) = self.inner.take() else {
            return;
        };
        // Shutdown both providers on the same runtime that hosts the exporter
        // background tasks. Ignoring errors — shutdown is best-effort.
        let tracer = inner.tracer_provider;
        let meter = inner.meter_provider;
        inner.rt_handle.block_on(async move {
            let _ = tracer.shutdown();
            let _ = meter.shutdown();
        });
    }
}

/// Initialize OTEL exporters + tracing subscriber.
///
/// The `rt_handle` is captured for background exporter tasks; it MUST remain
/// live until the returned `OtelGuard` is dropped.
pub fn init(cfg: &OtelConfig, rt_handle: &Handle) -> Result<OtelGuard, InitError> {
    if !cfg.is_enabled() {
        // Fully inert disabled path — no subscriber installed, no exporter
        // spun up, no dormant tracing callsites lit up.
        eprintln!("otel disabled (no OTEL_EXPORTER_OTLP_ENDPOINT)");
        return Ok(OtelGuard::no_op());
    }

    // Install the W3C trace-context propagator globally. This is safe to call
    // multiple times in the same process — later callers overwrite with an
    // equivalent propagator.
    global::set_text_map_propagator(TraceContextPropagator::new());

    let endpoint = cfg.endpoint.clone().unwrap_or_default();
    let resource = build_resource(cfg);

    // Construct tracer provider. The batch span processor spawns onto
    // `rt_handle` via the `rt-tokio` runtime.
    let _rt_guard = rt_handle.enter();
    let tracer_provider = build_tracer_provider(&endpoint, resource.clone())
        .map_err(|e| InitError::Tracer(e.to_string()))?;
    let meter_provider = build_meter_provider(&endpoint, resource, cfg)
        .map_err(|e| InitError::Meter(e.to_string()))?;

    let tracer = tracer_provider.tracer("cxdb-otel");
    let otel_layer = tracing_opentelemetry::OpenTelemetryLayer::new(tracer);
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer();

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(otel_layer)
        .try_init()
        .map_err(|_| InitError::SubscriberAlreadyInstalled)?;

    // Wire the tracer provider as the global so callers going through
    // `opentelemetry::global::tracer_provider()` see it.
    global::set_tracer_provider(tracer_provider.clone());

    eprintln!("otel initialized (endpoint={})", endpoint);

    Ok(OtelGuard {
        inner: Some(GuardInner {
            rt_handle: rt_handle.clone(),
            tracer_provider,
            meter_provider,
        }),
    })
}

fn build_resource(cfg: &OtelConfig) -> Resource {
    let mut attrs: Vec<KeyValue> = Vec::new();
    if let Some(name) = cfg.service_name.as_deref() {
        attrs.push(KeyValue::new("service.name", name.to_string()));
    }
    if let Some(raw) = cfg.resource_attributes.as_deref() {
        for pair in raw.split(',') {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            if let Some((k, v)) = pair.split_once('=') {
                attrs.push(KeyValue::new(k.trim().to_string(), v.trim().to_string()));
            }
        }
    }
    Resource::new(attrs)
}

fn build_tracer_provider(
    endpoint: &str,
    resource: Resource,
) -> Result<TracerProvider, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_timeout(Duration::from_secs(10))
        .build()?;

    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter, runtime::Tokio)
        .with_resource(resource)
        .build();

    Ok(provider)
}

fn build_meter_provider(
    endpoint: &str,
    resource: Resource,
    cfg: &OtelConfig,
) -> Result<SdkMeterProvider, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let temporality = match cfg.temporality_preference.as_deref().unwrap_or("delta") {
        "cumulative" => Temporality::Cumulative,
        _ => Temporality::Delta,
    };
    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_timeout(Duration::from_secs(10))
        .with_temporality(temporality)
        .build()?;

    let interval = Duration::from_millis(cfg.metric_export_interval_ms.unwrap_or(60_000));
    let reader = PeriodicReader::builder(exporter, runtime::Tokio)
        .with_interval(interval)
        .build();

    let provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(resource)
        .build();

    // Install as global so `opentelemetry::global::meter(...)` callers pick it up.
    global::set_meter_provider(provider.clone());
    Ok(provider)
}

// Re-export semconv attribute module so downstream crates can pull attribute
// names from a single place.
#[doc(hidden)]
pub use opentelemetry_semantic_conventions::attribute as _semconv_attribute;
