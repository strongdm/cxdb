//! Minimal emit surface for the `gen_ai.*` metric family.
//!
//! Exactly three helpers:
//!
//! - `emit_token_usage` — histogram samples for `gen_ai.client.token.usage`
//! - `emit_calls` — per-call counter `gen_ai.calls`
//! - `emit_usage_missing` — breadcrumb counter `gen_ai.usage_missing`
//!
//! Domain logic (bucket derivation, finish-reason mapping) lives in
//! `cxtx::otel::*`; this module only wraps the OpenTelemetry meter API
//! with lazy `OnceLock` instrument creation and a uniform attribute
//! builder.
//!
//! Cardinality view configuration (dropping `app.session_id` / `app.user`
//! / `app.wrapper_version` on all three metric names) is NOT applied via
//! otel views — this crate emits only the attributes it's asked to, and
//! callers are responsible for not passing the dropped attributes. The
//! per-metric attribute surface is documented per helper.

use std::borrow::Cow;

use opentelemetry::metrics::{Counter, Histogram};
use opentelemetry::{global, KeyValue};

/// A lightweight, clonable attribute builder used at every `gen_ai.*`
/// emit site. Keeps per-call allocation minimal and presents a single
/// call-shape across the three helpers.
#[derive(Debug, Clone, Default)]
pub struct Attrs {
    pairs: Vec<(Cow<'static, str>, Cow<'static, str>)>,
}

impl Attrs {
    pub fn new() -> Self {
        Self { pairs: Vec::new() }
    }

    /// Append a `(key, value)` pair. Both key and value may be `&'static
    /// str` (zero-copy) or owned `String` (copies into `Cow::Owned`).
    pub fn with(
        mut self,
        key: impl Into<Cow<'static, str>>,
        value: impl Into<Cow<'static, str>>,
    ) -> Self {
        self.pairs.push((key.into(), value.into()));
        self
    }

    /// Snapshot into the OpenTelemetry `KeyValue` slice shape expected by
    /// the meter API.
    pub fn to_kvs(&self) -> Vec<KeyValue> {
        self.pairs
            .iter()
            .map(|(k, v)| KeyValue::new(k.clone().into_owned(), v.clone().into_owned()))
            .collect()
    }

    /// Accessor for tests + callers that need to inspect which attributes
    /// are being emitted (e.g., the cardinality-view test).
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.pairs.iter().map(|(k, _)| k.as_ref())
    }
}

// Instruments are recreated per call. The meter provider's SDK
// deduplicates by (name, kind, unit, description) so instrument identity
// is preserved across calls; constructing a fresh handle here is an
// O(HashMap lookup) op inside the SDK, cheap enough per LLM call while
// remaining test-friendly (tests that swap the global meter provider
// don't get stuck with a handle pinned to the prior NoopMeterProvider).
fn token_usage() -> Histogram<u64> {
    global::meter("cxdb")
        .u64_histogram("gen_ai.client.token.usage")
        .with_unit("{token}")
        .with_description(
            "LLM token usage per non-zero bucket (input/cached/output/reasoning/cache_write*).",
        )
        .build()
}

fn calls() -> Counter<u64> {
    global::meter("cxdb")
        .u64_counter("gen_ai.calls")
        .with_unit("{call}")
        .with_description("LLM calls finalized with reported usage (unsampled).")
        .build()
}

fn usage_missing() -> Counter<u64> {
    global::meter("cxdb")
        .u64_counter("gen_ai.usage_missing")
        .with_unit("1")
        .with_description(
            "LLM calls finalized without usable usage; reason=error|not_reported|invalid.",
        )
        .build()
}

/// Emit one histogram sample per non-zero bucket. `buckets` is expected
/// to already be filtered (zero-valued entries skipped) by
/// `cxtx::otel::buckets::derive_and_validate`.
pub fn emit_token_usage(buckets: &[(impl TokenTypeName, u64)], attrs: &Attrs) {
    let base = attrs.to_kvs();
    let hist = token_usage();
    for (token_type, value) in buckets {
        if *value == 0 {
            continue;
        }
        let mut kvs = base.clone();
        kvs.push(KeyValue::new(
            "gen_ai.token.type",
            token_type.token_type_name().to_string(),
        ));
        hist.record(*value, &kvs);
    }
}

/// Increment `gen_ai.calls` by 1.
pub fn emit_calls(attrs: &Attrs) {
    calls().add(1, &attrs.to_kvs());
}

/// Increment `gen_ai.usage_missing` by 1. The `reason` attribute MUST be
/// present on `attrs` (the helper does not synthesize a default — callers
/// explicitly stamp `error` / `not_reported` / `invalid` per the dispatch
/// table).
pub fn emit_usage_missing(attrs: &Attrs) {
    usage_missing().add(1, &attrs.to_kvs());
}

/// Abstraction so callers in `cxtx::otel::buckets` can pass their
/// `TokenType` enum directly without depending on this crate.
pub trait TokenTypeName {
    fn token_type_name(&self) -> &str;
}

impl TokenTypeName for &str {
    fn token_type_name(&self) -> &str {
        self
    }
}

impl TokenTypeName for String {
    fn token_type_name(&self) -> &str {
        self.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attrs_round_trip() {
        let a = Attrs::new()
            .with("gen_ai.system", "anthropic")
            .with("app.client_tag", String::from("cxtx/claude"));
        let kvs = a.to_kvs();
        assert_eq!(kvs.len(), 2);
        let keys: Vec<&str> = a.keys().collect();
        assert_eq!(keys, vec!["gen_ai.system", "app.client_tag"]);
    }
}
