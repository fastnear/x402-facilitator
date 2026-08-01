//! Offline and loopback-only HTTP protocol conformance tests.

use std::error::Error;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::process::Command;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::{ACCESS_CONTROL_ALLOW_ORIGIN, CONTENT_TYPE, RETRY_AFTER};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use near_crypto::{InMemorySigner, KeyType, SecretKey, Signer};
use near_primitives::action::delegate::{DelegateAction, NonDelegateAction, SignedDelegateAction};
use near_primitives::action::{Action, FunctionCallAction};
use near_primitives::borsh;
use near_primitives::hash::CryptoHash;
use near_primitives::transaction::{SignedTransaction, Transaction};
use near_primitives::types::{AccountId, Balance, Gas};
use near_primitives::views::{
    AccessKeyPermissionView, AccessKeyView, AccountView, ExecutionMetadataView,
    ExecutionOutcomeView, ExecutionOutcomeWithIdView, ExecutionStatusView,
    FinalExecutionOutcomeView, FinalExecutionStatus,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tokio::sync::Barrier;
use tower::ServiceExt as _;
use url::Url;
use uuid::Uuid;
use x402_chain_near::{
    FinalBlock, NearChainProvider, NearNetwork, NearRpc, NearRpcError, TransactionLookup,
    V2NearExact,
};
use x402_facilitator_local::FacilitatorLocal;
use x402_types::chain::{ChainIdPattern, ChainProviderOps, ChainRegistry};
use x402_types::scheme::{SchemeBlueprints, SchemeConfig, SchemeRegistry};

use super::{AppState, OPENAPI_YAML, router};
use crate::VERSION;
use crate::auth::{ApiKeyAuthenticator, digest_api_key};
use crate::catalog::Catalog;
use crate::chain::ChainProvider;
use crate::config::{
    ChainKind, Eip155Config, Environment, PaymentIdentifierConfig, RequestLimits, ServiceConfig,
    SponsorshipConfig,
};
use crate::leadership::ReadinessState;
use crate::store::{ApiClient, PgStore, SettlementState};
use crate::telemetry::Metrics;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const TESTNET_USDC: &str = "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af";
const TEST_PAYEE: &str = "merchant.mike.testnet";
const TEST_PAYER: &str = "x402-http-payer.testnet";
const TEST_RELAYER: &str = "x402-http-relayer.testnet";
const BASE_SEPOLIA_USDC: &str = "0x036cbd53842c5426634e7929541ec2318f3dcf7e";
const BASE_PROTOCOL_SIGNER: &str = "0x51f2dbe5c2e1f3f0d9a5b6c7e8f9a0b1c2d3e4f5";
const TEST_PEPPER: [u8; 32] = [0x42; 32];

#[derive(Debug)]
struct MockRpc {
    block: FinalBlock,
    broadcast_journal: StdMutex<Option<PgPool>>,
    sends: AtomicUsize,
    payer_nonce: AtomicU64,
    relayer_nonce: AtomicU64,
    relayer_account_failures: AtomicUsize,
    advance_payer_nonce: AtomicBool,
}

impl MockRpc {
    fn new() -> Self {
        Self {
            block: FinalBlock {
                height: 1_000,
                hash: CryptoHash::hash_bytes(b"http-conformance-final-block"),
            },
            broadcast_journal: StdMutex::new(None),
            sends: AtomicUsize::new(0),
            payer_nonce: AtomicU64::new(0),
            relayer_nonce: AtomicU64::new(0),
            relayer_account_failures: AtomicUsize::new(0),
            advance_payer_nonce: AtomicBool::new(true),
        }
    }

    fn fail_next_relayer_account_lookup(&self) {
        self.relayer_account_failures.store(1, Ordering::SeqCst);
    }

    fn keep_payer_nonce_stable(&self) {
        self.advance_payer_nonce.store(false, Ordering::SeqCst);
    }

    fn require_submitted_before_broadcast(&self, pool: PgPool) {
        *self
            .broadcast_journal
            .lock()
            .unwrap_or_else(|_| std::process::abort()) = Some(pool);
    }

    async fn assert_submitted_before_broadcast(
        &self,
        signed_transaction: &SignedTransaction,
    ) -> Result<(), NearRpcError> {
        let pool = self
            .broadcast_journal
            .lock()
            .unwrap_or_else(|_| std::process::abort())
            .clone();
        let Some(pool) = pool else {
            return Ok(());
        };
        let transaction_hash = signed_transaction.get_hash().to_string();
        let transaction_bytes = borsh::to_vec(signed_transaction)
            .map_err(|_| NearRpcError::InvalidSignedTransaction)?;
        let row = sqlx::query_as::<_, (String, Option<Vec<u8>>)>(
            "SELECT state, outer_transaction_bytes \
             FROM settlements WHERE outer_transaction_hash = $1",
        )
        .bind(transaction_hash)
        .fetch_optional(&pool)
        .await
        .map_err(|_| {
            NearRpcError::InvalidResponse("HTTP fixture could not inspect the settlement journal")
        })?;
        match row {
            Some((state, Some(stored_bytes)))
                if state == "submitted" && stored_bytes == transaction_bytes =>
            {
                Ok(())
            }
            _ => Err(NearRpcError::InvalidResponse(
                "outer transaction was not durably submitted before broadcast",
            )),
        }
    }

    fn account() -> AccountView {
        AccountView {
            amount: Balance::from_yoctonear(10_u128.pow(24)),
            locked: Balance::ZERO,
            code_hash: CryptoHash::hash_bytes(b"deployed-contract"),
            storage_usage: 0,
            storage_paid_at: 0,
            global_contract_hash: None,
            global_contract_account_id: None,
        }
    }

    fn ensure_pinned(&self, block_hash: CryptoHash) -> Result<(), NearRpcError> {
        if block_hash == self.block.hash {
            Ok(())
        } else {
            Err(NearRpcError::InvalidResponse(
                "HTTP conformance query was not pinned",
            ))
        }
    }
}

#[async_trait]
impl NearRpc for MockRpc {
    async fn network_id(&self) -> Result<String, NearRpcError> {
        Ok("testnet".to_owned())
    }

    async fn final_block(&self) -> Result<FinalBlock, NearRpcError> {
        Ok(self.block)
    }

    async fn view_account(
        &self,
        block_hash: CryptoHash,
        account_id: AccountId,
    ) -> Result<AccountView, NearRpcError> {
        self.ensure_pinned(block_hash)?;
        if account_id.as_str() == TEST_RELAYER
            && self
                .relayer_account_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
        {
            return Err(NearRpcError::Timeout);
        }
        Ok(Self::account())
    }

    async fn view_access_key(
        &self,
        block_hash: CryptoHash,
        account_id: AccountId,
        _public_key: near_crypto::PublicKey,
    ) -> Result<AccessKeyView, NearRpcError> {
        self.ensure_pinned(block_hash)?;
        Ok(AccessKeyView {
            nonce: if account_id.as_str() == TEST_RELAYER {
                self.relayer_nonce.load(Ordering::SeqCst)
            } else {
                self.payer_nonce.load(Ordering::SeqCst)
            },
            permission: AccessKeyPermissionView::FullAccess,
        })
    }

    async fn call_function(
        &self,
        block_hash: CryptoHash,
        _contract_id: AccountId,
        method_name: String,
        _args: Vec<u8>,
    ) -> Result<Vec<u8>, NearRpcError> {
        self.ensure_pinned(block_hash)?;
        match method_name.as_str() {
            "ft_balance_of" => Ok(br#""1000000000""#.to_vec()),
            "storage_balance_of" => Ok(b"{}".to_vec()),
            _ => Err(NearRpcError::MethodNotFound),
        }
    }

    async fn send_transaction_final(
        &self,
        signed_transaction: SignedTransaction,
    ) -> Result<TransactionLookup, NearRpcError> {
        self.assert_submitted_before_broadcast(&signed_transaction)
            .await?;
        self.sends.fetch_add(1, Ordering::SeqCst);
        let transaction = &signed_transaction.transaction;
        self.relayer_nonce
            .store(transaction.nonce().nonce(), Ordering::SeqCst);
        let Transaction::V0(transaction) = transaction else {
            return Err(NearRpcError::InvalidResponse(
                "HTTP fixture expected transaction V0",
            ));
        };
        let Some(Action::Delegate(delegate)) = transaction.actions.first() else {
            return Err(NearRpcError::InvalidResponse(
                "HTTP fixture expected delegate action",
            ));
        };
        if self.advance_payer_nonce.load(Ordering::SeqCst) {
            self.payer_nonce
                .store(delegate.delegate_action.nonce, Ordering::SeqCst);
        }
        Ok(TransactionLookup::Final(Box::new(successful_outcome(
            signed_transaction,
        )?)))
    }

    async fn transaction_status_final(
        &self,
        _transaction_hash: CryptoHash,
        _signer_id: AccountId,
    ) -> Result<TransactionLookup, NearRpcError> {
        Ok(TransactionLookup::Unknown)
    }
}

fn outcome(
    id: CryptoHash,
    executor_id: AccountId,
    receipt_ids: Vec<CryptoHash>,
    status: ExecutionStatusView,
) -> ExecutionOutcomeWithIdView {
    ExecutionOutcomeWithIdView {
        proof: Vec::new(),
        block_hash: CryptoHash::hash_bytes(b"http-outcome-block"),
        id,
        outcome: ExecutionOutcomeView {
            logs: Vec::new(),
            receipt_ids,
            gas_burnt: Gas::from_gas(0),
            tokens_burnt: Balance::ZERO,
            executor_id,
            status,
            metadata: ExecutionMetadataView::default(),
        },
    }
}

fn successful_outcome(
    signed_transaction: SignedTransaction,
) -> Result<FinalExecutionOutcomeView, NearRpcError> {
    let relayer = TEST_RELAYER
        .parse()
        .map_err(|_| NearRpcError::InvalidResponse("invalid test relayer"))?;
    let payer = TEST_PAYER
        .parse()
        .map_err(|_| NearRpcError::InvalidResponse("invalid test payer"))?;
    let asset = TESTNET_USDC
        .parse()
        .map_err(|_| NearRpcError::InvalidResponse("invalid test asset"))?;
    let transaction_hash = signed_transaction.get_hash();
    let delegate_id = CryptoHash::hash_bytes(b"http-delegate-receipt");
    let token_id = CryptoHash::hash_bytes(b"http-token-receipt");
    Ok(FinalExecutionOutcomeView {
        status: FinalExecutionStatus::SuccessValue(Vec::new()),
        transaction: signed_transaction.into(),
        transaction_outcome: outcome(
            transaction_hash,
            relayer,
            vec![delegate_id],
            ExecutionStatusView::SuccessReceiptId(delegate_id),
        ),
        receipts_outcome: vec![
            outcome(
                delegate_id,
                payer,
                vec![token_id],
                ExecutionStatusView::SuccessReceiptId(token_id),
            ),
            outcome(
                token_id,
                asset,
                Vec::new(),
                ExecutionStatusView::SuccessValue(Vec::new()),
            ),
        ],
    })
}

struct TestApplication {
    router: Router,
    readiness: ReadinessState,
    rpc: Arc<MockRpc>,
    relayer_public_key: String,
}

fn test_signer(account_id: &str) -> TestResult<Signer> {
    let account_id = account_id.parse::<AccountId>()?;
    let secret_key = SecretKey::from_random(KeyType::ED25519);
    Ok(InMemorySigner::from_secret_key(account_id, secret_key))
}

fn service_config() -> TestResult<ServiceConfig> {
    Ok(ServiceConfig {
        environment: Environment::Testnet,
        chain_kind: ChainKind::Near,
        network: "near:testnet".to_owned(),
        bind_address: "127.0.0.1:0".parse()?,
        primary_rpc_url: Url::parse("https://primary.test.invalid")?,
        backup_rpc_url: Url::parse("https://backup.test.invalid")?,
        asset: TESTNET_USDC.to_owned(),
        asset_symbol: "USDC".to_owned(),
        minimum_amount: "1000".to_owned(),
        relayer_account_id: TEST_RELAYER.to_owned(),
        max_inner_gas: 30_000_000_000_000,
        database_max_connections: 16,
        request_limits: RequestLimits {
            body_bytes: 65_536,
            verify_per_minute: 100,
            settle_per_minute: 500,
            verify_timeout_seconds: 15,
            settle_timeout_seconds: 30,
            max_concurrent_verify: 64,
        },
        sponsorship: SponsorshipConfig {
            global_daily_yocto_near: "1000000".to_owned(),
            default_client_daily_yocto_near: "100000".to_owned(),
            reservation_yocto_near: "100".to_owned(),
            balance_warning_yocto_near: "200".to_owned(),
            balance_hard_stop_yocto_near: "100".to_owned(),
        },
        payment_identifier: PaymentIdentifierConfig::default(),
        accept_v1: false,
        eip155: None,
    })
}

fn base_protocol_config() -> TestResult<ServiceConfig> {
    let mut config = service_config()?;
    config.chain_kind = ChainKind::Eip155;
    config.network = "eip155:84532".to_owned();
    config.asset = BASE_SEPOLIA_USDC.to_owned();
    config.relayer_account_id = BASE_PROTOCOL_SIGNER.to_owned();
    config.max_inner_gas = 0;
    config.sponsorship = SponsorshipConfig {
        global_daily_yocto_near: "500000000000000000".to_owned(),
        default_client_daily_yocto_near: "100000000000000000".to_owned(),
        reservation_yocto_near: "10000000000000000".to_owned(),
        balance_warning_yocto_near: "1000000000000000000".to_owned(),
        balance_hard_stop_yocto_near: "250000000000000000".to_owned(),
    };
    config.eip155 = Some(Eip155Config {
        chain_id: 84_532,
        required_confirmations: 2,
        gas_limit: 120_000,
        max_fee_per_gas_wei: "1000000000".to_owned(),
    });
    Ok(config)
}

fn build_facilitator(provider: NearChainProvider) -> FacilitatorLocal<SchemeRegistry> {
    let chain_id = provider.chain_id();
    let mut providers = std::collections::HashMap::new();
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

fn build_application(store: PgStore, metrics: Metrics) -> TestResult<TestApplication> {
    build_application_with_catalog(store, metrics, Catalog::empty())
}

fn build_application_with_catalog(
    store: PgStore,
    metrics: Metrics,
    catalog: Catalog,
) -> TestResult<TestApplication> {
    let config = service_config()?;
    let rpc = Arc::new(MockRpc::new());
    let primary: Arc<dyn NearRpc> = rpc.clone();
    let backup: Arc<dyn NearRpc> = rpc.clone();
    let relayer_signer = test_signer(TEST_RELAYER)?;
    let relayer_public_key = relayer_signer.public_key().to_string();
    let provider = NearChainProvider::new(NearNetwork::Testnet, primary, Arc::new(relayer_signer))
        .with_backup_rpc(backup);
    let facilitator = build_facilitator(provider.clone());
    let auth = ApiKeyAuthenticator::new(store.clone(), Environment::Testnet, TEST_PEPPER)?;
    let readiness = ReadinessState::default();
    let state = AppState::new(
        config,
        store,
        auth,
        Some(facilitator),
        ChainProvider::Near(provider),
        readiness.clone(),
        catalog,
        metrics,
    );
    Ok(TestApplication {
        router: router(state),
        readiness,
        rpc,
        relayer_public_key,
    })
}

// `/supported` and static protocol rejection need only the neutral provider
// identity. A NEAR-backed provider with an address-shaped account keeps this
// HTTP fixture offline while exercising the real EVM router branch
// (`facilitator: None`) and canonical Base configuration.
fn build_base_protocol_application(
    store: PgStore,
    metrics: Metrics,
) -> TestResult<TestApplication> {
    let config = base_protocol_config()?;
    let rpc = Arc::new(MockRpc::new());
    let primary: Arc<dyn NearRpc> = rpc.clone();
    let backup: Arc<dyn NearRpc> = rpc.clone();
    let relayer_signer = test_signer(BASE_PROTOCOL_SIGNER)?;
    let relayer_public_key = relayer_signer.public_key().to_string();
    let provider = NearChainProvider::new(NearNetwork::Testnet, primary, Arc::new(relayer_signer))
        .with_backup_rpc(backup);
    let auth = ApiKeyAuthenticator::new(store.clone(), Environment::Testnet, TEST_PEPPER)?;
    let readiness = ReadinessState::default();
    let state = AppState::new(
        config,
        store,
        auth,
        None,
        ChainProvider::Near(provider),
        readiness.clone(),
        Catalog::empty(),
        metrics,
    );
    Ok(TestApplication {
        router: router(state),
        readiness,
        rpc,
        relayer_public_key,
    })
}

fn valid_request(signer: &Signer, nonce: u64, identifier: Option<&str>) -> TestResult<Value> {
    let transfer = Action::FunctionCall(Box::new(FunctionCallAction {
        method_name: "ft_transfer".to_owned(),
        args: serde_json::to_vec(&json!({
            "receiver_id": TEST_PAYEE,
            "amount": "1000",
        }))?,
        gas: Gas::from_gas(30_000_000_000_000),
        deposit: Balance::from_yoctonear(1),
    }));
    let action = NonDelegateAction::try_from(transfer)?;
    let delegate = DelegateAction {
        sender_id: TEST_PAYER.parse()?,
        receiver_id: TESTNET_USDC.parse()?,
        actions: vec![action],
        nonce,
        max_block_height: 1_050,
        public_key: signer.public_key(),
    };
    let encoded = STANDARD.encode(borsh::to_vec(&SignedDelegateAction::sign(
        signer, delegate,
    ))?);
    let requirements = json!({
        "scheme": "exact",
        "network": "near:testnet",
        "amount": "1000",
        "payTo": TEST_PAYEE,
        "maxTimeoutSeconds": 60,
        "asset": TESTNET_USDC,
    });
    let mut payment_payload = json!({
        "x402Version": 2,
        "accepted": requirements.clone(),
        "payload": {
            "signedDelegateAction": encoded,
        },
    });
    if let Some(identifier) = identifier {
        payment_payload["extensions"] = json!({
            "payment-identifier": {
                "info": {
                    "required": true,
                    "id": identifier,
                },
                "schema": {},
            },
        });
    }
    Ok(json!({
        "x402Version": 2,
        "paymentPayload": payment_payload,
        "paymentRequirements": requirements,
    }))
}

fn invalid_version_request(signer: &Signer) -> TestResult<Value> {
    invalid_version_request_with_nonce(signer, 1)
}

fn invalid_version_request_with_nonce(signer: &Signer, nonce: u64) -> TestResult<Value> {
    let mut request = valid_request(signer, nonce, None)?;
    request["x402Version"] = json!(1);
    request["paymentPayload"]["x402Version"] = json!(1);
    Ok(request)
}

fn base_invalid_version_request() -> Value {
    let requirements = json!({
        "scheme": "exact",
        "network": "eip155:84532",
        "asset": BASE_SEPOLIA_USDC,
        "amount": "1000",
        "payTo": "0xa2acb5d3ac1c35999532624188470ec6228039dc",
        "maxTimeoutSeconds": 60,
        "extra": {"name": "USDC", "version": "2"},
    });
    json!({
        "x402Version": 1,
        "paymentPayload": {
            "x402Version": 1,
            "accepted": requirements.clone(),
            "payload": {
                "authorization": {
                    "from": "0x11efa374c489d106f9b6ac1b9b73a7a54c237c6d",
                    "to": "0xa2acb5d3ac1c35999532624188470ec6228039dc",
                    "value": "1000",
                    "validAfter": "0",
                    "validBefore": "9999999999",
                    "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001",
                },
                "signature": "0xdeadbeef",
            },
        },
        "paymentRequirements": requirements,
    })
}

fn api_key(seed: u8) -> (String, String) {
    let prefix = format!("x402_test_{}", hex::encode([seed; 12]));
    let raw = format!("{prefix}.{}", hex::encode([seed.wrapping_add(1); 32]));
    (prefix, raw)
}

async fn seed_client(
    store: &PgStore,
    seed: u8,
    verify_rate: u32,
    settle_rate: u32,
) -> TestResult<(ApiClient, String)> {
    let client = ApiClient {
        id: Uuid::new_v4(),
        name: format!("http-conformance-{seed}"),
        environment: "testnet".to_owned(),
        daily_budget_yocto_near: "100000".to_owned(),
        verify_rate_per_minute: verify_rate,
        settle_rate_per_minute: settle_rate,
    };
    let (prefix, raw) = api_key(seed);
    let digest = digest_api_key(&TEST_PEPPER, raw.as_bytes())?;
    store
        .create_client(&client, Uuid::new_v4(), &prefix, &digest)
        .await?;
    store
        .allow_payee(client.id, "near:testnet", TESTNET_USDC, TEST_PAYEE)
        .await?;
    Ok((client, raw))
}

fn http_request(
    method: Method,
    path: &str,
    body: Vec<u8>,
    content_type: Option<&str>,
    api_key: Option<&str>,
    bearer: Option<&str>,
) -> TestResult<Request<Body>> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(content_type) = content_type {
        builder = builder.header(CONTENT_TYPE, content_type);
    }
    if let Some(api_key) = api_key {
        builder = builder.header("x-api-key", api_key);
    }
    if let Some(bearer) = bearer {
        builder = builder.header("authorization", format!("Bearer {bearer}"));
    }
    Ok(builder.body(Body::from(body))?)
}

struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    bytes: Vec<u8>,
}

impl TestResponse {
    fn json(&self) -> TestResult<Value> {
        Ok(serde_json::from_slice(&self.bytes)?)
    }
}

async fn call(router: &Router, request: Request<Body>) -> TestResult<TestResponse> {
    let response = router.clone().oneshot(request).await?;
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1_048_576).await?.to_vec();
    Ok(TestResponse {
        status,
        headers,
        bytes,
    })
}

