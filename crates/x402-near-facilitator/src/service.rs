//! Custom HTTP surface and durable settlement orchestration.
//!
//! `FacilitatorLocal` remains the scheme router.  The HTTP handlers are custom
//! because x402 protocol failures are successful HTTP exchanges, whereas
//! malformed/authentication/availability failures use ordinary HTTP status
//! codes.  Settlement is spawned into a detached task before the handler waits,
//! so dropping or timing out the HTTP request never cancels an in-flight
//! broadcast.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, RETRY_AFTER};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use chrono::Utc;
use near_primitives::action::Action;
use near_primitives::hash::CryptoHash;
use near_primitives::transaction::Transaction;
use near_primitives::types::AccountId;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore};
use tower::ServiceBuilder;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::Instrument as _;
use uuid::Uuid;
use x402_chain_near::{
    VerificationFailure, VerificationPolicy, decode_signed_delegate, decode_signed_transaction,
    signed_delegate_hash, signed_transaction_hash,
};
use x402_facilitator_local::FacilitatorLocal;
use x402_types::chain::ChainId;
use x402_types::facilitator::Facilitator;
use x402_types::proto::{SupportedPaymentKind, SupportedResponse};
use x402_types::scheme::SchemeRegistry;

use crate::VERSION;
use crate::auth::{ApiKeyAuthenticator, AuthError, AuthenticatedClient};
use crate::chain::{
    AuthorizationMetadata, BroadcastOutcome, ChainProvider, Prepared, PreparedDetail,
    ReconcileVerdict, RecoveryPolicy, SignerHead, StoredEvmSubmission, TerminalOutcome,
    VerifiedPayment,
};
use crate::config::{ChainKind, ServiceConfig};
use crate::leadership::ReadinessState;
use crate::protocol::{
    ParsedRequest, SettleResponse, VerifyResponse, decimal_is_at_least, parse_request,
    request_fingerprint,
};
use crate::store::{
    ClaimOutcome, EvmAuthorizationMetadata, EvmPreparedJournalEntry, NewSettlement, PgStore,
    PreparedJournalEntry, RetryOutcome, RetryReservation, SettlementRecord, SettlementState,
    StoreError, TerminalJournalEntry,
};
use crate::telemetry::Metrics;
use crate::v1_compat::{self, WireVersion};

const SERVICE_NAME: &str = "x402-near-facilitator";
const RETRY_SECONDS: &str = "1";
const LANDING_PAGE: &str = concat!(
    r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="description" content="Open-source x402 exact-payment facilitator for Circle USDC on NEAR and Base.">
  <title>x402 facilitator for NEAR and Base</title>
</head>
<body>
  <main>
    <h1>x402 facilitator for NEAR and Base</h1>
    <p>Open-source, API-key-gated settlement for x402 <code>exact</code>
    payments in Circle USDC, with sponsored gas and durable recovery.</p>
    <p>This hostname is one network-pinned facilitator instance. Inspect its
    live protocol capabilities at <a href="/supported">/supported</a>.</p>
    <p>The reference deployment serves NEAR mainnet, NEAR testnet, and Base
    mainnet. Canonical x402 v2 is preferred; the scheme is <code>exact</code>,
    the asset is Circle USDC, the facilitator fee is zero, and sponsored gas
    is bounded by per-client policy.</p>
    <ul>
      <li><a href="https://github.com/fastnear/x402-facilitator">Source and documentation</a></li>
      <li><a href="https://github.com/fastnear/x402-facilitator/blob/main/docs/reference-access.md">Request reference-instance access</a></li>
      <li><a href="https://github.com/fastnear/x402-facilitator/security/policy">Security policy</a></li>
    </ul>
    <p>Service version <code>"#,
    env!("CARGO_PKG_VERSION"),
    r#"</code>.</p>
  </main>
</body>
</html>
"#
);

#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct AppState {
    config: Arc<ServiceConfig>,
    store: PgStore,
    auth: ApiKeyAuthenticator,
    // The x402-rs contract surface for /verify and /supported. `Some` for NEAR
    // (V2NearExact + NearChainProvider); `None` for EVM, whose read-only verify is
    // served directly by the neutral provider (the upstream V2Eip155Exact blueprint
    // is generic over `&P` and cannot be registered via this assembly).
    facilitator: Option<Arc<FacilitatorLocal<SchemeRegistry>>>,
    provider: Arc<ChainProvider>,
    readiness: ReadinessState,
    rates: Arc<RateLimiter>,
    verify_slots: Arc<Semaphore>,
    relayer_lock: Arc<Mutex<()>>,
    metrics: Metrics,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: ServiceConfig,
        store: PgStore,
        auth: ApiKeyAuthenticator,
        facilitator: Option<FacilitatorLocal<SchemeRegistry>>,
        provider: ChainProvider,
        readiness: ReadinessState,
        metrics: Metrics,
    ) -> Self {
        let max_concurrent_verify = config.request_limits.max_concurrent_verify;
        Self {
            config: Arc::new(config),
            store,
            auth,
            facilitator: facilitator.map(Arc::new),
            provider: Arc::new(provider),
            readiness,
            rates: Arc::new(RateLimiter::default()),
            verify_slots: Arc::new(Semaphore::new(max_concurrent_verify)),
            relayer_lock: Arc::new(Mutex::new(())),
            metrics,
        }
    }

    pub fn readiness(&self) -> &ReadinessState {
        &self.readiness
    }

    pub fn store(&self) -> &PgStore {
        &self.store
    }

    pub fn relayer_lock(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.relayer_lock)
    }

    /// Refresh the chain-dependent readiness gates.  Both independent RPCs
    /// must report the configured network and finality, and the configured
    /// relayer key must be `FullAccess`, active in policy, and funded above the
    /// hard-stop threshold.
    pub async fn refresh_chain_readiness(&self) -> bool {
        let rpc_ready = self.provider.readiness_probe().await;
        self.readiness.set_rpc(rpc_ready);

        let signer = self.provider.signer_head().await;
        let signer_account_id = self.provider.signer_account_id();
        let policy_active = self
            .store
            .relayer_is_active(
                &self.config.network,
                &signer_account_id,
                &self.provider.signer_public_key(),
            )
            .await
            .unwrap_or(false);
        if let Ok(head) = &signer
            && let Ok(balance_yocto_near) = head.signer_balance_atomic.to_string().parse::<f64>()
        {
            self.metrics.record_relayer(
                balance_yocto_near / 1_000_000_000_000_000_000_000_000_f64,
                !policy_active,
            );
        }
        let relayer_ready = signer
            .is_ok_and(|head| signer_is_funded(&self.config, head.signer_balance_atomic))
            && policy_active;
        self.readiness.set_relayer(relayer_ready);

        if let Ok(summary) = self.store.journal_summary().await {
            let total = summary
                .reserved
                .saturating_add(summary.prepared)
                .saturating_add(summary.submitted);
            self.metrics.record_pending_settlements(total);
            self.metrics
                .record_journal_state("reserved", summary.reserved);
            self.metrics
                .record_journal_state("prepared", summary.prepared);
            self.metrics
                .record_journal_state("submitted", summary.submitted);
            let age = summary
                .oldest_created_at
                .and_then(|created| (Utc::now() - created).to_std().ok())
                .map_or(0.0, |duration| duration.as_secs_f64());
            self.metrics.record_oldest_pending_age(age);
        }
        if let Ok(usage) = self.store.global_sponsorship_usage_today().await
            && let Some(ratio) = decimal_usage_ratio(
                &usage.reserved_yocto_near,
                &usage.spent_yocto_near,
                &self.config.sponsorship.global_daily_yocto_near,
            )
        {
            self.metrics.record_budget_used_ratio(ratio);
        }
        rpc_ready && relayer_ready
    }
}

fn decimal_usage_ratio(reserved: &str, spent: &str, limit: &str) -> Option<f64> {
    let limit = limit.parse::<f64>().ok()?;
    if limit <= 0.0 {
        return None;
    }
    Some((reserved.parse::<f64>().ok()? + spent.parse::<f64>().ok()?) / limit)
}

fn signer_is_funded(config: &ServiceConfig, balance: u128) -> bool {
    let Ok(hard_stop) = config
        .sponsorship
        .balance_hard_stop_yocto_near
        .parse::<u128>()
    else {
        return false;
    };
    let required = if config.chain_kind == ChainKind::Eip155 {
        let Ok(reservation) = config.sponsorship.reservation_yocto_near.parse::<u128>() else {
            return false;
        };
        hard_stop.checked_add(reservation)
    } else {
        Some(hard_stop)
    };
    required.is_some_and(|required| balance >= required)
}

fn require_evm_recovery_balance(
    state: &AppState,
    record: &SettlementRecord,
    balance: u128,
) -> Result<(), StoreError> {
    let required = state
        .config
        .sponsorship
        .balance_hard_stop_yocto_near
        .parse::<u128>()
        .map_err(|_| StoreError::Corrupt("configured EVM balance hard stop is invalid".to_owned()))
        .and_then(|hard_stop| {
            record
                .reserved_yocto_near
                .parse::<u128>()
                .map_err(|_| {
                    StoreError::Corrupt(
                        "journaled EVM sponsorship reservation is invalid".to_owned(),
                    )
                })
                .and_then(|reservation| {
                    hard_stop.checked_add(reservation).ok_or_else(|| {
                        StoreError::Corrupt(
                            "EVM recovery balance requirement overflowed".to_owned(),
                        )
                    })
                })
        });
    let required = match required {
        Ok(required) => required,
        Err(error) => {
            state.readiness.set_relayer(false);
            state.readiness.set_reconciliation(false);
            return Err(error);
        }
    };
    if balance < required {
        state.readiness.set_relayer(false);
        state.readiness.set_reconciliation(false);
        return Err(StoreError::Corrupt(
            "EVM signer balance does not cover the durable recovery reservation".to_owned(),
        ));
    }
    Ok(())
}

pub fn router(state: AppState) -> Router {
    let request_id = HeaderName::from_static("x-request-id");
    Router::new()
        .route("/", get(landing))
        .route("/supported", get(supported))
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/verify", post(verify))
        .route("/settle", post(settle))
        .with_state(state)
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::new(request_id.clone(), MakeRequestUuid))
                .layer(SetSensitiveRequestHeadersLayer::new([
                    AUTHORIZATION,
                    HeaderName::from_static("x-api-key"),
                ]))
                .layer(
                    TraceLayer::new_for_http()
                        .make_span_with(|request: &Request| {
                            tracing::info_span!(
                                "http_request",
                                method = %request.method()
                            )
                        })
                        .on_response(DefaultOnResponse::new().include_headers(false)),
                )
                .layer(PropagateRequestIdLayer::new(request_id)),
        )
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    retry: bool,
}

impl ApiError {
    const fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
            retry: false,
        }
    }

    const fn unavailable(code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code,
            message,
            retry: true,
        }
    }

    const fn rate_limited(code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code,
            message,
            retry: true,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({
            "error": {
                "code": self.code,
                "message": self.message,
            }
        });
        let mut response = (self.status, axum::Json(body)).into_response();
        if self.retry {
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from_static(RETRY_SECONDS));
        }
        response
    }
}

#[derive(Serialize)]
struct HealthResponse<'a> {
    status: &'a str,
    service: &'a str,
    version: &'a str,
}

#[derive(Serialize)]
struct ReadyResponse {
    ready: bool,
    checks: ReadyChecks,
}

#[derive(Serialize)]
struct ReadyChecks {
    database: &'static str,
    leadership: &'static str,
    reconciliation: &'static str,
    rpc: &'static str,
    relayer: &'static str,
}

async fn landing() -> Html<&'static str> {
    Html(LANDING_PAGE)
}

async fn health() -> axum::Json<HealthResponse<'static>> {
    axum::Json(HealthResponse {
        status: "ok",
        service: SERVICE_NAME,
        version: VERSION,
    })
}

