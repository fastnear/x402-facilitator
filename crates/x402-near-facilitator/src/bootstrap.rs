//! Shared startup construction for the service and admin binaries.
//!
//! The service (`main.rs`) and the admin `reconcile` command build the same
//! settlement provider from the same validated config and relayer key, in the
//! same chain-branched way. Keeping that one ordered sequence here means the
//! two binaries cannot drift: a change to how a chain is wired — a new
//! readiness guard, a different signer parse — lands in exactly one place.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use near_crypto::{InMemorySigner, SecretKey};
use near_primitives::types::AccountId;
use x402_chain_eip155_provider::provider::EvmChainProvider;
use x402_chain_near::{JsonRpcNearRpc, NearChainProvider, NearNetwork, NearRpc, V2NearExact};
use x402_facilitator_local::FacilitatorLocal;
use x402_types::chain::{ChainIdPattern, ChainProviderOps, ChainRegistry};
use x402_types::scheme::{SchemeBlueprints, SchemeConfig, SchemeRegistry};

use crate::chain::ChainProvider;
use crate::config::{ChainKind, Environment, ServiceConfig};

/// Build the settlement provider and, for NEAR only, the registered
/// `FacilitatorLocal`, from a validated config and the relayer service key.
///
/// The chain family has already been accepted by [`ServiceConfig`] validation,
/// so this only performs construction. NEAR uses an ed25519 delegate signer
/// plus the registered `V2NearExact` facilitator; EVM uses a secp256k1 signer
/// and serves `/verify` directly through the neutral provider — the upstream
/// `V2Eip155Exact` blueprint is generic over `&P` and cannot be registered
/// through this assembly, so eip155 returns no facilitator.
///
/// Returns `(Some(facilitator), ChainProvider::Near(..))` for NEAR and
/// `(None, ChainProvider::Evm(..))` for eip155.
pub async fn build_chain_provider(
    config: &ServiceConfig,
    relayer_key: &str,
) -> Result<(Option<FacilitatorLocal<SchemeRegistry>>, ChainProvider)> {
    match config.chain_kind {
        ChainKind::Near => {
            let relayer_account =
                AccountId::from_str(&config.relayer_account_id).context("parse relayer account")?;
            let secret_key =
                SecretKey::from_str(relayer_key).context("parse relayer service key")?;
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
            Ok((Some(facilitator), ChainProvider::Near(provider)))
        }
        ChainKind::Eip155 => {
            // The eip155 block and 0x asset are validated at load; the secp256k1
            // key is a mode-0600 credential parsed inside the provider and never
            // logged.
            let eip155 = config
                .eip155
                .as_ref()
                .context("eip155 configuration block missing after validation")?;
            let rpc_urls = [
                config.primary_rpc_url.clone(),
                config.backup_rpc_url.clone(),
            ];
            let provider = EvmChainProvider::connect_from_config(
                eip155.chain_id,
                &rpc_urls,
                relayer_key,
                &config.asset,
                eip155.required_confirmations,
                eip155.gas_limit,
            )
            .await
            .context("connect EVM settlement provider")?;
            // The signer address is derived from the key file, but the
            // relayer-policy lookups key on config.relayer_account_id. A mismatch
            // would fail readiness silently, so fail fast instead.
            let signer_address = provider.signer_address().to_string();
            ensure!(
                config
                    .relayer_account_id
                    .eq_ignore_ascii_case(&signer_address),
                "config relayer_account_id {} does not match the EVM signer address {} \
                 derived from the key file",
                config.relayer_account_id,
                signer_address
            );
            Ok((None, ChainProvider::Evm(Box::new(provider))))
        }
    }
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
