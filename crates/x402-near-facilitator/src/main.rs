use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use clap::Parser;
use near_crypto::{InMemorySigner, SecretKey};
use near_primitives::types::AccountId;
use x402_chain_eip155_provider::provider::EvmChainProvider;
use x402_chain_near::{JsonRpcNearRpc, NearChainProvider, NearNetwork, NearRpc, V2NearExact};
use x402_facilitator_local::{FacilitatorLocal, util::SigDown};
use x402_near_facilitator::auth::ApiKeyAuthenticator;
use x402_near_facilitator::chain::ChainProvider;
use x402_near_facilitator::config::{
    ChainKind, Environment, OtelConfig, SecretFiles, ServiceConfig,
};
use x402_near_facilitator::leadership::{LeadershipHandle, ReadinessState};
use x402_near_facilitator::service::{AppState, reconcile, router};
use x402_near_facilitator::store::PgStore;
use x402_near_facilitator::telemetry::TelemetryGuard;
use x402_types::chain::{ChainIdPattern, ChainProviderOps, ChainRegistry};
use x402_types::scheme::{SchemeBlueprints, SchemeConfig, SchemeRegistry};

#[derive(Debug, Parser)]
#[command(
    name = "x402-near-facilitator",
    version,
    about = "Durable NEAR x402 facilitator"
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
    let config = ServiceConfig::load(&cli.config).context("load service config")?;
    let secret_files = SecretFiles::from_environment().context("locate credentials")?;
    let secrets = secret_files.load().context("load credentials")?;
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

    // The settlement provider and relayer-key algorithm are chain-specific: NEAR
    // uses an ed25519 delegate signer plus the registered V2NearExact facilitator;
    // EVM uses a secp256k1 signer and serves /verify directly through the neutral
    // provider (no registered facilitator — the upstream V2Eip155Exact blueprint
    // is generic over `&P` and cannot be registered via this assembly). `validate`
    // has already accepted the chain-appropriate config.
    let (facilitator, provider): (Option<FacilitatorLocal<SchemeRegistry>>, ChainProvider) =
        match config.chain_kind {
            ChainKind::Near => {
                let relayer_account = AccountId::from_str(&config.relayer_account_id)
                    .context("parse relayer account")?;
                let secret_key = SecretKey::from_str(secrets.relayer_key.as_str())
                    .context("parse relayer service key")?;
                let signer = InMemorySigner::from_secret_key(relayer_account, secret_key);
                let primary: Arc<JsonRpcNearRpc> =
                    Arc::new(JsonRpcNearRpc::connect(config.primary_rpc_url.as_str()));
                let backup: Arc<JsonRpcNearRpc> =
                    Arc::new(JsonRpcNearRpc::connect(config.backup_rpc_url.as_str()));
                let network = match config.environment {
                    Environment::Mainnet => NearNetwork::Mainnet,
                    Environment::Testnet => NearNetwork::Testnet,
                };
                let provider = NearChainProvider::new(
                    network,
                    Arc::clone(&primary) as Arc<dyn NearRpc>,
                    Arc::new(signer),
                )
                .with_backup_rpc(Arc::clone(&backup) as Arc<dyn NearRpc>);
                let facilitator = build_facilitator(provider.clone());
                (Some(facilitator), ChainProvider::Near(provider))
            }
            ChainKind::Eip155 => {
                // The eip155 block and 0x asset are validated at load; the
                // secp256k1 key is a mode-0600 credential parsed inside the
                // provider and never logged.
                let eip155 = config
                    .eip155
                    .as_ref()
                    .context("eip155 configuration block missing after validation")?;
                let rpc_urls = [config.primary_rpc_url.clone(), config.backup_rpc_url.clone()];
                let provider = EvmChainProvider::connect_from_config(
                    eip155.chain_id,
                    &rpc_urls,
                    secrets.relayer_key.as_str(),
                    &config.asset,
                    eip155.required_confirmations,
                    eip155.gas_limit,
                )
                .await
                .context("connect EVM settlement provider")?;
                (None, ChainProvider::Evm(Box::new(provider)))
            }
        };

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
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            monitor_state.refresh_chain_readiness().await;
        }
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

fn build_facilitator(provider: NearChainProvider) -> FacilitatorLocal<SchemeRegistry> {
    let chain_id = provider.chain_id();
    let mut providers = HashMap::new();
    providers.insert(chain_id.clone(), provider);
    let chains = ChainRegistry::new(providers);
    let blueprints = SchemeBlueprints::new().and_register(V2NearExact);
    let schemes = vec![SchemeConfig {
        enabled: true,
        id: "v2-near-exact".to_owned(),
        chains: ChainIdPattern::exact(chain_id.namespace, chain_id.reference),
        config: None,
    }];
    FacilitatorLocal::new(SchemeRegistry::build(chains, blueprints, &schemes))
}
