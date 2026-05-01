//! Test helpers. Not linked into production paths.

use std::sync::OnceLock;

use tokio::runtime::Handle;

use crate::{init, InitError, OtelConfig, OtelGuard};

/// Install a subscriber exactly once per test process. Subsequent calls
/// return `None`; the first call returns the real guard. Tests that merely
/// need *some* subscriber in place (e.g., to verify tracing spans are not
/// dropped) should use this — tests that need to capture spans use an
/// `InMemorySpanExporter` per-test and should NOT rely on the global.
static INSTALLED: OnceLock<()> = OnceLock::new();

pub fn install_once(
    cfg: &OtelConfig,
    rt_handle: &Handle,
) -> Result<Option<OtelGuard>, InitError> {
    if INSTALLED.get().is_some() {
        return Ok(None);
    }
    let guard = init(cfg, rt_handle)?;
    INSTALLED.set(()).ok();
    Ok(Some(guard))
}
