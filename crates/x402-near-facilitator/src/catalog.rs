//! Immutable, opt-in discovery catalog for independently operated merchants.
//!
//! The checked-in manifest is part of the signed source and is embedded in the
//! service binary. It is never derived from API-client or settlement records.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;

use chrono::{DateTime, FixedOffset};
use near_primitives::types::AccountId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::config::{ServiceConfig, canonical_eip712_domain, canonical_usdc_asset, is_evm_address};

const EMBEDDED_MANIFEST: &str = include_str!("../../../docs/catalog/resources.json");
const MANIFEST_VERSION: u8 = 1;
const MAX_MANIFEST_BYTES: usize = 262_144;
const MAX_RESOURCES: usize = 1_000;
const MAX_ACCEPTS: usize = 4;
const MAX_URL_BYTES: usize = 2_048;
const MAX_DESCRIPTION_BYTES: usize = 512;
const MAX_MIME_TYPE_BYTES: usize = 128;
const MAX_SERVICE_NAME_BYTES: usize = 32;
const MAX_TAGS: usize = 5;
const MAX_TAG_BYTES: usize = 32;
const MAX_EXTENSION_NAME_BYTES: usize = 64;
const MAX_EXTENSIONS_BYTES: usize = 65_536;
const MAX_JSON_DEPTH: usize = 16;
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1_000;
const MAX_FILTER_BYTES: usize = 200;
const MAX_QUERY_BYTES: usize = 4_096;
const OPERATOR_OWNED_RESOURCE_HOSTS: [&str; 5] = [
    "x402-demo.mikedotexe.com",
    "x402-demo-test.mikedotexe.com",
    "x402-demo-base.mikedotexe.com",
    "merchant-near.mikedotexe.com",
    "merchant-base.mikedotexe.com",
];

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    resources: Vec<DiscoveryResource>,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("invalid catalog JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid catalog: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogQuery {
    pub resource_type: Option<String>,
    pub pay_to: Option<String>,
    pub scheme: Option<String>,
    pub network: Option<String>,
    pub extension: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryResourcesResponse {
    pub x402_version: u8,
    pub items: Vec<DiscoveryResource>,
    pub pagination: DiscoveryPagination,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiscoveryPagination {
    pub limit: usize,
    pub offset: usize,
    pub total: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoveryPaymentRequirements {
    pub scheme: String,
    pub network: String,
    pub asset: String,
    pub amount: String,
    pub pay_to: String,
    pub max_timeout_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<PaymentExtra>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PaymentExtra {
    Evm(Eip712Domain),
    Near(EmptyExtra),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyExtra {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Eip712Domain {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryResource {
    pub resource: String,
    #[serde(rename = "type")]
    pub resource_type: String,
    pub x402_version: u8,
    pub accepts: Vec<DiscoveryPaymentRequirements>,
    pub last_updated: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<HashMap<String, Value>>,
    #[serde(skip)]
    updated_at: DateTime<FixedOffset>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogManifest {
    schema_version: u8,
    resources: Vec<CatalogEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogEntry {
    resource: String,
    #[serde(rename = "type")]
    resource_type: String,
    x402_version: u8,
    accepts: Vec<DiscoveryPaymentRequirements>,
    last_updated: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    service_name: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    icon_url: Option<String>,
    #[serde(default)]
    extensions: Option<HashMap<String, Value>>,
    admission: Admission,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Admission {
    reviewed_at: String,
    opt_in_evidence_url: String,
    pay_to_control_evidence_url: String,
}

impl Catalog {
    pub fn load_embedded_for(config: &ServiceConfig) -> Result<Self, CatalogError> {
        Self::from_json_for(EMBEDDED_MANIFEST, &config.network, &config.asset)
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn from_json_for(
        source: &str,
        network: &str,
        asset: &str,
    ) -> Result<Self, CatalogError> {
        if source.len() > MAX_MANIFEST_BYTES {
            return Err(CatalogError::Invalid(
                "manifest exceeds the 256 KiB bound".to_owned(),
            ));
        }
        let manifest: CatalogManifest = serde_json::from_str(source)?;
        if manifest.schema_version != MANIFEST_VERSION {
            return Err(CatalogError::Invalid(format!(
                "schemaVersion must be {MANIFEST_VERSION}"
            )));
        }
        if manifest.resources.len() > MAX_RESOURCES {
            return Err(CatalogError::Invalid(format!(
                "resources must contain at most {MAX_RESOURCES} entries"
            )));
        }

        let mut seen = HashSet::new();
        let mut selected = Vec::new();
        for entry in manifest.resources {
            let (normalized_resource, updated_at) = validate_entry(&entry)?;
            if !seen.insert(normalized_resource) {
                return Err(CatalogError::Invalid(
                    "resource URLs must be unique".to_owned(),
                ));
            }
            let entry_network = &entry.accepts[0].network;
            let entry_asset = &entry.accepts[0].asset;
            if entry_network == network && asset_matches(network, entry_asset, asset) {
                selected.push(entry.into_resource(updated_at));
            }
        }
        selected.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.resource.cmp(&right.resource))
        });
        Ok(Self {
            resources: selected,
        })
    }

    pub fn list(&self, query: &CatalogQuery) -> DiscoveryResourcesResponse {
        let matching = self
            .resources
            .iter()
            .filter(|resource| resource_matches(resource, query))
            .cloned()
            .collect::<Vec<_>>();
        let total = matching.len();
        let items = matching
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect();
        DiscoveryResourcesResponse {
            x402_version: 2,
            items,
            pagination: DiscoveryPagination {
                limit: query.limit,
                offset: query.offset,
                total,
            },
        }
    }
}

impl CatalogQuery {
    pub fn parse(raw: Option<&str>) -> Result<Self, CatalogError> {
        let raw = raw.unwrap_or_default();
        if raw.len() > MAX_QUERY_BYTES {
            return invalid("discovery query exceeds the 4 KiB bound");
        }
        validate_raw_query(raw)?;
        let mut query = Self {
            limit: DEFAULT_LIMIT,
            ..Self::default()
        };
        let mut seen = HashSet::new();
        for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
            let key = key.into_owned();
            let value = value.into_owned();
            if !seen.insert(key.clone()) {
                return Err(CatalogError::Invalid(format!(
                    "duplicate query parameter {key}"
                )));
            }
            match key.as_str() {
                "type" => query.resource_type = Some(validate_filter(key.as_str(), value)?),
                "payTo" => query.pay_to = Some(validate_filter(key.as_str(), value)?),
                "scheme" => query.scheme = Some(validate_filter(key.as_str(), value)?),
                "network" => query.network = Some(validate_filter(key.as_str(), value)?),
                "extensions" => query.extension = Some(validate_filter(key.as_str(), value)?),
                "limit" => {
                    query.limit = parse_bounded_integer("limit", &value, 1, MAX_LIMIT)?;
                }
                "offset" => {
                    query.offset = parse_bounded_integer("offset", &value, 0, usize::MAX)?;
                }
                _ => {
                    return Err(CatalogError::Invalid(format!(
                        "unknown query parameter {key}"
                    )));
                }
            }
        }
        Ok(query)
    }
}

impl Default for CatalogQuery {
    fn default() -> Self {
        Self {
            resource_type: None,
            pay_to: None,
            scheme: None,
            network: None,
            extension: None,
            limit: DEFAULT_LIMIT,
            offset: 0,
        }
    }
}

impl CatalogEntry {
    fn into_resource(self, updated_at: DateTime<FixedOffset>) -> DiscoveryResource {
        DiscoveryResource {
            resource: self.resource,
            resource_type: self.resource_type,
            x402_version: self.x402_version,
            accepts: self.accepts,
            last_updated: self.last_updated,
            description: self.description,
            mime_type: self.mime_type,
            service_name: self.service_name,
            tags: self.tags,
            icon_url: self.icon_url,
            extensions: self.extensions,
            updated_at,
        }
    }
}

fn validate_entry(entry: &CatalogEntry) -> Result<(String, DateTime<FixedOffset>), CatalogError> {
    let resource = validate_public_https_url("resource", &entry.resource)?;
    if resource.host_str().is_some_and(|host| {
        OPERATOR_OWNED_RESOURCE_HOSTS
            .iter()
            .any(|owned| host.eq_ignore_ascii_case(owned))
    }) {
        return invalid("operator-owned reference resources cannot enter the public catalog");
    }
    if entry.resource_type != "http" {
        return invalid("type must be http");
    }
    if entry.x402_version != 2 {
        return invalid("x402Version must be 2");
    }
    if entry.accepts.is_empty() || entry.accepts.len() > MAX_ACCEPTS {
        return invalid("accepts must contain between one and four requirements");
    }
    let network = entry.accepts[0].network.as_str();
    let asset = entry.accepts[0].asset.as_str();
    for requirements in &entry.accepts {
        validate_requirements(requirements)?;
        if requirements.network != network || !asset_matches(network, &requirements.asset, asset) {
            return invalid("all accepts entries must use one network and canonical asset");
        }
    }
    let updated_at = validate_timestamp("lastUpdated", &entry.last_updated)?;
    validate_timestamp("admission.reviewedAt", &entry.admission.reviewed_at)?;
    validate_public_https_url(
        "admission.optInEvidenceUrl",
        &entry.admission.opt_in_evidence_url,
    )?;
    validate_public_https_url(
        "admission.payToControlEvidenceUrl",
        &entry.admission.pay_to_control_evidence_url,
    )?;
    validate_optional_text(
        "description",
        entry.description.as_deref(),
        MAX_DESCRIPTION_BYTES,
        false,
    )?;
    validate_optional_text(
        "mimeType",
        entry.mime_type.as_deref(),
        MAX_MIME_TYPE_BYTES,
        true,
    )?;
    validate_optional_text(
        "serviceName",
        entry.service_name.as_deref(),
        MAX_SERVICE_NAME_BYTES,
        true,
    )?;
    if let Some(tags) = &entry.tags {
        if tags.is_empty() || tags.len() > MAX_TAGS {
            return invalid("tags must contain between one and five values");
        }
        let mut seen = HashSet::new();
        for tag in tags {
            validate_text("tag", tag, MAX_TAG_BYTES, true)?;
            if !seen.insert(tag.to_ascii_lowercase()) {
                return invalid("tags must be unique ignoring ASCII case");
            }
        }
    }
    if let Some(icon_url) = &entry.icon_url {
        validate_public_https_url("iconUrl", icon_url)?;
    }
    let Some(extensions) = &entry.extensions else {
        return invalid("extensions.bazaar is required");
    };
    let serialized = serde_json::to_vec(extensions)?;
    if serialized.len() > MAX_EXTENSIONS_BYTES {
        return invalid("extensions exceed the 64 KiB bound");
    }
    for key in extensions.keys() {
        validate_text("extension name", key, MAX_EXTENSION_NAME_BYTES, true)?;
    }
    if extensions
        .values()
        .any(|value| json_depth(value) > MAX_JSON_DEPTH)
    {
        return invalid("extensions exceed the maximum JSON depth");
    }
    let Some(Value::Object(bazaar)) = extensions.get("bazaar") else {
        return invalid("extensions.bazaar must be an object");
    };
    if !matches!(bazaar.get("info"), Some(Value::Object(_)))
        || !matches!(bazaar.get("schema"), Some(Value::Object(_)))
    {
        return invalid("extensions.bazaar must contain object info and schema fields");
    }
    Ok((resource.to_string(), updated_at))
}

fn validate_requirements(requirements: &DiscoveryPaymentRequirements) -> Result<(), CatalogError> {
    if requirements.scheme != "exact" {
        return invalid("accepts.scheme must be exact");
    }
    let Some(expected_asset) = canonical_usdc_asset(&requirements.network) else {
        return invalid("accepts.network is not a supported repository profile");
    };
    if !asset_matches(&requirements.network, &requirements.asset, expected_asset) {
        return invalid("accepts.asset is not canonical Circle USDC for its network");
    }
    if requirements.amount.is_empty()
        || requirements.amount.starts_with('0')
        || !requirements
            .amount
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        || requirements.amount.parse::<u128>().is_err()
    {
        return invalid("accepts.amount must be a canonical positive atomic integer");
    }
    if requirements.max_timeout_seconds == 0 {
        return invalid("accepts.maxTimeoutSeconds must be positive");
    }
    if requirements.network.starts_with("eip155:") {
        if !is_evm_address(&requirements.pay_to) {
            return invalid("accepts.payTo must be a 20-byte EVM address");
        }
        let Some((expected_name, expected_version)) =
            canonical_eip712_domain(&requirements.network)
        else {
            return invalid("Base requirements are missing a canonical EIP-712 domain");
        };
        let Some(PaymentExtra::Evm(domain)) = &requirements.extra else {
            return invalid("Base requirements must include the canonical EIP-712 domain");
        };
        if domain.name != expected_name || domain.version != expected_version {
            return invalid("Base requirements use the wrong EIP-712 domain");
        }
    } else {
        requirements.pay_to.parse::<AccountId>().map_err(|error| {
            CatalogError::Invalid(format!("accepts.payTo is not a NEAR account ID: {error}"))
        })?;
        if !matches!(requirements.extra, None | Some(PaymentExtra::Near(_))) {
            return invalid("NEAR requirements cannot carry an EIP-712 domain");
        }
    }
    Ok(())
}

fn validate_public_https_url(name: &str, value: &str) -> Result<Url, CatalogError> {
    if value.is_empty() || value.len() > MAX_URL_BYTES {
        return invalid(format!("{name} must contain at most {MAX_URL_BYTES} bytes"));
    }
    let parsed = Url::parse(value)
        .map_err(|error| CatalogError::Invalid(format!("{name} is not a valid URL: {error}")))?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return invalid(format!(
            "{name} must be HTTPS without credentials or a fragment"
        ));
    }
    let Some(host) = parsed.host_str() else {
        return invalid(format!("{name} must contain a host"));
    };
    if host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().is_ok() {
        return invalid(format!("{name} must use a public DNS hostname"));
    }
    Ok(parsed)
}

fn validate_timestamp(name: &str, value: &str) -> Result<DateTime<FixedOffset>, CatalogError> {
    DateTime::parse_from_rfc3339(value)
        .map_err(|error| CatalogError::Invalid(format!("{name} is not RFC 3339: {error}")))
}

fn validate_optional_text(
    name: &str,
    value: Option<&str>,
    maximum: usize,
    ascii_only: bool,
) -> Result<(), CatalogError> {
    if let Some(value) = value {
        validate_text(name, value, maximum, ascii_only)?;
    }
    Ok(())
}

fn validate_text(
    name: &str,
    value: &str,
    maximum: usize,
    ascii_only: bool,
) -> Result<(), CatalogError> {
    if value.is_empty()
        || value.len() > maximum
        || value.chars().any(char::is_control)
        || (ascii_only && !value.is_ascii())
    {
        return invalid(format!(
            "{name} must be non-empty, bounded, and free of control characters"
        ));
    }
    Ok(())
}

fn validate_filter(name: &str, value: String) -> Result<String, CatalogError> {
    validate_text(name, &value, MAX_FILTER_BYTES, false)?;
    Ok(value)
}

fn parse_bounded_integer(
    name: &str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<usize, CatalogError> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return invalid(format!("{name} must be an unsigned integer"));
    }
    let parsed = value
        .parse::<usize>()
        .map_err(|_| CatalogError::Invalid(format!("{name} is too large")))?;
    if !(minimum..=maximum).contains(&parsed) {
        return invalid(format!("{name} must be between {minimum} and {maximum}"));
    }
    Ok(parsed)
}

fn validate_raw_query(raw: &str) -> Result<(), CatalogError> {
    for component in raw.split('&') {
        let mut decoded = Vec::with_capacity(component.len());
        let bytes = component.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'%' => {
                    if index + 2 >= bytes.len()
                        || !bytes[index + 1].is_ascii_hexdigit()
                        || !bytes[index + 2].is_ascii_hexdigit()
                    {
                        return invalid("query contains invalid percent encoding");
                    }
                    let high = hex_value(bytes[index + 1]);
                    let low = hex_value(bytes[index + 2]);
                    decoded.push((high << 4) | low);
                    index += 3;
                }
                b'+' => {
                    decoded.push(b' ');
                    index += 1;
                }
                byte => {
                    decoded.push(byte);
                    index += 1;
                }
            }
        }
        if std::str::from_utf8(&decoded).is_err() {
            return invalid("query must be valid UTF-8");
        }
    }
    Ok(())
}

const fn hex_value(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => 0,
    }
}

fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1 + values.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(values) => 1 + values.values().map(json_depth).max().unwrap_or(0),
        _ => 1,
    }
}

