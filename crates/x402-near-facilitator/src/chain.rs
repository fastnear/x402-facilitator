//! Chain-neutral settlement vocabulary shared by the settlement engine.
//!
//! The engine in [`crate::service`] speaks neutral value types while the closed
//! [`ChainProvider`] enum dispatches to the audited NEAR and EVM providers.
//! Provider-specific verified and prepared values remain typed inside enum
//! variants so recovery can validate them without weakening the shared model.

use std::fmt;
use std::future::Future;

use near_primitives::hash::CryptoHash;
use near_primitives::types::AccountId;
use near_primitives::views::FinalExecutionOutcomeView;
use x402_chain_eip155_provider::prepare::{
    EvmPrepared, ExpectedEvmSubmission, StoredTransactionError, validate_signed_transaction,
};
use x402_chain_eip155_provider::provider::{
    EvmChainProvider, EvmHead, EvmReconcileStatus, EvmTerminalOutcome, EvmVerifiedPayment,
    EvmVerifyRejection,
};
use x402_chain_near::{
    NearChainProvider, NearRpcError, PreparedTransaction as NearPrepared, RelayerHead,
    TransactionLookup, VerificationFailure as NearVerificationFailure, VerificationPolicy,
    VerifiedPayment as NearVerified, interpret_final_outcome, validate_final_outcome_identity,
};
use x402_types::chain::ChainProviderOps as _;
use x402_types::proto;

/// The settlement provider for the environment's chain. A closed enum (rather
/// than `dyn`) so the engine can hold one `Arc<ChainProvider>` and dispatch
/// inward with neutral value types.
pub enum ChainProvider {
    /// A NEAR delegate-settlement provider.
    Near(NearChainProvider),
    /// An EVM (eip155) ERC-3009 settlement provider. Boxed: it wraps the alloy
    /// provider stack and is much larger than the NEAR variant.
    Evm(Box<EvmChainProvider>),
}

impl fmt::Debug for ChainProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Near(_) => formatter.write_str("Near(<redacted>)"),
            Self::Evm(_) => formatter.write_str("Evm(<redacted>)"),
        }
    }
}

impl ChainProvider {
    /// Borrow the inner NEAR provider. The settlement engine speaks only neutral
    /// [`ChainProvider`] methods; this accessor exists for tests that drive the
    /// concrete `NearChainProvider` directly to stage journal fixtures.
    #[cfg(test)]
    #[must_use]
    pub fn as_near(&self) -> &NearChainProvider {
        match self {
            Self::Near(provider) => provider,
            // Test-only accessor for staging NEAR journal fixtures; the EVM tests
            // never reach it. Abort (not `panic!`) honors the crate's no-panic lint.
            Self::Evm(_) => std::process::abort(),
        }
    }

    /// The facilitator signer/relayer account identity (NEAR account id / EVM
    /// `0x` address).
    #[must_use]
    pub fn signer_account_id(&self) -> String {
        match self {
            Self::Near(provider) => provider.relayer_account_id().to_string(),
            Self::Evm(provider) => provider.signer_address().to_string(),
        }
    }

    /// The facilitator signer/relayer public key (NEAR ed25519 string; EVM
    /// address, or empty).
    #[must_use]
    pub fn signer_public_key(&self) -> String {
        match self {
            Self::Near(provider) => provider.relayer_public_key().to_string(),
            // EVM has no distinct public key; the address doubles as the signer
            // identity so the store's relayer-policy keys stay consistent.
            Self::Evm(provider) => provider.signer_address().to_string(),
        }
    }

    /// The confirmation depth an EVM settlement must reach before it is trusted
    /// as terminal. `None` for NEAR, whose finality is single-step (fast-final
    /// receipt), not confirmation-depth based. Journaled for EVM rows at prepare.
    #[must_use]
    pub fn required_confirmations(&self) -> Option<u64> {
        match self {
            Self::Near(_) => None,
            Self::Evm(provider) => Some(provider.required_confirmations()),
        }
    }

    /// Convert a provider-specific prepared value into the chain-neutral
    /// submission journal contract. The chain-specific value remains alongside
    /// it only for the immediate broadcast call.
    #[must_use]
    pub fn durable_submission(&self, prepared: &Prepared) -> DurableSubmission {
        let recovery_policy = match self {
            Self::Near(_) => RecoveryPolicy::NearFinality,
            Self::Evm(provider) => {
                RecoveryPolicy::EvmConfirmations(provider.required_confirmations())
            }
        };
        DurableSubmission {
            submitter: prepared.signer_id.clone(),
            nonce: prepared.signer_nonce,
            bytes: prepared.submit_bytes.clone(),
            hash: prepared.submit_hash.clone(),
            recovery_policy,
        }
    }

    /// Validate a stored EVM submission against its exact journal row before
    /// receipt evidence is considered. The provider supplies immutable chain,
    /// signer, token, gas, and EIP-712 domain facts; the binding supplies the
    /// settlement-specific values and historical fee policy.
    ///
    /// # Errors
    ///
    /// Returns a typed corruption reason for malformed fields, a provider/row
    /// mismatch, or any invalid signed envelope/calldata.
    pub fn validate_stored_submission(
        &self,
        bytes: &[u8],
        binding: &StoredEvmSubmission,
    ) -> Result<(), StoredSubmissionError> {
        let Self::Evm(provider) = self else {
            return Err(StoredSubmissionError::WrongProvider);
        };
        if binding.network != provider.caip2().to_string() {
            return Err(StoredSubmissionError::Field("network"));
        }
        let transaction_hash = binding
            .hash
            .parse()
            .map_err(|_| StoredSubmissionError::Field("transaction hash"))?;
        let facilitator_signer = binding
            .submitter
            .parse()
            .map_err(|_| StoredSubmissionError::Field("submitter"))?;
        if facilitator_signer != provider.signer_address() {
            return Err(StoredSubmissionError::Field("submitter"));
        }
        let account_nonce = u64::try_from(binding.nonce)
            .map_err(|_| StoredSubmissionError::Field("account nonce"))?;
        let token = binding
            .asset
            .parse()
            .map_err(|_| StoredSubmissionError::Field("asset"))?;
        if token != provider.asset() {
            return Err(StoredSubmissionError::Field("asset"));
        }
        let payer = binding
            .payer
            .parse()
            .map_err(|_| StoredSubmissionError::Field("payer"))?;
        let recipient = binding
            .payee
            .parse()
            .map_err(|_| StoredSubmissionError::Field("payee"))?;
        let value = binding
            .amount
            .parse()
            .map_err(|_| StoredSubmissionError::Field("amount"))?;
        let valid_after = binding
            .valid_after
            .parse()
            .map_err(|_| StoredSubmissionError::Field("validAfter"))?;
        let valid_before = binding
            .valid_before
            .parse()
            .map_err(|_| StoredSubmissionError::Field("validBefore"))?;
        let expected_scope = format!(
            "{}:{}:{}",
            binding.network,
            binding.asset.to_ascii_lowercase(),
            binding.payer.to_ascii_lowercase()
        );
        if binding.anchor_scope != expected_scope {
            return Err(StoredSubmissionError::Field("anchor scope"));
        }
        let domain = provider.transfer_domain();
        validate_signed_transaction(
            bytes,
            &ExpectedEvmSubmission {
                transaction_hash,
                facilitator_signer,
                chain_id: provider.chain_id(),
                account_nonce,
                token,
                gas_limit: binding.gas_limit,
                max_fee_per_gas: binding.max_fee_per_gas,
                payer,
                payee: recipient,
                value,
                valid_after,
                valid_before,
                authorization_nonce: binding.anchor_value.into(),
                payment_hash: binding.payment_hash.into(),
                domain,
            },
        )
        .map(|_| ())
        .map_err(StoredSubmissionError::Transaction)
    }

