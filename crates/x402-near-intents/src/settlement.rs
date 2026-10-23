//! Quote binding and settlement-evidence interpretation.

use std::cmp::Ordering;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::one_click::{
    AuthenticatedExecutionStatus, ExecutionStatusResponse, OneClickError, QuoteRequest,
    QuoteResponse, SwapStatus, Token,
};
use crate::signature::VerifiedQuote;
use crate::state::InstrumentId;
use crate::wire::{
    ConsumptionKey, NearIntentsExtra, PaymentRequirements, ValidatedPayment, WireError,
    validate_atomic_amount, validate_caip2, validate_requirements,
};
use crate::{ASSET_TRANSFER_METHOD, PAYMENT_FLOW};

const MAX_CONTEXT_FIELD_BYTES: usize = 512;

/// Explicit x402-to-1Click asset mapping validated against a token snapshot.
#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetMapping {
    network: String,
    x402_asset: String,
    provider_asset: String,
    provider_blockchain: String,
}

impl fmt::Debug for AssetMapping {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssetMapping")
            .field("network", &self.network)
            .field("asset", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl AssetMapping {
    /// Bind an operator-reviewed x402 mapping to exactly one 1Click token.
    pub fn from_token_snapshot(
        network: String,
        x402_asset: String,
        provider_asset: String,
        tokens: &[Token],
    ) -> Result<Self, SettlementEvidenceError> {
        validate_caip2(&network)?;
        for value in [&x402_asset, &provider_asset] {
            validate_context_text(value)?;
        }
        let provider_blockchain = provider_blockchain_for_network(&network)
            .ok_or(SettlementEvidenceError::AssetMapping)?;
        let matching = tokens
            .iter()
            .filter(|token| token.asset_id == provider_asset)
            .collect::<Vec<_>>();
        if matching.len() != 1
            || matching[0].blockchain != provider_blockchain
            || !token_matches_x402_asset(&network, &x402_asset, matching[0])
        {
            return Err(SettlementEvidenceError::AssetMapping);
        }
        Ok(Self {
            network,
            x402_asset,
            provider_asset,
            provider_blockchain: provider_blockchain.to_owned(),
        })
    }

    pub fn network(&self) -> &str {
        &self.network
    }

    pub fn x402_asset(&self) -> &str {
        &self.x402_asset
    }

    pub fn provider_asset(&self) -> &str {
        &self.provider_asset
    }
}

/// x402-facing route facts that are not exposed in the origin requirements.
#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteContext {
    resource_id: String,
    origin: AssetMapping,
    destination: AssetMapping,
    destination_pay_to: String,
    destination_amount: String,
    refund_intents_account: String,
}

impl fmt::Debug for QuoteContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuoteContext")
            .field("resource_id", &"<redacted>")
            .field("origin_network", &self.origin.network)
            .field("destination_network", &self.destination.network)
            .field("payment", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl QuoteContext {
    pub fn new(
        resource_id: String,
        origin: AssetMapping,
        destination: AssetMapping,
        destination_pay_to: String,
        destination_amount: String,
        refund_intents_account: String,
    ) -> Result<Self, SettlementEvidenceError> {
        validate_context_text(&resource_id)?;
        validate_context_text(&destination_pay_to)?;
        validate_atomic_amount(&destination_amount, "destination.amount")?;
        validate_context_text(&refund_intents_account)?;
        Ok(Self {
            resource_id,
            origin,
            destination,
            destination_pay_to,
            destination_amount,
            refund_intents_account,
        })
    }

    pub fn resource_id(&self) -> &str {
        &self.resource_id
    }

    pub fn origin(&self) -> &AssetMapping {
        &self.origin
    }

    pub fn destination(&self) -> &AssetMapping {
        &self.destination
    }

    pub fn destination_pay_to(&self) -> &str {
        &self.destination_pay_to
    }

    pub fn destination_amount(&self) -> &str {
        &self.destination_amount
    }
}

/// Structurally bound quote and exact requirements issued from it.
///
/// Runtime construction remains closed until a verifier-produced raw response
/// can be normalized into the provider DTO without dropping signed fields.
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IssuedQuote {
    context: QuoteContext,
    instrument_id: InstrumentId,
    requirements: PaymentRequirements,
    minimum_input_amount: String,
    deadline: DateTime<Utc>,
    provider_document: Value,
    provider_quote_hash: String,
    provider_response: QuoteResponse,
}

impl fmt::Debug for IssuedQuote {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IssuedQuote")
            .field("context", &self.context)
            .field("requirements", &self.requirements)
            .field("provider_quote", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl IssuedQuote {
    pub fn context(&self) -> &QuoteContext {
        &self.context
    }

    pub const fn instrument_id(&self) -> InstrumentId {
        self.instrument_id
    }

    pub fn requirements(&self) -> &PaymentRequirements {
        &self.requirements
    }

    pub fn minimum_input_amount(&self) -> &str {
        &self.minimum_input_amount
    }

    pub const fn deadline(&self) -> DateTime<Utc> {
        self.deadline
    }

    pub fn provider_response(&self) -> &QuoteResponse {
        &self.provider_response
    }

    /// Complete raw response retained for re-verification after recovery.
    pub fn provider_document(&self) -> &Value {
        &self.provider_document
    }

    pub fn provider_quote_hash(&self) -> &str {
        &self.provider_quote_hash
    }

    /// Bind an authenticated raw 1Click response to the request and x402 route
    /// that produced it. DTO normalization happens only after verification.
    pub fn from_verified_response(
        context: QuoteContext,
        request: &QuoteRequest,
        verified: &VerifiedQuote,
        now: DateTime<Utc>,
    ) -> Result<Self, SettlementEvidenceError> {
        let response: QuoteResponse = serde_json::from_value(verified.document().clone())
            .map_err(|_| SettlementEvidenceError::Quote("response shape"))?;
        Self::from_parts(
            context,
            request,
            response,
            verified.document().clone(),
            verified.quote_hash(),
            now,
        )
    }

    fn from_parts(
        context: QuoteContext,
        request: &QuoteRequest,
        response: QuoteResponse,
        provider_document: Value,
        provider_quote_hash: String,
        now: DateTime<Utc>,
    ) -> Result<Self, SettlementEvidenceError> {
        if request.dry
            || request.amount != context.destination_amount
            || request.origin_asset != context.origin.provider_asset
            || request.destination_asset != context.destination.provider_asset
            || request.recipient != context.destination_pay_to
            || request.refund_to != context.refund_intents_account
            || response.quote_request != *request
        {
            return Err(SettlementEvidenceError::QuoteRequestMismatch);
        }
        if response.correlation_id.is_empty()
            || response.signature.is_empty()
            || DateTime::parse_from_rfc3339(&response.timestamp).is_err()
        {
            return Err(SettlementEvidenceError::Quote("response metadata"));
        }
        let quote = &response.quote;
        let advertised_amount = quote.advertised_amount()?;
        validate_atomic_amount(advertised_amount, "quote.amountIn")?;
        validate_atomic_amount(&quote.min_amount_in, "quote.minAmountIn")?;
        validate_atomic_amount(&quote.amount_out, "quote.amountOut")?;
        if decimal_cmp(advertised_amount, &quote.min_amount_in).is_lt()
            || quote.amount_out != context.destination_amount
        {
            return Err(SettlementEvidenceError::Quote("amounts"));
        }
        if !quote.time_estimate.is_finite() || quote.time_estimate < 0.0 {
            return Err(SettlementEvidenceError::Quote("timeEstimate"));
        }
        let deposit_address = quote
            .deposit_address
            .as_deref()
            .ok_or(SettlementEvidenceError::Quote("depositAddress"))?;
        let quote_deadline = quote
            .deadline
            .as_deref()
            .ok_or(SettlementEvidenceError::Quote("deadline"))?;
        let requested_deadline = DateTime::parse_from_rfc3339(&request.deadline)
            .map_err(|_| SettlementEvidenceError::Quote("request.deadline"))?
            .with_timezone(&Utc);
        let deadline = DateTime::parse_from_rfc3339(quote_deadline)
            .map_err(|_| SettlementEvidenceError::Quote("deadline"))?
            .with_timezone(&Utc);
        if deadline > requested_deadline {
            return Err(SettlementEvidenceError::Quote("deadline"));
        }
        let remaining = deadline.signed_duration_since(now).num_seconds();
        let max_timeout_seconds = u64::try_from(remaining)
            .ok()
            .filter(|value| *value > 0)
            .ok_or(SettlementEvidenceError::Quote("deadline"))?;
        let requirements = PaymentRequirements {
            scheme: "exact".to_owned(),
            network: context.origin.network.clone(),
            amount: advertised_amount.to_owned(),
            asset: context.origin.x402_asset.clone(),
            pay_to: deposit_address.to_owned(),
            max_timeout_seconds,
            extra: NearIntentsExtra {
                asset_transfer_method: ASSET_TRANSFER_METHOD.to_owned(),
                payment_flow: PAYMENT_FLOW.to_owned(),
                deposit_memo: quote.deposit_memo.clone(),
            },
        };
        validate_requirements(&requirements)?;
        let instrument_id = InstrumentId::new(
            &requirements.network,
            &requirements.pay_to,
            requirements.extra.deposit_memo.as_deref(),
        )?;
        Ok(Self {
            context,
            instrument_id,
            requirements,
            minimum_input_amount: quote.min_amount_in.clone(),
            deadline,
            provider_document,
            provider_quote_hash,
            provider_response: response,
        })
    }

    #[cfg(test)]
    fn from_fixture_response(
        context: QuoteContext,
        request: &QuoteRequest,
        response: QuoteResponse,
        now: DateTime<Utc>,
    ) -> Result<Self, SettlementEvidenceError> {
        let provider_document = serde_json::to_value(&response)
            .map_err(|_| SettlementEvidenceError::Quote("fixture response"))?;
        Self::from_parts(
            context,
            request,
            response,
            provider_document,
            "fixture-quote-hash".to_owned(),
            now,
        )
    }
}

/// Origin-chain facts established independently of the proof presenter.
///
/// An origin adapter constructs this only after confirming the exact asset,
/// recipient, amount, sender, transaction identity, and its configured chain
/// finality. A 1Click status aggregate alone is insufficient to construct it.
#[derive(Clone, Eq, PartialEq)]
pub struct VerifiedOriginDeposit {
    consumption_key: ConsumptionKey,
    asset: String,
    recipient: String,
    deposit_memo: Option<String>,
    amount: String,
    sender: String,
}

impl fmt::Debug for VerifiedOriginDeposit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedOriginDeposit")
            .field("network", &self.consumption_key.network())
            .field("payment", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl VerifiedOriginDeposit {
    /// Construct structurally valid evidence for isolated model tests.
    /// Production construction stays unavailable until the first crate-owned
    /// finality-aware origin adapter exists.
    #[cfg(test)]
    fn from_fixture(
        network: &str,
        transaction_id: &str,
        asset: String,
        recipient: String,
        deposit_memo: Option<String>,
        amount: String,
        sender: String,
    ) -> Result<Self, SettlementEvidenceError> {
        validate_atomic_amount(&amount, "origin.amount")?;
        if asset.is_empty() || recipient.is_empty() || sender.is_empty() {
            return Err(SettlementEvidenceError::OriginDepositMismatch);
        }
        Ok(Self {
            consumption_key: ConsumptionKey::new(network, transaction_id)?,
            asset,
            recipient,
            deposit_memo,
            amount,
            sender,
        })
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum SettlementDecision {
    NotFinal {
        status: SwapStatus,
    },
    Succeeded {
        payer: String,
        destination_transactions: Vec<String>,
    },
    Failed {
        payer: String,
        status: SwapStatus,
    },
}

impl fmt::Debug for SettlementDecision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFinal { status } => formatter
                .debug_struct("NotFinal")
                .field("status", status)
                .finish(),
            Self::Succeeded {
                destination_transactions,
                ..
            } => formatter
                .debug_struct("Succeeded")
                .field("payer", &"<redacted>")
                .field("destination_transactions", &destination_transactions.len())
                .finish(),
            Self::Failed { status, .. } => formatter
                .debug_struct("Failed")
                .field("payer", &"<redacted>")
                .field("status", status)
                .finish(),
        }
    }
}

/// A payment decision plus the provider's aggregate refund observation.
///
/// 1Click does not attribute `refundedAmount` to an origin transaction or
/// sender. Callers must not forward this amount to `decision.payer` without
/// independent per-deposit evidence.
#[derive(Clone, Eq, PartialEq)]
pub struct SettlementAssessment {
    pub decision: SettlementDecision,
    pub unattributed_refund_amount: Option<String>,
}

impl fmt::Debug for SettlementAssessment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SettlementAssessment")
            .field("decision", &self.decision)
            .field(
                "unattributed_refund_amount",
                &self
                    .unattributed_refund_amount
                    .as_ref()
                    .map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Combine independently verified origin-chain facts with an authenticated
/// 1Click provider assertion. This function performs no I/O or state mutation.
///
/// The provider's execution fields are not covered by its quote signature.
/// Before runtime enablement, the destination transaction must either be
/// independently confirmed or covered by an explicit reviewed provider trust
/// contract.
pub fn evaluate_status(
    issued: &IssuedQuote,
    payment: &ValidatedPayment,
    origin: &VerifiedOriginDeposit,
    status: &AuthenticatedExecutionStatus,
) -> Result<SettlementAssessment, SettlementEvidenceError> {
    if payment.resource_id() != issued.context.resource_id
        || payment.requirements() != &issued.requirements
    {
        return Err(SettlementEvidenceError::IssuanceMismatch);
    }
    if payment.consumption_key() != &origin.consumption_key
        || origin.asset != issued.requirements.asset
        || origin.recipient != issued.requirements.pay_to
        || origin.deposit_memo != issued.requirements.extra.deposit_memo
        || decimal_cmp(&origin.amount, &issued.requirements.amount).is_lt()
    {
        return Err(SettlementEvidenceError::OriginDepositMismatch);
    }
    if status.quote_hash() != issued.provider_quote_hash {
        return Err(SettlementEvidenceError::StatusQuoteMismatch);
    }
    let status = status.response();
    if status.correlation_id.is_empty() || DateTime::parse_from_rfc3339(&status.updated_at).is_err()
    {
        return Err(SettlementEvidenceError::StatusMetadata);
    }

    let unattributed_refund_amount = status.swap_details.refunded_amount.clone();
    validate_optional_amount(
        unattributed_refund_amount.as_deref(),
        "status.swapDetails.refundedAmount",
    )?;

    let decision = match status.status {
        SwapStatus::KnownDepositTx
        | SwapStatus::PendingDeposit
        | SwapStatus::IncompleteDeposit
        | SwapStatus::Processing => SettlementDecision::NotFinal {
            status: status.status,
        },
        SwapStatus::Unknown => return Err(SettlementEvidenceError::UnknownStatus),
        SwapStatus::Success => {
            if !status_has_origin_payment(status, payment.consumption_key()) {
                return Err(SettlementEvidenceError::MissingOriginAttribution);
            }
            if status.swap_details.amount_out.as_deref()
                != Some(issued.context.destination_amount.as_str())
            {
                return Err(SettlementEvidenceError::DestinationAmountMismatch);
            }
            let destination_transactions = status
                .swap_details
                .destination_chain_tx_hashes
                .iter()
                .map(|transaction| transaction.hash.clone())
                .collect::<Vec<_>>();
            if destination_transactions.is_empty()
                || destination_transactions.iter().any(String::is_empty)
            {
                return Err(SettlementEvidenceError::MissingDestinationTransaction);
            }
            SettlementDecision::Succeeded {
                payer: origin.sender.clone(),
                destination_transactions,
            }
        }
        SwapStatus::Refunded | SwapStatus::Failed => SettlementDecision::Failed {
            payer: origin.sender.clone(),
            status: status.status,
        },
    };
    Ok(SettlementAssessment {
        decision,
        unattributed_refund_amount,
    })
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum SettlementEvidenceError {
    #[error("invalid quote field: {0}")]
    Quote(&'static str),
    #[error("configured x402 asset mapping does not match the 1Click token snapshot")]
    AssetMapping,
    #[error("1Click quote did not echo the exact request")]
    QuoteRequestMismatch,
    #[error("payment does not match the exact issued resource and requirements")]
    IssuanceMismatch,
    #[error("origin deposit does not match the issued instrument")]
    OriginDepositMismatch,
    #[error("1Click status is not bound to the issued quote")]
    StatusQuoteMismatch,
    #[error("1Click status metadata is invalid")]
    StatusMetadata,
    #[error("1Click success did not attribute the presented origin transaction")]
    MissingOriginAttribution,
    #[error("1Click success did not report the exact destination amount")]
    DestinationAmountMismatch,
    #[error("1Click success did not report a destination transaction")]
    MissingDestinationTransaction,
    #[error("1Click returned an unknown status")]
    UnknownStatus,
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error(transparent)]
    OneClick(#[from] OneClickError),
    #[error(transparent)]
    Claim(#[from] crate::state::ClaimError),
}

fn validate_context_text(value: &str) -> Result<(), SettlementEvidenceError> {
    if value.is_empty()
        || value.len() > MAX_CONTEXT_FIELD_BYTES
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(SettlementEvidenceError::Quote("context"));
    }
    Ok(())
}

fn provider_blockchain_for_network(network: &str) -> Option<&'static str> {
    match network {
        "eip155:1" => Some("eth"),
        "eip155:10" => Some("op"),
        "eip155:56" => Some("bsc"),
        "eip155:100" => Some("gnosis"),
        "eip155:137" => Some("pol"),
        "eip155:8453" => Some("base"),
        "eip155:42161" => Some("arb"),
        "eip155:43114" => Some("avax"),
        "near:mainnet" => Some("near"),
        "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp" => Some("sol"),
        "bip122:000000000019d6689c085ae165831e93" => Some("btc"),
        _ => None,
    }
}

fn token_matches_x402_asset(network: &str, x402_asset: &str, token: &Token) -> bool {
    let namespace = network.split_once(':').map(|(namespace, _)| namespace);
    match namespace {
        Some("eip155") => token.contract_address.as_deref().is_some_and(|contract| {
            canonical_evm_address(contract)
                .zip(canonical_evm_address(x402_asset))
                .is_some_and(|(left, right)| left == right)
        }),
        Some("near" | "solana") => token.contract_address.as_deref() == Some(x402_asset),
        Some("bip122") => {
            token.contract_address.is_none()
                && token.symbol.eq_ignore_ascii_case("BTC")
                && x402_asset.eq_ignore_ascii_case("BTC")
        }
        _ => false,
    }
}

fn canonical_evm_address(value: &str) -> Option<String> {
    let digits = value.strip_prefix("0x")?;
    if digits.len() != 40 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(digits.to_ascii_lowercase())
}

fn status_has_origin_payment(status: &ExecutionStatusResponse, expected: &ConsumptionKey) -> bool {
    status
        .swap_details
        .origin_chain_tx_hashes
        .iter()
        .filter_map(|transaction| ConsumptionKey::new(expected.network(), &transaction.hash).ok())
        .any(|observed| observed == *expected)
}

fn decimal_cmp(left: &str, right: &str) -> Ordering {
    let left = left.trim_start_matches('0');
    let right = right.trim_start_matches('0');
    let left = if left.is_empty() { "0" } else { left };
    let right = if right.is_empty() { "0" } else { right };
    left.len()
        .cmp(&right.len())
        .then_with(|| left.as_bytes().cmp(right.as_bytes()))
}

fn validate_optional_amount(
    amount: Option<&str>,
    field: &'static str,
) -> Result<(), SettlementEvidenceError> {
    if amount
        .is_some_and(|value| value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(SettlementEvidenceError::Quote(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;
    use crate::one_click::{
        AuthenticatedExecutionStatus, ExecutionStatusResponse, Quote, SwapDetails,
        TransactionDetails, authenticate_status_fixture,
    };
    use crate::signature::QuoteSignatureVerifier;
    use crate::wire::parse_payment;

    const ORIGIN_TX: &str = "0x9bcff372aee89b648c922b850573b22387c31d693079f5e37cd255814e2d615a";

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 4, 15, 0, 0)
            .single()
            .unwrap_or_else(|| std::process::abort())
    }

    fn token_snapshot() -> Vec<Token> {
        vec![
            Token {
                asset_id: "nep141:arb-usdc.omft.near".to_owned(),
                decimals: 6,
                blockchain: "arb".to_owned(),
                symbol: "USDC".to_owned(),
                contract_address: Some("0xaf88d065e77c8cc2239327c5edb3a432268e5831".to_owned()),
            },
            Token {
                asset_id: "nep141:base-usdc.omft.near".to_owned(),
                decimals: 6,
                blockchain: "base".to_owned(),
                symbol: "USDC".to_owned(),
                contract_address: Some("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".to_owned()),
            },
        ]
    }

    fn context() -> QuoteContext {
        let tokens = token_snapshot();
        let origin = AssetMapping::from_token_snapshot(
            "eip155:42161".to_owned(),
            "0xaf88d065e77c8cC2239327C5EDb3A432268e5831".to_owned(),
            "nep141:arb-usdc.omft.near".to_owned(),
            &tokens,
        )
        .unwrap_or_else(|_| std::process::abort());
        let destination = AssetMapping::from_token_snapshot(
            "eip155:8453".to_owned(),
            "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".to_owned(),
            "nep141:base-usdc.omft.near".to_owned(),
            &tokens,
        )
        .unwrap_or_else(|_| std::process::abort());
        QuoteContext::new(
            "https://api.example.com/premium-data".to_owned(),
            origin,
            destination,
            "0xMerchantOnBase".to_owned(),
            "1000000".to_owned(),
            "facilitator.near".to_owned(),
        )
        .unwrap_or_else(|_| std::process::abort())
    }

    fn request() -> QuoteRequest {
        QuoteRequest::exact_output(
            "nep141:arb-usdc.omft.near".to_owned(),
            "nep141:base-usdc.omft.near".to_owned(),
            "1000000".to_owned(),
            "0xMerchantOnBase".to_owned(),
            "facilitator.near".to_owned(),
            "2026-09-04T15:10:00Z".to_owned(),
            100,
            None,
            Some("x402".to_owned()),
        )
    }

    fn response() -> QuoteResponse {
        QuoteResponse {
            correlation_id: "correlation".to_owned(),
            timestamp: "2026-09-04T15:00:00Z".to_owned(),
            signature: "service-signature".to_owned(),
            quote_request: request(),
            quote: Quote {
                deposit_address: Some("0x76b4c56085ED136a8744D52bE956396624a730E8".to_owned()),
                deposit_memo: None,
                amount_in: Some("1005000".to_owned()),
                max_amount_in: None,
                min_amount_in: "1000000".to_owned(),
                amount_out: "1000000".to_owned(),
                deadline: Some("2026-09-04T15:10:00Z".to_owned()),
                time_estimate: 120.0,
            },
        }
    }

    fn signed_quote_document() -> Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/deterministic-wet-exact-output.json"
        ))
        .unwrap_or_else(|_| std::process::abort())
    }

    fn issued() -> IssuedQuote {
        IssuedQuote::from_fixture_response(context(), &request(), response(), now())
            .unwrap_or_else(|_| std::process::abort())
    }

    fn payment(issued: &IssuedQuote) -> ValidatedPayment {
        payment_for_resource(issued, &issued.context.resource_id)
    }

    fn payment_for_resource(issued: &IssuedQuote, resource_id: &str) -> ValidatedPayment {
        let requirements =
            serde_json::to_value(&issued.requirements).unwrap_or_else(|_| std::process::abort());
        parse_payment(
            resource_id,
            &requirements,
            &requirements,
            &json!({"txHash": ORIGIN_TX}),
        )
        .unwrap_or_else(|_| std::process::abort())
    }

    fn origin() -> VerifiedOriginDeposit {
        VerifiedOriginDeposit::from_fixture(
            "eip155:42161",
            ORIGIN_TX,
            "0xaf88d065e77c8cC2239327C5EDb3A432268e5831".to_owned(),
            "0x76b4c56085ED136a8744D52bE956396624a730E8".to_owned(),
            None,
            "1005000".to_owned(),
            "0xClient".to_owned(),
        )
        .unwrap_or_else(|_| std::process::abort())
    }

    fn status(issued: &IssuedQuote, state: SwapStatus) -> AuthenticatedExecutionStatus {
        let response = ExecutionStatusResponse {
            correlation_id: "status-correlation".to_owned(),
            quote_response: issued.provider_response.clone(),
            status: state,
            updated_at: "2026-09-04T15:02:00Z".to_owned(),
            swap_details: SwapDetails {
                origin_chain_tx_hashes: vec![TransactionDetails {
                    hash: ORIGIN_TX.to_owned(),
                    explorer_url: None,
                }],
                destination_chain_tx_hashes: vec![TransactionDetails {
                    hash: "0xdestination".to_owned(),
                    explorer_url: None,
                }],
                amount_out: Some("1000000".to_owned()),
                ..SwapDetails::default()
            },
        };
        AuthenticatedExecutionStatus::from_fixture(
            response,
            issued.provider_quote_hash().to_owned(),
        )
    }

    #[test]
    fn quote_is_bound_to_exact_request_and_live_api_amount_name() {
        let issued = issued();
        assert_eq!(issued.requirements.amount, "1005000");
        assert_eq!(issued.minimum_input_amount, "1000000");
        assert_eq!(issued.requirements.max_timeout_seconds, 600);

        let mut changed = response();
        changed.quote_request.recipient = "attacker".to_owned();
        assert_eq!(
            IssuedQuote::from_fixture_response(context(), &request(), changed, now()).err(),
            Some(SettlementEvidenceError::QuoteRequestMismatch)
        );
    }

    #[test]
    fn authenticated_quote_and_status_capabilities_gate_success() {
        // DO NOT FUND: this trust root exists only for the deterministic,
        // expired fixture and is unavailable outside test builds.
        let verifier = QuoteSignatureVerifier::for_test(
            "ed25519:9C6hybhQ6Aycep9jaUnP6uL9ZYvDjUp1aSkFWPUFJtpj",
        )
        .unwrap_or_else(|_| std::process::abort());
        let document = signed_quote_document();
        let authenticated_quote = verifier
            .verify(&document)
            .unwrap_or_else(|_| std::process::abort());
        let issued =
            IssuedQuote::from_verified_response(context(), &request(), &authenticated_quote, now())
                .unwrap_or_else(|_| std::process::abort());
        assert_eq!(issued.provider_document(), &document);
        assert_eq!(
            issued.provider_quote_hash(),
            "3Nnstyx8CZPxpBMdN2QpPxGH1tNxiud858Z8LBtHVAoL"
        );

        let status_document = json!({
            "correlationId": "status-correlation",
            "quoteResponse": document,
            "status": "SUCCESS",
            "updatedAt": "2026-09-04T15:02:00Z",
            "swapDetails": {
                "originChainTxHashes": [{"hash": ORIGIN_TX}],
                "destinationChainTxHashes": [{"hash": "0xdestination"}],
                "amountOut": "1000000"
            }
        });
        let authenticated_status = authenticate_status_fixture(status_document, &verifier)
            .unwrap_or_else(|_| std::process::abort());
        let assessment =
            evaluate_status(&issued, &payment(&issued), &origin(), &authenticated_status);
        assert!(matches!(
            assessment,
            Ok(SettlementAssessment {
                decision: SettlementDecision::Succeeded { .. },
                unattributed_refund_amount: None,
            })
        ));
    }

    #[test]
    fn asset_mapping_rejects_chain_contract_and_snapshot_ambiguity() {
        let tokens = token_snapshot();
        let wrong_chain = AssetMapping::from_token_snapshot(
            "eip155:8453".to_owned(),
            "0xaf88d065e77c8cc2239327c5edb3a432268e5831".to_owned(),
            "nep141:arb-usdc.omft.near".to_owned(),
            &tokens,
        );
        assert_eq!(
            wrong_chain.err(),
            Some(SettlementEvidenceError::AssetMapping)
        );

        let wrong_contract = AssetMapping::from_token_snapshot(
            "eip155:42161".to_owned(),
            "0x0000000000000000000000000000000000000001".to_owned(),
            "nep141:arb-usdc.omft.near".to_owned(),
            &tokens,
        );
        assert_eq!(
            wrong_contract.err(),
            Some(SettlementEvidenceError::AssetMapping)
        );

        let mut ambiguous = tokens.clone();
        ambiguous.push(tokens[0].clone());
        let duplicate = AssetMapping::from_token_snapshot(
            "eip155:42161".to_owned(),
            "0xaf88d065e77c8cc2239327c5edb3a432268e5831".to_owned(),
            "nep141:arb-usdc.omft.near".to_owned(),
            &ambiguous,
        );
        assert_eq!(duplicate.err(), Some(SettlementEvidenceError::AssetMapping));

        let testnet = AssetMapping::from_token_snapshot(
            "eip155:84532".to_owned(),
            "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".to_owned(),
            "nep141:base-usdc.omft.near".to_owned(),
            &tokens,
        );
        assert_eq!(testnet.err(), Some(SettlementEvidenceError::AssetMapping));
    }

    #[test]
    fn quote_request_cannot_substitute_route_or_refund_account() {
        let substitutions = [
            ("origin", "attacker-origin"),
            ("destination", "attacker-destination"),
            ("recipient", "attacker-recipient"),
            ("refund", "attacker.near"),
        ];
        for (field, value) in substitutions {
            let mut changed_request = request();
            match field {
                "origin" => changed_request.origin_asset = value.to_owned(),
                "destination" => changed_request.destination_asset = value.to_owned(),
                "recipient" => changed_request.recipient = value.to_owned(),
                "refund" => changed_request.refund_to = value.to_owned(),
                _ => std::process::abort(),
            }
            let mut changed_response = response();
            changed_response.quote_request = changed_request.clone();
            assert_eq!(
                IssuedQuote::from_fixture_response(
                    context(),
                    &changed_request,
                    changed_response,
                    now(),
                )
                .err(),
                Some(SettlementEvidenceError::QuoteRequestMismatch),
                "substitution unexpectedly accepted for {field}"
            );
        }
    }

    #[test]
    fn quote_deadline_cannot_extend_the_requested_policy() {
        let mut shorter_request = request();
        shorter_request.deadline = "2026-09-04T15:05:00Z".to_owned();
        let mut later_response = response();
        later_response.quote_request = shorter_request.clone();
        assert_eq!(
            IssuedQuote::from_fixture_response(context(), &shorter_request, later_response, now(),)
                .err(),
            Some(SettlementEvidenceError::Quote("deadline"))
        );

        let mut malformed_request = request();
        malformed_request.deadline = "not-a-time".to_owned();
        let mut matching_response = response();
        matching_response.quote_request = malformed_request.clone();
        assert_eq!(
            IssuedQuote::from_fixture_response(
                context(),
                &malformed_request,
                matching_response,
                now(),
            )
            .err(),
            Some(SettlementEvidenceError::Quote("request.deadline"))
        );
    }

    #[test]
    fn success_requires_both_origin_and_exact_destination_evidence() {
        let issued = issued();
        let payment = payment(&issued);
        let success = status(&issued, SwapStatus::Success);
        assert!(matches!(
            evaluate_status(&issued, &payment, &origin(), &success),
            Ok(SettlementAssessment {
                decision: SettlementDecision::Succeeded {
                    ref payer,
                    ref destination_transactions,
                },
                unattributed_refund_amount: None,
            }) if payer == "0xClient" && destination_transactions == &["0xdestination"]
        ));

        let mut wrong_output = success.clone();
        wrong_output.response_mut().swap_details.amount_out = Some("999999".to_owned());
        assert_eq!(
            evaluate_status(&issued, &payment, &origin(), &wrong_output).err(),
            Some(SettlementEvidenceError::DestinationAmountMismatch)
        );

        let mut missing_proof = success;
        missing_proof
            .response_mut()
            .swap_details
            .origin_chain_tx_hashes
            .clear();
        assert_eq!(
            evaluate_status(&issued, &payment, &origin(), &missing_proof).err(),
            Some(SettlementEvidenceError::MissingOriginAttribution)
        );
    }

    #[test]
    fn status_correlation_is_diagnostic_but_signed_quote_changes_fail() {
        let issued = issued();
        let payment = payment(&issued);
        let mut changed_correlation = status(&issued, SwapStatus::Success);
        changed_correlation
            .response_mut()
            .quote_response
            .correlation_id = "status-copy".to_owned();
        assert!(evaluate_status(&issued, &payment, &origin(), &changed_correlation).is_ok());

        let mut changed_quote = changed_correlation;
        changed_quote.response_mut().quote_response.quote.amount_out = "999999".to_owned();
        changed_quote.set_quote_hash("different-authenticated-quote".to_owned());
        assert_eq!(
            evaluate_status(&issued, &payment, &origin(), &changed_quote).err(),
            Some(SettlementEvidenceError::StatusQuoteMismatch)
        );
    }

    #[test]
    fn aggregate_status_cannot_make_a_dust_proof_valid() {
        let issued = issued();
        let payment = payment(&issued);
        let dust = VerifiedOriginDeposit::from_fixture(
            "eip155:42161",
            ORIGIN_TX,
            issued.requirements.asset.clone(),
            issued.requirements.pay_to.clone(),
            None,
            "1".to_owned(),
            "0xClient".to_owned(),
        )
        .unwrap_or_else(|_| std::process::abort());
        assert_eq!(
            evaluate_status(
                &issued,
                &payment,
                &dust,
                &status(&issued, SwapStatus::Success)
            )
            .err(),
            Some(SettlementEvidenceError::OriginDepositMismatch)
        );
    }

    #[test]
    fn incomplete_is_retryable_and_refund_is_terminal_failure() {
        let issued = issued();
        let payment = payment(&issued);
        assert_eq!(
            evaluate_status(
                &issued,
                &payment,
                &origin(),
                &status(&issued, SwapStatus::IncompleteDeposit)
            )
            .ok(),
            Some(SettlementAssessment {
                decision: SettlementDecision::NotFinal {
                    status: SwapStatus::IncompleteDeposit
                },
                unattributed_refund_amount: None,
            })
        );

        let mut refunded = status(&issued, SwapStatus::Refunded);
        refunded.response_mut().swap_details.refunded_amount = Some("1005000".to_owned());
        assert!(matches!(
            evaluate_status(&issued, &payment, &origin(), &refunded),
            Ok(SettlementAssessment {
                decision: SettlementDecision::Failed { .. },
                unattributed_refund_amount: Some(ref amount),
            }) if amount == "1005000"
        ));
    }

    #[test]
    fn successful_surplus_is_not_misclassified_as_payment_failure() {
        let issued = issued();
        let payment = payment(&issued);
        let mut success = status(&issued, SwapStatus::Success);
        success.response_mut().swap_details.refunded_amount = Some("5000".to_owned());
        assert!(matches!(
            evaluate_status(&issued, &payment, &origin(), &success),
            Ok(SettlementAssessment {
                decision: SettlementDecision::Succeeded { .. },
                unattributed_refund_amount: Some(ref amount),
            }) if amount == "5000"
        ));
    }

    #[test]
    fn payment_must_match_resource_and_memo_from_issuance() {
        let issued = issued();
        let wrong_resource = payment_for_resource(&issued, "https://attacker.example/resource");
        assert_eq!(
            evaluate_status(
                &issued,
                &wrong_resource,
                &origin(),
                &status(&issued, SwapStatus::Success)
            )
            .err(),
            Some(SettlementEvidenceError::IssuanceMismatch)
        );

        let mut wrong_memo = origin();
        wrong_memo.deposit_memo = Some("unexpected".to_owned());
        let payment = payment(&issued);
        assert_eq!(
            evaluate_status(
                &issued,
                &payment,
                &wrong_memo,
                &status(&issued, SwapStatus::Success)
            )
            .err(),
            Some(SettlementEvidenceError::OriginDepositMismatch)
        );
    }
}