async fn ready(State(state): State<AppState>) -> Response {
    let database = state
        .store
        .operationally_ready(&state.config.network, &state.config.asset)
        .await
        .unwrap_or(false);
    let snapshot = state.readiness.snapshot();
    let is_ready = database
        && snapshot.leadership
        && snapshot.reconciliation
        && snapshot.rpc
        && snapshot.relayer;
    let body = ReadyResponse {
        ready: is_ready,
        checks: ReadyChecks {
            database: readiness_word(database),
            leadership: readiness_word(snapshot.leadership),
            reconciliation: readiness_word(snapshot.reconciliation),
            rpc: readiness_word(snapshot.rpc),
            relayer: readiness_word(snapshot.relayer),
        },
    };
    let status = if is_ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let mut response = (status, axum::Json(body)).into_response();
    if !is_ready {
        response
            .headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from_static(RETRY_SECONDS));
    }
    response
}

const fn readiness_word(value: bool) -> &'static str {
    if value { "ready" } else { "not_ready" }
}

async fn supported(State(state): State<AppState>) -> Response {
    let response = match &state.facilitator {
        Some(facilitator) => match facilitator.supported().await {
            Ok(response) => response,
            Err(_) => {
                return ApiError::unavailable(
                    "facilitator_unavailable",
                    "supported payment methods are temporarily unavailable",
                )
                .into_response();
            }
        },
        // EVM has no registered facilitator; advertise the single configured
        // eip155 exact scheme, USDC network, and the facilitator signer address
        // clients embed in the payment authorization.
        None => evm_supported(&state),
    };
    axum::Json(response).into_response()
}

/// Synthesize the `/supported` response for an EVM instance from config. NEAR
/// derives the equivalent from its registered facilitator handlers.
fn evm_supported(state: &AppState) -> SupportedResponse {
    evm_supported_for(
        &state.config.network,
        state.config.accept_v1,
        &state.provider.signer_account_id(),
    )
}

fn evm_supported_for(network: &str, accept_v1: bool, signer: &str) -> SupportedResponse {
    let mut signers = std::collections::HashMap::new();
    if let Ok(chain_id) = network.parse::<ChainId>() {
        signers.insert(chain_id, vec![signer.to_owned()]);
    }
    let mut kinds = vec![SupportedPaymentKind {
        x402_version: 2,
        scheme: "exact".to_owned(),
        network: network.to_owned(),
        extra: None,
    }];
    if accept_v1 && let Some(alias) = v1_compat::v1_network_name(network) {
        kinds.push(SupportedPaymentKind {
            x402_version: 1,
            scheme: "exact".to_owned(),
            network: alias.to_owned(),
            extra: None,
        });
    }
    SupportedResponse {
        kinds,
        extensions: vec!["payment-identifier".to_owned()],
        signers,
    }
}

async fn verify(State(state): State<AppState>, request: Request) -> Response {
    let started = Instant::now();
    let deadline = Duration::from_secs(state.config.request_limits.verify_timeout_seconds);
    let response = match tokio::time::timeout(deadline, verify_inner(&state, request)).await {
        Ok(response) => response,
        Err(_) => {
            ApiError::unavailable("verification_timeout", "verification timed out").into_response()
        }
    };
    state.metrics.record_request(
        "verify",
        if response.status().is_success() {
            "completed"
        } else {
            "rejected"
        },
        started.elapsed().as_secs_f64(),
    );
    response
}

async fn verify_inner(state: &AppState, request: Request) -> Response {
    let authenticated = match authenticate(state, request.headers()).await {
        Ok(authenticated) => authenticated,
        Err(error) => return error.into_response(),
    };
    let client = authenticated.client;
    if !state
        .rates
        .check(
            &authenticated.key_prefix,
            Operation::Verify,
            client
                .verify_rate_per_minute
                .min(state.config.request_limits.verify_per_minute),
        )
        .await
    {
        return ApiError::rate_limited("rate_limit_exceeded", "verification rate limit exceeded")
            .into_response();
    }
    let parsed = match read_and_parse(state, request).await {
        Ok(parsed) => parsed,
        Err(error) => return error.into_response(),
    };
    // Every protocol response below (including journal replays) flows through
    // the wire-dialect finalizer, so a legacy v1 request gets v1-shaped output
    // no matter which branch produced it.
    let wire = parsed.wire;
    let response = async {
        if let Some(response) = static_verify_failure(state, &parsed) {
            return protocol_json(StatusCode::OK, &response);
        }
        match state
            .store
            .payee_allowed(
                client.id,
                &parsed.meta.network,
                &parsed.meta.asset,
                &parsed.meta.pay_to,
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                return protocol_json(
                    StatusCode::OK,
                    &VerifyResponse::invalid("payee_not_allowed", None, None),
                );
            }
            Err(_) => {
                return ApiError::unavailable(
                    "database_unavailable",
                    "verification policy is temporarily unavailable",
                )
                .into_response();
            }
        }

        let Ok(permit) = state.verify_slots.clone().try_acquire_owned() else {
            return ApiError::unavailable(
                "verification_capacity_exhausted",
                "verification capacity is temporarily exhausted",
            )
            .into_response();
        };
        let deadline = Duration::from_secs(state.config.request_limits.verify_timeout_seconds);
        let Some(facilitator) = &state.facilitator else {
            // EVM: the neutral provider verifies (no registered facilitator surface).
            let response = match tokio::time::timeout(deadline, evm_verify(state, &parsed)).await {
                Ok(response) => response,
                Err(_) => {
                    ApiError::unavailable("verification_timeout", "EVM verification timed out")
                        .into_response()
                }
            };
            drop(permit);
            return response;
        };
        let response = near_routed_verify(facilitator, &parsed, deadline).await;
        drop(permit);
        response
    }
    .await;
    finalize_wire_response(response, wire).await
}

/// Route a NEAR verify through the registered facilitator. Parity with the
/// EVM retry: an ambiguous lookup response gets the same bounded retry before
/// surfacing as a 503. Timeouts are not retried — the outer verify deadline
/// already bounds the exchange.
async fn near_routed_verify(
    facilitator: &FacilitatorLocal<SchemeRegistry>,
    parsed: &ParsedRequest,
    deadline: Duration,
) -> Response {
    let result = crate::retry::retry_while_transient(
        || tokio::time::timeout(deadline, facilitator.verify(&parsed.raw)),
        |outcome| matches!(outcome, Ok(Ok(response)) if response_is_rpc_ambiguous(&response.0)),
    )
    .await;
    match result {
        Ok(Ok(response)) => {
            if response_is_rpc_ambiguous(&response.0) {
                ApiError::unavailable(
                    "rpc_unavailable",
                    "NEAR verification is temporarily unavailable",
                )
                .into_response()
            } else {
                axum::Json(response.0).into_response()
            }
        }
        Ok(Err(_)) | Err(_) => ApiError::unavailable(
            "verification_unavailable",
            "NEAR verification is temporarily unavailable",
        )
        .into_response(),
    }
}

/// Verify an EVM payment through the neutral provider (EVM has no registered
/// facilitator surface). Mirrors the NEAR verify disposition: a valid payment
/// returns `isValid: true`; an ambiguous on-chain lookup is a 503; a definitive
/// rejection returns `isValid: false` with the machine reason.
async fn evm_verify(state: &AppState, parsed: &ParsedRequest) -> Response {
    let policy = VerificationPolicy {
        max_sponsored_gas: state.config.max_inner_gas,
    };
    match state.provider.verify(&parsed.raw, &policy).await {
        Ok(verified) => protocol_json(StatusCode::OK, &VerifyResponse::valid(verified.payer)),
        Err(rejection) if rejection.rpc_ambiguous => ApiError::unavailable(
            "rpc_unavailable",
            "EVM verification is temporarily unavailable",
        )
        .into_response(),
        Err(rejection) => protocol_json(
            StatusCode::OK,
            &VerifyResponse::invalid(&rejection.reason, None, None),
        ),
    }
}

/// The NEAR x402-rs scheme gate for settlement: route the raw payment through the
/// registered facilitator and short-circuit if it is unavailable or reports the
/// payment invalid (deferring to a prior settlement on a race). Returns
/// `Some(response)` to short-circuit settlement, `None` to proceed to the durable
/// journal. EVM has no registered facilitator and skips this gate. The body is
/// unchanged from the former inline gate.
async fn facilitator_verify_gate(
    state: &AppState,
    facilitator: &FacilitatorLocal<SchemeRegistry>,
    parsed: &ParsedRequest,
    payment_hash: &[u8; 32],
    client_id: Uuid,
    fingerprint: &[u8; 32],
) -> Option<Response> {
    // Parity with the EVM retry: retry ambiguous lookup responses before the
    // gate short-circuits settlement with a 503.
    let routed = crate::retry::retry_while_transient(
        || facilitator.verify(&parsed.raw),
        |outcome| matches!(outcome, Ok(response) if response_is_rpc_ambiguous(&response.0)),
    )
    .await;
    let Ok(routed) = routed else {
        if let Some(response) = prior_settlement_race_response(
            state,
            client_id,
            parsed.meta.payment_identifier.as_deref(),
            payment_hash,
            fingerprint,
        )
        .await
        {
            return Some(response);
        }
        return Some(
            ApiError::unavailable(
                "verification_unavailable",
                "NEAR verification is temporarily unavailable",
            )
            .into_response(),
        );
    };
    if !routed
        .0
        .get("isValid")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        if let Some(response) = prior_settlement_race_response(
            state,
            client_id,
            parsed.meta.payment_identifier.as_deref(),
            payment_hash,
            fingerprint,
        )
        .await
        {
            return Some(response);
        }
        if response_is_rpc_ambiguous(&routed.0) {
            return Some(
                ApiError::unavailable(
                    "rpc_unavailable",
                    "NEAR verification is temporarily unavailable",
                )
                .into_response(),
            );
        }
        return Some(settle_from_verify_failure(&routed.0, &state.config.network).into_response());
    }
    None
}

async fn settle(State(state): State<AppState>, request: Request) -> Response {
    let started = Instant::now();
    let deadline = Duration::from_secs(state.config.request_limits.settle_timeout_seconds);
    let response = match tokio::time::timeout(deadline, settle_inner(&state, request)).await {
        Ok(response) => response,
        Err(_) => ApiError::unavailable(
            "settlement_pending",
            "settlement is still pending; retry with the same payment identifier",
        )
        .into_response(),
    };
    state.metrics.record_request(
        "settle",
        if response.status().is_success() {
            "completed"
        } else {
            "rejected"
        },
        started.elapsed().as_secs_f64(),
    );
    response
}

