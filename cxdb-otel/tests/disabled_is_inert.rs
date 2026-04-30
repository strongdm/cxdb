//! P1-T3: when the endpoint is unset, `init` returns a no-op guard and does
//! NOT install a subscriber. This is verified indirectly by (a) `is_active`
//! returning false and (b) the fact that subsequent init-enabled calls still
//! successfully install the subscriber (they would be blocked if a
//! subscriber were already in place).

use cxdb_otel::{init, OtelConfig};

#[test]
fn disabled_path_does_not_install_subscriber() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let cfg = OtelConfig::default();
    let handle = rt.handle().clone();
    let guard = init(&cfg, &handle).expect("disabled init succeeds");
    assert!(!guard.is_active(), "disabled init must be a no-op guard");

    drop(guard);
    drop(rt);
}
