// Clippy allows for pre-existing (pre-sprint-016) lints across the cxtx
// proxy / delivery surfaces. Kept at the crate level to avoid churn in
// unrelated call sites while this sprint's DoD demands a clean
// `clippy -D warnings` run.
#![allow(
    clippy::collapsible_if,
    clippy::large_enum_variant,
    clippy::manual_async_fn,
    clippy::needless_borrow,
    clippy::needless_borrows_for_generic_args,
    clippy::result_large_err,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::useless_conversion,
    clippy::useless_format
)]

pub mod cli;
pub mod cxdb_http;
pub mod delivery;
pub mod ledger;
pub mod otel;
pub mod provider;
pub mod proxy;
pub mod session;
pub mod turns;

/// Cross-module test helpers. Not public API — gated on `cfg(test)` so
/// the helper is only compiled during test builds, but also `pub`
/// enough that tests in any module can lock a process-wide Mutex.
#[cfg(test)]
pub(crate) mod test_sync {
    use std::sync::{Mutex, MutexGuard};

    /// Sprint 021: shared lock for tests that mutate `CXTX_TENANT` (or
    /// any other process-global env). `cargo test` runs test functions
    /// in parallel within a binary — concurrent env writes flake.
    /// Every test that reads or writes these env vars MUST hold
    /// `env_lock()` for its duration.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Acquire the env lock (poisoned guard recovery included).
    pub fn env_lock() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }
}

use anyhow::{Context, Result};
use cli::Cli;
use delivery::DeliveryHandle;
use ledger::SessionLedgerWriter;
use provider::ProviderKind;
use proxy::ProxyServer;
use session::SessionRuntime;
use std::process::Stdio;
use tokio::process::Command;

pub async fn run(cli: Cli) -> Result<i32> {
    let provider = cli.command.provider();
    let effective_url = cli.effective_url();
    let cxdb_url = cli
        .effective_url()
        .parse()
        .with_context(|| format!("invalid CXDB URL: {effective_url}"))?;

    let upstream = provider
        .resolve_upstream_base()
        .context("failed to resolve provider upstream base URL")?;
    let (listener, proxy_base_url) = ProxyServer::bind(provider, &upstream)
        .await
        .context("failed to reserve local reverse proxy listener")?;
    let args = provider.child_args_for_proxy(cli.command.args(), Some(&proxy_base_url));
    let allowlisted_env = provider.capture_env_allowlist();
    let session = SessionRuntime::new(provider, args.clone(), allowlisted_env)?;
    let ledger = SessionLedgerWriter::create(&session).await?;

    let proxy = ProxyServer::start_with_listener(
        provider,
        upstream,
        session.clone(),
        ledger.clone(),
        listener,
        proxy_base_url.clone(),
    )
    .await
    .context("failed to start local reverse proxy")?;
    let delivery = DeliveryHandle::start(
        cxdb_url,
        session.clone(),
        ledger.clone(),
        provider.client_tag().to_string(),
    )
    .await?;
    proxy.set_delivery(delivery.clone()).await;

    let mut command = Command::new(provider.command_name());
    command.args(&args);
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());
    for name in provider.upstream_base_env_names() {
        command.env_remove(name);
    }
    command.envs(provider.injected_env(&proxy_base_url));

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            ledger
                .note_delivery_state("child_launch_failed", 0, Some(err.to_string()))
                .await
                .ok();
            delivery.shutdown().await.ok();
            proxy.shutdown().await.ok();
            ledger.finalize().await.ok();
            return Err(err).with_context(|| {
                format!(
                    "failed to launch {} using PATH resolution",
                    provider.command_name()
                )
            });
        }
    };
    session.set_child_pid(child.id());
    ledger.note_child_pid(child.id()).await?;

    delivery.enqueue_create_context().await?;
    delivery.enqueue_turn(session.session_start_turn()).await?;

    let status = child.wait().await?;
    let exit_code = status.code().unwrap_or(1);

    delivery
        .enqueue_turn(session.session_end_turn(exit_code, status.success()))
        .await?;
    ledger.note_child_exit(exit_code).await?;

    proxy.shutdown().await?;
    delivery.shutdown().await?;
    ledger.finalize().await?;

    Ok(exit_code)
}

pub async fn run_provider_command(
    provider: ProviderKind,
    args: Vec<String>,
    url: &str,
) -> Result<i32> {
    run(Cli::for_tests(provider, args, url)).await
}