// The ordered HTTP flow mirrors the security boundary from authentication
// through durable claim creation and terminal replay.
#[allow(clippy::too_many_lines)]
async fn settle_inner(state: &AppState, request: Request) -> Response {
    let authenticated = match authenticate(state, request.headers()).await {
        Ok(authenticated) => authenticated,
        Err(error) => return error.into_response(),
    };
    let client = authenticated.client;
    if !state
        .rates
        .check(
            &authenticated.key_prefix,
            Operation::Settle,
            client
                .settle_rate_per_minute
                .min(state.config.request_limits.settle_per_minute),
        )
        .await
    {
        return ApiError::rate_limited("rate_limit_exceeded", "settlement rate limit exceeded")
            .into_response();
    }
    let parsed = match read_and_parse(state, request).await {
        Ok(parsed) => parsed,
        Err(error) => return error.into_response(),
    };
    // Every protocol response below (including journal replays) flows through
    // the wire-dialect finalizer, so a legacy v1 request gets v1-shaped output
    // no matter which branch produced it.
    let wire = parsed.wire;
    let response = async {
        // Reject a statically unsupported protocol envelope before deriving a
        // chain-specific payment identity. This keeps invalid-version behavior
        // chain-neutral and avoids asking a provider to interpret a dialect it
        // does not serve.
        if let Some(response) = static_settle_failure(state, &parsed) {
            return protocol_json(StatusCode::OK, &response);
        }
        // Chain-neutral pre-verify payment identity for idempotency. NEAR decodes
        // and signature-checks the base64 signed delegate; eip155 computes the
        // offline ERC-3009 EIP-712 transfer hash from the authorization (no RPC).
        // The authoritative on-chain validity check is `provider.verify` below, for
        // both chains; this only establishes the idempotency key.
        let payment_hash = match state.config.chain_kind {
            ChainKind::Near => {
                let signed_delegate_action =
                    parsed.meta.signed_delegate_action.as_deref().unwrap_or("");
                let decoded = match decode_signed_delegate(signed_delegate_action) {
                    Ok(decoded) => decoded,
                    Err(failure) => {
                        return protocol_json(
                            StatusCode::OK,
                            &SettleResponse::failure(
                                failure.reason(),
                                None,
                                None,
                                String::new(),
                                state.config.network.clone(),
                            ),
                        );
                    }
                };
                if !decoded.signed_delegate.verify() {
                    return protocol_json(
                        StatusCode::OK,
                        &SettleResponse::failure(
                            VerificationFailure::InvalidSignature.reason(),
                            None,
                            None,
                            String::new(),
                            state.config.network.clone(),
                        ),
                    );
                }
                decoded.payment_hash
            }
            ChainKind::Eip155 => match state.provider.offline_payment_hash(&parsed.raw) {
                Ok(hash) => hash,
                Err(reason) => {
                    return protocol_json(
                        StatusCode::OK,
                        &SettleResponse::failure(
                            reason,
                            None,
                            None,
                            String::new(),
                            state.config.network.clone(),
                        ),
                    );
                }
            },
        };
        let Ok(fingerprint) = request_fingerprint(&parsed.value, &payment_hash) else {
            return ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request JSON cannot be canonicalized",
            )
            .into_response();
        };
        match prior_settlement_response(
            state,
            client.id,
            parsed.meta.payment_identifier.as_deref(),
            &payment_hash,
            &fingerprint,
        )
        .await
        {
            Ok(Some(response)) => return response,
            Ok(None) => {}
            Err(_) => {
                return ApiError::unavailable(
                    "database_unavailable",
                    "settlement journal is temporarily unavailable",
                )
                .into_response();
            }
        }
        if !state.readiness.can_settle() {
            if let Some(response) = prior_settlement_race_response(
                state,
                client.id,
                parsed.meta.payment_identifier.as_deref(),
                &payment_hash,
                &fingerprint,
            )
            .await
            {
                return response;
            }
            return ApiError::unavailable(
                "settlement_unavailable",
                "settlement is temporarily unavailable",
            )
            .into_response();
        }
        match state
            .store
            .payee_allowed(
                client.id,
                &parsed.meta.network,
                &parsed.meta.asset,
                &parsed.meta.pay_to,
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                if let Some(response) = prior_settlement_race_response(
                    state,
                    client.id,
                    parsed.meta.payment_identifier.as_deref(),
                    &payment_hash,
                    &fingerprint,
                )
                .await
                {
                    return response;
                }
                return protocol_json(
                    StatusCode::OK,
                    &SettleResponse::failure(
                        "payee_not_allowed",
                        None,
                        None,
                        String::new(),
                        state.config.network.clone(),
                    ),
                );
            }
            Err(_) => {
                return ApiError::unavailable(
                    "database_unavailable",
                    "settlement policy is temporarily unavailable",
                )
                .into_response();
            }
        }

        // Route the raw payment through the registered x402-rs scheme before exposing
        // the chain-specific VerifiedPayment needed by the durable journal. EVM has no
        // registered facilitator; its neutral provider.verify below is the sole
        // verification, so the gate is skipped for EVM.
        if let Some(facilitator) = &state.facilitator
            && let Some(response) = facilitator_verify_gate(
                state,
                facilitator,
                &parsed,
                &payment_hash,
                client.id,
                &fingerprint,
            )
            .await
        {
            return response;
        }
        let policy = VerificationPolicy {
            max_sponsored_gas: state.config.max_inner_gas,
        };
        let verified = match state.provider.verify(&parsed.raw, &policy).await {
            Ok(verified) => verified,
            Err(rejection) if rejection.rpc_ambiguous => {
                if let Some(response) = prior_settlement_race_response(
                    state,
                    client.id,
                    parsed.meta.payment_identifier.as_deref(),
                    &payment_hash,
                    &fingerprint,
                )
                .await
                {
                    return response;
                }
                return ApiError::unavailable(
                    "rpc_unavailable",
                    "verification is temporarily unavailable",
                )
                .into_response();
            }
            Err(rejection) => {
                if let Some(response) = prior_settlement_race_response(
                    state,
                    client.id,
                    parsed.meta.payment_identifier.as_deref(),
                    &payment_hash,
                    &fingerprint,
                )
                .await
                {
                    return response;
                }
                return protocol_json(
                    StatusCode::OK,
                    &SettleResponse::failure(
                        rejection.reason,
                        None,
                        None,
                        String::new(),
                        state.config.network.clone(),
                    ),
                );
            }
        };
        if verified.payment_hash != payment_hash {
            return ApiError::unavailable(
                "verification_inconsistent",
                "payment verification was internally inconsistent",
            )
            .into_response();
        }
        // Persist only the neutral single-use identity and the minimum metadata
        // needed to validate a prepared submission during recovery. In particular,
        // the signed EVM authorization is never copied into the reservation row.
        let identity = verified.identity();
        let (
            chain_kind,
            delegate_public_key,
            delegate_nonce,
            delegate_max_block_height,
            authorization_metadata,
            signer_address,
        ) = match identity.authorization {
            AuthorizationMetadata::Near {
                delegate_public_key,
                delegate_nonce,
                max_block_height,
            } => (
                ChainKind::Near,
                Some(delegate_public_key),
                Some(delegate_nonce),
                Some(max_block_height),
                None,
                None,
            ),
            AuthorizationMetadata::Evm {
                version,
                valid_after,
                valid_before,
            } => (
                ChainKind::Eip155,
                None,
                None,
                None,
                Some(EvmAuthorizationMetadata {
                    version,
                    valid_after,
                    valid_before,
                }),
                Some(state.provider.signer_account_id()),
            ),
        };
        let new = NewSettlement {
            id: Uuid::new_v4(),
            api_client_id: client.id,
            payment_identifier: parsed.meta.payment_identifier.clone(),
            payment_hash: identity.request_hash,
            request_fingerprint: fingerprint,
            anchor_scope: identity.anchor_scope,
            anchor_value: identity.anchor_value,
            x402_version: parsed.meta.x402_version,
            scheme: parsed.meta.scheme.clone(),
            network: parsed.meta.network.clone(),
            asset: parsed.meta.asset.clone(),
            pay_to: parsed.meta.pay_to.clone(),
            amount: parsed.meta.amount.clone(),
            payer: verified.payer.clone(),
            chain_kind,
            delegate_public_key,
            delegate_nonce,
            delegate_max_block_height,
            authorization_metadata,
            signer_address,
            policy_snapshot: state.config.policy_snapshot(),
            reservation_yocto_near: state.config.sponsorship.reservation_yocto_near.clone(),
            global_daily_budget_yocto_near: state
                .config
                .sponsorship
                .global_daily_yocto_near
                .clone(),
            client_daily_budget_yocto_near: client.daily_budget_yocto_near.clone(),
        };
        let Ok(claim) = state.store.claim_settlement(&new).await else {
            return ApiError::unavailable(
                "database_unavailable",
                "settlement journal is temporarily unavailable",
            )
            .into_response();
        };
        let settlement_id = match claim {
            ClaimOutcome::New(record) => {
                spawn_settlement_worker(state, record.id, parsed.raw.clone());
                record.id
            }
            ClaimOutcome::Existing(record) => {
                state.metrics.record_idempotency_replay();
                if record.state.is_terminal() {
                    return stored_terminal_response(&record);
                }
                if record.state == SettlementState::AwaitingRetry {
                    let retry = RetryReservation {
                        settlement_id: record.id,
                        policy_snapshot: state.config.policy_snapshot(),
                        reservation_yocto_near: state
                            .config
                            .sponsorship
                            .reservation_yocto_near
                            .clone(),
                        global_daily_budget_yocto_near: state
                            .config
                            .sponsorship
                            .global_daily_yocto_near
                            .clone(),
                        client_daily_budget_yocto_near: client.daily_budget_yocto_near.clone(),
                    };
                    match state.store.resume_awaiting_retry(&retry).await {
                        Ok(RetryOutcome::Resumed(resumed)) => {
                            spawn_settlement_worker(state, resumed.id, parsed.raw.clone());
                            resumed.id
                        }
                        Ok(RetryOutcome::SettlementBusy) => {
                            return ApiError::unavailable(
                                "settlement_busy",
                                "the configured signer is settling another payment",
                            )
                            .into_response();
                        }
                        Ok(RetryOutcome::BudgetExceeded) => {
                            return ApiError::rate_limited(
                                "sponsorship_budget_exhausted",
                                "sponsorship budget is exhausted",
                            )
                            .into_response();
                        }
                        // A concurrent identical retry may have resumed the row
                        // after our lookup. Join that attempt rather than treating
                        // the expected state race as a database outage.
                        Err(StoreError::Transition { .. }) => {
                            match state.store.settlement(record.id).await {
                                Ok(Some(current))
                                    if current.state != SettlementState::AwaitingRetry =>
                                {
                                    current.id
                                }
                                Ok(Some(_)) => {
                                    return ApiError::unavailable(
                                        "settlement_retryable",
                                        "settlement can be retried with the same request",
                                    )
                                    .into_response();
                                }
                                Ok(None) | Err(_) => {
                                    return ApiError::unavailable(
                                        "database_unavailable",
                                        "settlement journal is temporarily unavailable",
                                    )
                                    .into_response();
                                }
                            }
                        }
                        Err(_) => {
                            return ApiError::unavailable(
                                "database_unavailable",
                                "settlement journal is temporarily unavailable",
                            )
                            .into_response();
                        }
                    }
                } else {
                    record.id
                }
            }
            ClaimOutcome::IdentifierConflict => {
                return ApiError::new(
                    StatusCode::CONFLICT,
                    "payment_identifier_conflict",
                    "payment identifier was already used for another request",
                )
                .into_response();
            }
            ClaimOutcome::DuplicateSettlement => {
                return protocol_json(
                    StatusCode::OK,
                    &SettleResponse::failure(
                        "duplicate_settlement",
                        None,
                        Some(verified.payer.clone()),
                        String::new(),
                        state.config.network.clone(),
                    ),
                );
            }
            ClaimOutcome::SettlementBusy => {
                return ApiError::unavailable(
                    "settlement_busy",
                    "the configured signer is settling another payment",
                )
                .into_response();
            }
            ClaimOutcome::BudgetExceeded => {
                return ApiError::rate_limited(
                    "sponsorship_budget_exhausted",
                    "sponsorship budget is exhausted",
                )
                .into_response();
            }
        };

        let deadline = Duration::from_secs(state.config.request_limits.settle_timeout_seconds);
        match tokio::time::timeout(deadline, wait_for_terminal(&state.store, settlement_id)).await {
            Ok(Ok(record)) => completed_settlement_response(&record),
            Ok(Err(_)) => ApiError::unavailable(
                "database_unavailable",
                "settlement journal is temporarily unavailable",
            )
            .into_response(),
            Err(_) => ApiError::unavailable(
                "settlement_pending",
                "settlement is still pending; retry with the same payment identifier",
            )
            .into_response(),
        }
    }
    .await;
    finalize_wire_response(response, wire).await
}

/// Protocol responses are small in-memory JSON bodies; this bound only guards
/// the re-buffering in [`finalize_wire_response`] against a pathological bug.
const PROTOCOL_RESPONSE_REBUFFER_LIMIT: usize = 1_048_576;

/// Rewrite a successful protocol response into the legacy v1 dialect when the
/// request arrived as v1 wire. Our response fields are already a v1-compatible
/// superset, so the only change is echoing `network` as the legacy alias.
/// Non-200 responses (auth, rate, malformed, availability) pass through: v1
/// SDKs treat those as ordinary HTTP failures.
async fn finalize_wire_response(response: Response, wire: WireVersion) -> Response {
    if wire == WireVersion::V2 || response.status() != StatusCode::OK {
        return response;
    }
    let (parts, body) = response.into_parts();
    let Ok(bytes) = to_bytes(body, PROTOCOL_RESPONSE_REBUFFER_LIMIT).await else {
        return ApiError::unavailable(
            "response_serialization_failed",
            "response serialization failed",
        )
        .into_response();
    };
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    v1_compat::translate_response_value_to_v1(&mut value);
    match serde_json::to_vec(&value) {
        Ok(translated) => {
            let mut response = Response::from_parts(parts, Body::from(translated));
            response.headers_mut().remove(CONTENT_LENGTH);
            response
        }
        Err(_) => ApiError::unavailable(
            "response_serialization_failed",
            "response serialization failed",
        )
        .into_response(),
    }
}