    /// The eip155 pre-verify payment identity: the offline ERC-3009 EIP-712
    /// transfer hash used by the settle path for idempotency before the
    /// authoritative on-chain verify. NEAR derives its equivalent hash by
    /// decoding the signed delegate at the service layer, so this is eip155-only
    /// and returns `not_eip155` for a NEAR provider (the settle path never calls
    /// it there).
    ///
    /// # Errors
    ///
    /// Returns the rejection reason string if the request is not a well-formed
    /// eip155 payment, or `not_eip155` for a NEAR provider.
    pub fn offline_payment_hash(&self, request: &proto::VerifyRequest) -> Result<[u8; 32], String> {
        match self {
            Self::Near(_) => Err("not_eip155".to_owned()),
            Self::Evm(provider) => provider
                .offline_payment_hash(request)
                .map_err(|rejection| rejection.reason),
        }
    }

    /// Probe that both configured RPC endpoints report the expected chain and a
    /// final block. This is the chain-liveness half of readiness.
    pub async fn readiness_probe(&self) -> bool {
        match self {
            Self::Near(provider) => {
                let expected = provider.chain_id().reference;
                matches!(provider.rpc_network_id().await, Ok(network) if network == expected)
                    && matches!(
                        provider.backup_rpc_network_id().await,
                        Ok(network) if network == expected
                    )
                    && provider.rpc_final_block().await.is_ok()
                    && provider.backup_rpc_final_block().await.is_ok()
            }
            Self::Evm(provider) => provider.readiness_probe().await,
        }
    }

    /// Observe the chain-dependent readiness inputs.
    ///
    /// NEAR intentionally retains its independent two-endpoint liveness probe
    /// and relayer-status read. EVM obtains one conservative primary/backup
    /// [`EvmHead`] and derives both the RPC and signer gates from that same
    /// snapshot, avoiding a second burst of identical RPC calls per refresh.
    pub async fn readiness_observation(&self) -> ChainReadinessObservation {
        match self {
            Self::Near(_) => ChainReadinessObservation {
                rpc_ready: self.readiness_probe().await,
                signer_head: self.signer_head().await,
            },
            Self::Evm(provider) => observe_evm_readiness(|| provider.head()).await,
        }
    }

    /// A fresh snapshot of the signer and chain head, used to gate readiness and
    /// prepare a submission. For NEAR this also enforces that the relayer key is
    /// full-access (the underlying `relayer_status` errors otherwise).
    pub async fn signer_head(&self) -> Result<SignerHead, SignerHeadError> {
        match self {
            Self::Near(provider) => {
                let status = provider.relayer_status().await?;
                Ok(SignerHead {
                    chain_block_height: status.block_height,
                    chain_block_ref: status.block_hash.to_string(),
                    signer_nonce: u128::from(status.access_key_nonce),
                    signer_id: provider.relayer_account_id().to_string(),
                    signer_public_key: provider.relayer_public_key().to_string(),
                    signer_balance_atomic: status.account.amount.as_yoctonear(),
                })
            }
            Self::Evm(provider) => {
                let head = provider
                    .head()
                    .await
                    .map_err(|error| SignerHeadError(error.to_string()))?;
                Ok(evm_head_to_signer_head(&head))
            }
        }
    }

    /// Verify a raw payment against policy, returning a neutral verified payment
    /// or a neutral [`VerifyRejection`] carrying the reason and its
    /// RPC-ambiguity flag (without exposing the per-chain failure enum).
    pub async fn verify(
        &self,
        request: &proto::VerifyRequest,
        policy: &VerificationPolicy,
    ) -> Result<VerifiedPayment, VerifyRejection> {
        match self {
            Self::Near(provider) => {
                // Same bounded retry the EVM provider applies internally:
                // ambiguous RPC lookups (throttling, transient failures) get
                // two short retries; definitive rejections return immediately.
                let near = crate::retry::retry_while_transient(
                    || provider.verify(request, policy),
                    |outcome| {
                        matches!(outcome, Err(failure) if near_verification_is_rpc_ambiguous(*failure))
                    },
                )
                .await
                .map_err(VerifyRejection::from_near)?;
                Ok(VerifiedPayment {
                    payer: near.payer.to_string(),
                    payment_hash: *near.payment_hash(),
                    requirements: Requirements {
                        network: near.requirements.network.as_str().to_owned(),
                        asset: near.requirements.asset.to_string(),
                        pay_to: near.requirements.pay_to.to_string(),
                        amount: near.requirements.amount,
                        amount_decimal: near.requirements.amount_decimal.clone(),
                    },
                    detail: VerifiedDetail::Near(near),
                })
            }
            Self::Evm(provider) => {
                // The EVM scheme carries its own limits in the signed ERC-3009
                // authorization; the NEAR gas policy does not apply.
                let _ = policy;
                let evm = provider
                    .verify(request)
                    .await
                    .map_err(VerifyRejection::from_evm)?;
                Ok(VerifiedPayment {
                    payer: evm.payer.to_string(),
                    payment_hash: evm.payment_hash.0,
                    requirements: Requirements {
                        network: provider.caip2().to_string(),
                        asset: evm.asset.to_string(),
                        pay_to: evm.pay_to.to_string(),
                        amount: u128::try_from(evm.amount).unwrap_or(u128::MAX),
                        amount_decimal: evm.amount.to_string(),
                    },
                    detail: VerifiedDetail::Evm(evm),
                })
            }
        }
    }

