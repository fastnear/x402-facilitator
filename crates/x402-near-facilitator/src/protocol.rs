use std::{cmp::Ordering, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use x402_types::proto;

use crate::config::PaymentIdentifierConfig;
use crate::v1_compat::{self, WireVersion};

pub const PAYMENT_IDENTIFIER_EXTENSION: &str = "payment-identifier";
const REQUEST_FINGERPRINT_DOMAIN: &[u8] = b"x402-near/request/v1\0";
const AUTHORIZATION_FLOW: &str = "authorization";
const EIP3009_METHOD: &str = "eip3009";
const NEAR_INTENTS_METHOD: &str = "near-intents";
const UPFRONT_FLOW: &str = "upfront";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettlementRoute {
    Direct,
    NearIntentsUpfrontDisabled,
    Unsupported,
}

#[derive(Clone)]
pub struct ParsedRequest {
    pub raw: proto::VerifyRequest,
    pub value: Value,
    /// The wire dialect the request arrived in. `raw`/`value`/`meta` are
    /// always canonical v2 (v1 requests are translated before parsing); the
    /// tag only drives response formatting.
    pub wire: WireVersion,
    pub meta: RequestMeta,
}

impl fmt::Debug for ParsedRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParsedRequest")
            .field("raw", &"<redacted>")
            .field("value", &"<redacted>")
            .field("wire", &self.wire)
            .field("meta", &self.meta)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct RequestMeta {
    pub x402_version: u8,
    pub scheme: String,
    pub network: String,
    pub asset: String,
    pub amount: String,
    pub pay_to: String,
    /// NEAR only: the base64 `SignedDelegateAction`. `None` for eip155, whose
    /// mechanism payload is an ERC-3009 authorization plus the payer signature.
    pub signed_delegate_action: Option<String>,
    /// Closed route classification performed before mechanism payload parsing.
    /// The draft NEAR Intents `upfront` route is recognized across origin
    /// networks but remains disabled until its upstream contract is merged.
    pub settlement_route: SettlementRoute,
    pub payment_identifier: Option<String>,
}

impl fmt::Debug for RequestMeta {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RequestMeta")
            .field("x402_version", &self.x402_version)
            .field("scheme", &self.scheme)
            .field("network", &self.network)
            .field("asset", &"<redacted>")
            .field("amount", &"<redacted>")
            .field("pay_to", &"<redacted>")
            .field("signed_delegate_action", &"<redacted>")
            .field("settlement_route", &self.settlement_route)
            .field("payment_identifier", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResponse {
    pub is_valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Map<String, Value>>,
}

impl fmt::Debug for VerifyResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifyResponse")
            .field("is_valid", &self.is_valid)
            .field("invalid_reason", &self.invalid_reason)
            .field("invalid_message", &"<redacted>")
            .field("payer", &"<redacted>")
            .field("extensions", &"<redacted>")
            .finish()
    }
}

impl VerifyResponse {
    pub fn valid(payer: String) -> Self {
        Self {
            is_valid: true,
            invalid_reason: None,
            invalid_message: None,
            payer: Some(payer),
            extensions: None,
        }
    }

    pub fn invalid(
        reason: impl Into<String>,
        message: Option<String>,
        payer: Option<String>,
    ) -> Self {
        Self {
            is_valid: false,
            invalid_reason: Some(reason.into()),
            invalid_message: message,
            payer,
            extensions: None,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettleResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
    pub transaction: String,
    pub network: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Map<String, Value>>,
}

impl fmt::Debug for SettleResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SettleResponse")
            .field("success", &self.success)
            .field("error_reason", &self.error_reason)
            .field("error_message", &"<redacted>")
            .field("payer", &"<redacted>")
            .field("transaction", &"<redacted>")
            .field("network", &self.network)
            .field("extensions", &"<redacted>")
            .finish()
    }
}

impl SettleResponse {
    pub fn success(payer: String, transaction: String, network: String) -> Self {
        Self {
            success: true,
            error_reason: None,
            error_message: None,
            payer: Some(payer),
            transaction,
            network,
            extensions: None,
        }
    }

    pub fn failure(
        reason: impl Into<String>,
        message: Option<String>,
        payer: Option<String>,
        transaction: String,
        network: String,
    ) -> Self {
        Self {
            success: false,
            error_reason: Some(reason.into()),
            error_message: message,
            payer,
            transaction,
            network,
            extensions: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportedKind {
    pub x402_version: u8,
    pub scheme: String,
    pub network: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportedResponse {
    pub kinds: Vec<SupportedKind>,
    pub extensions: Vec<String>,
    pub signers: std::collections::HashMap<String, Vec<String>>,
}

impl fmt::Debug for SupportedResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SupportedResponse")
            .field("kinds", &self.kinds)
            .field("extensions", &self.extensions)
            .field("signers", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("request body must be a JSON object")]
    NotObject,
    #[error("missing or invalid field {0}")]
    Field(&'static str),
    #[error("malformed payment-identifier extension: {0}")]
    PaymentIdentifier(String),
    #[error("failed to preserve request JSON: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn parse_request(
    body: &[u8],
    identifier_policy: &PaymentIdentifierConfig,
    accept_v1: bool,
) -> Result<ParsedRequest, RequestError> {
    let mut value: Value = serde_json::from_slice(body)?;
    let wire = if accept_v1 && value.as_object().is_some_and(v1_compat::is_v1_request) {
        WireVersion::V1
    } else {
        WireVersion::V2
    };
    if wire == WireVersion::V1 {
        let translated = {
            let object = value.as_object().ok_or(RequestError::NotObject)?;
            v1_compat::translate_v1_to_v2(object)?
        };
        value = translated;
    }
    let object = value.as_object().ok_or(RequestError::NotObject)?;
    ensure_allowed_keys(
        object,
        &["x402Version", "paymentPayload", "paymentRequirements"],
        "request",
    )?;
    let x402_version = object
        .get("x402Version")
        .and_then(Value::as_u64)
        .and_then(|value| u8::try_from(value).ok())
        .ok_or(RequestError::Field("x402Version"))?;
    let payload = object
        .get("paymentPayload")
        .and_then(Value::as_object)
        .ok_or(RequestError::Field("paymentPayload"))?;
    ensure_allowed_keys(
        payload,
        &[
            "x402Version",
            "resource",
            "accepted",
            "payload",
            "extensions",
        ],
        "paymentPayload",
    )?;
    required_u8(payload, "x402Version", "paymentPayload.x402Version")?;
    validate_resource(payload.get("resource"))?;
    let requirements = object
        .get("paymentRequirements")
        .and_then(Value::as_object)
        .ok_or(RequestError::Field("paymentRequirements"))?;
    validate_requirements_shape(requirements, "paymentRequirements")?;
    let accepted = payload
        .get("accepted")
        .and_then(Value::as_object)
        .ok_or(RequestError::Field("paymentPayload.accepted"))?;
    validate_requirements_shape(accepted, "paymentPayload.accepted")?;
    let mechanism_payload = payload
        .get("payload")
        .and_then(Value::as_object)
        .ok_or(RequestError::Field("paymentPayload.payload"))?;
    if let Some(extensions) = payload.get("extensions")
        && !extensions.is_object()
    {
        return Err(RequestError::Field("paymentPayload.extensions"));
    }

    let scheme = required_string(requirements, "scheme", "paymentRequirements.scheme")?;
    let network = required_string(requirements, "network", "paymentRequirements.network")?;
    let asset = required_string(requirements, "asset", "paymentRequirements.asset")?;
    let amount = required_decimal_string(requirements, "amount", "paymentRequirements.amount")?;
    let pay_to = required_string(requirements, "payTo", "paymentRequirements.payTo")?;

    // These fields must at least have the expected scalar shape at the HTTP
    // boundary. Exact equality and chain semantics remain the mechanism's job.
    required_string(accepted, "scheme", "paymentPayload.accepted.scheme")?;
    required_string(accepted, "network", "paymentPayload.accepted.network")?;
    required_string(accepted, "asset", "paymentPayload.accepted.asset")?;
    required_decimal_string(accepted, "amount", "paymentPayload.accepted.amount")?;
    required_string(accepted, "payTo", "paymentPayload.accepted.payTo")?;

    let (signed_delegate_action, settlement_route) =
        parse_routed_mechanism_payload(accepted, requirements, mechanism_payload, &network, wire)?;

    let payment_identifier = extract_payment_identifier(payload, identifier_policy)?;
    let raw = proto::VerifyRequest::from(serde_json::value::to_raw_value(&value)?);
    Ok(ParsedRequest {
        raw,
        value,
        wire,
        meta: RequestMeta {
            x402_version,
            scheme,
            network,
            asset,
            amount,
            pay_to,
            signed_delegate_action,
            settlement_route,
            payment_identifier,
        },
    })
}

fn parse_routed_mechanism_payload(
    accepted: &Map<String, Value>,
    requirements: &Map<String, Value>,
    mechanism_payload: &Map<String, Value>,
    network: &str,
    wire: WireVersion,
) -> Result<(Option<String>, SettlementRoute), RequestError> {
    let accepted_selector = parse_settlement_selector(
        accepted,
        "paymentPayload.accepted.extra.assetTransferMethod",
        "paymentPayload.accepted.extra.paymentFlow",
    )?;
    let requirements_selector = parse_settlement_selector(
        requirements,
        "paymentRequirements.extra.assetTransferMethod",
        "paymentRequirements.extra.paymentFlow",
    )?;

    let draft_selector_seen = [accepted_selector, requirements_selector]
        .into_iter()
        .any(|(method, flow)| method == Some(NEAR_INTENTS_METHOD) || flow == Some(UPFRONT_FLOW));
    let route = if draft_selector_seen {
        let exact_draft_pair = accepted_selector == (Some(NEAR_INTENTS_METHOD), Some(UPFRONT_FLOW))
            && requirements_selector == accepted_selector;
        if exact_draft_pair && wire == WireVersion::V2 {
            SettlementRoute::NearIntentsUpfrontDisabled
        } else {
            SettlementRoute::Unsupported
        }
    } else if direct_selector_supported(network, accepted_selector)
        && direct_selector_supported(network, requirements_selector)
    {
        SettlementRoute::Direct
    } else {
        SettlementRoute::Unsupported
    };
    let signed_delegate = if route == SettlementRoute::Direct {
        parse_mechanism_payload(mechanism_payload, network)?
    } else {
        None
    };
    Ok((signed_delegate, route))
}

fn direct_selector_supported(network: &str, (method, flow): (Option<&str>, Option<&str>)) -> bool {
    if network.starts_with("near:") {
        method.is_none() && matches!(flow, None | Some(AUTHORIZATION_FLOW))
    } else if network.starts_with("eip155:") {
        matches!(method, None | Some(EIP3009_METHOD))
            && matches!(flow, None | Some(AUTHORIZATION_FLOW))
    } else {
        method.is_none() && flow.is_none()
    }
}

fn parse_settlement_selector<'a>(
    requirements: &'a Map<String, Value>,
    method_path: &'static str,
    flow_path: &'static str,
) -> Result<(Option<&'a str>, Option<&'a str>), RequestError> {
    let Some(extra) = requirements.get("extra") else {
        return Ok((None, None));
    };
    let extra = extra.as_object().ok_or(RequestError::Field(method_path))?;
    let method = match extra.get("assetTransferMethod") {
        None => Ok(None),
        Some(Value::String(method)) => Ok(Some(method.as_str())),
        Some(_) => Err(RequestError::Field(method_path)),
    }?;
    let flow = match extra.get("paymentFlow") {
        None => Ok(None),
        Some(Value::String(flow)) => Ok(Some(flow.as_str())),
        Some(_) => Err(RequestError::Field(flow_path)),
    }?;
    Ok((method, flow))
}

/// Validate the chain-specific mechanism payload (`paymentPayload.payload`) and
/// return the NEAR base64 `SignedDelegateAction`, or `None` for eip155. NEAR
/// carries a signed delegate; eip155 carries an ERC-3009 `authorization` object
/// plus the payer `signature`. The HTTP boundary denies unknown authorization
/// fields and enforces lossless scalar encodings; address/signature semantics
/// remain the upstream eip155 provider's job.
fn parse_mechanism_payload(
    mechanism_payload: &Map<String, Value>,
    network: &str,
) -> Result<Option<String>, RequestError> {
    if network.starts_with("eip155:") {
        ensure_allowed_keys(
            mechanism_payload,
            &["authorization", "signature"],
            "paymentPayload.payload",
        )?;
        let authorization = mechanism_payload
            .get("authorization")
            .and_then(Value::as_object)
            .ok_or(RequestError::Field("paymentPayload.payload.authorization"))?;
        validate_eip155_authorization(authorization)?;
        required_string(
            mechanism_payload,
            "signature",
            "paymentPayload.payload.signature",
        )?;
        Ok(None)
    } else {
        ensure_allowed_keys(
            mechanism_payload,
            &["signedDelegateAction"],
            "paymentPayload.payload",
        )?;
        Ok(Some(required_string(
            mechanism_payload,
            "signedDelegateAction",
            "paymentPayload.payload.signedDelegateAction",
        )?))
    }
}

fn validate_eip155_authorization(authorization: &Map<String, Value>) -> Result<(), RequestError> {
    const PATH: &str = "paymentPayload.payload.authorization";
    ensure_allowed_keys(
        authorization,
        &["from", "to", "value", "validAfter", "validBefore", "nonce"],
        PATH,
    )?;
    required_string(authorization, "from", PATH)?;
    required_string(authorization, "to", PATH)?;
    required_decimal_string(authorization, "value", PATH)?;
    required_decimal_string(authorization, "validAfter", PATH)?;
    required_decimal_string(authorization, "validBefore", PATH)?;
    let nonce = required_string(authorization, "nonce", PATH)?;
    let valid_nonce = nonce
        .strip_prefix("0x")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()));
    if !valid_nonce {
        return Err(RequestError::Field(PATH));
    }
    Ok(())
}

pub fn request_fingerprint(
    request: &Value,
    payment_hash: &[u8; 32],
) -> Result<[u8; 32], serde_json::Error> {
    let mut canonical = Vec::new();
    write_canonical_json(request, &mut canonical)?;
    let mut hash = Sha256::new();
    hash.update(REQUEST_FINGERPRINT_DOMAIN);
    hash.update(canonical);
    hash.update(payment_hash);
    Ok(hash.finalize().into())
}

pub fn decimal_is_at_least(value: &str, minimum: &str) -> bool {
    compare_decimal(value, minimum) != Ordering::Less
}

pub fn validate_identifier(
    identifier: &str,
    policy: &PaymentIdentifierConfig,
) -> Result<(), RequestError> {
    if identifier.len() < policy.min_length || identifier.len() > policy.max_length {
        return Err(RequestError::PaymentIdentifier(format!(
            "id length must be {}..={}",
            policy.min_length, policy.max_length
        )));
    }
    if !identifier
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(RequestError::PaymentIdentifier(
            "id contains characters outside [A-Za-z0-9_-]".to_owned(),
        ));
    }
    Ok(())
}

fn extract_payment_identifier(
    payload: &Map<String, Value>,
    policy: &PaymentIdentifierConfig,
) -> Result<Option<String>, RequestError> {
    let extension = payload
        .get("extensions")
        .and_then(Value::as_object)
        .and_then(|extensions| extensions.get(PAYMENT_IDENTIFIER_EXTENSION));
    let Some(extension) = extension else {
        if policy.required {
            return Err(RequestError::PaymentIdentifier(
                "an id is required by this facilitator".to_owned(),
            ));
        }
        return Ok(None);
    };
    let extension = extension
        .as_object()
        .ok_or_else(|| RequestError::PaymentIdentifier("extension must be an object".to_owned()))?;
    ensure_allowed_keys(
        extension,
        &["info", "schema"],
        "paymentPayload.extensions.payment-identifier",
    )?;
    let info = extension
        .get("info")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            RequestError::PaymentIdentifier("extension.info must be an object".to_owned())
        })?;
    ensure_allowed_keys(
        info,
        &["required", "id"],
        "paymentPayload.extensions.payment-identifier.info",
    )?;
    if info.get("required").and_then(Value::as_bool).is_none() {
        return Err(RequestError::PaymentIdentifier(
            "extension.info.required must be a boolean".to_owned(),
        ));
    }
    if !extension.get("schema").is_some_and(Value::is_object) {
        return Err(RequestError::PaymentIdentifier(
            "extension.schema must be an object".to_owned(),
        ));
    }
    let identifier = info.get("id");
    match identifier {
        Some(Value::String(identifier)) => {
            validate_identifier(identifier, policy)?;
            Ok(Some(identifier.clone()))
        }
        Some(_) => Err(RequestError::PaymentIdentifier(
            "extension.info.id must be a string".to_owned(),
        )),
        None if policy.required => Err(RequestError::PaymentIdentifier(
            "an id is required by this facilitator".to_owned(),
        )),
        None => Ok(None),
    }
}