async fn prior_settlement_response(
    state: &AppState,
    api_client_id: Uuid,
    payment_identifier: Option<&str>,
    payment_hash: &[u8; 32],
    request_fingerprint: &[u8; 32],
) -> Result<Option<Response>, StoreError> {
    let Some(claim) = state
        .store
        .find_existing_settlement(
            api_client_id,
            payment_identifier,
            payment_hash,
            request_fingerprint,
        )
        .await?
    else {
        return Ok(None);
    };
    let response = match claim {
        ClaimOutcome::Existing(record) => {
            state.metrics.record_idempotency_replay();
            if record.state == SettlementState::AwaitingRetry {
                return Ok(None);
            }
            if record.state.is_terminal() {
                stored_terminal_response(&record)
            } else {
                let deadline =
                    Duration::from_secs(state.config.request_limits.settle_timeout_seconds);
                match tokio::time::timeout(deadline, wait_for_terminal(&state.store, record.id))
                    .await
                {
                    Ok(Ok(record)) => completed_settlement_response(&record),
                    Ok(Err(error)) => return Err(error),
                    Err(_) => ApiError::unavailable(
                        "settlement_pending",
                        "settlement is still pending; retry with the same payment identifier",
                    )
                    .into_response(),
                }
            }
        }
        ClaimOutcome::IdentifierConflict => ApiError::new(
            StatusCode::CONFLICT,
            "payment_identifier_conflict",
            "payment identifier was already used for another request",
        )
        .into_response(),
        ClaimOutcome::DuplicateSettlement => protocol_json(
            StatusCode::OK,
            &SettleResponse::failure(
                "duplicate_settlement",
                None,
                None,
                String::new(),
                state.config.network.clone(),
            ),
        ),
        ClaimOutcome::New(_) | ClaimOutcome::SettlementBusy | ClaimOutcome::BudgetExceeded => {
            return Err(StoreError::Corrupt(
                "existing-settlement lookup returned a non-existing outcome".to_owned(),
            ));
        }
    };
    Ok(Some(response))
}

async fn prior_settlement_race_response(
    state: &AppState,
    api_client_id: Uuid,
    payment_identifier: Option<&str>,
    payment_hash: &[u8; 32],
    request_fingerprint: &[u8; 32],
) -> Option<Response> {
    match prior_settlement_response(
        state,
        api_client_id,
        payment_identifier,
        payment_hash,
        request_fingerprint,
    )
    .await
    {
        Ok(response) => response,
        Err(_) => Some(
            ApiError::unavailable(
                "database_unavailable",
                "settlement journal is temporarily unavailable",
            )
            .into_response(),
        ),
    }
}

async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<AuthenticatedClient, ApiError> {
    match state.auth.authenticate(headers).await {
        Ok(authenticated) => {
            let store = state.store.clone();
            let prefix = authenticated.key_prefix.clone();
            tokio::spawn(async move {
                let _result = store.touch_api_key(&prefix).await;
            });
            Ok(authenticated)
        }
        Err(AuthError::Invalid | AuthError::Configuration) => Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_api_key",
            "missing or invalid API key",
        )),
        Err(AuthError::Store(_) | AuthError::Entropy) => Err(ApiError::unavailable(
            "authentication_unavailable",
            "authentication is temporarily unavailable",
        )),
    }
}

async fn read_and_parse(state: &AppState, request: Request) -> Result<ParsedRequest, ApiError> {
    ensure_json(request.headers())?;
    let bytes = to_bytes(request.into_body(), state.config.request_limits.body_bytes)
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_too_large",
                "request body exceeds 64 KiB",
            )
        })?;
    parse_request(
        &bytes,
        &state.config.payment_identifier,
        state.config.accept_v1,
    )
    .map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "malformed_request",
            "request body is not a canonical x402 request",
        )
    })
}

fn ensure_json(headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(content_type) = headers.get(CONTENT_TYPE) else {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Content-Type must be application/json",
        ));
    };
    let Ok(content_type) = content_type.to_str() else {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Content-Type must be application/json",
        ));
    };
    if content_type
        .split(';')
        .next()
        .is_none_or(|value| !value.trim().eq_ignore_ascii_case("application/json"))
    {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Content-Type must be application/json",
        ));
    }
    Ok(())
}

fn static_verify_failure(state: &AppState, request: &ParsedRequest) -> Option<VerifyResponse> {
    static_failure_reason(state, request).map(|reason| VerifyResponse::invalid(reason, None, None))
}

fn static_settle_failure(state: &AppState, request: &ParsedRequest) -> Option<SettleResponse> {
    static_failure_reason(state, request).map(|reason| {
        SettleResponse::failure(
            reason,
            None,
            None,
            String::new(),
            state.config.network.clone(),
        )
    })
}

fn static_failure_reason(state: &AppState, request: &ParsedRequest) -> Option<&'static str> {
    if request.meta.x402_version != 2 {
        Some("invalid_x402_version")
    } else if request.meta.scheme != "exact" {
        Some("unsupported_scheme")
    } else if request.meta.network != state.config.network {
        Some("invalid_network")
    } else if !configured_asset_matches(
        state.config.chain_kind,
        &state.config.asset,
        &request.meta.asset,
    ) {
        Some("invalid_asset")
    } else if !decimal_is_at_least(&request.meta.amount, &state.config.minimum_amount) {
        Some("amount_below_minimum")
    } else {
        None
    }
}

fn configured_asset_matches(chain_kind: ChainKind, configured: &str, requested: &str) -> bool {
    match chain_kind {
        ChainKind::Near => requested == configured,
        ChainKind::Eip155 => requested.eq_ignore_ascii_case(configured),
    }
}

fn settle_from_verify_failure(value: &Value, network: &str) -> Response {
    let reason = value
        .get("invalidReason")
        .and_then(Value::as_str)
        .unwrap_or("invalid_payment");
    let payer = value
        .get("payer")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    protocol_json(
        StatusCode::OK,
        &SettleResponse::failure(reason, None, payer, String::new(), network.to_owned()),
    )
}

fn response_is_rpc_ambiguous(value: &Value) -> bool {
    value
        .get("invalidReason")
        .and_then(Value::as_str)
        .is_some_and(|reason| {
            matches!(
                reason,
                "invalid_exact_near_current_block_height_unavailable"
                    | "invalid_exact_near_access_key_lookup_failed"
                    | "invalid_exact_near_account_lookup_failed"
                    | "invalid_exact_near_token_account_lookup_failed"
                    | "invalid_exact_near_balance_check_failed"
                    | "invalid_exact_near_storage_check_failed"
            )
        })
}

fn protocol_json<T: Serialize>(status: StatusCode, body: &T) -> Response {
    match serde_json::to_vec(body) {
        Ok(bytes) => raw_json(status, bytes),
        Err(_) => ApiError::unavailable(
            "response_serialization_failed",
            "response serialization failed",
        )
        .into_response(),
    }
}

fn raw_json(status: StatusCode, bytes: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

fn stored_terminal_response(record: &SettlementRecord) -> Response {
    let status = record
        .terminal_http_status
        .and_then(|status| StatusCode::from_u16(status).ok())
        .unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
    let bytes = record.terminal_response_bytes.clone().unwrap_or_else(|| {
        service_error_bytes(
            "journal_incomplete",
            "terminal settlement response is unavailable",
        )
    });
    let mut response = raw_json(status, bytes);
    if status == StatusCode::SERVICE_UNAVAILABLE {
        response
            .headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from_static(RETRY_SECONDS));
    }
    response
}

fn completed_settlement_response(record: &SettlementRecord) -> Response {
    if record.state.is_terminal() {
        return stored_terminal_response(record);
    }
    ApiError::unavailable(
        "settlement_retryable",
        "settlement can be retried with the same request",
    )
    .into_response()
}

async fn wait_for_terminal(store: &PgStore, id: Uuid) -> Result<SettlementRecord, StoreError> {
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        let record = store
            .settlement(id)
            .await?
            .ok_or_else(|| StoreError::Corrupt("settlement disappeared".to_owned()))?;
        if record.state.is_terminal() || record.state == SettlementState::AwaitingRetry {
            return Ok(record);
        }
    }
}

fn spawn_settlement_worker(
    state: &AppState,
    settlement_id: Uuid,
    request: x402_types::proto::VerifyRequest,
) {
    let worker_state = state.clone();
    let worker_span = tracing::info_span!(
        "settlement_worker",
        network = %state.config.network,
        version = VERSION
    );
    tokio::spawn(
        async move {
            run_new_settlement(worker_state, settlement_id, request).await;
        }
        .instrument(worker_span),
    );
}

