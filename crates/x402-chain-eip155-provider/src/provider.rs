//! The live EVM settlement provider: RPC-facing verify, signer head, and
//! broadcast, built on upstream `x402-chain-eip155`.
//!
//! This is the network-touching half of the durable path. Verification is reused
//! wholesale from upstream (`verify_eip3009_payment`: EIP-712 domain, EIP-1271 /
//! EIP-6492 signatures, on-chain balance and a transfer simulation); this
//! provider adds the durable submit: it snapshots the signer's account nonce,
//! hands the offline core ([`crate::prepare`] / [`crate::settle`]) what it needs
//! to sign a journalable transaction, and broadcasts the signed bytes raw —
//! always reporting `Pending`, because an EVM outcome is never trusted at
//! submission (confirmation-depth resolution lands in 5c).
//!
//! The RPC methods here are exercised against Base Sepolia in the 5e/5f drills;
//! the offline mapping ([`classify_verify_error`]) is unit-tested.

use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_provider::Provider;
use alloy_signer_local::PrivateKeySigner;
use std::fmt;
use url::Url;
use x402_types::chain::{ChainId, FromConfig};
use x402_types::proto;
use x402_types::proto::v2::VerifyResponse;
use x402_types::scheme::X402SchemeFacilitatorError;

use x402_chain_eip155::chain::config::{Eip155ChainConfig, Eip155ChainConfigInner};
use x402_chain_eip155::chain::{Eip155ChainReference, Eip155MetaTransactionProvider};
use x402_chain_eip155::v2_eip155_exact::FacilitatorVerifyRequest;
use x402_chain_eip155::v2_eip155_exact::eip3009::verify_eip3009_payment;

use crate::prepare::Erc3009Authorization;
use crate::settle::{build_transfer_domain, eip712_transfer_hash};

/// A verified EVM payment: the neutral facts the engine keys on, plus the
/// authorization and signature the durable submit path needs to build calldata.
#[derive(Clone)]
pub struct EvmVerifiedPayment {
    /// Payer address recovered/authorized by verification.
    pub payer: Address,
    /// The EIP-712 transfer digest — the payment's canonical identity and the
    /// journal's idempotency anchor.
    pub payment_hash: B256,
    /// Token contract the transfer settles on.
    pub asset: Address,
    /// Recipient address.
    pub pay_to: Address,
    /// Amount in the token's smallest unit.
    pub amount: U256,
    /// The ERC-3009 authorization the payer signed.
    pub authorization: Erc3009Authorization,
    /// The payer's raw signature bytes (opaque; classified at prepare time).
    signature: Bytes,
}

impl EvmVerifiedPayment {
    /// The payer's raw signature bytes.
    #[must_use]
    pub fn signature(&self) -> &Bytes {
        &self.signature
    }
}

impl fmt::Debug for EvmVerifiedPayment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EvmVerifiedPayment")
            .field("payer", &self.payer)
            .field("payment_hash", &self.payment_hash)
            .field("asset", &self.asset)
            .field("pay_to", &self.pay_to)
            .field("amount", &self.amount)
            .field("authorization", &self.authorization)
            .field("signature", &"<redacted>")
            .finish()
    }
}

/// A neutral verification rejection: a machine reason plus whether it reflects an
/// unavailable/ambiguous RPC lookup (→ retry / 503) rather than a definitively
/// invalid payment (→ reject). Mirrors the NEAR provider's rejection shape.
#[derive(Clone, Debug)]
pub struct EvmVerifyRejection {
    /// Stable-ish machine reason for logging and the HTTP disposition.
    pub reason: String,
    /// Whether the failure is an ambiguous/unavailable on-chain lookup rather
    /// than a definitive rejection.
    pub rpc_ambiguous: bool,
}

impl EvmVerifyRejection {
    fn definitive(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            rpc_ambiguous: false,
        }
    }
}

/// Classify an upstream facilitator error into a neutral rejection. An on-chain
/// failure is an ambiguous lookup (retryable / 503); a verification error is a
/// definitive rejection.
#[must_use]
pub fn classify_verify_error(error: &X402SchemeFacilitatorError) -> EvmVerifyRejection {
    match error {
        X402SchemeFacilitatorError::OnchainFailure(detail) => EvmVerifyRejection {
            reason: format!("onchain_failure: {detail}"),
            rpc_ambiguous: true,
        },
        X402SchemeFacilitatorError::PaymentVerification(inner) => {
            EvmVerifyRejection::definitive(inner.to_string())
        }
    }
}

