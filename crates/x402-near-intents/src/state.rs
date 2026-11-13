//! Pure quote/proof claim transitions for a future durable store.
//!
//! A proof claim alone is insufficient for a shared 1Click instrument: two
//! different transaction hashes could both appear in one aggregate status.
//! The store must claim the proof and instrument together. These transitions
//! define that invariant without selecting a database schema prematurely.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::wire::{ConsumptionKey, validate_caip2};

const INSTRUMENT_HASH_DOMAIN: &[u8] = b"x402-near-intents/instrument/v1\0";
const MAX_INSTRUMENT_PART_BYTES: usize = 256;

#[derive(Clone, Copy, Eq, Hash, PartialEq, Deserialize, Serialize)]
pub struct ProofId([u8; 32]);

impl ProofId {
    pub fn from_consumption_key(key: &ConsumptionKey) -> Self {
        Self(key.payment_hash())
    }
}

impl fmt::Debug for ProofId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProofId(<redacted>)")
    }
}

/// Domain-separated identity for `(network, depositAddress, depositMemo)`.
#[derive(Clone, Copy, Eq, Hash, PartialEq, Deserialize, Serialize)]
pub struct InstrumentId([u8; 32]);

impl InstrumentId {
    pub fn new(
        network: &str,
        deposit_address: &str,
        deposit_memo: Option<&str>,
    ) -> Result<Self, ClaimError> {
        validate_caip2(network).map_err(|_| ClaimError::InvalidInstrument)?;
        validate_instrument_part(deposit_address)?;
        if let Some(memo) = deposit_memo {
            validate_instrument_part(memo)?;
        }

        let mut hash = Sha256::new();
        hash.update(INSTRUMENT_HASH_DOMAIN);
        hash_len_prefixed(&mut hash, network.as_bytes());
        let canonical_address = canonical_instrument_address(network, deposit_address)?;
        hash_len_prefixed(&mut hash, canonical_address.as_bytes());
        match deposit_memo {
            Some(memo) => {
                hash.update([1]);
                hash_len_prefixed(&mut hash, memo.as_bytes());
            }
            None => hash.update([0]),
        }
        Ok(Self(hash.finalize().into()))
    }
}

