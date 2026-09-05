//! Legacy x402 v1 wire compatibility (gated by the `accept_v1` config flag).
//!
//! v1 requests name networks by legacy alias ("base", "base-sepolia"), carry
//! `scheme`/`network` at the top of `paymentPayload` instead of an `accepted`
//! echo, and call the amount `maxAmountRequired`. Requests are translated
//! strictly into the canonical v2 shape before the normal pipeline runs, so
//! the journal fingerprint and every downstream check see one settlement
//! identity regardless of wire dialect. Responses only translate the
//! `network` field back to the legacy alias: everything else we emit is
//! already a field-compatible superset of the v1 vocabulary
//! (`isValid`/`invalidReason`/`payer`, `success`/`errorReason`/`transaction`),
//! and legacy readers ignore unknown reason codes.

use serde_json::{Map, Value};

use crate::protocol::{RequestError, ensure_allowed_keys, required_string};

/// Which wire dialect a request arrived in. Everything after `parse_request`
/// operates on canonical v2; the tag only drives response formatting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireVersion {
    V1,
    V2,
}

/// The legacy v1 network aliases this facilitator recognizes. v1 predates
/// CAIP-2 identifiers and never covered NEAR, so the map is eip155-only by
/// nature. Kept local (rather than the upstream registry) so the accepted
/// alias surface is explicit and cannot drift with a dependency bump.
const V1_NETWORKS: &[(&str, &str)] = &[("base", "eip155:8453"), ("base-sepolia", "eip155:84532")];

pub fn caip2_for_v1_network(name: &str) -> Option<&'static str> {
    V1_NETWORKS
        .iter()
        .find(|(alias, _)| *alias == name)
        .map(|(_, caip2)| *caip2)
}

pub fn v1_network_name(network: &str) -> Option<&'static str> {
    V1_NETWORKS
        .iter()
        .find(|(_, caip2)| *caip2 == network)
        .map(|(alias, _)| *alias)
}

/// A request is v1 wire iff its top-level `x402Version` is exactly 1.
pub fn is_v1_request(object: &Map<String, Value>) -> bool {
    object.get("x402Version").and_then(Value::as_u64) == Some(1)
}

/// Strictly translate a legacy v1 verify/settle request into the canonical v2
/// value the rest of the pipeline expects, mirroring the v2 parser's
/// deny-unknown-keys posture at every level. The translation is shallow on
/// purpose: the full v2 walker re-runs on the output, so amount digits,
/// timeout positivity, and the chain mechanism payload are all re-validated
/// there. Any failure surfaces as the same 400 `malformed_request` a
/// non-translated bad request gets.
pub fn translate_v1_to_v2(object: &Map<String, Value>) -> Result<Value, RequestError> {
    ensure_allowed_keys(
        object,
        &["x402Version", "paymentPayload", "paymentRequirements"],
        "request",
    )?;
    let requirements = object
        .get("paymentRequirements")
        .and_then(Value::as_object)
        .ok_or(RequestError::Field("paymentRequirements"))?;
    let requirements_scheme =
        required_string(requirements, "scheme", "paymentRequirements.scheme")?;
    let requirements_network =
        required_string(requirements, "network", "paymentRequirements.network")?;
    let translated_requirements = translate_requirements(requirements)?;

    let payload = object
        .get("paymentPayload")
        .and_then(Value::as_object)
        .ok_or(RequestError::Field("paymentPayload"))?;
    ensure_allowed_keys(
        payload,
        &["x402Version", "scheme", "network", "payload"],
        "paymentPayload",
    )?;
    if payload.get("x402Version").and_then(Value::as_u64) != Some(1) {
        return Err(RequestError::Field("paymentPayload.x402Version"));
    }
    // v1 semantic: the payload's top-level scheme/network name the
    // requirements entry being paid and must agree with it.
    if required_string(payload, "scheme", "paymentPayload.scheme")? != requirements_scheme {
        return Err(RequestError::Field("paymentPayload.scheme"));
    }
    if required_string(payload, "network", "paymentPayload.network")? != requirements_network {
        return Err(RequestError::Field("paymentPayload.network"));
    }
    let mechanism = payload
        .get("payload")
        .and_then(Value::as_object)
        .ok_or(RequestError::Field("paymentPayload.payload"))?;

    let mut payment_payload = Map::new();
    payment_payload.insert("x402Version".to_owned(), Value::from(2));
    payment_payload.insert("accepted".to_owned(), translated_requirements.clone());
    payment_payload.insert("payload".to_owned(), Value::Object(mechanism.clone()));
    let mut translated = Map::new();
    translated.insert("x402Version".to_owned(), Value::from(2));
    translated.insert("paymentPayload".to_owned(), Value::Object(payment_payload));
    translated.insert("paymentRequirements".to_owned(), translated_requirements);
    Ok(Value::Object(translated))
}