/// Why constructing the provider failed.
#[derive(Debug, thiserror::Error)]
pub enum EvmConnectError {
    /// The upstream chain config could not be assembled.
    #[error("evm chain config invalid: {0}")]
    Config(String),
    /// The upstream provider failed to connect / validate required contracts.
    #[error("evm provider connect failed: {0}")]
    Connect(String),
}

/// Why an RPC-facing operation failed.
#[derive(Debug, thiserror::Error)]
pub enum EvmRpcError {
    /// The JSON-RPC call failed at the transport.
    #[error("evm rpc call failed: {0}")]
    Rpc(String),
}

/// The live EVM settlement provider.
#[derive(Debug)]
pub struct EvmChainProvider {
    upstream: x402_chain_eip155::chain::Eip155ChainProvider,
    signer: PrivateKeySigner,
    chain_id: u64,
    asset: Address,
    required_confirmations: u64,
}

impl EvmChainProvider {
    /// Connect to an EVM chain: build the upstream provider (which validates the
    /// required x402 contracts on-chain) and retain the facilitator signer for
    /// durable submission. The `signer` is loaded from a mode-0600 credential by
    /// the caller; its key is never logged.
    ///
    /// # Errors
    ///
    /// Returns [`EvmConnectError`] if the chain config cannot be assembled or the
    /// upstream provider fails to connect / validate contracts.
    pub async fn connect(
        chain_id: u64,
        rpc_urls: &[Url],
        signer: PrivateKeySigner,
        asset: Address,
        required_confirmations: u64,
    ) -> Result<Self, EvmConnectError> {
        // Upstream builds its signing wallet from a config document; hand it the
        // same key we sign with. The hex key is transient and never logged.
        let key_hex = format!("0x{}", hex::encode(signer.to_bytes().as_slice()));
        let rpc = rpc_urls
            .iter()
            .map(|url| serde_json::json!({ "http": url.as_str() }))
            .collect::<Vec<_>>();
        let inner_document = serde_json::json!({
            "eip1559": true,
            "flashblocks": false,
            "signers": [key_hex],
            "rpc": rpc,
        });
        let inner: Eip155ChainConfigInner = serde_json::from_value(inner_document)
            .map_err(|error| EvmConnectError::Config(error.to_string()))?;
        let config = Eip155ChainConfig {
            chain_reference: Eip155ChainReference::new(chain_id),
            inner,
        };
        let upstream =
            <x402_chain_eip155::chain::Eip155ChainProvider as FromConfig<Eip155ChainConfig>>::from_config(&config)
                .await
                .map_err(|error| EvmConnectError::Connect(error.to_string()))?;
        Ok(Self {
            upstream,
            signer,
            chain_id,
            asset,
            required_confirmations,
        })
    }

    /// The facilitator signer (`from`) address.
    #[must_use]
    pub fn signer_address(&self) -> Address {
        self.signer.address()
    }

    /// The facilitator signer, for the durable prepare step.
    #[must_use]
    pub fn signer(&self) -> &PrivateKeySigner {
        &self.signer
    }

    /// The EIP-155 chain id this provider settles on.
    #[must_use]
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// The token contract this provider settles.
    #[must_use]
    pub fn asset(&self) -> Address {
        self.asset
    }

    /// Confirmation depth required before an outcome is treated as terminal.
    #[must_use]
    pub fn required_confirmations(&self) -> u64 {
        self.required_confirmations
    }