    /// Build and sign a submission from a verified payment and a signer-head
    /// snapshot. The returned [`Prepared`] is durable: recovery rebroadcasts its
    /// exact bytes and must never re-sign.
    ///
    /// `async` because a chain may need the network to price a submission (EVM
    /// reads the fee market); the NEAR path signs offline and never awaits.
    pub async fn prepare(
        &self,
        payment: &VerifiedPayment,
        head: &SignerHead,
    ) -> Result<Prepared, PrepareError> {
        match (self, &payment.detail) {
            (Self::Near(provider), VerifiedDetail::Near(near_payment)) => {
                // The neutral head carries the block reference as a string; NEAR
                // round-trips it back to a `CryptoHash` (base58, lossless). A
                // parse failure is impossible for a well-formed head and is
                // treated as a safe preparation failure (no broadcast).
                let block_hash = head
                    .chain_block_ref
                    .parse::<CryptoHash>()
                    .map_err(|_| PrepareError::InvalidSignerHead)?;
                let access_key_nonce = u64::try_from(head.signer_nonce)
                    .map_err(|_| PrepareError::InvalidSignerHead)?;
                let relayer_head = RelayerHead {
                    block_height: head.chain_block_height,
                    block_hash,
                    access_key_nonce,
                };
                let prepared = provider
                    .prepare_outer_transaction(near_payment, relayer_head)
                    .map_err(|error| PrepareError::Provider(error.to_string()))?;
                Ok(Prepared {
                    submit_bytes: prepared.signed_transaction_bytes().to_vec(),
                    submit_hash: prepared.transaction_hash.to_string(),
                    signer_id: prepared.signer_id.to_string(),
                    signer_public_key: prepared.signer_public_key.to_string(),
                    signer_nonce: u128::from(prepared.relayer_nonce),
                    detail: PreparedDetail::Near(prepared),
                })
            }
            (Self::Evm(provider), VerifiedDetail::Evm(evm_payment)) => {
                let account_nonce = u64::try_from(head.signer_nonce)
                    .map_err(|_| PrepareError::InvalidSignerHead)?;
                let prepared = provider
                    .prepare(evm_payment, account_nonce)
                    .await
                    .map_err(|error| PrepareError::Provider(error.to_string()))?;
                Ok(Prepared {
                    submit_bytes: prepared.signed_tx_rlp().to_vec(),
                    submit_hash: prepared.tx_hash.to_string(),
                    signer_id: prepared.signer_address.to_string(),
                    signer_public_key: prepared.signer_address.to_string(),
                    signer_nonce: u128::from(prepared.account_nonce),
                    detail: PreparedDetail::Evm(prepared),
                })
            }
            // A provider/detail chain mismatch is an impossible invariant
            // violation (verify pairs them); refuse rather than cross chains.
            _ => Err(PrepareError::InvalidSignerHead),
        }
    }

    /// Broadcast a prepared submission and classify the outcome. NEAR resolves
    /// to [`BroadcastOutcome::Terminal`] on fast finality (after receipt-graph
    /// validation), [`BroadcastOutcome::Rejected`] on deterministic rejection,
    /// or [`BroadcastOutcome::Pending`] when the outcome is indeterminate and
    /// must be resolved by reconciliation. (EVM will always return `Pending`
    /// until its confirmation-depth policy is met.)
    pub async fn broadcast(
        &self,
        prepared: &Prepared,
        payment: &VerifiedPayment,
    ) -> BroadcastOutcome {
        match (self, &prepared.detail, &payment.detail) {
            (
                Self::Near(provider),
                PreparedDetail::Near(near_prepared),
                VerifiedDetail::Near(near_payment),
            ) => {
                match provider
                    .broadcast_exact(near_prepared.signed_transaction_bytes())
                    .await
                {
                    Ok(TransactionLookup::Final(outcome)) => {
                        match interpret_near_final(
                            &outcome,
                            near_prepared.transaction_hash,
                            &provider.relayer_account_id(),
                            &near_payment.payer,
                            &near_payment.requirements.asset,
                        ) {
                            NearInterpretation::Terminal(terminal) => {
                                BroadcastOutcome::Terminal(terminal)
                            }
                            NearInterpretation::Indeterminate(_) => BroadcastOutcome::Pending,
                        }
                    }
                    Err(NearRpcError::TransactionRejected) => {
                        BroadcastOutcome::Rejected("transaction_rejected".to_owned())
                    }
                    Ok(TransactionLookup::Pending(_) | TransactionLookup::Unknown) | Err(_) => {
                        BroadcastOutcome::Pending
                    }
                }
            }
            (Self::Evm(provider), PreparedDetail::Evm(evm_prepared), _) => {
                // An EVM outcome is never trusted at submission: submit the raw
                // bytes and always report Pending. A send error is recoverable —
                // reconciliation rebroadcasts the same journaled bytes.
                let _ = provider.broadcast_raw(evm_prepared.signed_tx_rlp()).await;
                BroadcastOutcome::Pending
            }
            // Provider/detail chain mismatch (impossible); stay pending so the
            // durable bytes are never lost.
            _ => BroadcastOutcome::Pending,
        }
    }

