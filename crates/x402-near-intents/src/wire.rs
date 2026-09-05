//! Strict wire parsing and canonical payment-proof identity.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{ASSET_TRANSFER_METHOD, PAYMENT_FLOW};

const PAYMENT_HASH_DOMAIN: &[u8] = b"x402-near-intents/payment-proof/v1\0";
const MAX_NETWORK_BYTES: usize = 41;
const MAX_ASSET_BYTES: usize = 256;
const MAX_ADDRESS_BYTES: usize = 256;
const MAX_AMOUNT_DIGITS: usize = 78;
const MAX_MEMO_BYTES: usize = 256;
const MAX_RESOURCE_ID_BYTES: usize = 2_048;
const MAX_TRANSACTION_ID_BYTES: usize = 256;

/// Scheme-specific fields carried in `PaymentRequirements.extra`.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NearIntentsExtra {
    pub asset_transfer_method: String,
    pub payment_flow: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deposit_memo: Option<String>,
}

impl fmt::Debug for NearIntentsExtra {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NearIntentsExtra")
            .field("asset_transfer_method", &self.asset_transfer_method)
            .field("payment_flow", &self.payment_flow)
            .field(
                "deposit_memo",
                &self.deposit_memo.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// The current draft's exact payment requirements for a 1Click deposit.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaymentRequirements {
    pub scheme: String,
    pub network: String,
    pub amount: String,
    pub asset: String,
    pub pay_to: String,
    pub max_timeout_seconds: u64,
    pub extra: NearIntentsExtra,
}

impl fmt::Debug for PaymentRequirements {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PaymentRequirements")
            .field("scheme", &self.scheme)
            .field("network", &self.network)
            .field("amount", &"<redacted>")
            .field("asset", &"<redacted>")
            .field("pay_to", &"<redacted>")
            .field("max_timeout_seconds", &self.max_timeout_seconds)
            .field("extra", &self.extra)
            .finish()
    }
}

/// Client-submitted origin-chain payment proof.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaymentProof {
    pub tx_hash: String,
}

impl fmt::Debug for PaymentProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PaymentProof")
            .field("tx_hash", &"<redacted>")
            .finish()
    }
}

/// A structurally validated draft payment and its canonical replay identity.
#[derive(Clone)]
pub struct ValidatedPayment {
    resource_id: String,
    requirements: PaymentRequirements,
    proof: PaymentProof,
    consumption_key: ConsumptionKey,
}

impl ValidatedPayment {
    pub fn resource_id(&self) -> &str {
        &self.resource_id
    }

    pub fn requirements(&self) -> &PaymentRequirements {
        &self.requirements
    }

    pub fn proof(&self) -> &PaymentProof {
        &self.proof
    }

    pub fn consumption_key(&self) -> &ConsumptionKey {
        &self.consumption_key
    }
}

impl fmt::Debug for ValidatedPayment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedPayment")
            .field("resource_id", &"<redacted>")
            .field("requirements", &self.requirements)
            .field("proof", &self.proof)
            .field("consumption_key", &"<redacted>")
            .finish()
    }
}

/// Canonical `<CAIP-2>:<network transaction identifier>` consumption key.
#[derive(Clone, Eq, PartialEq)]
pub struct ConsumptionKey {
    network: String,
    transaction_id: String,
}

impl ConsumptionKey {
    pub fn new(network: &str, transaction_id: &str) -> Result<Self, WireError> {
        validate_caip2(network)?;
        let transaction_id = canonical_transaction_id(network, transaction_id)?;
        Ok(Self {
            network: network.to_owned(),
            transaction_id,
        })
    }