/// Translate v1 `PaymentRequirements` to the v2 shape: `maxAmountRequired`
/// becomes `amount`, the network alias becomes CAIP-2, and the v1
/// resource-metadata fields (`resource`, `description`, `mimeType`,
/// `outputSchema`) are dropped — v2 requirements have no analog (v2 carries
/// resource info in `paymentPayload.resource`, which v1 clients never send).
fn translate_requirements(requirements: &Map<String, Value>) -> Result<Value, RequestError> {
    ensure_allowed_keys(
        requirements,
        &[
            "scheme",
            "network",
            "maxAmountRequired",
            "resource",
            "description",
            "mimeType",
            "outputSchema",
            "payTo",
            "maxTimeoutSeconds",
            "asset",
            "extra",
        ],
        "paymentRequirements",
    )?;
    let network = required_string(requirements, "network", "paymentRequirements.network")?;
    let Some(caip2) = caip2_for_v1_network(&network) else {
        return Err(RequestError::Field("paymentRequirements.network"));
    };
    let scheme = required_string(requirements, "scheme", "paymentRequirements.scheme")?;
    let asset = required_string(requirements, "asset", "paymentRequirements.asset")?;
    let amount = required_string(
        requirements,
        "maxAmountRequired",
        "paymentRequirements.maxAmountRequired",
    )?;
    let pay_to = required_string(requirements, "payTo", "paymentRequirements.payTo")?;
    let max_timeout = requirements
        .get("maxTimeoutSeconds")
        .cloned()
        .ok_or(RequestError::Field("paymentRequirements.maxTimeoutSeconds"))?;

    let mut translated = Map::new();
    translated.insert("scheme".to_owned(), Value::String(scheme));
    translated.insert("network".to_owned(), Value::String(caip2.to_owned()));
    translated.insert("asset".to_owned(), Value::String(asset));
    translated.insert("amount".to_owned(), Value::String(amount));
    translated.insert("payTo".to_owned(), Value::String(pay_to));
    translated.insert("maxTimeoutSeconds".to_owned(), max_timeout);
    if let Some(extra) = requirements.get("extra") {
        translated.insert("extra".to_owned(), extra.clone());
    }
    Ok(Value::Object(translated))
}

