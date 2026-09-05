//! 1Click quote-signature verification pinned to the official TypeScript SDK.
//!
//! The signed field projection and unusual Ed25519 message format follow
//! `@defuse-protocol/one-click-sdk-typescript` 0.1.25. Verification is
//! performed on bounded raw JSON so fields covered by the signature cannot be
//! discarded by a narrower response model first. Fractional and unsafe
//! numeric values fail closed until ECMAScript number formatting is pinned by
//! differential fixtures.

use std::collections::BTreeMap;
use std::fmt;

use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::{Number, Value};
use sha2::{Digest, Sha256};

/// Official SDK revision whose signed projection is implemented here.
pub const SIGNATURE_SDK_REVISION: &str = "ae28ef0348f616dd30c174cb22dd1b1126d8f76b";

/// Production manager key pinned by the official SDK at that revision.
pub const PRODUCTION_MANAGER_PUBLIC_KEY: &str =
    "ed25519:reYaWhvwu8Jzo3WUM3zhn6VrhuMEF4eADL17qtRVifc";

const ED25519_PREFIX: &str = "ed25519:";
const MAX_SAFE_JAVASCRIPT_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_SAFE_JAVASCRIPT_INTEGER_F64: f64 = 9_007_199_254_740_991.0;
const REQUIRED_REQUEST_FIELDS: &[(&str, JsonKind)] = &[
    ("dry", JsonKind::Boolean),
    ("swapType", JsonKind::String),
    ("slippageTolerance", JsonKind::Integer),
    ("originAsset", JsonKind::String),
    ("depositType", JsonKind::String),
    ("destinationAsset", JsonKind::String),
    ("amount", JsonKind::String),
    ("refundTo", JsonKind::String),
    ("refundType", JsonKind::String),
    ("recipient", JsonKind::String),
    ("recipientType", JsonKind::String),
    ("deadline", JsonKind::String),
];
const OPTIONAL_REQUEST_FIELDS: &[(&str, JsonKind)] = &[
    ("quoteWaitingTimeMs", JsonKind::Integer),
    ("referral", JsonKind::String),
    ("virtualChainRecipient", JsonKind::String),
    ("virtualChainRefundRecipient", JsonKind::String),
    ("customRecipientMsg", JsonKind::String),
];
const DRY_QUOTE_FIELDS: &[(&str, JsonKind)] = &[
    ("amountIn", JsonKind::String),
    ("amountInFormatted", JsonKind::String),
    ("amountInUsd", JsonKind::String),
    ("minAmountIn", JsonKind::String),
    ("amountOut", JsonKind::String),
    ("amountOutFormatted", JsonKind::String),
    ("amountOutUsd", JsonKind::String),
    ("minAmountOut", JsonKind::String),
];
const WET_QUOTE_FIELDS: &[(&str, JsonKind)] = &[
    ("depositAddress", JsonKind::String),
    ("depositMemo", JsonKind::String),
    ("deadline", JsonKind::String),
    ("timeWhenInactive", JsonKind::String),
    ("timeEstimate", JsonKind::Integer),
    ("virtualChainRecipient", JsonKind::String),
    ("virtualChainRefundRecipient", JsonKind::String),
    ("customRecipientMsg", JsonKind::String),
    ("refundFee", JsonKind::String),
    ("withdrawFee", JsonKind::String),
];

#[derive(Clone)]
pub struct QuoteSignatureVerifier {
    key: VerifyingKey,
}

impl fmt::Debug for QuoteSignatureVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuoteSignatureVerifier")
            .field("trust_root", &"pinned-production-key")
            .finish_non_exhaustive()
    }
}

impl QuoteSignatureVerifier {
    pub fn production() -> Result<Self, QuoteSignatureError> {
        Self::from_encoded_public_key(PRODUCTION_MANAGER_PUBLIC_KEY)
    }

    /// Verify a raw quote response before narrowing it to provider DTOs.
    pub fn verify(&self, response: &Value) -> Result<VerifiedQuote, QuoteSignatureError> {
        let response = response
            .as_object()
            .ok_or(QuoteSignatureError::InvalidDocument)?;
        let signature = required_string(response.get("signature"))?;
        let (digest, quote_hash) = quote_hash(response)?;
        let signature = decode_fixed::<64>(signature)?;
        let signature = Signature::from_bytes(&signature);
        self.key
            .verify_strict(quote_hash.as_bytes(), &signature)
            .map_err(|_| QuoteSignatureError::InvalidSignature)?;
        Ok(VerifiedQuote {
            document: Value::Object(response.clone()),
            digest,
        })
    }

