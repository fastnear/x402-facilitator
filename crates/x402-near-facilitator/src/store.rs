use std::time::Duration;
use std::{borrow::Cow, fmt, str::FromStr};

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgRow};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::config::ChainKind;

const AUTHORIZATION_SCRUB_PENDING: &str = "x402-maintenance:0003-authorization-scrub:pending";
const AUTHORIZATION_SCRUB_COMPLETE: &str = "x402-maintenance:0003-authorization-scrub:complete";

fn embedded_migrator() -> sqlx::migrate::Migrator {
    let initial = sqlx::migrate::Migration::new(
        1,
        Cow::Borrowed("initial"),
        sqlx::migrate::MigrationType::Simple,
        Cow::Borrowed(include_str!("../../../migrations/0001_initial.sql")),
        false,
    );
    let multichain = sqlx::migrate::Migration::new(
        2,
        Cow::Borrowed("multichain settlement columns"),
        sqlx::migrate::MigrationType::Simple,
        Cow::Borrowed(include_str!("../../../migrations/0002_multichain.sql")),
        false,
    );
    let retry_anchors = sqlx::migrate::Migration::new(
        3,
        Cow::Borrowed("durable retry and settlement anchors"),
        sqlx::migrate::MigrationType::Simple,
        Cow::Borrowed(include_str!("../../../migrations/0003_retry_anchors.sql")),
        false,
    );
    sqlx::migrate::Migrator {
        migrations: Cow::Owned(vec![initial, multichain, retry_anchors]),
        ..sqlx::migrate::Migrator::DEFAULT
    }
}

#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct PgStore {
    pool: PgPool,
}

#[derive(Clone, Debug)]
pub struct ApiClient {
    pub id: Uuid,
    pub name: String,
    pub environment: String,
    pub daily_budget_yocto_near: String,
    pub verify_rate_per_minute: u32,
    pub settle_rate_per_minute: u32,
}

#[allow(missing_debug_implementations)]
pub struct ApiKeyCandidate {
    pub client: ApiClient,
    pub digest: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettlementState {
    Reserved,
    AwaitingRetry,
    Prepared,
    Submitted,
    Succeeded,
    Failed,
}

impl SettlementState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::AwaitingRetry => "awaiting_retry",
            Self::Prepared => "prepared",
            Self::Submitted => "submitted",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }
}

impl FromStr for SettlementState {
    type Err = StoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "reserved" => Ok(Self::Reserved),
            "awaiting_retry" => Ok(Self::AwaitingRetry),
            "prepared" => Ok(Self::Prepared),
            "submitted" => Ok(Self::Submitted),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            _ => Err(StoreError::Corrupt(format!(
                "unknown settlement state {value}"
            ))),
        }
    }
}

/// Non-sensitive ERC-3009 audit data retained after the full signed
/// authorization is scrubbed from the journal.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EvmAuthorizationMetadata {
    /// Canonical internal x402 wire version (always v2).
    pub version: u8,
    /// Inclusive ERC-3009 lower time bound as an exact decimal integer.
    pub valid_after: String,
    /// Exclusive ERC-3009 upper time bound as an exact decimal integer.
    pub valid_before: String,
}