    pub fn network(&self) -> &str {
        &self.network
    }

    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    /// Domain-separated fixed-width journal identity for this proof.
    pub fn payment_hash(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(PAYMENT_HASH_DOMAIN);
        hash.update(
            u64::try_from(self.network.len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        hash.update(self.network.as_bytes());
        hash.update(
            u64::try_from(self.transaction_id.len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        hash.update(self.transaction_id.as_bytes());
        hash.finalize().into()
    }

    /// Scope used by the facilitator's durable unique-anchor constraint.
    pub fn anchor_scope(&self) -> String {
        format!("near-intents:{}", self.network)
    }

    /// Fixed-width representation of the canonical transaction identifier.
    pub fn anchor_value(&self) -> [u8; 32] {
        Sha256::digest(self.transaction_id.as_bytes()).into()
    }
}

impl fmt::Debug for ConsumptionKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConsumptionKey")
            .field("network", &self.network)
            .field("transaction_id", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum WireError {
    #[error("invalid near-intents wire field: {0}")]
    Field(&'static str),
    #[error("paymentPayload.accepted does not equal paymentRequirements")]
    RequirementsMismatch,
    #[error("unsupported origin network namespace: {0}")]
    UnsupportedNetwork(String),
}

/// Parse the scheme-specific parts of an x402 `/settle` request.
///
/// The surrounding service remains responsible for the core request object,
/// x402 version, resource, extensions, authentication, and body-size limits.
pub fn parse_payment(
    resource_id: &str,
    accepted: &Value,
    requirements: &Value,
    payload: &Value,
) -> Result<ValidatedPayment, WireError> {
    validate_bounded_text(resource_id, MAX_RESOURCE_ID_BYTES, "resource.url")?;
    let accepted: PaymentRequirements = serde_json::from_value(accepted.clone())
        .map_err(|_| WireError::Field("paymentPayload.accepted"))?;
    let requirements: PaymentRequirements = serde_json::from_value(requirements.clone())
        .map_err(|_| WireError::Field("paymentRequirements"))?;
    let proof: PaymentProof = serde_json::from_value(payload.clone())
        .map_err(|_| WireError::Field("paymentPayload.payload"))?;

    if accepted != requirements {
        return Err(WireError::RequirementsMismatch);
    }
    validate_requirements(&requirements)?;
    let consumption_key = ConsumptionKey::new(&requirements.network, &proof.tx_hash)?;
    Ok(ValidatedPayment {
        resource_id: resource_id.to_owned(),
        requirements,
        proof,
        consumption_key,
    })
}

pub fn validate_requirements(requirements: &PaymentRequirements) -> Result<(), WireError> {
    if requirements.scheme != "exact" {
        return Err(WireError::Field("scheme"));
    }
    if requirements.extra.asset_transfer_method != ASSET_TRANSFER_METHOD {
        return Err(WireError::Field("extra.assetTransferMethod"));
    }
    if requirements.extra.payment_flow != PAYMENT_FLOW {
        return Err(WireError::Field("extra.paymentFlow"));
    }
    validate_caip2(&requirements.network)?;
    validate_bounded_text(&requirements.asset, MAX_ASSET_BYTES, "asset")?;
    validate_bounded_text(&requirements.pay_to, MAX_ADDRESS_BYTES, "payTo")?;
    validate_atomic_amount(&requirements.amount, "amount")?;
    if requirements.max_timeout_seconds == 0 {
        return Err(WireError::Field("maxTimeoutSeconds"));
    }
    if let Some(memo) = &requirements.extra.deposit_memo {
        validate_bounded_text(memo, MAX_MEMO_BYTES, "extra.depositMemo")?;
    }
    Ok(())
}

pub fn validate_caip2(network: &str) -> Result<(), WireError> {
    if network.len() > MAX_NETWORK_BYTES {
        return Err(WireError::Field("network"));
    }
    let Some((namespace, reference)) = network.split_once(':') else {
        return Err(WireError::Field("network"));
    };
    if namespace.len() < 3
        || namespace.len() > 8
        || !namespace
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || reference.is_empty()
        || reference.len() > 32
        || !reference
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(WireError::Field("network"));
    }
    Ok(())
}

pub fn validate_atomic_amount(value: &str, field: &'static str) -> Result<(), WireError> {
    if value.is_empty()
        || value.len() > MAX_AMOUNT_DIGITS
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(WireError::Field(field));
    }
    if value.bytes().all(|byte| byte == b'0') {
        return Err(WireError::Field(field));
    }
    Ok(())
}

/// Canonicalize transaction identifiers for the origin namespaces currently
/// covered by deterministic format rules. More namespaces require an explicit
/// adapter rather than accepting an opaque string as a replay key.
pub fn canonical_transaction_id(network: &str, value: &str) -> Result<String, WireError> {
    validate_bounded_text(value, MAX_TRANSACTION_ID_BYTES, "payload.txHash")?;
    let (namespace, _) = network.split_once(':').ok_or(WireError::Field("network"))?;
    match namespace {
        "eip155" => canonical_hex_hash(value, true),
        "bip122" | "stellar" => canonical_hex_hash(value, false),
        "near" => canonical_base58(value, 32),
        "solana" => canonical_base58(value, 64),
        other => Err(WireError::UnsupportedNetwork(other.to_owned())),
    }
}

fn canonical_hex_hash(value: &str, prefix: bool) -> Result<String, WireError> {
    let digits = if prefix {
        value
            .strip_prefix("0x")
            .ok_or(WireError::Field("payload.txHash"))?
    } else {
        value
    };
    if digits.len() != 64 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(WireError::Field("payload.txHash"));
    }
    let normalized = digits.to_ascii_lowercase();
    if prefix {
        Ok(format!("0x{normalized}"))
    } else {
        Ok(normalized)
    }
}

fn canonical_base58(value: &str, decoded_length: usize) -> Result<String, WireError> {
    let decoded = bs58::decode(value)
        .into_vec()
        .map_err(|_| WireError::Field("payload.txHash"))?;
    if decoded.len() != decoded_length || bs58::encode(decoded).into_string() != value {
        return Err(WireError::Field("payload.txHash"));
    }
    Ok(value.to_owned())
}

fn validate_bounded_text(
    value: &str,
    maximum: usize,
    field: &'static str,
) -> Result<(), WireError> {
    if value.is_empty()
        || value.len() > maximum
        || value.chars().any(char::is_control)
        || value.trim() != value
    {
        return Err(WireError::Field(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESOURCE: &str = "https://api.example.com/premium-data";

    fn requirements() -> Value {
        serde_json::json!({
            "scheme": "exact",
            "network": "eip155:42161",
            "amount": "1005000",
            "asset": "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
            "payTo": "0x76b4c56085ED136a8744D52bE956396624a730E8",
            "maxTimeoutSeconds": 280,
            "extra": {
                "assetTransferMethod": "near-intents",
                "paymentFlow": "upfront"
            }
        })
    }

    #[test]
    fn parses_current_upstream_example() {
        let accepted = requirements();
        let proof = serde_json::json!({
            "txHash": "0x9bcff372aee89b648c922b850573b22387c31d693079f5e37cd255814e2d615a"
        });
        let payment = parse_payment(RESOURCE, &accepted, &accepted, &proof);
        assert!(payment.is_ok());
        let Some(payment) = payment.ok() else {
            std::process::abort();
        };
        assert_eq!(payment.requirements.extra.payment_flow, "upfront");
        assert_eq!(payment.consumption_key.network(), "eip155:42161");
    }

    #[test]
    fn requirements_and_payload_are_closed() {
        let mut accepted = requirements();
        accepted["extra"]["refundTo"] = Value::String("attacker".to_owned());
        assert!(matches!(
            parse_payment(
                RESOURCE,
                &accepted,
                &accepted,
                &serde_json::json!({"txHash": format!("0x{}", "11".repeat(32))})
            ),
            Err(WireError::Field("paymentPayload.accepted"))
        ));

        let requirements = requirements();
        assert!(matches!(
            parse_payment(
                RESOURCE,
                &requirements,
                &requirements,
                &serde_json::json!({
                    "txHash": format!("0x{}", "11".repeat(32)),
                    "payer": "attacker"
                })
            ),
            Err(WireError::Field("paymentPayload.payload"))
        ));
    }

    #[test]
    fn requires_upfront_and_exact_requirement_equality() {
        let accepted = requirements();
        let mut changed = requirements();
        changed["extra"]["paymentFlow"] = Value::String("authorization".to_owned());
        let proof = serde_json::json!({"txHash": format!("0x{}", "11".repeat(32))});
        assert_eq!(
            parse_payment(RESOURCE, &accepted, &changed, &proof).err(),
            Some(WireError::RequirementsMismatch)
        );

        let mut both = requirements();
        both["extra"]["paymentFlow"] = Value::String("authorization".to_owned());
        assert_eq!(
            parse_payment(RESOURCE, &both, &both, &proof).err(),
            Some(WireError::Field("extra.paymentFlow"))
        );
    }

    #[test]
    fn evm_consumption_key_is_case_canonical_and_domain_separated() {
        let upper = format!("0x{}", "AB".repeat(32));
        let lower = format!("0x{}", "ab".repeat(32));
        let left = ConsumptionKey::new("eip155:42161", &upper);
        let right = ConsumptionKey::new("eip155:42161", &lower);
        assert!(left.is_ok());
        assert!(right.is_ok());
        let (Some(left), Some(right)) = (left.ok(), right.ok()) else {
            std::process::abort();
        };
        assert_eq!(left, right);
        assert_eq!(left.payment_hash(), right.payment_hash());
        assert_ne!(
            left.payment_hash(),
            ConsumptionKey::new("eip155:8453", &lower)
                .map(|key| key.payment_hash())
                .unwrap_or([0_u8; 32])
        );
    }

    #[test]
    fn known_namespaces_enforce_native_transaction_id_shape() {
        assert!(ConsumptionKey::new("eip155:1", "0x01").is_err());
        assert!(ConsumptionKey::new("near:mainnet", "not-base58!").is_err());
        assert!(ConsumptionKey::new("solana:mainnet", "short").is_err());
        assert!(matches!(
            ConsumptionKey::new("xrpl:0", &"A".repeat(64)),
            Err(WireError::UnsupportedNetwork(namespace)) if namespace == "xrpl"
        ));
    }

    #[test]
    fn debug_output_redacts_instrument_and_proof() {
        let accepted = requirements();
        let secret_hash = format!("0x{}", "12".repeat(32));
        let payment = parse_payment(
            RESOURCE,
            &accepted,
            &accepted,
            &serde_json::json!({"txHash": secret_hash}),
        );
        let Some(payment) = payment.ok() else {
            std::process::abort();
        };
        let debug = format!("{payment:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("0x76b4c56085ED136a8744D52bE956396624a730E8"));
        assert!(!debug.contains(&secret_hash));
    }
}