    fn from_encoded_public_key(value: &str) -> Result<Self, QuoteSignatureError> {
        let bytes = decode_fixed::<32>(value).map_err(|_| QuoteSignatureError::InvalidPublicKey)?;
        let key =
            VerifyingKey::from_bytes(&bytes).map_err(|_| QuoteSignatureError::InvalidPublicKey)?;
        Ok(Self { key })
    }

    /// Test-only trust root for deterministic, unfunded fixtures.
    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Result<Self, QuoteSignatureError> {
        Self::from_encoded_public_key(value)
    }
}

/// Quote document whose official signed projection has been authenticated.
#[derive(Clone)]
pub struct VerifiedQuote {
    document: Value,
    digest: [u8; 32],
}

impl VerifiedQuote {
    pub fn document(&self) -> &Value {
        &self.document
    }

    pub fn quote_hash(&self) -> String {
        bs58::encode(self.digest).into_string()
    }
}

impl fmt::Debug for VerifiedQuote {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VerifiedQuote(<redacted>)")
    }
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum QuoteSignatureError {
    #[error("invalid 1Click quote-signature trust root")]
    InvalidPublicKey,
    #[error("invalid 1Click quote signature document")]
    InvalidDocument,
    #[error("unsupported number in 1Click signed quote projection")]
    UnsupportedNumber,
    #[error("invalid 1Click quote signature")]
    InvalidSignature,
}

#[derive(Clone, Copy)]
enum JsonKind {
    Boolean,
    Integer,
    String,
}

fn quote_hash(
    response: &serde_json::Map<String, Value>,
) -> Result<([u8; 32], String), QuoteSignatureError> {
    let request = required_object(response.get("quoteRequest"))?;
    let quote = required_object(response.get("quote"))?;
    let timestamp = required_typed(response.get("timestamp"), JsonKind::String)?;
    let dry = required_typed(request.get("dry"), JsonKind::Boolean)?
        .as_bool()
        .ok_or(QuoteSignatureError::InvalidDocument)?;

    let mut projection = BTreeMap::new();
    copy_required(&mut projection, request, REQUIRED_REQUEST_FIELDS)?;
    copy_truthy(&mut projection, request, OPTIONAL_REQUEST_FIELDS)?;
    copy_required(&mut projection, quote, DRY_QUOTE_FIELDS)?;
    if !dry {
        // Object spread writes these keys even when their value is undefined,
        // so a missing/falsy quote value removes a same-named request value.
        for (field, kind) in WET_QUOTE_FIELDS {
            projection.remove(*field);
            if let Some(value) = quote.get(*field).filter(|value| js_truthy(value)) {
                validate_kind(value, *kind)?;
                projection.insert((*field).to_owned(), canonical_scalar(value, *kind)?);
            }
        }
    }
    projection.insert(
        "timestamp".to_owned(),
        canonical_scalar(timestamp, JsonKind::String)?,
    );

    let canonical = serialize_projection(&projection);
    let digest: [u8; 32] = Sha256::digest(canonical.as_bytes()).into();
    let encoded = bs58::encode(digest).into_string();
    Ok((digest, encoded))
}

fn copy_required(
    destination: &mut BTreeMap<String, String>,
    source: &serde_json::Map<String, Value>,
    fields: &[(&str, JsonKind)],
) -> Result<(), QuoteSignatureError> {
    for (field, kind) in fields {
        let value = required_typed(source.get(*field), *kind)?;
        destination.insert((*field).to_owned(), canonical_scalar(value, *kind)?);
    }
    Ok(())
}

fn copy_truthy(
    destination: &mut BTreeMap<String, String>,
    source: &serde_json::Map<String, Value>,
    fields: &[(&str, JsonKind)],
) -> Result<(), QuoteSignatureError> {
    for (field, kind) in fields {
        if let Some(value) = source.get(*field).filter(|value| js_truthy(value)) {
            validate_kind(value, *kind)?;
            destination.insert((*field).to_owned(), canonical_scalar(value, *kind)?);
        }
    }
    Ok(())
}

