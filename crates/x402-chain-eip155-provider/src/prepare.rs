//! Durable EVM settlement preparation: the offline, deterministic core of the
//! "real way" submit path.
//!
//! On NEAR the facilitator signs the outer meta-transaction, journals the exact
//! signed bytes and their hash, and only then broadcasts — so a crash between
//! signing and confirmation is recoverable by replaying the journaled bytes and
//! reconciling by hash. This module gives the EVM path the same property:
//!
//! 1. [`transfer_with_authorization_bytes_calldata`] /
//!    [`transfer_with_authorization_vrs_calldata`] encode the ERC-3009
//!    `transferWithAuthorization` call the facilitator submits on the payer's
//!    behalf. The two overloads mirror upstream `x402-chain-eip155`: the
//!    `bytes`-signature form for deployed EIP-1271 smart-wallet signatures,
//!    and the split `(v, r, s)` form for plain EOA signatures. (Choosing the
//!    overload from the payer's signature shape happens in the provider, next to
//!    verification; this module only encodes.)
//! 2. [`sign_settlement_transaction`] builds and signs the EIP-1559 transaction
//!    that carries that calldata, returning an [`EvmPrepared`] whose RLP bytes
//!    and transaction hash are **durable**: journaled once, replayed verbatim on
//!    recovery, never re-signed.
//!
//! Everything here is pure and side-effect-free — no RPC, no clock — so it is
//! covered by deterministic golden tests. The signed transaction pins its gas
//! fee envelope; see [`EvmFeeEnvelope`] for the operational consequence.

use alloy_consensus::transaction::SignerRecoverable;
use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
use alloy_eips::eip2718::{Decodable2718, Encodable2718};
use alloy_eips::eip2930::AccessList;
use alloy_primitives::{Address, B256, Bytes, TxKind, U256};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::{Eip712Domain, SolCall};
use std::fmt;

// ERC-3009 `transferWithAuthorization`, both on-chain overloads. Kept in a
// private module so the `sol!`-generated (undocumented, non-`Debug`) call types
// stay crate-internal; only the encoded calldata leaves this file. The `bytes`
// overload is declared first so it maps to `_0Call` and the `(v, r, s)` overload
// to `_1Call`, matching upstream `x402-chain-eip155`.
#[allow(clippy::all, clippy::pedantic, missing_docs)]
mod abi {
    alloy_sol_types::sol! {
        interface IErc3009 {
            function transferWithAuthorization(
                address from,
                address to,
                uint256 value,
                uint256 validAfter,
                uint256 validBefore,
                bytes32 nonce,
                bytes signature
            ) external;

            function transferWithAuthorization(
                address from,
                address to,
                uint256 value,
                uint256 validAfter,
                uint256 validBefore,
                bytes32 nonce,
                uint8 v,
                bytes32 r,
                bytes32 s
            ) external;
        }
    }
}

/// The ERC-3009 authorization the payer signed, in the token's native units.
/// These fields are fixed by the verified payment; the facilitator only chooses
/// the signature encoding and wraps the call in a transaction it pays for.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Erc3009Authorization {
    /// Token owner authorizing the transfer (`from`).
    pub from: Address,
    /// Recipient of the transfer (`to`).
    pub to: Address,
    /// Amount to transfer, in the token's smallest unit.
    pub value: U256,
    /// Authorization is invalid before this Unix timestamp (inclusive).
    pub valid_after: U256,
    /// Authorization is invalid at/after this Unix timestamp (exclusive).
    pub valid_before: U256,
    /// Unique 32-byte replay-protection nonce (chain-enforced exactly-once).
    pub nonce: B256,
}

impl fmt::Debug for Erc3009Authorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Erc3009Authorization")
            .field("from", &"<redacted>")
            .field("to", &"<redacted>")
            .field("value", &"<redacted>")
            .field("valid_after", &"<redacted>")
            .field("valid_before", &"<redacted>")
            .field("nonce", &"<redacted>")
            .finish()
    }
}

/// The minimal replay/recovery identity retained from an ERC-3009
/// authorization before the facilitator transaction is prepared.
///
/// It intentionally excludes the payer signature and the duplicated
/// payer/payee/value fields. Those values already live in the canonical
/// settlement row and the signature is carried by the exact signed transaction
/// bytes once prepared.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct EvmAuthorizationIdentity {
    /// Unique ERC-3009 replay-protection nonce.
    pub nonce: B256,
    /// Authorization lower time bound.
    pub valid_after: U256,
    /// Authorization upper time bound.
    pub valid_before: U256,
}

impl fmt::Debug for EvmAuthorizationIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EvmAuthorizationIdentity")
            .field("nonce", &"<redacted>")
            .field("valid_after", &"<redacted>")
            .field("valid_before", &"<redacted>")
            .finish()
    }
}

