//! The live EVM settlement provider: RPC-facing verify, signer head, and
//! broadcast, built on upstream `x402-chain-eip155`.
//!
//! This is the network-touching half of the durable path. Authoritative
//! ERC-3009 verification is reused from upstream, followed by this provider's
//! settleability gate (EOA and deployed EIP-1271 only). Preparation snapshots a
//! pending facilitator nonce, applies an absolute EIP-1559 fee cap, estimates
//! Base L1 data cost over the exact signed bytes, and returns journalable bytes.
//! Reconciliation reads explicitly configured primary and backup endpoints,
//! includes Base's receipt `l1Fee`, and trusts terminality only at the
//! settlement record's confirmation depth.

use alloy_primitives::{Address, B256, Bytes, U256, address};
use alloy_provider::{DynProvider, Provider, ProviderBuilder};
use alloy_rpc_types_eth::{TransactionInput, TransactionRequest};
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::SolCall;
use alloy_transport::{RpcError, TransportError, TransportErrorKind};
use std::fmt;
use std::future::Future;
use std::io::Write as _;
use std::path::Path;
use std::time::Duration;
use url::Url;
use x402_types::chain::{ChainId, FromConfig};
use x402_types::proto;
use x402_types::proto::PaymentVerificationError;
use x402_types::proto::v2::VerifyResponse;
use x402_types::scheme::X402SchemeFacilitatorError;

use x402_chain_eip155::chain::config::{Eip155ChainConfig, Eip155ChainConfigInner};
use x402_chain_eip155::chain::{Eip155ChainReference, Eip155MetaTransactionProvider};
use x402_chain_eip155::v2_eip155_exact::FacilitatorVerifyRequest;
use x402_chain_eip155::v2_eip155_exact::eip3009::verify_eip3009_payment;

use crate::prepare::{
    Erc3009Authorization, EvmAuthorizationIdentity, EvmFeeEnvelope, EvmPrepared, EvmSignError,
    EvmSignerHead, sign_settlement_transaction,
};
use crate::settle::{
    UnsupportedSignature, build_transfer_domain, classify_settleable_signature,
    eip712_transfer_hash, settlement_calldata,
};

const BASE_GAS_PRICE_ORACLE: Address = address!("0x420000000000000000000000000000000000000F");

#[allow(clippy::all, clippy::pedantic, missing_docs)]
mod base_abi {
    alloy_sol_types::sol! {
        function getL1Fee(bytes memory transaction) external view returns (uint256);
    }
}

fn canonical_domain_name(chain_id: u64) -> Option<&'static str> {
    match chain_id {
        8_453 => Some("USD Coin"),
        84_532 => Some("USDC"),
        _ => None,
    }
}

fn validate_token_domain(
    expected_name: &str,
    supplied_name: &str,
    supplied_version: &str,
) -> Result<(), EvmVerifyRejection> {
    if supplied_name != expected_name || supplied_version != "2" {
        return Err(EvmVerifyRejection::definitive("invalid_token_domain"));
    }
    Ok(())
}

fn classify_signature_before_rpc(
    authorization: &Erc3009Authorization,
    signature: &Bytes,
    domain: &alloy_sol_types::Eip712Domain,
) -> Result<B256, EvmVerifyRejection> {
    let payment_hash = eip712_transfer_hash(authorization, domain);
    classify_settleable_signature(authorization.from, signature, &payment_hash).map_err(
        |error| {
            EvmVerifyRejection::definitive(match error {
                UnsupportedSignature::CounterfactualWallet => "unsupported_eip6492",
                UnsupportedSignature::Malformed => "invalid_signature",
            })
        },
    )?;
    Ok(payment_hash)
}

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
    authorization: Erc3009Authorization,
    /// The payer's raw signature bytes (opaque; classified at prepare time).
    signature: Bytes,
}

impl EvmVerifiedPayment {
    #[must_use]
    pub(crate) fn signature(&self) -> &Bytes {
        &self.signature
    }

    /// Minimal chain-specific identity to retain before signed submission bytes
    /// exist. Payer signature bytes are never exposed as journal metadata.
    #[must_use]
    pub const fn authorization_identity(&self) -> EvmAuthorizationIdentity {
        EvmAuthorizationIdentity {
            nonce: self.authorization.nonce,
            valid_after: self.authorization.valid_after,
            valid_before: self.authorization.valid_before,
        }
    }
}

impl fmt::Debug for EvmVerifiedPayment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EvmVerifiedPayment")
            .field("payer", &"<redacted>")
            .field("payment_hash", &"<redacted>")
            .field("asset", &"<redacted>")
            .field("pay_to", &"<redacted>")
            .field("amount", &"<redacted>")
            .field("authorization", &"<redacted>")
            .field("signature", &"<redacted>")
            .finish()
    }
}

/// A snapshot of the facilitator signer's account and the chain head, read in
/// one shot to pin a submission and gate readiness.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct EvmHead {
    /// Latest block number (the EVM analog of NEAR's chain block height).
    pub block_number: u64,
    /// The signer's next account (transaction) nonce.
    pub account_nonce: u64,
    /// The signer's native-gas (ETH) balance in wei.
    pub gas_balance_wei: u128,
    /// The signer (`from`) address.
    pub signer_address: Address,
}

impl fmt::Debug for EvmHead {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EvmHead")
            .field("block_number", &self.block_number)
            .field("account_nonce", &"<redacted>")
            .field("gas_balance_wei", &self.gas_balance_wei)
            .field("signer_address", &"<redacted>")
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
        X402SchemeFacilitatorError::OnchainFailure(_) => EvmVerifyRejection {
            reason: "onchain_failure".to_owned(),
            rpc_ambiguous: true,
        },
        X402SchemeFacilitatorError::PaymentVerification(inner) => {
            EvmVerifyRejection::definitive(match inner {
                PaymentVerificationError::InvalidFormat(_) => "invalid_format",
                PaymentVerificationError::InvalidPaymentAmount => "invalid_payment_amount",
                PaymentVerificationError::Early => "authorization_not_yet_valid",
                PaymentVerificationError::Expired => "authorization_expired",
                PaymentVerificationError::ChainIdMismatch => "chain_id_mismatch",
                PaymentVerificationError::RecipientMismatch => "recipient_mismatch",
                PaymentVerificationError::AssetMismatch => "asset_mismatch",
                PaymentVerificationError::InsufficientFunds => "insufficient_funds",
                PaymentVerificationError::InsufficientAllowance => "insufficient_allowance",
                PaymentVerificationError::InvalidSignature(_) => "invalid_signature",
                PaymentVerificationError::TransactionSimulation(_) => "simulation_failed",
                PaymentVerificationError::UnsupportedChain => "unsupported_chain",
                PaymentVerificationError::UnsupportedScheme => "unsupported_scheme",
                PaymentVerificationError::AcceptedRequirementsMismatch => {
                    "accepted_requirements_mismatch"
                }
            })
        }
    }
}