/// Rewrite a protocol response's `network` (CAIP-2) back to the legacy v1
/// alias in place, when one exists. Verify responses have no `network` field
/// and pass through untouched.
pub fn translate_response_value_to_v1(value: &mut Value) {
    if let Some(network) = value.get_mut("network")
        && let Some(name) = network.as_str().and_then(v1_network_name)
    {
        *network = Value::String(name.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v1_request() -> Value {
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
                "outputSchema": null,
                "payTo": "0x7Ff46ab88688D528bCE3e59c470240c6901cF88c",
                "maxTimeoutSeconds": 300,
                "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                "extra": { "name": "USD Coin", "version": "2" },
            },
        })
    }

    fn as_object(value: &Value) -> &Map<String, Value> {
        let Some(object) = value.as_object() else {
            std::process::abort();
        };
        object
    }

    #[test]
    fn network_alias_map_round_trips() {
        assert_eq!(caip2_for_v1_network("base"), Some("eip155:8453"));
        assert_eq!(caip2_for_v1_network("base-sepolia"), Some("eip155:84532"));
        assert_eq!(caip2_for_v1_network("eip155:8453"), None);
        assert_eq!(v1_network_name("eip155:8453"), Some("base"));
        assert_eq!(v1_network_name("eip155:84532"), Some("base-sepolia"));
        assert_eq!(v1_network_name("near:mainnet"), None);
    }

    #[test]
    fn v1_sniff_requires_top_level_version_one() {
        assert!(is_v1_request(as_object(&v1_request())));
        assert!(!is_v1_request(as_object(&serde_json::json!({
            "x402Version": 2,
        }))));
        assert!(!is_v1_request(as_object(&serde_json::json!({}))));
    }

    #[test]
    fn translation_produces_the_canonical_v2_request() {
        let Ok(translated) = translate_v1_to_v2(as_object(&v1_request())) else {
            std::process::abort();
        };
        let expected_requirements = serde_json::json!({
            "scheme": "exact",
            "network": "eip155:8453",
            "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "amount": "1000",
            "payTo": "0x7Ff46ab88688D528bCE3e59c470240c6901cF88c",
            "maxTimeoutSeconds": 300,
            "extra": { "name": "USD Coin", "version": "2" },
        });
        assert_eq!(translated["x402Version"], 2);
        assert_eq!(translated["paymentRequirements"], expected_requirements);
        assert_eq!(translated["paymentPayload"]["x402Version"], 2);
        assert_eq!(
            translated["paymentPayload"]["accepted"],
            expected_requirements
        );
        assert_eq!(
            translated["paymentPayload"]["payload"],
            v1_request()["paymentPayload"]["payload"]
        );
        assert!(translated["paymentPayload"].get("scheme").is_none());
        assert!(translated["paymentPayload"].get("network").is_none());
    }

    #[test]
    fn v1_and_v2_transport_produce_one_canonical_fingerprint() {
        let legacy = v1_request();
        let Ok(canonical) = translate_v1_to_v2(as_object(&legacy)) else {
            std::process::abort();
        };
        let config = crate::config::PaymentIdentifierConfig::default();
        let Ok(parsed_v1) = crate::protocol::parse_request(
            &serde_json::to_vec(&legacy).unwrap_or_default(),
            &config,
            true,
        ) else {
            std::process::abort();
        };
        let Ok(parsed_v2) = crate::protocol::parse_request(
            &serde_json::to_vec(&canonical).unwrap_or_default(),
            &config,
            true,
        ) else {
            std::process::abort();
        };
        let payment_hash = [0x5a; 32];
        let Ok(v1_fingerprint) =
            crate::protocol::request_fingerprint(&parsed_v1.value, &payment_hash)
        else {
            std::process::abort();
        };
        let Ok(v2_fingerprint) =
            crate::protocol::request_fingerprint(&parsed_v2.value, &payment_hash)
        else {
            std::process::abort();
        };

        assert_eq!(parsed_v1.value, parsed_v2.value);
        assert_eq!(v1_fingerprint, v2_fingerprint);
    }

    #[test]
    fn legacy_wire_cannot_select_near_intents_upfront() {
        let mut request = v1_request();
        request["paymentRequirements"]["extra"] = serde_json::json!({
            "assetTransferMethod": "near-intents",
            "paymentFlow": "upfront",
        });
        request["paymentPayload"]["payload"] = serde_json::json!({
            "txHash": "origin-transaction",
        });
        let config = crate::config::PaymentIdentifierConfig::default();
        let parsed = crate::protocol::parse_request(
            &serde_json::to_vec(&request).unwrap_or_default(),
            &config,
            true,
        );
        let Ok(parsed) = parsed else {
            std::process::abort();
        };
        assert_eq!(
            parsed.meta.settlement_route,
            crate::protocol::SettlementRoute::Unsupported
        );
        assert!(parsed.meta.signed_delegate_action.is_none());
    }

    #[test]
    fn legacy_wire_accepts_explicit_direct_eip3009_defaults() {
        let mut request = v1_request();
        request["paymentRequirements"]["extra"]["assetTransferMethod"] =
            Value::String("eip3009".to_owned());
        request["paymentRequirements"]["extra"]["paymentFlow"] =
            Value::String("authorization".to_owned());
        let config = crate::config::PaymentIdentifierConfig::default();
        let parsed = crate::protocol::parse_request(
            &serde_json::to_vec(&request).unwrap_or_default(),
            &config,
            true,
        );
        let Ok(parsed) = parsed else {
            std::process::abort();
        };
        assert_eq!(
            parsed.meta.settlement_route,
            crate::protocol::SettlementRoute::Direct
        );
        assert!(parsed.meta.signed_delegate_action.is_none());
    }

    #[test]
    fn translation_rejects_unknown_keys_at_every_level() {
        let mut unknown_top = v1_request();
        unknown_top["unexpected"] = Value::Bool(true);
        assert!(translate_v1_to_v2(as_object(&unknown_top)).is_err());

        let mut unknown_payload = v1_request();
        unknown_payload["paymentPayload"]["accepted"] = serde_json::json!({});
        assert!(translate_v1_to_v2(as_object(&unknown_payload)).is_err());

        let mut unknown_requirements = v1_request();
        unknown_requirements["paymentRequirements"]["amount"] = Value::String("1000".to_owned());
        assert!(translate_v1_to_v2(as_object(&unknown_requirements)).is_err());
    }

    #[test]
    fn translation_rejects_unmapped_networks_and_disagreement() {
        for network in ["avalanche", "solana", "eip155:8453", "near:mainnet"] {
            let mut request = v1_request();
            request["paymentRequirements"]["network"] = Value::String(network.to_owned());
            request["paymentPayload"]["network"] = Value::String(network.to_owned());
            assert!(translate_v1_to_v2(as_object(&request)).is_err());
        }
        let mut disagreeing = v1_request();
        disagreeing["paymentPayload"]["network"] = Value::String("base-sepolia".to_owned());
        assert!(translate_v1_to_v2(as_object(&disagreeing)).is_err());
        let mut nested_v2 = v1_request();
        nested_v2["paymentPayload"]["x402Version"] = Value::from(2);
        assert!(translate_v1_to_v2(as_object(&nested_v2)).is_err());
    }

    #[test]
    fn response_translation_rewrites_only_mapped_networks() {
        let mut settle = serde_json::json!({
            "success": true,
            "transaction": "0xabc",
            "network": "eip155:8453",
            "payer": "0x150B4b68F0Aa687a70d2383A88A5294E6077296E",
        });
        translate_response_value_to_v1(&mut settle);
        assert_eq!(settle["network"], "base");

        let mut near = serde_json::json!({ "success": false, "network": "near:mainnet" });
        translate_response_value_to_v1(&mut near);
        assert_eq!(near["network"], "near:mainnet");

        let mut verify = serde_json::json!({ "isValid": true, "payer": "0xabc" });
        translate_response_value_to_v1(&mut verify);
        assert!(verify.get("network").is_none());
    }
}