fn ready(readiness: &ReadinessState) {
    readiness.set_leadership(true);
    readiness.set_reconciliation(true);
    readiness.set_rpc(true);
    readiness.set_relayer(true);
}

struct TestDatabase {
    store: PgStore,
    pool: PgPool,
    admin: PgPool,
    schema: String,
}

impl TestDatabase {
    async fn from_explicit_environment() -> TestResult<Option<Self>> {
        let Ok(raw) = std::env::var("X402_FACILITATOR_TEST_DATABASE_URL") else {
            eprintln!(
                "skipping protected HTTP checks: X402_FACILITATOR_TEST_DATABASE_URL is unset"
            );
            return Ok(None);
        };
        let url = Url::parse(&raw)?;
        if !matches!(url.scheme(), "postgres" | "postgresql")
            || !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
        {
            return Err(std::io::Error::other(
                "X402_FACILITATOR_TEST_DATABASE_URL must be a loopback PostgreSQL URL",
            )
            .into());
        }
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&raw)
            .await?;
        let schema = format!("x402_http_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await?;
        let options = PgConnectOptions::from_str(&raw)?.options([("search_path", schema.as_str())]);
        let pool = PgPoolOptions::new()
            .max_connections(24)
            .connect_with(options)
            .await?;
        let store = PgStore::from_explicit_test_pool(pool.clone());
        store.migrate().await?;
        Ok(Some(Self {
            store,
            pool,
            admin,
            schema,
        }))
    }