// Settlement deliberately remains one sequence so the revalidation, journal,
// and broadcast ordering can be reviewed without cross-function gaps.
#[allow(clippy::too_many_lines)]
async fn run_new_settlement(
    state: AppState,
    settlement_id: Uuid,
    request: x402_types::proto::VerifyRequest,
) {
    let _relayer_guard = state.relayer_lock.lock().await;
    if !state.readiness.can_settle() {
        retryable_service_failure(
            &state,
            settlement_id,
            "leadership_unavailable",
            "settlement leadership was lost before transaction preparation",
        )
        .await;
        return;
    }
    let policy = VerificationPolicy {
        max_sponsored_gas: state.config.max_inner_gas,
    };
    let payment = match state.provider.verify(&request, &policy).await {
        Ok(payment) => payment,
        Err(rejection) if rejection.rpc_ambiguous => {
            retryable_service_failure(
                &state,
                settlement_id,
                "rpc_unavailable",
                "verification was unavailable before transaction preparation",
            )
            .await;
            return;
        }
        Err(rejection) => {
            terminal_protocol_failure(&state, settlement_id, &rejection.reason, None, None).await;
            return;
        }
    };
    let Ok(Some(journaled)) = state.store.settlement(settlement_id).await else {
        retryable_service_failure(
            &state,
            settlement_id,
            "settlement_journal_unavailable",
            "the claimed settlement could not be reloaded",
        )
        .await;
        return;
    };
    let identity = payment.identity();
    if journaled.state != SettlementState::Reserved
        || journaled.payment_hash != identity.request_hash
        || journaled.anchor_scope != identity.anchor_scope
        || journaled.anchor_value != identity.anchor_value
        || journaled.payer != payment.payer
        || journaled.network != payment.requirements.network
        || !journaled
            .asset
            .eq_ignore_ascii_case(&payment.requirements.asset)
        || !journaled
            .pay_to
            .eq_ignore_ascii_case(&payment.requirements.pay_to)
        || !decimal_is_at_least(&journaled.amount, &payment.requirements.amount_decimal)
        || !decimal_is_at_least(&payment.requirements.amount_decimal, &journaled.amount)
    {
        terminal_protocol_failure(
            &state,
            settlement_id,
            "settlement_identity_mismatch",
            None,
            None,
        )
        .await;
        return;
    }
    let Ok(signer_head) = fresh_signer_head(&state).await else {
        retryable_service_failure(
            &state,
            settlement_id,
            "relayer_unavailable",
            "relayer policy, balance, or chain state is unavailable",
        )
        .await;
        return;
    };
    let Ok(prepared) = state.provider.prepare(&payment, &signer_head).await else {
        retryable_service_failure(
            &state,
            settlement_id,
            "transaction_preparation_failed",
            "outer transaction could not be prepared",
        )
        .await;
        return;
    };
    let submission = state.provider.durable_submission(&prepared);
    // EVM settles on its own durable path: journal the signed ERC-3009 transaction
    // into the dedicated eip155 columns, then submit. NEAR's access-key nonce
    // recheck and quarantine do not apply — an EVM re-submit is idempotent via the
    // single-use authorization nonce, and reorg safety comes from confirmation
    // depth at reconcile. The NEAR body below is unchanged.
    if let PreparedDetail::Evm(evm_prepared) = &prepared.detail {
        let Some(estimated_l1_fee) = evm_prepared.estimated_l1_fee_wei() else {
            retryable_service_failure(
                &state,
                settlement_id,
                "l1_fee_estimation_failed",
                "Base L1 data-fee estimation was unavailable",
            )
            .await;
            return;
        };
        if !evm_reservation_covers_liability(&state.config, estimated_l1_fee) {
            retryable_service_failure(
                &state,
                settlement_id,
                "sponsorship_reservation_insufficient",
                "the current Base transaction liability exceeds the reservation",
            )
            .await;
            return;
        }
        settle_prepared_evm(&state, settlement_id, &prepared, &submission, &payment).await;
        return;
    }
    let journal = PreparedJournalEntry {
        settlement_id,
        relayer_account_id: submission.submitter,
        relayer_public_key: prepared.signer_public_key.clone(),
        relayer_nonce: submission.nonce.to_string(),
        transaction_bytes: submission.bytes,
        transaction_hash: submission.hash,
    };
    if state.store.mark_prepared(&journal).await.is_err() {
        state.readiness.set_reconciliation(false);
        tracing::error!(event = "settlement_prepare_journal_failed");
        return;
    }

    let Ok(current_head) = fresh_signer_head(&state).await else {
        state.readiness.set_reconciliation(false);
        tracing::warn!(event = "settlement_paused_after_relayer_recheck");
        return;
    };
    if current_head.signer_nonce != signer_head.signer_nonce {
        let public_key = state.provider.signer_public_key();
        let signer_account_id = state.provider.signer_account_id();
        let _quarantine = state
            .store
            .quarantine_relayer(
                &state.config.network,
                &signer_account_id,
                &public_key,
                "relayer nonce changed between preparation and broadcast",
                &current_head.signer_nonce.to_string(),
            )
            .await;
        state.readiness.set_relayer(false);
        state.readiness.set_reconciliation(false);
        tracing::error!(event = "settlement_relayer_nonce_changed_before_broadcast");
        return;
    }

    // A prepared transaction is durable from this point.  Any leadership loss
    // leaves it for reconciliation; it must never be replaced with new bytes.
    if !state.readiness.can_settle() {
        state.readiness.set_reconciliation(false);
        tracing::warn!(event = "settlement_paused_after_prepare");
        return;
    }
    if state.store.mark_submitted(settlement_id).await.is_err() {
        state.readiness.set_reconciliation(false);
        tracing::error!(event = "settlement_submit_journal_failed");
        return;
    }
    // Recheck immediately before the external side effect, after the durable
    // state transition.  This is deliberately adjacent to broadcast.
    if !state.readiness.can_settle() {
        state.readiness.set_reconciliation(false);
        tracing::warn!(event = "settlement_paused_before_broadcast");
        return;
    }
    match state.provider.broadcast(&prepared, &payment).await {
        BroadcastOutcome::Terminal(outcome) => {
            finalize_terminal(&state, settlement_id, &payment, outcome).await;
        }
        BroadcastOutcome::Rejected(_) => {
            terminal_transaction_rejected(
                &state,
                settlement_id,
                Some(payment.payer.clone()),
                prepared.submit_hash.clone(),
            )
            .await;
        }
        BroadcastOutcome::Pending => {
            // Indeterminate: exact bytes/hash stay submitted for reconciliation.
            state.readiness.set_reconciliation(false);
            tracing::warn!(event = "settlement_broadcast_indeterminate");
        }
    }
}

// The EVM forward settlement tail: journal the signed transaction into the
// dedicated eip155 columns, mark it submitted, and broadcast. An EVM broadcast is
// never terminal in one shot — confirmation depth resolves it at reconcile — so
// this leaves the row `submitted` for reconciliation. Leadership is re-checked
// immediately before the durable transition and again immediately before the
// external broadcast, mirroring the NEAR path's fencing. The EVM pending account
// nonce is read from both RPCs immediately before broadcast; any drift quarantines
// the signer instead of risking a different transaction identity.
async fn settle_prepared_evm(
    state: &AppState,
    settlement_id: Uuid,
    prepared: &Prepared,
    submission: &crate::chain::DurableSubmission,
    payment: &VerifiedPayment,
) {
    let RecoveryPolicy::EvmConfirmations(required_confirmations) = submission.recovery_policy
    else {
        // Only an EVM provider reaches this path; a missing depth is a wiring
        // fault. Stay unready rather than journal an unconfirmable submission.
        state.readiness.set_reconciliation(false);
        tracing::error!(event = "evm_required_confirmations_missing");
        return;
    };
    let Ok(required_confirmations) = i32::try_from(required_confirmations) else {
        state.readiness.set_reconciliation(false);
        tracing::error!(event = "evm_required_confirmations_out_of_range");
        return;
    };
    let journal = EvmPreparedJournalEntry {
        settlement_id,
        signer_account_nonce: submission.nonce.to_string(),
        submitted_tx_rlp: submission.bytes.clone(),
        submitted_tx_hash: submission.hash.clone(),
        required_confirmations,
    };
    if state.store.mark_prepared_evm(&journal).await.is_err() {
        state.readiness.set_reconciliation(false);
        tracing::error!(event = "settlement_prepare_journal_failed");
        return;
    }
    // A prepared transaction is durable from this point; any leadership loss leaves
    // it for reconciliation and it must never be re-signed.
    if !state.readiness.can_settle() {
        state.readiness.set_reconciliation(false);
        tracing::warn!(event = "settlement_paused_after_prepare");
        return;
    }
    if state.store.mark_submitted(settlement_id).await.is_err() {
        state.readiness.set_reconciliation(false);
        tracing::error!(event = "settlement_submit_journal_failed");
        return;
    }
    // Recheck immediately before the external side effect, after the durable state
    // transition — deliberately adjacent to broadcast.
    if !state.readiness.can_settle() {
        state.readiness.set_reconciliation(false);
        tracing::warn!(event = "settlement_paused_before_broadcast");
        return;
    }
    let Ok(current_head) = fresh_signer_head(state).await else {
        state.readiness.set_reconciliation(false);
        tracing::warn!(event = "settlement_paused_after_signer_nonce_recheck");
        return;
    };
    if current_head.signer_nonce != submission.nonce {
        let signer_account_id = state.provider.signer_account_id();
        let _quarantine = state
            .store
            .quarantine_relayer(
                &state.config.network,
                &signer_account_id,
                &state.provider.signer_public_key(),
                "signer nonce changed between preparation and broadcast",
                &current_head.signer_nonce.to_string(),
            )
            .await;
        state.readiness.set_relayer(false);
        state.readiness.set_reconciliation(false);
        tracing::error!(event = "settlement_signer_nonce_changed_before_broadcast");
        return;
    }
    if !state.readiness.can_settle() {
        state.readiness.set_reconciliation(false);
        tracing::warn!(event = "settlement_paused_after_signer_nonce_recheck");
        return;
    }
    match state.provider.broadcast(prepared, payment).await {
        // EVM terminality is accepted only through recovery, where the stored
        // (possibly raised) confirmation depth and dual-reader merge are applied.
        BroadcastOutcome::Terminal(_) => {
            state.readiness.set_reconciliation(false);
            tracing::warn!(event = "evm_broadcast_terminal_deferred_to_reconciliation");
        }
        BroadcastOutcome::Rejected(_) => {
            terminal_transaction_rejected(
                state,
                settlement_id,
                Some(payment.payer.clone()),
                prepared.submit_hash.clone(),
            )
            .await;
        }
        BroadcastOutcome::Pending => {
            // Indeterminate by design: the exact bytes/hash stay submitted for
            // confirmation-depth reconciliation.
            state.readiness.set_reconciliation(false);
            tracing::warn!(event = "settlement_broadcast_indeterminate");
        }
    }
}

fn evm_reservation_covers_liability(config: &ServiceConfig, estimated_l1_fee: u128) -> bool {
    let Some(eip155) = &config.eip155 else {
        return false;
    };
    let Ok(max_fee_per_gas) = eip155.max_fee_per_gas_wei.parse::<u128>() else {
        return false;
    };
    let Ok(reservation) = config.sponsorship.reservation_yocto_near.parse::<u128>() else {
        return false;
    };
    u128::from(eip155.gas_limit)
        .checked_mul(max_fee_per_gas)
        .and_then(|l2| l2.checked_add(estimated_l1_fee))
        .is_some_and(|liability| reservation >= liability)
}

async fn fresh_signer_head(state: &AppState) -> Result<SignerHead, StoreError> {
    let head = state.provider.signer_head().await.map_err(|_| {
        state.readiness.set_relayer(false);
        StoreError::Corrupt("relayer chain state is unavailable".to_owned())
    })?;
    let signer_account_id = state.provider.signer_account_id();
    let policy_active = state
        .store
        .relayer_is_active(
            &state.config.network,
            &signer_account_id,
            &head.signer_public_key,
        )
        .await?;
    let funded = signer_is_funded(&state.config, head.signer_balance_atomic);
    if !policy_active || !funded {
        state.readiness.set_relayer(false);
        return Err(StoreError::Corrupt(
            "relayer policy or balance hard stop is not satisfied".to_owned(),
        ));
    }
    state.readiness.set_relayer(true);
    Ok(head)
}

async fn finalize_terminal(
    state: &AppState,
    settlement_id: Uuid,
    payment: &VerifiedPayment,
    outcome: TerminalOutcome,
) {
    let (terminal_state, response, error_code) = if outcome.success {
        (
            SettlementState::Succeeded,
            SettleResponse::success(
                payment.payer.clone(),
                outcome.tx_hash.clone(),
                state.config.network.clone(),
            ),
            None,
        )
    } else {
        (
            SettlementState::Failed,
            SettleResponse::failure(
                "transaction_failed",
                outcome.failure_detail.clone(),
                Some(payment.payer.clone()),
                outcome.tx_hash.clone(),
                state.config.network.clone(),
            ),
            Some("transaction_failed".to_owned()),
        )
    };
    let (metric_result, metric_reason) = if outcome.success {
        ("succeeded", "success")
    } else {
        ("failed", "transaction_failed")
    };
    let Ok(bytes) = serde_json::to_vec(&response) else {
        tracing::error!(event = "terminal_response_serialization_failed");
        return;
    };
    let entry = TerminalJournalEntry {
        settlement_id,
        state: terminal_state,
        http_status: StatusCode::OK.as_u16(),
        response_bytes: bytes,
        error_code,
        error_detail: None,
        gas_burnt: Some(outcome.gas_units.to_string()),
        tokens_burnt: Some(outcome.fee_atomic.to_string()),
        actual_yocto_near: outcome.fee_atomic.to_string(),
        mined_block_number: outcome.mined_block_number.map(|number| number.to_string()),
        mined_block_hash: outcome.mined_block_hash.clone(),
        confirmations: outcome
            .confirmations
            .and_then(|depth| i32::try_from(depth).ok()),
    };
    if state.store.mark_terminal(&entry).await.is_err() {
        state.readiness.set_reconciliation(false);
        tracing::error!(event = "terminal_journal_failed");
    } else {
        state
            .metrics
            .record_settlement_cost(outcome.gas_units, yocto_near_metric(outcome.fee_atomic));
        state
            .metrics
            .record_settlement_result(metric_result, metric_reason);
        tracing::info!(
            event = "settlement_terminal",
            result = metric_result,
            reason = metric_reason
        );
    }
}

fn yocto_near_metric(value: u128) -> f64 {
    value.to_string().parse::<f64>().unwrap_or(f64::MAX)
}

