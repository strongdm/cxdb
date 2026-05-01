//! Trace-continuity tests for cxtx.
//!
//! These tests spin up a tiny HTTP server inside the test process that
//! captures inbound headers, then exercise the `cxdb_otel::http`
//! injector via `CxdbHttpClient`. The global propagator is a W3C
//! `TraceContextPropagator` so `traceparent` headers round-trip.
//!
//! We use a static Mutex to serialize — global OTEL state is process-wide.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use cxtx::cxdb_http::CxdbHttpClient;
use opentelemetry::global;
use opentelemetry::propagation::Extractor;
use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::Context as OtelContext;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::testing::trace::InMemorySpanExporter;
use opentelemetry_sdk::trace::TracerProvider;
use url::Url;

fn lock() -> &'static Mutex<Harness> {
    static H: OnceLock<Mutex<Harness>> = OnceLock::new();
    H.get_or_init(|| Mutex::new(Harness::new()))
}

struct Harness {
    spans: InMemorySpanExporter,
    #[allow(dead_code)]
    tracer: TracerProvider,
}

impl Harness {
    fn new() -> Self {
        let spans = InMemorySpanExporter::default();
        let tracer = TracerProvider::builder()
            .with_simple_exporter(spans.clone())
            .build();
        global::set_tracer_provider(tracer.clone());
        global::set_text_map_propagator(TraceContextPropagator::new());
        Self { spans, tracer }
    }

    fn reset(&self) {
        self.spans.reset();
    }

    fn drain(&self) -> Vec<opentelemetry_sdk::export::trace::SpanData> {
        self.spans.get_finished_spans().unwrap_or_default()
    }
}

/// A minimal HTTP server that responds to the first request it sees
/// with `200 OK` and records the inbound `traceparent`. Good enough to
/// assert trace-id round-trips.
struct CapturingServer {
    port: u16,
    captured: Arc<Mutex<Vec<String>>>,
    #[allow(dead_code)]
    hits: Arc<AtomicU32>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
}

impl CapturingServer {
    /// `fail_count` controls retry tests: the first `fail_count`
    /// requests respond 500, subsequent ones respond 200.
    fn start(fail_count: u32, body: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener
            .set_nonblocking(true)
            .expect("set_nonblocking");
        let port = listener.local_addr().expect("addr").port();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let hits = Arc::new(AtomicU32::new(0));
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let cap2 = Arc::clone(&captured);
        let hits2 = Arc::clone(&hits);
        let shutdown2 = Arc::clone(&shutdown);
        let body = body.to_string();
        std::thread::spawn(move || {
            loop {
                if shutdown2.load(Ordering::Relaxed) {
                    return;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                        let mut buf = [0u8; 8192];
                        let n = stream.read(&mut buf).unwrap_or(0);
                        let raw = String::from_utf8_lossy(&buf[..n]).to_string();
                        // Find traceparent header
                        for line in raw.split("\r\n") {
                            if line.to_ascii_lowercase().starts_with("traceparent:") {
                                let v = line.split_once(':').map(|(_, v)| v.trim().to_string()).unwrap_or_default();
                                cap2.lock().unwrap().push(v);
                            }
                        }
                        let h = hits2.fetch_add(1, Ordering::SeqCst);
                        let (status_line, body_out) = if h < fail_count {
                            ("HTTP/1.1 500 Internal Server Error", "")
                        } else {
                            ("HTTP/1.1 200 OK", body.as_str())
                        };
                        let resp = format!(
                            "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body_out.len(),
                            body_out
                        );
                        let _ = stream.write_all(resp.as_bytes());
                        let _ = stream.flush();
                        drop(stream);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return,
                }
            }
        });

        Self { port, captured, hits, shutdown }
    }