    /// Reconcile a submitted transaction against both configured RPCs, returning
    /// a neutral verdict. The NEAR impl compares the two *raw* final outcomes for
    /// integrity (honest RPCs must agree byte-for-byte on a finalized
    /// transaction) and then validates identity + receipt graph, so the engine
    /// never sees NEAR primitives. `rpc_failover` reports that the backup RPC
    /// supplied a final outcome the primary did not. (EVM's variant applies the
    /// confirmation-depth policy instead of raw-outcome equality.)
    pub async fn reconcile_status(
        &self,
        submit_hash: &str,
        signer: &str,
        payer: &str,
        asset: &str,
        recovery_policy: RecoveryPolicy,
    ) -> ReconcileStatus {
        match self {
            Self::Near(provider) => {
                if recovery_policy != RecoveryPolicy::NearFinality {
                    return ReconcileStatus::verdict(ReconcileVerdict::Ambiguous);
                }
                let (Ok(hash), Ok(signer_id), Ok(payer_id), Ok(asset_id)) = (
                    submit_hash.parse::<CryptoHash>(),
                    signer.parse::<AccountId>(),
                    payer.parse::<AccountId>(),
                    asset.parse::<AccountId>(),
                ) else {
                    return ReconcileStatus::verdict(ReconcileVerdict::Ambiguous);
                };
                let primary = provider.query_transaction(hash, signer_id.clone()).await;
                let backup = provider.query_transaction_backup(hash, signer_id).await;
                let primary_final = near_final_outcome(&primary);
                let backup_final = near_final_outcome(&backup);
                if final_outcomes_conflict(primary_final, backup_final) {
                    return ReconcileStatus::verdict(ReconcileVerdict::Conflict);
                }
                let rpc_failover = primary_final.is_none() && backup_final.is_some();
                if let Some(outcome) = primary_final.or(backup_final) {
                    let verdict = match interpret_near_final(
                        outcome,
                        hash,
                        &provider.relayer_account_id(),
                        &payer_id,
                        &asset_id,
                    ) {
                        NearInterpretation::Terminal(terminal) => {
                            ReconcileVerdict::Terminal(terminal)
                        }
                        NearInterpretation::Indeterminate(reason) => {
                            ReconcileVerdict::Indeterminate(reason)
                        }
                    };
                    return ReconcileStatus {
                        verdict,
                        rpc_failover,
                    };
                }
                if [primary.as_ref(), backup.as_ref()]
                    .into_iter()
                    .any(|lookup| matches!(lookup, Ok(TransactionLookup::Pending(_))))
                {
                    return ReconcileStatus::verdict(ReconcileVerdict::Pending);
                }
                if near_lookup_unknown(&primary) && near_lookup_unknown(&backup) {
                    return ReconcileStatus::verdict(ReconcileVerdict::Unknown);
                }
                ReconcileStatus::verdict(ReconcileVerdict::Ambiguous)
            }
            Self::Evm(provider) => {
                // EVM reconciles on the transaction hash alone; the NEAR signer/
                // payer/asset identity checks live inside the provider's verify.
                let _ = (signer, payer, asset);
                let RecoveryPolicy::EvmConfirmations(required_confirmations) = recovery_policy
                else {
                    return ReconcileStatus::verdict(ReconcileVerdict::Ambiguous);
                };
                match provider
                    .reconcile_hash_with_confirmations(submit_hash, required_confirmations)
                    .await
                {
                    Ok(EvmReconcileStatus::Terminal(outcome)) => ReconcileStatus::verdict(
                        ReconcileVerdict::Terminal(evm_terminal_to_neutral(&outcome)),
                    ),
                    Ok(EvmReconcileStatus::Mined { .. } | EvmReconcileStatus::Pending) => {
                        ReconcileStatus::verdict(ReconcileVerdict::Pending)
                    }
                    Ok(EvmReconcileStatus::Unknown) => {
                        ReconcileStatus::verdict(ReconcileVerdict::Unknown)
                    }
                    Err(_) => ReconcileStatus::verdict(ReconcileVerdict::Ambiguous),
                }
            }
        }
    }

    /// A signer/chain-head snapshot from the *backup* RPC, for the dual-RPC nonce
    /// and expiry cross-checks during recovery. Carries height and nonce only;
    /// balance is not observed from the backup head (`signer_balance_atomic` is
    /// zero and unused by the recovery cross-checks).
    pub async fn backup_signer_head(&self) -> Result<SignerHead, SignerHeadError> {
        match self {
            Self::Near(provider) => {
                let head = provider.backup_relayer_head().await?;
                Ok(SignerHead {
                    chain_block_height: head.block_height,
                    chain_block_ref: head.block_hash.to_string(),
                    signer_nonce: u128::from(head.access_key_nonce),
                    signer_id: provider.relayer_account_id().to_string(),
                    signer_public_key: provider.relayer_public_key().to_string(),
                    signer_balance_atomic: 0,
                })
            }
            // EVM has no independent backup RPC; the primary head is authoritative
            // (integrity comes from confirmation depth, not dual-RPC agreement).
            Self::Evm(provider) => {
                let head = provider
                    .head()
                    .await
                    .map_err(|error| SignerHeadError(error.to_string()))?;
                Ok(evm_head_to_signer_head(&head))
            }
        }
    }

    /// Rebroadcast the exact durable submission bytes during recovery and
    /// classify the outcome, reusing the same interpretation as
    /// [`Self::broadcast`]. Never re-signs: the journaled bytes and their
    /// deterministic hash are replayed unchanged.
    pub async fn rebroadcast(
        &self,
        submit_bytes: &[u8],
        submit_hash: &str,
        payer: &str,
        asset: &str,
    ) -> BroadcastOutcome {
        match self {
            Self::Near(provider) => match provider.broadcast_exact(submit_bytes).await {
                Ok(TransactionLookup::Final(outcome)) => {
                    let (Ok(hash), Ok(payer_id), Ok(asset_id)) = (
                        submit_hash.parse::<CryptoHash>(),
                        payer.parse::<AccountId>(),
                        asset.parse::<AccountId>(),
                    ) else {
                        return BroadcastOutcome::Pending;
                    };
                    match interpret_near_final(
                        &outcome,
                        hash,
                        &provider.relayer_account_id(),
                        &payer_id,
                        &asset_id,
                    ) {
                        NearInterpretation::Terminal(terminal) => {
                            BroadcastOutcome::Terminal(terminal)
                        }
                        NearInterpretation::Indeterminate(_) => BroadcastOutcome::Pending,
                    }
                }
                Err(NearRpcError::TransactionRejected) => {
                    BroadcastOutcome::Rejected("transaction_rejected".to_owned())
                }
                Ok(TransactionLookup::Pending(_) | TransactionLookup::Unknown) | Err(_) => {
                    BroadcastOutcome::Pending
                }
            },
            Self::Evm(provider) => {
                // Rebroadcast the exact journaled bytes; an EVM outcome is never
                // terminal at submission, and the single-use ERC-3009 nonce makes
                // a re-submit idempotent.
                let _ = (submit_hash, payer, asset);
                let _ = provider.broadcast_raw(submit_bytes).await;
                BroadcastOutcome::Pending
            }
        }
    }
}

/// Chain-neutral inputs for one readiness refresh.
///
/// `rpc_ready` and `signer_head` remain independent because NEAR's liveness
/// probe and relayer-status query intentionally have distinct semantics. EVM
/// fills both fields from one conservative provider snapshot.
pub struct ChainReadinessObservation {
    /// Whether the provider's required RPC endpoints are currently usable.
    pub rpc_ready: bool,
    /// The signer snapshot used for funding and policy readiness.
    pub signer_head: Result<SignerHead, SignerHeadError>,
}

impl fmt::Debug for ChainReadinessObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChainReadinessObservation")
            .field("rpc_ready", &self.rpc_ready)
            .field("signer_ready", &self.signer_head.is_ok())
            .finish()
    }
}

