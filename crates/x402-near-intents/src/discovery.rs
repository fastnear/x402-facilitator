//! Strict model for the draft `crosschain-swap` discovery extension.
//!
//! The extension is indicative only. It intentionally contains neither a
//! deposit instrument nor a payment proof; clients may pay only an entry that
//! also appears in `accepts[]`.

use std::collections::HashSet;
use std::fmt;

use chrono::DateTime;
use serde::{Deserialize, Serialize};

use crate::wire::{WireError, validate_atomic_amount, validate_caip2};

const MAX_PROVIDER_BYTES: usize = 64;
const MAX_ASSET_BYTES: usize = 256;
const MAX_ORIGINS: usize = 128;

/// The extension value exactly as proposed at the pinned draft revision.
///
/// Core x402 v2 currently also requires a `schema` sibling. The draft omits
/// it, so this type must not be advertised until upstream resolves that
/// inconsistency.
#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CrosschainSwapExtension {
    pub info: CrosschainSwapInfo,
}

impl fmt::Debug for CrosschainSwapExtension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CrosschainSwapExtension")
            .field("provider", &self.info.provider)
            .field("origin_count", &self.info.origins.len())
            .field("route", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CrosschainSwapInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub destination: Destination,
    pub origins: Vec<Origin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
}

impl fmt::Debug for CrosschainSwapInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CrosschainSwapInfo")
            .field("provider", &self.provider)
            .field("origin_count", &self.origins.len())
            .field("route", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Destination {
    pub network: String,
    pub asset: String,
    pub amount: String,
}

impl fmt::Debug for Destination {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Destination")
            .field("network", &self.network)
            .field("payment", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Origin {
    pub network: String,
    pub asset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indicative_amount: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_estimate: Option<f64>,
}

impl fmt::Debug for Origin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Origin")
            .field("network", &self.network)
            .field("payment", &"<redacted>")
            .field("time_estimate", &self.time_estimate)
            .finish_non_exhaustive()
    }
}

impl CrosschainSwapExtension {
    pub fn validate(&self) -> Result<(), DiscoveryError> {
        if let Some(provider) = &self.info.provider {
            validate_text(provider, MAX_PROVIDER_BYTES, "info.provider")?;
        }
        validate_caip2(&self.info.destination.network)?;
        validate_text(
            &self.info.destination.asset,
            MAX_ASSET_BYTES,
            "info.destination.asset",
        )?;
        validate_atomic_amount(&self.info.destination.amount, "info.destination.amount")?;

        if self.info.origins.is_empty() || self.info.origins.len() > MAX_ORIGINS {
            return Err(DiscoveryError::Field("info.origins"));
        }
        let mut routes = HashSet::with_capacity(self.info.origins.len());
        for origin in &self.info.origins {
            validate_caip2(&origin.network)?;
            validate_text(&origin.asset, MAX_ASSET_BYTES, "info.origins[].asset")?;
            if !routes.insert((origin.network.as_str(), origin.asset.as_str())) {
                return Err(DiscoveryError::DuplicateOrigin);
            }
            if let Some(amount) = &origin.indicative_amount {
                validate_atomic_amount(amount, "info.origins[].indicativeAmount")?;
            }
            if origin
                .time_estimate
                .is_some_and(|seconds| !seconds.is_finite() || seconds <= 0.0)
            {
                return Err(DiscoveryError::Field("info.origins[].timeEstimate"));
            }
        }
        if self
            .info
            .expires
            .as_deref()
            .is_some_and(|value| DateTime::parse_from_rfc3339(value).is_err())
        {
            return Err(DiscoveryError::Field("info.expires"));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum DiscoveryError {
    #[error("invalid crosschain-swap field: {0}")]
    Field(&'static str),
    #[error("crosschain-swap contains a duplicate origin route")]
    DuplicateOrigin,
    #[error(transparent)]
    Wire(#[from] WireError),
}

fn validate_text(value: &str, maximum: usize, field: &'static str) -> Result<(), DiscoveryError> {
    if value.is_empty()
        || value.len() > maximum
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(DiscoveryError::Field(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn draft_example() -> serde_json::Value {
        json!({
            "info": {
                "provider": "near-intents",
                "destination": {
                    "network": "eip155:8453",
                    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                    "amount": "1000000"
                },
                "origins": [
                    {
                        "network": "eip155:42161",
                        "asset": "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
                        "indicativeAmount": "1005000",
                        "timeEstimate": 120
                    },
                    {
                        "network": "bip122:000000000019d6689c085ae165831e93",
                        "asset": "BTC",
                        "indicativeAmount": "1520",
                        "timeEstimate": 1800
                    }
                ],
                "expires": "2026-09-04T15:10:00Z"
            }
        })
    }

    #[test]
    fn parses_and_validates_the_pinned_draft_example() {
        let extension = serde_json::from_value::<CrosschainSwapExtension>(draft_example());
        assert!(extension.is_ok());
        let Some(extension) = extension.ok() else {
            std::process::abort();
        };
        assert!(extension.validate().is_ok());

        let serialized = serde_json::to_value(extension);
        assert!(serialized.is_ok());
        let Some(serialized) = serialized.ok() else {
            std::process::abort();
        };
        assert!(serialized.get("payTo").is_none());
        assert!(serialized.get("txHash").is_none());
        assert!(serialized["info"].get("payTo").is_none());
    }

    #[test]
    fn rejects_duplicate_routes_and_invalid_indicative_values() {
        let mut duplicate = draft_example();
        duplicate["info"]["origins"][1] = duplicate["info"]["origins"][0].clone();
        let parsed = serde_json::from_value::<CrosschainSwapExtension>(duplicate);
        let Some(parsed) = parsed.ok() else {
            std::process::abort();
        };
        assert_eq!(parsed.validate(), Err(DiscoveryError::DuplicateOrigin));

        let mut invalid = draft_example();
        invalid["info"]["origins"][0]["indicativeAmount"] = json!("1.25");
        let parsed = serde_json::from_value::<CrosschainSwapExtension>(invalid);
        let Some(parsed) = parsed.ok() else {
            std::process::abort();
        };
        assert!(matches!(
            parsed.validate(),
            Err(DiscoveryError::Wire(WireError::Field(
                "info.origins[].indicativeAmount"
            )))
        ));
    }

    #[test]
    fn wire_shape_is_closed_and_requires_real_routes() {
        let mut unknown = draft_example();
        unknown["info"]["instrument"] = json!("must-not-appear");
        assert!(serde_json::from_value::<CrosschainSwapExtension>(unknown).is_err());

        let mut empty = draft_example();
        empty["info"]["origins"] = json!([]);
        let parsed = serde_json::from_value::<CrosschainSwapExtension>(empty);
        let Some(parsed) = parsed.ok() else {
            std::process::abort();
        };
        assert_eq!(
            parsed.validate(),
            Err(DiscoveryError::Field("info.origins"))
        );
    }
}
