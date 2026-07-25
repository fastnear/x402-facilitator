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
//!    `bytes`-signature form for EIP-1271 / EIP-6492 smart-wallet signatures,
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
use alloy_sol_types::SolCall;
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

/// Encode `transferWithAuthorization` with an opaque signature blob — the
/// ERC-3009 `bytes` overload used for EIP-1271 / EIP-6492 smart-wallet
/// signatures.
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
}

impl EvmPrepared {
    /// The signed, EIP-2718-encoded transaction bytes to broadcast (and to
    /// rebroadcast verbatim during recovery).
    #[must_use]
    pub fn signed_tx_rlp(&self) -> &[u8] {
        &self.signed_tx_rlp
    }
}

impl fmt::Debug for EvmPrepared {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EvmPrepared")
            .field("tx_hash", &self.tx_hash)
            .field("signer_address", &self.signer_address)
            .field("account_nonce", &self.account_nonce)
            .field("signed_tx_rlp", &"<redacted>")
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
    })
}

/// Validate journaled EVM transaction bytes during recovery: they must decode to
/// a signed transaction whose hash and recovered signer match the journal, before
/// any RPC result for this settlement is trusted. Offline and deterministic — the
/// EVM analog of NEAR's stored-transaction Borsh + signature validation.
///
/// # Errors
///
/// Returns a describing message if the bytes are malformed, the transaction hash
/// does not match `expected_tx_hash`, or the recovered signer does not match
/// `expected_signer` (both compared case-insensitively as `0x` hex).
pub fn validate_signed_transaction(
    signed_tx_rlp: &[u8],
    expected_tx_hash: &str,
    expected_signer: &str,
) -> Result<(), String> {
    let envelope = TxEnvelope::decode_2718(&mut &signed_tx_rlp[..])
        .map_err(|error| format!("evm transaction bytes are invalid: {error}"))?;
    if !envelope
        .tx_hash()
        .to_string()
        .eq_ignore_ascii_case(expected_tx_hash)
    {
        return Err("evm transaction bytes do not match journaled hash".to_owned());
    }
    let signer = envelope
        .recover_signer()
        .map_err(|error| format!("evm signer recovery failed: {error}"))?;
    if !signer.to_string().eq_ignore_ascii_case(expected_signer) {
        return Err("evm transaction signer does not match journaled signer".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256, keccak256};

    // A throwaway, well-known test private key (secp256k1 scalar = 1). NOT a real
    // credential — used only to make the signing golden tests deterministic.
    fn test_signer() -> Result<PrivateKeySigner, Box<dyn std::error::Error>> {
        let key = b256!("0000000000000000000000000000000000000000000000000000000000000001");
        Ok(PrivateKeySigner::from_bytes(&key)?)
    }

    fn sample_authorization() -> Erc3009Authorization {
        Erc3009Authorization {
            from: address!("0x1111111111111111111111111111111111111111"),
            to: address!("0x2222222222222222222222222222222222222222"),
            value: U256::from(1_000_000_u64),
            valid_after: U256::ZERO,
            valid_before: U256::from(4_000_000_000_u64),
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

    #[test]
    fn validate_accepts_our_signed_tx_and_rejects_tampering()
    -> Result<(), Box<dyn std::error::Error>> {
        let signer = test_signer()?;
        let head = EvmSignerHead {
            chain_id: 84_532,
            account_nonce: 3,
        };
        let fees = EvmFeeEnvelope {
            gas_limit: 120_000,
            max_fee_per_gas: 2_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
        };
        let calldata = transfer_with_authorization_bytes_calldata(
            &sample_authorization(),
            Bytes::from(vec![0xef_u8; 65]),
        );
        let prepared = sign_settlement_transaction(&signer, head, fees, ASSET, calldata)?;
        let hash = prepared.tx_hash.to_string();
        let address = signer.address().to_string();

        // Accepts the exact journaled bytes.
        assert!(validate_signed_transaction(prepared.signed_tx_rlp(), &hash, &address).is_ok());
        // Rejects a mismatched hash and a mismatched signer.
        assert!(
            validate_signed_transaction(
                prepared.signed_tx_rlp(),
                &B256::ZERO.to_string(),
                &address
            )
            .is_err()
        );
        assert!(
            validate_signed_transaction(
                prepared.signed_tx_rlp(),
                &hash,
                "0x1111111111111111111111111111111111111111",
            )
            .is_err()
        );
        // Rejects tampered bytes.
        let mut tampered = prepared.signed_tx_rlp().to_vec();
        if let Some(byte) = tampered.get_mut(12) {
            *byte ^= 0xff;
        }
        assert!(validate_signed_transaction(&tampered, &hash, &address).is_err());
        Ok(())
    }
}