async fn observe_evm_readiness<Load, Loaded, Error>(load_head: Load) -> ChainReadinessObservation
where
    Load: FnOnce() -> Loaded,
    Loaded: Future<Output = Result<EvmHead, Error>>,
    Error: fmt::Display,
{
    match load_head().await {
        Ok(head) => ChainReadinessObservation {
            rpc_ready: true,
            signer_head: Ok(evm_head_to_signer_head(&head)),
        },
        Err(error) => ChainReadinessObservation {
            rpc_ready: false,
            signer_head: Err(SignerHeadError(error.to_string())),
        },
    }
}

/// The exact-scheme requirements a payment was verified against, in neutral
/// (string / atomic-unit) form for logging and cross-checks.
#[derive(Clone)]
pub struct Requirements {
    /// CAIP-2 network id (e.g. `near:mainnet`, `eip155:8453`).
    pub network: String,
    /// Asset identifier: NEAR account id or EVM `0x` token address.
    pub asset: String,
    /// Recipient: NEAR account id or EVM `0x` address.
    pub pay_to: String,
    /// Amount in the asset's atomic units.
    pub amount: u128,
    /// Amount as the canonical decimal string advertised in requirements.
    pub amount_decimal: String,
}

impl fmt::Debug for Requirements {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Requirements")
            .field("network", &self.network)
            .field("asset", &"<redacted>")
            .field("pay_to", &"<redacted>")
            .field("amount", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// A verified payment ready to be prepared and submitted. Neutral fields drive
/// the engine; `detail` carries the chain-specific verified state the same
/// provider consumes in [`ChainProvider::prepare`].
#[derive(Clone)]
pub struct VerifiedPayment {
    /// Payer identity: NEAR account id or EVM `0x` address.
    pub payer: String,
    /// Canonical per-chain payload hash (idempotency + integrity anchor).
    pub payment_hash: [u8; 32],
    /// The requirements this payment satisfies.
    pub requirements: Requirements,
    /// Chain-specific verified state.
    pub detail: VerifiedDetail,
}

impl VerifiedPayment {
    /// Produce the durable, chain-neutral payment identity written at claim
    /// time. The request hash identifies this exact authorization; the scoped
    /// anchor identifies the chain-enforced single-use primitive.
    #[must_use]
    pub fn identity(&self) -> PaymentIdentity {
        let authorization = match &self.detail {
            VerifiedDetail::Near(near) => AuthorizationMetadata::Near {
                delegate_public_key: near.payer_public_key.to_string(),
                delegate_nonce: near.delegate_nonce.to_string(),
                max_block_height: near.max_block_height.to_string(),
            },
            VerifiedDetail::Evm(evm) => {
                let authorization = evm.authorization_identity();
                AuthorizationMetadata::Evm {
                    version: 2,
                    valid_after: authorization.valid_after.to_string(),
                    valid_before: authorization.valid_before.to_string(),
                }
            }
        };
        let (anchor_scope, anchor_value) = match &self.detail {
            VerifiedDetail::Near(_) => ("near".to_owned(), self.payment_hash),
            VerifiedDetail::Evm(evm) => (
                format!(
                    "{}:{}:{}",
                    self.requirements.network,
                    self.requirements.asset.to_ascii_lowercase(),
                    self.payer.to_ascii_lowercase()
                ),
                evm.authorization_identity().nonce.0,
            ),
        };
        PaymentIdentity {
            request_hash: self.payment_hash,
            anchor_scope,
            anchor_value,
            authorization,
        }
    }
}

/// Durable identity extracted from a verified payment without retaining the
/// signed bearer payload.
#[derive(Clone)]
pub struct PaymentIdentity {
    /// Hash of the exact signed payment request.
    pub request_hash: [u8; 32],
    /// Namespace in which the chain-enforced anchor is single use.
    pub anchor_scope: String,
    /// Exact chain-enforced replay anchor (delegate hash / ERC-3009 nonce).
    pub anchor_value: [u8; 32],
    /// Minimal metadata needed to bind a stored submission during recovery.
    pub authorization: AuthorizationMetadata,
}

impl fmt::Debug for PaymentIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PaymentIdentity")
            .field("request_hash", &"<redacted>")
            .field("anchor_scope", &"<redacted>")
            .field("anchor_value", &"<redacted>")
            .field("authorization", &self.authorization)
            .finish()
    }
}

/// Minimal chain-specific authorization metadata retained by the journal.
#[derive(Clone)]
pub enum AuthorizationMetadata {
    /// NEAR delegate identity needed for stored-transaction validation.
    Near {
        /// Delegate public key.
        delegate_public_key: String,
        /// Delegate nonce.
        delegate_nonce: String,
        /// Delegate expiry block height.
        max_block_height: String,
    },
    /// EVM validity window; payer/token/value and nonce are stored in neutral
    /// settlement columns and the scoped anchor.
    Evm {
        /// Metadata schema version.
        version: u8,
        /// ERC-3009 lower validity bound.
        valid_after: String,
        /// ERC-3009 upper validity bound.
        valid_before: String,
    },
}

impl fmt::Debug for AuthorizationMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Near { .. } => formatter.write_str("Near(<redacted>)"),
            Self::Evm { version, .. } => formatter
                .debug_struct("Evm")
                .field("version", version)
                .field("authorization", &"<redacted>")
                .finish(),
        }
    }
}

impl fmt::Debug for VerifiedPayment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedPayment")
            .field("payer", &"<redacted>")
            .field("payment_hash", &"<redacted>")
            .field("requirements", &self.requirements)
            .field("detail", &"<redacted>")
            .finish()
    }
}

/// Chain-specific verified-payment state.
#[derive(Clone)]
pub enum VerifiedDetail {
    /// NEAR: the decoded, signature-checked delegate payment.
    Near(NearVerified),
    /// EVM: the verified ERC-3009 authorization + payer signature.
    Evm(EvmVerifiedPayment),
}

impl fmt::Debug for VerifiedDetail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Near(_) => formatter.write_str("Near(<redacted>)"),
            Self::Evm(_) => formatter.write_str("Evm(<redacted>)"),
        }
    }
}

/// Recovery rules carried with the exact durable submission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryPolicy {
    /// NEAR final execution and receipt-graph validation.
    NearFinality,
    /// EVM conservative confirmation depth.
    EvmConfirmations(u64),
}