/// Encode `transferWithAuthorization` with an opaque signature blob — the
/// ERC-3009 `bytes` overload used for deployed EIP-1271 smart-wallet
/// signatures. The provider rejects EIP-6492 before preparation.
#[must_use]
pub fn transfer_with_authorization_bytes_calldata(
    authorization: &Erc3009Authorization,
    signature: Bytes,
) -> Bytes {
    abi::IErc3009::transferWithAuthorization_0Call {
        from: authorization.from,
        to: authorization.to,
        value: authorization.value,
        validAfter: authorization.valid_after,
        validBefore: authorization.valid_before,
        nonce: authorization.nonce,
        signature,
    }
    .abi_encode()
    .into()
}

/// Encode `transferWithAuthorization` with a split `(v, r, s)` signature — the
/// standard ERC-3009 overload for plain EOA payer signatures.
#[must_use]
pub fn transfer_with_authorization_vrs_calldata(
    authorization: &Erc3009Authorization,
    v: u8,
    r: B256,
    s: B256,
) -> Bytes {
    abi::IErc3009::transferWithAuthorization_1Call {
        from: authorization.from,
        to: authorization.to,
        value: authorization.value,
        validAfter: authorization.valid_after,
        validBefore: authorization.valid_before,
        nonce: authorization.nonce,
        v,
        r,
        s,
    }
    .abi_encode()
    .into()
}

/// The facilitator signer's account head that pins a submission: the EVM chain
/// id the transaction commits to, and the account (transaction) nonce it burns.
/// Obtained from a fresh RPC snapshot and journaled so recovery replays the same
/// nonce rather than re-deriving it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvmSignerHead {
    /// EIP-155 chain id the signed transaction is bound to.
    pub chain_id: u64,
    /// Facilitator account (transaction) nonce this submission consumes.
    pub account_nonce: u64,
}

/// The gas/fee envelope baked into the signed transaction.
///
/// Once signed these values are immutable: an EIP-1559 transaction commits to
/// its `max_fee_per_gas`, so if the base fee climbs past the cap after
/// preparation the transaction simply waits until the market recedes rather than
/// failing. The cap must therefore be provisioned with generous headroom.
/// Fee-bump replacement (resubmitting the same nonce at a higher cap) is a
/// deliberate future refinement — see `docs/evm-v2-design.md`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvmFeeEnvelope {
    /// Maximum gas units the transaction may consume.
    pub gas_limit: u64,
    /// Maximum total fee per gas unit (wei) the facilitator will pay.
    pub max_fee_per_gas: u128,
    /// Maximum priority fee per gas unit (wei) offered to the proposer.
    pub max_priority_fee_per_gas: u128,
}

/// A durable, signed EVM settlement transaction.
///
/// `signed_tx_rlp` (the EIP-2718 typed-envelope bytes) and `tx_hash` are
/// journaled and replayed byte-for-byte on recovery; the transaction is never
/// re-signed once prepared.
#[derive(Clone)]
pub struct EvmPrepared {
    /// The mined-transaction hash, the reconciliation lookup key.
    pub tx_hash: B256,
    /// The facilitator signer address (`from`) that signed the transaction.
    pub signer_address: Address,
    /// The account nonce this transaction burns.
    pub account_nonce: u64,
    /// EIP-2718 typed-envelope RLP bytes, ready for `eth_sendRawTransaction`.
    signed_tx_rlp: Vec<u8>,
    /// Base L1 data fee estimated by the `GasPriceOracle` over the fully signed
    /// EIP-2718 bytes. `None` only for the pure offline signing helper; the live
    /// provider fills it before returning a prepared submission.
    estimated_l1_fee_wei: Option<u128>,
}

impl EvmPrepared {
    /// The signed, EIP-2718-encoded transaction bytes to broadcast (and to
    /// rebroadcast verbatim during recovery).
    #[must_use]
    pub fn signed_tx_rlp(&self) -> &[u8] {
        &self.signed_tx_rlp
    }

    /// Base L1 data fee estimated over the exact signed bytes.
    #[must_use]
    pub const fn estimated_l1_fee_wei(&self) -> Option<u128> {
        self.estimated_l1_fee_wei
    }

    pub(crate) const fn set_estimated_l1_fee_wei(&mut self, fee_wei: u128) {
        self.estimated_l1_fee_wei = Some(fee_wei);
    }
}

impl fmt::Debug for EvmPrepared {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EvmPrepared")
            .field("tx_hash", &"<redacted>")
            .field("signer_address", &"<redacted>")
            .field("account_nonce", &"<redacted>")
            .field("signed_tx_rlp", &"<redacted>")
            .field("estimated_l1_fee_wei", &self.estimated_l1_fee_wei)
            .finish()
    }
}

