use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use clap::Parser;
use x402_facilitator_local::util::SigDown;
use x402_near_facilitator::auth::ApiKeyAuthenticator;
use x402_near_facilitator::bootstrap::build_chain_provider;
use x402_near_facilitator::catalog::Catalog;
use x402_near_facilitator::config::{OtelConfig, SecretFiles, ServiceConfig};
use x402_near_facilitator::leadership::{LeadershipHandle, ReadinessState};
use x402_near_facilitator::service::{AppState, reconcile, router};
use x402_near_facilitator::store::PgStore;
use x402_near_facilitator::telemetry::TelemetryGuard;

#[derive(Debug, Parser)]
#[command(
    name = "x402-near-facilitator",
    version,
    about = "Durable NEAR and Base x402 facilitator"
)]
struct Cli {
    /// Non-secret JSON service configuration.
    #[arg(long)]
    config: PathBuf,
}

#[tokio::main]
// Startup is kept as one ordered fail-closed sequence so no listener can
// become ready before configuration, storage, leadership, and recovery.
#[allow(clippy::too_many_lines)]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut config = ServiceConfig::load(&cli.config).context("load service config")?;
    let secret_files = SecretFiles::from_environment().context("locate credentials")?;
    let secrets = secret_files.load().context("load credentials")?;
    config
        .apply_rpc_url_overrides(
            secrets.primary_rpc_url.as_ref().map(|value| value.as_str()),
            secrets.backup_rpc_url.as_ref().map(|value| value.as_str()),
        )
        .context("apply protected RPC configuration")?;
    let catalog = Catalog::load_embedded_for(&config).context("load embedded discovery catalog")?;
    let otel = OtelConfig::from_environment().context("load telemetry configuration")?;
    let telemetry = TelemetryGuard::initialize(config.environment, otel.as_ref())
        .context("initialize telemetry")?;

    let store = PgStore::connect(
        secrets.database_url.as_str(),
        config.database_max_connections,
    )
    .await
    .context("connect application database")?;
    // Migrations are intentionally never run by the service process.
    ensure!(
        store
            .schema_compatible()
            .await
            .context("validate database schema")?,
        "database schema is missing, incomplete, or incompatible"
    );
    let auth = ApiKeyAuthenticator::new(
        store.clone(),
        config.environment,
        secrets.api_key_pepper.as_bytes(),
    )
    .context("initialize API authentication")?;

    // The settlement provider and relayer-key algorithm are chain-specific; the
    // one ordered construction sequence lives in `bootstrap` so this binary and
    // the admin `reconcile` command cannot drift. NEAR yields a registered
    // facilitator; EVM yields `None` and serves /verify through the neutral
    // provider.
    let (facilitator, provider) = build_chain_provider(&config, secrets.relayer_key.as_str())
        .await
        .context("construct settlement provider")?;

    // Register the relayer/signer identity for policy checks. The neutral
    // accessors yield the NEAR account id + ed25519 key, or the EVM signer address
    // (doubling as id and key), so the store's relayer-policy keys stay
    // chain-consistent.
    store
        .upsert_relayer(
            &config.network,
            &provider.signer_account_id(),
            &provider.signer_public_key(),
        )
        .await
        .context("register relayer identity")?;
    let readiness = ReadinessState::default();
    let state = AppState::new(
        config.clone(),
        store.clone(),
        auth,
        facilitator,
        provider,
        readiness.clone(),
        catalog,
        telemetry.metrics(),
    );
    state.refresh_chain_readiness().await;
    let leadership = LeadershipHandle::spawn(
        secrets.database_direct_url,
        &config.network,
        readiness.clone(),
    );

    let reconciliation_state = state.clone();
    let reconciliation_task = tokio::spawn(async move {
        loop {
            let snapshot = reconciliation_state.readiness().snapshot();
            if snapshot.leadership
                && !snapshot.reconciliation
                && reconcile(&reconciliation_state).await.is_err()
            {
                tracing::warn!(event = "startup_reconciliation_failed");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
    let monitor_state = state.clone();
    let readiness_task = tokio::spawn(async move {
        run_readiness_monitor(Duration::from_secs(15), move || {
            let state = monitor_state.clone();
            async move {
                state.refresh_chain_readiness().await;
            }
        })
        .await;
    });

    let listener = tokio::net::TcpListener::bind(config.bind_address)
        .await
        .context("bind HTTP listener")?;
    let signals = SigDown::try_new().context("register shutdown signals")?;
    let cancellation = signals.cancellation_token();
    tracing::info!(
        event = "service_started",
        environment = ?config.environment,
        network = %config.network,
        bind = %config.bind_address,
        version = x402_near_facilitator::VERSION,
    );
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            cancellation.cancelled().await;
        })
        .await
        .context("serve HTTP")?;

    reconciliation_task.abort();
    readiness_task.abort();
    leadership.shutdown().await;
    Ok(())
}

fn readiness_refresh_interval(period: Duration) -> tokio::time::Interval {
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval
}

async fn run_readiness_monitor<Refresh, RefreshFuture>(period: Duration, mut refresh: Refresh)
where
    Refresh: FnMut() -> RefreshFuture,
    RefreshFuture: Future<Output = ()>,
{
    let mut interval = readiness_refresh_interval(period);
    // Startup already took a synchronous readiness snapshot before the listener
    // can become ready. Tokio intervals tick immediately, so consume that tick
    // rather than issuing the same dual-RPC snapshot twice at startup.
    interval.tick().await;
    loop {
        interval.tick().await;
        refresh().await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::run_readiness_monitor;

    #[tokio::test(start_paused = true)]
    async fn readiness_monitor_waits_a_full_period_after_the_startup_snapshot() {
        let refreshes = Arc::new(AtomicUsize::new(0));
        let monitor_refreshes = Arc::clone(&refreshes);
        let monitor = tokio::spawn(run_readiness_monitor(Duration::from_secs(15), move || {
            let refreshes = Arc::clone(&monitor_refreshes);
            async move {
                refreshes.fetch_add(1, Ordering::SeqCst);
            }
        }));

        tokio::task::yield_now().await;
        assert_eq!(refreshes.load(Ordering::SeqCst), 0);

        tokio::time::advance(Duration::from_secs(14)).await;
        tokio::task::yield_now().await;
        assert_eq!(refreshes.load(Ordering::SeqCst), 0);

        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(refreshes.load(Ordering::SeqCst), 1);

        tokio::time::advance(Duration::from_secs(15)).await;
        tokio::task::yield_now().await;
        assert_eq!(refreshes.load(Ordering::SeqCst), 2);

        monitor.abort();
        assert!(monitor.await.is_err());
    }
}
