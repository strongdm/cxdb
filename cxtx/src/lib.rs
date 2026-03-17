pub mod cli;
pub mod cxdb_http;
pub mod delivery;
pub mod ledger;
pub mod provider;
pub mod proxy;
pub mod session;
pub mod turns;

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
    let args = cli.command.args().to_vec();
    let cxdb_url = cli
        .url
        .parse()
        .with_context(|| format!("invalid CXDB URL: {}", cli.url))?;

    let upstream = provider
        .resolve_upstream_base()
        .context("failed to resolve provider upstream base URL")?;
    let allowlisted_env = provider.capture_env_allowlist();
    let session = SessionRuntime::new(provider, args.clone(), allowlisted_env)?;
    let ledger = SessionLedgerWriter::create(&session).await?;

    let proxy = ProxyServer::start(provider, upstream, session.clone(), ledger.clone())
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
    command.envs(provider.injected_env(&proxy.proxy_base_url()));

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

    delivery.shutdown().await?;
    proxy.shutdown().await?;
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
