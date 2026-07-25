//! Mapping a verified EIP-3009 payment to the transaction the facilitator signs.
//!
//! This is the second half of the offline durable core: given the payer's
//! ERC-3009 authorization and their signature, produce the exact
//! `transferWithAuthorization` calldata to submit. The subtlety is that the
//! *overload* depends on the signature's shape, and it must match how upstream
//! `x402-chain-eip155` **verified** it — otherwise the facilitator would settle
//! a call it never simulated. So the classification is delegated wholesale to
//! upstream's [`StructuredSignature`]:
//!
//! - a plain EOA signature → the split `(v, r, s)` overload;
//! - a deployed smart-wallet (EIP-1271) signature → the opaque `bytes` overload;
//! - a counterfactual (EIP-6492) signature → **rejected**: settling it requires
//!   deploying the wallet in the same transaction (a Multicall3 aggregate), which
//!   the durable path does not yet build. This is a deliberate, documented v1
//!   boundary, not silent behavior.
//!
//! The EIP-712 transfer hash doubles as the payment's canonical identity (the
//! journal's idempotency anchor) and as the prehash the classifier needs.

use alloy_primitives::{Address, B256, Bytes};
use alloy_sol_types::{Eip712Domain, SolStruct, eip712_domain};
use x402_chain_eip155::chain::EOASignatureExt;
use x402_chain_eip155::v1_eip155_exact::{StructuredSignature, TransferWithAuthorization};

use crate::prepare::{
    Erc3009Authorization, transfer_with_authorization_bytes_calldata,
    transfer_with_authorization_vrs_calldata,
};

/// Build the EIP-712 domain for an ERC-3009 token, from the `name`/`version` the
/// requirements advertise plus the chain id and token address. For the V2 scheme
/// the `name`/`version` travel in the requirements, so this needs no RPC.
#[must_use]
pub fn build_transfer_domain(
    name: &str,
    version: &str,
    chain_id: u64,
    asset: Address,
) -> Eip712Domain {
    eip712_domain! {
        name: name.to_string(),
        version: version.to_string(),
        chain_id: chain_id,
        verifying_contract: asset,
    }
}

/// The EIP-712 signing hash of the authorization under `domain`.
///
/// This is exactly the digest the payer signed, so it is both the collision-
/// resistant identity of this payment (idempotency anchor) and the prehash used
/// to classify the signature in [`settlement_calldata`].
#[must_use]
pub fn eip712_transfer_hash(authorization: &Erc3009Authorization, domain: &Eip712Domain) -> B256 {
    let transfer = TransferWithAuthorization {
        from: authorization.from,
        to: authorization.to,
        value: authorization.value,
        validAfter: authorization.valid_after,
        validBefore: authorization.valid_before,
        nonce: authorization.nonce,
    };
    transfer.eip712_signing_hash(domain)
}

/// Why a payer signature cannot be turned into settlement calldata.
#[derive(Debug, thiserror::Error)]
pub enum UnsupportedSignature {
    /// A counterfactual EIP-6492 signature: settling it needs an in-transaction
    /// wallet deployment the durable path does not yet build.
    #[error("counterfactual (EIP-6492) smart-wallet signatures are not yet settleable")]
    CounterfactualWallet,
    /// The signature bytes could not be classified (e.g. a malformed EIP-6492
    /// wrapper).
    #[error("the payer signature could not be parsed")]
    Malformed,
}