/// Why a settlement transaction could not be signed.
#[derive(Debug, thiserror::Error)]
pub enum EvmSignError {
    /// The signer failed to produce a signature over the transaction hash.
    #[error("signing the settlement transaction failed: {0}")]
    Sign(#[from] alloy_signer::Error),
}

/// Build and sign the EIP-1559 transaction that calls `transferWithAuthorization`
/// on `asset`, carrying the pre-encoded `calldata`.
///
/// Deterministic in every input (the signer uses RFC-6979 deterministic ECDSA),
/// so the returned RLP and hash are stable and safe to journal before any
/// network side effect.
///
/// # Errors
///
/// Returns [`EvmSignError::Sign`] if the signer cannot sign the transaction
/// hash.
pub fn sign_settlement_transaction(
    signer: &PrivateKeySigner,
    head: EvmSignerHead,
    fees: EvmFeeEnvelope,
    asset: Address,
    calldata: Bytes,
) -> Result<EvmPrepared, EvmSignError> {
    let transaction = TxEip1559 {
        chain_id: head.chain_id,
        nonce: head.account_nonce,
        gas_limit: fees.gas_limit,
        max_fee_per_gas: fees.max_fee_per_gas,
        max_priority_fee_per_gas: fees.max_priority_fee_per_gas,
        to: TxKind::Call(asset),
        value: U256::ZERO,
        access_list: AccessList::default(),
        input: calldata,
    };
    let signature_hash = transaction.signature_hash();
    let signature = signer.sign_hash_sync(&signature_hash)?;
    let envelope = TxEnvelope::Eip1559(transaction.into_signed(signature));
    Ok(EvmPrepared {
        tx_hash: *envelope.tx_hash(),
        signer_address: signer.address(),
        account_nonce: head.account_nonce,
        signed_tx_rlp: envelope.encoded_2718(),
        estimated_l1_fee_wei: None,
    })
}

/// Which supported ERC-3009 overload a durable transaction calls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Erc3009SignatureKind {
    /// Standard EOA `(v,r,s)` overload.
    Eoa,
    /// Opaque deployed-contract EIP-1271 signature overload.
    Eip1271,
}

/// Typed expectations used to bind durable signed bytes to one settlement row.
#[derive(Clone, Eq, PartialEq)]
pub struct ExpectedEvmSubmission {
    /// Journaled transaction hash.
    pub transaction_hash: B256,
    /// Configured facilitator signer.
    pub facilitator_signer: Address,
    /// Configured EIP-155 chain id.
    pub chain_id: u64,
    /// Journaled facilitator account nonce.
    pub account_nonce: u64,
    /// Configured token call target.
    pub token: Address,
    /// Exact configured gas limit.
    pub gas_limit: u64,
    /// Absolute fee cap; the signed transaction may not exceed it.
    pub max_fee_per_gas: u128,
    /// Verified payer.
    pub payer: Address,
    /// Required payee.
    pub payee: Address,
    /// Required token amount.
    pub value: U256,
    /// Journaled authorization lower time bound.
    pub valid_after: U256,
    /// Journaled authorization upper time bound.
    pub valid_before: U256,
    /// Journaled chain single-use anchor.
    pub authorization_nonce: B256,
    /// Canonical EIP-712 payment hash.
    pub payment_hash: B256,
    /// Exact token EIP-712 domain used during verification.
    pub domain: Eip712Domain,
}

impl fmt::Debug for ExpectedEvmSubmission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExpectedEvmSubmission")
            .field("transaction_hash", &"<redacted>")
            .field("facilitator_signer", &"<redacted>")
            .field("chain_id", &self.chain_id)
            .field("account_nonce", &"<redacted>")
            .field("token", &self.token)
            .field("gas_limit", &self.gas_limit)
            .field("max_fee_per_gas", &self.max_fee_per_gas)
            .field("payer", &"<redacted>")
            .field("payee", &"<redacted>")
            .field("value", &"<redacted>")
            .field("valid_after", &"<redacted>")
            .field("valid_before", &"<redacted>")
            .field("authorization_nonce", &"<redacted>")
            .field("payment_hash", &"<redacted>")
            .field("domain", &"<redacted>")
            .finish()
    }
}

/// Safe decoded facts returned after a durable submission passes every binding
/// check. Bearer signature bytes and authorization nonce are deliberately not
/// exposed through `Debug`.
#[derive(Clone, Eq, PartialEq)]
pub struct DecodedEvmSubmission {
    /// Transaction hash verified from the exact bytes.
    pub transaction_hash: B256,
    /// Recovered facilitator signer.
    pub facilitator_signer: Address,
    /// EIP-155 chain id.
    pub chain_id: u64,
    /// Facilitator account nonce.
    pub account_nonce: u64,
    /// Token call target.
    pub token: Address,
    /// Signed gas limit.
    pub gas_limit: u64,
    /// Signed maximum fee per gas.
    pub max_fee_per_gas: u128,
    /// Signed maximum priority fee per gas.
    pub max_priority_fee_per_gas: u128,
    /// Decoded ERC-3009 authorization.
    pub authorization: Erc3009Authorization,
    /// Recomputed canonical payment hash.
    pub payment_hash: B256,
    /// Supported ERC-3009 overload.
    pub signature_kind: Erc3009SignatureKind,
}

