//! P1-T4: subscriber-collision hard-fail regression.
//!
//! Calling `cxdb_otel::init` twice with an enabled endpoint must return
//! `Err(InitError::SubscriberAlreadyInstalled)` on the second call.

use cxdb_otel::{init, InitError, OtelConfig};

#[test]
fn second_init_fails_loudly() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let cfg = OtelConfig {
        endpoint: Some("http://127.0.0.1:14317".to_string()),
        ..Default::default()
    };

    // First init installs the subscriber and tracer provider.
    let handle = rt.handle().clone();
    let guard = init(&cfg, &handle).expect("first init succeeds");
    assert!(guard.is_active(), "first init must produce a live guard");

    // Second init must fail loudly — do NOT demote to warn.
    let second = init(&cfg, &handle);
    match second {
        Err(InitError::SubscriberAlreadyInstalled) => {}
        Err(other) => panic!("expected SubscriberAlreadyInstalled, got {other:?}"),
        Ok(_) => panic!("expected SubscriberAlreadyInstalled, got Ok"),
    }

    // Explicit drop order: guard, then runtime.
    drop(guard);
    drop(rt);
}