async fn terminal_protocol_failure(
    state: &AppState,
    settlement_id: Uuid,
    reason: &str,
    payer: Option<String>,
    transaction: Option<String>,
) {
    let response = SettleResponse::failure(
        reason,
        None,
        payer,
        transaction.unwrap_or_default(),
        state.config.network.clone(),
    );
    let Ok(bytes) = serde_json::to_vec(&response) else {
        return;
    };
    let result = state
        .store
        .mark_terminal(&TerminalJournalEntry {
            settlement_id,
            state: SettlementState::Failed,
            http_status: StatusCode::OK.as_u16(),
            response_bytes: bytes,
            error_code: Some(reason.to_owned()),
            error_detail: None,
            gas_burnt: Some("0".to_owned()),
            tokens_burnt: Some("0".to_owned()),
            actual_yocto_near: "0".to_owned(),
            mined_block_number: None,
            mined_block_hash: None,
            confirmations: None,
        })
        .await;
    if result.is_ok() {
        state.metrics.record_settlement_result("failed", reason);
        tracing::info!(event = "settlement_terminal", result = "failed", reason);
    } else {
        state.readiness.set_reconciliation(false);
        tracing::error!(event = "terminal_journal_failed");
    }
}

async fn retryable_service_failure(
    state: &AppState,
    settlement_id: Uuid,
    code: &'static str,
    _message: &'static str,
) {
    let result = state.store.mark_awaiting_retry(settlement_id, code).await;
    if result.is_ok() {
        state.metrics.record_settlement_result("retryable", code);
        tracing::info!(
            event = "settlement_retry_released",
            result = "retryable",
            reason = code
        );
    } else {
        state.readiness.set_reconciliation(false);
        tracing::error!(event = "settlement_retry_journal_failed");
    }
}

async fn terminal_transaction_rejected(
    state: &AppState,
    settlement_id: Uuid,
    payer: Option<String>,
    transaction_hash: String,
) {
    let response = SettleResponse::failure(
        "transaction_rejected",
        None,
        payer,
        transaction_hash,
        state.config.network.clone(),
    );
    let Ok(response_bytes) = serde_json::to_vec(&response) else {
        state.readiness.set_reconciliation(false);
        return;
    };
    let result = state
        .store
        .mark_terminal(&TerminalJournalEntry {
            settlement_id,
            state: SettlementState::Failed,
            http_status: StatusCode::OK.as_u16(),
            response_bytes,
            error_code: Some("transaction_rejected".to_owned()),
            error_detail: None,
            gas_burnt: Some("0".to_owned()),
            tokens_burnt: Some("0".to_owned()),
            actual_yocto_near: "0".to_owned(),
            mined_block_number: None,
            mined_block_hash: None,
            confirmations: None,
        })
        .await;
    if result.is_ok() {
        state
            .metrics
            .record_settlement_result("failed", "transaction_rejected");
        tracing::info!(
            event = "settlement_terminal",
            result = "failed",
            reason = "transaction_rejected"
        );
    } else {
        state.readiness.set_reconciliation(false);
        tracing::error!(event = "terminal_journal_failed");
    }
}

fn service_error_bytes(code: &str, message: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "error": {
            "code": code,
            "message": message,
        }
    }))
    .unwrap_or_else(|_| {
        b"{\"error\":{\"code\":\"internal_error\",\"message\":\"internal error\"}}".to_vec()
    })
}

/// Reconcile nonterminal rows after leadership acquisition.  This function
/// never signs replacement bytes: prepared/submitted rows are queried and, if
/// safe, rebroadcast using only the exact journaled transaction.
pub async fn reconcile(state: &AppState) -> Result<(), StoreError> {
    if !state.readiness.snapshot().leadership {
        return Err(StoreError::Corrupt(
            "reconciliation requires leadership".to_owned(),
        ));
    }
    state.readiness.set_reconciliation(false);
    let _relayer_guard = state.relayer_lock.lock().await;
    let records = state.store.nonterminal_settlements().await?;
    state
        .metrics
        .record_pending_settlements(u64::try_from(records.len()).unwrap_or(u64::MAX));

    // Release every pre-prepare reservation before ordered transaction
    // reconciliation. A still-pending prepared transaction must stop later
    // nonce processing, but it must not strand unrelated Reserved rows (and
    // their sponsorship budget) behind it.
    for record in records
        .iter()
        .filter(|record| record.state == SettlementState::Reserved)
    {
        state.store.note_reconciliation(record.id).await?;
        // The service is not ready while startup reconciliation runs, therefore
        // every observed reserved row predates this leader. With no prepared
        // bytes there can have been no broadcast.
        retryable_service_failure(
            state,
            record.id,
            "recovered_before_prepare",
            "an interrupted settlement was released before transaction preparation",
        )
        .await;
    }

    for record in records {
        match record.state {
            SettlementState::Prepared | SettlementState::Submitted => {
                state.store.note_reconciliation(record.id).await?;
                reconcile_prepared(state, &record).await?;
                let remains_nonterminal = state
                    .store
                    .settlement(record.id)
                    .await?
                    .is_some_and(|current| !current.state.is_terminal());
                if remains_nonterminal {
                    break;
                }
            }
            SettlementState::AwaitingRetry => {
                state.store.note_reconciliation(record.id).await?;
            }
            SettlementState::Reserved | SettlementState::Succeeded | SettlementState::Failed => {}
        }
    }
    let remaining = state.store.nonterminal_settlements().await?;
    state
        .metrics
        .record_pending_settlements(u64::try_from(remaining.len()).unwrap_or(u64::MAX));
    state.readiness.set_reconciliation(remaining.is_empty());
    Ok(())
}

// Recovery keeps every exact-byte/hash and dual-RPC decision adjacent.
#[allow(clippy::too_many_lines)]
async fn reconcile_prepared(state: &AppState, record: &SettlementRecord) -> Result<(), StoreError> {
    // EVM settlements reconcile on a different path — signer/RLP validation on the
    // dedicated eip155 columns, confirmation depth, and rebroadcast-on-unknown,
    // with no NEAR nonce-quarantine or delegate-expiry machinery. Take it before
    // the NEAR relayer-identity guard below, which reads NEAR-only columns that are
    // NULL on an EVM row. The NEAR path below is unchanged.
    if matches!(&*state.provider, ChainProvider::Evm(_)) {
        return reconcile_prepared_evm(state, record).await;
    }
    let expected_account = state.provider.signer_account_id();
    let expected_public_key = state.provider.signer_public_key();
    if record.relayer_account_id.as_deref() != Some(expected_account.as_str())
        || record.relayer_public_key.as_deref() != Some(expected_public_key.as_str())
    {
        state.readiness.set_relayer(false);
        return Err(StoreError::Corrupt(
            "journaled relayer identity does not match configured relayer".to_owned(),
        ));
    }
    let hash = record
        .outer_transaction_hash
        .as_deref()
        .ok_or_else(|| StoreError::Corrupt("prepared row has no transaction hash".to_owned()))?
        .parse::<CryptoHash>()
        .map_err(|_| StoreError::Corrupt("prepared row has invalid transaction hash".to_owned()))?;
    // Validate the exact persisted bytes before trusting *any* RPC result for
    // this journal row, including an already-final transaction.
    let bytes = record
        .outer_transaction_bytes
        .as_deref()
        .ok_or_else(|| StoreError::Corrupt("prepared row has no transaction bytes".to_owned()))?;
    let signer = record
        .relayer_account_id
        .as_deref()
        .ok_or_else(|| StoreError::Corrupt("prepared row has no signer".to_owned()))?
        .parse::<AccountId>()
        .map_err(|_| StoreError::Corrupt("prepared row has invalid signer".to_owned()))?;
    validate_stored_transaction(record, bytes, hash, &signer).inspect_err(|_| {
        state.readiness.set_relayer(false);
    })?;
    // The provider performs the dual-RPC query, raw-outcome conflict check, and
    // receipt interpretation, returning a neutral verdict.
    let status = state
        .provider
        .reconcile_status(
            &hash.to_string(),
            signer.as_str(),
            &record.payer,
            &record.asset,
            RecoveryPolicy::NearFinality,
        )
        .await;
    if status.rpc_failover {
        state.metrics.record_rpc_failover("reconcile_transaction");
    }
    match status.verdict {
        ReconcileVerdict::Conflict => {
            state.readiness.set_reconciliation(false);
            return Err(StoreError::Corrupt(
                "primary and backup RPCs returned conflicting final outcomes".to_owned(),
            ));
        }
        ReconcileVerdict::Terminal(outcome) => {
            finalize_reconciled_terminal(state, record, outcome).await?;
            return Ok(());
        }
        ReconcileVerdict::Indeterminate(reason) => {
            state.readiness.set_reconciliation(false);
            tracing::warn!(event = "reconciliation_outcome_indeterminate", reason = %reason);
            return Ok(());
        }
        ReconcileVerdict::Pending => return Ok(()),
        ReconcileVerdict::Unknown => {}
        ReconcileVerdict::Ambiguous => {
            state.readiness.set_reconciliation(false);
            return Err(StoreError::Corrupt(
                "RPC ambiguity prevented settlement reconciliation".to_owned(),
            ));
        }
    }

    let primary_status = fresh_signer_head(state).await?;
    let backup_head = state.provider.backup_signer_head().await.map_err(|_| {
        StoreError::Corrupt("backup relayer state unavailable during reconciliation".to_owned())
    })?;
    let prepared_nonce = record
        .relayer_nonce
        .as_deref()
        .and_then(|nonce| nonce.parse::<u128>().ok())
        .ok_or_else(|| StoreError::Corrupt("prepared row has invalid nonce".to_owned()))?;
    if primary_status.signer_nonce >= prepared_nonce || backup_head.signer_nonce >= prepared_nonce {
        let public_key = record
            .relayer_public_key
            .as_deref()
            .ok_or_else(|| StoreError::Corrupt("prepared row has no public key".to_owned()))?;
        let signer_account_id = state.provider.signer_account_id();
        state
            .store
            .quarantine_relayer(
                &state.config.network,
                &signer_account_id,
                public_key,
                "nonce advanced while exact transaction remained unknown",
                &primary_status
                    .signer_nonce
                    .max(backup_head.signer_nonce)
                    .to_string(),
            )
            .await?;
        state.readiness.set_relayer(false);
        return Err(StoreError::Corrupt(
            "relayer key quarantined after unknown nonce advance".to_owned(),
        ));
    }
    let delegate_max_height = record
        .delegate_max_block_height
        .parse::<u64>()
        .map_err(|_| StoreError::Corrupt("journal delegate expiry is invalid".to_owned()))?;
    if primary_status
        .chain_block_height
        .max(backup_head.chain_block_height)
        >= delegate_max_height
    {
        terminal_protocol_failure(
            state,
            record.id,
            VerificationFailure::DelegateActionExpired.reason(),
            Some(record.payer.clone()),
            (record.state == SettlementState::Submitted).then(|| hash.to_string()),
        )
        .await;
        return Ok(());
    }
    if record.state == SettlementState::Prepared {
        state.store.mark_submitted(record.id).await?;
    }
    // Leadership is rechecked immediately before the rebroadcast side effect.
    if !can_reconciliation_broadcast(state) {
        return Err(StoreError::Corrupt(
            "leadership lost before reconciliation broadcast".to_owned(),
        ));
    }
    let current_primary = fresh_signer_head(state).await?;
    let current_backup = state.provider.backup_signer_head().await.map_err(|_| {
        StoreError::Corrupt("backup relayer state unavailable before rebroadcast".to_owned())
    })?;
    if current_primary.signer_nonce != primary_status.signer_nonce
        || current_backup.signer_nonce != backup_head.signer_nonce
    {
        state.readiness.set_relayer(false);
        return Err(StoreError::Corrupt(
            "relayer nonce changed before exact-byte rebroadcast".to_owned(),
        ));
    }
    match state
        .provider
        .rebroadcast(bytes, &hash.to_string(), &record.payer, &record.asset)
        .await
    {
        BroadcastOutcome::Terminal(outcome) => {
            finalize_reconciled_terminal(state, record, outcome).await?;
        }
        BroadcastOutcome::Rejected(_) => {
            terminal_transaction_rejected(
                state,
                record.id,
                Some(record.payer.clone()),
                hash.to_string(),
            )
            .await;
        }
        // Still in flight (or an indeterminate final): stay submitted; the outer
        // reconcile loop recomputes readiness from the remaining nonterminal set.
        BroadcastOutcome::Pending => {}
    }
    Ok(())
}