impl fmt::Debug for DecodedEvmSubmission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DecodedEvmSubmission")
            .field("transaction_hash", &"<redacted>")
            .field("facilitator_signer", &"<redacted>")
            .field("chain_id", &self.chain_id)
            .field("account_nonce", &"<redacted>")
            .field("token", &self.token)
            .field("gas_limit", &self.gas_limit)
            .field("max_fee_per_gas", &self.max_fee_per_gas)
            .field("max_priority_fee_per_gas", &self.max_priority_fee_per_gas)
            .field("authorization", &"<redacted>")
            .field("payment_hash", &"<redacted>")
            .field("signature_kind", &self.signature_kind)
            .finish()
    }
}

/// A deterministic durable-submission validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum StoredTransactionError {
    /// The bytes are not one canonical EIP-2718 envelope.
    #[error("stored EVM transaction envelope is invalid")]
    InvalidEnvelope,
    /// Extra bytes follow the first decoded envelope.
    #[error("stored EVM transaction contains trailing bytes")]
    TrailingBytes,
    /// Only EIP-1559 submissions are accepted.
    #[error("stored EVM transaction is not EIP-1559")]
    UnsupportedEnvelope,
    /// The computed transaction hash differs from the journal.
    #[error("stored EVM transaction hash does not match")]
    TransactionHashMismatch,
    /// The facilitator signature cannot be recovered.
    #[error("stored EVM transaction signer cannot be recovered")]
    SignerRecovery,
    /// The recovered signer differs from the configured facilitator.
    #[error("stored EVM transaction signer does not match")]
    SignerMismatch,
    /// The EIP-155 chain id differs from the deployment.
    #[error("stored EVM transaction chain id does not match")]
    ChainIdMismatch,
    /// The facilitator account nonce differs from the journal.
    #[error("stored EVM transaction account nonce does not match")]
    AccountNonceMismatch,
    /// The transaction does not call the configured token.
    #[error("stored EVM transaction token target does not match")]
    TokenMismatch,
    /// Settlement transactions must transfer no native ETH.
    #[error("stored EVM transaction transfers native value")]
    NativeValue,
    /// The gas limit differs from the configured durable policy.
    #[error("stored EVM transaction gas limit does not match")]
    GasLimitMismatch,
    /// The signed fee exceeds the configured cap.
    #[error("stored EVM transaction exceeds the fee cap")]
    FeeCapExceeded,
    /// The signed EIP-1559 fee envelope is internally invalid.
    #[error("stored EVM transaction fee envelope is invalid")]
    InvalidFeeEnvelope,
    /// The durable signer never creates access-list entries.
    #[error("stored EVM transaction carries an unexpected access list")]
    UnexpectedAccessList,
    /// The calldata is not one canonical supported ERC-3009 overload.
    #[error("stored EVM transaction calldata is invalid")]
    InvalidCalldata,
    /// A decoded ERC-3009 field differs from the settlement row.
    #[error("stored EVM authorization does not match the settlement")]
    AuthorizationMismatch,
    /// The supplied EIP-712 domain is inconsistent with chain/token identity.
    #[error("stored EVM payment domain does not match the deployment")]
    DomainMismatch,
    /// The decoded authorization does not recompute to the journaled payment hash.
    #[error("stored EVM payment hash does not match the authorization")]
    PaymentHashMismatch,
}

