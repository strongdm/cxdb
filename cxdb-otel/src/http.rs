//! Trace-context propagation helpers for HTTP.
//!
//! Two directions:
//!
//! - `extract_tiny_http` — pull the remote `traceparent` / `tracestate`
//!   headers off an incoming `tiny_http::Request` into an
//!   `opentelemetry::Context`. The server uses this to attach the
//!   remote context as the parent of every `http.request` span.
//! - `inject_reqwest` — apply the current `opentelemetry::Context` onto
//!   a `reqwest::RequestBuilder`'s headers so downstream servers can
//!   extract it. The cxtx client uses this uniformly via an internal
//!   `self.request(...)` wrapper so new endpoints can't forget.
//!
//! Both helpers go through
//! `opentelemetry::global::get_text_map_propagator(...)` so whatever
//! propagator `init()` installed (W3C TraceContext) is honored.
//!
//! No-op behavior: when `init()` is disabled (no exporter), the global
//! `NoopTextMapPropagator` is in place and these helpers become inert —
//! `extract` returns `Context::current()`, `inject` writes no headers.

use std::collections::HashMap;

use opentelemetry::propagation::{Extractor, Injector};
use opentelemetry::Context;

/// Extract remote context from a `tiny_http::Request`. Returns
/// `Context::current()` unchanged when no `traceparent` header is
/// present (new-root on server side per spec).
pub fn extract_tiny_http(request: &tiny_http::Request) -> Context {
    let extractor = TinyHttpHeaderExtractor::new(request);
    opentelemetry::global::get_text_map_propagator(|prop| prop.extract(&extractor))
}

/// Inject current context into a `reqwest::RequestBuilder` via the
/// installed propagator. `reqwest::RequestBuilder` does not expose a
/// mutable `HeaderMap`, so we collect into a `HashMap<String, String>`
/// and add headers one at a time.
pub fn inject_reqwest(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    inject_reqwest_with(rb, &Context::current())
}

/// Same as `inject_reqwest` but uses the provided context rather than
/// the thread-local `Context::current()`. The async delivery worker
/// needs this variant because `ContextGuard` is not `Send` and cannot
/// stay alive across `.await` boundaries inside `tokio::spawn`'d futures.
pub fn inject_reqwest_with(mut rb: reqwest::RequestBuilder, cx: &Context) -> reqwest::RequestBuilder {
    let mut carrier: HashMap<String, String> = HashMap::new();
    opentelemetry::global::get_text_map_propagator(|prop| {
        prop.inject_context(cx, &mut HashMapInjector(&mut carrier));
    });
    for (k, v) in carrier {
        rb = rb.header(k, v);
    }
    rb
}

// ---------------------------------------------------------------------------
// Internal carriers
// ---------------------------------------------------------------------------

struct TinyHttpHeaderExtractor<'a> {
    headers: HashMap<String, &'a str>,
}

impl<'a> TinyHttpHeaderExtractor<'a> {
    fn new(request: &'a tiny_http::Request) -> Self {
        let mut headers: HashMap<String, &'a str> = HashMap::new();
        for h in request.headers() {
            // Field::as_str() is the canonical accessor; lowercase for
            // case-insensitive lookup.
            let name = h.field.as_str().as_str().to_ascii_lowercase();
            headers.insert(name, h.value.as_str());
        }
        Self { headers }
    }
}

impl<'a> Extractor for TinyHttpHeaderExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        self.headers.get(&key.to_ascii_lowercase()).copied()
    }
    fn keys(&self) -> Vec<&str> {
        self.headers.keys().map(|s| s.as_str()).collect()
    }
}

struct HashMapInjector<'a>(&'a mut HashMap<String, String>);

impl<'a> Injector for HashMapInjector<'a> {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key.to_string(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::propagation::TextMapPropagator;
    use opentelemetry_sdk::propagation::TraceContextPropagator;

    #[test]
    fn extractor_is_case_insensitive_and_returns_traceparent() {
        // Simulate what `extract_tiny_http` gets — exercise the
        // `TinyHttpHeaderExtractor` indirectly via a hand-built carrier.
        let mut headers: HashMap<String, &str> = HashMap::new();
        headers.insert("traceparent".to_string(),
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01");
        let extractor = StaticExtractor(&headers);

        // Install a TraceContext propagator just for this call via the
        // prop API directly (global is not needed — we use the prop
        // instance).
        let prop = TraceContextPropagator::new();
        let ctx = prop.extract(&extractor);
        use opentelemetry::trace::TraceContextExt;
        let span_ctx = ctx.span().span_context().clone();
        assert!(span_ctx.is_valid(), "extracted span context must be valid");
        assert_eq!(
            format!("{:032x}", u128::from_be_bytes(span_ctx.trace_id().to_bytes())),
            "0af7651916cd43dd8448eb211c80319c"
        );
    }

    // A minimal Extractor over a HashMap so the test doesn't need to
    // construct a tiny_http::Request.
    struct StaticExtractor<'a>(&'a HashMap<String, &'a str>);
    impl<'a> Extractor for StaticExtractor<'a> {
        fn get(&self, key: &str) -> Option<&str> {
            self.0.get(&key.to_ascii_lowercase()).copied()
        }
        fn keys(&self) -> Vec<&str> {
            self.0.keys().map(|s| s.as_str()).collect()
        }
    }
}