impl fmt::Debug for EvmAuthorizationMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EvmAuthorizationMetadata")
            .field("version", &self.version)
            .field("validity_window", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl EvmAuthorizationMetadata {
    fn to_json(&self) -> Value {
        serde_json::json!({
            "version": self.version,
            "validAfter": self.valid_after,
            "validBefore": self.valid_before,
        })
    }
}

#[derive(Clone)]
pub struct SettlementRecord {
    pub id: Uuid,
    pub api_client_id: Uuid,
    pub payment_identifier: Option<String>,
    pub payment_hash: [u8; 32],
    pub request_fingerprint: [u8; 32],
    pub anchor_scope: String,
    pub anchor_value: [u8; 32],
    pub state: SettlementState,
    pub chain_kind: ChainKind,
    pub network: String,
    pub asset: String,
    pub pay_to: String,
    pub amount: String,
    pub payer: String,
    pub authorization_metadata: Option<EvmAuthorizationMetadata>,
    pub policy_snapshot: Value,
    pub delegate_public_key: String,
    pub delegate_nonce: String,
    pub delegate_max_block_height: String,
    pub reservation_date: NaiveDate,
    pub reserved_yocto_near: String,
    pub relayer_account_id: Option<String>,
    pub relayer_public_key: Option<String>,
    pub relayer_nonce: Option<String>,
    pub outer_transaction_bytes: Option<Vec<u8>>,
    pub outer_transaction_hash: Option<String>,
    // EVM (eip155) submission identity. NULL on NEAR rows; the EVM reconcile path
    // reads these instead of the NEAR relayer / outer-transaction columns. One DB
    // per instance means every row shares the instance's chain, so the provider
    // kind — not this per-row set — selects the reconcile path.
    pub signer_address: Option<String>,
    pub signer_account_nonce: Option<String>,
    pub submitted_tx_rlp: Option<Vec<u8>>,
    pub submitted_tx_hash: Option<String>,
    pub confirmations: Option<i32>,
    pub required_confirmations: Option<i32>,
    pub attempt_count: u32,
    pub retry_code: Option<String>,
    pub terminal_http_status: Option<u16>,
    pub terminal_response_bytes: Option<Vec<u8>>,
    pub created_at: DateTime<Utc>,
}

impl fmt::Debug for SettlementRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SettlementRecord")
            .field("state", &self.state)
            .field("chain_kind", &self.chain_kind)
            .field("network", &self.network)
            .field("payment", &"<redacted>")
            .field("submission", &"<redacted>")
            .field("attempt_count", &self.attempt_count)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct JournalSummary {
    pub reserved: u64,
    pub prepared: u64,
    pub submitted: u64,
    pub oldest_created_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct SponsorshipUsage {
    pub reserved_yocto_near: String,
    pub spent_yocto_near: String,
}

#[derive(Clone)]
pub struct NewSettlement {
    pub id: Uuid,
    pub api_client_id: Uuid,
    pub payment_identifier: Option<String>,
    pub payment_hash: [u8; 32],
    pub request_fingerprint: [u8; 32],
    /// Domain for the chain-enforced replay anchor.
    pub anchor_scope: String,
    /// Exact 32-byte NEAR delegate hash or ERC-3009 authorization nonce.
    pub anchor_value: [u8; 32],
    pub x402_version: u8,
    pub scheme: String,
    pub network: String,
    pub asset: String,
    pub pay_to: String,
    pub amount: String,
    pub payer: String,
    /// The settlement chain; selects which authorization columns are populated.
    pub chain_kind: ChainKind,
    // NEAR delegate identity: `Some` for NEAR, `None` for EVM (nullable since
    // migration 0002; the conditional CHECK re-requires it for NEAR rows).
    pub delegate_public_key: Option<String>,
    pub delegate_nonce: Option<String>,
    pub delegate_max_block_height: Option<String>,
    // Only the non-sensitive EVM authorization validity window remains in the
    // journal. The signed RLP is persisted later at prepare.
    pub authorization_metadata: Option<EvmAuthorizationMetadata>,
    pub signer_address: Option<String>,
    pub policy_snapshot: Value,
    pub reservation_yocto_near: String,
    pub global_daily_budget_yocto_near: String,
    pub client_daily_budget_yocto_near: String,
}

impl fmt::Debug for NewSettlement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewSettlement")
            .field("chain_kind", &self.chain_kind)
            .field("network", &self.network)
            .field("x402_version", &self.x402_version)
            .field("payment", &"<redacted>")
            .field("authorization", &"<redacted>")
            .field("policy", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Current policy and budget inputs used to reacquire a dormant settlement.
#[derive(Clone, Debug)]
pub struct RetryReservation {
    pub settlement_id: Uuid,
    pub policy_snapshot: Value,
    pub reservation_yocto_near: String,
    pub global_daily_budget_yocto_near: String,
    pub client_daily_budget_yocto_near: String,
}

#[derive(Clone)]
pub struct PreparedJournalEntry {
    pub settlement_id: Uuid,
    pub relayer_account_id: String,
    pub relayer_public_key: String,
    pub relayer_nonce: String,
    pub transaction_bytes: Vec<u8>,
    pub transaction_hash: String,
}

impl fmt::Debug for PreparedJournalEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedJournalEntry")
            .field("settlement", &"<redacted>")
            .field("relayer", &"<redacted>")
            .field("submission", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// The durable EVM submission journal written at prepare: the signed ERC-3009
/// transaction (RLP + hash), the account nonce it burns, and the confirmation
/// depth it must reach to be terminal. `signer_address` and authorization metadata
/// were written at reservation; this completes the eip155 non-terminal CHECK.
#[derive(Clone)]
pub struct EvmPreparedJournalEntry {
    pub settlement_id: Uuid,
    pub signer_account_nonce: String,
    pub submitted_tx_rlp: Vec<u8>,
    pub submitted_tx_hash: String,
    pub required_confirmations: i32,
}

impl fmt::Debug for EvmPreparedJournalEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EvmPreparedJournalEntry")
            .field("settlement", &"<redacted>")
            .field("submission", &"<redacted>")
            .field("required_confirmations", &self.required_confirmations)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct TerminalJournalEntry {
    pub settlement_id: Uuid,
    pub state: SettlementState,
    pub http_status: u16,
    pub response_bytes: Vec<u8>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    pub gas_burnt: Option<String>,
    pub tokens_burnt: Option<String>,
    pub actual_yocto_near: String,
    /// eip155 reorg-safety audit trail behind the confirmation-depth decision;
    /// all `None` for NEAR. `mined_block_number` is a decimal string for the
    /// `NUMERIC(20,0)` column.
    pub mined_block_number: Option<String>,
    pub mined_block_hash: Option<String>,
    pub confirmations: Option<i32>,
}

impl fmt::Debug for TerminalJournalEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalJournalEntry")
            .field("state", &self.state)
            .field("http_status", &self.http_status)
            .field("result", &"<redacted>")
            .field("confirmations", &self.confirmations)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub enum ClaimOutcome {
    New(SettlementRecord),
    Existing(SettlementRecord),
    IdentifierConflict,
    DuplicateSettlement,
    SettlementBusy,
    BudgetExceeded,
}

#[derive(Clone, Debug)]
pub enum RetryOutcome {
    Resumed(Box<SettlementRecord>),
    SettlementBusy,
    BudgetExceeded,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database operation failed")]
    Database(#[source] sqlx::Error),
    #[error("database migration failed")]
    Migration(#[source] sqlx::migrate::MigrateError),
    #[error("database state is inconsistent: {0}")]
    Corrupt(String),
    #[error("invalid database configuration: {0}")]
    Configuration(String),
    #[error("invalid journal input: {0}")]
    InvalidInput(String),
    #[error("invalid state transition from {from} to {to}")]
    Transition { from: String, to: String },
}

impl From<sqlx::Error> for StoreError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl PgStore {
    #[cfg(test)]
    pub(crate) fn from_explicit_test_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn connect(database_url: &str, max_connections: u32) -> Result<Self, StoreError> {
        let options = PgConnectOptions::from_str(database_url).map_err(|_| {
            StoreError::Configuration("database URL could not be parsed".to_owned())
        })?;
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .min_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .idle_timeout(Duration::from_secs(300))
            .after_connect(|connection, _metadata| {
                Box::pin(async move {
                    sqlx::query("SET TIME ZONE 'UTC'")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SET statement_timeout = '15s'")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SET lock_timeout = '5s'")
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<(), StoreError> {
        embedded_migrator()
            .run(&self.pool)
            .await
            .map_err(StoreError::Migration)?;
        self.complete_authorization_scrub().await
    }

    pub async fn ping(&self) -> Result<(), StoreError> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    pub async fn schema_compatible(&self) -> Result<bool, StoreError> {
        for migration in embedded_migrator().iter() {
            let checksum: Option<Vec<u8>> = sqlx::query_scalar(
                "SELECT checksum FROM _sqlx_migrations \
                 WHERE version = $1 AND success = true",
            )
            .bind(migration.version)
            .fetch_optional(&self.pool)
            .await?;
            if checksum.as_deref() != Some(migration.checksum.as_ref()) {
                return Ok(false);
            }
        }
        Ok(self.authorization_scrub_marker().await?.as_deref()
            == Some(AUTHORIZATION_SCRUB_COMPLETE))
    }

    async fn authorization_scrub_marker(&self) -> Result<Option<String>, StoreError> {
        sqlx::query_scalar("SELECT obj_description('settlements'::regclass, 'pg_class')")
            .fetch_one(&self.pool)
            .await
            .map_err(StoreError::Database)
    }

    async fn complete_authorization_scrub(&self) -> Result<(), StoreError> {
        match self.authorization_scrub_marker().await?.as_deref() {
            Some(AUTHORIZATION_SCRUB_COMPLETE) => return Ok(()),
            Some(AUTHORIZATION_SCRUB_PENDING) => {}
            _ => {
                return Err(StoreError::Corrupt(
                    "authorization-scrub maintenance marker is missing or invalid".to_owned(),
                ));
            }
        }

        // VACUUM cannot run in a transaction. Use one dedicated pooled
        // connection with no statement timeout, close it afterward so the
        // service's normal timeout is restored on the replacement connection,
        // and only then mark the rewrite complete. A crash before the comment
        // update safely repeats the rewrite on the next admin invocation.
        let mut connection = self.pool.acquire().await?;
        sqlx::query("SET statement_timeout = 0")
            .execute(&mut *connection)
            .await?;
        let rewrite = sqlx::query("VACUUM (FULL, ANALYZE) settlements")
            .execute(&mut *connection)
            .await;
        connection.close().await?;
        rewrite?;
        sqlx::query(
            "COMMENT ON TABLE settlements IS \
             'x402-maintenance:0003-authorization-scrub:complete'",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn active_clients_have_payee_policy(
        &self,
        network: &str,
        asset: &str,
    ) -> Result<bool, StoreError> {
        let query = if is_eip155_network(network) {
            "SELECT \
                EXISTS ( \
                    SELECT 1 FROM api_clients c \
                    WHERE c.status = 'active' \
                ) \
                AND NOT EXISTS ( \
                    SELECT 1 FROM api_clients c \
                    WHERE c.status = 'active' \
                      AND NOT EXISTS ( \
                        SELECT 1 FROM api_client_payees p \
                        WHERE p.client_id = c.id \
                          AND p.network = $1 \
                          AND lower(p.asset) = lower($2) \
                      ) \
                )"
        } else {
            "SELECT \
                EXISTS ( \
                    SELECT 1 FROM api_clients c \
                    WHERE c.status = 'active' \
                ) \
                AND NOT EXISTS ( \
                    SELECT 1 FROM api_clients c \
                    WHERE c.status = 'active' \
                      AND NOT EXISTS ( \
                        SELECT 1 FROM api_client_payees p \
                        WHERE p.client_id = c.id \
                          AND p.network = $1 \
                          AND p.asset = $2 \
                      ) \
                )"
        };
        sqlx::query_scalar(query)
            .bind(network)
            .bind(asset)
            .fetch_one(&self.pool)
            .await
            .map_err(StoreError::Database)
    }

    pub async fn operationally_ready(
        &self,
        network: &str,
        asset: &str,
    ) -> Result<bool, StoreError> {
        self.ping().await?;
        Ok(self.schema_compatible().await?
            && self
                .active_clients_have_payee_policy(network, asset)
                .await?)
    }

    pub async fn lookup_api_key(
        &self,
        key_prefix: &str,
    ) -> Result<Option<ApiKeyCandidate>, StoreError> {
        let row = sqlx::query(
            "SELECT c.id, c.name, c.environment, \
                    c.daily_budget_yocto_near::text AS daily_budget, \
                    c.verify_rate_per_minute, c.settle_rate_per_minute, k.key_digest \
             FROM api_keys k \
             JOIN api_clients c ON c.id = k.client_id \
             WHERE k.key_prefix = $1 \
               AND k.status = 'active' \
               AND c.status = 'active'",
        )
        .bind(key_prefix)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(api_key_from_row).transpose()
    }

    pub async fn touch_api_key(&self, key_prefix: &str) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE api_keys SET last_used_at = now() \
             WHERE key_prefix = $1 AND status = 'active'",
        )
        .bind(key_prefix)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn payee_allowed(
        &self,
        client_id: Uuid,
        network: &str,
        asset: &str,
        pay_to: &str,
    ) -> Result<bool, StoreError> {
        let query = if is_eip155_network(network) {
            "SELECT EXISTS( \
                SELECT 1 FROM api_client_payees \
                WHERE client_id = $1 AND network = $2 \
                  AND lower(asset) = lower($3) AND lower(pay_to) = lower($4) \
             )"
        } else {
            "SELECT EXISTS( \
                SELECT 1 FROM api_client_payees \
                WHERE client_id = $1 AND network = $2 AND asset = $3 AND pay_to = $4 \
             )"
        };
        let allowed: bool = sqlx::query_scalar(query)
            .bind(client_id)
            .bind(network)
            .bind(asset)
            .bind(pay_to)
            .fetch_one(&self.pool)
            .await?;
        Ok(allowed)
    }

    #[allow(clippy::too_many_lines)]
    pub async fn claim_settlement(&self, new: &NewSettlement) -> Result<ClaimOutcome, StoreError> {
        if new.chain_kind == ChainKind::Eip155
            && new
                .authorization_metadata
                .as_ref()
                .is_none_or(|metadata| metadata.version != 2)
        {
            return Err(StoreError::InvalidInput(
                "EVM authorization metadata must use canonical x402 version 2".to_owned(),
            ));
        }
        if let Some(existing) = self
            .find_existing_settlement(
                new.api_client_id,
                new.payment_identifier.as_deref(),
                &new.payment_hash,
                &new.request_fingerprint,
            )
            .await?
        {
            return Ok(existing);
        }
        if self
            .anchor_is_claimed(&new.anchor_scope, &new.anchor_value)
            .await?
        {
            if let Some(existing) = self
                .find_existing_settlement(
                    new.api_client_id,
                    new.payment_identifier.as_deref(),
                    &new.payment_hash,
                    &new.request_fingerprint,
                )
                .await?
            {
                return Ok(existing);
            }
            return Ok(ClaimOutcome::DuplicateSettlement);
        }
        if self
            .active_evm_signer_exists(new.chain_kind, &new.network, new.signer_address.as_deref())
            .await?
        {
            if let Some(existing) = self
                .find_existing_settlement(
                    new.api_client_id,
                    new.payment_identifier.as_deref(),
                    &new.payment_hash,
                    &new.request_fingerprint,
                )
                .await?
            {
                return Ok(existing);
            }
            return Ok(ClaimOutcome::SettlementBusy);
        }

        let usage_date = Utc::now().date_naive();
        let authorization_metadata = new
            .authorization_metadata
            .as_ref()
            .map(EvmAuthorizationMetadata::to_json);
        let mut transaction = self.pool.begin().await?;
        // Insert first so an EVM claim acquires the partial-unique signer slot
        // before locking sponsorship ledgers. Dormant retries use the same
        // signer-slot→budget order; any later denial rolls this row back.
        let inserted = sqlx::query(
            "INSERT INTO settlements ( \
                id, api_client_id, payment_identifier, payment_hash, request_fingerprint, \
                state, x402_version, scheme, network, asset, pay_to, amount, payer, \
                chain_kind, anchor_scope, anchor_value, \
                delegate_public_key, delegate_nonce, delegate_max_block_height, \
                authorization_metadata, signer_address, \
                policy_snapshot, reservation_date, reserved_yocto_near \
             ) VALUES ( \
                $1, $2, $3, $4, $5, 'reserved', $6, $7, $8, $9, $10, $11::numeric, $12, \
                $13, $14, $15, $16, $17::numeric, $18::numeric, $19, $20, \
                $21, $22, $23::numeric \
             ) \
             ON CONFLICT DO NOTHING",
        )
        .bind(new.id)
        .bind(new.api_client_id)
        .bind(&new.payment_identifier)
        .bind(new.payment_hash.as_slice())
        .bind(new.request_fingerprint.as_slice())
        .bind(i16::from(new.x402_version))
        .bind(&new.scheme)
        .bind(&new.network)
        .bind(&new.asset)
        .bind(&new.pay_to)
        .bind(&new.amount)
        .bind(&new.payer)
        .bind(new.chain_kind.as_str())
        .bind(&new.anchor_scope)
        .bind(new.anchor_value.as_slice())
        .bind(&new.delegate_public_key)
        .bind(&new.delegate_nonce)
        .bind(&new.delegate_max_block_height)
        .bind(&authorization_metadata)
        .bind(&new.signer_address)
        .bind(&new.policy_snapshot)
        .bind(usage_date)
        .bind(&new.reservation_yocto_near)
        .execute(&mut *transaction)
        .await?;

        if inserted.rows_affected() == 0 {
            transaction.rollback().await?;
            if let Some(existing) = self
                .find_existing_settlement(
                    new.api_client_id,
                    new.payment_identifier.as_deref(),
                    &new.payment_hash,
                    &new.request_fingerprint,
                )
                .await?
            {
                return Ok(existing);
            }
            if self
                .anchor_is_claimed(&new.anchor_scope, &new.anchor_value)
                .await?
            {
                return Ok(ClaimOutcome::DuplicateSettlement);
            }
            if self
                .active_evm_signer_exists(
                    new.chain_kind,
                    &new.network,
                    new.signer_address.as_deref(),
                )
                .await?
            {
                return Ok(ClaimOutcome::SettlementBusy);
            }
            return Err(StoreError::Corrupt(
                "settlement insert conflicted but conflicting row was not visible".to_owned(),
            ));
        }

        if !reserve_global_budget(
            &mut transaction,
            usage_date,
            &new.reservation_yocto_near,
            &new.global_daily_budget_yocto_near,
        )
        .await?
        {
            transaction.rollback().await?;
            return Ok(ClaimOutcome::BudgetExceeded);
        }
        if !reserve_client_budget(
            &mut transaction,
            usage_date,
            new.api_client_id,
            &new.reservation_yocto_near,
            &new.client_daily_budget_yocto_near,
        )
        .await?
        {
            transaction.rollback().await?;
            return Ok(ClaimOutcome::BudgetExceeded);
        }

        insert_event(
            &mut transaction,
            new.id,
            None,
            SettlementState::Reserved,
            Some("claimed"),
            &serde_json::json!({}),
        )
        .await?;
        transaction.commit().await?;
        let record = self
            .settlement(new.id)
            .await?
            .ok_or_else(|| StoreError::Corrupt("inserted settlement disappeared".to_owned()))?;
        Ok(ClaimOutcome::New(record))
    }

    pub async fn settlement(&self, id: Uuid) -> Result<Option<SettlementRecord>, StoreError> {
        let row = sqlx::query(SETTLEMENT_SELECT)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| settlement_from_row(&row)).transpose()
    }

    pub async fn nonterminal_settlements(&self) -> Result<Vec<SettlementRecord>, StoreError> {
        let sql = format!(
            "{SETTLEMENT_SELECT_BASE} \
             WHERE state IN ('reserved', 'prepared', 'submitted') ORDER BY created_at"
        );
        let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| settlement_from_row(&row))
            .collect()
    }

    pub async fn journal_summary(&self) -> Result<JournalSummary, StoreError> {
        let row = sqlx::query(
            "SELECT \
                count(*) FILTER (WHERE state = 'reserved') AS reserved, \
                count(*) FILTER (WHERE state = 'prepared') AS prepared, \
                count(*) FILTER (WHERE state = 'submitted') AS submitted, \
                min(created_at) AS oldest_created_at \
             FROM settlements \
             WHERE state IN ('reserved', 'prepared', 'submitted')",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(JournalSummary {
            reserved: nonnegative_count(&row, "reserved")?,
            prepared: nonnegative_count(&row, "prepared")?,
            submitted: nonnegative_count(&row, "submitted")?,
            oldest_created_at: row.try_get("oldest_created_at")?,
        })
    }

    pub async fn global_sponsorship_usage_today(&self) -> Result<SponsorshipUsage, StoreError> {
        let row = sqlx::query(
            "SELECT \
                COALESCE(( \
                    SELECT reserved_yocto_near::text \
                    FROM daily_global_sponsorship WHERE usage_date = CURRENT_DATE \
                ), '0') AS reserved, \
                COALESCE(( \
                    SELECT spent_yocto_near::text \
                    FROM daily_global_sponsorship WHERE usage_date = CURRENT_DATE \
                ), '0') AS spent",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(SponsorshipUsage {
            reserved_yocto_near: row.try_get("reserved")?,
            spent_yocto_near: row.try_get("spent")?,
        })
    }

    /// Relinquish a pre-broadcast settlement claim and its sponsorship budget.
    ///
    /// The state change and both budget-ledger updates commit atomically. The
    /// row keeps its payment anchor for exactly-once replay protection, but no
    /// longer owns an EVM signer slot and is excluded from startup recovery.
    pub async fn mark_awaiting_retry(&self, id: Uuid, retry_code: &str) -> Result<(), StoreError> {
        if retry_code.is_empty() {
            return Err(StoreError::InvalidInput(
                "retry code must not be empty".to_owned(),
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT state, reservation_date, api_client_id, \
                    reserved_yocto_near::text AS reserved \
             FROM settlements WHERE id = $1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| StoreError::Corrupt("retry settlement not found".to_owned()))?;
        let from: String = row.try_get("state")?;
        let from_state = SettlementState::from_str(&from)?;
        if from_state != SettlementState::Reserved {
            transaction.rollback().await?;
            return Err(StoreError::Transition {
                from,
                to: SettlementState::AwaitingRetry.as_str().to_owned(),
            });
        }
        let usage_date: NaiveDate = row.try_get("reservation_date")?;
        let client_id: Uuid = row.try_get("api_client_id")?;
        let reserved: String = row.try_get("reserved")?;
        release_budget(&mut transaction, usage_date, client_id, &reserved, "0").await?;

        let result = sqlx::query(
            "UPDATE settlements SET \
                state = 'awaiting_retry', reserved_yocto_near = 0, retry_code = $2, \
                updated_at = now() \
             WHERE id = $1 AND state = 'reserved'",
        )
        .bind(id)
        .bind(retry_code)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StoreError::Transition {
                from,
                to: SettlementState::AwaitingRetry.as_str().to_owned(),
            });
        }
        insert_event(
            &mut transaction,
            id,
            Some(SettlementState::Reserved),
            SettlementState::AwaitingRetry,
            Some(retry_code),
            &serde_json::json!({}),
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Reacquire current sponsorship policy for a dormant retry.
    ///
    /// Budget reservation, policy refresh, attempt increment, and state change
    /// share one transaction. An EVM row can only resume when its configured
    /// signer is not owned by another active settlement.
    #[allow(clippy::too_many_lines)]
    pub async fn resume_awaiting_retry(
        &self,
        retry: &RetryReservation,
    ) -> Result<RetryOutcome, StoreError> {
        let usage_date = Utc::now().date_naive();
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT state, api_client_id \
             FROM settlements WHERE id = $1 FOR UPDATE",
        )
        .bind(retry.settlement_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| StoreError::Corrupt("retry settlement not found".to_owned()))?;
        let from: String = row.try_get("state")?;
        let from_state = SettlementState::from_str(&from)?;
        if from_state != SettlementState::AwaitingRetry {
            transaction.rollback().await?;
            return Err(StoreError::Transition {
                from,
                to: SettlementState::Reserved.as_str().to_owned(),
            });
        }
        let client_id: Uuid = row.try_get("api_client_id")?;

        // Acquire the partial-unique EVM signer slot before locking sponsorship
        // ledgers. Terminalization/relinquishment locks its settlement row
        // before those ledgers, so matching that order avoids a row↔budget
        // deadlock. Any later budget failure rolls this update back.
        let update = sqlx::query(
            "UPDATE settlements SET \
                state = 'reserved', policy_snapshot = $2, reservation_date = $3, \
                reserved_yocto_near = $4::numeric, attempt_count = attempt_count + 1, \
                retry_code = NULL, updated_at = now() \
             WHERE id = $1 AND state = 'awaiting_retry'",
        )
        .bind(retry.settlement_id)
        .bind(&retry.policy_snapshot)
        .bind(usage_date)
        .bind(&retry.reservation_yocto_near)
        .execute(&mut *transaction)
        .await;
        let result = match update {
            Ok(result) => result,
            Err(error) if constraint_name(&error) == Some("settlements_evm_active_signer_idx") => {
                transaction.rollback().await?;
                return Ok(RetryOutcome::SettlementBusy);
            }
            Err(error) => {
                transaction.rollback().await?;
                return Err(StoreError::Database(error));
            }
        };
        if result.rows_affected() != 1 {
            return Err(StoreError::Transition {
                from,
                to: SettlementState::Reserved.as_str().to_owned(),
            });
        }

        if !reserve_global_budget(
            &mut transaction,
            usage_date,
            &retry.reservation_yocto_near,
            &retry.global_daily_budget_yocto_near,
        )
        .await?
        {
            transaction.rollback().await?;
            return Ok(RetryOutcome::BudgetExceeded);
        }
        if !reserve_client_budget(
            &mut transaction,
            usage_date,
            client_id,
            &retry.reservation_yocto_near,
            &retry.client_daily_budget_yocto_near,
        )
        .await?
        {
            transaction.rollback().await?;
            return Ok(RetryOutcome::BudgetExceeded);
        }

        insert_event(
            &mut transaction,
            retry.settlement_id,
            Some(SettlementState::AwaitingRetry),
            SettlementState::Reserved,
            Some("retry_resumed"),
            &serde_json::json!({}),
        )
        .await?;
        transaction.commit().await?;
        let record = self
            .settlement(retry.settlement_id)
            .await?
            .ok_or_else(|| StoreError::Corrupt("resumed settlement disappeared".to_owned()))?;
        Ok(RetryOutcome::Resumed(Box::new(record)))
    }

    /// Raise an active EVM settlement's terminal confirmation depth.
    ///
    /// A lower runtime setting can never weaken the policy already persisted
    /// with a signed submission.
    pub async fn raise_required_confirmations(
        &self,
        id: Uuid,
        minimum: i32,
    ) -> Result<i32, StoreError> {
        if minimum < 1 {
            return Err(StoreError::InvalidInput(
                "required confirmations must be at least 1".to_owned(),
            ));
        }
        let updated: Option<i32> = sqlx::query_scalar(
            "UPDATE settlements SET \
                required_confirmations = GREATEST(required_confirmations, $2), \
                updated_at = now() \
             WHERE id = $1 AND chain_kind = 'eip155' \
               AND state IN ('prepared', 'submitted') \
             RETURNING required_confirmations",
        )
        .bind(id)
        .bind(minimum)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(required) = updated {
            return Ok(required);
        }
        let from: Option<String> =
            sqlx::query_scalar("SELECT state FROM settlements WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        match from {
            Some(from) => Err(StoreError::Transition {
                from,
                to: "confirmation_policy_raised".to_owned(),
            }),
            None => Err(StoreError::Corrupt(
                "confirmation settlement not found".to_owned(),
            )),
        }
    }

    pub async fn mark_prepared(&self, entry: &PreparedJournalEntry) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE settlements SET \
                state = 'prepared', relayer_account_id = $2, relayer_public_key = $3, \
                relayer_nonce = $4::numeric, outer_transaction_bytes = $5, \
                outer_transaction_hash = $6, prepared_at = now(), updated_at = now() \
             WHERE id = $1 AND state = 'reserved'",
        )
        .bind(entry.settlement_id)
        .bind(&entry.relayer_account_id)
        .bind(&entry.relayer_public_key)
        .bind(&entry.relayer_nonce)
        .bind(&entry.transaction_bytes)
        .bind(&entry.transaction_hash)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            let from = state_for_update(&mut transaction, entry.settlement_id).await?;
            return Err(StoreError::Transition {
                from,
                to: SettlementState::Prepared.as_str().to_owned(),
            });
        }
        insert_event(
            &mut transaction,
            entry.settlement_id,
            Some(SettlementState::Reserved),
            SettlementState::Prepared,
            Some("outer_transaction_persisted"),
            &serde_json::json!({"transaction": entry.transaction_hash}),
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Journal the durable EVM submission at prepare: the signed ERC-3009
    /// transaction (RLP + hash), the account nonce it burns, and the confirmation
    /// depth it must reach to be terminal. `signer_address` and
    /// authorization metadata were written at reservation; together these satisfy the
    /// eip155 non-terminal CHECK. This never touches the NEAR relayer /
    /// outer-transaction columns, and shares `mark_prepared`'s reserved→prepared
    /// transition guard and lifecycle event.
    pub async fn mark_prepared_evm(
        &self,
        entry: &EvmPreparedJournalEntry,
    ) -> Result<(), StoreError> {
        if entry.required_confirmations < 1 {
            return Err(StoreError::InvalidInput(
                "required confirmations must be at least 1".to_owned(),
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE settlements SET \
                state = 'prepared', signer_account_nonce = $2::numeric, \
                submitted_tx_rlp = $3, submitted_tx_hash = $4, \
                required_confirmations = $5, prepared_at = now(), updated_at = now() \
             WHERE id = $1 AND state = 'reserved'",
        )
        .bind(entry.settlement_id)
        .bind(&entry.signer_account_nonce)
        .bind(&entry.submitted_tx_rlp)
        .bind(&entry.submitted_tx_hash)
        .bind(entry.required_confirmations)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            let from = state_for_update(&mut transaction, entry.settlement_id).await?;
            return Err(StoreError::Transition {
                from,
                to: SettlementState::Prepared.as_str().to_owned(),
            });
        }
        insert_event(
            &mut transaction,
            entry.settlement_id,
            Some(SettlementState::Reserved),
            SettlementState::Prepared,
            Some("evm_transaction_persisted"),
            &serde_json::json!({ "transaction": entry.submitted_tx_hash }),
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn mark_submitted(&self, id: Uuid) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE settlements SET state = 'submitted', submitted_at = now(), updated_at = now() \
             WHERE id = $1 AND state = 'prepared'",
        )
        .bind(id)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            let from = state_for_update(&mut transaction, id).await?;
            return Err(StoreError::Transition {
                from,
                to: SettlementState::Submitted.as_str().to_owned(),
            });
        }
        insert_event(
            &mut transaction,
            id,
            Some(SettlementState::Prepared),
            SettlementState::Submitted,
            Some("broadcast_started"),
            &serde_json::json!({}),
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    // One cohesive terminalization transaction: lock the row, enforce the
    // idempotent/allowed transition, release the budget, and write the terminal
    // result plus the eip155 confirmation-depth audit columns.
    #[allow(clippy::too_many_lines)]
    pub async fn mark_terminal(&self, entry: &TerminalJournalEntry) -> Result<(), StoreError> {
        if !entry.state.is_terminal() {
            return Err(StoreError::Transition {
                from: "unknown".to_owned(),
                to: entry.state.as_str().to_owned(),
            });
        }
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT state, reservation_date, api_client_id, \
                    reserved_yocto_near::text AS reserved \
             FROM settlements WHERE id = $1 FOR UPDATE",
        )
        .bind(entry.settlement_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| StoreError::Corrupt("terminal settlement not found".to_owned()))?;
        let from: String = row.try_get("state")?;
        let from_state = SettlementState::from_str(&from)?;
        if from_state.is_terminal() {
            // Terminal transitions are idempotent only when the exact body and
            // status already match.
            let existing = sqlx::query(
                "SELECT terminal_http_status, terminal_response_bytes \
                 FROM settlements WHERE id = $1",
            )
            .bind(entry.settlement_id)
            .fetch_one(&mut *transaction)
            .await?;
            let status: Option<i16> = existing.try_get("terminal_http_status")?;
            let bytes: Option<Vec<u8>> = existing.try_get("terminal_response_bytes")?;
            transaction.rollback().await?;
            if status == i16::try_from(entry.http_status).ok()
                && bytes.as_deref() == Some(entry.response_bytes.as_slice())
            {
                return Ok(());
            }
            return Err(StoreError::Transition {
                from,
                to: entry.state.as_str().to_owned(),
            });
        }
        let transition_allowed = matches!(
            (from_state, entry.state),
            (SettlementState::Reserved, SettlementState::Failed)
                | (
                    SettlementState::Prepared | SettlementState::Submitted,
                    SettlementState::Succeeded | SettlementState::Failed,
                )
        );
        if !transition_allowed {
            transaction.rollback().await?;
            return Err(StoreError::Transition {
                from,
                to: entry.state.as_str().to_owned(),
            });
        }
        let usage_date: NaiveDate = row.try_get("reservation_date")?;
        let client_id: Uuid = row.try_get("api_client_id")?;
        let reserved: String = row.try_get("reserved")?;

        release_budget(
            &mut transaction,
            usage_date,
            client_id,
            &reserved,
            &entry.actual_yocto_near,
        )
        .await?;
        let status = i16::try_from(entry.http_status).map_err(|_| {
            StoreError::Corrupt("terminal HTTP status does not fit SMALLINT".to_owned())
        })?;
        sqlx::query(
            "UPDATE settlements SET \
                state = $2, terminal_http_status = $3, terminal_response_bytes = $4, \
                error_code = $5, error_detail = $6, gas_burnt = $7::numeric, \
                tokens_burnt = $8::numeric, mined_block_number = $9::numeric, \
                mined_block_hash = $10, confirmations = $11, \
                finalized_at = now(), updated_at = now() \
             WHERE id = $1",
        )
        .bind(entry.settlement_id)
        .bind(entry.state.as_str())
        .bind(status)
        .bind(&entry.response_bytes)
        .bind(&entry.error_code)
        .bind(&entry.error_detail)
        .bind(entry.gas_burnt.as_deref().unwrap_or("0"))
        .bind(entry.tokens_burnt.as_deref().unwrap_or("0"))
        .bind(entry.mined_block_number.as_deref())
        .bind(entry.mined_block_hash.as_deref())
        .bind(entry.confirmations)
        .execute(&mut *transaction)
        .await?;
        insert_event(
            &mut transaction,
            entry.settlement_id,
            Some(from_state),
            entry.state,
            entry.error_code.as_deref(),
            &serde_json::json!({}),
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn note_reconciliation(&self, id: Uuid) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE settlements SET last_reconciled_at = now(), \
                    reconciliation_attempts = reconciliation_attempts + 1, updated_at = now() \
             WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_relayer(
        &self,
        network: &str,
        account_id: &str,
        public_key: &str,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO relayers (network, account_id, public_key) VALUES ($1, $2, $3) \
             ON CONFLICT (network, account_id, public_key) DO NOTHING",
        )
        .bind(network)
        .bind(account_id)
        .bind(public_key)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn quarantine_relayer(
        &self,
        network: &str,
        account_id: &str,
        public_key: &str,
        reason: &str,
        observed_nonce: &str,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO relayers ( \
                network, account_id, public_key, status, quarantine_reason, last_observed_nonce \
             ) VALUES ($1, $2, $3, 'quarantined', $4, $5::numeric) \
             ON CONFLICT (network, account_id, public_key) DO UPDATE SET \
                status = 'quarantined', quarantine_reason = EXCLUDED.quarantine_reason, \
                last_observed_nonce = EXCLUDED.last_observed_nonce, updated_at = now()",
        )
        .bind(network)
        .bind(account_id)
        .bind(public_key)
        .bind(reason)
        .bind(observed_nonce)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn relayer_is_active(
        &self,
        network: &str,
        account_id: &str,
        public_key: &str,
    ) -> Result<bool, StoreError> {
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM relayers \
             WHERE network = $1 AND account_id = $2 AND public_key = $3",
        )
        .bind(network)
        .bind(account_id)
        .bind(public_key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(matches!(status.as_deref(), Some("active")))
    }

    pub async fn create_client(
        &self,
        client: &ApiClient,
        key_id: Uuid,
        key_prefix: &str,
        key_digest: &[u8; 32],
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO api_clients ( \
                id, name, environment, daily_budget_yocto_near, \
                verify_rate_per_minute, settle_rate_per_minute \
             ) VALUES ($1, $2, $3, $4::numeric, $5, $6)",
        )
        .bind(client.id)
        .bind(&client.name)
        .bind(&client.environment)
        .bind(&client.daily_budget_yocto_near)
        .bind(
            i32::try_from(client.verify_rate_per_minute)
                .map_err(|_| StoreError::Configuration("verify rate is too large".to_owned()))?,
        )
        .bind(
            i32::try_from(client.settle_rate_per_minute)
                .map_err(|_| StoreError::Configuration("settle rate is too large".to_owned()))?,
        )
        .execute(&mut *transaction)
        .await?;
        insert_api_key(&mut transaction, key_id, client.id, key_prefix, key_digest).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn client_environment(&self, client_id: Uuid) -> Result<String, StoreError> {
        sqlx::query_scalar(
            "SELECT environment FROM api_clients WHERE id = $1 AND status = 'active'",
        )
        .bind(client_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::Corrupt("active client not found".to_owned()))
    }

    pub async fn rotate_client_key(
        &self,
        client_id: Uuid,
        key_id: Uuid,
        key_prefix: &str,
        key_digest: &[u8; 32],
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM api_clients WHERE id = $1 AND status = 'active')",
        )
        .bind(client_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !active {
            return Err(StoreError::Corrupt("active client not found".to_owned()));
        }
        sqlx::query(
            "UPDATE api_keys SET status = 'revoked', revoked_at = now() \
             WHERE client_id = $1 AND status = 'active'",
        )
        .bind(client_id)
        .execute(&mut *transaction)
        .await?;
        insert_api_key(&mut transaction, key_id, client_id, key_prefix, key_digest).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn revoke_client(&self, client_id: Uuid) -> Result<bool, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let changed = sqlx::query(
            "UPDATE api_clients SET status = 'revoked', revoked_at = now(), updated_at = now() \
             WHERE id = $1 AND status = 'active'",
        )
        .bind(client_id)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        sqlx::query(
            "UPDATE api_keys SET status = 'revoked', revoked_at = now() \
             WHERE client_id = $1 AND status = 'active'",
        )
        .bind(client_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(changed == 1)
    }

    pub async fn allow_payee(
        &self,
        client_id: Uuid,
        network: &str,
        asset: &str,
        pay_to: &str,
    ) -> Result<(), StoreError> {
        let (asset, pay_to) = if is_eip155_network(network) {
            (asset.to_ascii_lowercase(), pay_to.to_ascii_lowercase())
        } else {
            (asset.to_owned(), pay_to.to_owned())
        };
        sqlx::query(
            "INSERT INTO api_client_payees (client_id, network, asset, pay_to) \
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(client_id)
        .bind(network)
        .bind(asset)
        .bind(pay_to)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_client_budget(
        &self,
        client_id: Uuid,
        daily_yocto_near: &str,
    ) -> Result<bool, StoreError> {
        let changed = sqlx::query(
            "UPDATE api_clients SET daily_budget_yocto_near = $2::numeric, updated_at = now() \
             WHERE id = $1 AND status = 'active'",
        )
        .bind(client_id)
        .bind(daily_yocto_near)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(changed == 1)
    }

    pub async fn find_existing_settlement(
        &self,
        api_client_id: Uuid,
        payment_identifier: Option<&str>,
        payment_hash: &[u8; 32],
        request_fingerprint: &[u8; 32],
    ) -> Result<Option<ClaimOutcome>, StoreError> {
        if let Some(identifier) = payment_identifier {
            let sql = format!(
                "{SETTLEMENT_SELECT_BASE} \
                 WHERE (api_client_id = $1 AND payment_identifier = $2) \
                    OR payment_hash = $3"
            );
            let rows = sqlx::query(&sql)
                .bind(api_client_id)
                .bind(identifier)
                .bind(payment_hash.as_slice())
                .fetch_all(&self.pool)
                .await?;
            let mut payment_hash_exists = false;
            for row in rows {
                let existing = settlement_from_row(&row)?;
                if existing.api_client_id == api_client_id
                    && existing.payment_identifier.as_deref() == Some(identifier)
                {
                    return if existing.request_fingerprint == *request_fingerprint {
                        Ok(Some(ClaimOutcome::Existing(existing)))
                    } else {
                        Ok(Some(ClaimOutcome::IdentifierConflict))
                    };
                }
                payment_hash_exists |= existing.payment_hash == *payment_hash;
            }
            if payment_hash_exists {
                return Ok(Some(ClaimOutcome::DuplicateSettlement));
            }
            return Ok(None);
        }
        let sql = format!("{SETTLEMENT_SELECT_BASE} WHERE payment_hash = $1");
        if let Some(row) = sqlx::query(&sql)
            .bind(payment_hash.as_slice())
            .fetch_optional(&self.pool)
            .await?
        {
            let existing = settlement_from_row(&row)?;
            return if existing.api_client_id == api_client_id
                && existing.request_fingerprint == *request_fingerprint
            {
                Ok(Some(ClaimOutcome::Existing(existing)))
            } else {
                Ok(Some(ClaimOutcome::DuplicateSettlement))
            };
        }
        Ok(None)
    }

    async fn anchor_is_claimed(
        &self,
        anchor_scope: &str,
        anchor_value: &[u8; 32],
    ) -> Result<bool, StoreError> {
        sqlx::query_scalar(
            "SELECT EXISTS( \
                SELECT 1 FROM settlements \
                WHERE anchor_scope = $1 AND anchor_value = $2 \
             )",
        )
        .bind(anchor_scope)
        .bind(anchor_value.as_slice())
        .fetch_one(&self.pool)
        .await
        .map_err(StoreError::Database)
    }

    async fn active_evm_signer_exists(
        &self,
        chain_kind: ChainKind,
        network: &str,
        signer_address: Option<&str>,
    ) -> Result<bool, StoreError> {
        if chain_kind != ChainKind::Eip155 {
            return Ok(false);
        }
        let Some(signer_address) = signer_address else {
            return Ok(false);
        };
        sqlx::query_scalar(
            "SELECT EXISTS( \
                SELECT 1 FROM settlements \
                WHERE chain_kind = 'eip155' \
                  AND state IN ('reserved', 'prepared', 'submitted') \
                  AND network = $1 \
                  AND lower(signer_address) = lower($2::text) \
             )",
        )
        .bind(network)
        .bind(signer_address)
        .fetch_one(&self.pool)
        .await
        .map_err(StoreError::Database)
    }
}

// The delegate identity is NULL on EVM rows (migration 0002); COALESCE to '' so
// the NEAR-typed record fields stay `String` and every NEAR read is unchanged,
// while EVM rows (never read on the NEAR path) carry harmless empties. The
// dedicated EVM submission columns (signer_address / submitted_tx_*) follow the
// NEAR outer-transaction columns and are NULL on NEAR rows.
const SETTLEMENT_SELECT_BASE: &str = "SELECT \
    id, api_client_id, payment_identifier, payment_hash, request_fingerprint, \
    anchor_scope, anchor_value, state, chain_kind, \
    network, asset, pay_to, amount::text AS amount, payer, authorization_metadata, \
    policy_snapshot, \
    COALESCE(delegate_public_key, '') AS delegate_public_key, \
    COALESCE(delegate_nonce::text, '') AS delegate_nonce, \
    COALESCE(delegate_max_block_height::text, '') AS delegate_max_block_height, \
    reservation_date, reserved_yocto_near::text AS reserved_yocto_near, relayer_account_id, \
    relayer_public_key, relayer_nonce::text AS relayer_nonce, outer_transaction_bytes, \
    outer_transaction_hash, signer_address, signer_account_nonce::text AS signer_account_nonce, \
    submitted_tx_rlp, submitted_tx_hash, confirmations, required_confirmations, \
    attempt_count, retry_code, \
    terminal_http_status, terminal_response_bytes, created_at \
    FROM settlements";

const SETTLEMENT_SELECT: &str = "SELECT \
    id, api_client_id, payment_identifier, payment_hash, request_fingerprint, \
    anchor_scope, anchor_value, state, chain_kind, \
    network, asset, pay_to, amount::text AS amount, payer, authorization_metadata, \
    policy_snapshot, \
    COALESCE(delegate_public_key, '') AS delegate_public_key, \
    COALESCE(delegate_nonce::text, '') AS delegate_nonce, \
    COALESCE(delegate_max_block_height::text, '') AS delegate_max_block_height, \
    reservation_date, reserved_yocto_near::text AS reserved_yocto_near, relayer_account_id, \
    relayer_public_key, relayer_nonce::text AS relayer_nonce, outer_transaction_bytes, \
    outer_transaction_hash, signer_address, signer_account_nonce::text AS signer_account_nonce, \
    submitted_tx_rlp, submitted_tx_hash, confirmations, required_confirmations, \
    attempt_count, retry_code, \
    terminal_http_status, terminal_response_bytes, created_at \
    FROM settlements WHERE id = $1";

fn api_key_from_row(row: &PgRow) -> Result<ApiKeyCandidate, StoreError> {
    let verify_rate: i32 = row.try_get("verify_rate_per_minute")?;
    let settle_rate: i32 = row.try_get("settle_rate_per_minute")?;
    Ok(ApiKeyCandidate {
        client: ApiClient {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            environment: row.try_get("environment")?,
            daily_budget_yocto_near: row.try_get("daily_budget")?,
            verify_rate_per_minute: u32::try_from(verify_rate)
                .map_err(|_| StoreError::Corrupt("negative verify rate".to_owned()))?,
            settle_rate_per_minute: u32::try_from(settle_rate)
                .map_err(|_| StoreError::Corrupt("negative settle rate".to_owned()))?,
        },
        digest: row.try_get("key_digest")?,
    })
}

fn settlement_from_row(row: &PgRow) -> Result<SettlementRecord, StoreError> {
    let payment_hash = fixed_hash(row.try_get("payment_hash")?, "payment_hash")?;
    let request_fingerprint =
        fixed_hash(row.try_get("request_fingerprint")?, "request_fingerprint")?;
    let anchor_value = fixed_hash(row.try_get("anchor_value")?, "anchor_value")?;
    let authorization_metadata: Option<Value> = row.try_get("authorization_metadata")?;
    let authorization_metadata = authorization_metadata
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| {
            StoreError::Corrupt(format!("authorization_metadata is invalid: {error}"))
        })?;
    let attempt_count: i32 = row.try_get("attempt_count")?;
    let terminal_http_status: Option<i16> = row.try_get("terminal_http_status")?;
    Ok(SettlementRecord {
        id: row.try_get("id")?,
        api_client_id: row.try_get("api_client_id")?,
        payment_identifier: row.try_get("payment_identifier")?,
        payment_hash,
        request_fingerprint,
        anchor_scope: row.try_get("anchor_scope")?,
        anchor_value,
        state: SettlementState::from_str(row.try_get("state")?)?,
        chain_kind: chain_kind_from_str(row.try_get("chain_kind")?)?,
        network: row.try_get("network")?,
        asset: row.try_get("asset")?,
        pay_to: row.try_get("pay_to")?,
        amount: row.try_get("amount")?,
        payer: row.try_get("payer")?,
        authorization_metadata,
        policy_snapshot: row.try_get("policy_snapshot")?,
        delegate_public_key: row.try_get("delegate_public_key")?,
        delegate_nonce: row.try_get("delegate_nonce")?,
        delegate_max_block_height: row.try_get("delegate_max_block_height")?,
        reservation_date: row.try_get("reservation_date")?,
        reserved_yocto_near: row.try_get("reserved_yocto_near")?,
        relayer_account_id: row.try_get("relayer_account_id")?,
        relayer_public_key: row.try_get("relayer_public_key")?,
        relayer_nonce: row.try_get("relayer_nonce")?,
        outer_transaction_bytes: row.try_get("outer_transaction_bytes")?,
        outer_transaction_hash: row.try_get("outer_transaction_hash")?,
        signer_address: row.try_get("signer_address")?,
        signer_account_nonce: row.try_get("signer_account_nonce")?,
        submitted_tx_rlp: row.try_get("submitted_tx_rlp")?,
        submitted_tx_hash: row.try_get("submitted_tx_hash")?,
        confirmations: row.try_get("confirmations")?,
        required_confirmations: row.try_get("required_confirmations")?,
        attempt_count: u32::try_from(attempt_count)
            .map_err(|_| StoreError::Corrupt("attempt_count is negative".to_owned()))?,
        retry_code: row.try_get("retry_code")?,
        terminal_http_status: terminal_http_status
            .map(u16::try_from)
            .transpose()
            .map_err(|_| StoreError::Corrupt("negative terminal status".to_owned()))?,
        terminal_response_bytes: row.try_get("terminal_response_bytes")?,
        created_at: row.try_get("created_at")?,
    })
}

fn chain_kind_from_str(value: &str) -> Result<ChainKind, StoreError> {
    match value {
        "near" => Ok(ChainKind::Near),
        "eip155" => Ok(ChainKind::Eip155),
        _ => Err(StoreError::Corrupt(format!(
            "unknown settlement chain kind {value}"
        ))),
    }
}

fn constraint_name(error: &sqlx::Error) -> Option<&str> {
    error
        .as_database_error()
        .and_then(|error| error.constraint())
}

fn fixed_hash(bytes: Vec<u8>, field: &str) -> Result<[u8; 32], StoreError> {
    bytes
        .try_into()
        .map_err(|_| StoreError::Corrupt(format!("{field} does not contain exactly 32 bytes")))
}

fn nonnegative_count(row: &PgRow, field: &str) -> Result<u64, StoreError> {
    let count: i64 = row.try_get(field)?;
    u64::try_from(count).map_err(|_| StoreError::Corrupt(format!("{field} count is negative")))
}

async fn reserve_global_budget(
    transaction: &mut Transaction<'_, Postgres>,
    usage_date: NaiveDate,
    reservation: &str,
    limit: &str,
) -> Result<bool, StoreError> {
    let row = sqlx::query(
        "INSERT INTO daily_global_sponsorship (usage_date, reserved_yocto_near) \
         SELECT $1, $2::numeric WHERE $2::numeric <= $3::numeric \
         ON CONFLICT (usage_date) DO UPDATE SET \
            reserved_yocto_near = daily_global_sponsorship.reserved_yocto_near + $2::numeric, \
            updated_at = now() \
         WHERE daily_global_sponsorship.reserved_yocto_near \
             + daily_global_sponsorship.spent_yocto_near + $2::numeric <= $3::numeric \
         RETURNING 1",
    )
    .bind(usage_date)
    .bind(reservation)
    .bind(limit)
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(row.is_some())
}

async fn reserve_client_budget(
    transaction: &mut Transaction<'_, Postgres>,
    usage_date: NaiveDate,
    client_id: Uuid,
    reservation: &str,
    limit: &str,
) -> Result<bool, StoreError> {
    let row = sqlx::query(
        "INSERT INTO daily_client_sponsorship ( \
            usage_date, client_id, reserved_yocto_near \
         ) SELECT $1, $2, $3::numeric WHERE $3::numeric <= $4::numeric \
         ON CONFLICT (usage_date, client_id) DO UPDATE SET \
            reserved_yocto_near = daily_client_sponsorship.reserved_yocto_near + $3::numeric, \
            updated_at = now() \
         WHERE daily_client_sponsorship.reserved_yocto_near \
             + daily_client_sponsorship.spent_yocto_near + $3::numeric <= $4::numeric \
         RETURNING 1",
    )
    .bind(usage_date)
    .bind(client_id)
    .bind(reservation)
    .bind(limit)
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(row.is_some())
}

async fn release_budget(
    transaction: &mut Transaction<'_, Postgres>,
    usage_date: NaiveDate,
    client_id: Uuid,
    reservation: &str,
    actual: &str,
) -> Result<(), StoreError> {
    let global = sqlx::query(
        "UPDATE daily_global_sponsorship SET \
            reserved_yocto_near = reserved_yocto_near - $2::numeric, \
            spent_yocto_near = spent_yocto_near + $3::numeric, updated_at = now() \
         WHERE usage_date = $1 AND reserved_yocto_near >= $2::numeric",
    )
    .bind(usage_date)
    .bind(reservation)
    .bind(actual)
    .execute(&mut **transaction)
    .await?;
    let client = sqlx::query(
        "UPDATE daily_client_sponsorship SET \
            reserved_yocto_near = reserved_yocto_near - $3::numeric, \
            spent_yocto_near = spent_yocto_near + $4::numeric, updated_at = now() \
         WHERE usage_date = $1 AND client_id = $2 \
           AND reserved_yocto_near >= $3::numeric",
    )
    .bind(usage_date)
    .bind(client_id)
    .bind(reservation)
    .bind(actual)
    .execute(&mut **transaction)
    .await?;
    if global.rows_affected() != 1 || client.rows_affected() != 1 {
        return Err(StoreError::Corrupt(
            "sponsorship reservation ledger row is missing".to_owned(),
        ));
    }
    Ok(())
}

async fn insert_event(
    transaction: &mut Transaction<'_, Postgres>,
    settlement_id: Uuid,
    from: Option<SettlementState>,
    to: SettlementState,
    code: Option<&str>,
    metadata: &Value,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO settlement_events (settlement_id, from_state, to_state, code, metadata) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(settlement_id)
    .bind(from.map(SettlementState::as_str))
    .bind(to.as_str())
    .bind(code)
    .bind(metadata)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn state_for_update(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<String, StoreError> {
    sqlx::query_scalar("SELECT state FROM settlements WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or_else(|| StoreError::Corrupt("settlement not found".to_owned()))
}

async fn insert_api_key(
    transaction: &mut Transaction<'_, Postgres>,
    key_id: Uuid,
    client_id: Uuid,
    key_prefix: &str,
    key_digest: &[u8; 32],
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO api_keys (id, client_id, key_prefix, key_digest) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(key_id)
    .bind(client_id)
    .bind(key_prefix)
    .bind(key_digest.as_slice())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn is_eip155_network(network: &str) -> bool {
    network.starts_with("eip155:")
}

#[cfg(test)]
#[path = "store_postgres_tests.rs"]
mod postgres_tests;