/// Validate journaled EVM transaction bytes during recovery and bind them to
/// the exact settlement row before any RPC receipt is trusted.
///
/// # Errors
///
/// Returns a typed [`StoredTransactionError`] for any malformed envelope,
/// identity mismatch, policy violation, or calldata mismatch.
pub fn validate_signed_transaction(
    signed_tx_rlp: &[u8],
    expected: &ExpectedEvmSubmission,
) -> Result<DecodedEvmSubmission, StoredTransactionError> {
    let mut remaining = signed_tx_rlp;
    let envelope = TxEnvelope::decode_2718(&mut remaining)
        .map_err(|_| StoredTransactionError::InvalidEnvelope)?;
    if !remaining.is_empty() {
        return Err(StoredTransactionError::TrailingBytes);
    }
    if envelope.encoded_2718() != signed_tx_rlp {
        return Err(StoredTransactionError::InvalidEnvelope);
    }
    let TxEnvelope::Eip1559(signed_eip1559) = &envelope else {
        return Err(StoredTransactionError::UnsupportedEnvelope);
    };
    if *envelope.tx_hash() != expected.transaction_hash {
        return Err(StoredTransactionError::TransactionHashMismatch);
    }
    let recovered_signer = envelope
        .recover_signer()
        .map_err(|_| StoredTransactionError::SignerRecovery)?;
    if recovered_signer != expected.facilitator_signer {
        return Err(StoredTransactionError::SignerMismatch);
    }
    let transaction = signed_eip1559.tx();
    if transaction.chain_id != expected.chain_id {
        return Err(StoredTransactionError::ChainIdMismatch);
    }
    if transaction.nonce != expected.account_nonce {
        return Err(StoredTransactionError::AccountNonceMismatch);
    }
    if transaction.to != TxKind::Call(expected.token) {
        return Err(StoredTransactionError::TokenMismatch);
    }
    if transaction.value != U256::ZERO {
        return Err(StoredTransactionError::NativeValue);
    }
    if transaction.gas_limit != expected.gas_limit {
        return Err(StoredTransactionError::GasLimitMismatch);
    }
    if transaction.max_fee_per_gas > expected.max_fee_per_gas {
        return Err(StoredTransactionError::FeeCapExceeded);
    }
    if transaction.max_fee_per_gas == 0
        || transaction.max_priority_fee_per_gas > transaction.max_fee_per_gas
    {
        return Err(StoredTransactionError::InvalidFeeEnvelope);
    }
    if !transaction.access_list.0.is_empty() {
        return Err(StoredTransactionError::UnexpectedAccessList);
    }
    if expected.domain.chain_id != Some(U256::from(expected.chain_id))
        || expected.domain.verifying_contract != Some(expected.token)
        || expected.domain.salt.is_some()
    {
        return Err(StoredTransactionError::DomainMismatch);
    }

    let (authorization, signature_kind, payer_signature) =
        decode_authorization_call(&transaction.input)?;
    let expected_authorization = Erc3009Authorization {
        from: expected.payer,
        to: expected.payee,
        value: expected.value,
        valid_after: expected.valid_after,
        valid_before: expected.valid_before,
        nonce: expected.authorization_nonce,
    };
    if authorization != expected_authorization {
        return Err(StoredTransactionError::AuthorizationMismatch);
    }
    let payment_hash = crate::settle::eip712_transfer_hash(&authorization, &expected.domain);
    if payment_hash != expected.payment_hash {
        return Err(StoredTransactionError::PaymentHashMismatch);
    }
    let classified = crate::settle::classify_settleable_signature(
        authorization.from,
        &payer_signature,
        &payment_hash,
    )
    .map_err(|_| StoredTransactionError::InvalidCalldata)?;
    let classified_kind = match classified {
        crate::settle::SettleableSignature::Eoa { .. } => Erc3009SignatureKind::Eoa,
        crate::settle::SettleableSignature::Eip1271(_) => Erc3009SignatureKind::Eip1271,
    };
    if classified_kind != signature_kind {
        return Err(StoredTransactionError::InvalidCalldata);
    }

    Ok(DecodedEvmSubmission {
        transaction_hash: expected.transaction_hash,
        facilitator_signer: recovered_signer,
        chain_id: transaction.chain_id,
        account_nonce: transaction.nonce,
        token: expected.token,
        gas_limit: transaction.gas_limit,
        max_fee_per_gas: transaction.max_fee_per_gas,
        max_priority_fee_per_gas: transaction.max_priority_fee_per_gas,
        authorization,
        payment_hash,
        signature_kind,
    })
}