/// Chain-neutral exact bytes and identity persisted before broadcast.
#[derive(Clone)]
pub struct DurableSubmission {
    /// Facilitator relayer/signer identity.
    pub submitter: String,
    /// Submission account/access-key nonce.
    pub nonce: u128,
    /// Exact signed bytes; recovery never creates replacements.
    pub bytes: Vec<u8>,
    /// Hash of the exact signed bytes.
    pub hash: String,
    /// Provider-owned reconciliation policy.
    pub recovery_policy: RecoveryPolicy,
}

impl fmt::Debug for DurableSubmission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DurableSubmission")
            .field("submitter", &"<redacted>")
            .field("nonce", &"<redacted>")
            .field("bytes", &"<redacted>")
            .field("hash", &"<redacted>")
            .field("recovery_policy", &self.recovery_policy)
            .finish()
    }
}

/// Journal values that bind one stored EVM envelope to one settlement.
pub struct StoredEvmSubmission {
    /// CAIP-2 network.
    pub network: String,
    /// Journaled transaction hash.
    pub hash: String,
    /// Configured facilitator signer recorded at claim.
    pub submitter: String,
    /// Journaled signer account nonce.
    pub nonce: u128,
    /// Circle USDC token address.
    pub asset: String,
    /// Verified payer address.
    pub payer: String,
    /// Policy-authorized recipient.
    pub payee: String,
    /// Exact atomic token amount.
    pub amount: String,
    /// ERC-3009 lower validity bound.
    pub valid_after: String,
    /// ERC-3009 upper validity bound.
    pub valid_before: String,
    /// Scoped ERC-3009 nonce namespace.
    pub anchor_scope: String,
    /// Raw ERC-3009 nonce.
    pub anchor_value: [u8; 32],
    /// Canonical signed-payment identity.
    pub payment_hash: [u8; 32],
    /// Gas limit captured in the admission policy snapshot.
    pub gas_limit: u64,
    /// Maximum fee per gas captured in the admission policy snapshot.
    pub max_fee_per_gas: u128,
}

impl fmt::Debug for StoredEvmSubmission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredEvmSubmission")
            .field("network", &self.network)
            .field("payment", &"<redacted>")
            .field("gas_limit", &self.gas_limit)
            .field("max_fee_per_gas", &self.max_fee_per_gas)
            .finish_non_exhaustive()
    }
}

/// Typed failure while binding stored bytes to a settlement row.
#[derive(Debug, thiserror::Error)]
pub enum StoredSubmissionError {
    /// The configured provider cannot validate this submission family.
    #[error("stored submission does not match the configured provider")]
    WrongProvider,
    /// A journal field is malformed or conflicts with immutable provider state.
    #[error("stored EVM submission has an invalid {0}")]
    Field(&'static str),
    /// Exact-envelope validation rejected the bytes or calldata.
    #[error("stored EVM transaction is inconsistent with the settlement")]
    Transaction(#[source] StoredTransactionError),
}

/// A snapshot of the facilitator's signer/relayer and chain head, used to
/// prepare a submission and to gate readiness.
#[derive(Clone)]
pub struct SignerHead {
    /// NEAR block height / EVM block number at snapshot time.
    pub chain_block_height: u64,
    /// NEAR block hash / EVM unused (empty).
    pub chain_block_ref: String,
    /// NEAR access-key nonce / EVM account (transaction) nonce.
    pub signer_nonce: u128,
    /// Signer identity: NEAR relayer account id / EVM signer `0x` address.
    pub signer_id: String,
    /// Signer public key (NEAR ed25519) / empty for EVM (address is `signer_id`).
    pub signer_public_key: String,
    /// Signer's native-gas balance in atomic units (yoctoNEAR / wei).
    pub signer_balance_atomic: u128,
}

impl fmt::Debug for SignerHead {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignerHead")
            .field("chain_block_height", &self.chain_block_height)
            .field("chain_block_ref", &"<redacted>")
            .field("signer_nonce", &"<redacted>")
            .field("signer_id", &"<redacted>")
            .field("signer_public_key", &"<redacted>")
            .field("signer_balance_atomic", &"<redacted>")
            .finish()
    }
}

/// A prepared, signed submission. `submit_bytes`/`submit_hash` are durable and
/// must never be re-signed once journaled; recovery rebroadcasts these exact
/// bytes.
#[derive(Clone)]
pub struct Prepared {
    /// Signed submission bytes: Borsh `SignedTransaction` (NEAR) / RLP tx (EVM).
    pub submit_bytes: Vec<u8>,
    /// Submission hash: NEAR `CryptoHash` string / EVM `0x` tx hash.
    pub submit_hash: String,
    /// Signer identity used for the submission.
    pub signer_id: String,
    /// Signer public key (NEAR ed25519) / empty for EVM (address is `signer_id`).
    pub signer_public_key: String,
    /// Signer nonce burned by this submission.
    pub signer_nonce: u128,
    /// Chain-specific durable extras.
    pub detail: PreparedDetail,
}

impl fmt::Debug for Prepared {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Prepared")
            .field("submit_bytes", &"<redacted>")
            .field("submit_hash", &"<redacted>")
            .field("signer_id", &"<redacted>")
            .field("signer_public_key", &"<redacted>")
            .field("signer_nonce", &"<redacted>")
            .field("detail", &"<redacted>")
            .finish()
    }
}

/// Chain-specific prepared-transaction state.
#[derive(Clone)]
pub enum PreparedDetail {
    /// NEAR: the prepared outer meta-transaction.
    Near(NearPrepared),
    /// EVM: the durable signed ERC-3009 settlement transaction.
    Evm(EvmPrepared),
}

impl fmt::Debug for PreparedDetail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Near(_) => formatter.write_str("Near(<redacted>)"),
            Self::Evm(_) => formatter.write_str("Evm(<redacted>)"),
        }
    }
}

/// The result of broadcasting a prepared submission.
#[derive(Debug)]
pub enum BroadcastOutcome {
    /// The chain reached a terminal outcome in one shot (NEAR fast finality).
    Terminal(TerminalOutcome),
    /// Deterministic on-chain or relayer rejection (never retried).
    Rejected(String),
    /// Submitted but not yet terminal — recovery/confirmation resolves it.
    /// EVM always lands here until the confirmation-depth policy is met.
    Pending,
}

/// The lifecycle position of a submitted transaction, observed during
/// reconciliation.
#[derive(Debug)]
pub enum StatusState {
    /// The chain has no record of the submission.
    Unknown,
    /// Seen but not yet mined/final.
    Pending,
    /// Mined with a given confirmation depth (EVM), below the required depth.
    Mined {
        /// Confirmations observed so far.
        confirmations: u64,
        /// Block number the transaction was mined in.
        block_number: u64,
    },
    /// Terminal and trusted (NEAR final, or EVM at/after required confirmations).
    Final,
}