fn asset_matches(network: &str, left: &str, right: &str) -> bool {
    if network.starts_with("eip155:") {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

fn resource_matches(resource: &DiscoveryResource, query: &CatalogQuery) -> bool {
    query
        .resource_type
        .as_ref()
        .is_none_or(|value| resource.resource_type == *value)
        && query.network.as_ref().is_none_or(|value| {
            resource
                .accepts
                .iter()
                .any(|requirements| requirements.network == *value)
        })
        && query.scheme.as_ref().is_none_or(|value| {
            resource
                .accepts
                .iter()
                .any(|requirements| requirements.scheme == *value)
        })
        && query.pay_to.as_ref().is_none_or(|value| {
            resource.accepts.iter().any(|requirements| {
                if requirements.network.starts_with("eip155:") {
                    requirements.pay_to.eq_ignore_ascii_case(value)
                } else {
                    requirements.pay_to == *value
                }
            })
        })
        && query.extension.as_ref().is_none_or(|value| {
            resource
                .extensions
                .as_ref()
                .is_some_and(|extensions| extensions.contains_key(value))
        })
}

fn invalid<T>(message: impl Into<String>) -> Result<T, CatalogError> {
    Err(CatalogError::Invalid(message.into()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::config::{
        BASE_MAINNET_USDC, BASE_SEPOLIA_USDC, NEAR_MAINNET_USDC, NEAR_TESTNET_USDC,
    };

    const BASE: &str = "eip155:8453";

    fn valid_entry() -> Value {
        json!({
            "resource": "https://merchant.example/v1/evidence",
            "type": "http",
            "x402Version": 2,
            "accepts": [{
                "scheme": "exact",
                "network": BASE,
                "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                "amount": "1000",
                "payTo": "0x1111111111111111111111111111111111111111",
                "maxTimeoutSeconds": 300,
                "extra": {"name": "USD Coin", "version": "2"}
            }],
            "lastUpdated": "2026-07-31T12:00:00Z",
            "description": "Independent evidence API",
            "mimeType": "application/json",
            "serviceName": "Independent API",
            "tags": ["evidence", "base"],
            "iconUrl": "https://merchant.example/icon.png",
            "extensions": {
                "bazaar": {
                    "info": {"input": {"type": "http", "method": "POST"}},
                    "schema": {"type": "object"}
                }
            },
            "admission": {
                "reviewedAt": "2026-07-31T12:00:00Z",
                "optInEvidenceUrl": "https://github.com/example/project/issues/1",
                "payToControlEvidenceUrl": "https://merchant.example/payments"
            }
        })
    }

    fn manifest(entry: &Value) -> String {
        json!({"schemaVersion": 1, "resources": [entry]}).to_string()
    }

    fn replace(entry: &mut Value, pointer: &str, value: Value) {
        let Some(target) = entry.pointer_mut(pointer) else {
            std::process::abort();
        };
        *target = value;
    }

    #[test]
    fn embedded_catalog_is_valid_and_empty() -> Result<(), CatalogError> {
        let catalog = Catalog::from_json_for(EMBEDDED_MANIFEST, BASE, BASE_MAINNET_USDC)?;
        assert_eq!(
            catalog.list(&CatalogQuery::parse(None)?).pagination.total,
            0
        );
        Ok(())
    }

    #[test]
    fn valid_entry_filters_and_paginates() -> Result<(), CatalogError> {
        let catalog = Catalog::from_json_for(&manifest(&valid_entry()), BASE, BASE_MAINNET_USDC)?;
        let response = catalog.list(&CatalogQuery::parse(Some(
            "network=eip155%3A8453&payTo=0x1111111111111111111111111111111111111111&extensions=bazaar&limit=1&offset=0",
        ))?);
        assert_eq!(response.pagination.total, 1);
        assert_eq!(response.items.len(), 1);
        assert_eq!(response.items[0].x402_version, 2);
        assert_eq!(
            catalog
                .list(&CatalogQuery::parse(Some("extensions=BAZAAR"))?)
                .pagination
                .total,
            0
        );
        Ok(())
    }

    #[test]
    fn evm_pay_to_filter_is_case_insensitive() -> Result<(), CatalogError> {
        let mut entry = valid_entry();
        let pay_to = format!("0xAbCd{}", "0".repeat(36));
        entry["accepts"][0]["payTo"] = json!(pay_to);
        let catalog = Catalog::from_json_for(&manifest(&entry), BASE, BASE_MAINNET_USDC)?;
        let query = format!("payTo={}", pay_to.to_ascii_lowercase());
        assert_eq!(
            catalog
                .list(&CatalogQuery::parse(Some(&query))?)
                .pagination
                .total,
            1
        );
        Ok(())
    }

    #[test]
    fn manifest_rejects_unknown_fields_and_duplicates() {
        let mut unknown = valid_entry();
        unknown["unexpected"] = json!(true);
        assert!(Catalog::from_json_for(&manifest(&unknown), BASE, BASE_MAINNET_USDC).is_err());

        let entry = valid_entry();
        let duplicate = json!({"schemaVersion": 1, "resources": [entry.clone(), entry]});
        assert!(Catalog::from_json_for(&duplicate.to_string(), BASE, BASE_MAINNET_USDC).is_err());
    }

    #[test]
    fn manifest_rejects_wrong_profile_domain_and_payment_fields() {
        for (pointer, value) in [
            ("/accepts/0/network", json!("eip155:1")),
            (
                "/accepts/0/asset",
                json!("0x2222222222222222222222222222222222222222"),
            ),
            ("/accepts/0/amount", json!("0")),
            ("/accepts/0/payTo", json!("not-an-address")),
            ("/accepts/0/maxTimeoutSeconds", json!(0)),
            ("/accepts/0/extra/name", json!("USDC")),
            ("/x402Version", json!(1)),
            ("/type", json!("mcp")),
        ] {
            let mut entry = valid_entry();
            replace(&mut entry, pointer, value);
            assert!(
                Catalog::from_json_for(&manifest(&entry), BASE, BASE_MAINNET_USDC).is_err(),
                "accepted invalid field at {pointer}"
            );
        }
    }

    #[test]
    fn manifest_rejects_bad_urls_timestamps_and_bazaar_shapes() {
        for (pointer, value) in [
            ("/resource", json!("http://merchant.example/resource")),
            ("/resource", json!("https://localhost/resource")),
            ("/lastUpdated", json!("yesterday")),
            (
                "/admission/optInEvidenceUrl",
                json!("https://127.0.0.1/issue"),
            ),
            ("/extensions/bazaar/info", json!(null)),
            ("/extensions/bazaar/schema", json!(true)),
        ] {
            let mut entry = valid_entry();
            replace(&mut entry, pointer, value);
            assert!(
                Catalog::from_json_for(&manifest(&entry), BASE, BASE_MAINNET_USDC).is_err(),
                "accepted invalid field at {pointer}"
            );
        }
    }

    #[test]
    fn manifest_rejects_operator_owned_reference_resources() {
        for host in OPERATOR_OWNED_RESOURCE_HOSTS {
            let mut entry = valid_entry();
            entry["resource"] = json!(format!("https://{host}/work"));
            assert!(Catalog::from_json_for(&manifest(&entry), BASE, BASE_MAINNET_USDC).is_err());
        }
    }

    #[test]
    fn query_rejects_unknown_duplicate_and_invalid_bounds() {
        for query in [
            "unknown=value",
            "network=a&network=b",
            "limit=0",
            "limit=1001",
            "limit=one",
            "limit=01",
            "offset=-1",
            "offset=00",
            "type=",
            "network=%ZZ",
            "network=%FF",
        ] {
            assert!(
                CatalogQuery::parse(Some(query)).is_err(),
                "accepted {query}"
            );
        }
        assert!(CatalogQuery::parse(Some(&"a".repeat(MAX_QUERY_BYTES + 1))).is_err());
    }

    #[test]
    fn all_four_network_profiles_are_accepted() -> Result<(), CatalogError> {
        for (network, asset, pay_to, extra) in [
            (
                "near:mainnet",
                NEAR_MAINNET_USDC,
                "merchant.near",
                json!({}),
            ),
            (
                "near:testnet",
                NEAR_TESTNET_USDC,
                "merchant.testnet",
                json!({}),
            ),
            (
                "eip155:8453",
                BASE_MAINNET_USDC,
                "0x1111111111111111111111111111111111111111",
                json!({"name": "USD Coin", "version": "2"}),
            ),
            (
                "eip155:84532",
                BASE_SEPOLIA_USDC,
                "0x1111111111111111111111111111111111111111",
                json!({"name": "USDC", "version": "2"}),
            ),
        ] {
            let mut entry = valid_entry();
            replace(&mut entry, "/accepts/0/network", json!(network));
            replace(&mut entry, "/accepts/0/asset", json!(asset));
            replace(&mut entry, "/accepts/0/payTo", json!(pay_to));
            replace(&mut entry, "/accepts/0/extra", extra);
            let catalog = Catalog::from_json_for(&manifest(&entry), network, asset)?;
            assert_eq!(catalog.list(&CatalogQuery::default()).pagination.total, 1);
        }
        Ok(())
    }

    #[test]
    fn manifest_rejects_mixed_networks_and_excessive_metadata() {
        let mut mixed = valid_entry();
        let mut second = mixed["accepts"][0].clone();
        second["network"] = json!("eip155:84532");
        second["asset"] = json!(BASE_SEPOLIA_USDC);
        second["extra"] = json!({"name": "USDC", "version": "2"});
        let Some(accepts) = mixed["accepts"].as_array_mut() else {
            std::process::abort();
        };
        accepts.push(second);
        assert!(Catalog::from_json_for(&manifest(&mixed), BASE, BASE_MAINNET_USDC).is_err());

        let mut long_description = valid_entry();
        long_description["description"] = json!("x".repeat(MAX_DESCRIPTION_BYTES + 1));
        assert!(
            Catalog::from_json_for(&manifest(&long_description), BASE, BASE_MAINNET_USDC).is_err()
        );

        let mut too_many_tags = valid_entry();
        too_many_tags["tags"] = json!(["one", "two", "three", "four", "five", "six"]);
        assert!(
            Catalog::from_json_for(&manifest(&too_many_tags), BASE, BASE_MAINNET_USDC).is_err()
        );
    }

    #[test]
    fn ordering_uses_timestamp_value_then_resource() -> Result<(), CatalogError> {
        let first = valid_entry();
        let mut newest = valid_entry();
        newest["resource"] = json!("https://merchant.example/newest");
        newest["lastUpdated"] = json!("2026-07-31T08:30:00-04:00");
        let mut same_time = valid_entry();
        same_time["resource"] = json!("https://merchant.example/a-same-time");
        same_time["lastUpdated"] = json!("2026-07-31T12:00:00Z");
        let source = json!({
            "schemaVersion": 1,
            "resources": [first, newest, same_time]
        })
        .to_string();
        let catalog = Catalog::from_json_for(&source, BASE, BASE_MAINNET_USDC)?;
        let items = catalog.list(&CatalogQuery::default()).items;
        assert_eq!(items[0].resource, "https://merchant.example/newest");
        assert_eq!(items[1].resource, "https://merchant.example/a-same-time");
        assert_eq!(items[2].resource, "https://merchant.example/v1/evidence");
        Ok(())
    }
}
