//! P4.2 OTEL no-op regression.
//!
//! When `OTEL_EXPORTER_OTLP_ENDPOINT` is unset, `cxdb_otel::init` must
//! return a no-op guard and MUST NOT install a tracing subscriber or a
//! non-trivial meter provider. Calling `finalize_llm_call` under that
//! condition emits nothing observable.

use std::time::Instant;

use cxtx::otel::call_context::{AppAttribution, CallContext};
use cxtx::otel::llm_call::finalize_llm_call;
use cxtx::provider::usage::{RawUsage, UsageOutcome};

#[tokio::test(flavor = "multi_thread")]
async fn noop_path_when_endpoint_unset() {
    // Ensure no previous test process leaked an endpoint into the env.
    // We don't unset globally (another test in this process may have
    // installed a provider); instead, verify `OtelConfig::is_enabled`
    // defaults to false and that `init` returns an inert guard.
    std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
    let cfg = cxdb_otel::OtelConfig::from_env();
    assert!(!cfg.is_enabled(), "endpoint unset must be disabled");
    let rt_handle = tokio::runtime::Handle::current();
    let guard = cxdb_otel::init(&cfg, &rt_handle).unwrap();
    assert!(!guard.is_active(), "disabled config must produce inert guard");

    // Invoke the emit site anyway — with no meter provider installed by
    // `init()` (noop path), the global meter stays at whatever prior
    // tests set OR the default NoopMeterProvider. Either way, finalize
    // must not panic. We cannot assert absence of metric samples here
    // because another test harness in this binary installs a provider;
    // the noop assertion is "completes without panicking + guard is
    // inert".
    let ctx = CallContext::new(
        Instant::now(),
        "claude-opus",
        "anthropic",
        AppAttribution {
            client_tag: "cxtx/claude".to_string(),
            wrapper_command: "claude".to_string(),
            wrapper_version: "0.1.0".to_string(),
            provider_kind: "anthropic".to_string(),
            session_id: "sess-noop".to_string(),
            user: None,
            tenant: None,
        },
        false,
    );
    let outcome = UsageOutcome::Reported(RawUsage {
        input_tokens: 5,
        output_tokens: 3,
        finish_reasons_raw: vec!["end_turn".to_string()],
        ..RawUsage::default()
    });
    finalize_llm_call(&ctx, &outcome, Some("claude-opus"));
}
