use std::{fmt, sync::Arc};

use async_trait::async_trait;
use near_crypto::{PublicKey, Signature, Signer};
use near_primitives::{
    action::Action,
    hash::CryptoHash,
    transaction::{SignedTransaction, Transaction, TransactionV0},
    types::{AccountId, Nonce},
    views::AccessKeyPermissionView,
    views::AccountView,
};
use x402_types::{chain::ChainProviderOps, proto};

use crate::{
    mechanism::verify_proto_request,
    rpc::{NearRpc, NearRpcError, decode_signed_transaction},
    types::{
        NearNetwork, PreparedTransaction, TransactionLookup, VerificationFailure,
        VerificationPolicy, VerifiedPayment,
    },
};

pub trait NearRelayerSigner: Send + Sync {
    fn account_id(&self) -> AccountId;
    fn public_key(&self) -> PublicKey;
    fn sign(&self, bytes: &[u8]) -> Signature;
}

impl NearRelayerSigner for Signer {
    fn account_id(&self) -> AccountId {
        self.get_account_id()
    }

    fn public_key(&self) -> PublicKey {
        Signer::public_key(self)
    }

    fn sign(&self, bytes: &[u8]) -> Signature {
        Signer::sign(self, bytes)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct RelayerHead {
    pub block_height: u64,
    pub block_hash: CryptoHash,
    pub access_key_nonce: Nonce,
}

impl fmt::Debug for RelayerHead {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayerHead")
            .field("block_height", &self.block_height)
            .field("block_hash", &"<redacted>")
            .field("access_key_nonce", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct RelayerStatus {
    pub block_height: u64,
    pub block_hash: CryptoHash,
    pub access_key_nonce: Nonce,
    pub account: AccountView,
}

/// A bounded, secret-free reason why the dual-reader NEAR liveness probe
/// could not be used for readiness.
///
/// This deliberately carries neither provider identity nor any observed chain
/// value. Callers may expose its fixed [`Self::as_str`] code only through
/// protected telemetry and structured logs; public readiness remains a boolean
/// gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NearRpcReadinessFailure {
    /// The configured primary reader did not produce a usable liveness result.
    #[error("primary NEAR RPC readiness probe unavailable")]
    PrimaryRpcUnavailable,
    /// The configured backup reader did not produce a usable liveness result.
    #[error("backup NEAR RPC readiness probe unavailable")]
    BackupRpcUnavailable,
    /// Neither configured reader produced a usable liveness result.
    #[error("both NEAR RPC readiness probes unavailable")]
    BothRpcUnavailable,
    /// Both readers responded, but at least one reported the wrong NEAR chain.
    #[error("NEAR RPC chain identity did not match the configured chain")]
    ChainIdMismatch,
}

impl NearRpcReadinessFailure {
    /// Stable low-cardinality code for protected telemetry and structured logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PrimaryRpcUnavailable => "primary_rpc_unavailable",
            Self::BackupRpcUnavailable => "backup_rpc_unavailable",
            Self::BothRpcUnavailable => "both_rpc_unavailable",
            Self::ChainIdMismatch => "chain_id_mismatch",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NearReadinessReader {
    Primary,
    Backup,
}

impl NearReadinessReader {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Backup => "backup",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NearReadinessOperation {
    Status,
    FinalBlock,
}

impl NearReadinessOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::FinalBlock => "block_final",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NearReadinessDependencyError {
    Timeout,
    RpcRequest,
    InvalidResponse,
    AccountNotFound,
    AccessKeyNotFound,
    MethodNotFound,
    TransactionUnknown,
    TransactionRejected,
    TransactionTemporarilyRejected,
    InvalidSignedTransaction,
}

impl NearReadinessDependencyError {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::RpcRequest => "rpc_request",
            Self::InvalidResponse => "invalid_response",
            Self::AccountNotFound => "account_not_found",
            Self::AccessKeyNotFound => "access_key_not_found",
            Self::MethodNotFound => "method_not_found",
            Self::TransactionUnknown => "transaction_unknown",
            Self::TransactionRejected => "transaction_rejected",
            Self::TransactionTemporarilyRejected => "transaction_temporarily_rejected",
            Self::InvalidSignedTransaction => "invalid_signed_transaction",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NearReadinessEndpointError {
    operation: NearReadinessOperation,
    error: NearReadinessDependencyError,
}

impl fmt::Debug for RelayerStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayerStatus")
            .field("block_height", &self.block_height)
            .field("block_hash", &"<redacted>")
            .field("access_key_nonce", &"<redacted>")
            .field("account", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum SettlementDisposition {
    Succeeded {
        transaction: CryptoHash,
    },
    Failed {
        transaction: Option<CryptoHash>,
        reason: String,
        message: Option<String>,
    },
}

impl fmt::Debug for SettlementDisposition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Succeeded { .. } => formatter
                .debug_struct("SettlementDisposition::Succeeded")
                .field("transaction", &"<redacted>")
                .finish(),
            Self::Failed {
                transaction,
                reason: _,
                message,
            } => formatter
                .debug_struct("SettlementDisposition::Failed")
                .field("transaction", &transaction.as_ref().map(|_| "<redacted>"))
                .field("reason", &"<redacted>")
                .field("message", &message.as_ref().map(|_| "<redacted>"))
                .finish(),
        }
    }
}

#[async_trait]
pub trait NearSettlementCoordinator: Send + Sync {
    async fn settle(
        &self,
        provider: &NearChainProvider,
        payment: VerifiedPayment,
    ) -> Result<SettlementDisposition, NearRpcError>;
}

#[derive(Debug)]
struct SettlementDisabled;

#[async_trait]
impl NearSettlementCoordinator for SettlementDisabled {
    async fn settle(
        &self,
        _provider: &NearChainProvider,
        _payment: VerifiedPayment,
    ) -> Result<SettlementDisposition, NearRpcError> {
        Err(NearRpcError::Request(
            "durable settlement coordinator is not configured".to_owned(),
        ))
    }
}

#[derive(Clone)]
pub struct NearChainProvider {
    network: NearNetwork,
    rpc: Arc<dyn NearRpc>,
    backup_rpc: Option<Arc<dyn NearRpc>>,
    relayer: Arc<dyn NearRelayerSigner>,
    coordinator: Arc<dyn NearSettlementCoordinator>,
}

#[allow(clippy::missing_errors_doc)]
impl NearChainProvider {
    #[must_use]
    pub fn new(
        network: NearNetwork,
        rpc: Arc<dyn NearRpc>,
        relayer: Arc<dyn NearRelayerSigner>,
    ) -> Self {
        Self {
            network,
            rpc,
            backup_rpc: None,
            relayer,
            coordinator: Arc::new(SettlementDisabled),
        }
    }

    #[must_use]
    pub fn with_backup_rpc(mut self, backup_rpc: Arc<dyn NearRpc>) -> Self {
        self.backup_rpc = Some(backup_rpc);
        self
    }

    #[must_use]
    pub fn with_settlement_coordinator(
        mut self,
        coordinator: Arc<dyn NearSettlementCoordinator>,
    ) -> Self {
        self.coordinator = coordinator;
        self
    }

    #[must_use]
    pub const fn network(&self) -> NearNetwork {
        self.network
    }

    #[must_use]
    pub fn relayer_account_id(&self) -> AccountId {
        self.relayer.account_id()
    }

    #[must_use]
    pub fn relayer_public_key(&self) -> PublicKey {
        self.relayer.public_key()
    }

    pub async fn rpc_network_id(&self) -> Result<String, NearRpcError> {
        self.rpc.network_id().await
    }

    pub async fn backup_rpc_network_id(&self) -> Result<String, NearRpcError> {
        let backup = self
            .backup_rpc
            .as_ref()
            .ok_or(NearRpcError::InvalidResponse(
                "backup RPC is not configured",
            ))?;
        backup.network_id().await
    }

    pub async fn rpc_final_block(&self) -> Result<crate::rpc::FinalBlock, NearRpcError> {
        self.rpc.final_block().await
    }

    pub async fn backup_rpc_final_block(&self) -> Result<crate::rpc::FinalBlock, NearRpcError> {
        let backup = self
            .backup_rpc
            .as_ref()
            .ok_or(NearRpcError::InvalidResponse(
                "backup RPC is not configured",
            ))?;
        backup.final_block().await
    }

    /// Probe both configured RPC readers independently for the expected NEAR
    /// chain identity and a final block.
    ///
    /// All four read-only calls are allowed to complete even when one fails, so
    /// the result identifies the unavailable reader without trusting the other
    /// reader as sufficient evidence. Availability takes precedence over chain
    /// identity when only one reader returns a complete result, matching the
    /// EVM dual-reader readiness policy.
    ///
    /// # Errors
    ///
    /// Returns a fixed [`NearRpcReadinessFailure`] without provider URLs,
    /// response text, chain values, or credentials.
    pub async fn readiness_probe(&self) -> Result<(), NearRpcReadinessFailure> {
        let primary = probe_rpc_readiness(self.rpc.as_ref());
        let backup = async {
            let rpc = self.backup_rpc.as_ref().ok_or_else(|| {
                near_readiness_endpoint_error(
                    NearReadinessOperation::Status,
                    &NearRpcError::InvalidResponse("backup RPC is not configured"),
                )
            })?;
            probe_rpc_readiness(rpc.as_ref()).await
        };
        let (primary, backup) = tokio::join!(primary, backup);
        let expected = self.network.chain_id();
        classify_rpc_readiness(&expected.reference, &primary, &backup)
    }

    pub async fn verify(
        &self,
        request: &proto::VerifyRequest,
        policy: &VerificationPolicy,
    ) -> Result<VerifiedPayment, VerificationFailure> {
        verify_proto_request(self, request, policy).await
    }

    pub async fn relayer_head(&self) -> Result<RelayerHead, NearRpcError> {
        self.relayer_head_from(&self.rpc).await
    }

    pub async fn backup_relayer_head(&self) -> Result<RelayerHead, NearRpcError> {
        let backup = self
            .backup_rpc
            .as_ref()
            .ok_or(NearRpcError::InvalidResponse(
                "backup RPC is not configured",
            ))?;
        self.relayer_head_from(backup).await
    }

    pub async fn relayer_status(&self) -> Result<RelayerStatus, NearRpcError> {
        let block = self.rpc.final_block().await?;
        let account_id = self.relayer.account_id();
        let access_key = self
            .rpc
            .view_access_key(block.hash, account_id.clone(), self.relayer.public_key())
            .await?;
        if !matches!(access_key.permission, AccessKeyPermissionView::FullAccess) {
            return Err(NearRpcError::InvalidResponse(
                "relayer key is not full access",
            ));
        }
        let account = self.rpc.view_account(block.hash, account_id).await?;
        Ok(RelayerStatus {
            block_height: block.height,
            block_hash: block.hash,
            access_key_nonce: access_key.nonce,
            account,
        })
    }

    pub fn prepare_outer_transaction(
        &self,
        payment: &VerifiedPayment,
        relayer_head: RelayerHead,
    ) -> Result<PreparedTransaction, NearRpcError> {
        let relayer_nonce = relayer_head
            .access_key_nonce
            .checked_add(1)
            .ok_or(NearRpcError::InvalidResponse("relayer nonce overflow"))?;
        let signer_id = self.relayer.account_id();
        let signer_public_key = self.relayer.public_key();
        let transaction = Transaction::V0(TransactionV0 {
            signer_id: signer_id.clone(),
            public_key: signer_public_key.clone(),
            nonce: relayer_nonce,
            receiver_id: payment.payer.clone(),
            block_hash: relayer_head.block_hash,
            actions: vec![Action::Delegate(Box::new(
                payment.signed_delegate().clone(),
            ))],
        });
        let (transaction_hash, _) = transaction.get_hash_and_size();
        let signature = self.relayer.sign(transaction_hash.as_ref());
        let signed_transaction = SignedTransaction::new(signature, transaction);
        let signed_transaction_bytes = borsh::to_vec(&signed_transaction)
            .map_err(|_| NearRpcError::InvalidSignedTransaction)?;

        Ok(PreparedTransaction::new(
            transaction_hash,
            relayer_nonce,
            signer_id,
            signer_public_key,
            signed_transaction_bytes,
        ))
    }

    pub async fn broadcast_exact(
        &self,
        signed_transaction_bytes: &[u8],
    ) -> Result<TransactionLookup, NearRpcError> {
        let signed_transaction = decode_signed_transaction(signed_transaction_bytes)?;
        self.rpc.send_transaction_final(signed_transaction).await
    }

    pub async fn query_transaction(
        &self,
        transaction_hash: CryptoHash,
        signer_id: AccountId,
    ) -> Result<TransactionLookup, NearRpcError> {
        self.rpc
            .transaction_status_final(transaction_hash, signer_id)
            .await
    }

    pub async fn query_transaction_backup(
        &self,
        transaction_hash: CryptoHash,
        signer_id: AccountId,
    ) -> Result<TransactionLookup, NearRpcError> {
        let backup = self
            .backup_rpc
            .as_ref()
            .ok_or(NearRpcError::InvalidResponse(
                "backup RPC is not configured",
            ))?;
        backup
            .transaction_status_final(transaction_hash, signer_id)
            .await
    }

    async fn relayer_head_from(&self, rpc: &Arc<dyn NearRpc>) -> Result<RelayerHead, NearRpcError> {
        let block = rpc.final_block().await?;
        let access_key = rpc
            .view_access_key(
                block.hash,
                self.relayer.account_id(),
                self.relayer.public_key(),
            )
            .await?;
        if !matches!(access_key.permission, AccessKeyPermissionView::FullAccess) {
            return Err(NearRpcError::InvalidResponse(
                "relayer key is not full access",
            ));
        }
        Ok(RelayerHead {
            block_height: block.height,
            block_hash: block.hash,
            access_key_nonce: access_key.nonce,
        })
    }

    pub(crate) fn rpc(&self) -> &dyn NearRpc {
        self.rpc.as_ref()
    }

    pub(crate) async fn coordinate_settlement(
        &self,
        payment: VerifiedPayment,
    ) -> Result<SettlementDisposition, NearRpcError> {
        self.coordinator.settle(self, payment).await
    }
}

async fn probe_rpc_readiness(rpc: &dyn NearRpc) -> Result<String, NearReadinessEndpointError> {
    let (network_id, final_block) = tokio::join!(rpc.network_id(), rpc.final_block());
    let network_id = network_id
        .map_err(|error| near_readiness_endpoint_error(NearReadinessOperation::Status, &error))?;
    final_block.map_err(|error| {
        near_readiness_endpoint_error(NearReadinessOperation::FinalBlock, &error)
    })?;
    Ok(network_id)
}

fn classify_rpc_readiness(
    expected_network: &str,
    primary: &Result<String, NearReadinessEndpointError>,
    backup: &Result<String, NearReadinessEndpointError>,
) -> Result<(), NearRpcReadinessFailure> {
    match (primary, backup) {
        (Err(primary), Err(backup)) => {
            log_near_readiness_dependency_failure(NearReadinessReader::Primary, *primary);
            log_near_readiness_dependency_failure(NearReadinessReader::Backup, *backup);
            Err(NearRpcReadinessFailure::BothRpcUnavailable)
        }
        (Err(primary), Ok(_)) => {
            log_near_readiness_dependency_failure(NearReadinessReader::Primary, *primary);
            Err(NearRpcReadinessFailure::PrimaryRpcUnavailable)
        }
        (Ok(_), Err(backup)) => {
            log_near_readiness_dependency_failure(NearReadinessReader::Backup, *backup);
            Err(NearRpcReadinessFailure::BackupRpcUnavailable)
        }
        (Ok(primary), Ok(backup)) if primary == expected_network && backup == expected_network => {
            Ok(())
        }
        (Ok(_), Ok(_)) => Err(NearRpcReadinessFailure::ChainIdMismatch),
    }
}

fn log_near_readiness_dependency_failure(
    reader: NearReadinessReader,
    failure: NearReadinessEndpointError,
) {
    tracing::warn!(
        event = "chain_readiness_dependency_failure",
        chain_family = "near",
        component = "rpc",
        reader = reader.as_str(),
        operation = failure.operation.as_str(),
        dependency_error = failure.error.as_str()
    );
}

fn classify_near_readiness_error(error: &NearRpcError) -> NearReadinessDependencyError {
    match error {
        NearRpcError::AccountNotFound => NearReadinessDependencyError::AccountNotFound,
        NearRpcError::AccessKeyNotFound => NearReadinessDependencyError::AccessKeyNotFound,
        NearRpcError::MethodNotFound => NearReadinessDependencyError::MethodNotFound,
        NearRpcError::TransactionUnknown => NearReadinessDependencyError::TransactionUnknown,
        NearRpcError::TransactionRejected => NearReadinessDependencyError::TransactionRejected,
        NearRpcError::TransactionTemporarilyRejected => {
            NearReadinessDependencyError::TransactionTemporarilyRejected
        }
        NearRpcError::Timeout => NearReadinessDependencyError::Timeout,
        NearRpcError::InvalidResponse(_) => NearReadinessDependencyError::InvalidResponse,
        NearRpcError::InvalidSignedTransaction => {
            NearReadinessDependencyError::InvalidSignedTransaction
        }
        NearRpcError::Request(_) => NearReadinessDependencyError::RpcRequest,
    }
}

fn near_readiness_endpoint_error(
    operation: NearReadinessOperation,
    error: &NearRpcError,
) -> NearReadinessEndpointError {
    NearReadinessEndpointError {
        operation,
        error: classify_near_readiness_error(error),
    }
}

impl ChainProviderOps for NearChainProvider {
    fn signer_addresses(&self) -> Vec<String> {
        vec![self.relayer.account_id().to_string()]
    }

    fn chain_id(&self) -> x402_types::chain::ChainId {
        self.network.chain_id()
    }
}

impl fmt::Debug for NearChainProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NearChainProvider")
            .field("network", &self.network)
            .field("relayer_account_id", &"<redacted>")
            .field("relayer_public_key", &"<redacted>")
            .field("backup_rpc_configured", &self.backup_rpc.is_some())
            .finish_non_exhaustive()
    }
}