    async fn cleanup(self) -> TestResult {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await?;
        self.admin.close().await;
        Ok(())
    }
}

fn disconnected_store() -> PgStore {
    let options = PgConnectOptions::new()
        .host("127.0.0.1")
        .port(1)
        .username("x402_explicit_disconnected_test")
        .database("x402_explicit_disconnected_test");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_millis(50))
        .connect_lazy_with(options);
    PgStore::from_explicit_test_pool(pool)
}

async fn assert_public_discovery_contract(application: &TestApplication) -> TestResult {
    let landing = call(
        &application.router,
        http_request(Method::GET, "/", Vec::new(), None, None, None)?,
    )
    .await?;
    assert_eq!(landing.status, StatusCode::OK);
    let landing = String::from_utf8(landing.bytes)?;
    assert!(landing.contains("href=\"/llms.txt\""));
    assert!(landing.contains("href=\"/openapi.yaml\""));
    assert!(landing.contains("href=\"/discovery/resources\""));

    let openapi = call(
        &application.router,
        http_request(Method::GET, "/openapi.yaml", Vec::new(), None, None, None)?,
    )
    .await?;
    assert_eq!(openapi.status, StatusCode::OK);
    assert_eq!(openapi.bytes, OPENAPI_YAML.as_bytes());
    assert_eq!(
        openapi
            .headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/yaml; charset=utf-8")
    );

    let llms = call(
        &application.router,
        http_request(Method::GET, "/llms.txt", Vec::new(), None, None, None)?,
    )
    .await?;
    assert_eq!(llms.status, StatusCode::OK);
    assert_eq!(
        llms.headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()),
        Some("text/plain; charset=utf-8")
    );
    let llms = String::from_utf8(llms.bytes)?;
    assert!(llms.contains("Network: near:testnet"));
    assert!(llms.contains(TESTNET_USDC));
    assert!(llms.contains("Facilitator fee: 0"));
    assert!(llms.contains("EIP-712 domain name \"USD Coin\", version \"2\""));
    assert!(!llms.to_ascii_lowercase().contains("api key:"));

    let discovery = call(
        &application.router,
        http_request(
            Method::GET,
            "/discovery/resources",
            Vec::new(),
            None,
            None,
            None,
        )?,
    )
    .await?;
    assert_eq!(discovery.status, StatusCode::OK);
    assert_eq!(
        discovery.json()?,
        json!({
            "x402Version": 2,
            "items": [],
            "pagination": {"limit": 100, "offset": 0, "total": 0}
        })
    );
    Ok(())
}