/// The outcome of a reconciliation status query.
#[derive(Debug)]
pub struct StatusOutcome {
    /// Where the submission sits in its lifecycle.
    pub state: StatusState,
    /// Present when `state` is terminal.
    pub terminal: Option<TerminalOutcome>,
}

/// A terminal settlement outcome with the cost and evidence needed for the
/// journal. The provider validates the chain-specific receipt/log locus before
/// constructing this, so the engine consumes only neutral fields.
#[derive(Clone)]
pub struct TerminalOutcome {
    /// Whether the transfer succeeded (inner receipt / log success).
    pub success: bool,
    /// The settled transaction hash.
    pub tx_hash: String,
    /// Recipient balance delta in atomic units, when observable (NEAR: `None`).
    pub recipient_delta_atomic: Option<u128>,
    /// Native-gas fee actually spent by the facilitator, in atomic units
    /// (yoctoNEAR / wei).
    pub fee_atomic: u128,
    /// Gas units consumed (NEAR gas / EVM gas used), for the cost metric.
    pub gas_units: u64,
    /// Present iff `!success`: the authoritative on-chain failure reason.
    pub failure_detail: Option<String>,
    /// The block the settled transaction mined into. `None` for NEAR (finality
    /// is fast-final, not block-anchored); `Some` for eip155, journaled as the
    /// reorg-safety audit trail behind the confirmation-depth decision.
    pub mined_block_number: Option<u64>,
    /// The mined block hash, when the chain reports it. eip155-only.
    pub mined_block_hash: Option<String>,
    /// The confirmation depth observed when this outcome was accepted as
    /// terminal (`>= required_confirmations` for eip155). `None` for NEAR.
    pub confirmations: Option<u64>,
}

impl fmt::Debug for TerminalOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalOutcome")
            .field("success", &self.success)
            .field("tx_hash", &"<redacted>")
            .field("recipient_delta_atomic", &"<redacted>")
            .field("fee_atomic", &"<redacted>")
            .field("gas_units", &self.gas_units)
            .field("failure_detail", &self.failure_detail)
            .field("mined_block_number", &self.mined_block_number)
            .field("mined_block_hash", &"<redacted>")
            .field("confirmations", &self.confirmations)
            .finish()
    }
}

/// A neutral verification rejection: the machine reason plus the flag the engine
/// needs to choose an HTTP disposition, without exposing the per-chain failure
/// enum.
#[derive(Clone, Debug)]
pub struct VerifyRejection {
    /// Machine reason (e.g. `insufficient_funds`). NEAR reasons are a fixed set;
    /// EVM reasons carry the upstream detail, so this is owned.
    pub reason: String,
    /// Whether the failure reflects an unavailable/ambiguous RPC lookup rather
    /// than a definitive invalid payment (engine returns 503, not a rejection).
    pub rpc_ambiguous: bool,
}

impl VerifyRejection {
    fn from_near(failure: NearVerificationFailure) -> Self {
        Self {
            reason: failure.reason().to_owned(),
            rpc_ambiguous: near_verification_is_rpc_ambiguous(failure),
        }
    }

    fn from_evm(rejection: EvmVerifyRejection) -> Self {
        Self {
            reason: rejection.reason,
            rpc_ambiguous: rejection.rpc_ambiguous,
        }
    }
}

/// NEAR verification failures that reflect an unavailable/ambiguous RPC lookup
/// rather than a definitive invalid payment.
const fn near_verification_is_rpc_ambiguous(failure: NearVerificationFailure) -> bool {
    matches!(
        failure,
        NearVerificationFailure::CurrentBlockHeightUnavailable
            | NearVerificationFailure::AccessKeyLookupFailed
            | NearVerificationFailure::AccountLookupFailed
            | NearVerificationFailure::TokenAccountLookupFailed
            | NearVerificationFailure::BalanceCheckFailed
            | NearVerificationFailure::StorageCheckFailed
    )
}

/// Why a submission could not be prepared from a verified payment + signer head.
#[derive(Debug)]
pub enum PrepareError {
    /// The neutral signer head did not carry a valid chain reference or nonce.
    InvalidSignerHead,
    /// The chain provider failed to build or sign the submission (the string is
    /// the per-chain failure, opaque to the engine).
    Provider(String),
}

/// The signer/chain head could not be read (an RPC was unavailable, or the NEAR
/// relayer key is not full-access). Opaque at the engine boundary — any head
/// failure is treated as a readiness fault; the string is for logs.
#[derive(Debug)]
pub struct SignerHeadError(String);

impl fmt::Display for SignerHeadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SignerHeadError {}

impl From<NearRpcError> for SignerHeadError {
    fn from(error: NearRpcError) -> Self {
        Self(error.to_string())
    }
}

/// Map an [`EvmHead`] snapshot into the neutral signer head. EVM has no
/// block-hash signing anchor, so `chain_block_ref` is empty; the address doubles
/// as both signer id and public key so the store's relayer-policy keys align.
fn evm_head_to_signer_head(head: &EvmHead) -> SignerHead {
    let address = head.signer_address.to_string();
    SignerHead {
        chain_block_height: head.block_number,
        chain_block_ref: String::new(),
        signer_nonce: u128::from(head.account_nonce),
        signer_id: address.clone(),
        signer_public_key: address,
        signer_balance_atomic: head.gas_balance_wei,
    }
}

/// Map an EVM terminal outcome into the neutral terminal outcome. The recipient
/// balance delta is not observed here (`None`); the fee is the facilitator's
/// actual wei gas cost.
fn evm_terminal_to_neutral(outcome: &EvmTerminalOutcome) -> TerminalOutcome {
    TerminalOutcome {
        success: outcome.success,
        tx_hash: outcome.tx_hash.to_string(),
        recipient_delta_atomic: None,
        fee_atomic: outcome.fee_wei,
        gas_units: outcome.gas_used,
        failure_detail: (!outcome.success).then(|| "evm_execution_reverted".to_owned()),
        mined_block_number: Some(outcome.block_number),
        mined_block_hash: outcome.block_hash.map(|hash| hash.to_string()),
        confirmations: Some(outcome.confirmations),
    }
}

/// The neutral verdict from reconciling a submission against both RPCs.
#[derive(Debug)]
pub struct ReconcileStatus {
    /// Where the submission sits after cross-checking primary and backup.
    pub verdict: ReconcileVerdict,
    /// Whether the backup RPC supplied a final outcome the primary did not.
    pub rpc_failover: bool,
}

