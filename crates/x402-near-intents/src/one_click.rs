//! Typed, bounded, authenticated access to the 1Click API.

use std::fmt;
use std::time::Duration;

use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use url::Url;
use zeroize::Zeroizing;

use crate::signature::{QuoteSignatureVerifier, VerifiedQuote};

const MAX_RESPONSE_BYTES: usize = 1_048_576;
const X_API_KEY: HeaderName = HeaderName::from_static("x-api-key");

/// Authentication accepted by the official 1Click API.
#[derive(Clone, Copy)]
pub enum Authentication<'a> {
    Bearer(&'a str),
    ApiKey(&'a str),
}

impl fmt::Debug for Authentication<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Authentication(<redacted>)")
    }
}

/// A redirect-disabled client whose authentication header is marked sensitive.
#[derive(Clone)]
pub struct OneClickClient {
    base_url: Url,
    client: Client,
}

impl fmt::Debug for OneClickClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OneClickClient")
            .field("base_url", &self.base_url)
            .field("authentication", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl OneClickClient {
    pub fn new(
        base_url: Url,
        authentication: Authentication<'_>,
        timeout: Duration,
    ) -> Result<Self, OneClickError> {
        validate_base_url(&base_url)?;
        if timeout.is_zero() || timeout > Duration::from_secs(120) {
            return Err(OneClickError::Configuration("invalid request timeout"));
        }

        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let (name, mut value) = match authentication {
            Authentication::Bearer(token) => {
                validate_credential(token)?;
                let encoded = Zeroizing::new(format!("Bearer {token}"));
                let value = HeaderValue::from_str(encoded.as_str())
                    .map_err(|_| OneClickError::Configuration("invalid bearer credential"))?;
                (AUTHORIZATION, value)
            }
            Authentication::ApiKey(key) => {
                validate_credential(key)?;
                let value = HeaderValue::from_str(key)
                    .map_err(|_| OneClickError::Configuration("invalid API credential"))?;
                (X_API_KEY, value)
            }
        };
        value.set_sensitive(true);
        headers.insert(name, value);

        let client = Client::builder()
            .default_headers(headers)
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| OneClickError::Configuration("failed to build HTTP client"))?;
        Ok(Self { base_url, client })
    }

    pub async fn tokens(&self) -> Result<Vec<Token>, OneClickError> {
        let endpoint = "/v0/tokens";
        let url = self.endpoint(endpoint)?;
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| OneClickError::Unavailable(endpoint))?;
        decode_response(endpoint, response).await
    }

    /// Request and authenticate a quote before any response field is consumed.
    pub async fn quote(
        &self,
        request: &QuoteRequest,
        verifier: &QuoteSignatureVerifier,
    ) -> Result<VerifiedQuote, OneClickError> {
        let endpoint = "/v0/quote";
        let url = self.endpoint(endpoint)?;
        let response = self
            .client
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .json(request)
            .send()
            .await
            .map_err(|_| OneClickError::Unavailable(endpoint))?;
        let document: Value = decode_response(endpoint, response).await?;
        verifier
            .verify(&document)
            .map_err(|_| OneClickError::QuoteSignature(endpoint))
    }

    pub async fn submit_deposit(
        &self,
        request: &SubmitDepositRequest,
        verifier: &QuoteSignatureVerifier,
    ) -> Result<AuthenticatedExecutionStatus, OneClickError> {
        let endpoint = "/v0/deposit/submit";
        let url = self.endpoint(endpoint)?;
        let response = self
            .client
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .json(request)
            .send()
            .await
            .map_err(|_| OneClickError::Unavailable(endpoint))?;
        decode_authenticated_status(endpoint, response, verifier).await
    }

    pub async fn status(
        &self,
        deposit_address: &str,
        deposit_memo: Option<&str>,
        verifier: &QuoteSignatureVerifier,
    ) -> Result<AuthenticatedExecutionStatus, OneClickError> {
        if deposit_address.is_empty() {
            return Err(OneClickError::Configuration("empty deposit address"));
        }
        let endpoint = "/v0/status";
        let mut url = self.endpoint(endpoint)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("depositAddress", deposit_address);
            if let Some(memo) = deposit_memo {
                query.append_pair("depositMemo", memo);
            }
        }
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| OneClickError::Unavailable(endpoint))?;
        decode_authenticated_status(endpoint, response, verifier).await
    }

    fn endpoint(&self, path: &'static str) -> Result<Url, OneClickError> {
        self.base_url
            .join(path)
            .map_err(|_| OneClickError::Configuration("invalid endpoint URL"))
    }
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum OneClickError {
    #[error("invalid 1Click client configuration: {0}")]
    Configuration(&'static str),
    #[error("1Click dependency is unavailable at {0}")]
    Unavailable(&'static str),
    #[error("1Click returned HTTP {status} at {endpoint}")]
    Status { endpoint: &'static str, status: u16 },
    #[error("1Click response exceeded the configured bound at {0}")]
    ResponseTooLarge(&'static str),
    #[error("1Click returned an invalid response at {0}")]
    InvalidResponse(&'static str),
    #[error("1Click quote signature verification failed at {0}")]
    QuoteSignature(&'static str),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Token {
    pub asset_id: String,
    pub decimals: u32,
    pub blockchain: String,
    pub symbol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_address: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SwapType {
    ExactOutput,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DepositType {
    OriginChain,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RefundType {
    Intents,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RecipientType {
    DestinationChain,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DepositMode {
    Simple,
    Memo,
}

/// Closed request used to mint an x402-compatible wet `EXACT_OUTPUT` quote.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuoteRequest {
    pub dry: bool,
    pub swap_type: SwapType,
    pub slippage_tolerance: u16,
    pub origin_asset: String,
    pub deposit_type: DepositType,
    pub destination_asset: String,
    pub amount: String,
    pub refund_to: String,
    pub refund_type: RefundType,
    pub recipient: String,
    pub recipient_type: RecipientType,
    pub deadline: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deposit_mode: Option<DepositMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referral: Option<String>,
}

impl fmt::Debug for QuoteRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuoteRequest")
            .field("dry", &self.dry)
            .field("swap_type", &self.swap_type)
            .field("slippage_tolerance", &self.slippage_tolerance)
            .field("route", &"<redacted>")
            .field("deadline", &"<redacted>")
            .field("deposit_mode", &self.deposit_mode)
            .finish_non_exhaustive()
    }
}

impl QuoteRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn exact_output(
        origin_asset: String,
        destination_asset: String,
        destination_amount: String,
        recipient: String,
        refund_intents_account: String,
        deadline: String,
        slippage_tolerance: u16,
        deposit_mode: Option<DepositMode>,
        referral: Option<String>,
    ) -> Self {
        Self {
            dry: false,
            swap_type: SwapType::ExactOutput,
            slippage_tolerance,
            origin_asset,
            deposit_type: DepositType::OriginChain,
            destination_asset,
            amount: destination_amount,
            refund_to: refund_intents_account,
            refund_type: RefundType::Intents,
            recipient,
            recipient_type: RecipientType::DestinationChain,
            deadline,
            deposit_mode,
            referral,
        }
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteResponse {
    pub correlation_id: String,
    pub timestamp: String,
    pub signature: String,
    pub quote_request: QuoteRequest,
    pub quote: Quote,
}

impl fmt::Debug for QuoteResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuoteResponse")
            .field("correlation_id", &"<redacted>")
            .field("timestamp", &"<redacted>")
            .field("signature", &"<redacted>")
            .field("quote_request", &self.quote_request)
            .field("quote", &self.quote)
            .finish()
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    /// Omitted by the live API for a dry quote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deposit_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deposit_memo: Option<String>,
    /// Live 1Click `OpenAPI` field. The draft currently calls this
    /// `maxAmountIn`; validation accepts either spelling, but never conflicting
    /// values, until upstream resolves the mismatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount_in: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_amount_in: Option<String>,
    pub min_amount_in: String,
    pub amount_out: String,
    /// Omitted by the live API for a dry quote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    pub time_estimate: f64,
}

impl fmt::Debug for Quote {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Quote")
            .field("instrument", &"<redacted>")
            .field("amounts", &"<redacted>")
            .field("deadline", &"<redacted>")
            .field("time_estimate", &self.time_estimate)
            .finish_non_exhaustive()
    }
}

impl Quote {
    pub fn advertised_amount(&self) -> Result<&str, OneClickError> {
        match (self.amount_in.as_deref(), self.max_amount_in.as_deref()) {
            (Some(amount), None) | (None, Some(amount)) => Ok(amount),
            (Some(left), Some(right)) if left == right => Ok(left),
            _ => Err(OneClickError::InvalidResponse("/v0/quote")),
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitDepositRequest {
    pub tx_hash: String,
    pub deposit_address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub near_sender_account: Option<String>,
}

impl fmt::Debug for SubmitDepositRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SubmitDepositRequest")
            .field("payment", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SwapStatus {
    KnownDepositTx,
    PendingDeposit,
    IncompleteDeposit,
    Processing,
    Success,
    Refunded,
    Failed,
    #[serde(other)]
    Unknown,
}

impl SwapStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Success | Self::Refunded | Self::Failed)
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionStatusResponse {
    pub correlation_id: String,
    pub quote_response: QuoteResponse,
    pub status: SwapStatus,
    pub updated_at: String,
    pub swap_details: SwapDetails,
}

impl fmt::Debug for ExecutionStatusResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutionStatusResponse")
            .field("status", &self.status)
            .field("payment", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Provider status received from the configured HTTPS origin after its nested
/// quote was cryptographically authenticated.
///
/// The execution fields themselves are provider assertions; 1Click does not
/// sign `status`, `swapDetails`, destination hashes, or `updatedAt`.
#[derive(Clone)]
pub struct AuthenticatedExecutionStatus {
    response: ExecutionStatusResponse,
    quote_hash: String,
}

impl AuthenticatedExecutionStatus {
    pub fn response(&self) -> &ExecutionStatusResponse {
        &self.response
    }

    pub fn quote_hash(&self) -> &str {
        &self.quote_hash
    }

    #[cfg(test)]
    pub(crate) fn from_fixture(response: ExecutionStatusResponse, quote_hash: String) -> Self {
        Self {
            response,
            quote_hash,
        }
    }

    #[cfg(test)]
    pub(crate) fn response_mut(&mut self) -> &mut ExecutionStatusResponse {
        &mut self.response
    }

    #[cfg(test)]
    pub(crate) fn set_quote_hash(&mut self, quote_hash: String) {
        self.quote_hash = quote_hash;
    }
}

impl fmt::Debug for AuthenticatedExecutionStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthenticatedExecutionStatus")
            .field("status", &self.response.status)
            .field("payment", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapDetails {
    #[serde(default)]
    pub origin_chain_tx_hashes: Vec<TransactionDetails>,
    #[serde(default)]
    pub destination_chain_tx_hashes: Vec<TransactionDetails>,
    #[serde(default)]
    pub near_tx_hashes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount_in: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount_out: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deposited_amount: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refunded_amount: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refund_reason: Option<String>,
}

impl fmt::Debug for SwapDetails {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SwapDetails")
            .field("origin_transactions", &self.origin_chain_tx_hashes.len())
            .field(
                "destination_transactions",
                &self.destination_chain_tx_hashes.len(),
            )
            .field("near_transactions", &self.near_tx_hashes.len())
            .field("amounts", &"<redacted>")
            .field("refund", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionDetails {
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explorer_url: Option<String>,
}

impl fmt::Debug for TransactionDetails {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TransactionDetails(<redacted>)")
    }
}

async fn decode_response<T: DeserializeOwned>(
    endpoint: &'static str,
    mut response: Response,
) -> Result<T, OneClickError> {
    let status = response.status();
    if !status.is_success() {
        return Err(status_error(endpoint, status));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(OneClickError::ResponseTooLarge(endpoint));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| OneClickError::Unavailable(endpoint))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(OneClickError::ResponseTooLarge(endpoint));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| OneClickError::InvalidResponse(endpoint))
}

async fn decode_authenticated_status(
    endpoint: &'static str,
    response: Response,
    verifier: &QuoteSignatureVerifier,
) -> Result<AuthenticatedExecutionStatus, OneClickError> {
    let document: Value = decode_response(endpoint, response).await?;
    authenticate_status_document(endpoint, document, verifier)
}

fn authenticate_status_document(
    endpoint: &'static str,
    document: Value,
    verifier: &QuoteSignatureVerifier,
) -> Result<AuthenticatedExecutionStatus, OneClickError> {
    let quote = document
        .get("quoteResponse")
        .ok_or(OneClickError::InvalidResponse(endpoint))?;
    let authenticated_quote = verifier
        .verify(quote)
        .map_err(|_| OneClickError::QuoteSignature(endpoint))?;
    let response =
        serde_json::from_value(document).map_err(|_| OneClickError::InvalidResponse(endpoint))?;
    Ok(AuthenticatedExecutionStatus {
        response,
        quote_hash: authenticated_quote.quote_hash(),
    })
}

#[cfg(test)]
pub(crate) fn authenticate_status_fixture(
    document: Value,
    verifier: &QuoteSignatureVerifier,
) -> Result<AuthenticatedExecutionStatus, OneClickError> {
    authenticate_status_document("/v0/status", document, verifier)
}

fn status_error(endpoint: &'static str, status: StatusCode) -> OneClickError {
    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
        OneClickError::Unavailable(endpoint)
    } else {
        OneClickError::Status {
            endpoint,
            status: status.as_u16(),
        }
    }
}

fn validate_base_url(url: &Url) -> Result<(), OneClickError> {
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(OneClickError::Configuration(
            "base URL must be an HTTPS origin",
        ));
    }
    Ok(())
}

fn validate_credential(value: &str) -> Result<(), OneClickError> {
    if value.is_empty() || value.len() > 16_384 || value.chars().any(char::is_control) {
        return Err(OneClickError::Configuration("invalid credential"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_requires_an_https_origin_and_redacts_authentication() {
        let insecure = Url::parse("http://1click.example/");
        assert!(insecure.is_ok());
        let Some(insecure) = insecure.ok() else {
            std::process::abort();
        };
        assert!(
            OneClickClient::new(
                insecure,
                Authentication::Bearer("top-secret"),
                Duration::from_secs(30)
            )
            .is_err()
        );
        assert_eq!(
            format!("{:?}", Authentication::Bearer("top-secret")),
            "Authentication(<redacted>)"
        );
    }

    #[test]
    fn quote_request_is_closed_and_uses_current_draft_refund_model() {
        let request = QuoteRequest::exact_output(
            "nep141:arb-usdc.omft.near".to_owned(),
            "nep141:base-usdc.omft.near".to_owned(),
            "1000000".to_owned(),
            "0xmerchant".to_owned(),
            "facilitator.near".to_owned(),
            "2026-09-04T15:10:00Z".to_owned(),
            100,
            None,
            Some("x402".to_owned()),
        );
        let value = serde_json::to_value(&request);
        assert!(value.is_ok());
        let Some(value) = value.ok() else {
            std::process::abort();
        };
        assert_eq!(value["dry"], false);
        assert_eq!(value["swapType"], "EXACT_OUTPUT");
        assert_eq!(value["refundType"], "INTENTS");
        assert_eq!(value["depositType"], "ORIGIN_CHAIN");
        assert_eq!(value["recipientType"], "DESTINATION_CHAIN");
        assert!(value.get("connectedWallets").is_none());
    }

    #[test]
    fn quote_amount_accepts_live_or_draft_spelling_but_never_conflict() {
        let mut quote = Quote {
            deposit_address: Some("deposit".to_owned()),
            deposit_memo: None,
            amount_in: Some("101".to_owned()),
            max_amount_in: None,
            min_amount_in: "100".to_owned(),
            amount_out: "100".to_owned(),
            deadline: Some("2026-09-04T15:10:00Z".to_owned()),
            time_estimate: 120.0,
        };
        assert_eq!(quote.advertised_amount().ok(), Some("101"));
        quote.max_amount_in = Some("101".to_owned());
        assert_eq!(quote.advertised_amount().ok(), Some("101"));
        quote.max_amount_in = Some("102".to_owned());
        assert!(quote.advertised_amount().is_err());
    }

    #[test]
    fn status_terminal_classification_waits_for_an_actual_terminal_status() {
        assert!(!SwapStatus::IncompleteDeposit.is_terminal());
        assert!(!SwapStatus::Processing.is_terminal());
        assert!(SwapStatus::Success.is_terminal());
        assert!(SwapStatus::Refunded.is_terminal());
        assert!(SwapStatus::Failed.is_terminal());
        assert!(!SwapStatus::Unknown.is_terminal());
    }

    #[test]
    fn signed_live_dry_shape_stays_outside_the_closed_runtime_dto() {
        let document: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/oneclick-production-dry-exact-output-2026-09-04.json"
        ))
        .unwrap_or_else(|_| std::process::abort());
        let verifier =
            QuoteSignatureVerifier::production().unwrap_or_else(|_| std::process::abort());
        assert!(verifier.verify(&document).is_ok());

        // Live echoes add defaults and this dry fixture uses an out-of-scope
        // refund mode. Runtime normalization remains deliberately fail-closed.
        assert!(serde_json::from_value::<QuoteResponse>(document).is_err());
    }
}