async fn assert_public_contract(application: &TestApplication, payer: &Signer) -> TestResult {
    assert_public_discovery_contract(application).await?;

    let health = call(
        &application.router,
        http_request(Method::GET, "/healthz", Vec::new(), None, None, None)?,
    )
    .await?;
    assert_eq!(health.status, StatusCode::OK);
    assert_eq!(
        health.json()?,
        json!({
            "status": "ok",
            "service": "x402-near-facilitator",
            "version": VERSION,
        })
    );

    let readiness = call(
        &application.router,
        http_request(Method::GET, "/readyz", Vec::new(), None, None, None)?,
    )
    .await?;
    assert_eq!(readiness.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        readiness
            .headers
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok()),
        Some("1")
    );
    let readiness_json = readiness.json()?;
    assert_eq!(readiness_json["ready"], false);
    assert_eq!(readiness_json["checks"]["database"], "not_ready");
    assert_eq!(
        readiness_json["checks"]
            .as_object()
            .map(serde_json::Map::len),
        Some(5)
    );
    assert_eq!(
        readiness_json.as_object().map(serde_json::Map::len),
        Some(2)
    );

    let supported = call(
        &application.router,
        http_request(Method::GET, "/supported", Vec::new(), None, None, None)?,
    )
    .await?;
    assert_eq!(supported.status, StatusCode::OK);
    assert!(supported.headers.get(ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
    let supported_json = supported.json()?;
    assert_eq!(supported_json["kinds"][0]["x402Version"], 2);
    assert_eq!(supported_json["kinds"][0]["scheme"], "exact");
    assert_eq!(supported_json["kinds"][0]["network"], "near:testnet");
    assert_eq!(supported_json["extensions"], json!(["payment-identifier"]));
    assert_eq!(supported_json["signers"]["near:testnet"][0], TEST_RELAYER);

    let request = serde_json::to_vec(&invalid_version_request(payer)?)?;
    let unauthorized = call(
        &application.router,
        http_request(
            Method::POST,
            "/verify",
            request.clone(),
            Some("application/json"),
            None,
            None,
        )?,
    )
    .await?;
    assert_eq!(unauthorized.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unauthorized.json()?["error"]["code"], "invalid_api_key");

    let (_, first) = api_key(90);
    let (_, second) = api_key(91);
    let conflicting = call(
        &application.router,
        http_request(
            Method::POST,
            "/verify",
            request,
            Some("application/json"),
            Some(&first),
            Some(&second),
        )?,
    )
    .await?;
    assert_eq!(conflicting.status, StatusCode::UNAUTHORIZED);

    let options = call(
        &application.router,
        http_request(Method::OPTIONS, "/verify", Vec::new(), None, None, None)?,
    )
    .await?;
    assert_eq!(options.status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(options.headers.get(ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
    Ok(())
}

fn populated_discovery_manifest() -> Value {
    json!({
        "schemaVersion": 1,
        "resources": [
            {
                "resource": "https://merchant-two.example/work",
                "type": "http",
                "x402Version": 2,
                "accepts": [{
                    "scheme": "exact",
                    "network": "near:testnet",
                    "asset": TESTNET_USDC,
                    "amount": "2000",
                    "payTo": "second-merchant.testnet",
                    "maxTimeoutSeconds": 300
                }],
                "lastUpdated": "2026-07-31T13:00:00Z",
                "description": "Second independent merchant",
                "extensions": {"bazaar": {
                    "info": {"input": {"type": "http", "method": "POST"}},
                    "schema": {"type": "object"}
                }},
                "admission": {
                    "reviewedAt": "2026-07-31T13:00:00Z",
                    "optInEvidenceUrl": "https://github.com/example/two/issues/1",
                    "payToControlEvidenceUrl": "https://merchant-two.example/payments"
                }
            },
            {
                "resource": "https://merchant-one.example/work",
                "type": "http",
                "x402Version": 2,
                "accepts": [{
                    "scheme": "exact",
                    "network": "near:testnet",
                    "asset": TESTNET_USDC,
                    "amount": "1000",
                    "payTo": "first-merchant.testnet",
                    "maxTimeoutSeconds": 300
                }],
                "lastUpdated": "2026-07-31T12:00:00Z",
                "description": "First independent merchant",
                "extensions": {"bazaar": {
                    "info": {"input": {"type": "http", "method": "POST"}},
                    "schema": {"type": "object"}
                }},
                "admission": {
                    "reviewedAt": "2026-07-31T12:00:00Z",
                    "optInEvidenceUrl": "https://github.com/example/one/issues/1",
                    "payToControlEvidenceUrl": "https://merchant-one.example/payments"
                }
            }
        ]
    })
}

#[tokio::test]
async fn discovery_catalog_filters_paginates_and_rejects_bad_queries() -> TestResult {
    let manifest = populated_discovery_manifest();
    let catalog = Catalog::from_json_for(
        &serde_json::to_string(&manifest)?,
        "near:testnet",
        TESTNET_USDC,
    )?;
    let application =
        build_application_with_catalog(disconnected_store(), Metrics::for_tests(), catalog)?;

    let first_page = call(
        &application.router,
        http_request(
            Method::GET,
            "/discovery/resources?type=http&network=near%3Atestnet&scheme=exact&extensions=bazaar&limit=1&offset=0",
            Vec::new(),
            None,
            None,
            None,
        )?,
    )
    .await?;
    assert_eq!(first_page.status, StatusCode::OK);
    let first_page = first_page.json()?;
    assert_eq!(first_page["pagination"]["total"], 2);
    assert_eq!(
        first_page["items"][0]["resource"],
        "https://merchant-two.example/work"
    );
    assert!(first_page["items"][0].get("admission").is_none());

    let payee = call(
        &application.router,
        http_request(
            Method::GET,
            "/discovery/resources?payTo=first-merchant.testnet",
            Vec::new(),
            None,
            None,
            None,
        )?,
    )
    .await?;
    assert_eq!(payee.json()?["pagination"]["total"], 1);

    for path in [
        "/discovery/resources?limit=0",
        "/discovery/resources?limit=01",
        "/discovery/resources?network=a&network=b",
        "/discovery/resources?network=%ZZ",
        "/discovery/resources?network=%FF",
        "/discovery/resources?unknown=value",
    ] {
        let response = call(
            &application.router,
            http_request(Method::GET, path, Vec::new(), None, None, None)?,
        )
        .await?;
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert_eq!(response.json()?["error"]["code"], "invalid_discovery_query");
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn assert_protected_contract(
    database: &TestDatabase,
    metrics: Metrics,
    payer: &Signer,
) -> TestResult {
    let (_client, key) = seed_client(&database.store, 1, 100, 500).await?;
    let application = build_application(database.store.clone(), metrics)?;
    database
        .store
        .upsert_relayer(
            "near:testnet",
            TEST_RELAYER,
            &application.relayer_public_key,
        )
        .await?;

    let unavailable = call(
        &application.router,
        http_request(Method::GET, "/readyz", Vec::new(), None, None, None)?,
    )
    .await?;
    assert_eq!(unavailable.status, StatusCode::SERVICE_UNAVAILABLE);
    ready(&application.readiness);
    let available = call(
        &application.router,
        http_request(Method::GET, "/readyz", Vec::new(), None, None, None)?,
    )
    .await?;
    assert_eq!(available.status, StatusCode::OK);
    assert_eq!(available.json()?["ready"], true);

    let invalid_version = invalid_version_request(payer)?;
    let invalid_bytes = serde_json::to_vec(&invalid_version)?;
    for bearer_only in [false, true] {
        let response = call(
            &application.router,
            http_request(
                Method::POST,
                "/verify",
                invalid_bytes.clone(),
                Some("application/json; charset=utf-8"),
                (!bearer_only).then_some(key.as_str()),
                bearer_only.then_some(key.as_str()),
            )?,
        )
        .await?;
        assert_eq!(response.status, StatusCode::OK);
        let value = response.json()?;
        assert_eq!(value["isValid"], false);
        assert_eq!(value["invalidReason"], "invalid_x402_version");
        assert_eq!(value.as_object().map(serde_json::Map::len), Some(2));
    }

    let valid_verification = call(
        &application.router,
        http_request(
            Method::POST,
            "/verify",
            serde_json::to_vec(&valid_request(payer, 50, None)?)?,
            Some("application/json"),
            Some(&key),
            Some(&key),
        )?,
    )
    .await?;
    assert_eq!(valid_verification.status, StatusCode::OK);
    assert_eq!(
        valid_verification.json()?,
        json!({"isValid": true, "payer": TEST_PAYER})
    );

    let mut unsupported_method = valid_request(payer, 51, None)?;
    unsupported_method["paymentPayload"]["accepted"]["extra"] =
        json!({"assetTransferMethod": "intents-verifier"});
    unsupported_method["paymentRequirements"]["extra"] =
        json!({"assetTransferMethod": "intents-verifier"});
    unsupported_method["paymentPayload"]["payload"] = json!({
        "signedIntent": {
            "standard": "nep413",
            "payload": {"message": "{}"},
            "signature": "placeholder",
        },
    });
    let sends_before_unsupported = application.rpc.sends.load(Ordering::SeqCst);
    let unsupported_verify = call(
        &application.router,
        http_request(
            Method::POST,
            "/verify",
            serde_json::to_vec(&unsupported_method)?,
            Some("application/json"),
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(unsupported_verify.status, StatusCode::OK);
    assert_eq!(
        unsupported_verify.json()?,
        json!({
            "isValid": false,
            "invalidReason": "unsupported_asset_transfer_method",
        })
    );
    let unsupported_settle = call(
        &application.router,
        http_request(
            Method::POST,
            "/settle",
            serde_json::to_vec(&unsupported_method)?,
            Some("application/json"),
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(unsupported_settle.status, StatusCode::OK);
    assert_eq!(
        unsupported_settle.json()?,
        json!({
            "success": false,
            "errorReason": "unsupported_asset_transfer_method",
            "transaction": "",
            "network": "near:testnet",
        })
    );
    assert_eq!(
        application.rpc.sends.load(Ordering::SeqCst),
        sends_before_unsupported
    );

    for (nonce, accepted_only) in [(52, true), (53, false)] {
        let mut one_sided_unsupported_method = valid_request(payer, nonce, None)?;
        if accepted_only {
            one_sided_unsupported_method["paymentPayload"]["accepted"]["extra"] =
                json!({"assetTransferMethod": "intents-verifier"});
        } else {
            one_sided_unsupported_method["paymentRequirements"]["extra"] =
                json!({"assetTransferMethod": "intents-verifier"});
        }
        let one_sided_unsupported_verify = call(
            &application.router,
            http_request(
                Method::POST,
                "/verify",
                serde_json::to_vec(&one_sided_unsupported_method)?,
                Some("application/json"),
                Some(&key),
                None,
            )?,
        )
        .await?;
        assert_eq!(one_sided_unsupported_verify.status, StatusCode::OK);
        assert_eq!(
            one_sided_unsupported_verify.json()?,
            json!({
                "isValid": false,
                "invalidReason": "unsupported_asset_transfer_method",
            })
        );
        assert_eq!(
            application.rpc.sends.load(Ordering::SeqCst),
            sends_before_unsupported
        );
    }

    let mut malformed_method = valid_request(payer, 54, None)?;
    malformed_method["paymentPayload"]["accepted"]["extra"] = json!({"assetTransferMethod": 7});
    malformed_method["paymentRequirements"]["extra"] = json!({"assetTransferMethod": 7});
    let malformed_method = call(
        &application.router,
        http_request(
            Method::POST,
            "/verify",
            serde_json::to_vec(&malformed_method)?,
            Some("application/json"),
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(malformed_method.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        malformed_method.json()?["error"]["code"],
        "malformed_request"
    );

    let missing_media = call(
        &application.router,
        http_request(
            Method::POST,
            "/verify",
            invalid_bytes.clone(),
            None,
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(missing_media.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let wrong_media = call(
        &application.router,
        http_request(
            Method::POST,
            "/verify",
            invalid_bytes.clone(),
            Some("text/plain"),
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(wrong_media.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let malformed = call(
        &application.router,
        http_request(
            Method::POST,
            "/verify",
            b"{".to_vec(),
            Some("application/json"),
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);

    let mut wrong_amount_type = invalid_version.clone();
    wrong_amount_type["paymentRequirements"]["amount"] = json!(1000);
    wrong_amount_type["paymentPayload"]["accepted"]["amount"] = json!(1000);
    let wrong_shape = call(
        &application.router,
        http_request(
            Method::POST,
            "/verify",
            serde_json::to_vec(&wrong_amount_type)?,
            Some("application/json"),
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(wrong_shape.status, StatusCode::BAD_REQUEST);

    let too_large = call(
        &application.router,
        http_request(
            Method::POST,
            "/verify",
            vec![b' '; 65_537],
            Some("application/json"),
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(too_large.status, StatusCode::PAYLOAD_TOO_LARGE);

    let mut bad_identifier = invalid_version.clone();
    bad_identifier["paymentPayload"]["extensions"] = json!({
        "payment-identifier": {
            "info": {"required": true, "id": "too-short"},
        },
    });
    let bad_identifier = call(
        &application.router,
        http_request(
            Method::POST,
            "/verify",
            serde_json::to_vec(&bad_identifier)?,
            Some("application/json"),
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(bad_identifier.status, StatusCode::BAD_REQUEST);

    let settle_failure = call(
        &application.router,
        http_request(
            Method::POST,
            "/settle",
            invalid_bytes.clone(),
            Some("application/json"),
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(settle_failure.status, StatusCode::OK);
    let settle_failure_json = settle_failure.json()?;
    assert_eq!(settle_failure_json["success"], false);
    assert_eq!(settle_failure_json["errorReason"], "invalid_x402_version");
    assert_eq!(settle_failure_json["transaction"], "");
    assert_eq!(settle_failure_json["network"], "near:testnet");
    assert_eq!(
        settle_failure_json.as_object().map(serde_json::Map::len),
        Some(4)
    );

    let (_rate_client, rate_key) = seed_client(&database.store, 2, 2, 100).await?;
    for expected in [
        StatusCode::OK,
        StatusCode::OK,
        StatusCode::TOO_MANY_REQUESTS,
    ] {
        let response = call(
            &application.router,
            http_request(
                Method::POST,
                "/verify",
                invalid_bytes.clone(),
                Some("application/json"),
                Some(&rate_key),
                None,
            )?,
        )
        .await?;
        assert_eq!(response.status, expected);
        if expected == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(
                response
                    .headers
                    .get(RETRY_AFTER)
                    .and_then(|value| value.to_str().ok()),
                Some("1")
            );
        }
    }

    let (revoked_client, revoked_key) = seed_client(&database.store, 3, 100, 100).await?;
    let before_revoke = call(
        &application.router,
        http_request(
            Method::POST,
            "/verify",
            invalid_bytes.clone(),
            Some("application/json"),
            None,
            Some(&revoked_key),
        )?,
    )
    .await?;
    assert_eq!(before_revoke.status, StatusCode::OK);
    assert!(database.store.revoke_client(revoked_client.id).await?);
    let after_revoke = call(
        &application.router,
        http_request(
            Method::POST,
            "/verify",
            invalid_bytes.clone(),
            Some("application/json"),
            Some(&revoked_key),
            None,
        )?,
    )
    .await?;
    assert_eq!(after_revoke.status, StatusCode::UNAUTHORIZED);

    let identifier = "payment_http_0000000000000001";
    let settlement = valid_request(payer, 1, Some(identifier))?;
    let settlement_bytes = serde_json::to_vec(&settlement)?;
    application
        .rpc
        .require_submitted_before_broadcast(database.pool.clone());
    let concurrency = 200;
    let barrier = Arc::new(Barrier::new(concurrency + 1));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..concurrency {
        let router = application.router.clone();
        let body = settlement_bytes.clone();
        let key = key.clone();
        let barrier = Arc::clone(&barrier);
        tasks.spawn(async move {
            barrier.wait().await;
            let request = http_request(
                Method::POST,
                "/settle",
                body,
                Some("application/json"),
                Some(&key),
                None,
            )?;
            call(&router, request).await
        });
    }
    barrier.wait().await;
    let mut terminal_bytes: Option<Vec<u8>> = None;
    let mut completed = 0;
    while let Some(joined) = tasks.join_next().await {
        let response = joined??;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.json()?["success"], true);
        completed += 1;
        if let Some(expected) = &terminal_bytes {
            assert_eq!(&response.bytes, expected);
        } else {
            terminal_bytes = Some(response.bytes);
        }
    }
    assert_eq!(completed, concurrency);
    assert_eq!(application.rpc.sends.load(Ordering::SeqCst), 1);
    let settlement_id: Uuid =
        sqlx::query_scalar("SELECT id FROM settlements WHERE payment_identifier = $1")
            .bind(identifier)
            .fetch_one(&database.pool)
            .await?;
    let states: Vec<String> = sqlx::query_scalar(
        "SELECT to_state FROM settlement_events WHERE settlement_id = $1 ORDER BY id",
    )
    .bind(settlement_id)
    .fetch_all(&database.pool)
    .await?;
    assert_eq!(
        states,
        vec!["reserved", "prepared", "submitted", "succeeded"]
    );

    let replay = call(
        &application.router,
        http_request(
            Method::POST,
            "/settle",
            settlement_bytes,
            Some("application/json"),
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(replay.bytes, terminal_bytes.unwrap_or_default());
    assert_eq!(application.rpc.sends.load(Ordering::SeqCst), 1);

    let mut conflict = settlement.clone();
    conflict["paymentRequirements"]["extra"] = json!({"changed": true});
    let conflict = call(
        &application.router,
        http_request(
            Method::POST,
            "/settle",
            serde_json::to_vec(&conflict)?,
            Some("application/json"),
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(conflict.status, StatusCode::CONFLICT);
    assert_eq!(
        conflict.json()?["error"]["code"],
        "payment_identifier_conflict"
    );

    let mut duplicate = settlement;
    duplicate["paymentPayload"]["extensions"]["payment-identifier"]["info"]["id"] =
        json!("payment_http_0000000000000002");
    let duplicate = call(
        &application.router,
        http_request(
            Method::POST,
            "/settle",
            serde_json::to_vec(&duplicate)?,
            Some("application/json"),
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(duplicate.status, StatusCode::OK);
    assert_eq!(duplicate.json()?["errorReason"], "duplicate_settlement");

    let (_official_client, official_key) = seed_client(&database.store, 4, 100, 100).await?;
    let (_official_rate_client, official_rate_key) =
        seed_client(&database.store, 5, 2, 100).await?;
    let (_, invalid_official_key) = api_key(99);
    let official_valid = valid_request(payer, 2, Some("payment_official_000000000001"))?;
    let official_invalid_version = invalid_version_request_with_nonce(payer, 99)?;
    let mut official_conflict = official_valid.clone();
    official_conflict["paymentRequirements"]["extra"] = json!({"changed": true});
    let mut official_duplicate = official_valid.clone();
    official_duplicate["paymentPayload"]["extensions"]["payment-identifier"]["info"]["id"] =
        json!("payment_official_000000000002");
    let official_scenario = json!({
        "network": "near:testnet",
        "apiKey": official_key,
        "rateApiKey": official_rate_key,
        "invalidApiKey": invalid_official_key,
        "expectedPayer": TEST_PAYER,
        "invalidVersion": official_invalid_version,
        "valid": official_valid,
        "conflict": official_conflict,
        "duplicate": official_duplicate,
    });
    let official_client_ran =
        run_official_client_if_requested(&application.router, &official_scenario).await?;
    if std::env::var("X402_RUN_NODE_CLIENT_CONFORMANCE").as_deref() == Ok("1")
        && !official_client_ran
    {
        return Err(
            std::io::Error::other("required NEAR official-client conformance did not run").into(),
        );
    }
    assert_eq!(
        application.rpc.sends.load(Ordering::SeqCst),
        if official_client_ran { 2 } else { 1 }
    );

    assert!(
        database
            .store
            .set_client_budget(
                database
                    .store
                    .lookup_api_key(&api_key(1).0)
                    .await?
                    .ok_or_else(|| std::io::Error::other("test client disappeared"))?
                    .client
                    .id,
                "99",
            )
            .await?
    );
    let budget_request = valid_request(payer, 3, Some("payment_http_0000000000000003"))?;
    let budget = call(
        &application.router,
        http_request(
            Method::POST,
            "/settle",
            serde_json::to_vec(&budget_request)?,
            Some("application/json"),
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(budget.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        budget.json()?["error"]["code"],
        "sponsorship_budget_exhausted"
    );

    // A pre-prepare signer-head outage releases both the row and its budget.
    // Identical identifierless requests may then race to resume one new
    // attempt; altered canonical requests retain duplicate-payment semantics.
    let (retry_client, retry_key) = seed_client(&database.store, 6, 100, 100).await?;
    let retry_request = valid_request(payer, 10, None)?;
    let retry_bytes = serde_json::to_vec(&retry_request)?;
    let sends_before_retry = application.rpc.sends.load(Ordering::SeqCst);
    application.rpc.fail_next_relayer_account_lookup();
    let transient = call(
        &application.router,
        http_request(
            Method::POST,
            "/settle",
            retry_bytes.clone(),
            Some("application/json"),
            Some(&retry_key),
            None,
        )?,
    )
    .await?;
    assert_eq!(transient.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(transient.json()?["error"]["code"], "settlement_retryable");
    assert_eq!(
        transient
            .headers
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok()),
        Some("1")
    );
    assert_eq!(
        application.rpc.sends.load(Ordering::SeqCst),
        sends_before_retry
    );

    let retry_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM settlements \
         WHERE api_client_id = $1 AND payment_identifier IS NULL",
    )
    .bind(retry_client.id)
    .fetch_one(&database.pool)
    .await?;
    let released = database
        .store
        .settlement(retry_id)
        .await?
        .ok_or_else(|| std::io::Error::other("retryable settlement disappeared"))?;
    assert_eq!(released.state, SettlementState::AwaitingRetry);
    assert_eq!(released.reserved_yocto_near, "0");
    assert_eq!(released.attempt_count, 1);
    assert_eq!(released.retry_code.as_deref(), Some("relayer_unavailable"));
    let released_usage = database.store.global_sponsorship_usage_today().await?;
    assert_eq!(released_usage.reserved_yocto_near, "0");

    let mut altered_retry = retry_request;
    altered_retry["paymentRequirements"]["extra"] = json!({"changed": true});
    let altered_retry = call(
        &application.router,
        http_request(
            Method::POST,
            "/settle",
            serde_json::to_vec(&altered_retry)?,
            Some("application/json"),
            Some(&retry_key),
            None,
        )?,
    )
    .await?;
    assert_eq!(altered_retry.status, StatusCode::OK);
    assert_eq!(altered_retry.json()?["errorReason"], "duplicate_settlement");

    ready(&application.readiness);
    application.rpc.keep_payer_nonce_stable();
    let retry_concurrency = 50;
    let barrier = Arc::new(Barrier::new(retry_concurrency + 1));
    let mut retry_tasks = tokio::task::JoinSet::new();
    for _ in 0..retry_concurrency {
        let router = application.router.clone();
        let body = retry_bytes.clone();
        let key = retry_key.clone();
        let barrier = Arc::clone(&barrier);
        retry_tasks.spawn(async move {
            barrier.wait().await;
            call(
                &router,
                http_request(
                    Method::POST,
                    "/settle",
                    body,
                    Some("application/json"),
                    Some(&key),
                    None,
                )?,
            )
            .await
        });
    }
    barrier.wait().await;
    let mut retry_terminal_bytes: Option<Vec<u8>> = None;
    while let Some(joined) = retry_tasks.join_next().await {
        let response = joined??;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.json()?["success"], true);
        if let Some(expected) = &retry_terminal_bytes {
            assert_eq!(&response.bytes, expected);
        } else {
            retry_terminal_bytes = Some(response.bytes);
        }
    }
    assert_eq!(
        application.rpc.sends.load(Ordering::SeqCst),
        sends_before_retry + 1
    );
    let retried = database
        .store
        .settlement(retry_id)
        .await?
        .ok_or_else(|| std::io::Error::other("retried settlement disappeared"))?;
    assert_eq!(retried.state, SettlementState::Succeeded);
    assert_eq!(retried.attempt_count, 2);
    assert_eq!(retried.retry_code, None);
    let retry_states: Vec<String> = sqlx::query_scalar(
        "SELECT to_state FROM settlement_events WHERE settlement_id = $1 ORDER BY id",
    )
    .bind(retry_id)
    .fetch_all(&database.pool)
    .await?;
    assert_eq!(
        retry_states,
        vec![
            "reserved",
            "awaiting_retry",
            "reserved",
            "prepared",
            "submitted",
            "succeeded"
        ]
    );

    Ok(())
}

async fn assert_base_protocol_contract(database: &TestDatabase, metrics: Metrics) -> TestResult {
    let (_client, key) = seed_client(&database.store, 7, 100, 100).await?;
    let (_, invalid_key) = api_key(98);
    let application = build_base_protocol_application(database.store.clone(), metrics)?;

    let supported = call(
        &application.router,
        http_request(Method::GET, "/supported", Vec::new(), None, None, None)?,
    )
    .await?;
    assert_eq!(supported.status, StatusCode::OK);
    let supported = supported.json()?;
    assert_eq!(
        supported["kinds"],
        json!([{
            "x402Version": 2,
            "scheme": "exact",
            "network": "eip155:84532",
        }])
    );
    assert_eq!(supported["extensions"], json!(["payment-identifier"]));
    assert_eq!(
        supported["signers"]["eip155:84532"],
        json!([BASE_PROTOCOL_SIGNER])
    );

    let invalid_version = base_invalid_version_request();
    let verify_response = call(
        &application.router,
        http_request(
            Method::POST,
            "/verify",
            serde_json::to_vec(&invalid_version)?,
            Some("application/json"),
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(verify_response.status, StatusCode::OK);
    assert_eq!(
        verify_response.json()?["invalidReason"],
        "invalid_x402_version"
    );
    let settle_response = call(
        &application.router,
        http_request(
            Method::POST,
            "/settle",
            serde_json::to_vec(&invalid_version)?,
            Some("application/json"),
            Some(&key),
            None,
        )?,
    )
    .await?;
    assert_eq!(settle_response.status, StatusCode::OK);
    assert_eq!(
        settle_response.json()?["errorReason"],
        "invalid_x402_version"
    );

    let scenario = json!({
        "mode": "protocol",
        "network": "eip155:84532",
        "expectedSigner": BASE_PROTOCOL_SIGNER,
        "apiKey": key,
        "invalidApiKey": invalid_key,
        "invalidVersion": invalid_version,
    });
    let official_client_ran =
        run_official_client_if_requested(&application.router, &scenario).await?;
    if std::env::var("X402_RUN_NODE_CLIENT_CONFORMANCE").as_deref() == Ok("1")
        && !official_client_ran
    {
        return Err(
            std::io::Error::other("required Base official-client conformance did not run").into(),
        );
    }
    Ok(())
}

async fn run_official_client_if_requested(router: &Router, scenario: &Value) -> TestResult<bool> {
    if std::env::var("X402_RUN_NODE_CLIENT_CONFORMANCE").as_deref() != Ok("1") {
        eprintln!(
            "skipping official HTTPFacilitatorClient check: \
             X402_RUN_NODE_CLIENT_CONFORMANCE is not 1"
        );
        return Ok(false);
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or_else(|| std::io::Error::other("workspace root is unavailable"))?;
    let harness = root.join("conformance/http-client/check.mjs");
    let installed = root.join("conformance/http-client/node_modules/@x402/core");
    if !installed.is_dir() {
        return Err(std::io::Error::other(
            "run `npm --prefix conformance/http-client ci` before the official-client check",
        )
        .into());
    }
    let scenario_path = std::env::temp_dir().join(format!(
        "x402-http-client-scenario-{}.json",
        Uuid::new_v4().simple()
    ));
    let mut scenario_file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&scenario_path)?;
    scenario_file.write_all(&serde_json::to_vec(scenario)?)?;
    scenario_file.sync_all()?;
    drop(scenario_file);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(axum::serve(listener, router.clone()).into_future());
    let harness_path = harness.clone();
    let scenario_path_for_process = scenario_path.clone();
    let output_result = tokio::task::spawn_blocking(move || {
        Command::new("node")
            .arg(harness_path)
            .arg(format!("http://{address}"))
            .arg(scenario_path_for_process)
            .output()
    })
    .await?;
    server.abort();
    std::fs::remove_file(scenario_path)?;
    let output = output_result?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "official HTTPFacilitatorClient harness failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
        .into());
    }
    let summary: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(summary["supported"], true);
    assert_eq!(summary["invalidVersion"]["verify"], "invalid_x402_version");
    assert_eq!(summary["invalidVersion"]["settle"], "invalid_x402_version");
    assert_eq!(summary["authentication"], true);
    if scenario["mode"] == "protocol" {
        assert_eq!(summary["mode"], "protocol");
    } else {
        assert_eq!(summary["valid"]["verify"], true);
        assert_eq!(summary["valid"]["settle"], true);
        assert_eq!(summary["valid"]["replay"], true);
        assert_eq!(summary["conflict"], 409);
        assert_eq!(summary["duplicate"], "duplicate_settlement");
        assert_eq!(summary["rateLimit"], 429);
    }
    Ok(true)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn custom_http_surface_matches_x402_contract() -> TestResult {
    // This test needs metric handles, not a process-global tracing subscriber.
    let metrics = Metrics::for_tests();
    let payer = test_signer(TEST_PAYER)?;

    let public_application = build_application(disconnected_store(), metrics.clone())?;
    assert_public_contract(&public_application, &payer).await?;

    let Some(database) = TestDatabase::from_explicit_environment().await? else {
        return Ok(());
    };
    let result = async {
        assert_protected_contract(&database, metrics.clone(), &payer).await?;
        assert_base_protocol_contract(&database, metrics).await
    }
    .await;
    let cleanup = database.cleanup().await;
    result?;
    cleanup
}