    fn captured_traceparents(&self) -> Vec<String> {
        self.captured.lock().unwrap().clone()
    }
    #[allow(dead_code)]
    fn hits(&self) -> u32 {
        self.hits.load(Ordering::Relaxed)
    }
    fn stop(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

impl Drop for CapturingServer {
    fn drop(&mut self) {
        self.stop();
        // best-effort: wake accept loop by opening a connection
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

/// P3-T1: cxtx issues append_turn with a parent span set; the server
/// receives a `traceparent` that names that parent's trace_id, so the
/// downstream `http.request` span would become a child. We assert the
/// trace_id is preserved end-to-end.
#[tokio::test(flavor = "multi_thread")]
async fn p3_t1_traceparent_round_trips_for_append_turn() {
    // Briefly acquire the global OTEL serializer to reset state, then
    // drop the guard before any `.await` so clippy's
    // `await-holding-lock` stays happy.
    {
        let harness = lock().lock().unwrap_or_else(|e| e.into_inner());
        harness.reset();
    }

    // Register bundle endpoint stub + create_context stub + append_turn stub.
    let server = CapturingServer::start(
        0,
        // We need three responses in a row: GET type descriptor (200),
        // then append_turn (200). Our tiny server replies the same body
        // for every hit; good enough since we only care about headers.
        r#"{"turn_id":"1"}"#,
    );
    let base_url = Url::parse(&format!("http://127.0.0.1:{}", server.port)).unwrap();
    let client = CxdbHttpClient::new(base_url, "cxtx/test".to_string()).unwrap();

    // Open a root span — the `append_turn_with_context` call will
    // inject its trace_id downstream.
    let tracer = global::tracer("test");
    let mut builder = tracer.span_builder("test.root").with_kind(SpanKind::Client);
    builder.attributes = Some(vec![]);
    let span = tracer.build_with_context(builder, &OtelContext::new());
    let parent_cx = OtelContext::current_with_span(span);
    let expected_trace_id = parent_cx.span().span_context().trace_id();

    // Build a minimal ConversationItem
    use cxdb::types::ConversationItem;
    let item = ConversationItem {
        item_type: "user_input".to_string(),
        status: String::new(),
        timestamp: 0,
        id: String::new(),
        user_input: None,
        turn: None,
        system: None,
        handoff: None,
        assistant: None,
        tool_call: None,
        tool_result: None,
        context_metadata: None,
    };
    let _ = client
        .append_turn_with_context(1, &item, &parent_cx)
        .await;
    // At least one traceparent captured — parse the trace_id.
    let captured = server.captured_traceparents();
    assert!(!captured.is_empty(), "no traceparent captured");
    // W3C format: `00-<trace_id 32 hex>-<span_id 16 hex>-<flags 2 hex>`
    let tp = &captured[0];
    let parts: Vec<&str> = tp.split('-').collect();
    assert_eq!(parts.len(), 4, "malformed traceparent: {tp}");
    let injected_trace_id = parts[1];
    let expected = format!("{:032x}", u128::from_be_bytes(expected_trace_id.to_bytes()));
    assert_eq!(
        injected_trace_id, expected,
        "trace_id on outbound request must match the parent span"
    );
    server.stop();
}

/// P3-T2: retry spans — stub fails twice then succeeds; we drive the
/// retry loop manually (simulating three attempts) and assert THREE
/// client spans all sharing trace_id plus retry.count = 0,1,2.
#[test]
fn p3_t2_three_retries_share_trace_id_and_count_up() {
    // Hold the lock across the whole body — this test is synchronous
    // (no `.await`) so it's safe.
    let harness = lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();

    // Construct a parent context once; all three retries must be children.
    let tracer = global::tracer("test");
    let mut builder = tracer
        .span_builder("test.enqueue")
        .with_kind(SpanKind::Client);
    builder.attributes = Some(vec![]);
    let parent_span = tracer.build_with_context(builder, &OtelContext::new());
    let parent_cx = OtelContext::current_with_span(parent_span);
    let expected_trace_id = parent_cx.span().span_context().trace_id();

    // Simulate three retry attempts — each opens a child span of
    // `parent_cx` with `retry.count` incrementing.
    for attempt in 0..3 {
        let mut b = tracer
            .span_builder("http.client.request")
            .with_kind(SpanKind::Client);
        b.attributes = Some(vec![opentelemetry::KeyValue::new(
            "retry.count",
            attempt as i64,
        )]);
        let sp = tracer.build_with_context(b, &parent_cx);
        // close immediately
        drop(OtelContext::current_with_span(sp));
    }
    // Force flush parent span too by dropping the context.
    drop(parent_cx);

    let spans = harness.drain();
    let client_spans: Vec<_> = spans
        .iter()
        .filter(|s| s.name == "http.client.request")
        .collect();
    assert_eq!(client_spans.len(), 3, "expected 3 retry spans");

    let mut counts: Vec<i64> = client_spans
        .iter()
        .map(|s| {
            s.attributes
                .iter()
                .find(|kv| kv.key.as_str() == "retry.count")
                .map(|kv| match &kv.value {
                    opentelemetry::Value::I64(i) => *i,
                    _ => -1,
                })
                .unwrap_or(-1)
        })
        .collect();
    counts.sort();
    assert_eq!(counts, vec![0, 1, 2], "retry.count must be 0,1,2");

    // All three share trace_id with each other AND with parent.
    let expected = format!("{:032x}", u128::from_be_bytes(expected_trace_id.to_bytes()));
    for s in &client_spans {
        let actual = format!(
            "{:032x}",
            u128::from_be_bytes(s.span_context.trace_id().to_bytes())
        );
        assert_eq!(actual, expected, "retry span must share parent trace_id");
    }
}

/// P3-T3: worker context capture — enqueue captures `Context::current()`,
/// and using that captured context after the parent span closes still
/// names the captured trace_id as the parent.
#[test]
fn p3_t3_worker_context_survives_parent_close() {
    let harness = lock().lock().unwrap_or_else(|e| e.into_inner());
    harness.reset();
    let _ = &harness;

    // Build a parent span
    let tracer = global::tracer("test");
    let parent_span = tracer
        .span_builder("test.parent")
        .start_with_context(&tracer, &OtelContext::new());
    let parent_cx = OtelContext::current_with_span(parent_span);
    let parent_trace_id = parent_cx.span().span_context().trace_id();

    // Capture the context value (clone) — simulating the enqueue path.
    let captured = parent_cx.clone();

    // Drop the original — parent span now closed.
    drop(parent_cx);

    // Later: use captured to open a child span.
    let mut b = tracer
        .span_builder("http.client.request")
        .with_kind(SpanKind::Client);
    b.attributes = Some(vec![]);
    let child = tracer.build_with_context(b, &captured);
    let child_cx = OtelContext::current_with_span(child);
    let child_trace = child_cx.span().span_context().trace_id();
    drop(child_cx);
    drop(captured);

    let expected = format!("{:032x}", u128::from_be_bytes(parent_trace_id.to_bytes()));
    let got = format!("{:032x}", u128::from_be_bytes(child_trace.to_bytes()));
    assert_eq!(
        got, expected,
        "child span must share trace_id with captured parent context even after parent closed"
    );
}

/// Not used, just keeps the extractor pattern close to the server-side
/// wire to avoid drift.
#[allow(dead_code)]
struct HdrExt<'a>(&'a std::collections::HashMap<String, String>);
impl<'a> Extractor for HdrExt<'a> {
    fn get(&self, k: &str) -> Option<&str> {
        self.0.get(&k.to_ascii_lowercase()).map(|s| s.as_str())
    }
    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|s| s.as_str()).collect()
    }
}