fn decode_authorization_call(
    input: &[u8],
) -> Result<(Erc3009Authorization, Erc3009SignatureKind, Bytes), StoredTransactionError> {
    if input.starts_with(&abi::IErc3009::transferWithAuthorization_0Call::SELECTOR) {
        let call = abi::IErc3009::transferWithAuthorization_0Call::abi_decode_validate(input)
            .map_err(|_| StoredTransactionError::InvalidCalldata)?;
        if call.abi_encode() != input {
            return Err(StoredTransactionError::InvalidCalldata);
        }
        return Ok((
            Erc3009Authorization {
                from: call.from,
                to: call.to,
                value: call.value,
                valid_after: call.validAfter,
                valid_before: call.validBefore,
                nonce: call.nonce,
            },
            Erc3009SignatureKind::Eip1271,
            call.signature,
        ));
    }
    if input.starts_with(&abi::IErc3009::transferWithAuthorization_1Call::SELECTOR) {
        let call = abi::IErc3009::transferWithAuthorization_1Call::abi_decode_validate(input)
            .map_err(|_| StoredTransactionError::InvalidCalldata)?;
        if call.abi_encode() != input {
            return Err(StoredTransactionError::InvalidCalldata);
        }
        let mut payer_signature = Vec::with_capacity(65);
        payer_signature.extend_from_slice(call.r.as_slice());
        payer_signature.extend_from_slice(call.s.as_slice());
        payer_signature.push(call.v);
        return Ok((
            Erc3009Authorization {
                from: call.from,
                to: call.to,
                value: call.value,
                valid_after: call.validAfter,
                valid_before: call.validBefore,
                nonce: call.nonce,
            },
            Erc3009SignatureKind::Eoa,
            payer_signature.into(),
        ));
    }
    Err(StoredTransactionError::InvalidCalldata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settle::{build_transfer_domain, eip712_transfer_hash};
    use alloy_primitives::{address, b256, keccak256};
    use x402_chain_eip155::chain::EOASignatureExt;

    // DO NOT FUND. A throwaway, well-known test key (secp256k1 scalar = 1),
    // used only to make expired/impossible signing tests deterministic.
    fn test_signer() -> Result<PrivateKeySigner, Box<dyn std::error::Error>> {
        let key = b256!("0000000000000000000000000000000000000000000000000000000000000001");
        Ok(PrivateKeySigner::from_bytes(&key)?)
    }

    fn test_payer() -> Result<PrivateKeySigner, Box<dyn std::error::Error>> {
        // DO NOT FUND. Deterministic key used only with the expired authorization
        // returned by `sample_authorization`.
        let key = b256!("00000000000000000000000000000000000000000000000000000000000000a1");
        Ok(PrivateKeySigner::from_bytes(&key)?)
    }

    fn sample_authorization() -> Erc3009Authorization {
        Erc3009Authorization {
            from: address!("0x1111111111111111111111111111111111111111"),
            to: address!("0x2222222222222222222222222222222222222222"),
            value: U256::from(1_000_000_u64),
            valid_after: U256::ZERO,
            valid_before: U256::from(1_u64),
            nonce: b256!("00000000000000000000000000000000000000000000000000000000000000aa"),
        }
    }

    // Base Sepolia USDC, an arbitrary but realistic call target for the tests.
    const ASSET: Address = address!("0x036CbD53842c5426634e7929541eC2318f3dCF7e");

    #[test]
    fn bytes_calldata_carries_the_standard_erc3009_selector() {
        let calldata = transfer_with_authorization_bytes_calldata(
            &sample_authorization(),
            Bytes::from(vec![0xab_u8; 65]),
        );
        let expected = keccak256(
            "transferWithAuthorization(address,address,uint256,uint256,uint256,bytes32,bytes)",
        );
        assert_eq!(&calldata[..4], &expected[..4]);
    }

    #[test]
    fn vrs_calldata_carries_the_standard_erc3009_selector() {
        let calldata = transfer_with_authorization_vrs_calldata(
            &sample_authorization(),
            27,
            B256::repeat_byte(0x11),
            B256::repeat_byte(0x22),
        );
        let expected = keccak256(
            "transferWithAuthorization(address,address,uint256,uint256,uint256,bytes32,uint8,bytes32,bytes32)",
        );
        assert_eq!(&calldata[..4], &expected[..4]);
    }

    #[test]
    fn signed_transaction_is_deterministic_and_recovers_to_signer()
    -> Result<(), Box<dyn std::error::Error>> {
        let signer = test_signer()?;
        let head = EvmSignerHead {
            chain_id: 84_532,
            account_nonce: 7,
        };
        let fees = EvmFeeEnvelope {
            gas_limit: 120_000,
            max_fee_per_gas: 2_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
        };
        let calldata = transfer_with_authorization_vrs_calldata(
            &sample_authorization(),
            27,
            B256::repeat_byte(0x11),
            B256::repeat_byte(0x22),
        );

        let first = sign_settlement_transaction(&signer, head, fees, ASSET, calldata.clone())?;
        let second = sign_settlement_transaction(&signer, head, fees, ASSET, calldata)?;

        // Determinism: identical inputs yield byte-identical durable artifacts,
        // so a journaled submission and its recovery replay are the same tx.
        assert_eq!(first.signed_tx_rlp(), second.signed_tx_rlp());
        assert_eq!(first.tx_hash, second.tx_hash);
        assert_eq!(first.signer_address, signer.address());
        assert_eq!(first.account_nonce, 7);

        // The durable RLP decodes to a valid signed EIP-1559 transaction whose
        // hash matches what we journaled and whose signer recovers to us.
        let decoded = TxEnvelope::decode_2718(&mut first.signed_tx_rlp())?;
        assert_eq!(*decoded.tx_hash(), first.tx_hash);
        assert_eq!(decoded.recover_signer()?, signer.address());
        Ok(())
    }

    #[test]
    fn changing_the_nonce_changes_the_durable_hash() -> Result<(), Box<dyn std::error::Error>> {
        let signer = test_signer()?;
        let fees = EvmFeeEnvelope {
            gas_limit: 120_000,
            max_fee_per_gas: 2_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
        };
        let calldata = transfer_with_authorization_bytes_calldata(
            &sample_authorization(),
            Bytes::from(vec![0xcd_u8; 65]),
        );

        let head_a = EvmSignerHead {
            chain_id: 84_532,
            account_nonce: 1,
        };
        let head_b = EvmSignerHead {
            chain_id: 84_532,
            account_nonce: 2,
        };
        let a = sign_settlement_transaction(&signer, head_a, fees, ASSET, calldata.clone())?;
        let b = sign_settlement_transaction(&signer, head_b, fees, ASSET, calldata)?;
        assert_ne!(a.tx_hash, b.tx_hash);
        Ok(())
    }

    fn expected_submission(
        prepared: &EvmPrepared,
        signer: Address,
        authorization: Erc3009Authorization,
        fees: EvmFeeEnvelope,
        domain: Eip712Domain,
    ) -> ExpectedEvmSubmission {
        ExpectedEvmSubmission {
            transaction_hash: prepared.tx_hash,
            facilitator_signer: signer,
            chain_id: 84_532,
            account_nonce: prepared.account_nonce,
            token: ASSET,
            gas_limit: fees.gas_limit,
            max_fee_per_gas: fees.max_fee_per_gas,
            payer: authorization.from,
            payee: authorization.to,
            value: authorization.value,
            valid_after: authorization.valid_after,
            valid_before: authorization.valid_before,
            authorization_nonce: authorization.nonce,
            payment_hash: eip712_transfer_hash(&authorization, &domain),
            domain,
        }
    }

    fn sample_fees() -> EvmFeeEnvelope {
        EvmFeeEnvelope {
            gas_limit: 120_000,
            max_fee_per_gas: 2_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
        }
    }

    #[test]
    fn validator_accepts_both_supported_erc3009_overloads() -> Result<(), Box<dyn std::error::Error>>
    {
        let signer = test_signer()?;
        let head = EvmSignerHead {
            chain_id: 84_532,
            account_nonce: 3,
        };
        let fees = sample_fees();
        let authorization = sample_authorization();
        let domain = build_transfer_domain("USD Coin", "2", head.chain_id, ASSET);
        let bytes_calldata = transfer_with_authorization_bytes_calldata(
            &authorization,
            Bytes::from(vec![0xef_u8; 80]),
        );
        let bytes_prepared =
            sign_settlement_transaction(&signer, head, fees, ASSET, bytes_calldata)?;
        let bytes_expected = expected_submission(
            &bytes_prepared,
            signer.address(),
            authorization,
            fees,
            domain.clone(),
        );
        assert_eq!(
            validate_signed_transaction(bytes_prepared.signed_tx_rlp(), &bytes_expected)?
                .signature_kind,
            Erc3009SignatureKind::Eip1271
        );

        let payer = test_payer()?;
        let vrs_authorization = Erc3009Authorization {
            from: payer.address(),
            ..authorization
        };
        let vrs_hash = eip712_transfer_hash(&vrs_authorization, &domain);
        let payer_signature = payer.sign_hash_sync(&vrs_hash)?;
        let vrs_calldata = transfer_with_authorization_vrs_calldata(
            &vrs_authorization,
            payer_signature.v_legacy(),
            payer_signature.r_bytes(),
            payer_signature.s_bytes(),
        );
        let vrs_prepared = sign_settlement_transaction(&signer, head, fees, ASSET, vrs_calldata)?;
        let vrs_expected = expected_submission(
            &vrs_prepared,
            signer.address(),
            vrs_authorization,
            fees,
            domain,
        );
        assert_eq!(
            validate_signed_transaction(vrs_prepared.signed_tx_rlp(), &vrs_expected)?
                .signature_kind,
            Erc3009SignatureKind::Eoa
        );
        Ok(())
    }

    #[test]
    fn validator_rejects_cross_row_swap_and_wrong_identity_fields()
    -> Result<(), Box<dyn std::error::Error>> {
        let signer = test_signer()?;
        let head = EvmSignerHead {
            chain_id: 84_532,
            account_nonce: 9,
        };
        let fees = sample_fees();
        let authorization = sample_authorization();
        let domain = build_transfer_domain("USD Coin", "2", head.chain_id, ASSET);
        let calldata = transfer_with_authorization_bytes_calldata(
            &authorization,
            Bytes::from(vec![0xa5_u8; 96]),
        );
        let prepared = sign_settlement_transaction(&signer, head, fees, ASSET, calldata)?;
        let expected =
            expected_submission(&prepared, signer.address(), authorization, fees, domain);

        let mut wrong = expected.clone();
        wrong.account_nonce += 1;
        assert_eq!(
            validate_signed_transaction(prepared.signed_tx_rlp(), &wrong),
            Err(StoredTransactionError::AccountNonceMismatch)
        );
        wrong = expected.clone();
        wrong.chain_id = 8_453;
        assert_eq!(
            validate_signed_transaction(prepared.signed_tx_rlp(), &wrong),
            Err(StoredTransactionError::ChainIdMismatch)
        );
        wrong = expected.clone();
        wrong.token = address!("0x3333333333333333333333333333333333333333");
        assert_eq!(
            validate_signed_transaction(prepared.signed_tx_rlp(), &wrong),
            Err(StoredTransactionError::TokenMismatch)
        );
        wrong = expected.clone();
        wrong.payee = address!("0x4444444444444444444444444444444444444444");
        assert_eq!(
            validate_signed_transaction(prepared.signed_tx_rlp(), &wrong),
            Err(StoredTransactionError::AuthorizationMismatch)
        );
        wrong = expected.clone();
        wrong.authorization_nonce = B256::repeat_byte(0x99);
        assert_eq!(
            validate_signed_transaction(prepared.signed_tx_rlp(), &wrong),
            Err(StoredTransactionError::AuthorizationMismatch)
        );
        wrong = expected.clone();
        wrong.payment_hash = B256::ZERO;
        assert_eq!(
            validate_signed_transaction(prepared.signed_tx_rlp(), &wrong),
            Err(StoredTransactionError::PaymentHashMismatch)
        );
        Ok(())
    }

    #[test]
    fn validator_rejects_trailing_bytes_fee_cap_and_bad_calldata()
    -> Result<(), Box<dyn std::error::Error>> {
        let signer = test_signer()?;
        let head = EvmSignerHead {
            chain_id: 84_532,
            account_nonce: 11,
        };
        let fees = sample_fees();
        let authorization = sample_authorization();
        let domain = build_transfer_domain("USD Coin", "2", head.chain_id, ASSET);
        let calldata = transfer_with_authorization_bytes_calldata(
            &authorization,
            Bytes::from(vec![0x7b_u8; 72]),
        );
        let prepared = sign_settlement_transaction(&signer, head, fees, ASSET, calldata)?;
        let expected = expected_submission(
            &prepared,
            signer.address(),
            authorization,
            fees,
            domain.clone(),
        );

        let mut trailing = prepared.signed_tx_rlp().to_vec();
        trailing.push(0);
        assert_eq!(
            validate_signed_transaction(&trailing, &expected),
            Err(StoredTransactionError::TrailingBytes)
        );
        let mut tampered = prepared.signed_tx_rlp().to_vec();
        let Some(last) = tampered.last_mut() else {
            return Err("signed transaction unexpectedly empty".into());
        };
        *last ^= 0x01;
        assert!(validate_signed_transaction(&tampered, &expected).is_err());
        let mut low_cap = expected.clone();
        low_cap.max_fee_per_gas = fees.max_fee_per_gas - 1;
        assert_eq!(
            validate_signed_transaction(prepared.signed_tx_rlp(), &low_cap),
            Err(StoredTransactionError::FeeCapExceeded)
        );

        let malformed =
            sign_settlement_transaction(&signer, head, fees, ASSET, Bytes::from(vec![0; 64]))?;
        let malformed_expected =
            expected_submission(&malformed, signer.address(), authorization, fees, domain);
        assert_eq!(
            validate_signed_transaction(malformed.signed_tx_rlp(), &malformed_expected),
            Err(StoredTransactionError::InvalidCalldata)
        );
        Ok(())
    }

    #[test]
    fn debug_output_redacts_authorization_and_submission_sentinels()
    -> Result<(), Box<dyn std::error::Error>> {
        let facilitator = test_signer()?;
        let authorization = Erc3009Authorization {
            from: address!("0x1212121212121212121212121212121212121212"),
            to: address!("0x3434343434343434343434343434343434343434"),
            value: U256::from(987_654_321_012_345_678_u64),
            valid_after: U256::from(3_456_789_012_u64),
            valid_before: U256::from(4_567_890_123_u64),
            nonce: B256::repeat_byte(0x77),
        };
        let identity = EvmAuthorizationIdentity {
            nonce: authorization.nonce,
            valid_after: authorization.valid_after,
            valid_before: authorization.valid_before,
        };
        let head = EvmSignerHead {
            chain_id: 84_532,
            account_nonce: 6_543_219,
        };
        let fees = EvmFeeEnvelope {
            gas_limit: 121_337,
            max_fee_per_gas: 2_345_678_901,
            max_priority_fee_per_gas: 123_456_789,
        };
        let domain = build_transfer_domain("USD Coin", "2", head.chain_id, ASSET);
        let calldata = transfer_with_authorization_bytes_calldata(
            &authorization,
            Bytes::from(vec![0xa5_u8; 80]),
        );
        let prepared = sign_settlement_transaction(&facilitator, head, fees, ASSET, calldata)?;
        let expected = expected_submission(
            &prepared,
            facilitator.address(),
            authorization,
            fees,
            domain,
        );
        let decoded = validate_signed_transaction(prepared.signed_tx_rlp(), &expected)?;
        let outputs = [
            format!("{authorization:?}"),
            format!("{identity:?}"),
            format!("{prepared:?}"),
            format!("{expected:?}"),
            format!("{decoded:?}"),
        ];
        let sentinels = [
            authorization.from.to_string(),
            authorization.to.to_string(),
            authorization.value.to_string(),
            authorization.valid_after.to_string(),
            authorization.valid_before.to_string(),
            authorization.nonce.to_string(),
            prepared.tx_hash.to_string(),
            facilitator.address().to_string(),
            head.account_nonce.to_string(),
            expected.payment_hash.to_string(),
        ];
        for output in outputs {
            for sentinel in &sentinels {
                assert!(
                    !output.contains(sentinel),
                    "Debug output exposed a sensitive sentinel"
                );
            }
        }
        Ok(())
    }
}