/// Select the ERC-3009 overload matching the payer signature's shape and return
/// the encoded `transferWithAuthorization` calldata.
///
/// Classification is upstream's [`StructuredSignature`], so a payment that
/// verified is settled through the *same* interpretation of its signature.
///
/// # Errors
///
/// Returns [`UnsupportedSignature::CounterfactualWallet`] for an EIP-6492
/// counterfactual signature, or [`UnsupportedSignature::Malformed`] if the bytes
/// cannot be classified.
pub fn settlement_calldata(
    authorization: &Erc3009Authorization,
    signature: &Bytes,
    eip712_prehash: &B256,
) -> Result<Bytes, UnsupportedSignature> {
    let structured =
        StructuredSignature::try_from_bytes(signature.clone(), authorization.from, eip712_prehash)
            .map_err(|_| UnsupportedSignature::Malformed)?;
    match structured {
        StructuredSignature::EOA(eoa) => Ok(transfer_with_authorization_vrs_calldata(
            authorization,
            eoa.v_legacy(),
            eoa.r_bytes(),
            eoa.s_bytes(),
        )),
        StructuredSignature::EIP1271(bytes) => Ok(transfer_with_authorization_bytes_calldata(
            authorization,
            bytes,
        )),
        StructuredSignature::EIP6492 { .. } => Err(UnsupportedSignature::CounterfactualWallet),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{U256, address, b256, hex, keccak256};
    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;

    // Base Sepolia USDC — a realistic verifying contract for the domain.
    const ASSET: Address = address!("0x036CbD53842c5426634e7929541eC2318f3dCF7e");
    const VRS_SELECTOR_SIG: &str = "transferWithAuthorization(address,address,uint256,uint256,uint256,bytes32,uint8,bytes32,bytes32)";
    const BYTES_SELECTOR_SIG: &str =
        "transferWithAuthorization(address,address,uint256,uint256,uint256,bytes32,bytes)";

    // A throwaway test key standing in for a payer wallet. NOT a real credential.
    fn payer() -> Result<PrivateKeySigner, Box<dyn std::error::Error>> {
        let key = b256!("00000000000000000000000000000000000000000000000000000000000000a1");
        Ok(PrivateKeySigner::from_bytes(&key)?)
    }

    fn authorization(from: Address) -> Erc3009Authorization {
        Erc3009Authorization {
            from,
            to: address!("0x2222222222222222222222222222222222222222"),
            value: U256::from(1_000_000_u64),
            valid_after: U256::ZERO,
            valid_before: U256::from(4_000_000_000_u64),
            nonce: b256!("00000000000000000000000000000000000000000000000000000000000000aa"),
        }
    }

    #[test]
    fn eip712_transfer_hash_is_deterministic() {
        let auth = authorization(address!("0x1111111111111111111111111111111111111111"));
        let domain = build_transfer_domain("USD Coin", "2", 84_532, ASSET);
        let a = eip712_transfer_hash(&auth, &domain);
        let b = eip712_transfer_hash(&auth, &domain);
        assert_eq!(a, b);
        // A different chain id yields a different digest (domain separation).
        let other = build_transfer_domain("USD Coin", "2", 8_453, ASSET);
        assert_ne!(eip712_transfer_hash(&auth, &other), a);
    }

    #[test]
    fn eoa_signature_dispatches_to_the_vrs_overload() -> Result<(), Box<dyn std::error::Error>> {
        let payer = payer()?;
        let auth = authorization(payer.address());
        let domain = build_transfer_domain("USD Coin", "2", 84_532, ASSET);
        let prehash = eip712_transfer_hash(&auth, &domain);
        // The payer signs the EIP-712 digest as an EOA would.
        let signature = payer.sign_hash_sync(&prehash)?;
        let signature_bytes = Bytes::from(signature.as_bytes().to_vec());

        let calldata = settlement_calldata(&auth, &signature_bytes, &prehash)?;
        let selector = keccak256(VRS_SELECTOR_SIG);
        assert_eq!(&calldata[..4], &selector[..4]);
        Ok(())
    }

    #[test]
    fn opaque_contract_signature_dispatches_to_the_bytes_overload()
    -> Result<(), Box<dyn std::error::Error>> {
        let auth = authorization(address!("0x1111111111111111111111111111111111111111"));
        let domain = build_transfer_domain("USD Coin", "2", 84_532, ASSET);
        let prehash = eip712_transfer_hash(&auth, &domain);
        // A contract-wallet blob: not a 64/65-byte EOA signature and no EIP-6492
        // magic suffix, so it is classified EIP-1271 and passed through verbatim.
        let signature_bytes = Bytes::from(vec![0x5a_u8; 100]);
        let calldata = settlement_calldata(&auth, &signature_bytes, &prehash)?;
        let selector = keccak256(BYTES_SELECTOR_SIG);
        assert_eq!(&calldata[..4], &selector[..4]);
        Ok(())
    }

    #[test]
    fn counterfactual_shaped_signature_is_rejected_not_settled() {
        let auth = authorization(address!("0x1111111111111111111111111111111111111111"));
        let prehash = b256!("00000000000000000000000000000000000000000000000000000000000000bb");
        // EIP-6492 magic suffix with an undecodable prefix: it must be refused,
        // never quietly reinterpreted as a plain signature.
        let mut blob = vec![0x00_u8; 40];
        blob.extend_from_slice(&hex!(
            "6492649264926492649264926492649264926492649264926492649264926492"
        ));
        let signature_bytes = Bytes::from(blob);
        assert!(settlement_calldata(&auth, &signature_bytes, &prehash).is_err());
    }
}