fn validate_requirements_shape(
    object: &Map<String, Value>,
    path: &'static str,
) -> Result<(), RequestError> {
    ensure_allowed_keys(
        object,
        &[
            "scheme",
            "network",
            "asset",
            "amount",
            "payTo",
            "maxTimeoutSeconds",
            "extra",
        ],
        path,
    )?;
    if object
        .get("maxTimeoutSeconds")
        .and_then(Value::as_u64)
        .filter(|timeout| *timeout > 0)
        .is_none()
    {
        return Err(RequestError::Field(path));
    }
    if object.get("extra").is_some_and(|extra| !extra.is_object()) {
        return Err(RequestError::Field(path));
    }
    Ok(())
}

fn validate_resource(resource: Option<&Value>) -> Result<(), RequestError> {
    let Some(resource) = resource else {
        return Ok(());
    };
    let resource = resource
        .as_object()
        .ok_or(RequestError::Field("paymentPayload.resource"))?;
    ensure_allowed_keys(
        resource,
        &[
            "url",
            "description",
            "mimeType",
            "serviceName",
            "tags",
            "iconUrl",
        ],
        "paymentPayload.resource",
    )?;
    required_string(resource, "url", "paymentPayload.resource.url")?;
    for field in ["description", "mimeType", "serviceName", "iconUrl"] {
        if resource.get(field).is_some_and(|value| !value.is_string()) {
            return Err(RequestError::Field("paymentPayload.resource"));
        }
    }
    if let Some(tags) = resource.get("tags") {
        let tags = tags
            .as_array()
            .ok_or(RequestError::Field("paymentPayload.resource.tags"))?;
        if tags.iter().any(|tag| !tag.is_string()) {
            return Err(RequestError::Field("paymentPayload.resource.tags"));
        }
    }
    Ok(())
}