// EVM reconciliation: validate the journaled RLP bytes, then resolve by
// confirmation depth. No NEAR nonce-quarantine or delegate-expiry — an EVM
// outcome is terminal only at the required confirmation depth, and an unknown
// transaction is re-submitted (idempotent via the single-use ERC-3009 nonce).
#[allow(clippy::too_many_lines)]
async fn reconcile_prepared_evm(
    state: &AppState,
    record: &SettlementRecord,
) -> Result<(), StoreError> {
    // The EVM submission identity lives in the dedicated eip155 columns
    // (signer_address / submitted_tx_*), not the NEAR relayer / outer-transaction
    // columns. Guard the journaled signer against the configured signer first.
    let expected_signer = state.provider.signer_account_id();
    let signer = record
        .signer_address
        .as_deref()
        .ok_or_else(|| StoreError::Corrupt("prepared row has no signer".to_owned()))?;
    if signer != expected_signer {
        state.readiness.set_relayer(false);
        return Err(StoreError::Corrupt(
            "journaled signer does not match configured signer".to_owned(),
        ));
    }
    let hash = record
        .submitted_tx_hash
        .as_deref()
        .ok_or_else(|| StoreError::Corrupt("prepared row has no transaction hash".to_owned()))?;
    let bytes = record
        .submitted_tx_rlp
        .as_deref()
        .ok_or_else(|| StoreError::Corrupt("prepared row has no transaction bytes".to_owned()))?;
    let configured_confirmations = state
        .provider
        .required_confirmations()
        .and_then(|depth| i32::try_from(depth).ok())
        .ok_or_else(|| {
            StoreError::Corrupt("configured EVM confirmation depth is invalid".to_owned())
        })?;
    let effective_confirmations = state
        .store
        .raise_required_confirmations(record.id, configured_confirmations)
        .await?;
    let required_confirmations = u64::try_from(effective_confirmations).map_err(|_| {
        StoreError::Corrupt("journaled EVM confirmation depth is invalid".to_owned())
    })?;
    let metadata = record.authorization_metadata.as_ref().ok_or_else(|| {
        StoreError::Corrupt("prepared EVM row has no authorization metadata".to_owned())
    })?;
    if metadata.version != 2 {
        return Err(StoreError::Corrupt(
            "prepared EVM authorization metadata version is invalid".to_owned(),
        ));
    }
    let (gas_limit, max_fee_per_gas, admission_confirmations) = evm_policy_snapshot(record)?;
    if required_confirmations < admission_confirmations {
        return Err(StoreError::Corrupt(
            "journaled EVM confirmation policy was lowered".to_owned(),
        ));
    }
    let signer_nonce = record
        .signer_account_nonce
        .as_deref()
        .and_then(|nonce| nonce.parse::<u128>().ok())
        .ok_or_else(|| StoreError::Corrupt("prepared EVM row has invalid nonce".to_owned()))?;
    let binding = StoredEvmSubmission {
        network: record.network.clone(),
        hash: hash.to_owned(),
        submitter: signer.to_owned(),
        nonce: signer_nonce,
        asset: record.asset.clone(),
        payer: record.payer.clone(),
        payee: record.pay_to.clone(),
        amount: record.amount.clone(),
        valid_after: metadata.valid_after.clone(),
        valid_before: metadata.valid_before.clone(),
        anchor_scope: record.anchor_scope.clone(),
        anchor_value: record.anchor_value,
        payment_hash: record.payment_hash,
        gas_limit,
        max_fee_per_gas,
    };
    // Validate every field of the exact persisted envelope and both supported
    // ERC-3009 calldata forms before trusting any RPC evidence.
    state
        .provider
        .validate_stored_submission(bytes, &binding)
        .map_err(|error| StoreError::Corrupt(error.to_string()))
        .inspect_err(|_| state.readiness.set_relayer(false))?;
    let status = state
        .provider
        .reconcile_status(
            hash,
            signer,
            &record.payer,
            &record.asset,
            RecoveryPolicy::EvmConfirmations(required_confirmations),
        )
        .await;
    match status.verdict {
        ReconcileVerdict::Terminal(outcome) => {
            finalize_reconciled_terminal(state, record, outcome).await?;
            Ok(())
        }
        // Mined below the confirmation depth, or still in the mempool: wait.
        ReconcileVerdict::Pending => Ok(()),
        ReconcileVerdict::Indeterminate(reason) => {
            state.readiness.set_reconciliation(false);
            tracing::warn!(event = "reconciliation_outcome_indeterminate", reason = %reason);
            Ok(())
        }
        ReconcileVerdict::Unknown => {
            // No receipt: mempool-dropped or reorged out. Re-submit the exact
            // journaled bytes; the single-use ERC-3009 authorization nonce makes a
            // re-submit idempotent, and the confirmation-depth policy guards reorg.
            if record.state == SettlementState::Prepared {
                state.store.mark_submitted(record.id).await?;
            }
            if !can_reconciliation_broadcast(state) {
                return Err(StoreError::Corrupt(
                    "leadership lost before reconciliation broadcast".to_owned(),
                ));
            }
            let current_head = fresh_signer_head(state).await?;
            require_evm_recovery_balance(state, record, current_head.signer_balance_atomic)?;
            if current_head.signer_nonce > signer_nonce {
                let signer_account_id = state.provider.signer_account_id();
                state
                    .store
                    .quarantine_relayer(
                        &state.config.network,
                        &signer_account_id,
                        &state.provider.signer_public_key(),
                        "signer nonce advanced while exact transaction remained unknown",
                        &current_head.signer_nonce.to_string(),
                    )
                    .await?;
                state.readiness.set_relayer(false);
                state.readiness.set_reconciliation(false);
                return Err(StoreError::Corrupt(
                    "EVM signer quarantined after unknown nonce advance".to_owned(),
                ));
            }
            if current_head.signer_nonce != signer_nonce {
                state.readiness.set_reconciliation(false);
                return Err(StoreError::Corrupt(
                    "EVM signer nonce regressed before exact-byte rebroadcast".to_owned(),
                ));
            }
            if !can_reconciliation_broadcast(state) {
                return Err(StoreError::Corrupt(
                    "leadership lost immediately before reconciliation broadcast".to_owned(),
                ));
            }
            match state
                .provider
                .rebroadcast(bytes, hash, &record.payer, &record.asset)
                .await
            {
                BroadcastOutcome::Terminal(outcome) => {
                    finalize_reconciled_terminal(state, record, outcome).await?;
                }
                BroadcastOutcome::Rejected(_) => {
                    terminal_transaction_rejected(
                        state,
                        record.id,
                        Some(record.payer.clone()),
                        hash.to_owned(),
                    )
                    .await;
                }
                BroadcastOutcome::Pending => {}
            }
            Ok(())
        }
        // EVM reconcile never returns Conflict (no dual-RPC); Ambiguous is a
        // malformed hash or an RPC error — stay unready and retry.
        ReconcileVerdict::Conflict | ReconcileVerdict::Ambiguous => {
            state.readiness.set_reconciliation(false);
            Err(StoreError::Corrupt(
                "evm reconciliation was ambiguous".to_owned(),
            ))
        }
    }
}

fn evm_policy_snapshot(record: &SettlementRecord) -> Result<(u64, u128, u64), StoreError> {
    if record
        .policy_snapshot
        .get("chainKind")
        .and_then(Value::as_str)
        != Some("eip155")
        || record
            .policy_snapshot
            .get("network")
            .and_then(Value::as_str)
            != Some(record.network.as_str())
        || !record
            .policy_snapshot
            .get("asset")
            .and_then(Value::as_str)
            .is_some_and(|asset| asset.eq_ignore_ascii_case(&record.asset))
    {
        return Err(StoreError::Corrupt(
            "EVM settlement policy identity is inconsistent".to_owned(),
        ));
    }
    let eip155 = record
        .policy_snapshot
        .get("eip155")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            StoreError::Corrupt("EVM settlement has no stored admission policy".to_owned())
        })?;
    let chain_id = eip155
        .get("chainId")
        .and_then(Value::as_u64)
        .ok_or_else(|| StoreError::Corrupt("stored EVM chain id is invalid".to_owned()))?;
    let network_chain_id = record
        .network
        .strip_prefix("eip155:")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| StoreError::Corrupt("journaled EVM network is invalid".to_owned()))?;
    if chain_id != network_chain_id {
        return Err(StoreError::Corrupt(
            "stored EVM chain policy does not match the network".to_owned(),
        ));
    }
    let gas_limit = eip155
        .get("gasLimit")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| StoreError::Corrupt("stored EVM gas limit is invalid".to_owned()))?;
    let max_fee_per_gas = eip155
        .get("maxFeePerGasWei")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<u128>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| StoreError::Corrupt("stored EVM fee cap is invalid".to_owned()))?;
    let required_confirmations = eip155
        .get("requiredConfirmations")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            StoreError::Corrupt("stored EVM confirmation policy is invalid".to_owned())
        })?;
    Ok((gas_limit, max_fee_per_gas, required_confirmations))
}

fn validate_stored_transaction(
    record: &SettlementRecord,
    bytes: &[u8],
    expected_hash: CryptoHash,
    expected_signer: &AccountId,
) -> Result<(), StoreError> {
    let signed = decode_signed_transaction(bytes)
        .map_err(|_| StoreError::Corrupt("prepared transaction bytes are invalid".to_owned()))?;
    if signed_transaction_hash(bytes).ok() != Some(expected_hash)
        || signed.get_hash() != expected_hash
    {
        return Err(StoreError::Corrupt(
            "prepared transaction bytes do not match journaled hash".to_owned(),
        ));
    }
    if !signed
        .signature
        .verify(signed.get_hash().as_ref(), signed.transaction.public_key())
    {
        return Err(StoreError::Corrupt(
            "prepared outer transaction signature is invalid".to_owned(),
        ));
    }
    let Transaction::V0(transaction) = &signed.transaction else {
        return Err(StoreError::Corrupt(
            "prepared outer transaction is not V0".to_owned(),
        ));
    };
    let expected_public_key = record
        .relayer_public_key
        .as_deref()
        .ok_or_else(|| StoreError::Corrupt("prepared row has no public key".to_owned()))?;
    let expected_nonce = record
        .relayer_nonce
        .as_deref()
        .and_then(|nonce| nonce.parse::<u64>().ok())
        .ok_or_else(|| StoreError::Corrupt("prepared row has invalid nonce".to_owned()))?;
    let payer = record
        .payer
        .parse::<AccountId>()
        .map_err(|_| StoreError::Corrupt("journal payer is invalid".to_owned()))?;
    if transaction.signer_id != *expected_signer
        || transaction.public_key.to_string() != expected_public_key
        || transaction.nonce != expected_nonce
        || transaction.receiver_id != payer
        || transaction.actions.len() != 1
    {
        return Err(StoreError::Corrupt(
            "prepared outer transaction identity does not match the journal".to_owned(),
        ));
    }
    let Some(Action::Delegate(delegate)) = transaction.actions.first() else {
        return Err(StoreError::Corrupt(
            "prepared outer transaction does not contain one delegate action".to_owned(),
        ));
    };
    if !delegate.verify() {
        return Err(StoreError::Corrupt(
            "prepared delegate signature is invalid".to_owned(),
        ));
    }
    let delegate_hash = signed_delegate_hash(delegate)
        .map_err(|_| StoreError::Corrupt("prepared delegate cannot be hashed".to_owned()))?;
    let expected_delegate_nonce = record
        .delegate_nonce
        .parse::<u64>()
        .map_err(|_| StoreError::Corrupt("journal delegate nonce is invalid".to_owned()))?;
    let expected_max_height = record
        .delegate_max_block_height
        .parse::<u64>()
        .map_err(|_| StoreError::Corrupt("journal delegate expiry is invalid".to_owned()))?;
    if delegate_hash != record.payment_hash
        || delegate.delegate_action.sender_id != payer
        || delegate.delegate_action.public_key.to_string() != record.delegate_public_key
        || delegate.delegate_action.nonce != expected_delegate_nonce
        || delegate.delegate_action.max_block_height != expected_max_height
    {
        return Err(StoreError::Corrupt(
            "prepared delegate does not match the settlement journal".to_owned(),
        ));
    }
    Ok(())
}