fn required_object(
    value: Option<&Value>,
) -> Result<&serde_json::Map<String, Value>, QuoteSignatureError> {
    value
        .and_then(Value::as_object)
        .ok_or(QuoteSignatureError::InvalidDocument)
}

fn required_string(value: Option<&Value>) -> Result<&str, QuoteSignatureError> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(QuoteSignatureError::InvalidDocument)
}

fn required_typed(value: Option<&Value>, kind: JsonKind) -> Result<&Value, QuoteSignatureError> {
    let value = value.ok_or(QuoteSignatureError::InvalidDocument)?;
    validate_kind(value, kind)?;
    Ok(value)
}

fn validate_kind(value: &Value, kind: JsonKind) -> Result<(), QuoteSignatureError> {
    let valid = match kind {
        JsonKind::Boolean => value.is_boolean(),
        JsonKind::Integer => value.as_number().is_some_and(number_is_integer),
        JsonKind::String => value.is_string(),
    };
    if valid {
        Ok(())
    } else if matches!(kind, JsonKind::Integer) && value.is_number() {
        Err(QuoteSignatureError::UnsupportedNumber)
    } else {
        Err(QuoteSignatureError::InvalidDocument)
    }
}

fn canonical_scalar(value: &Value, kind: JsonKind) -> Result<String, QuoteSignatureError> {
    match kind {
        JsonKind::Boolean => Ok(if value.as_bool() == Some(true) {
            "true".to_owned()
        } else {
            "false".to_owned()
        }),
        JsonKind::Integer => canonical_integer(
            value
                .as_number()
                .ok_or(QuoteSignatureError::InvalidDocument)?,
        ),
        JsonKind::String => {
            serde_json::to_string(value.as_str().ok_or(QuoteSignatureError::InvalidDocument)?)
                .map_err(|_| QuoteSignatureError::InvalidDocument)
        }
    }
}

fn canonical_integer(number: &Number) -> Result<String, QuoteSignatureError> {
    if let Some(value) = number.as_i64() {
        if value.unsigned_abs() <= MAX_SAFE_JAVASCRIPT_INTEGER {
            return Ok(value.to_string());
        }
        return Err(QuoteSignatureError::UnsupportedNumber);
    }
    if let Some(value) = number.as_u64() {
        if value <= MAX_SAFE_JAVASCRIPT_INTEGER {
            return Ok(value.to_string());
        }
        return Err(QuoteSignatureError::UnsupportedNumber);
    }
    let value = number
        .as_f64()
        .filter(|value| {
            value.is_finite()
                && value.fract() == 0.0
                && value.abs() <= MAX_SAFE_JAVASCRIPT_INTEGER_F64
        })
        .ok_or(QuoteSignatureError::UnsupportedNumber)?;
    if value == 0.0 {
        Ok("0".to_owned())
    } else {
        Ok(format!("{value:.0}"))
    }
}

fn number_is_integer(number: &Number) -> bool {
    number
        .as_i64()
        .is_some_and(|value| value.unsigned_abs() <= MAX_SAFE_JAVASCRIPT_INTEGER)
        || number
            .as_u64()
            .is_some_and(|value| value <= MAX_SAFE_JAVASCRIPT_INTEGER)
        || number.as_f64().is_some_and(|value| {
            value.is_finite()
                && value.fract() == 0.0
                && value.abs() <= MAX_SAFE_JAVASCRIPT_INTEGER_F64
        })
}

fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn serialize_projection(projection: &BTreeMap<String, String>) -> String {
    let fields = projection
        .iter()
        // Projection keys come exclusively from the fixed ASCII field tables.
        .map(|(key, value)| format!("\"{key}\":{value}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{fields}}}")
}

fn decode_fixed<const LENGTH: usize>(value: &str) -> Result<[u8; LENGTH], QuoteSignatureError> {
    let encoded = value.strip_prefix(ED25519_PREFIX).unwrap_or(value);
    let decoded = bs58::decode(encoded)
        .into_vec()
        .map_err(|_| QuoteSignatureError::InvalidDocument)?;
    decoded
        .try_into()
        .map_err(|_| QuoteSignatureError::InvalidDocument)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAGING_MANAGER_PUBLIC_KEY: &str = "ed25519:5J5tkaxyPoR3Q9S8LXfo5bWnXK5Z2bctJ4mB9gENh7co";

    fn official_wet_quote() -> Value {
        serde_json::json!({
            "correlationId": "d4f1b110-46cc-4682-aa3f-44d81ffe4b80",
            "timestamp": "2026-06-23T17:10:41.104Z",
            "signature": "ed25519:53wcpim7FDNLbBHVezUpakthWq2TR9Lag3PwW3e8Cxmz4bFEodcc4rui5BiVHRRaHocYE9URVapzJD8JxLNDs8K9",
            "quoteRequest": {
                "dry": false,
                "depositMode": "SIMPLE",
                "swapType": "EXACT_INPUT",
                "slippageTolerance": 100,
                "originAsset": "1cs_v1:btc:native:coin",
                "depositType": "ORIGIN_CHAIN",
                "destinationAsset": "nep141:eth-0xdac17f958d2ee523a2206206994597c13d831ec7.stft.near",
                "amount": "10000",
                "refundTo": "bc1q6mte80265ghwq4vsrpm9lnaz46uvdreu9z8wly",
                "refundType": "ORIGIN_CHAIN",
                "recipient": "0xcac3C41676deF4FE375E57118f3eB83A99105577",
                "recipientType": "DESTINATION_CHAIN",
                "deadline": "2026-06-23T19:00:00.000Z",
                "confidentiality": "public",
                "quoteWaitingTimeMs": 0,
                "appFees": [{
                    "recipient": "5880ad2b362620fadf759cbceb1cd5737ce8c6ed7fb8e9942881e6731f9247dd",
                    "fee": 10
                }]
            },
            "quote": {
                "amountIn": "10000",
                "amountInFormatted": "0.0001",
                "amountInUsd": "6.237600000000",
                "minAmountIn": "10000",
                "amountOut": "5931560",
                "amountOutFormatted": "5.93156",
                "amountOutUsd": "5.925171709880",
                "minAmountOut": "5872244",
                "timeEstimate": 812,
                "refundFee": "1900",
                "withdrawFee": "300000",
                "deadline": "2026-06-26T19:00:00.000Z",
                "timeWhenInactive": "2026-06-26T19:00:00.000Z",
                "depositAddress": "bc1q873cxltdc560dth6tpwqpehq9uvhxxcdgwnmnw"
            }
        })
    }

    fn production_dry_quote() -> Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/oneclick-production-dry-exact-output-2026-09-04.json"
        ))
        .unwrap_or_else(|_| std::process::abort())
    }

    fn deterministic_wet_quote() -> Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/deterministic-wet-exact-output.json"
        ))
        .unwrap_or_else(|_| std::process::abort())
    }

    #[test]
    fn verifies_official_sdk_0_1_25_wet_quote_vector() {
        let verifier = QuoteSignatureVerifier::from_encoded_public_key(STAGING_MANAGER_PUBLIC_KEY)
            .unwrap_or_else(|_| std::process::abort());
        let result = verifier.verify(&official_wet_quote());
        assert!(result.is_ok());
        assert_eq!(
            result.map(|quote| quote.quote_hash()).ok(),
            Some("XS2Ej8EbPHKiDBfxaFY3y6az5pCDb8eh4bSdAErvZy7".to_owned())
        );
    }

    #[test]
    fn verifies_sanitized_production_dry_quote_with_production_key() {
        let verifier =
            QuoteSignatureVerifier::production().unwrap_or_else(|_| std::process::abort());
        let result = verifier.verify(&production_dry_quote());
        assert!(result.is_ok());
        assert_eq!(
            result.map(|quote| quote.quote_hash()).ok(),
            Some("GYtm1avcPcvNRnPZanKBx1RB541cHnhnLpTSomEmpUn3".to_owned())
        );
    }

    #[test]
    fn verifies_deterministic_unfunded_exact_output_fixture() {
        // DO NOT FUND: this public key belongs only to the deterministic test
        // fixture and is never accepted by production construction.
        let verifier = QuoteSignatureVerifier::for_test(
            "ed25519:9C6hybhQ6Aycep9jaUnP6uL9ZYvDjUp1aSkFWPUFJtpj",
        )
        .unwrap_or_else(|_| std::process::abort());
        let result = verifier.verify(&deterministic_wet_quote());
        assert_eq!(
            result.map(|quote| quote.quote_hash()).ok(),
            Some("3Nnstyx8CZPxpBMdN2QpPxGH1tNxiud858Z8LBtHVAoL".to_owned())
        );
    }

    #[test]
    fn production_signature_binds_route_refund_and_deadline() {
        let verifier =
            QuoteSignatureVerifier::production().unwrap_or_else(|_| std::process::abort());
        for (field, value) in [
            ("recipient", Value::String("attacker.near".to_owned())),
            ("refundTo", Value::String("attacker.near".to_owned())),
            (
                "deadline",
                Value::String("2026-09-04T22:39:57.000Z".to_owned()),
            ),
        ] {
            let mut quote = production_dry_quote();
            quote["quoteRequest"][field] = value;
            assert_eq!(
                verifier.verify(&quote).err(),
                Some(QuoteSignatureError::InvalidSignature)
            );
        }
    }

    #[test]
    fn rejects_a_mutated_deposit_address_or_amount() {
        let verifier = QuoteSignatureVerifier::from_encoded_public_key(STAGING_MANAGER_PUBLIC_KEY)
            .unwrap_or_else(|_| std::process::abort());
        for (field, value) in [
            ("depositAddress", Value::String("attacker".to_owned())),
            ("amountOut", Value::String("1".to_owned())),
        ] {
            let mut quote = official_wet_quote();
            quote["quote"][field] = value;
            assert_eq!(
                verifier.verify(&quote).err(),
                Some(QuoteSignatureError::InvalidSignature)
            );
        }
    }

    #[test]
    fn ignores_unsigned_diagnostics_but_binds_timestamp() {
        let verifier = QuoteSignatureVerifier::from_encoded_public_key(STAGING_MANAGER_PUBLIC_KEY)
            .unwrap_or_else(|_| std::process::abort());
        let mut diagnostics = official_wet_quote();
        diagnostics["correlationId"] = Value::String("status-correlation".to_owned());
        diagnostics["quoteRequest"]["appFees"] = serde_json::json!([]);
        assert!(verifier.verify(&diagnostics).is_ok());

        diagnostics["timestamp"] = Value::String("2026-06-23T17:10:42.104Z".to_owned());
        assert_eq!(
            verifier.verify(&diagnostics).err(),
            Some(QuoteSignatureError::InvalidSignature)
        );
    }

    #[test]
    fn rejects_fractional_numbers_until_js_serialization_is_pinned() {
        let verifier = QuoteSignatureVerifier::from_encoded_public_key(STAGING_MANAGER_PUBLIC_KEY)
            .unwrap_or_else(|_| std::process::abort());
        let mut quote = official_wet_quote();
        quote["quote"]["timeEstimate"] = serde_json::json!(812.5);
        assert_eq!(
            verifier.verify(&quote).err(),
            Some(QuoteSignatureError::UnsupportedNumber)
        );
    }

    #[test]
    fn rejects_integers_that_javascript_cannot_represent_exactly() {
        let verifier = QuoteSignatureVerifier::from_encoded_public_key(STAGING_MANAGER_PUBLIC_KEY)
            .unwrap_or_else(|_| std::process::abort());
        let mut quote = official_wet_quote();
        quote["quote"]["timeEstimate"] = serde_json::json!(9_007_199_254_740_992_u64);
        assert_eq!(
            verifier.verify(&quote).err(),
            Some(QuoteSignatureError::UnsupportedNumber)
        );
    }

    #[test]
    fn production_key_is_well_formed_and_debug_output_is_redacted() {
        let verifier = QuoteSignatureVerifier::production();
        assert!(verifier.is_ok());
        let Some(verifier) = verifier.ok() else {
            std::process::abort();
        };
        assert_eq!(
            format!("{verifier:?}"),
            "QuoteSignatureVerifier { trust_root: \"pinned-production-key\", .. }"
        );
        assert_eq!(
            format!("{:?}", verifier.verify(&official_wet_quote())),
            "Err(InvalidSignature)"
        );
    }
}