pub(crate) fn ensure_allowed_keys(
    object: &Map<String, Value>,
    allowed: &[&str],
    path: &'static str,
) -> Result<(), RequestError> {
    if object
        .keys()
        .any(|key| !allowed.iter().any(|allowed| key == allowed))
    {
        return Err(RequestError::Field(path));
    }
    Ok(())
}

fn required_u8(
    object: &Map<String, Value>,
    key: &'static str,
    path: &'static str,
) -> Result<u8, RequestError> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u8::try_from(value).ok())
        .ok_or(RequestError::Field(path))
}

pub(crate) fn required_string(
    object: &Map<String, Value>,
    key: &'static str,
    path: &'static str,
) -> Result<String, RequestError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or(RequestError::Field(path))
}

fn required_decimal_string(
    object: &Map<String, Value>,
    key: &'static str,
    path: &'static str,
) -> Result<String, RequestError> {
    let value = required_string(object, key, path)?;
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(value)
    } else {
        Err(RequestError::Field(path))
    }
}

fn compare_decimal(left: &str, right: &str) -> Ordering {
    let left = left.trim_start_matches('0');
    let right = right.trim_start_matches('0');
    let left = if left.is_empty() { "0" } else { left };
    let right = if right.is_empty() { "0" } else { right };
    left.len()
        .cmp(&right.len())
        .then_with(|| left.as_bytes().cmp(right.as_bytes()))
}