impl ReconcileStatus {
    /// A verdict with no RPC failover (the common case for every branch that
    /// does not consult a backup-only final outcome).
    #[must_use]
    fn verdict(verdict: ReconcileVerdict) -> Self {
        Self {
            verdict,
            rpc_failover: false,
        }
    }
}

/// The lifecycle position a reconciliation observed for a submitted transaction.
#[derive(Debug)]
pub enum ReconcileVerdict {
    /// An authoritative terminal outcome (identity + receipt graph validated).
    Terminal(TerminalOutcome),
    /// A final outcome exists but is not authoritative (identity mismatch or a
    /// non-definitive receipt); stay submitted and retry. Carries the reason.
    Indeterminate(String),
    /// At least one RPC reports the submission pending; wait.
    Pending,
    /// Both RPCs report no record; proceed to recovery (nonce/expiry/rebroadcast).
    Unknown,
    /// Primary and backup disagree on a final outcome; integrity fault.
    Conflict,
    /// Neither final, pending, nor both-unknown; RPC ambiguity, integrity fault.
    Ambiguous,
}

/// The interpretation of a NEAR final outcome, shared by broadcast, rebroadcast,
/// and reconcile so all three classify identically.
enum NearInterpretation {
    /// An authoritative success or definitive failure.
    Terminal(TerminalOutcome),
    /// Identity mismatch or a non-authoritative receipt; carries the reason.
    Indeterminate(String),
}

/// Bind a NEAR final outcome to the prepared transaction and interpret its
/// receipt graph. Identity mismatch or a non-authoritative (indeterminate)
/// receipt state yields [`NearInterpretation::Indeterminate`] (the caller keeps
/// the submission for reconciliation); only an authoritative success or
/// definitive failure is [`NearInterpretation::Terminal`].
fn interpret_near_final(
    outcome: &FinalExecutionOutcomeView,
    transaction_hash: CryptoHash,
    signer: &AccountId,
    payer: &AccountId,
    asset: &AccountId,
) -> NearInterpretation {
    if let Err(error) = validate_final_outcome_identity(outcome, transaction_hash, signer, payer) {
        return NearInterpretation::Indeterminate(error.to_string());
    }
    let (gas_units, fee_atomic) = execution_cost_near(outcome);
    match interpret_final_outcome(outcome, payer, asset) {
        Ok(_) => NearInterpretation::Terminal(TerminalOutcome {
            success: true,
            tx_hash: transaction_hash.to_string(),
            recipient_delta_atomic: None,
            fee_atomic,
            gas_units,
            failure_detail: None,
            mined_block_number: None,
            mined_block_hash: None,
            confirmations: None,
        }),
        Err(error) if error.is_definitive_failure() => {
            NearInterpretation::Terminal(TerminalOutcome {
                success: false,
                tx_hash: transaction_hash.to_string(),
                recipient_delta_atomic: None,
                fee_atomic,
                gas_units,
                failure_detail: Some(error.to_string()),
                mined_block_number: None,
                mined_block_hash: None,
                confirmations: None,
            })
        }
        Err(error) => NearInterpretation::Indeterminate(error.to_string()),
    }
}

/// The raw final outcome from a NEAR transaction lookup, if present.
fn near_final_outcome(
    lookup: &Result<TransactionLookup, NearRpcError>,
) -> Option<&FinalExecutionOutcomeView> {
    match lookup {
        Ok(TransactionLookup::Final(outcome)) => Some(outcome.as_ref()),
        Ok(TransactionLookup::Unknown | TransactionLookup::Pending(_)) | Err(_) => None,
    }
}

/// Whether a NEAR transaction lookup authoritatively reports "no such record".
fn near_lookup_unknown(lookup: &Result<TransactionLookup, NearRpcError>) -> bool {
    matches!(
        lookup,
        Ok(TransactionLookup::Unknown) | Err(NearRpcError::TransactionUnknown)
    )
}

/// Two RPCs conflict when both report a final outcome and the outcomes differ.
/// A finalized transaction is deterministic, so honest RPCs must agree.
fn final_outcomes_conflict<T: Eq>(primary: Option<&T>, backup: Option<&T>) -> bool {
    matches!((primary, backup), (Some(primary), Some(backup)) if primary != backup)
}

/// Sum gas and tokens burnt across the transaction and its receipts. (Mirrors
/// the reconcile path's `execution_cost` in `service`; that copy is removed when
/// reconcile moves behind this provider.)
fn execution_cost_near(outcome: &FinalExecutionOutcomeView) -> (u64, u128) {
    let mut gas = outcome.transaction_outcome.outcome.gas_burnt.as_gas();
    let mut tokens = outcome
        .transaction_outcome
        .outcome
        .tokens_burnt
        .as_yoctonear();
    for receipt in &outcome.receipts_outcome {
        gas = gas.saturating_add(receipt.outcome.gas_burnt.as_gas());
        tokens = tokens.saturating_add(receipt.outcome.tokens_burnt.as_yoctonear());
    }
    (gas, tokens)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{EvmHead, final_outcomes_conflict, observe_evm_readiness};

    #[test]
    fn conflicting_final_results_fail_closed() {
        assert!(!final_outcomes_conflict(Some(&1_u8), Some(&1_u8)));
        assert!(final_outcomes_conflict(Some(&1_u8), Some(&2_u8)));
        assert!(!final_outcomes_conflict(Some(&1_u8), None));
    }

    #[tokio::test]
    async fn evm_readiness_uses_one_conservative_head_snapshot() {
        let calls = Cell::new(0_u8);
        let observation = observe_evm_readiness(|| {
            calls.set(calls.get().saturating_add(1));
            async {
                Ok::<_, &'static str>(EvmHead {
                    block_number: 42,
                    account_nonce: 7,
                    gas_balance_wei: 1_000,
                    signer_address: [0_u8; 20].into(),
                })
            }
        })
        .await;

        assert_eq!(calls.get(), 1);
        assert!(observation.rpc_ready);
        let signer = observation
            .signer_head
            .unwrap_or_else(|_| std::process::abort());
        assert_eq!(signer.chain_block_height, 42);
        assert_eq!(signer.signer_nonce, 7);
        assert_eq!(signer.signer_balance_atomic, 1_000);
    }

    #[tokio::test]
    async fn evm_readiness_fails_both_gates_from_one_failed_snapshot() {
        let calls = Cell::new(0_u8);
        let observation = observe_evm_readiness(|| {
            calls.set(calls.get().saturating_add(1));
            async { Err::<EvmHead, _>("unavailable") }
        })
        .await;

        assert_eq!(calls.get(), 1);
        assert!(!observation.rpc_ready);
        assert!(observation.signer_head.is_err());
    }
}