/// Why constructing the provider failed.
#[derive(Debug, thiserror::Error)]
pub enum EvmConnectError {
    /// The upstream chain config could not be assembled.
    #[error("evm chain config invalid: {0}")]
    Config(&'static str),
    /// The upstream provider failed to connect / validate required contracts.
    #[error("evm provider connect failed")]
    Connect,
}

/// Generate a new secp256k1 signer, write its `0x`-prefixed hex private key to a
/// fresh mode-0600 file, and return the signer's `0x` address. The file is opened
/// `create_new`, so an existing credential is never clobbered. The private key is
/// never returned, printed, or placed in process arguments — it exists only in the
/// short-lived generator process and in `output`. The written form (`0x` + 64 hex,
/// one trailing newline) is exactly what the service credential loader expects.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] if `output` already exists or cannot
/// be created / written.
pub fn generate_signer_key_file(output: &Path) -> std::io::Result<String> {
    let signer = PrivateKeySigner::random();
    let address = signer.address().to_string();
    let key_hex = format!("0x{}", hex::encode(signer.to_bytes().as_slice()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(output)?;
    writeln!(file, "{key_hex}")?;
    file.sync_all()?;
    Ok(address)
}

/// Why an RPC-facing operation failed.
#[derive(Debug, thiserror::Error)]
pub enum EvmRpcError {
    /// The JSON-RPC call failed at the transport.
    #[error("evm rpc call failed")]
    Rpc,
    /// Independent readers disagree about identity or durable chain state.
    #[error("evm RPC readers disagree: {0}")]
    ReaderDisagreement(&'static str),
    /// A persisted transaction hash is malformed.
    #[error("stored EVM transaction hash is malformed")]
    InvalidTransactionHash,
    /// An endpoint returned a malformed or mismatched transaction object.
    #[error("EVM transaction lookup is malformed: {0}")]
    InvalidTransaction(&'static str),
    /// A receipt omitted or malformed a required Base field.
    #[error("Base transaction receipt is malformed: {0}")]
    InvalidReceipt(&'static str),
    /// The requested confirmation policy is invalid.
    #[error("required EVM confirmations must be at least one")]
    InvalidConfirmations,
    /// The independent safe head is behind the claimed receipt block.
    #[error("EVM head is behind the transaction receipt")]
    HeadBehindReceipt,
    /// Base L1 or execution fee arithmetic exceeded the supported range.
    #[error("EVM fee value exceeds the supported range")]
    FeeOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EvmReadinessReader {
    Primary,
    Backup,
}

impl EvmReadinessReader {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Backup => "backup",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EvmReadinessOperation {
    ChainId,
    BlockNumber,
    PendingNonce,
    GasBalance,
}

impl EvmReadinessOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ChainId => "eth_chainId",
            Self::BlockNumber => "eth_blockNumber",
            Self::PendingNonce => "eth_getTransactionCount_pending",
            Self::GasBalance => "eth_getBalance",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EvmReadinessDependencyError {
    JsonRpcError,
    NullResponse,
    UnsupportedFeature,
    LocalUsage,
    Serialization,
    Deserialization,
    HttpRateLimited,
    HttpTemporarilyUnavailable,
    HttpClientError,
    HttpServerError,
    HttpStatus,
    MissingBatchResponse,
    BackendGone,
    PubsubUnavailable,
    CustomTransport,
    NonRetryableTransport,
    Transport,
}

impl EvmReadinessDependencyError {
    const fn as_str(self) -> &'static str {
        match self {
            Self::JsonRpcError => "json_rpc_error",
            Self::NullResponse => "null_response",
            Self::UnsupportedFeature => "unsupported_feature",
            Self::LocalUsage => "local_usage",
            Self::Serialization => "serialization",
            Self::Deserialization => "deserialization",
            Self::HttpRateLimited => "http_rate_limited",
            Self::HttpTemporarilyUnavailable => "http_temporarily_unavailable",
            Self::HttpClientError => "http_client_error",
            Self::HttpServerError => "http_server_error",
            Self::HttpStatus => "http_status",
            Self::MissingBatchResponse => "missing_batch_response",
            Self::BackendGone => "backend_gone",
            Self::PubsubUnavailable => "pubsub_unavailable",
            Self::CustomTransport => "custom_transport",
            Self::NonRetryableTransport => "non_retryable_transport",
            Self::Transport => "transport",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EvmReadinessEndpointError {
    operation: EvmReadinessOperation,
    error: EvmReadinessDependencyError,
    http_status: Option<u16>,
    json_rpc_code: Option<i64>,
}

/// A bounded, secret-free reason why an EVM dual-reader signer-head snapshot
/// could not be used for readiness.
///
/// This deliberately carries neither provider identity nor any observed chain
/// value. The service may expose its fixed [`Self::as_str`] code only through
/// protected telemetry and structured logs; public readiness remains a boolean
/// gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum EvmHeadSnapshotFailure {
    /// The configured primary reader did not produce a usable snapshot.
    #[error("primary EVM RPC head snapshot unavailable")]
    PrimaryRpcUnavailable,
    /// The configured backup reader did not produce a usable snapshot.
    #[error("backup EVM RPC head snapshot unavailable")]
    BackupRpcUnavailable,
    /// Neither configured reader produced a usable snapshot.
    #[error("both EVM RPC head snapshots unavailable")]
    BothRpcUnavailable,
    /// A reader reported a chain identity other than the configured Base chain.
    #[error("EVM RPC chain identity did not match the configured chain")]
    ChainIdMismatch,
    /// The readers disagreed about the signer's pending transaction nonce.
    #[error("EVM RPC readers disagreed about the pending signer nonce")]
    PendingNonceDisagreement,
}

impl EvmHeadSnapshotFailure {
    /// Stable low-cardinality code for protected telemetry and structured logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PrimaryRpcUnavailable => "primary_rpc_unavailable",
            Self::BackupRpcUnavailable => "backup_rpc_unavailable",
            Self::BothRpcUnavailable => "both_rpc_unavailable",
            Self::ChainIdMismatch => "chain_id_mismatch",
            Self::PendingNonceDisagreement => "pending_nonce_disagreement",
        }
    }

    const fn into_rpc_error(self) -> EvmRpcError {
        match self {
            Self::PrimaryRpcUnavailable | Self::BackupRpcUnavailable | Self::BothRpcUnavailable => {
                EvmRpcError::Rpc
            }
            Self::ChainIdMismatch => EvmRpcError::ReaderDisagreement("chain id"),
            Self::PendingNonceDisagreement => {
                EvmRpcError::ReaderDisagreement("pending account nonce")
            }
        }
    }
}

/// Why a settlement could not be prepared.
#[derive(Debug, thiserror::Error)]
pub enum EvmPrepareError {
    /// The payer signature could not be turned into calldata.
    #[error(transparent)]
    Signature(#[from] UnsupportedSignature),
    /// An RPC needed to pin the submission (nonce, fee market) failed.
    #[error(transparent)]
    Rpc(#[from] EvmRpcError),
    /// Signing the settlement transaction failed.
    #[error(transparent)]
    Sign(#[from] EvmSignError),
    /// RPC fee estimation would sign above the deployment's absolute cap.
    #[error("estimated EVM max fee per gas exceeds the configured cap")]
    FeeCapExceeded,
    /// RPC returned an internally invalid EIP-1559 fee estimate.
    #[error("EVM RPC returned an invalid fee estimate")]
    InvalidFeeEstimate,
}

/// The current lifecycle position of a submitted settlement transaction.
#[derive(Clone, Debug)]
pub enum EvmReconcileStatus {
    /// No receipt: not mined yet, dropped from the mempool, or reorged out. The
    /// engine keeps the submission and rebroadcasts the same journaled bytes — a
    /// re-submit is idempotent because the ERC-3009 authorization nonce is
    /// single-use on-chain.
    Unknown,
    /// A receipt exists but is not yet anchored to a block (transient).
    Pending,
    /// Mined but below the required confirmation depth; wait and re-check.
    Mined {
        /// Confirmations observed so far (inclusive of the mining block).
        confirmations: u64,
        /// Block the transaction was mined in.
        block_number: u64,
    },
    /// At or beyond the required confirmation depth — reorg-safe and terminal.
    Terminal(EvmTerminalOutcome),
}

/// A terminal EVM settlement outcome (mined to the required confirmation depth).
#[derive(Clone)]
pub struct EvmTerminalOutcome {
    /// Whether the on-chain execution succeeded (receipt status).
    pub success: bool,
    /// The settled transaction hash.
    pub tx_hash: B256,
    /// Block the transaction was mined in.
    pub block_number: u64,
    /// Hash of the mining block, when reported.
    pub block_hash: Option<B256>,
    /// Confirmations at the moment terminality was decided.
    pub confirmations: u64,
    /// Gas units consumed.
    pub gas_used: u64,
    /// Facilitator gas fee actually paid, in wei.
    pub fee_wei: u128,
    /// EVM execution fee (`gasUsed * effectiveGasPrice`), in wei.
    pub execution_fee_wei: u128,
    /// Base L1 data fee reported by the receipt, in wei.
    pub l1_fee_wei: u128,
}

impl fmt::Debug for EvmTerminalOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EvmTerminalOutcome")
            .field("success", &self.success)
            .field("tx_hash", &"<redacted>")
            .field("block_number", &self.block_number)
            .field("block_hash", &"<redacted>")
            .field("confirmations", &self.confirmations)
            .field("gas_used", &self.gas_used)
            .field("fee_wei", &self.fee_wei)
            .field("execution_fee_wei", &self.execution_fee_wei)
            .field("l1_fee_wei", &self.l1_fee_wei)
            .finish()
    }
}

/// The receipt facts the confirmation-depth policy needs, extracted from a mined
/// transaction receipt.
#[derive(Clone, Copy, Eq, PartialEq)]
struct ReceiptFacts {
    block_number: u64,
    block_hash: Option<B256>,
    success: bool,
    tx_hash: B256,
    gas_used: u64,
    fee_wei: u128,
    execution_fee_wei: u128,
    l1_fee_wei: u128,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ReceiptObservation {
    Unknown,
    Pending,
    Mined(ReceiptFacts),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TransactionObservation {
    Missing,
    Pending {
        account_nonce: u64,
    },
    Mined {
        account_nonce: u64,
        block_number: u64,
        block_hash: B256,
    },
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct EndpointHead {
    chain_id: u64,
    block_number: u64,
    pending_account_nonce: u64,
    gas_balance_wei: u128,
}

fn merge_receipts(
    primary: ReceiptObservation,
    backup: ReceiptObservation,
) -> Result<ReceiptObservation, EvmRpcError> {
    match (primary, backup) {
        (ReceiptObservation::Unknown, ReceiptObservation::Unknown) => {
            Ok(ReceiptObservation::Unknown)
        }
        (ReceiptObservation::Mined(primary), ReceiptObservation::Mined(backup))
            if primary == backup =>
        {
            Ok(ReceiptObservation::Mined(primary))
        }
        (ReceiptObservation::Mined(_), ReceiptObservation::Mined(_)) => {
            Err(EvmRpcError::ReaderDisagreement("receipt facts"))
        }
        _ => Ok(ReceiptObservation::Pending),
    }
}

fn merge_transactions(
    primary: TransactionObservation,
    backup: TransactionObservation,
) -> Result<TransactionObservation, EvmRpcError> {
    match (primary, backup) {
        (TransactionObservation::Missing, TransactionObservation::Missing) => {
            Ok(TransactionObservation::Missing)
        }
        (TransactionObservation::Missing, known) | (known, TransactionObservation::Missing) => {
            Ok(known)
        }
        (
            TransactionObservation::Pending {
                account_nonce: primary_nonce,
            },
            TransactionObservation::Pending {
                account_nonce: backup_nonce,
            },
        ) if primary_nonce == backup_nonce => Ok(TransactionObservation::Pending {
            account_nonce: primary_nonce,
        }),
        (
            TransactionObservation::Pending {
                account_nonce: pending_nonce,
            },
            TransactionObservation::Mined { account_nonce, .. },
        )
        | (
            TransactionObservation::Mined { account_nonce, .. },
            TransactionObservation::Pending {
                account_nonce: pending_nonce,
            },
        ) if pending_nonce == account_nonce => {
            Ok(TransactionObservation::Pending { account_nonce })
        }
        (
            TransactionObservation::Mined {
                account_nonce: primary_nonce,
                block_number: primary_block,
                block_hash: primary_hash,
            },
            TransactionObservation::Mined {
                account_nonce: backup_nonce,
                block_number: backup_block,
                block_hash: backup_hash,
            },
        ) if primary_nonce == backup_nonce
            && primary_block == backup_block
            && primary_hash == backup_hash =>
        {
            Ok(primary)
        }
        _ => Err(EvmRpcError::ReaderDisagreement("transaction facts")),
    }
}

fn merge_head_values(
    expected_chain_id: u64,
    signer_address: Address,
    primary: EndpointHead,
    backup: EndpointHead,
) -> Result<EvmHead, EvmHeadSnapshotFailure> {
    if primary.chain_id != expected_chain_id || backup.chain_id != expected_chain_id {
        return Err(EvmHeadSnapshotFailure::ChainIdMismatch);
    }
    if primary.pending_account_nonce != backup.pending_account_nonce {
        return Err(EvmHeadSnapshotFailure::PendingNonceDisagreement);
    }
    Ok(EvmHead {
        block_number: primary.block_number.min(backup.block_number),
        account_nonce: primary.pending_account_nonce,
        gas_balance_wei: primary.gas_balance_wei.min(backup.gas_balance_wei),
        signer_address,
    })
}

fn merge_head_snapshot(
    expected_chain_id: u64,
    signer_address: Address,
    primary: Result<EndpointHead, EvmRpcError>,
    backup: Result<EndpointHead, EvmRpcError>,
) -> Result<EvmHead, EvmHeadSnapshotFailure> {
    match (primary, backup) {
        (Err(_), Err(_)) => Err(EvmHeadSnapshotFailure::BothRpcUnavailable),
        (Err(_), Ok(_)) => Err(EvmHeadSnapshotFailure::PrimaryRpcUnavailable),
        (Ok(_), Err(_)) => Err(EvmHeadSnapshotFailure::BackupRpcUnavailable),
        (Ok(primary), Ok(backup)) => {
            merge_head_values(expected_chain_id, signer_address, primary, backup)
        }
    }
}

fn merge_head_snapshot_with_diagnostics(
    expected_chain_id: u64,
    signer_address: Address,
    primary: Result<EndpointHead, EvmReadinessEndpointError>,
    backup: Result<EndpointHead, EvmReadinessEndpointError>,
) -> Result<EvmHead, EvmHeadSnapshotFailure> {
    match (&primary, &backup) {
        (Err(primary), Err(backup)) => {
            log_evm_readiness_dependency_failure(EvmReadinessReader::Primary, primary);
            log_evm_readiness_dependency_failure(EvmReadinessReader::Backup, backup);
        }
        (Err(primary), Ok(_)) => {
            log_evm_readiness_dependency_failure(EvmReadinessReader::Primary, primary);
        }
        (Ok(_), Err(backup)) => {
            log_evm_readiness_dependency_failure(EvmReadinessReader::Backup, backup);
        }
        (Ok(_), Ok(_)) => {}
    }
    merge_head_snapshot(
        expected_chain_id,
        signer_address,
        primary.map_err(|_| EvmRpcError::Rpc),
        backup.map_err(|_| EvmRpcError::Rpc),
    )
}

fn log_evm_readiness_dependency_failure(
    reader: EvmReadinessReader,
    failure: &EvmReadinessEndpointError,
) {
    tracing::warn!(
        event = "chain_readiness_dependency_failure",
        chain_family = "eip155",
        component = "head",
        reader = reader.as_str(),
        operation = failure.operation.as_str(),
        dependency_error = failure.error.as_str(),
        http_status = failure.http_status.unwrap_or(0),
        json_rpc_code = failure.json_rpc_code.unwrap_or(0)
    );
}

fn classify_transport_error(
    error: &TransportError,
) -> (EvmReadinessDependencyError, Option<u16>, Option<i64>) {
    match error {
        RpcError::ErrorResp(payload) => (
            EvmReadinessDependencyError::JsonRpcError,
            None,
            Some(payload.code),
        ),
        RpcError::NullResp => (EvmReadinessDependencyError::NullResponse, None, None),
        RpcError::UnsupportedFeature(_) => {
            (EvmReadinessDependencyError::UnsupportedFeature, None, None)
        }
        RpcError::LocalUsageError(_) => (EvmReadinessDependencyError::LocalUsage, None, None),
        RpcError::SerError(_) => (EvmReadinessDependencyError::Serialization, None, None),
        RpcError::DeserError { .. } => (EvmReadinessDependencyError::Deserialization, None, None),
        RpcError::Transport(kind) => classify_transport_kind(kind),
    }
}

fn classify_transport_kind(
    kind: &TransportErrorKind,
) -> (EvmReadinessDependencyError, Option<u16>, Option<i64>) {
    match kind {
        TransportErrorKind::HttpError(error) => {
            let classified = match error.status {
                429 => EvmReadinessDependencyError::HttpRateLimited,
                503 => EvmReadinessDependencyError::HttpTemporarilyUnavailable,
                400..=499 => EvmReadinessDependencyError::HttpClientError,
                500..=599 => EvmReadinessDependencyError::HttpServerError,
                _ => EvmReadinessDependencyError::HttpStatus,
            };
            (classified, Some(error.status), None)
        }
        TransportErrorKind::MissingBatchResponse(_) => (
            EvmReadinessDependencyError::MissingBatchResponse,
            None,
            None,
        ),
        TransportErrorKind::BackendGone => (EvmReadinessDependencyError::BackendGone, None, None),
        TransportErrorKind::PubsubUnavailable => {
            (EvmReadinessDependencyError::PubsubUnavailable, None, None)
        }
        TransportErrorKind::Custom(_) => (EvmReadinessDependencyError::CustomTransport, None, None),
        TransportErrorKind::NonRetryable(_) => (
            EvmReadinessDependencyError::NonRetryableTransport,
            None,
            None,
        ),
        _ => (EvmReadinessDependencyError::Transport, None, None),
    }
}

fn evm_readiness_endpoint_error(
    operation: EvmReadinessOperation,
    error: &TransportError,
) -> EvmReadinessEndpointError {
    let (error, http_status, json_rpc_code) = classify_transport_error(error);
    EvmReadinessEndpointError {
        operation,
        error,
        http_status,
        json_rpc_code,
    }
}

fn bounded_fee_envelope(
    gas_limit: u64,
    cap: u128,
    primary_max_fee: u128,
    primary_priority_fee: u128,
    backup_max_fee: u128,
    backup_priority_fee: u128,
) -> Result<EvmFeeEnvelope, EvmPrepareError> {
    if primary_max_fee == 0
        || backup_max_fee == 0
        || primary_priority_fee > primary_max_fee
        || backup_priority_fee > backup_max_fee
    {
        return Err(EvmPrepareError::InvalidFeeEstimate);
    }
    let max_fee_per_gas = primary_max_fee.max(backup_max_fee);
    if max_fee_per_gas > cap {
        return Err(EvmPrepareError::FeeCapExceeded);
    }
    Ok(EvmFeeEnvelope {
        gas_limit,
        max_fee_per_gas,
        max_priority_fee_per_gas: primary_priority_fee.max(backup_priority_fee),
    })
}

/// Apply the confirmation-depth policy: an outcome is terminal (and reorg-safe)
/// only at or beyond `required_confirmations`; otherwise it is still `Mined`.
/// Pure — the "receipt vanished" (reorg) case is handled by the caller as
/// `Unknown`, keeping the submission live for rebroadcast.
fn classify_confirmations(
    receipt: &ReceiptFacts,
    head_block: u64,
    required_confirmations: u64,
) -> Result<EvmReconcileStatus, EvmRpcError> {
    if required_confirmations == 0 {
        return Err(EvmRpcError::InvalidConfirmations);
    }
    let confirmations = head_block
        .checked_sub(receipt.block_number)
        .ok_or(EvmRpcError::HeadBehindReceipt)?
        .saturating_add(1);
    if confirmations >= required_confirmations {
        Ok(EvmReconcileStatus::Terminal(EvmTerminalOutcome {
            success: receipt.success,
            tx_hash: receipt.tx_hash,
            block_number: receipt.block_number,
            block_hash: receipt.block_hash,
            confirmations,
            gas_used: receipt.gas_used,
            fee_wei: receipt.fee_wei,
            execution_fee_wei: receipt.execution_fee_wei,
            l1_fee_wei: receipt.l1_fee_wei,
        }))
    } else {
        Ok(EvmReconcileStatus::Mined {
            confirmations,
            block_number: receipt.block_number,
        })
    }
}

fn classify_reconciliation(
    first_receipt: ReceiptObservation,
    transaction: TransactionObservation,
    rechecked_receipt: Option<ReceiptObservation>,
    head_block: u64,
    required_confirmations: u64,
) -> Result<EvmReconcileStatus, EvmRpcError> {
    match first_receipt {
        ReceiptObservation::Unknown => match transaction {
            TransactionObservation::Missing => Ok(EvmReconcileStatus::Unknown),
            TransactionObservation::Pending { .. } | TransactionObservation::Mined { .. } => {
                Ok(EvmReconcileStatus::Pending)
            }
        },
        ReceiptObservation::Pending => Ok(EvmReconcileStatus::Pending),
        ReceiptObservation::Mined(first) => match rechecked_receipt {
            Some(ReceiptObservation::Mined(second)) if first == second => {
                classify_confirmations(&second, head_block, required_confirmations)
            }
            _ => Ok(EvmReconcileStatus::Pending),
        },
    }
}

fn parse_hex_quantity(value: &serde_json::Value) -> Result<u128, EvmRpcError> {
    let raw = value
        .as_str()
        .ok_or(EvmRpcError::InvalidReceipt("non-string quantity"))?;
    let digits = raw
        .strip_prefix("0x")
        .ok_or(EvmRpcError::InvalidReceipt("quantity prefix"))?;
    if digits.is_empty() || (digits.len() > 1 && digits.starts_with('0')) {
        return Err(EvmRpcError::InvalidReceipt("non-canonical quantity"));
    }
    u128::from_str_radix(digits, 16).map_err(|_| EvmRpcError::FeeOverflow)
}

fn parse_raw_receipt(
    expected_hash: B256,
    value: Option<serde_json::Value>,
) -> Result<ReceiptObservation, EvmRpcError> {
    let Some(value) = value else {
        return Ok(ReceiptObservation::Unknown);
    };
    let object = value
        .as_object()
        .ok_or(EvmRpcError::InvalidReceipt("receipt object"))?;
    let tx_hash = object
        .get("transactionHash")
        .and_then(serde_json::Value::as_str)
        .ok_or(EvmRpcError::InvalidReceipt("transactionHash"))?
        .parse::<B256>()
        .map_err(|_| EvmRpcError::InvalidReceipt("transactionHash"))?;
    if tx_hash != expected_hash {
        return Err(EvmRpcError::InvalidReceipt("transactionHash mismatch"));
    }
    let Some(block_number_value) = object.get("blockNumber") else {
        return Err(EvmRpcError::InvalidReceipt("blockNumber"));
    };
    if block_number_value.is_null() {
        return Ok(ReceiptObservation::Pending);
    }
    let block_number = u64::try_from(parse_hex_quantity(block_number_value)?)
        .map_err(|_| EvmRpcError::InvalidReceipt("blockNumber overflow"))?;
    let block_hash = object
        .get("blockHash")
        .and_then(serde_json::Value::as_str)
        .ok_or(EvmRpcError::InvalidReceipt("blockHash"))?
        .parse::<B256>()
        .map_err(|_| EvmRpcError::InvalidReceipt("blockHash"))?;
    let status = match parse_hex_quantity(
        object
            .get("status")
            .ok_or(EvmRpcError::InvalidReceipt("status"))?,
    )? {
        0 => false,
        1 => true,
        _ => return Err(EvmRpcError::InvalidReceipt("status value")),
    };
    let gas_used = u64::try_from(parse_hex_quantity(
        object
            .get("gasUsed")
            .ok_or(EvmRpcError::InvalidReceipt("gasUsed"))?,
    )?)
    .map_err(|_| EvmRpcError::InvalidReceipt("gasUsed overflow"))?;
    let effective_gas_price = parse_hex_quantity(
        object
            .get("effectiveGasPrice")
            .ok_or(EvmRpcError::InvalidReceipt("effectiveGasPrice"))?,
    )?;
    let l1_fee_wei = parse_hex_quantity(
        object
            .get("l1Fee")
            .ok_or(EvmRpcError::InvalidReceipt("l1Fee"))?,
    )?;
    let execution_fee_wei = u128::from(gas_used)
        .checked_mul(effective_gas_price)
        .ok_or(EvmRpcError::FeeOverflow)?;
    let fee_wei = execution_fee_wei
        .checked_add(l1_fee_wei)
        .ok_or(EvmRpcError::FeeOverflow)?;
    Ok(ReceiptObservation::Mined(ReceiptFacts {
        block_number,
        block_hash: Some(block_hash),
        success: status,
        tx_hash,
        gas_used,
        fee_wei,
        execution_fee_wei,
        l1_fee_wei,
    }))
}

fn parse_transaction_quantity(value: &serde_json::Value) -> Result<u128, EvmRpcError> {
    let raw = value
        .as_str()
        .ok_or(EvmRpcError::InvalidTransaction("non-string quantity"))?;
    let digits = raw
        .strip_prefix("0x")
        .ok_or(EvmRpcError::InvalidTransaction("quantity prefix"))?;
    if digits.is_empty() || (digits.len() > 1 && digits.starts_with('0')) {
        return Err(EvmRpcError::InvalidTransaction("non-canonical quantity"));
    }
    u128::from_str_radix(digits, 16)
        .map_err(|_| EvmRpcError::InvalidTransaction("quantity overflow"))
}

fn parse_raw_transaction(
    expected_hash: B256,
    expected_chain_id: u64,
    expected_signer: Address,
    expected_token: Address,
    value: Option<serde_json::Value>,
) -> Result<TransactionObservation, EvmRpcError> {
    let Some(value) = value else {
        return Ok(TransactionObservation::Missing);
    };
    let object = value
        .as_object()
        .ok_or(EvmRpcError::InvalidTransaction("transaction object"))?;
    let tx_hash = object
        .get("hash")
        .and_then(serde_json::Value::as_str)
        .ok_or(EvmRpcError::InvalidTransaction("hash"))?
        .parse::<B256>()
        .map_err(|_| EvmRpcError::InvalidTransaction("hash"))?;
    if tx_hash != expected_hash {
        return Err(EvmRpcError::InvalidTransaction("hash mismatch"));
    }
    let chain_id = u64::try_from(parse_transaction_quantity(
        object
            .get("chainId")
            .ok_or(EvmRpcError::InvalidTransaction("chainId"))?,
    )?)
    .map_err(|_| EvmRpcError::InvalidTransaction("chainId overflow"))?;
    if chain_id != expected_chain_id {
        return Err(EvmRpcError::InvalidTransaction("chainId mismatch"));
    }
    let transaction_type = parse_transaction_quantity(
        object
            .get("type")
            .ok_or(EvmRpcError::InvalidTransaction("type"))?,
    )?;
    if transaction_type != 2 {
        return Err(EvmRpcError::InvalidTransaction("transaction type"));
    }
    let from = object
        .get("from")
        .and_then(serde_json::Value::as_str)
        .ok_or(EvmRpcError::InvalidTransaction("from"))?
        .parse::<Address>()
        .map_err(|_| EvmRpcError::InvalidTransaction("from"))?;
    if from != expected_signer {
        return Err(EvmRpcError::InvalidTransaction("from mismatch"));
    }
    let to = object
        .get("to")
        .and_then(serde_json::Value::as_str)
        .ok_or(EvmRpcError::InvalidTransaction("to"))?
        .parse::<Address>()
        .map_err(|_| EvmRpcError::InvalidTransaction("to"))?;
    if to != expected_token {
        return Err(EvmRpcError::InvalidTransaction("to mismatch"));
    }
    if parse_transaction_quantity(
        object
            .get("value")
            .ok_or(EvmRpcError::InvalidTransaction("value"))?,
    )? != 0
    {
        return Err(EvmRpcError::InvalidTransaction("native value"));
    }
    let input = object
        .get("input")
        .and_then(serde_json::Value::as_str)
        .ok_or(EvmRpcError::InvalidTransaction("input"))?;
    if input.len() <= 2 || !input.starts_with("0x") {
        return Err(EvmRpcError::InvalidTransaction("input"));
    }
    let account_nonce = u64::try_from(parse_transaction_quantity(
        object
            .get("nonce")
            .ok_or(EvmRpcError::InvalidTransaction("nonce"))?,
    )?)
    .map_err(|_| EvmRpcError::InvalidTransaction("nonce overflow"))?;
    let block_number = object
        .get("blockNumber")
        .ok_or(EvmRpcError::InvalidTransaction("blockNumber"))?;
    let block_hash = object
        .get("blockHash")
        .ok_or(EvmRpcError::InvalidTransaction("blockHash"))?;
    match (block_number.is_null(), block_hash.is_null()) {
        (true, true) => Ok(TransactionObservation::Pending { account_nonce }),
        (false, false) => {
            let block_number = u64::try_from(parse_transaction_quantity(block_number)?)
                .map_err(|_| EvmRpcError::InvalidTransaction("blockNumber overflow"))?;
            let block_hash = block_hash
                .as_str()
                .ok_or(EvmRpcError::InvalidTransaction("blockHash"))?
                .parse::<B256>()
                .map_err(|_| EvmRpcError::InvalidTransaction("blockHash"))?;
            Ok(TransactionObservation::Mined {
                account_nonce,
                block_number,
                block_hash,
            })
        }
        _ => Err(EvmRpcError::InvalidTransaction(
            "inconsistent block identity",
        )),
    }
}

/// The live EVM settlement provider.
pub struct EvmChainProvider {
    upstream: x402_chain_eip155::chain::Eip155ChainProvider,
    primary_reader: DynProvider,
    backup_reader: DynProvider,
    signer: PrivateKeySigner,
    chain_id: u64,
    asset: Address,
    required_confirmations: u64,
    gas_limit: u64,
    max_fee_per_gas: u128,
    transfer_domain_name: &'static str,
}

impl fmt::Debug for EvmChainProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EvmChainProvider")
            .field("upstream", &"<redacted>")
            .field("primary_reader", &"<redacted>")
            .field("backup_reader", &"<redacted>")
            .field("signer", &"<redacted>")
            .field("chain_id", &self.chain_id)
            .field("asset", &self.asset)
            .field("required_confirmations", &self.required_confirmations)
            .field("gas_limit", &self.gas_limit)
            .field("max_fee_per_gas", &self.max_fee_per_gas)
            .field("transfer_domain_name", &self.transfer_domain_name)
            .finish()
    }
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
        gas_limit: u64,
        max_fee_per_gas: u128,
    ) -> Result<Self, EvmConnectError> {
        if rpc_urls.len() < 2 || rpc_urls[0] == rpc_urls[1] {
            return Err(EvmConnectError::Config(
                "distinct primary and backup RPC URLs are required",
            ));
        }
        if required_confirmations == 0 || gas_limit == 0 || max_fee_per_gas == 0 {
            return Err(EvmConnectError::Config(
                "confirmations, gas limit, and max fee per gas must be positive",
            ));
        }
        let transfer_domain_name =
            canonical_domain_name(chain_id).ok_or(EvmConnectError::Config(
                "durable Circle USDC settlement supports Base mainnet and Base Sepolia only",
            ))?;
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
            .map_err(|_| EvmConnectError::Config("upstream EVM configuration was rejected"))?;
        let config = Eip155ChainConfig {
            chain_reference: Eip155ChainReference::new(chain_id),
            inner,
        };
        let upstream = <x402_chain_eip155::chain::Eip155ChainProvider as FromConfig<
            Eip155ChainConfig,
        >>::from_config(&config)
        .await
        .map_err(|_| EvmConnectError::Connect)?;
        let primary_reader = ProviderBuilder::new()
            .connect_http(rpc_urls[0].clone())
            .erased();
        let backup_reader = ProviderBuilder::new()
            .connect_http(rpc_urls[1].clone())
            .erased();
        Ok(Self {
            upstream,
            primary_reader,
            backup_reader,
            signer,
            chain_id,
            asset,
            required_confirmations,
            gas_limit,
            max_fee_per_gas,
            transfer_domain_name,
        })
    }

    /// Connect from configuration primitives: parse the secp256k1 signer key (a
    /// mode-0600 credential) and the USDC asset `0x` address, then
    /// [`Self::connect`]. Keeps alloy types out of the binary. On a signer-key
    /// parse failure the key material — including its length or shape — is never
    /// placed in the returned error.
    ///
    /// # Errors
    ///
    /// Returns [`EvmConnectError::Config`] if the signer key or asset address is
    /// malformed, or an upstream error from [`Self::connect`].
    pub async fn connect_from_config(
        chain_id: u64,
        rpc_urls: &[Url],
        signer_key: &str,
        asset: &str,
        required_confirmations: u64,
        gas_limit: u64,
        max_fee_per_gas: u128,
    ) -> Result<Self, EvmConnectError> {
        let signer = signer_key
            .parse::<PrivateKeySigner>()
            .map_err(|_| EvmConnectError::Config("signer key is not a valid secp256k1 hex key"))?;
        let asset = asset
            .parse::<Address>()
            .map_err(|_| EvmConnectError::Config("asset is not a valid 0x address"))?;
        Self::connect(
            chain_id,
            rpc_urls,
            signer,
            asset,
            required_confirmations,
            gas_limit,
            max_fee_per_gas,
        )
        .await
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

    /// The gas cap fixed into every settlement transaction. Must exceed the
    /// settle path's actual gas (EOA `transferWithAuthorization` ~70k; a deployed
    /// EIP-1271 wallet up to ~200k). Over-provisioning is free — gas is billed by
    /// usage, not by the cap — provided `gas_limit * max_fee_per_gas` stays within
    /// the signer's balance.
    #[must_use]
    pub fn gas_limit(&self) -> u64 {
        self.gas_limit
    }

    /// Absolute EIP-1559 fee cap enforced both before signing and during
    /// durable-byte validation.
    #[must_use]
    pub const fn max_fee_per_gas(&self) -> u128 {
        self.max_fee_per_gas
    }

    /// The chain-specific Circle USDC EIP-712 domain used by verification and
    /// durable recovery.
    #[must_use]
    pub fn transfer_domain(&self) -> alloy_sol_types::Eip712Domain {
        build_transfer_domain(self.transfer_domain_name, "2", self.chain_id, self.asset)
    }

    /// The chain-specific Circle USDC EIP-712 token name.
    #[must_use]
    pub const fn transfer_domain_name(&self) -> &'static str {
        self.transfer_domain_name
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
            .map_err(|_| EvmVerifyRejection::definitive("invalid_format"))?;
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
        validate_token_domain(
            self.transfer_domain_name,
            &payload.accepted.extra.name,
            &payload.accepted.extra.version,
        )?;
        let authorization = Erc3009Authorization {
            from: payload.payload.authorization.from,
            to: payload.payload.authorization.to,
            value: payload.payload.authorization.value,
            valid_after: U256::from(payload.payload.authorization.valid_after.as_secs()),
            valid_before: U256::from(payload.payload.authorization.valid_before.as_secs()),
            nonce: payload.payload.authorization.nonce,
        };
        let domain = self.transfer_domain();
        // Reject malformed and counterfactual signature envelopes before any
        // RPC. Otherwise upstream may return a reason-less Invalid response,
        // collapsing a stable public rejection into generic `invalid_payment`.
        // EOA and opaque EIP-1271 shapes continue to upstream's authoritative
        // balance/simulation/signature verification.
        let payment_hash =
            classify_signature_before_rpc(&authorization, &payload.payload.signature, &domain)?;

        // Upstream's authoritative decision (domain, balance, simulation).
        // Ambiguous transport failures (public endpoints 429 under burst) get
        // the bounded retry; definitive rejections return immediately.
        let response = retry_transient(
            || verify_eip3009_payment(&self.upstream, &payload, &requirements),
            |error| classify_verify_error(error).rpc_ambiguous,
        )
        .await
        .map_err(|error| classify_verify_error(&error))?;
        let payer = match response {
            VerifyResponse::Valid { payer } => payer,
            VerifyResponse::Invalid { .. } => {
                return Err(EvmVerifyRejection::definitive("invalid_payment"));
            }
        };
        let payer = payer
            .parse::<Address>()
            .map_err(|_| EvmVerifyRejection::definitive("invalid_payer_address"))?;

        if payer != authorization.from {
            return Err(EvmVerifyRejection::definitive("payer_mismatch"));
        }
        Ok(EvmVerifiedPayment {
            payer,
            payment_hash,
            asset,
            pay_to: Address::from(payload.accepted.pay_to),
            amount: payload.accepted.amount,
            authorization,
            signature: payload.payload.signature,
        })
    }

    /// Compute the ERC-3009 EIP-712 transfer hash offline (no RPC) from a raw
    /// verify/settle request. This is the payment's idempotency identity; the
    /// settle path needs it before the authoritative on-chain [`Self::verify`].
    /// It is byte-identical to the `payment_hash` `verify` returns for the same
    /// payment, so the downstream consistency check holds for eip155 exactly as
    /// it does for NEAR's decoded delegate hash.
    ///
    /// # Errors
    ///
    /// Returns [`EvmVerifyRejection`] if the request is not a well-formed eip155
    /// exact (ERC-3009) payment or its asset does not match this instance.
    pub fn offline_payment_hash(
        &self,
        request: &proto::VerifyRequest,
    ) -> Result<[u8; 32], EvmVerifyRejection> {
        let parsed = FacilitatorVerifyRequest::try_from(request.clone())
            .map_err(|_| EvmVerifyRejection::definitive("invalid_format"))?;
        let payload = match parsed {
            FacilitatorVerifyRequest::Eip3009 {
                payment_payload, ..
            } => payment_payload,
            FacilitatorVerifyRequest::Permit2 { .. } => {
                return Err(EvmVerifyRejection::definitive("unsupported_permit2_scheme"));
            }
        };
        let asset = Address::from(payload.accepted.asset);
        if asset != self.asset {
            return Err(EvmVerifyRejection::definitive("asset_mismatch"));
        }
        validate_token_domain(
            self.transfer_domain_name,
            &payload.accepted.extra.name,
            &payload.accepted.extra.version,
        )?;
        let authorization = Erc3009Authorization {
            from: payload.payload.authorization.from,
            to: payload.payload.authorization.to,
            value: payload.payload.authorization.value,
            valid_after: U256::from(payload.payload.authorization.valid_after.as_secs()),
            valid_before: U256::from(payload.payload.authorization.valid_before.as_secs()),
            nonce: payload.payload.authorization.nonce,
        };
        let domain = self.transfer_domain();
        Ok(eip712_transfer_hash(&authorization, &domain).0)
    }

    /// Snapshot the signer's next account (transaction) nonce.
    ///
    /// # Errors
    ///
    /// Returns [`EvmRpcError`] if the nonce lookup fails.
    pub async fn account_nonce(&self) -> Result<u64, EvmRpcError> {
        self.pending_account_nonce().await
    }

    /// Read the facilitator's pending nonce independently from both configured
    /// endpoints. A disagreement is indeterminate and therefore rejected.
    ///
    /// # Errors
    ///
    /// Returns [`EvmRpcError`] if either endpoint is unavailable, reports the
    /// wrong chain, or disagrees about the pending nonce.
    pub async fn pending_account_nonce(&self) -> Result<u64, EvmRpcError> {
        Ok(self.head().await?.account_nonce)
    }

    /// The signer's native-gas (ETH) balance in wei. Clamped into `u128`, which
    /// holds any realistic balance.
    ///
    /// # Errors
    ///
    /// Returns [`EvmRpcError`] if the balance lookup fails.
    pub async fn gas_balance_wei(&self) -> Result<u128, EvmRpcError> {
        Ok(self.head().await?.gas_balance_wei)
    }

    /// A one-shot snapshot of the signer's account nonce and gas balance at the
    /// current block — the EVM signer head the settlement engine journals and
    /// gates on.
    ///
    /// # Errors
    ///
    /// Returns [`EvmRpcError`] if any of the block/nonce/balance lookups fail.
    pub async fn head(&self) -> Result<EvmHead, EvmRpcError> {
        let (primary, backup) = tokio::join!(
            retry_transient(|| self.endpoint_head_once(&self.primary_reader), |_| true),
            retry_transient(|| self.endpoint_head_once(&self.backup_reader), |_| true),
        );
        merge_head_snapshot(self.chain_id, self.signer.address(), primary, backup)
            .map_err(EvmHeadSnapshotFailure::into_rpc_error)
    }

    /// Read one conservative dual-reader snapshot for readiness.
    ///
    /// Unlike [`Self::head`], this preserves only a closed, secret-free failure
    /// class for the readiness boundary. It never changes the provider's
    /// fail-closed behavior or exposes provider URLs, response bodies, signer
    /// data, or observed chain values.
    ///
    /// # Errors
    ///
    /// Returns [`EvmHeadSnapshotFailure`] when either reader is unavailable,
    /// reports the wrong chain identity, or disagrees about the pending nonce.
    pub async fn readiness_head(&self) -> Result<EvmHead, EvmHeadSnapshotFailure> {
        let (primary, backup) = tokio::join!(
            retry_transient(
                || self.readiness_endpoint_head_once(&self.primary_reader),
                |_| true
            ),
            retry_transient(
                || self.readiness_endpoint_head_once(&self.backup_reader),
                |_| true
            ),
        );
        merge_head_snapshot_with_diagnostics(self.chain_id, self.signer.address(), primary, backup)
    }

    async fn endpoint_head_once(
        &self,
        provider: &DynProvider,
    ) -> Result<EndpointHead, EvmRpcError> {
        let address = self.signer.address();
        let (chain_id, block_number, pending_account_nonce, balance) = tokio::try_join!(
            async { provider.get_chain_id().await.map_err(|_| EvmRpcError::Rpc) },
            async {
                provider
                    .get_block_number()
                    .await
                    .map_err(|_| EvmRpcError::Rpc)
            },
            async {
                provider
                    .get_transaction_count(address)
                    .pending()
                    .await
                    .map_err(|_| EvmRpcError::Rpc)
            },
            async {
                provider
                    .get_balance(address)
                    .await
                    .map_err(|_| EvmRpcError::Rpc)
            },
        )?;
        Ok(EndpointHead {
            chain_id,
            block_number,
            pending_account_nonce,
            gas_balance_wei: u128::try_from(balance).unwrap_or(u128::MAX),
        })
    }

    async fn readiness_endpoint_head_once(
        &self,
        provider: &DynProvider,
    ) -> Result<EndpointHead, EvmReadinessEndpointError> {
        let address = self.signer.address();
        let chain_id = provider.get_chain_id().await.map_err(|error| {
            evm_readiness_endpoint_error(EvmReadinessOperation::ChainId, &error)
        })?;
        let block_number = provider.get_block_number().await.map_err(|error| {
            evm_readiness_endpoint_error(EvmReadinessOperation::BlockNumber, &error)
        })?;
        let pending_account_nonce = provider
            .get_transaction_count(address)
            .pending()
            .await
            .map_err(|error| {
                evm_readiness_endpoint_error(EvmReadinessOperation::PendingNonce, &error)
            })?;
        let balance = provider.get_balance(address).await.map_err(|error| {
            evm_readiness_endpoint_error(EvmReadinessOperation::GasBalance, &error)
        })?;
        Ok(EndpointHead {
            chain_id,
            block_number,
            pending_account_nonce,
            gas_balance_wei: u128::try_from(balance).unwrap_or(u128::MAX),
        })
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
            .map_err(|_| EvmRpcError::Rpc)?;
        Ok(())
    }

    /// Prepare a durable, signed settlement transaction for a verified payment:
    /// encode the ERC-3009 call (choosing the overload from the payer signature),
    /// pin the given account nonce (from the journaled head) and the current fee
    /// market (RPC), and sign offline. The returned [`EvmPrepared`] is journaled
    /// and broadcast; it is never re-signed.
    ///
    /// The fee envelope is fixed into the signed transaction — see the
    /// fee-immutability note in `docs/evm-v2-design.md`.
    ///
    /// # Errors
    ///
    /// Returns [`EvmPrepareError`] if the signature is unsupported, an RPC fails,
    /// or signing fails.
    pub async fn prepare(
        &self,
        payment: &EvmVerifiedPayment,
        account_nonce: u64,
    ) -> Result<EvmPrepared, EvmPrepareError> {
        let calldata = settlement_calldata(
            &payment.authorization,
            payment.signature(),
            &payment.payment_hash,
        )?;
        let fees = self.fee_envelope().await?;
        let head = EvmSignerHead {
            chain_id: self.chain_id,
            account_nonce,
        };
        let mut prepared =
            sign_settlement_transaction(&self.signer, head, fees, self.asset, calldata)
                .map_err(EvmPrepareError::Sign)?;
        let l1_fee = self.estimate_l1_fee_wei(prepared.signed_tx_rlp()).await?;
        prepared.set_estimated_l1_fee_wei(l1_fee);
        Ok(prepared)
    }

    /// Snapshot the current EIP-1559 fee market and pair it with the configured
    /// gas cap. Priced with alloy's estimator (which carries base-fee headroom);
    /// the cap is immutable once signed.
    async fn fee_envelope(&self) -> Result<EvmFeeEnvelope, EvmPrepareError> {
        let (primary, backup) = tokio::join!(
            retry_transient(
                || async {
                    self.primary_reader
                        .estimate_eip1559_fees()
                        .await
                        .map_err(|_| EvmRpcError::Rpc)
                },
                |_| true,
            ),
            retry_transient(
                || async {
                    self.backup_reader
                        .estimate_eip1559_fees()
                        .await
                        .map_err(|_| EvmRpcError::Rpc)
                },
                |_| true,
            ),
        );
        let primary = primary?;
        let backup = backup?;
        bounded_fee_envelope(
            self.gas_limit,
            self.max_fee_per_gas,
            primary.max_fee_per_gas,
            primary.max_priority_fee_per_gas,
            backup.max_fee_per_gas,
            backup.max_priority_fee_per_gas,
        )
    }

    /// Estimate Base's L1 data fee over the exact fully signed EIP-2718 bytes.
    /// Both configured readers must return a value; the larger one is retained.
    ///
    /// # Errors
    ///
    /// Returns [`EvmRpcError`] if either oracle call fails or its response does
    /// not contain a supported integer fee.
    pub async fn estimate_l1_fee_wei(&self, signed_tx_rlp: &[u8]) -> Result<u128, EvmRpcError> {
        let (primary, backup) = tokio::join!(
            retry_transient(
                || Self::estimate_l1_fee_once(&self.primary_reader, signed_tx_rlp),
                |_| true,
            ),
            retry_transient(
                || Self::estimate_l1_fee_once(&self.backup_reader, signed_tx_rlp),
                |_| true,
            ),
        );
        Ok(primary?.max(backup?))
    }

    async fn estimate_l1_fee_once(
        provider: &DynProvider,
        signed_tx_rlp: &[u8],
    ) -> Result<u128, EvmRpcError> {
        let call = base_abi::getL1FeeCall {
            transaction: Bytes::copy_from_slice(signed_tx_rlp),
        };
        let request = TransactionRequest::default()
            .to(BASE_GAS_PRICE_ORACLE)
            .input(TransactionInput::new(call.abi_encode().into()));
        let response = provider.call(request).await.map_err(|_| EvmRpcError::Rpc)?;
        let decoded = base_abi::getL1FeeCall::abi_decode_returns_validate(&response)
            .map_err(|_| EvmRpcError::InvalidReceipt("GasPriceOracle response"))?;
        u128::try_from(decoded).map_err(|_| EvmRpcError::FeeOverflow)
    }

    /// Reconcile a submitted transaction against the chain. A transaction known
    /// by hash but not yet receipted is `Pending`; `Unknown` requires both
    /// independent endpoints to lack both receipt and transaction. A terminal
    /// outcome requires matching receipts before and after the conservative head
    /// read, at or beyond the required confirmation depth.
    ///
    /// # Errors
    ///
    /// Returns [`EvmRpcError`] if the receipt or head lookups fail.
    pub async fn reconcile(&self, tx_hash: B256) -> Result<EvmReconcileStatus, EvmRpcError> {
        self.reconcile_with_confirmations(tx_hash, self.required_confirmations)
            .await
    }

    /// Reconcile using the confirmation depth stored with this settlement,
    /// rather than silently applying the process's current default.
    ///
    /// # Errors
    ///
    /// Returns [`EvmRpcError`] when the depth is zero, an endpoint is
    /// unavailable, the readers conflict, or a Base receipt is malformed.
    pub async fn reconcile_with_confirmations(
        &self,
        tx_hash: B256,
        required_confirmations: u64,
    ) -> Result<EvmReconcileStatus, EvmRpcError> {
        if required_confirmations == 0 {
            return Err(EvmRpcError::InvalidConfirmations);
        }
        let (primary_receipt, backup_receipt, primary_transaction, backup_transaction) = tokio::join!(
            retry_transient(
                || Self::receipt_once(&self.primary_reader, tx_hash),
                |_| true,
            ),
            retry_transient(
                || Self::receipt_once(&self.backup_reader, tx_hash),
                |_| true,
            ),
            retry_transient(
                || self.transaction_once(&self.primary_reader, tx_hash),
                |_| true,
            ),
            retry_transient(
                || self.transaction_once(&self.backup_reader, tx_hash),
                |_| true,
            ),
        );
        let first_receipt = merge_receipts(primary_receipt?, backup_receipt?)?;
        let transaction = merge_transactions(primary_transaction?, backup_transaction?)?;
        let head_block = self.safe_block_number().await?;
        let rechecked_receipt = if matches!(first_receipt, ReceiptObservation::Mined(_)) {
            let (primary, backup) = tokio::join!(
                retry_transient(
                    || Self::receipt_once(&self.primary_reader, tx_hash),
                    |_| true,
                ),
                retry_transient(
                    || Self::receipt_once(&self.backup_reader, tx_hash),
                    |_| true,
                ),
            );
            Some(merge_receipts(primary?, backup?)?)
        } else {
            None
        };
        classify_reconciliation(
            first_receipt,
            transaction,
            rechecked_receipt,
            head_block,
            required_confirmations,
        )
    }

    async fn receipt_once(
        provider: &DynProvider,
        tx_hash: B256,
    ) -> Result<ReceiptObservation, EvmRpcError> {
        let value = provider
            .client()
            .request::<_, Option<serde_json::Value>>("eth_getTransactionReceipt", (tx_hash,))
            .await
            .map_err(|_| EvmRpcError::Rpc)?;
        parse_raw_receipt(tx_hash, value)
    }

    async fn transaction_once(
        &self,
        provider: &DynProvider,
        tx_hash: B256,
    ) -> Result<TransactionObservation, EvmRpcError> {
        let value = provider
            .client()
            .request::<_, Option<serde_json::Value>>("eth_getTransactionByHash", (tx_hash,))
            .await
            .map_err(|_| EvmRpcError::Rpc)?;
        parse_raw_transaction(
            tx_hash,
            self.chain_id,
            self.signer.address(),
            self.asset,
            value,
        )
    }

    async fn safe_block_number(&self) -> Result<u64, EvmRpcError> {
        let (primary, backup) = tokio::join!(
            retry_transient(|| Self::block_head_once(&self.primary_reader), |_| true),
            retry_transient(|| Self::block_head_once(&self.backup_reader), |_| true),
        );
        let (primary_chain, primary_block) = primary?;
        let (backup_chain, backup_block) = backup?;
        if primary_chain != self.chain_id || backup_chain != self.chain_id {
            return Err(EvmRpcError::ReaderDisagreement("chain id"));
        }
        Ok(primary_block.min(backup_block))
    }

    async fn block_head_once(provider: &DynProvider) -> Result<(u64, u64), EvmRpcError> {
        tokio::try_join!(
            async { provider.get_chain_id().await.map_err(|_| EvmRpcError::Rpc) },
            async {
                provider
                    .get_block_number()
                    .await
                    .map_err(|_| EvmRpcError::Rpc)
            },
        )
    }

    /// Reconcile by a hex transaction-hash string — the neutral form the journal
    /// stores — so callers need not handle EVM hash types. A malformed hash is a
    /// journal corruption and surfaces as an [`EvmRpcError`].
    ///
    /// # Errors
    ///
    /// Returns [`EvmRpcError`] if the hash is malformed or a lookup fails.
    pub async fn reconcile_hash(&self, tx_hash: &str) -> Result<EvmReconcileStatus, EvmRpcError> {
        self.reconcile_hash_with_confirmations(tx_hash, self.required_confirmations)
            .await
    }

    /// Reconcile a journal hash with that row's immutable confirmation policy.
    ///
    /// # Errors
    ///
    /// Returns [`EvmRpcError`] if the hash is malformed or a lookup fails.
    pub async fn reconcile_hash_with_confirmations(
        &self,
        tx_hash: &str,
        required_confirmations: u64,
    ) -> Result<EvmReconcileStatus, EvmRpcError> {
        let hash = tx_hash
            .parse::<B256>()
            .map_err(|_| EvmRpcError::InvalidTransactionHash)?;
        self.reconcile_with_confirmations(hash, required_confirmations)
            .await
    }

    /// Probe that the connected RPC reports the expected chain id and a live head.
    /// The chain-liveness half of readiness.
    pub async fn readiness_probe(&self) -> bool {
        self.head().await.is_ok()
    }

    /// The CAIP-2 chain id, e.g. `eip155:84532`.
    #[must_use]
    pub fn caip2(&self) -> ChainId {
        ChainId::new("eip155", self.chain_id.to_string())
    }
}

/// Backoff schedule between read-only RPC retry attempts (initial attempt plus
/// one retry per entry). Public endpoints rate-limit under burst — a paid flow
/// fans out several calls — and without retries one throttled call surfaces as
/// a client-facing 503 through the engine's fail-closed ambiguity handling
/// (2026-07-26 incident). Two short retries absorb burst throttling without
/// masking a real outage.
const RPC_RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(300), Duration::from_millis(900)];

/// Bounded retry for read-only RPC operations. Retries only while `transient`
/// classifies the error as retryable; definitive results and rejections return
/// immediately. Broadcast is deliberately never routed through this helper —
/// submission recovery belongs to the journaled reconcile loop, which
/// rebroadcasts exact stored bytes.
async fn retry_transient<T, E, Fut>(
    mut call: impl FnMut() -> Fut,
    transient: impl Fn(&E) -> bool,
) -> Result<T, E>
where
    Fut: Future<Output = Result<T, E>>,
{
    let mut result = call().await;
    for delay in RPC_RETRY_DELAYS {
        match &result {
            Err(error) if transient(error) => {
                tokio::time::sleep(delay).await;
                result = call().await;
            }
            _ => break,
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, hex};
    use alloy_sol_types::SolValue;
    use x402_types::proto::PaymentVerificationError;

    #[test]
    fn circle_usdc_domain_is_pinned_per_base_chain() {
        assert_eq!(canonical_domain_name(8_453), Some("USD Coin"));
        assert_eq!(canonical_domain_name(84_532), Some("USDC"));
        assert_eq!(canonical_domain_name(1), None);

        assert!(validate_token_domain("USD Coin", "USD Coin", "2").is_ok());
        assert!(matches!(
            validate_token_domain("USDC", "USD Coin", "2"),
            Err(EvmVerifyRejection { ref reason, .. }) if reason == "invalid_token_domain"
        ));
        assert!(matches!(
            validate_token_domain("USD Coin", "USD Coin", "1"),
            Err(EvmVerifyRejection { ref reason, .. }) if reason == "invalid_token_domain"
        ));
    }

    fn signature_test_authorization() -> Erc3009Authorization {
        Erc3009Authorization {
            from: address!("0x1111111111111111111111111111111111111111"),
            to: address!("0x2222222222222222222222222222222222222222"),
            value: U256::from(1_000_u64),
            valid_after: U256::ZERO,
            valid_before: U256::from(1_u64),
            nonce: B256::repeat_byte(0x42),
        }
    }

    #[test]
    fn verified_payment_debug_redacts_every_bearer_and_payment_identifier() {
        let authorization = signature_test_authorization();
        let payment = EvmVerifiedPayment {
            payer: authorization.from,
            payment_hash: B256::repeat_byte(0x43),
            asset: address!("0x4444444444444444444444444444444444444444"),
            pay_to: authorization.to,
            amount: authorization.value,
            authorization,
            signature: Bytes::from(vec![0x45_u8; 65]),
        };

        let debug = format!("{payment:?}");
        for sentinel in [
            "0x1111111111111111111111111111111111111111",
            "0x2222222222222222222222222222222222222222",
            "0x4444444444444444444444444444444444444444",
            &B256::repeat_byte(0x43).to_string(),
            &Bytes::from(vec![0x45_u8; 65]).to_string(),
            "1000",
        ] {
            assert!(
                !debug.to_ascii_lowercase().contains(sentinel),
                "sensitive value escaped Debug redaction: {debug}"
            );
        }
        assert_eq!(debug.matches("<redacted>").count(), 7);
    }

    #[test]
    fn malformed_eip6492_is_invalid_signature_before_rpc() {
        let authorization = signature_test_authorization();
        let domain = build_transfer_domain(
            "USDC",
            "2",
            84_532,
            address!("0x036CbD53842c5426634e7929541eC2318f3dCF7e"),
        );
        let mut malformed = vec![0_u8; 40];
        malformed.extend_from_slice(&hex!(
            "6492649264926492649264926492649264926492649264926492649264926492"
        ));
        assert!(matches!(
            classify_signature_before_rpc(&authorization, &Bytes::from(malformed), &domain),
            Err(EvmVerifyRejection {
                ref reason,
                rpc_ambiguous: false
            }) if reason == "invalid_signature"
        ));
    }

    #[test]
    fn well_formed_eip6492_is_unsupported_before_rpc() {
        let authorization = signature_test_authorization();
        let domain = build_transfer_domain(
            "USDC",
            "2",
            84_532,
            address!("0x036CbD53842c5426634e7929541eC2318f3dCF7e"),
        );
        let mut wrapper = (
            address!("0x3333333333333333333333333333333333333333"),
            Bytes::from(vec![0x12_u8; 48]),
            Bytes::from(vec![0x34_u8; 80]),
        )
            .abi_encode_params();
        wrapper.extend_from_slice(&hex!(
            "6492649264926492649264926492649264926492649264926492649264926492"
        ));
        assert!(matches!(
            classify_signature_before_rpc(&authorization, &Bytes::from(wrapper), &domain),
            Err(EvmVerifyRejection {
                ref reason,
                rpc_ambiguous: false
            }) if reason == "unsupported_eip6492"
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn transient_errors_retry_until_success() {
        let attempts = std::cell::Cell::new(0_u32);
        let result: Result<u32, &str> = retry_transient(
            || {
                attempts.set(attempts.get() + 1);
                let attempt = attempts.get();
                async move {
                    if attempt < 3 {
                        Err("throttled")
                    } else {
                        Ok(attempt)
                    }
                }
            },
            |_| true,
        )
        .await;
        assert_eq!(result, Ok(3));
        assert_eq!(attempts.get(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn definitive_errors_never_retry() {
        let attempts = std::cell::Cell::new(0_u32);
        let result: Result<u32, &str> = retry_transient(
            || {
                attempts.set(attempts.get() + 1);
                async { Err("invalid_signature") }
            },
            |_| false,
        )
        .await;
        assert_eq!(result, Err("invalid_signature"));
        assert_eq!(attempts.get(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn transient_retries_are_bounded_and_return_the_last_error() {
        let attempts = std::cell::Cell::new(0_usize);
        let result: Result<u32, &str> = retry_transient(
            || {
                attempts.set(attempts.get() + 1);
                async { Err("throttled") }
            },
            |_| true,
        )
        .await;
        assert_eq!(result, Err("throttled"));
        assert_eq!(attempts.get(), 1 + RPC_RETRY_DELAYS.len());
    }

    #[test]
    fn onchain_failure_is_ambiguous_verification_error_is_definitive() {
        let sentinel = "0xsensitive-calldata-sentinel";
        let onchain = X402SchemeFacilitatorError::OnchainFailure(sentinel.to_owned());
        let classified = classify_verify_error(&onchain);
        assert!(classified.rpc_ambiguous);
        assert_eq!(classified.reason, "onchain_failure");
        assert!(!format!("{classified:?}").contains(sentinel));
        assert!(!format!("{:?}", EvmPrepareError::Rpc(EvmRpcError::Rpc)).contains(sentinel));

        let verification =
            X402SchemeFacilitatorError::PaymentVerification(PaymentVerificationError::Expired);
        let classified = classify_verify_error(&verification);
        assert!(!classified.rpc_ambiguous);
        assert_eq!(classified.reason, "authorization_expired");

        let signature = X402SchemeFacilitatorError::PaymentVerification(
            PaymentVerificationError::InvalidSignature("dynamic upstream detail".to_owned()),
        );
        assert_eq!(
            classify_verify_error(&signature).reason,
            "invalid_signature"
        );
    }

    fn facts(success: bool) -> ReceiptFacts {
        ReceiptFacts {
            block_number: 100,
            block_hash: Some(B256::repeat_byte(0x01)),
            success,
            tx_hash: B256::repeat_byte(0x02),
            gas_used: 70_000,
            fee_wei: 140_000_000_000,
            execution_fee_wei: 120_000_000_000,
            l1_fee_wei: 20_000_000_000,
        }
    }

    #[test]
    fn terminal_only_at_or_beyond_required_confirmations() {
        // head 104, mined at 100 -> exactly 5 confirmations, required 5 -> terminal.
        assert!(matches!(
            classify_confirmations(&facts(true), 104, 5),
            Ok(EvmReconcileStatus::Terminal(ref outcome))
                if outcome.confirmations == 5 && outcome.success && outcome.block_number == 100
        ));
        // head 103 -> 4 confirmations < 5 -> still mined, not terminal.
        assert!(matches!(
            classify_confirmations(&facts(true), 103, 5),
            Ok(EvmReconcileStatus::Mined {
                confirmations: 4,
                block_number: 100
            })
        ));
    }

    #[test]
    fn a_confirmed_revert_is_terminal_failure_not_retried() {
        // A reverted transaction, once deep enough, is a definitive terminal
        // failure — never retried into a fresh submission.
        assert!(matches!(
            classify_confirmations(&facts(false), 200, 5),
            Ok(EvmReconcileStatus::Terminal(ref outcome)) if !outcome.success
        ));
    }

    #[test]
    fn dual_receipt_merge_waits_on_presence_difference_and_rejects_fact_conflict() {
        let mined = ReceiptObservation::Mined(facts(true));
        assert!(matches!(
            merge_receipts(mined, mined),
            Ok(ReceiptObservation::Mined(_))
        ));
        assert!(matches!(
            merge_receipts(mined, ReceiptObservation::Unknown),
            Ok(ReceiptObservation::Pending)
        ));
        let mut different = facts(true);
        different.block_number += 1;
        assert!(matches!(
            merge_receipts(mined, ReceiptObservation::Mined(different)),
            Err(EvmRpcError::ReaderDisagreement("receipt facts"))
        ));
    }

    #[test]
    fn receipt_missing_but_exact_transaction_known_is_pending()
    -> Result<(), Box<dyn std::error::Error>> {
        let hash = B256::repeat_byte(0x51);
        let signer = Address::repeat_byte(0x52);
        let token = Address::repeat_byte(0x53);
        let raw = serde_json::json!({
            "hash": hash.to_string(),
            "chainId": "0x14a34",
            "type": "0x2",
            "from": signer.to_string(),
            "to": token.to_string(),
            "value": "0x0",
            "input": "0x12345678",
            "nonce": "0x7",
            "blockNumber": null,
            "blockHash": null
        });
        let mut mismatched = raw.clone();
        mismatched["hash"] = serde_json::Value::String(B256::repeat_byte(0x54).to_string());
        assert!(matches!(
            parse_raw_transaction(hash, 84_532, signer, token, Some(mismatched)),
            Err(EvmRpcError::InvalidTransaction("hash mismatch"))
        ));
        let transaction = parse_raw_transaction(hash, 84_532, signer, token, Some(raw))?;
        assert!(matches!(
            transaction,
            TransactionObservation::Pending { account_nonce: 7 }
        ));
        assert!(matches!(
            classify_reconciliation(ReceiptObservation::Unknown, transaction, None, 100, 5),
            Ok(EvmReconcileStatus::Pending)
        ));
        assert!(matches!(
            classify_reconciliation(
                ReceiptObservation::Unknown,
                TransactionObservation::Missing,
                None,
                100,
                5
            ),
            Ok(EvmReconcileStatus::Unknown)
        ));
        Ok(())
    }

    #[test]
    fn transaction_reader_conflicts_fail_closed() {
        let primary = TransactionObservation::Mined {
            account_nonce: 7,
            block_number: 100,
            block_hash: B256::repeat_byte(0x61),
        };
        let different_block = TransactionObservation::Mined {
            account_nonce: 7,
            block_number: 101,
            block_hash: B256::repeat_byte(0x61),
        };
        assert!(matches!(
            merge_transactions(primary, different_block),
            Err(EvmRpcError::ReaderDisagreement("transaction facts"))
        ));
        let different_nonce = TransactionObservation::Pending { account_nonce: 8 };
        assert!(matches!(
            merge_transactions(primary, different_nonce),
            Err(EvmRpcError::ReaderDisagreement("transaction facts"))
        ));
    }

    #[test]
    fn receipt_disappearing_after_head_read_cannot_be_terminal() {
        let first = ReceiptObservation::Mined(facts(true));
        let transaction = TransactionObservation::Mined {
            account_nonce: 7,
            block_number: 100,
            block_hash: B256::repeat_byte(0x01),
        };
        assert!(matches!(
            classify_reconciliation(
                first,
                transaction,
                Some(ReceiptObservation::Unknown),
                200,
                5
            ),
            Ok(EvmReconcileStatus::Pending)
        ));
        assert!(matches!(
            classify_reconciliation(first, transaction, Some(first), 104, 5),
            Ok(EvmReconcileStatus::Terminal(_))
        ));
    }

    #[test]
    fn dual_head_merge_uses_safe_minimum_and_requires_pending_nonce_agreement()
    -> Result<(), Box<dyn std::error::Error>> {
        let primary = EndpointHead {
            chain_id: 84_532,
            block_number: 105,
            pending_account_nonce: 7,
            gas_balance_wei: 50,
        };
        let backup = EndpointHead {
            block_number: 103,
            gas_balance_wei: 40,
            ..primary
        };
        let merged = merge_head_values(84_532, Address::repeat_byte(0x11), primary, backup)?;
        assert_eq!(merged.block_number, 103);
        assert_eq!(merged.gas_balance_wei, 40);

        let divergent = EndpointHead {
            pending_account_nonce: 8,
            ..backup
        };
        assert!(matches!(
            merge_head_values(84_532, Address::repeat_byte(0x11), primary, divergent),
            Err(EvmHeadSnapshotFailure::PendingNonceDisagreement)
        ));
        Ok(())
    }

    #[test]
    fn readiness_head_snapshot_failure_classes_are_bounded_and_preserve_head_semantics() {
        let cases = [
            (
                EvmHeadSnapshotFailure::PrimaryRpcUnavailable,
                "primary_rpc_unavailable",
            ),
            (
                EvmHeadSnapshotFailure::BackupRpcUnavailable,
                "backup_rpc_unavailable",
            ),
            (
                EvmHeadSnapshotFailure::BothRpcUnavailable,
                "both_rpc_unavailable",
            ),
            (EvmHeadSnapshotFailure::ChainIdMismatch, "chain_id_mismatch"),
            (
                EvmHeadSnapshotFailure::PendingNonceDisagreement,
                "pending_nonce_disagreement",
            ),
        ];

        for (failure, code) in cases {
            assert_eq!(failure.as_str(), code);
            match failure.into_rpc_error() {
                EvmRpcError::Rpc => {
                    assert!(matches!(
                        failure,
                        EvmHeadSnapshotFailure::PrimaryRpcUnavailable
                            | EvmHeadSnapshotFailure::BackupRpcUnavailable
                            | EvmHeadSnapshotFailure::BothRpcUnavailable
                    ));
                }
                EvmRpcError::ReaderDisagreement("chain id") => {
                    assert_eq!(failure, EvmHeadSnapshotFailure::ChainIdMismatch);
                }
                EvmRpcError::ReaderDisagreement("pending account nonce") => {
                    assert_eq!(failure, EvmHeadSnapshotFailure::PendingNonceDisagreement);
                }
                _ => std::process::abort(),
            }
        }
    }

    #[test]
    fn readiness_dependency_errors_are_bounded_without_provider_text() {
        let sentinel = "https://credentialed-rpc.invalid/path?token=never-log-this";
        let rate_limited = TransportErrorKind::http_error(429, sentinel.to_owned());
        let unavailable = TransportErrorKind::http_error(503, sentinel.to_owned());
        let custom = TransportErrorKind::custom_str(sentinel);
        let custom_for_debug = TransportErrorKind::custom_str(sentinel);

        assert_eq!(
            classify_transport_error(&rate_limited),
            (
                EvmReadinessDependencyError::HttpRateLimited,
                Some(429),
                None
            )
        );
        assert_eq!(
            classify_transport_error(&unavailable),
            (
                EvmReadinessDependencyError::HttpTemporarilyUnavailable,
                Some(503),
                None
            )
        );
        assert_eq!(
            evm_readiness_endpoint_error(EvmReadinessOperation::ChainId, &custom),
            EvmReadinessEndpointError {
                operation: EvmReadinessOperation::ChainId,
                error: EvmReadinessDependencyError::CustomTransport,
                http_status: None,
                json_rpc_code: None,
            }
        );

        let diagnostic = format!(
            "{:?}",
            evm_readiness_endpoint_error(EvmReadinessOperation::ChainId, &custom_for_debug)
        );
        assert!(!diagnostic.contains(sentinel));
        assert!(!diagnostic.contains("credentialed-rpc"));
    }

    #[test]
    fn readiness_head_snapshot_distinguishes_reader_availability_without_identity_data() {
        let endpoint = EndpointHead {
            chain_id: 84_532,
            block_number: 100,
            pending_account_nonce: 7,
            gas_balance_wei: 50,
        };
        let signer = Address::repeat_byte(0x11);

        assert!(matches!(
            merge_head_snapshot(84_532, signer, Err(EvmRpcError::Rpc), Ok(endpoint)),
            Err(EvmHeadSnapshotFailure::PrimaryRpcUnavailable)
        ));
        assert!(matches!(
            merge_head_snapshot(84_532, signer, Ok(endpoint), Err(EvmRpcError::Rpc)),
            Err(EvmHeadSnapshotFailure::BackupRpcUnavailable)
        ));
        assert!(matches!(
            merge_head_snapshot(84_532, signer, Err(EvmRpcError::Rpc), Err(EvmRpcError::Rpc)),
            Err(EvmHeadSnapshotFailure::BothRpcUnavailable)
        ));

        let wrong_chain = EndpointHead {
            chain_id: 1,
            ..endpoint
        };
        assert!(matches!(
            merge_head_snapshot(84_532, signer, Ok(endpoint), Ok(wrong_chain)),
            Err(EvmHeadSnapshotFailure::ChainIdMismatch)
        ));

        // Reader availability takes precedence when the other reader cannot
        // supply a complete snapshot. This retains `head()`'s historical
        // fail-closed error ordering: it never treats one reader's value as
        // sufficient evidence for a dual-reader snapshot.
        assert!(matches!(
            merge_head_snapshot(84_532, signer, Ok(wrong_chain), Err(EvmRpcError::Rpc)),
            Err(EvmHeadSnapshotFailure::BackupRpcUnavailable)
        ));
        assert!(matches!(
            merge_head_snapshot(84_532, signer, Err(EvmRpcError::Rpc), Ok(wrong_chain)),
            Err(EvmHeadSnapshotFailure::PrimaryRpcUnavailable)
        ));
    }

    #[test]
    fn fee_estimator_is_bounded_by_absolute_cap() -> Result<(), Box<dyn std::error::Error>> {
        let fees = bounded_fee_envelope(120_000, 30, 20, 2, 25, 3)?;
        assert_eq!(fees.max_fee_per_gas, 25);
        assert_eq!(fees.max_priority_fee_per_gas, 3);
        assert!(matches!(
            bounded_fee_envelope(120_000, 24, 20, 2, 25, 3),
            Err(EvmPrepareError::FeeCapExceeded)
        ));
        assert!(matches!(
            bounded_fee_envelope(120_000, 30, 20, 21, 25, 3),
            Err(EvmPrepareError::InvalidFeeEstimate)
        ));
        Ok(())
    }

    #[test]
    fn base_receipt_fee_includes_execution_and_l1_components()
    -> Result<(), Box<dyn std::error::Error>> {
        let hash = B256::repeat_byte(0x42);
        let raw = serde_json::json!({
            "transactionHash": hash.to_string(),
            "blockNumber": "0x64",
            "blockHash": B256::repeat_byte(0x43).to_string(),
            "status": "0x1",
            "gasUsed": "0x10",
            "effectiveGasPrice": "0x20",
            "l1Fee": "0x30"
        });
        let observation = parse_raw_receipt(hash, Some(raw))?;
        let ReceiptObservation::Mined(parsed) = observation else {
            return Err("expected mined receipt".into());
        };
        assert_eq!(parsed.execution_fee_wei, 0x200);
        assert_eq!(parsed.l1_fee_wei, 0x30);
        assert_eq!(parsed.fee_wei, 0x230);
        Ok(())
    }

    #[test]
    fn generated_signer_file_is_private_and_round_trips() -> Result<(), Box<dyn std::error::Error>>
    {
        let path = std::env::temp_dir().join(format!("x402-evm-keygen-{}.key", std::process::id()));
        let _remove_stale = std::fs::remove_file(&path);
        let address = generate_signer_key_file(&path)?;

        // The printed value is a valid 0x address.
        assert!(address.parse::<Address>().is_ok());
        // The credential round-trips to the same signer address the caller was told.
        let contents = std::fs::read_to_string(&path)?;
        let signer = contents.trim().parse::<PrivateKeySigner>()?;
        assert_eq!(signer.address().to_string(), address);
        // The credential is owner-only (mode 0600).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path)?.permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // create_new refuses to clobber an existing credential.
        assert!(generate_signer_key_file(&path).is_err());

        let _remove = std::fs::remove_file(&path);
        Ok(())
    }
}