impl fmt::Debug for InstrumentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InstrumentId(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Lease {
    pub generation: u64,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
pub enum ProofState {
    Unseen,
    Bound {
        instrument_id: InstrumentId,
        generation: u64,
        lease: Option<Lease>,
    },
    Consumed {
        instrument_id: InstrumentId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
pub enum InstrumentState {
    Open,
    Bound { proof_id: ProofId },
    Terminal { proof_id: ProofId },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Claim {
    proof_id: ProofId,
    instrument_id: InstrumentId,
    generation: u64,
}

impl Claim {
    pub const fn generation(self) -> u64 {
        self.generation
    }
}

/// Atomically model ownership of both the global proof and issued instrument.
///
/// The caller must apply the returned pair in one database transaction. On a
/// retry, only the proof already bound to this instrument may reacquire an
/// expired or released lease.
pub fn claim_pair(
    proof_state: ProofState,
    instrument_state: InstrumentState,
    proof_id: ProofId,
    instrument_id: InstrumentId,
    now: DateTime<Utc>,
    lease_expires_at: DateTime<Utc>,
) -> Result<(ProofState, InstrumentState, Claim), ClaimError> {
    if lease_expires_at <= now {
        return Err(ClaimError::InvalidLease);
    }

    let (generation, proof_was_unseen) = match proof_state {
        ProofState::Unseen => (1, true),
        ProofState::Bound {
            instrument_id: bound,
            generation,
            lease,
        } => {
            if bound != instrument_id {
                return Err(ClaimError::ProofAlreadyBound);
            }
            if lease.is_some_and(|lease| lease.generation != generation) {
                return Err(ClaimError::InconsistentState);
            }
            if lease.is_some_and(|lease| lease.expires_at > now) {
                return Err(ClaimError::ProofInFlight);
            }
            generation
                .checked_add(1)
                .map(|generation| (generation, false))
                .ok_or(ClaimError::GenerationExhausted)?
        }
        ProofState::Consumed { .. } => return Err(ClaimError::ProofConsumed),
    };

    match instrument_state {
        InstrumentState::Open if proof_was_unseen => {}
        InstrumentState::Open => return Err(ClaimError::InconsistentState),
        InstrumentState::Bound { proof_id: bound } if bound == proof_id && !proof_was_unseen => {}
        InstrumentState::Bound { proof_id: bound } if bound == proof_id => {
            return Err(ClaimError::InconsistentState);
        }
        InstrumentState::Bound { .. } => return Err(ClaimError::InstrumentAlreadyBound),
        InstrumentState::Terminal { .. } => return Err(ClaimError::InstrumentTerminal),
    }

    let lease = Lease {
        generation,
        expires_at: lease_expires_at,
    };
    let proof_state = ProofState::Bound {
        instrument_id,
        generation,
        lease: Some(lease),
    };
    let instrument_state = InstrumentState::Bound { proof_id };
    let claim = Claim {
        proof_id,
        instrument_id,
        generation,
    };
    Ok((proof_state, instrument_state, claim))
}

/// Release a nonterminal attempt while preserving the permanent pairing.
pub fn release_pair(
    proof_state: ProofState,
    instrument_state: InstrumentState,
    claim: Claim,
) -> Result<(ProofState, InstrumentState), ClaimError> {
    validate_active_claim(proof_state, instrument_state, claim)?;
    Ok((
        ProofState::Bound {
            instrument_id: claim.instrument_id,
            generation: claim.generation,
            lease: None,
        },
        instrument_state,
    ))
}

/// Consume the proof and instrument after a durable terminal result exists.
pub fn complete_pair(
    proof_state: ProofState,
    instrument_state: InstrumentState,
    claim: Claim,
) -> Result<(ProofState, InstrumentState), ClaimError> {
    validate_active_claim(proof_state, instrument_state, claim)?;
    Ok((
        ProofState::Consumed {
            instrument_id: claim.instrument_id,
        },
        InstrumentState::Terminal {
            proof_id: claim.proof_id,
        },
    ))
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum ClaimError {
    #[error("invalid near-intents instrument identity")]
    InvalidInstrument,
    #[error("claim lease must expire after it starts")]
    InvalidLease,
    #[error("payment proof is already in flight")]
    ProofInFlight,
    #[error("payment proof is already bound to another instrument")]
    ProofAlreadyBound,
    #[error("payment proof was already consumed")]
    ProofConsumed,
    #[error("payment instrument is already bound to another proof")]
    InstrumentAlreadyBound,
    #[error("payment instrument is terminal")]
    InstrumentTerminal,
    #[error("claim generation was exhausted")]
    GenerationExhausted,
    #[error("claim state is internally inconsistent")]
    InconsistentState,
    #[error("claim belongs to an earlier lease generation")]
    StaleClaim,
}

fn validate_active_claim(
    proof_state: ProofState,
    instrument_state: InstrumentState,
    claim: Claim,
) -> Result<(), ClaimError> {
    let proof_matches = matches!(
        proof_state,
        ProofState::Bound {
            instrument_id,
            generation,
            lease: Some(Lease {
                generation: lease_generation,
                ..
            }),
        } if instrument_id == claim.instrument_id
            && generation == claim.generation
            && lease_generation == claim.generation
    );
    let instrument_matches = matches!(
        instrument_state,
        InstrumentState::Bound { proof_id } if proof_id == claim.proof_id
    );
    if proof_matches && instrument_matches {
        Ok(())
    } else {
        Err(ClaimError::StaleClaim)
    }
}

fn validate_instrument_part(value: &str) -> Result<(), ClaimError> {
    if value.is_empty()
        || value.len() > MAX_INSTRUMENT_PART_BYTES
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(ClaimError::InvalidInstrument);
    }
    Ok(())
}

fn canonical_instrument_address(network: &str, address: &str) -> Result<String, ClaimError> {
    let namespace = network
        .split_once(':')
        .map(|(namespace, _)| namespace)
        .ok_or(ClaimError::InvalidInstrument)?;
    if namespace != "eip155" {
        return Ok(address.to_owned());
    }
    let digits = address
        .strip_prefix("0x")
        .ok_or(ClaimError::InvalidInstrument)?;
    if digits.len() != 40 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ClaimError::InvalidInstrument);
    }
    Ok(format!("0x{}", digits.to_ascii_lowercase()))
}

fn hash_len_prefixed(hash: &mut Sha256, value: &[u8]) {
    let length = u64::try_from(value.len()).unwrap_or(u64::MAX);
    hash.update(length.to_be_bytes());
    hash.update(value);
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn instant(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 4, 15, 0, second)
            .single()
            .unwrap_or_else(|| std::process::abort())
    }

    fn ids(seed: u8) -> (ProofId, InstrumentId) {
        let proof = ConsumptionKey::new(
            "eip155:42161",
            &format!("0x{}", format!("{seed:02x}").repeat(32)),
        )
        .unwrap_or_else(|_| std::process::abort());
        let instrument = InstrumentId::new(
            "eip155:42161",
            &format!("0x{}", format!("{seed:02x}").repeat(20)),
            None,
        )
        .unwrap_or_else(|_| std::process::abort());
        (ProofId::from_consumption_key(&proof), instrument)
    }

    #[test]
    fn first_claim_binds_both_proof_and_instrument() {
        let (proof_id, instrument_id) = ids(1);
        let claimed = claim_pair(
            ProofState::Unseen,
            InstrumentState::Open,
            proof_id,
            instrument_id,
            instant(0),
            instant(10),
        );
        assert!(claimed.is_ok());
        let Some((proof_state, instrument_state, _)) = claimed.ok() else {
            std::process::abort();
        };
        assert!(matches!(proof_state, ProofState::Bound { .. }));
        assert_eq!(instrument_state, InstrumentState::Bound { proof_id });
    }

    #[test]
    fn quote_cannot_be_claimed_by_a_second_proof() {
        let (first_proof, instrument_id) = ids(1);
        let (second_proof, _) = ids(2);
        assert_eq!(
            claim_pair(
                ProofState::Unseen,
                InstrumentState::Bound {
                    proof_id: first_proof
                },
                second_proof,
                instrument_id,
                instant(0),
                instant(10),
            ),
            Err(ClaimError::InstrumentAlreadyBound)
        );
    }

    #[test]
    fn nonterminal_release_allows_only_the_same_pair_to_retry() {
        let (proof_id, instrument_id) = ids(1);
        let (proof_state, instrument_state, first_claim) = claim_pair(
            ProofState::Unseen,
            InstrumentState::Open,
            proof_id,
            instrument_id,
            instant(0),
            instant(10),
        )
        .unwrap_or_else(|_| std::process::abort());
        let (proof_state, instrument_state) =
            release_pair(proof_state, instrument_state, first_claim)
                .unwrap_or_else(|_| std::process::abort());
        let retry = claim_pair(
            proof_state,
            instrument_state,
            proof_id,
            instrument_id,
            instant(1),
            instant(11),
        );
        assert!(retry.is_ok());
        assert_eq!(retry.ok().map(|(_, _, claim)| claim.generation()), Some(2));
    }

    #[test]
    fn expired_lease_is_reclaimable_and_fences_the_old_worker() {
        let (proof_id, instrument_id) = ids(1);
        let (proof_state, instrument_state, first_claim) = claim_pair(
            ProofState::Unseen,
            InstrumentState::Open,
            proof_id,
            instrument_id,
            instant(0),
            instant(5),
        )
        .unwrap_or_else(|_| std::process::abort());
        let (proof_state, instrument_state, second_claim) = claim_pair(
            proof_state,
            instrument_state,
            proof_id,
            instrument_id,
            instant(5),
            instant(10),
        )
        .unwrap_or_else(|_| std::process::abort());
        assert_eq!(second_claim.generation(), 2);
        assert_eq!(
            complete_pair(proof_state, instrument_state, first_claim),
            Err(ClaimError::StaleClaim)
        );
    }

    #[test]
    fn inconsistent_recovered_generations_are_rejected() {
        let (proof_id, instrument_id) = ids(1);
        let (_, instrument_state, claim) = claim_pair(
            ProofState::Unseen,
            InstrumentState::Open,
            proof_id,
            instrument_id,
            instant(0),
            instant(5),
        )
        .unwrap_or_else(|_| std::process::abort());
        let corrupted = ProofState::Bound {
            instrument_id,
            generation: claim.generation() + 1,
            lease: Some(Lease {
                generation: claim.generation(),
                expires_at: instant(5),
            }),
        };
        assert_eq!(
            complete_pair(corrupted, instrument_state, claim),
            Err(ClaimError::StaleClaim)
        );
        assert_eq!(
            claim_pair(
                corrupted,
                instrument_state,
                proof_id,
                instrument_id,
                instant(5),
                instant(10),
            ),
            Err(ClaimError::InconsistentState)
        );
    }

    #[test]
    fn one_proof_cannot_move_between_instruments() {
        let (proof_id, first_instrument) = ids(1);
        let (_, second_instrument) = ids(2);
        let proof_state = ProofState::Bound {
            instrument_id: first_instrument,
            generation: 1,
            lease: None,
        };
        assert_eq!(
            claim_pair(
                proof_state,
                InstrumentState::Open,
                proof_id,
                second_instrument,
                instant(0),
                instant(10),
            ),
            Err(ClaimError::ProofAlreadyBound)
        );
    }

    #[test]
    fn terminal_transition_consumes_both_sides() {
        let (proof_id, instrument_id) = ids(1);
        let (proof_state, instrument_state, claim) = claim_pair(
            ProofState::Unseen,
            InstrumentState::Open,
            proof_id,
            instrument_id,
            instant(0),
            instant(10),
        )
        .unwrap_or_else(|_| std::process::abort());
        let completed = complete_pair(proof_state, instrument_state, claim);
        assert_eq!(
            completed,
            Ok((
                ProofState::Consumed { instrument_id },
                InstrumentState::Terminal { proof_id }
            ))
        );
    }

    #[test]
    fn memo_is_part_of_the_instrument_identity() {
        let without_memo = InstrumentId::new("near:mainnet", "shared-address", None);
        let first = InstrumentId::new("near:mainnet", "shared-address", Some("memo-1"));
        let second = InstrumentId::new("near:mainnet", "shared-address", Some("memo-2"));
        assert!(without_memo.is_ok());
        assert!(first.is_ok());
        assert!(second.is_ok());
        assert_ne!(without_memo.as_ref().ok(), first.as_ref().ok());
        assert_ne!(first.as_ref().ok(), second.as_ref().ok());
    }

    #[test]
    fn evm_address_case_cannot_split_one_instrument_identity() {
        let lower = InstrumentId::new(
            "eip155:42161",
            "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd",
            None,
        );
        let upper = InstrumentId::new(
            "eip155:42161",
            "0xABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCD",
            None,
        );
        assert!(lower.is_ok());
        assert_eq!(lower, upper);
    }
}