fn can_reconciliation_broadcast(state: &AppState) -> bool {
    let snapshot = state.readiness.snapshot();
    snapshot.leadership && snapshot.rpc && snapshot.relayer
}

async fn finalize_reconciled_terminal(
    state: &AppState,
    record: &SettlementRecord,
    outcome: TerminalOutcome,
) -> Result<(), StoreError> {
    let (settlement_state, response, error_code) = if outcome.success {
        (
            SettlementState::Succeeded,
            SettleResponse::success(
                record.payer.clone(),
                outcome.tx_hash.clone(),
                state.config.network.clone(),
            ),
            None,
        )
    } else {
        (
            SettlementState::Failed,
            SettleResponse::failure(
                "transaction_failed",
                outcome.failure_detail.clone(),
                Some(record.payer.clone()),
                outcome.tx_hash.clone(),
                state.config.network.clone(),
            ),
            Some("transaction_failed".to_owned()),
        )
    };
    let (metric_result, metric_reason) = if outcome.success {
        ("succeeded", "success")
    } else {
        ("failed", "transaction_failed")
    };
    let bytes =
        serde_json::to_vec(&response).map_err(|error| StoreError::Corrupt(error.to_string()))?;
    let fee_overrun = if record.chain_kind == ChainKind::Eip155 {
        let reservation = record
            .reserved_yocto_near
            .parse::<u128>()
            .map_err(|_| StoreError::Corrupt("EVM reservation is invalid".to_owned()))?;
        outcome.fee_atomic > reservation
    } else {
        false
    };
    if fee_overrun {
        // Quarantine before terminalization releases the EVM signer's active
        // settlement slot. If this write fails the row stays nonterminal and
        // reconciliation remains false, so a later readiness refresh cannot
        // fail open during a terminal→quarantine gap.
        state.readiness.set_relayer(false);
        state.readiness.set_reconciliation(false);
        let signer_account_id = state.provider.signer_account_id();
        state
            .store
            .quarantine_relayer(
                &state.config.network,
                &signer_account_id,
                &state.provider.signer_public_key(),
                "actual Base fee exceeded the sponsorship reservation",
                record.signer_account_nonce.as_deref().unwrap_or("0"),
            )
            .await?;
    }
    state
        .store
        .mark_terminal(&TerminalJournalEntry {
            settlement_id: record.id,
            state: settlement_state,
            http_status: StatusCode::OK.as_u16(),
            response_bytes: bytes,
            error_code,
            error_detail: None,
            gas_burnt: Some(outcome.gas_units.to_string()),
            tokens_burnt: Some(outcome.fee_atomic.to_string()),
            actual_yocto_near: outcome.fee_atomic.to_string(),
            mined_block_number: outcome.mined_block_number.map(|number| number.to_string()),
            mined_block_hash: outcome.mined_block_hash.clone(),
            confirmations: outcome
                .confirmations
                .and_then(|depth| i32::try_from(depth).ok()),
        })
        .await?;
    state
        .metrics
        .record_settlement_cost(outcome.gas_units, yocto_near_metric(outcome.fee_atomic));
    state
        .metrics
        .record_settlement_result(metric_result, metric_reason);
    tracing::info!(
        event = "settlement_terminal",
        result = metric_result,
        reason = metric_reason
    );
    if fee_overrun {
        return Err(StoreError::Corrupt(
            "Base sponsorship quarantined after reservation overrun".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Operation {
    Verify,
    Settle,
}

#[derive(Default)]
struct RateLimiter {
    windows: Mutex<HashMap<(String, Operation), RateWindow>>,
}

struct RateWindow {
    started: Instant,
    count: u32,
}

impl RateLimiter {
    async fn check(&self, key_prefix: &str, operation: Operation, limit: u32) -> bool {
        let mut windows = self.windows.lock().await;
        let now = Instant::now();
        let window = windows
            .entry((key_prefix.to_owned(), operation))
            .or_insert(RateWindow {
                started: now,
                count: 0,
            });
        if now.duration_since(window.started) >= Duration::from_secs(60) {
            window.started = now;
            window.count = 0;
        }
        if window.count >= limit {
            return false;
        }
        window.count = window.count.saturating_add(1);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use tower::ServiceExt as _;
    use tower_http::trace::DefaultMakeSpan;
    use tracing_subscriber::fmt::{MakeWriter, format::FmtSpan};

    use super::*;

    #[derive(Clone)]
    struct CaptureWriter(Arc<StdMutex<Vec<u8>>>);

    struct CaptureGuard(Arc<StdMutex<Vec<u8>>>);

    impl std::io::Write for CaptureGuard {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("capture lock poisoned"))?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> MakeWriter<'writer> for CaptureWriter {
        type Writer = CaptureGuard;

        fn make_writer(&'writer self) -> Self::Writer {
            CaptureGuard(Arc::clone(&self.0))
        }
    }

    #[test]
    fn content_type_is_strict_but_allows_charset() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        assert!(ensure_json(&headers).is_ok());
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        assert!(ensure_json(&headers).is_err());
    }

    #[test]
    fn configured_asset_matching_respects_chain_address_semantics() {
        assert!(configured_asset_matches(
            ChainKind::Eip155,
            "0x036cbd53842c5426634e7929541ec2318f3dcf7e",
            "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
        ));
        assert!(!configured_asset_matches(
            ChainKind::Near,
            "usdc.fakes.testnet",
            "USDC.FAKES.TESTNET",
        ));
    }

    #[tokio::test]
    async fn rate_limiter_enforces_each_operation_separately() {
        let limiter = RateLimiter::default();
        assert!(limiter.check("x402_test_a", Operation::Verify, 1).await);
        assert!(!limiter.check("x402_test_a", Operation::Verify, 1).await);
        assert!(limiter.check("x402_test_b", Operation::Verify, 1).await);
        assert!(limiter.check("x402_test_a", Operation::Settle, 1).await);
    }

    #[tokio::test]
    async fn v1_wire_responses_echo_the_legacy_network_alias() {
        let settle = SettleResponse::failure(
            "payee_not_allowed",
            None,
            None,
            String::new(),
            "eip155:8453".to_owned(),
        );
        let translated =
            finalize_wire_response(protocol_json(StatusCode::OK, &settle), WireVersion::V1).await;
        assert_eq!(translated.status(), StatusCode::OK);
        let bytes = to_bytes(translated.into_body(), PROTOCOL_RESPONSE_REBUFFER_LIMIT)
            .await
            .unwrap_or_default();
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        assert_eq!(value["network"], "base");
        assert_eq!(value["success"], false);
        assert_eq!(value["errorReason"], "payee_not_allowed");

        let untouched =
            finalize_wire_response(protocol_json(StatusCode::OK, &settle), WireVersion::V2).await;
        let bytes = to_bytes(untouched.into_body(), PROTOCOL_RESPONSE_REBUFFER_LIMIT)
            .await
            .unwrap_or_default();
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        assert_eq!(value["network"], "eip155:8453");

        let error = finalize_wire_response(
            ApiError::new(StatusCode::BAD_REQUEST, "malformed_request", "bad").into_response(),
            WireVersion::V1,
        )
        .await;
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn service_errors_have_stable_nested_shape() {
        let value: Value =
            serde_json::from_slice(&service_error_bytes("pending", "retry")).unwrap_or(Value::Null);
        assert_eq!(value["error"]["code"], "pending");
        assert_eq!(value["error"]["message"], "retry");
    }

    #[tokio::test]
    async fn landing_page_identifies_the_service_and_public_next_steps() {
        let response = landing().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/html; charset=utf-8")
        );
        let bytes = to_bytes(response.into_body(), 32 * 1024)
            .await
            .unwrap_or_else(|_| std::process::abort());
        let body = String::from_utf8_lossy(&bytes);
        assert!(body.contains("x402 facilitator for NEAR and Base"));
        assert!(body.contains("href=\"/supported\""));
        assert!(body.contains("docs/reference-access.md"));
        assert!(body.contains("security/policy"));
        assert!(body.contains(VERSION));
    }

    #[test]
    fn evm_support_advertises_payment_identifier_for_both_wire_versions() {
        let supported = evm_supported_for(
            "eip155:8453",
            true,
            "0x1111111111111111111111111111111111111111",
        );
        assert_eq!(supported.extensions, vec!["payment-identifier"]);
        assert_eq!(supported.kinds.len(), 2);
        assert_eq!(supported.kinds[0].network, "eip155:8453");
        assert_eq!(supported.kinds[1].network, "base");
    }

    #[test]
    fn evm_reservation_includes_l2_cap_and_current_l1_estimate() {
        let config = ServiceConfig {
            environment: crate::config::Environment::Testnet,
            chain_kind: ChainKind::Eip155,
            network: "eip155:84532".to_owned(),
            bind_address: "127.0.0.1:0"
                .parse()
                .unwrap_or_else(|_| std::process::abort()),
            primary_rpc_url: url::Url::parse("https://primary.invalid")
                .unwrap_or_else(|_| std::process::abort()),
            backup_rpc_url: url::Url::parse("https://backup.invalid")
                .unwrap_or_else(|_| std::process::abort()),
            asset: "0x036CbD53842c5426634e7929541eC2318f3dCF7e".to_owned(),
            asset_symbol: "USDC".to_owned(),
            minimum_amount: "1".to_owned(),
            relayer_account_id: "0x1111111111111111111111111111111111111111".to_owned(),
            max_inner_gas: 0,
            database_max_connections: 1,
            request_limits: crate::config::RequestLimits::default(),
            sponsorship: crate::config::SponsorshipConfig {
                global_daily_yocto_near: "100000".to_owned(),
                default_client_daily_yocto_near: "100000".to_owned(),
                reservation_yocto_near: "1100".to_owned(),
                balance_warning_yocto_near: "100000".to_owned(),
                balance_hard_stop_yocto_near: "1000".to_owned(),
            },
            payment_identifier: crate::config::PaymentIdentifierConfig::default(),
            accept_v1: false,
            eip155: Some(crate::config::Eip155Config {
                chain_id: 84_532,
                required_confirmations: 2,
                gas_limit: 100,
                max_fee_per_gas_wei: "10".to_owned(),
            }),
        };
        assert!(evm_reservation_covers_liability(&config, 100));
        assert!(!evm_reservation_covers_liability(&config, 101));
    }

    #[test]
    fn signed_delegate_decoder_is_linked_into_service_boundary() {
        assert!(x402_chain_near::decode_signed_delegate("not-base64").is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn authentication_headers_are_redacted_before_http_tracing() {
        let bytes = Arc::new(StdMutex::new(Vec::new()));
        let writer = CaptureWriter(Arc::clone(&bytes));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer)
            .with_ansi(false)
            .without_time()
            .with_span_events(FmtSpan::NEW)
            .finish();
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        let application = Router::new().route("/healthz", get(health)).layer(
            ServiceBuilder::new()
                .layer(SetSensitiveRequestHeadersLayer::new([
                    AUTHORIZATION,
                    HeaderName::from_static("x-api-key"),
                ]))
                .layer(
                    TraceLayer::new_for_http().make_span_with(
                        DefaultMakeSpan::new()
                            .level(tracing::Level::INFO)
                            .include_headers(true),
                    ),
                ),
        );
        let secret = format!("x402_test_{}.{}", "a".repeat(24), "b".repeat(64));
        let request = Request::builder()
            .uri("/healthz")
            .header("x-api-key", &secret)
            .header(AUTHORIZATION, format!("Bearer {secret}"))
            .body(Body::empty())
            .unwrap_or_else(|_| std::process::abort());
        let response = application
            .oneshot(request)
            .await
            .unwrap_or_else(|error| match error {});
        assert_eq!(response.status(), StatusCode::OK);
        let output = bytes.lock().map_or_else(
            |_| std::process::abort(),
            |bytes| String::from_utf8_lossy(&bytes).into_owned(),
        );
        assert!(!output.contains(&secret));
        assert!(
            output.matches("Sensitive").count() >= 2,
            "captured trace did not include an explicit redaction marker: {output}"
        );
    }
}

#[cfg(test)]
#[path = "service_http_tests.rs"]
mod http_tests;

#[cfg(test)]
#[path = "service_recovery_tests.rs"]
mod recovery_tests;