    /// Verify a raw payment. Reuses upstream's authoritative EIP-3009 checks and,
    /// on success, returns the durable [`EvmVerifiedPayment`] the submit path
    /// consumes.
    ///
    /// # Errors
    ///
    /// Returns [`EvmVerifyRejection`] for an unsupported scheme, a malformed
    /// request, a mismatched asset, an invalid payment, or an ambiguous on-chain
    /// lookup.
    pub async fn verify(
        &self,
        request: &proto::VerifyRequest,
    ) -> Result<EvmVerifiedPayment, EvmVerifyRejection> {
        let parsed = FacilitatorVerifyRequest::try_from(request.clone())
            .map_err(|error| EvmVerifyRejection::definitive(error.to_string()))?;
        let (payload, requirements) = match parsed {
            FacilitatorVerifyRequest::Eip3009 {
                payment_payload,
                payment_requirements,
                ..
            } => (payment_payload, payment_requirements),
            FacilitatorVerifyRequest::Permit2 { .. } => {
                return Err(EvmVerifyRejection::definitive("unsupported_permit2_scheme"));
            }
        };

        let asset = Address::from(payload.accepted.asset);
        if asset != self.asset {
            return Err(EvmVerifyRejection::definitive("asset_mismatch"));
        }

        // Upstream's authoritative decision (domain, balance, simulation).
        let response = verify_eip3009_payment(&self.upstream, &payload, &requirements)
            .await
            .map_err(|error| classify_verify_error(&error))?;
        let payer = match response {
            VerifyResponse::Valid { payer } => payer,
            VerifyResponse::Invalid { reason, .. } => {
                return Err(EvmVerifyRejection::definitive(reason));
            }
        };
        let payer = payer
            .parse::<Address>()
            .map_err(|_| EvmVerifyRejection::definitive("invalid_payer_address"))?;

        let authorization = Erc3009Authorization {
            from: payload.payload.authorization.from,
            to: payload.payload.authorization.to,
            value: payload.payload.authorization.value,
            valid_after: U256::from(payload.payload.authorization.valid_after.as_secs()),
            valid_before: U256::from(payload.payload.authorization.valid_before.as_secs()),
            nonce: payload.payload.authorization.nonce,
        };
        let domain = build_transfer_domain(
            &payload.accepted.extra.name,
            &payload.accepted.extra.version,
            self.chain_id,
            asset,
        );
        Ok(EvmVerifiedPayment {
            payer,
            payment_hash: eip712_transfer_hash(&authorization, &domain),
            asset,
            pay_to: Address::from(payload.accepted.pay_to),
            amount: payload.accepted.amount,
            authorization,
            signature: payload.payload.signature,
        })
    }

    /// Snapshot the signer's next account (transaction) nonce.
    ///
    /// # Errors
    ///
    /// Returns [`EvmRpcError`] if the nonce lookup fails.
    pub async fn account_nonce(&self) -> Result<u64, EvmRpcError> {
        self.upstream
            .inner()
            .get_transaction_count(self.signer.address())
            .await
            .map_err(|error| EvmRpcError::Rpc(error.to_string()))
    }

    /// The signer's native-gas (ETH) balance in wei. Clamped into `u128`, which
    /// holds any realistic balance.
    ///
    /// # Errors
    ///
    /// Returns [`EvmRpcError`] if the balance lookup fails.
    pub async fn gas_balance_wei(&self) -> Result<u128, EvmRpcError> {
        let balance = self
            .upstream
            .inner()
            .get_balance(self.signer.address())
            .await
            .map_err(|error| EvmRpcError::Rpc(error.to_string()))?;
        Ok(u128::try_from(balance).unwrap_or(u128::MAX))
    }

    /// Broadcast a signed settlement transaction raw. The outcome is always
    /// treated as `Pending` by the engine (never terminal at submission), and the
    /// transaction hash is already known from the journaled `EvmPrepared`. A
    /// transport error here is recoverable — the reconcile loop rebroadcasts the
    /// same journaled bytes.
    ///
    /// # Errors
    ///
    /// Returns [`EvmRpcError`] if the node rejects the raw transaction.
    pub async fn broadcast_raw(&self, signed_tx_rlp: &[u8]) -> Result<(), EvmRpcError> {
        // Awaiting the call performs `eth_sendRawTransaction`; the returned
        // pending-tx watcher is intentionally dropped — confirmation is resolved
        // by the reconcile loop, not awaited inline.
        let _submitted = self
            .upstream
            .inner()
            .send_raw_transaction(signed_tx_rlp)
            .await
            .map_err(|error| EvmRpcError::Rpc(error.to_string()))?;
        Ok(())
    }

    /// Probe that the connected RPC reports the expected chain id and a live head.
    /// The chain-liveness half of readiness.
    pub async fn readiness_probe(&self) -> bool {
        let inner = self.upstream.inner();
        let chain_ok = matches!(inner.get_chain_id().await, Ok(id) if id == self.chain_id);
        let head_ok = inner.get_block_number().await.is_ok();
        chain_ok && head_ok
    }

    /// The CAIP-2 chain id, e.g. `eip155:84532`.
    #[must_use]
    pub fn caip2(&self) -> ChainId {
        ChainId::new("eip155", self.chain_id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x402_types::proto::PaymentVerificationError;

    #[test]
    fn onchain_failure_is_ambiguous_verification_error_is_definitive() {
        let onchain = X402SchemeFacilitatorError::OnchainFailure("rpc timeout".to_owned());
        let classified = classify_verify_error(&onchain);
        assert!(classified.rpc_ambiguous);
        assert!(classified.reason.contains("onchain_failure"));

        let verification =
            X402SchemeFacilitatorError::PaymentVerification(PaymentVerificationError::Expired);
        assert!(!classify_verify_error(&verification).rpc_ambiguous);
    }
}