fn write_canonical_json(value: &Value, output: &mut Vec<u8>) -> Result<(), serde_json::Error> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(true) => output.extend_from_slice(b"true"),
        Value::Bool(false) => output.extend_from_slice(b"false"),
        Value::Number(number) => output.extend_from_slice(number.to_string().as_bytes()),
        Value::String(string) => {
            output.extend_from_slice(serde_json::to_string(string)?.as_bytes());
        }
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(b']');
        }
        Value::Object(object) => {
            output.push(b'{');
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                output.extend_from_slice(serde_json::to_string(key)?.as_bytes());
                output.push(b':');
                if let Some(child) = object.get(key) {
                    write_canonical_json(child, output)?;
                }
            }
            output.push(b'}');
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredProtocolResponse {
    pub success: Option<bool>,
    pub is_valid: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_value() -> Value {
        serde_json::json!({
            "x402Version": 2,
            "paymentPayload": {
                "x402Version": 2,
                "accepted": {
                    "scheme": "exact",
                    "network": "near:testnet",
                    "asset": "usdc.fakes.testnet",
                    "amount": "1000",
                    "payTo": "merchant.testnet",
                    "maxTimeoutSeconds": 60,
                    "extra": {},
                },
                "payload": {
                    "signedDelegateAction": "c2lnbmVkLWRlbGVnYXRl",
                },
                "extensions": {
                    "payment-identifier": {
                        "info": {
                            "required": true,
                            "id": "payment_1234567890123456",
                        },
                        "schema": {},
                    },
                },
            },
            "paymentRequirements": {
                "scheme": "exact",
                "network": "near:testnet",
                "asset": "usdc.fakes.testnet",
                "amount": "1000",
                "payTo": "merchant.testnet",
                "maxTimeoutSeconds": 60,
                "extra": {},
            },
        })
    }

    fn encoded(value: &Value) -> Vec<u8> {
        serde_json::to_vec(value).unwrap_or_else(|_| std::process::abort())
    }

    #[test]
    fn identifier_validation_matches_extension_spec() {
        let policy = PaymentIdentifierConfig::default();
        assert!(validate_identifier("pay_123456789012", &policy).is_ok());
        assert!(validate_identifier("too-short", &policy).is_err());
        assert!(validate_identifier("pay_1234567890/$", &policy).is_err());
    }

    #[test]
    fn canonical_fingerprint_ignores_object_key_order() {
        let left = serde_json::json!({"b": 2, "a": {"d": 4, "c": 3}});
        let right = serde_json::json!({"a": {"c": 3, "d": 4}, "b": 2});
        let payment_hash = [7_u8; 32];
        assert_eq!(
            request_fingerprint(&left, &payment_hash).ok(),
            request_fingerprint(&right, &payment_hash).ok()
        );
    }

    #[test]
    fn canonical_json_is_valid_and_has_sorted_object_keys() {
        let mut output = Vec::new();
        assert!(write_canonical_json(&serde_json::json!({"b": 2, "a": 1}), &mut output).is_ok());
        assert_eq!(output, br#"{"a":1,"b":2}"#);
        assert!(serde_json::from_slice::<Value>(&output).is_ok());
    }

    #[test]
    fn request_shape_rejects_unknown_and_missing_nested_fields() {
        let policy = PaymentIdentifierConfig::default();
        let valid = request_value();
        assert!(parse_request(&encoded(&valid), &policy, false).is_ok());

        let mut unknown_top = valid.clone();
        unknown_top["unexpected"] = Value::Bool(true);
        assert!(parse_request(&encoded(&unknown_top), &policy, false).is_err());

        let mut unknown_payload = valid.clone();
        unknown_payload["paymentPayload"]["unexpected"] = Value::Bool(true);
        assert!(parse_request(&encoded(&unknown_payload), &policy, false).is_err());

        let mut missing_nested_version = valid;
        if let Some(payload) = missing_nested_version
            .get_mut("paymentPayload")
            .and_then(Value::as_object_mut)
        {
            payload.remove("x402Version");
        }
        assert!(parse_request(&encoded(&missing_nested_version), &policy, false).is_err());
    }

    fn eip155_request_value() -> Value {
        serde_json::json!({
            "x402Version": 2,
            "paymentPayload": {
                "x402Version": 2,
                "accepted": {
                    "scheme": "exact",
                    "network": "eip155:84532",
                    "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                    "amount": "1000",
                    "payTo": "0xA2AcB5d3aC1c35999532624188470eC6228039Dc",
                    "maxTimeoutSeconds": 60,
                    "extra": { "name": "USDC", "version": "2" },
                },
                "payload": {
                    "authorization": {
                        "from": "0x11eFA374c489D106F9B6AC1b9b73a7A54C237c6D",
                        "to": "0xA2AcB5d3aC1c35999532624188470eC6228039Dc",
                        "value": "1000",
                        "validAfter": "0",
                        "validBefore": "9999999999",
                        "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001",
                    },
                    "signature": "0xdeadbeef",
                },
            },
            "paymentRequirements": {
                "scheme": "exact",
                "network": "eip155:84532",
                "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                "amount": "1000",
                "payTo": "0xA2AcB5d3aC1c35999532624188470eC6228039Dc",
                "maxTimeoutSeconds": 60,
                "extra": { "name": "USDC", "version": "2" },
            },
        })
    }

    #[test]
    fn eip155_payload_parses_with_authorization_and_no_signed_delegate() {
        let policy = PaymentIdentifierConfig::default();
        let Ok(parsed) = parse_request(&encoded(&eip155_request_value()), &policy, false) else {
            std::process::abort();
        };
        assert_eq!(parsed.meta.network, "eip155:84532");
        assert!(parsed.meta.signed_delegate_action.is_none());
    }

    #[test]
    fn direct_eip155_accepts_only_its_default_or_explicit_selectors() {
        let policy = PaymentIdentifierConfig::default();
        let selectors = [
            (None, None),
            (Some("eip3009"), None),
            (None, Some("authorization")),
            (Some("eip3009"), Some("authorization")),
        ];
        for (accepted_selector, requirements_selector) in
            selectors.into_iter().flat_map(|accepted| {
                selectors
                    .into_iter()
                    .map(move |requirements| (accepted, requirements))
            })
        {
            let mut request = eip155_request_value();
            for (pointer, (method, flow)) in [
                ("/paymentPayload/accepted/extra", accepted_selector),
                ("/paymentRequirements/extra", requirements_selector),
            ] {
                let Some(extra) = request.pointer_mut(pointer) else {
                    std::process::abort();
                };
                if let Some(method) = method {
                    extra["assetTransferMethod"] = Value::String(method.to_owned());
                }
                if let Some(flow) = flow {
                    extra["paymentFlow"] = Value::String(flow.to_owned());
                }
            }
            let Ok(parsed) = parse_request(&encoded(&request), &policy, false) else {
                std::process::abort();
            };
            assert_eq!(parsed.meta.settlement_route, SettlementRoute::Direct);
        }
    }

    #[test]
    fn direct_eip155_rejects_unknown_or_mismatched_selectors_before_payload_parsing() {
        let policy = PaymentIdentifierConfig::default();
        for (accepted_method, requirements_method) in [
            (Some("permit2"), Some("permit2")),
            (Some("future"), Some("future")),
            (Some("permit2"), None),
            (None, Some("permit2")),
        ] {
            let mut request = eip155_request_value();
            if let Some(method) = accepted_method {
                request["paymentPayload"]["accepted"]["extra"]["assetTransferMethod"] =
                    Value::String(method.to_owned());
            }
            if let Some(method) = requirements_method {
                request["paymentRequirements"]["extra"]["assetTransferMethod"] =
                    Value::String(method.to_owned());
            }
            request["paymentPayload"]["payload"] = serde_json::json!({"txHash": "proof"});
            let Ok(parsed) = parse_request(&encoded(&request), &policy, false) else {
                std::process::abort();
            };
            assert_eq!(parsed.meta.settlement_route, SettlementRoute::Unsupported);
            assert!(parsed.meta.signed_delegate_action.is_none());
        }
    }

    #[test]
    fn near_payload_still_requires_signed_delegate() {
        let policy = PaymentIdentifierConfig::default();
        let Ok(parsed) = parse_request(&encoded(&request_value()), &policy, false) else {
            std::process::abort();
        };
        assert_eq!(
            parsed.meta.signed_delegate_action.as_deref(),
            Some("c2lnbmVkLWRlbGVnYXRl")
        );
        assert_eq!(parsed.meta.settlement_route, SettlementRoute::Direct);
    }

    #[test]
    fn near_asset_transfer_method_accepts_absent_without_rewriting() {
        let policy = PaymentIdentifierConfig::default();

        let mut absent = request_value();
        absent["paymentPayload"]["accepted"]
            .as_object_mut()
            .unwrap_or_else(|| std::process::abort())
            .remove("extra");
        absent["paymentRequirements"]
            .as_object_mut()
            .unwrap_or_else(|| std::process::abort())
            .remove("extra");
        let Ok(defaulted) = parse_request(&encoded(&absent), &policy, false) else {
            std::process::abort();
        };
        assert_eq!(defaulted.value, absent);
        assert_eq!(defaulted.meta.settlement_route, SettlementRoute::Direct);
    }

    #[test]
    fn near_delegate_accepts_explicit_authorization_flow() {
        let policy = PaymentIdentifierConfig::default();
        let mut request = request_value();
        request["paymentPayload"]["accepted"]["extra"]["paymentFlow"] =
            Value::String("authorization".to_owned());
        request["paymentRequirements"]["extra"]["paymentFlow"] =
            Value::String("authorization".to_owned());
        let Ok(parsed) = parse_request(&encoded(&request), &policy, false) else {
            std::process::abort();
        };
        assert_eq!(parsed.meta.settlement_route, SettlementRoute::Direct);
        assert!(parsed.meta.signed_delegate_action.is_some());
    }

    #[test]
    fn near_asset_transfer_method_rejects_non_string_values() {
        let policy = PaymentIdentifierConfig::default();
        let mut request = request_value();
        request["paymentPayload"]["accepted"]["extra"]["assetTransferMethod"] = Value::Bool(true);
        request["paymentRequirements"]["extra"]["assetTransferMethod"] = Value::Bool(true);
        assert!(matches!(
            parse_request(&encoded(&request), &policy, false),
            Err(RequestError::Field(
                "paymentPayload.accepted.extra.assetTransferMethod"
            ))
        ));
    }

    #[test]
    fn near_supplied_transfer_methods_are_typed_as_unsupported_before_service_policy() {
        let policy = PaymentIdentifierConfig::default();
        for (accepted, requirements) in [
            (Some("delegate"), Some("delegate")),
            (Some("intents-verifier"), Some("intents-verifier")),
            (Some("future-method-a"), Some("future-method-b")),
            (Some("intents-verifier"), None),
            (None, Some("intents-verifier")),
        ] {
            let mut request = request_value();
            if let Some(accepted) = accepted {
                request["paymentPayload"]["accepted"]["extra"]["assetTransferMethod"] =
                    Value::String(accepted.to_owned());
            }
            if let Some(requirements) = requirements {
                request["paymentRequirements"]["extra"]["assetTransferMethod"] =
                    Value::String(requirements.to_owned());
            }
            let input = request.clone();
            let Ok(parsed) = parse_request(&encoded(&input), &policy, false) else {
                std::process::abort();
            };
            assert_eq!(parsed.value, input);
            assert_eq!(parsed.meta.settlement_route, SettlementRoute::Unsupported);
            assert_eq!(parsed.meta.signed_delegate_action.as_deref(), None);
        }
    }

    #[test]
    fn near_unsupported_method_does_not_parse_delegate_payload_shape() {
        let policy = PaymentIdentifierConfig::default();
        let mut request = request_value();
        request["paymentPayload"]["accepted"]["extra"]["assetTransferMethod"] =
            Value::String("intents-verifier".to_owned());
        request["paymentRequirements"]["extra"]["assetTransferMethod"] =
            Value::String("intents-verifier".to_owned());
        request["paymentPayload"]["payload"] = serde_json::json!({
            "signedIntent": {
                "standard": "nep413",
                "payload": {"message": "{}"},
                "signature": "placeholder"
            }
        });
        let Ok(parsed) = parse_request(&encoded(&request), &policy, false) else {
            std::process::abort();
        };
        assert_eq!(parsed.meta.settlement_route, SettlementRoute::Unsupported);
        assert_eq!(parsed.meta.signed_delegate_action, None);
    }

    #[test]
    fn near_intents_upfront_is_recognized_before_origin_payload_parsing() {
        let policy = PaymentIdentifierConfig::default();
        for network in ["near:mainnet", "eip155:8453", "solana:mainnet"] {
            let mut request = request_value();
            request["paymentPayload"]["accepted"]["network"] = Value::String(network.to_owned());
            request["paymentRequirements"]["network"] = Value::String(network.to_owned());
            request["paymentPayload"]["accepted"]["extra"] = serde_json::json!({
                "assetTransferMethod": "near-intents",
                "paymentFlow": "upfront",
            });
            request["paymentRequirements"]["extra"] = serde_json::json!({
                "assetTransferMethod": "near-intents",
                "paymentFlow": "upfront",
            });
            request["paymentPayload"]["payload"] = serde_json::json!({
                "txHash": "provider-specific-origin-transaction",
            });

            let Ok(parsed) = parse_request(&encoded(&request), &policy, false) else {
                std::process::abort();
            };
            assert_eq!(
                parsed.meta.settlement_route,
                SettlementRoute::NearIntentsUpfrontDisabled
            );
            assert!(parsed.meta.signed_delegate_action.is_none());
        }
    }

    #[test]
    fn near_intents_upfront_requires_the_exact_selector_pair() {
        let policy = PaymentIdentifierConfig::default();
        for (accepted_flow, requirements_flow) in [
            (Some("upfront"), None),
            (None, Some("upfront")),
            (Some("authorization"), Some("authorization")),
        ] {
            let mut request = eip155_request_value();
            request["paymentPayload"]["accepted"]["extra"] = serde_json::json!({
                "assetTransferMethod": "near-intents",
            });
            request["paymentRequirements"]["extra"] = serde_json::json!({
                "assetTransferMethod": "near-intents",
            });
            if let Some(flow) = accepted_flow {
                request["paymentPayload"]["accepted"]["extra"]["paymentFlow"] =
                    Value::String(flow.to_owned());
            }
            if let Some(flow) = requirements_flow {
                request["paymentRequirements"]["extra"]["paymentFlow"] =
                    Value::String(flow.to_owned());
            }
            request["paymentPayload"]["payload"] = serde_json::json!({"txHash": "proof"});

            let Ok(parsed) = parse_request(&encoded(&request), &policy, false) else {
                std::process::abort();
            };
            assert_eq!(parsed.meta.settlement_route, SettlementRoute::Unsupported);
            assert!(parsed.meta.signed_delegate_action.is_none());
        }

        let mut malformed = eip155_request_value();
        malformed["paymentPayload"]["accepted"]["extra"]["assetTransferMethod"] =
            Value::String("near-intents".to_owned());
        malformed["paymentRequirements"]["extra"]["assetTransferMethod"] =
            Value::String("near-intents".to_owned());
        malformed["paymentPayload"]["accepted"]["extra"]["paymentFlow"] = Value::Bool(true);
        malformed["paymentRequirements"]["extra"]["paymentFlow"] = Value::Bool(true);
        assert!(matches!(
            parse_request(&encoded(&malformed), &policy, false),
            Err(RequestError::Field(
                "paymentPayload.accepted.extra.paymentFlow"
            ))
        ));
    }

    #[test]
    fn mechanism_payload_must_match_network_namespace() {
        let policy = PaymentIdentifierConfig::default();
        // eip155 network carrying a NEAR signed-delegate payload is rejected.
        let mut evm_with_near = eip155_request_value();
        evm_with_near["paymentPayload"]["payload"] =
            serde_json::json!({ "signedDelegateAction": "c2lnbmVk" });
        assert!(parse_request(&encoded(&evm_with_near), &policy, false).is_err());
        // NEAR network carrying an eip155 authorization payload is rejected.
        let mut near_with_evm = request_value();
        near_with_evm["paymentPayload"]["payload"] =
            serde_json::json!({ "authorization": {}, "signature": "0xab" });
        assert!(parse_request(&encoded(&near_with_evm), &policy, false).is_err());
    }

    #[test]
    fn eip155_requires_authorization_object_and_signature_string() {
        let policy = PaymentIdentifierConfig::default();
        let mut no_signature = eip155_request_value();
        if let Some(payload) = no_signature["paymentPayload"]["payload"].as_object_mut() {
            payload.remove("signature");
        }
        assert!(parse_request(&encoded(&no_signature), &policy, false).is_err());

        let mut authorization_not_object = eip155_request_value();
        authorization_not_object["paymentPayload"]["payload"]["authorization"] =
            Value::String("nope".to_owned());
        assert!(parse_request(&encoded(&authorization_not_object), &policy, false).is_err());
    }

    #[test]
    fn eip155_authorization_is_strict_and_lossless_at_parse_boundary() {
        let policy = PaymentIdentifierConfig::default();
        let mut unknown = eip155_request_value();
        unknown["paymentPayload"]["payload"]["authorization"]["unexpected"] = Value::Bool(true);
        assert!(parse_request(&encoded(&unknown), &policy, false).is_err());

        for field in ["from", "to", "value", "validAfter", "validBefore", "nonce"] {
            let mut missing = eip155_request_value();
            missing["paymentPayload"]["payload"]["authorization"]
                .as_object_mut()
                .unwrap_or_else(|| std::process::abort())
                .remove(field);
            assert!(parse_request(&encoded(&missing), &policy, false).is_err());
        }

        for field in ["value", "validAfter", "validBefore"] {
            let mut numeric = eip155_request_value();
            numeric["paymentPayload"]["payload"]["authorization"][field] = Value::from(1);
            assert!(parse_request(&encoded(&numeric), &policy, false).is_err());

            let mut non_decimal = eip155_request_value();
            non_decimal["paymentPayload"]["payload"]["authorization"][field] =
                Value::String("1.0".to_owned());
            assert!(parse_request(&encoded(&non_decimal), &policy, false).is_err());
        }

        for nonce in [
            "0x01",
            "01",
            "0xgg00000000000000000000000000000000000000000000000000000000000000",
        ] {
            let mut invalid_nonce = eip155_request_value();
            invalid_nonce["paymentPayload"]["payload"]["authorization"]["nonce"] =
                Value::String(nonce.to_owned());
            assert!(parse_request(&encoded(&invalid_nonce), &policy, false).is_err());
        }
    }

    fn v1_wire_request_value() -> Value {
        serde_json::json!({
            "x402Version": 1,
            "paymentPayload": {
                "x402Version": 1,
                "scheme": "exact",
                "network": "base",
                "payload": {
                    "signature": "0xdeadbeef",
                    "authorization": {
                        "from": "0x150B4b68F0Aa687a70d2383A88A5294E6077296E",
                        "to": "0x7Ff46ab88688D528bCE3e59c470240c6901cF88c",
                        "value": "1000",
                        "validAfter": "0",
                        "validBefore": "9999999999",
                        "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001",
                    },
                },
            },
            "paymentRequirements": {
                "scheme": "exact",
                "network": "base",
                "maxAmountRequired": "1000",
                "resource": "https://x402-demo-base.mikedotexe.com/work",
                "description": "Deterministic paid work",
                "mimeType": "application/json",
                "payTo": "0x7Ff46ab88688D528bCE3e59c470240c6901cF88c",
                "maxTimeoutSeconds": 300,
                "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                "extra": { "name": "USD Coin", "version": "2" },
            },
        })
    }

    #[test]
    fn v1_wire_requests_translate_only_when_accepted() {
        let policy = PaymentIdentifierConfig::default();
        let body = encoded(&v1_wire_request_value());
        // Gate off: the strict v2 walker rejects the legacy shape outright.
        assert!(parse_request(&body, &policy, false).is_err());
        let Ok(parsed) = parse_request(&body, &policy, true) else {
            std::process::abort();
        };
        assert_eq!(parsed.wire, WireVersion::V1);
        assert_eq!(parsed.meta.x402_version, 2);
        assert_eq!(parsed.meta.network, "eip155:8453");
        assert_eq!(parsed.meta.amount, "1000");
        assert_eq!(parsed.meta.scheme, "exact");
        assert!(parsed.meta.signed_delegate_action.is_none());
        // The preserved request value is canonical v2, so the journal
        // fingerprint is dialect-independent.
        assert_eq!(parsed.value["x402Version"], 2);
        assert_eq!(parsed.value["paymentPayload"]["accepted"]["amount"], "1000");
    }

    #[test]
    fn translated_v1_authorization_rejects_unknown_or_lossy_fields() {
        let policy = PaymentIdentifierConfig::default();
        let mut unknown = v1_wire_request_value();
        unknown["paymentPayload"]["payload"]["authorization"]["unexpected"] = Value::Bool(true);
        assert!(parse_request(&encoded(&unknown), &policy, true).is_err());

        let mut numeric_window = v1_wire_request_value();
        numeric_window["paymentPayload"]["payload"]["authorization"]["validAfter"] = Value::from(0);
        assert!(parse_request(&encoded(&numeric_window), &policy, true).is_err());

        let mut short_nonce = v1_wire_request_value();
        short_nonce["paymentPayload"]["payload"]["authorization"]["nonce"] =
            Value::String("0x01".to_owned());
        assert!(parse_request(&encoded(&short_nonce), &policy, true).is_err());
    }

    #[test]
    fn v2_wire_requests_are_untouched_by_the_v1_gate() {
        let policy = PaymentIdentifierConfig::default();
        let body = encoded(&eip155_request_value());
        let Ok(parsed) = parse_request(&body, &policy, true) else {
            std::process::abort();
        };
        assert_eq!(parsed.wire, WireVersion::V2);
        assert_eq!(parsed.meta.network, "eip155:84532");
    }

    #[test]
    fn decimal_comparison_is_numeric() {
        assert!(decimal_is_at_least("001000", "1000"));
        assert!(!decimal_is_at_least("999", "1000"));
        assert!(decimal_is_at_least("1000000000000000000000000", "1000"));
    }

    #[test]
    fn settlement_failure_always_serializes_transaction_and_network() {
        let response = SettleResponse::failure(
            "settlement_failed",
            None,
            None,
            String::new(),
            "near:testnet".to_owned(),
        );
        let value = serde_json::to_value(response).ok();
        assert_eq!(
            value.as_ref().and_then(|value| value.get("transaction")),
            Some(&Value::String(String::new()))
        );
        assert_eq!(
            value.as_ref().and_then(|value| value.get("network")),
            Some(&Value::String("near:testnet".to_owned()))
        );
    }

    #[test]
    fn request_metadata_debug_output_redacts_payment_material() {
        let meta = RequestMeta {
            x402_version: 2,
            scheme: "exact".to_owned(),
            network: "near:testnet".to_owned(),
            asset: "sensitive-asset.testnet".to_owned(),
            amount: "sensitive-amount-1000".to_owned(),
            pay_to: "sensitive-payee.testnet".to_owned(),
            signed_delegate_action: Some("sensitive-signed-delegate".to_owned()),
            settlement_route: SettlementRoute::Direct,
            payment_identifier: Some("sensitive_identifier_1234".to_owned()),
        };
        let debug = format!("{meta:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("sensitive-asset.testnet"));
        assert!(!debug.contains("sensitive-amount-1000"));
        assert!(!debug.contains("sensitive-payee.testnet"));
        assert!(!debug.contains("sensitive-signed-delegate"));
        assert!(!debug.contains("sensitive_identifier_1234"));
    }

    #[test]
    fn response_debug_output_redacts_payer_transaction_and_signers() {
        let settle = SettleResponse::success(
            "sensitive-payer".to_owned(),
            "sensitive-transaction-hash".to_owned(),
            "eip155:8453".to_owned(),
        );
        let verify = VerifyResponse::valid("sensitive-payer".to_owned());
        let supported = SupportedResponse {
            kinds: Vec::new(),
            extensions: Vec::new(),
            signers: std::collections::HashMap::from([(
                "eip155:8453".to_owned(),
                vec!["sensitive-signer".to_owned()],
            )]),
        };
        for debug in [
            format!("{settle:?}"),
            format!("{verify:?}"),
            format!("{supported:?}"),
        ] {
            assert!(debug.contains("<redacted>"));
            assert!(!debug.contains("sensitive-payer"));
            assert!(!debug.contains("sensitive-transaction-hash"));
            assert!(!debug.contains("sensitive-signer"));
        }
    }
}
