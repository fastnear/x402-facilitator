use std::borrow::Cow;
use std::error::Error;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Row};
use tokio::sync::Barrier;
use url::Url;
use uuid::Uuid;

use super::{
    ApiClient, ClaimOutcome, EvmAuthorizationMetadata, NewSettlement, PgStore,
    PreparedJournalEntry, RetryOutcome, RetryReservation, SettlementRecord, SettlementState,
    StoreError, TerminalJournalEntry, embedded_migrator,
};
use crate::config::ChainKind;
use crate::store::EvmPreparedJournalEntry;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const RESERVATION: &str = "100";
const GLOBAL_LIMIT: &str = "1000";
const CLIENT_LIMIT: &str = "500";

struct TestDatabase {
    store: PgStore,
    pool: PgPool,
    admin: PgPool,
    schema: String,
}

impl TestDatabase {
    async fn new() -> TestResult<Option<Self>> {
        let Some(database) = Self::new_unmigrated().await? else {
            return Ok(None);
        };
        database.store.migrate().await?;
        Ok(Some(database))
    }

    async fn new_unmigrated() -> TestResult<Option<Self>> {
        let Some(database_url) = loopback_database_url()? else {
            eprintln!(
                "skipping PostgreSQL integration test: \
                 X402_FACILITATOR_TEST_DATABASE_URL is unset or not loopback"
            );
            return Ok(None);
        };

        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await?;
        let schema = format!("x402_test_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await?;

        let options =
            PgConnectOptions::from_str(&database_url)?.options([("search_path", schema.as_str())]);
        let pool = PgPoolOptions::new()
            .max_connections(48)
            .connect_with(options)
            .await?;
        let store = PgStore { pool: pool.clone() };

        Ok(Some(Self {
            store,
            pool,
            admin,
            schema,
        }))
    }

    async fn cleanup(self) -> TestResult {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await?;
        self.admin.close().await;
        Ok(())
    }
}

fn loopback_database_url() -> TestResult<Option<String>> {
    let Ok(raw) = std::env::var("X402_FACILITATOR_TEST_DATABASE_URL") else {
        return Ok(None);
    };
    let url = Url::parse(&raw)?;
    if !matches!(url.scheme(), "postgres" | "postgresql") {
        return Ok(None);
    }
    let is_loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    Ok(is_loopback.then_some(raw))
}

async fn seeded_store(database: &TestDatabase) -> TestResult<(ApiClient, NewSettlement)> {
    let client = ApiClient {
        id: Uuid::new_v4(),
        name: "postgres-test-client".to_owned(),
        environment: "testnet".to_owned(),
        daily_budget_yocto_near: CLIENT_LIMIT.to_owned(),
        verify_rate_per_minute: 60,
        settle_rate_per_minute: 10,
    };
    database
        .store
        .create_client(
            &client,
            Uuid::new_v4(),
            &format!("x402_test_{}", Uuid::new_v4().simple()),
            &[9; 32],
        )
        .await?;
    Ok((client.clone(), settlement_for(&client, 1)))
}

fn settlement_for(client: &ApiClient, seed: u8) -> NewSettlement {
    NewSettlement {
        id: Uuid::new_v4(),
        api_client_id: client.id,
        payment_identifier: Some(format!("payment-id-{}", Uuid::new_v4().simple())),
        payment_hash: [seed; 32],
        request_fingerprint: [seed.wrapping_add(1); 32],
        anchor_scope: "near".to_owned(),
        anchor_value: [seed; 32],
        x402_version: 2,
        scheme: "exact".to_owned(),
        network: "near:testnet".to_owned(),
        asset: "usdc.fakes.testnet".to_owned(),
        pay_to: "merchant.mike.testnet".to_owned(),
        amount: "1000".to_owned(),
        payer: "payer.testnet".to_owned(),
        chain_kind: ChainKind::Near,
        delegate_public_key: Some("ed25519:11111111111111111111111111111111".to_owned()),
        delegate_nonce: Some(u64::from(seed).to_string()),
        delegate_max_block_height: Some("1000".to_owned()),
        authorization_metadata: None,
        signer_address: None,
        policy_snapshot: json!({"test": true, "seed": seed}),
        reservation_yocto_near: RESERVATION.to_owned(),
        global_daily_budget_yocto_near: GLOBAL_LIMIT.to_owned(),
        client_daily_budget_yocto_near: CLIENT_LIMIT.to_owned(),
    }
}

const EVM_SIGNER: &str = "0x51f2dbe5c2e1f3f0d9a5b6c7e8f9a0b1c2d3e4f5";

fn evm_settlement_for(client: &ApiClient, seed: u8) -> NewSettlement {
    let mut anchor_value = [0; 32];
    anchor_value[31] = seed;
    NewSettlement {
        id: Uuid::new_v4(),
        api_client_id: client.id,
        payment_identifier: Some(format!("payment-id-{}", Uuid::new_v4().simple())),
        payment_hash: [seed; 32],
        request_fingerprint: [seed.wrapping_add(1); 32],
        anchor_scope: concat!(
            "eip155:84532:",
            "0x036cbd53842c5426634e7929541ec2318f3dcf7e:",
            "0x2222222222222222222222222222222222222222"
        )
        .to_owned(),
        anchor_value,
        x402_version: 2,
        scheme: "exact".to_owned(),
        network: "eip155:84532".to_owned(),
        asset: "0x036CbD53842c5426634e7929541eC2318f3dCF7e".to_owned(),
        pay_to: "0x1111111111111111111111111111111111111111".to_owned(),
        amount: "1000".to_owned(),
        payer: "0x2222222222222222222222222222222222222222".to_owned(),
        chain_kind: ChainKind::Eip155,
        delegate_public_key: None,
        delegate_nonce: None,
        delegate_max_block_height: None,
        authorization_metadata: Some(EvmAuthorizationMetadata {
            version: 2,
            valid_after: "0".to_owned(),
            valid_before: "9999999999".to_owned(),
        }),
        signer_address: Some(EVM_SIGNER.to_owned()),
        policy_snapshot: json!({"test": true, "seed": seed, "chain": "eip155"}),
        reservation_yocto_near: RESERVATION.to_owned(),
        global_daily_budget_yocto_near: GLOBAL_LIMIT.to_owned(),
        client_daily_budget_yocto_near: CLIENT_LIMIT.to_owned(),
    }
}

fn move_evm_settlement_to_mainnet(settlement: &mut NewSettlement) {
    settlement.network = "eip155:8453".to_owned();
    settlement.asset = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".to_owned();
    settlement.anchor_scope = concat!(
        "eip155:8453:",
        "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913:",
        "0x2222222222222222222222222222222222222222"
    )
    .to_owned();
}

#[test]
#[allow(clippy::too_many_lines)]
fn sensitive_journal_debug_output_is_redacted() {
    let sentinel_hash = "sentinel-transaction-hash";
    let sentinel_signature = "sentinel-authorization-signature";
    let sentinel_authorization = "sentinel-authorization-window";
    let sentinel_calldata = b"sentinel-calldata".to_vec();
    let sentinel_response = b"sentinel-terminal-response".to_vec();
    let sentinel_payment_hash = [0x5a; 32];
    let forbidden = [
        sentinel_hash.to_owned(),
        sentinel_signature.to_owned(),
        sentinel_authorization.to_owned(),
        format!("{sentinel_calldata:?}"),
        format!("{sentinel_response:?}"),
        format!("{sentinel_payment_hash:?}"),
    ];

    let metadata = EvmAuthorizationMetadata {
        version: 2,
        valid_after: sentinel_authorization.to_owned(),
        valid_before: sentinel_signature.to_owned(),
    };
    assert_redacted(&format!("{metadata:?}"), &forbidden);

    let client = ApiClient {
        id: Uuid::new_v4(),
        name: "debug-test-client".to_owned(),
        environment: "testnet".to_owned(),
        daily_budget_yocto_near: CLIENT_LIMIT.to_owned(),
        verify_rate_per_minute: 60,
        settle_rate_per_minute: 10,
    };
    let mut new = evm_settlement_for(&client, 0x5a);
    new.payment_identifier = Some(sentinel_signature.to_owned());
    new.payment_hash = sentinel_payment_hash;
    new.request_fingerprint = sentinel_payment_hash;
    new.anchor_scope = sentinel_authorization.to_owned();
    new.anchor_value = sentinel_payment_hash;
    new.authorization_metadata = Some(metadata.clone());
    new.policy_snapshot = json!({"authorization": sentinel_signature});
    assert_redacted(&format!("{new:?}"), &forbidden);

    let prepared = PreparedJournalEntry {
        settlement_id: new.id,
        relayer_account_id: sentinel_signature.to_owned(),
        relayer_public_key: sentinel_authorization.to_owned(),
        relayer_nonce: sentinel_signature.to_owned(),
        transaction_bytes: sentinel_calldata.clone(),
        transaction_hash: sentinel_hash.to_owned(),
    };
    assert_redacted(&format!("{prepared:?}"), &forbidden);

    let evm_prepared = EvmPreparedJournalEntry {
        settlement_id: new.id,
        signer_account_nonce: sentinel_signature.to_owned(),
        submitted_tx_rlp: sentinel_calldata.clone(),
        submitted_tx_hash: sentinel_hash.to_owned(),
        required_confirmations: 2,
    };
    assert_redacted(&format!("{evm_prepared:?}"), &forbidden);

    let terminal = TerminalJournalEntry {
        settlement_id: new.id,
        state: SettlementState::Failed,
        http_status: 503,
        response_bytes: sentinel_response.clone(),
        error_code: Some(sentinel_signature.to_owned()),
        error_detail: Some(sentinel_authorization.to_owned()),
        gas_burnt: Some(sentinel_signature.to_owned()),
        tokens_burnt: Some(sentinel_authorization.to_owned()),
        actual_yocto_near: sentinel_signature.to_owned(),
        mined_block_number: Some(sentinel_hash.to_owned()),
        mined_block_hash: Some(sentinel_hash.to_owned()),
        confirmations: Some(1),
    };
    assert_redacted(&format!("{terminal:?}"), &forbidden);

    let now = chrono::Utc::now();
    let record = SettlementRecord {
        id: new.id,
        api_client_id: new.api_client_id,
        payment_identifier: Some(sentinel_signature.to_owned()),
        payment_hash: sentinel_payment_hash,
        request_fingerprint: sentinel_payment_hash,
        anchor_scope: sentinel_authorization.to_owned(),
        anchor_value: sentinel_payment_hash,
        state: SettlementState::Prepared,
        chain_kind: ChainKind::Eip155,
        network: "eip155:84532".to_owned(),
        asset: sentinel_signature.to_owned(),
        pay_to: sentinel_authorization.to_owned(),
        amount: sentinel_signature.to_owned(),
        payer: sentinel_authorization.to_owned(),
        authorization_metadata: Some(metadata),
        policy_snapshot: json!({"authorization": sentinel_signature}),
        delegate_public_key: sentinel_signature.to_owned(),
        delegate_nonce: sentinel_authorization.to_owned(),
        delegate_max_block_height: sentinel_signature.to_owned(),
        reservation_date: now.date_naive(),
        reserved_yocto_near: sentinel_authorization.to_owned(),
        relayer_account_id: Some(sentinel_signature.to_owned()),
        relayer_public_key: Some(sentinel_authorization.to_owned()),
        relayer_nonce: Some(sentinel_signature.to_owned()),
        outer_transaction_bytes: Some(sentinel_calldata.clone()),
        outer_transaction_hash: Some(sentinel_hash.to_owned()),
        signer_address: Some(sentinel_signature.to_owned()),
        signer_account_nonce: Some(sentinel_authorization.to_owned()),
        submitted_tx_rlp: Some(sentinel_calldata),
        submitted_tx_hash: Some(sentinel_hash.to_owned()),
        confirmations: Some(1),
        required_confirmations: Some(2),
        attempt_count: 1,
        retry_code: Some(sentinel_authorization.to_owned()),
        terminal_http_status: None,
        terminal_response_bytes: Some(sentinel_response),
        created_at: now,
    };
    assert_redacted(&format!("{record:?}"), &forbidden);
    assert_redacted(&format!("{:?}", ClaimOutcome::New(record)), &forbidden);
}

fn assert_redacted(debug: &str, forbidden: &[String]) {
    assert!(debug.contains("<redacted>"));
    for sentinel in forbidden {
        assert!(
            !debug.contains(sentinel),
            "sensitive sentinel leaked through Debug: {sentinel}"
        );
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn migration_0003_backfills_anchors_and_scrubs_full_evm_authorization() -> TestResult {
    let Some(database) = TestDatabase::new_unmigrated().await? else {
        return Ok(());
    };
    let migrations = embedded_migrator();
    let v0_4_migrator = sqlx::migrate::Migrator {
        migrations: Cow::Owned(migrations.iter().take(2).cloned().collect()),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    v0_4_migrator.run(&database.pool).await?;

    let client = ApiClient {
        id: Uuid::new_v4(),
        name: "migration-test-client".to_owned(),
        environment: "testnet".to_owned(),
        daily_budget_yocto_near: CLIENT_LIMIT.to_owned(),
        verify_rate_per_minute: 60,
        settle_rate_per_minute: 10,
    };
    database
        .store
        .create_client(&client, Uuid::new_v4(), "migration-test", &[7; 32])
        .await?;
    let near_id = Uuid::new_v4();
    let near_hash = [0x11; 32];
    let near_fingerprint = [0x12_u8; 32];
    sqlx::query(
        "INSERT INTO settlements ( \
            id, api_client_id, payment_hash, request_fingerprint, state, \
            x402_version, scheme, network, asset, pay_to, amount, payer, \
            delegate_public_key, delegate_nonce, delegate_max_block_height, \
            policy_snapshot, reservation_date, reserved_yocto_near, chain_kind \
         ) VALUES ( \
            $1, $2, $3, $4, 'reserved', 2, 'exact', 'near:testnet', \
            'usdc.fakes.testnet', 'merchant.testnet', 1000, 'payer.testnet', \
            'ed25519:test', 1, 1000, '{}'::jsonb, CURRENT_DATE, 100, 'near' \
         )",
    )
    .bind(near_id)
    .bind(client.id)
    .bind(near_hash.as_slice())
    .bind(near_fingerprint.as_slice())
    .execute(&database.pool)
    .await?;

    let evm_id = Uuid::new_v4();
    let mut evm_nonce = [0; 32];
    evm_nonce[31] = 0xaa;
    let evm_hash = [0x22_u8; 32];
    let evm_fingerprint = [0x23_u8; 32];
    sqlx::query(
        "INSERT INTO settlements ( \
            id, api_client_id, payment_hash, request_fingerprint, state, \
            x402_version, scheme, network, asset, pay_to, amount, payer, \
            policy_snapshot, reservation_date, reserved_yocto_near, chain_kind, \
            evm_authorization, signer_address \
         ) VALUES ( \
            $1, $2, $3, $4, 'reserved', 2, 'exact', 'eip155:84532', \
            '0x036CbD53842c5426634e7929541eC2318f3dCF7e', \
            '0x1111111111111111111111111111111111111111', 1000, \
            '0x2222222222222222222222222222222222222222', \
            '{}'::jsonb, CURRENT_DATE, 100, 'eip155', $5, $6 \
         )",
    )
    .bind(evm_id)
    .bind(client.id)
    .bind(evm_hash.as_slice())
    .bind(evm_fingerprint.as_slice())
    .bind(json!({
        "from": "0x2222222222222222222222222222222222222222",
        "to": "0x1111111111111111111111111111111111111111",
        "value": "1000",
        "validAfter": "10",
        "validBefore": "9999999999",
        "nonce": format!("0x{}", hex::encode(evm_nonce)),
        "signature": "0xdeadbeef",
    }))
    .bind(EVM_SIGNER)
    .execute(&database.pool)
    .await?;

    // Exercise the forward migration over the lifecycle shapes a drained
    // v0.4.1 database can still contain: terminal rows and exact-byte recovery
    // rows for both chain families, in addition to the reserved rows above.
    let near_prepared_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO settlements ( \
            id, api_client_id, payment_hash, request_fingerprint, state, \
            x402_version, scheme, network, asset, pay_to, amount, payer, \
            delegate_public_key, delegate_nonce, delegate_max_block_height, \
            policy_snapshot, reservation_date, reserved_yocto_near, chain_kind, \
            relayer_account_id, relayer_public_key, relayer_nonce, \
            outer_transaction_bytes, outer_transaction_hash \
         ) VALUES ( \
            $1, $2, $3, $4, 'prepared', 2, 'exact', 'near:testnet', \
            'usdc.fakes.testnet', 'merchant.testnet', 1001, 'payer.testnet', \
            'ed25519:prepared', 2, 1001, '{}'::jsonb, CURRENT_DATE, 101, 'near', \
            'relayer.testnet', 'ed25519:relayer', 7, $5, 'near-prepared-hash' \
         )",
    )
    .bind(near_prepared_id)
    .bind(client.id)
    .bind([0x31_u8; 32].as_slice())
    .bind([0x32_u8; 32].as_slice())
    .bind([0x33_u8; 48].as_slice())
    .execute(&database.pool)
    .await?;

    let near_terminal_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO settlements ( \
            id, api_client_id, payment_hash, request_fingerprint, state, \
            x402_version, scheme, network, asset, pay_to, amount, payer, \
            delegate_public_key, delegate_nonce, delegate_max_block_height, \
            policy_snapshot, reservation_date, reserved_yocto_near, chain_kind, \
            relayer_account_id, relayer_public_key, relayer_nonce, \
            outer_transaction_bytes, outer_transaction_hash, \
            terminal_http_status, terminal_response_bytes, finalized_at \
         ) VALUES ( \
            $1, $2, $3, $4, 'succeeded', 2, 'exact', 'near:testnet', \
            'usdc.fakes.testnet', 'merchant.testnet', 1002, 'payer.testnet', \
            'ed25519:terminal', 3, 1002, '{}'::jsonb, CURRENT_DATE, 102, 'near', \
            'relayer.testnet', 'ed25519:relayer', 8, $5, 'near-terminal-hash', \
            200, $6, now() \
         )",
    )
    .bind(near_terminal_id)
    .bind(client.id)
    .bind([0x34_u8; 32].as_slice())
    .bind([0x35_u8; 32].as_slice())
    .bind([0x36_u8; 48].as_slice())
    .bind(br#"{"success":true}"#.as_slice())
    .execute(&database.pool)
    .await?;

    let evm_submitted_id = Uuid::new_v4();
    let evm_submitted_nonce = [0x41_u8; 32];
    sqlx::query(
        "INSERT INTO settlements ( \
            id, api_client_id, payment_hash, request_fingerprint, state, \
            x402_version, scheme, network, asset, pay_to, amount, payer, \
            policy_snapshot, reservation_date, reserved_yocto_near, chain_kind, \
            evm_authorization, signer_address, signer_account_nonce, \
            submitted_tx_rlp, submitted_tx_hash, required_confirmations \
         ) VALUES ( \
            $1, $2, $3, $4, 'submitted', 2, 'exact', 'eip155:84532', \
            '0x036CbD53842c5426634e7929541eC2318f3dCF7e', \
            '0x3333333333333333333333333333333333333333', 1003, \
            '0x4444444444444444444444444444444444444444', \
            '{}'::jsonb, CURRENT_DATE, 103, 'eip155', $5, $6, 11, \
            $7, '0xsubmitted', 2 \
         )",
    )
    .bind(evm_submitted_id)
    .bind(client.id)
    .bind([0x42_u8; 32].as_slice())
    .bind([0x43_u8; 32].as_slice())
    .bind(json!({
        "validAfter": "11",
        "validBefore": "9999999999",
        "nonce": format!("0x{}", hex::encode(evm_submitted_nonce)),
        "signature": "0xsubmitted-secret",
    }))
    .bind("0x5555555555555555555555555555555555555555")
    .bind([0x44_u8; 64].as_slice())
    .execute(&database.pool)
    .await?;

    let evm_terminal_id = Uuid::new_v4();
    let evm_terminal_nonce = [0x51_u8; 32];
    sqlx::query(
        "INSERT INTO settlements ( \
            id, api_client_id, payment_hash, request_fingerprint, state, \
            x402_version, scheme, network, asset, pay_to, amount, payer, \
            policy_snapshot, reservation_date, reserved_yocto_near, chain_kind, \
            evm_authorization, signer_address, signer_account_nonce, \
            submitted_tx_rlp, submitted_tx_hash, required_confirmations, \
            confirmations, mined_block_number, mined_block_hash, \
            terminal_http_status, terminal_response_bytes, finalized_at \
         ) VALUES ( \
            $1, $2, $3, $4, 'succeeded', 2, 'exact', 'eip155:8453', \
            '0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913', \
            '0x6666666666666666666666666666666666666666', 1004, \
            '0x7777777777777777777777777777777777777777', \
            '{}'::jsonb, CURRENT_DATE, 104, 'eip155', $5, $6, 12, \
            $7, '0xterminal', 2, 2, 100, '0xblock', 200, $8, now() \
         )",
    )
    .bind(evm_terminal_id)
    .bind(client.id)
    .bind([0x52_u8; 32].as_slice())
    .bind([0x53_u8; 32].as_slice())
    .bind(json!({
        "validAfter": "12",
        "validBefore": "9999999999",
        "nonce": format!("0x{}", hex::encode(evm_terminal_nonce)),
        "signature": "0xterminal-secret",
    }))
    .bind("0x8888888888888888888888888888888888888888")
    .bind([0x54_u8; 64].as_slice())
    .bind(br#"{"success":true}"#.as_slice())
    .execute(&database.pool)
    .await?;

    assert!(!database.store.schema_compatible().await?);
    let heap_before: i64 =
        sqlx::query_scalar("SELECT pg_relation_filenode('settlements'::regclass)::bigint")
            .fetch_one(&database.pool)
            .await?;
    let toast_before: i64 = sqlx::query_scalar(
        "SELECT toast.relfilenode::bigint \
         FROM pg_class AS heap \
         JOIN pg_class AS toast ON toast.oid = heap.reltoastrelid \
         WHERE heap.oid = 'settlements'::regclass",
    )
    .fetch_one(&database.pool)
    .await?;
    database.store.migrate().await?;
    assert!(database.store.schema_compatible().await?);
    let heap_after: i64 =
        sqlx::query_scalar("SELECT pg_relation_filenode('settlements'::regclass)::bigint")
            .fetch_one(&database.pool)
            .await?;
    let toast_after: i64 = sqlx::query_scalar(
        "SELECT toast.relfilenode::bigint \
         FROM pg_class AS heap \
         JOIN pg_class AS toast ON toast.oid = heap.reltoastrelid \
         WHERE heap.oid = 'settlements'::regclass",
    )
    .fetch_one(&database.pool)
    .await?;
    assert_ne!(
        heap_before, heap_after,
        "authorization scrub did not rewrite the populated v0.4.1 heap"
    );
    assert_ne!(
        toast_before, toast_after,
        "authorization scrub did not rewrite the populated v0.4.1 TOAST storage"
    );

    let near = sqlx::query(
        "SELECT anchor_scope, anchor_value, authorization_metadata \
         FROM settlements WHERE id = $1",
    )
    .bind(near_id)
    .fetch_one(&database.pool)
    .await?;
    assert_eq!(near.try_get::<String, _>("anchor_scope")?, "near");
    assert_eq!(near.try_get::<Vec<u8>, _>("anchor_value")?, near_hash);
    assert_eq!(
        near.try_get::<Option<Value>, _>("authorization_metadata")?,
        None
    );

    let evm = sqlx::query(
        "SELECT anchor_scope, anchor_value, authorization_metadata \
         FROM settlements WHERE id = $1",
    )
    .bind(evm_id)
    .fetch_one(&database.pool)
    .await?;
    assert_eq!(
        evm.try_get::<String, _>("anchor_scope")?,
        concat!(
            "eip155:84532:",
            "0x036cbd53842c5426634e7929541ec2318f3dcf7e:",
            "0x2222222222222222222222222222222222222222"
        )
    );
    assert_eq!(evm.try_get::<Vec<u8>, _>("anchor_value")?, evm_nonce);
    assert_eq!(
        evm.try_get::<Value, _>("authorization_metadata")?,
        json!({"version": 2, "validAfter": "10", "validBefore": "9999999999"})
    );
    let legacy_column_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
            SELECT 1 FROM information_schema.columns \
            WHERE table_schema = current_schema() \
              AND table_name = 'settlements' \
              AND column_name = 'evm_authorization' \
         )",
    )
    .fetch_one(&database.pool)
    .await?;
    assert!(!legacy_column_exists);
    let migrated_states: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, state FROM settlements \
         WHERE id = ANY($1) ORDER BY id",
    )
    .bind([
        near_prepared_id,
        near_terminal_id,
        evm_submitted_id,
        evm_terminal_id,
    ])
    .fetch_all(&database.pool)
    .await?;
    assert_eq!(migrated_states.len(), 4);
    assert!(migrated_states.contains(&(near_prepared_id, "prepared".to_owned())));
    assert!(migrated_states.contains(&(near_terminal_id, "succeeded".to_owned())));
    assert!(migrated_states.contains(&(evm_submitted_id, "submitted".to_owned())));
    assert!(migrated_states.contains(&(evm_terminal_id, "succeeded".to_owned())));

    database.cleanup().await
}

#[tokio::test]
async fn startup_requires_the_post_migration_authorization_scrub_rewrite() -> TestResult {
    let Some(database) = TestDatabase::new().await? else {
        return Ok(());
    };
    let (_client, settlement) = seeded_store(&database).await?;
    let ClaimOutcome::New(_) = database.store.claim_settlement(&settlement).await? else {
        return Err(std::io::Error::other("failed to seed scrub rewrite row").into());
    };
    let before: i64 =
        sqlx::query_scalar("SELECT pg_relation_filenode('settlements'::regclass)::bigint")
            .fetch_one(&database.pool)
            .await?;
    sqlx::query(
        "COMMENT ON TABLE settlements IS \
         'x402-maintenance:0003-authorization-scrub:pending'",
    )
    .execute(&database.pool)
    .await?;

    assert!(!database.store.schema_compatible().await?);
    database.store.migrate().await?;
    assert!(database.store.schema_compatible().await?);
    let after: i64 =
        sqlx::query_scalar("SELECT pg_relation_filenode('settlements'::regclass)::bigint")
            .fetch_one(&database.pool)
            .await?;
    assert_ne!(
        before, after,
        "authorization scrub did not rewrite the heap"
    );

    database.cleanup().await
}

#[tokio::test]
async fn schema_and_active_client_policy_gate_database_readiness() -> TestResult {
    let Some(database) = TestDatabase::new().await? else {
        return Ok(());
    };
    assert!(database.store.schema_compatible().await?);
    assert!(
        !database
            .store
            .operationally_ready("near:testnet", "usdc.fakes.testnet")
            .await?
    );

    let (client, _settlement) = seeded_store(&database).await?;
    assert!(
        !database
            .store
            .operationally_ready("near:testnet", "usdc.fakes.testnet")
            .await?
    );
    database
        .store
        .allow_payee(
            client.id,
            "near:testnet",
            "usdc.fakes.testnet",
            "merchant.mike.testnet",
        )
        .await?;
    assert!(
        database
            .store
            .operationally_ready("near:testnet", "usdc.fakes.testnet")
            .await?
    );

    database.cleanup().await
}

#[tokio::test]
async fn eip155_payee_policy_is_address_case_insensitive_but_near_remains_exact() -> TestResult {
    let Some(database) = TestDatabase::new().await? else {
        return Ok(());
    };
    let (client, _settlement) = seeded_store(&database).await?;
    let network = "eip155:84532";
    let asset = "0x036CbD53842c5426634e7929541eC2318f3dCF7e";
    let payee = "0xA2AcB5d3aC1c35999532624188470eC6228039Dc";
    database
        .store
        .allow_payee(client.id, network, asset, payee)
        .await?;

    assert!(
        database
            .store
            .active_clients_have_payee_policy(
                network,
                "0x036cbd53842c5426634e7929541ec2318f3dcf7e",
            )
            .await?
    );
    assert!(
        database
            .store
            .payee_allowed(
                client.id,
                network,
                "0X036CBD53842C5426634E7929541EC2318F3DCF7E",
                "0xA2ACB5D3AC1C35999532624188470EC6228039DC",
            )
            .await?
    );

    database
        .store
        .allow_payee(
            client.id,
            "near:testnet",
            "Asset.Testnet",
            "Merchant.Testnet",
        )
        .await?;
    assert!(
        !database
            .store
            .payee_allowed(
                client.id,
                "near:testnet",
                "asset.testnet",
                "merchant.testnet",
            )
            .await?
    );
    database.cleanup().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_hundred_identical_claims_create_one_reservation() -> TestResult {
    let Some(database) = TestDatabase::new().await? else {
        return Ok(());
    };
    let (_client, settlement) = seeded_store(&database).await?;
    let concurrency = 200;
    let barrier = Arc::new(Barrier::new(concurrency + 1));
    let mut tasks = Vec::with_capacity(concurrency);

    for _ in 0..concurrency {
        let store = database.store.clone();
        let candidate = settlement.clone();
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            store.claim_settlement(&candidate).await
        }));
    }
    barrier.wait().await;

    let mut inserted = 0;
    let mut joined = 0;
    for task in tasks {
        match task.await?? {
            ClaimOutcome::New(record) => {
                inserted += 1;
                assert_eq!(record.state, SettlementState::Reserved);
            }
            ClaimOutcome::Existing(record) => {
                joined += 1;
                assert_eq!(record.id, settlement.id);
            }
            other => {
                return Err(std::io::Error::other(format!(
                    "unexpected concurrent claim outcome: {other:?}"
                ))
                .into());
            }
        }
    }

    assert_eq!(inserted, 1);
    assert_eq!(joined, concurrency - 1);
    let settlement_count: i64 = sqlx::query_scalar("SELECT count(*) FROM settlements")
        .fetch_one(&database.pool)
        .await?;
    let event_count: i64 = sqlx::query_scalar("SELECT count(*) FROM settlement_events")
        .fetch_one(&database.pool)
        .await?;
    let global_reserved: String =
        sqlx::query_scalar("SELECT reserved_yocto_near::text FROM daily_global_sponsorship")
            .fetch_one(&database.pool)
            .await?;
    let client_reserved: String =
        sqlx::query_scalar("SELECT reserved_yocto_near::text FROM daily_client_sponsorship")
            .fetch_one(&database.pool)
            .await?;
    assert_eq!(settlement_count, 1);
    assert_eq!(event_count, 1);
    assert_eq!(global_reserved, RESERVATION);
    assert_eq!(client_reserved, RESERVATION);

    database.cleanup().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_distinct_payments_sharing_anchor_create_one_reservation() -> TestResult {
    let Some(database) = TestDatabase::new().await? else {
        return Ok(());
    };
    let (client, _) = seeded_store(&database).await?;
    let first = evm_settlement_for(&client, 30);
    let mut second = evm_settlement_for(&client, 31);
    // Keep the signer-slot uniqueness independent from this anchor race.
    second.signer_address = Some("0x61f2dbe5c2e1f3f0d9a5b6c7e8f9a0b1c2d3e4f5".to_owned());
    second.anchor_scope.clone_from(&first.anchor_scope);
    second.anchor_value = first.anchor_value;
    assert_ne!(first.payment_hash, second.payment_hash);
    assert_ne!(first.request_fingerprint, second.request_fingerprint);
    assert_ne!(first.payment_identifier, second.payment_identifier);

    let barrier = Arc::new(Barrier::new(3));
    let first_task = {
        let store = database.store.clone();
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            barrier.wait().await;
            store.claim_settlement(&first).await
        })
    };
    let second_task = {
        let store = database.store.clone();
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            barrier.wait().await;
            store.claim_settlement(&second).await
        })
    };
    barrier.wait().await;
    let outcomes = [first_task.await??, second_task.await??];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ClaimOutcome::New(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ClaimOutcome::DuplicateSettlement))
            .count(),
        1
    );
    let settlement_count: i64 = sqlx::query_scalar("SELECT count(*) FROM settlements")
        .fetch_one(&database.pool)
        .await?;
    let event_count: i64 = sqlx::query_scalar("SELECT count(*) FROM settlement_events")
        .fetch_one(&database.pool)
        .await?;
    let usage = database.store.global_sponsorship_usage_today().await?;
    assert_eq!(settlement_count, 1);
    assert_eq!(event_count, 1);
    assert_eq!(usage.reserved_yocto_near, RESERVATION);
    assert_eq!(usage.spent_yocto_near, "0");

    database.cleanup().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_evm_claims_grant_one_active_signer_owner() -> TestResult {
    let Some(database) = TestDatabase::new().await? else {
        return Ok(());
    };
    let (client, _) = seeded_store(&database).await?;
    let first = evm_settlement_for(&client, 20);
    let second = evm_settlement_for(&client, 21);
    let barrier = Arc::new(Barrier::new(3));

    let first_task = {
        let store = database.store.clone();
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            barrier.wait().await;
            store.claim_settlement(&first).await
        })
    };
    let second_task = {
        let store = database.store.clone();
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            barrier.wait().await;
            store.claim_settlement(&second).await
        })
    };
    barrier.wait().await;

    let outcomes = [first_task.await??, second_task.await??];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ClaimOutcome::New(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ClaimOutcome::SettlementBusy))
            .count(),
        1
    );
    let settlement_count: i64 = sqlx::query_scalar("SELECT count(*) FROM settlements")
        .fetch_one(&database.pool)
        .await?;
    let global_reserved: String =
        sqlx::query_scalar("SELECT reserved_yocto_near::text FROM daily_global_sponsorship")
            .fetch_one(&database.pool)
            .await?;
    assert_eq!(settlement_count, 1);
    assert_eq!(global_reserved, RESERVATION);

    let mut other_network = evm_settlement_for(&client, 22);
    move_evm_settlement_to_mainnet(&mut other_network);
    assert!(matches!(
        database.store.claim_settlement(&other_network).await?,
        ClaimOutcome::New(_)
    ));

    database.cleanup().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_evm_claim_and_dormant_retry_share_signer_then_budget_lock_order() -> TestResult {
    let Some(database) = TestDatabase::new().await? else {
        return Ok(());
    };
    let (client, _) = seeded_store(&database).await?;
    let dormant = evm_settlement_for(&client, 23);
    assert!(matches!(
        database.store.claim_settlement(&dormant).await?,
        ClaimOutcome::New(_)
    ));
    database
        .store
        .mark_awaiting_retry(dormant.id, "test_dormant")
        .await?;
    let contender = evm_settlement_for(&client, 24);
    let contender_id = contender.id;

    // Hold an advisory lock from a BEFORE INSERT trigger so the new claim is
    // paused exactly at signer-slot acquisition. With the old budget→slot
    // order it held the sponsorship row here while the retry held its
    // settlement/signer slot, deterministically forming a deadlock.
    let advisory_key = 7_402_155_i64;
    let mut blocker = database.pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(advisory_key)
        .execute(&mut *blocker)
        .await?;
    let trigger = format!(
        "CREATE FUNCTION pause_contender_insert() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN \
                 IF NEW.id = '{}'::uuid THEN \
                     PERFORM pg_advisory_xact_lock({advisory_key}); \
                 END IF; \
                 RETURN NEW; \
             END \
         $$; \
         CREATE TRIGGER pause_contender_insert \
         BEFORE INSERT ON settlements \
         FOR EACH ROW EXECUTE FUNCTION pause_contender_insert();",
        contender.id
    );
    sqlx::raw_sql(&trigger).execute(&database.pool).await?;

    let claim_task = {
        let store = database.store.clone();
        tokio::spawn(async move { store.claim_settlement(&contender).await })
    };
    let reached_insert = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS( \
                    SELECT 1 FROM pg_stat_activity \
                    WHERE wait_event_type = 'Lock' AND wait_event = 'advisory' \
                      AND query LIKE 'INSERT INTO settlements%' \
                 )",
            )
            .fetch_one(&database.pool)
            .await?;
            if waiting {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    if reached_insert.is_err() {
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(advisory_key)
            .execute(&mut *blocker)
            .await?;
        claim_task.abort();
        return Err(std::io::Error::other("contender never reached signer-slot insertion").into());
    }
    reached_insert??;

    let retry = RetryReservation {
        settlement_id: dormant.id,
        policy_snapshot: json!({"policy": "retry"}),
        reservation_yocto_near: RESERVATION.to_owned(),
        global_daily_budget_yocto_near: GLOBAL_LIMIT.to_owned(),
        client_daily_budget_yocto_near: CLIENT_LIMIT.to_owned(),
    };
    let retry_task = {
        let store = database.store.clone();
        tokio::spawn(async move { store.resume_awaiting_retry(&retry).await })
    };
    let retry_outcome = tokio::time::timeout(Duration::from_secs(2), retry_task).await;
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(advisory_key)
        .execute(&mut *blocker)
        .await?;
    let retry_outcome = retry_outcome
        .map_err(|_| std::io::Error::other("claim/retry lock order deadlocked"))???;
    assert!(matches!(retry_outcome, RetryOutcome::Resumed(_)));

    let claim_outcome = claim_task.await??;
    assert!(matches!(claim_outcome, ClaimOutcome::SettlementBusy));
    let owner = database
        .store
        .settlement(dormant.id)
        .await?
        .ok_or_else(|| std::io::Error::other("retry owner disappeared"))?;
    assert_eq!(owner.state, SettlementState::Reserved);
    assert!(database.store.settlement(contender_id).await?.is_none());
    let usage = database.store.global_sponsorship_usage_today().await?;
    assert_eq!(usage.reserved_yocto_near, RESERVATION);
    assert_eq!(usage.spent_yocto_near, "0");

    drop(blocker);
    database.cleanup().await
}

#[tokio::test]
async fn identity_and_anchor_conflicts_do_not_reserve_twice() -> TestResult {
    let Some(database) = TestDatabase::new().await? else {
        return Ok(());
    };
    let (client, original) = seeded_store(&database).await?;
    assert!(matches!(
        database.store.claim_settlement(&original).await?,
        ClaimOutcome::New(_)
    ));

    let mut identifier_conflict = settlement_for(&client, 3);
    identifier_conflict.payment_identifier = original.payment_identifier.clone();
    assert!(matches!(
        database
            .store
            .claim_settlement(&identifier_conflict)
            .await?,
        ClaimOutcome::IdentifierConflict
    ));

    let mut duplicate_delegate = settlement_for(&client, 4);
    duplicate_delegate.payment_hash = original.payment_hash;
    assert!(matches!(
        database.store.claim_settlement(&duplicate_delegate).await?,
        ClaimOutcome::DuplicateSettlement
    ));

    let mut duplicate_anchor = settlement_for(&client, 5);
    duplicate_anchor
        .anchor_scope
        .clone_from(&original.anchor_scope);
    duplicate_anchor.anchor_value = original.anchor_value;
    assert!(matches!(
        database.store.claim_settlement(&duplicate_anchor).await?,
        ClaimOutcome::DuplicateSettlement
    ));

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM settlements")
        .fetch_one(&database.pool)
        .await?;
    let budget = sqlx::query(
        "SELECT reserved_yocto_near::text AS reserved, spent_yocto_near::text AS spent \
         FROM daily_global_sponsorship",
    )
    .fetch_one(&database.pool)
    .await?;
    assert_eq!(count, 1);
    assert_eq!(budget.try_get::<String, _>("reserved")?, RESERVATION);
    assert_eq!(budget.try_get::<String, _>("spent")?, "0");

    database.cleanup().await
}

#[tokio::test]
async fn identifierless_identical_retry_replays_and_resumes() -> TestResult {
    let Some(database) = TestDatabase::new().await? else {
        return Ok(());
    };
    let (_client, mut settlement) = seeded_store(&database).await?;
    settlement.payment_identifier = None;
    assert!(matches!(
        database.store.claim_settlement(&settlement).await?,
        ClaimOutcome::New(_)
    ));
    database
        .store
        .mark_awaiting_retry(settlement.id, "rpc_preflight_unavailable")
        .await?;

    let replay = database.store.claim_settlement(&settlement).await?;
    let ClaimOutcome::Existing(record) = replay else {
        return Err(std::io::Error::other("identifierless retry was not replayed").into());
    };
    assert_eq!(record.id, settlement.id);
    assert_eq!(record.state, SettlementState::AwaitingRetry);

    let retry = RetryReservation {
        settlement_id: record.id,
        policy_snapshot: json!({"policy": "current"}),
        reservation_yocto_near: RESERVATION.to_owned(),
        global_daily_budget_yocto_near: GLOBAL_LIMIT.to_owned(),
        client_daily_budget_yocto_near: CLIENT_LIMIT.to_owned(),
    };
    let RetryOutcome::Resumed(resumed) = database.store.resume_awaiting_retry(&retry).await? else {
        return Err(std::io::Error::other("identifierless retry did not resume").into());
    };
    assert_eq!(resumed.state, SettlementState::Reserved);
    assert_eq!(resumed.attempt_count, 2);

    let mut mismatched = settlement.clone();
    mismatched.request_fingerprint = [0xee; 32];
    assert!(matches!(
        database.store.claim_settlement(&mismatched).await?,
        ClaimOutcome::DuplicateSettlement
    ));

    database.cleanup().await
}

#[tokio::test]
async fn client_budget_failure_rolls_back_global_reservation_atomically() -> TestResult {
    let Some(database) = TestDatabase::new().await? else {
        return Ok(());
    };
    let (client, mut settlement) = seeded_store(&database).await?;
    settlement.client_daily_budget_yocto_near = "99".to_owned();
    assert!(matches!(
        database.store.claim_settlement(&settlement).await?,
        ClaimOutcome::BudgetExceeded
    ));

    for table in [
        "settlements",
        "settlement_events",
        "daily_global_sponsorship",
        "daily_client_sponsorship",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&database.pool)
            .await?;
        assert_eq!(count, 0, "{table} retained a row after rollback");
    }

    let mut first = settlement_for(&client, 5);
    first.reservation_yocto_near = "60".to_owned();
    first.global_daily_budget_yocto_near = "100".to_owned();
    first.client_daily_budget_yocto_near = "100".to_owned();
    assert!(matches!(
        database.store.claim_settlement(&first).await?,
        ClaimOutcome::New(_)
    ));

    let mut second = settlement_for(&client, 6);
    second.reservation_yocto_near = "60".to_owned();
    second.global_daily_budget_yocto_near = "1000".to_owned();
    second.client_daily_budget_yocto_near = "100".to_owned();
    assert!(matches!(
        database.store.claim_settlement(&second).await?,
        ClaimOutcome::BudgetExceeded
    ));

    let mut third = settlement_for(&client, 7);
    third.reservation_yocto_near = "60".to_owned();
    third.global_daily_budget_yocto_near = "100".to_owned();
    third.client_daily_budget_yocto_near = "1000".to_owned();
    assert!(matches!(
        database.store.claim_settlement(&third).await?,
        ClaimOutcome::BudgetExceeded
    ));

    let global_reserved: String =
        sqlx::query_scalar("SELECT reserved_yocto_near::text FROM daily_global_sponsorship")
            .fetch_one(&database.pool)
            .await?;
    let client_reserved: String =
        sqlx::query_scalar("SELECT reserved_yocto_near::text FROM daily_client_sponsorship")
            .fetch_one(&database.pool)
            .await?;
    assert_eq!(global_reserved, "60");
    assert_eq!(client_reserved, "60");
    let settlement_count: i64 = sqlx::query_scalar("SELECT count(*) FROM settlements")
        .fetch_one(&database.pool)
        .await?;
    let event_count: i64 = sqlx::query_scalar("SELECT count(*) FROM settlement_events")
        .fetch_one(&database.pool)
        .await?;
    assert_eq!(settlement_count, 1);
    assert_eq!(event_count, 1);

    database.cleanup().await
}

#[tokio::test]
async fn retry_releases_budget_and_reacquires_current_policy_atomically() -> TestResult {
    let Some(database) = TestDatabase::new().await? else {
        return Ok(());
    };
    let (_client, settlement) = seeded_store(&database).await?;
    assert!(matches!(
        database.store.claim_settlement(&settlement).await?,
        ClaimOutcome::New(_)
    ));

    database
        .store
        .mark_awaiting_retry(settlement.id, "rpc_preflight_unavailable")
        .await?;
    let dormant = database
        .store
        .settlement(settlement.id)
        .await?
        .ok_or_else(|| std::io::Error::other("dormant settlement disappeared"))?;
    assert_eq!(dormant.state, SettlementState::AwaitingRetry);
    assert_eq!(dormant.reserved_yocto_near, "0");
    assert_eq!(dormant.attempt_count, 1);
    assert_eq!(
        dormant.retry_code.as_deref(),
        Some("rpc_preflight_unavailable")
    );
    assert!(database.store.nonterminal_settlements().await?.is_empty());
    let dormant_summary = database.store.journal_summary().await?;
    assert_eq!(dormant_summary.reserved, 0);
    assert_eq!(dormant_summary.prepared, 0);
    assert_eq!(dormant_summary.submitted, 0);

    let budget_after_release = database.store.global_sponsorship_usage_today().await?;
    assert_eq!(budget_after_release.reserved_yocto_near, "0");
    assert_eq!(budget_after_release.spent_yocto_near, "0");

    let denied = RetryReservation {
        settlement_id: settlement.id,
        policy_snapshot: json!({"policy": "denied"}),
        reservation_yocto_near: RESERVATION.to_owned(),
        global_daily_budget_yocto_near: GLOBAL_LIMIT.to_owned(),
        client_daily_budget_yocto_near: "99".to_owned(),
    };
    assert!(matches!(
        database.store.resume_awaiting_retry(&denied).await?,
        RetryOutcome::BudgetExceeded
    ));
    let still_dormant = database
        .store
        .settlement(settlement.id)
        .await?
        .ok_or_else(|| std::io::Error::other("budget-denied settlement disappeared"))?;
    assert_eq!(still_dormant.state, SettlementState::AwaitingRetry);
    assert_eq!(still_dormant.policy_snapshot, settlement.policy_snapshot);
    assert_eq!(still_dormant.attempt_count, 1);
    let budget_after_denial = database.store.global_sponsorship_usage_today().await?;
    assert_eq!(budget_after_denial.reserved_yocto_near, "0");

    let refreshed_policy = json!({"policy": "current", "revision": 2});
    let resumed = RetryReservation {
        settlement_id: settlement.id,
        policy_snapshot: refreshed_policy.clone(),
        reservation_yocto_near: "60".to_owned(),
        global_daily_budget_yocto_near: GLOBAL_LIMIT.to_owned(),
        client_daily_budget_yocto_near: CLIENT_LIMIT.to_owned(),
    };
    let RetryOutcome::Resumed(record) = database.store.resume_awaiting_retry(&resumed).await?
    else {
        return Err(std::io::Error::other("retry did not reacquire its reservation").into());
    };
    assert_eq!(record.state, SettlementState::Reserved);
    assert_eq!(record.reserved_yocto_near, "60");
    assert_eq!(record.policy_snapshot, refreshed_policy);
    assert_eq!(record.attempt_count, 2);
    assert_eq!(record.retry_code, None);
    let active = database.store.nonterminal_settlements().await?;
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, settlement.id);
    let resumed_summary = database.store.journal_summary().await?;
    assert_eq!(resumed_summary.reserved, 1);
    let budget_after_resume = database.store.global_sponsorship_usage_today().await?;
    assert_eq!(budget_after_resume.reserved_yocto_near, "60");
    assert_eq!(budget_after_resume.spent_yocto_near, "0");

    let states: Vec<String> = sqlx::query_scalar(
        "SELECT to_state FROM settlement_events WHERE settlement_id = $1 ORDER BY id",
    )
    .bind(settlement.id)
    .fetch_all(&database.pool)
    .await?;
    assert_eq!(states, vec!["reserved", "awaiting_retry", "reserved"]);

    database.cleanup().await
}

#[tokio::test]
async fn evm_retry_stays_dormant_while_another_settlement_owns_signer() -> TestResult {
    let Some(database) = TestDatabase::new().await? else {
        return Ok(());
    };
    let (client, _) = seeded_store(&database).await?;
    let first = evm_settlement_for(&client, 40);
    assert!(matches!(
        database.store.claim_settlement(&first).await?,
        ClaimOutcome::New(_)
    ));
    database
        .store
        .mark_awaiting_retry(first.id, "rpc_preflight_unavailable")
        .await?;

    let second = evm_settlement_for(&client, 41);
    assert!(matches!(
        database.store.claim_settlement(&second).await?,
        ClaimOutcome::New(_)
    ));
    let retry = RetryReservation {
        settlement_id: first.id,
        policy_snapshot: json!({"policy": "current"}),
        reservation_yocto_near: RESERVATION.to_owned(),
        global_daily_budget_yocto_near: GLOBAL_LIMIT.to_owned(),
        client_daily_budget_yocto_near: CLIENT_LIMIT.to_owned(),
    };
    assert!(matches!(
        database.store.resume_awaiting_retry(&retry).await?,
        RetryOutcome::SettlementBusy
    ));
    let dormant = database
        .store
        .settlement(first.id)
        .await?
        .ok_or_else(|| std::io::Error::other("signer-blocked retry disappeared"))?;
    assert_eq!(dormant.state, SettlementState::AwaitingRetry);
    assert_eq!(dormant.attempt_count, 1);
    assert_eq!(
        dormant.retry_code.as_deref(),
        Some("rpc_preflight_unavailable")
    );
    let usage = database.store.global_sponsorship_usage_today().await?;
    assert_eq!(usage.reserved_yocto_near, RESERVATION);
    assert_eq!(usage.spent_yocto_near, "0");

    database.cleanup().await
}

#[tokio::test]
async fn lifecycle_terminalization_and_replay_are_durable_and_idempotent() -> TestResult {
    let Some(database) = TestDatabase::new().await? else {
        return Ok(());
    };
    let (_client, settlement) = seeded_store(&database).await?;
    assert!(matches!(
        database.store.claim_settlement(&settlement).await?,
        ClaimOutcome::New(_)
    ));

    let invalid_success = TerminalJournalEntry {
        settlement_id: settlement.id,
        state: SettlementState::Succeeded,
        http_status: 200,
        response_bytes: br#"{"success":true}"#.to_vec(),
        error_code: None,
        error_detail: None,
        gas_burnt: Some("3".to_owned()),
        tokens_burnt: Some("7".to_owned()),
        actual_yocto_near: "7".to_owned(),
        mined_block_number: None,
        mined_block_hash: None,
        confirmations: None,
    };
    assert!(matches!(
        database.store.mark_terminal(&invalid_success).await,
        Err(StoreError::Transition { .. })
    ));

    let prepared = PreparedJournalEntry {
        settlement_id: settlement.id,
        relayer_account_id: "x402-relayer.mike.testnet".to_owned(),
        relayer_public_key: "ed25519:11111111111111111111111111111111".to_owned(),
        relayer_nonce: "42".to_owned(),
        transaction_bytes: vec![1, 2, 3, 4],
        transaction_hash: "transaction-hash".to_owned(),
    };
    database.store.mark_prepared(&prepared).await?;
    database.store.mark_submitted(settlement.id).await?;

    let terminal = TerminalJournalEntry {
        settlement_id: settlement.id,
        state: SettlementState::Succeeded,
        http_status: 200,
        response_bytes: br#"{"success":true,"transaction":"transaction-hash"}"#.to_vec(),
        error_code: None,
        error_detail: None,
        gas_burnt: Some("3".to_owned()),
        tokens_burnt: Some("7".to_owned()),
        actual_yocto_near: "7".to_owned(),
        mined_block_number: None,
        mined_block_hash: None,
        confirmations: None,
    };
    database.store.mark_terminal(&terminal).await?;
    database.store.mark_terminal(&terminal).await?;

    let replay = database.store.claim_settlement(&settlement).await?;
    let ClaimOutcome::Existing(record) = replay else {
        return Err(
            std::io::Error::other("terminal settlement was not replayed as existing").into(),
        );
    };
    assert_eq!(record.state, SettlementState::Succeeded);
    assert_eq!(record.terminal_http_status, Some(200));
    assert_eq!(
        record.terminal_response_bytes.as_deref(),
        Some(terminal.response_bytes.as_slice())
    );
    assert_eq!(
        record.outer_transaction_bytes.as_deref(),
        Some(prepared.transaction_bytes.as_slice())
    );

    let mut mismatched_replay = terminal.clone();
    mismatched_replay.response_bytes = br#"{"success":false}"#.to_vec();
    assert!(matches!(
        database.store.mark_terminal(&mismatched_replay).await,
        Err(StoreError::Transition { .. })
    ));

    let ledger = sqlx::query(
        "SELECT reserved_yocto_near::text AS reserved, spent_yocto_near::text AS spent \
         FROM daily_global_sponsorship",
    )
    .fetch_one(&database.pool)
    .await?;
    assert_eq!(ledger.try_get::<String, _>("reserved")?, "0");
    assert_eq!(ledger.try_get::<String, _>("spent")?, "7");

    let states: Vec<String> = sqlx::query_scalar(
        "SELECT to_state FROM settlement_events WHERE settlement_id = $1 ORDER BY id",
    )
    .bind(settlement.id)
    .fetch_all(&database.pool)
    .await?;
    assert_eq!(
        states,
        vec!["reserved", "prepared", "submitted", "succeeded"]
    );

    database.cleanup().await
}

// An EVM settlement rides the dedicated eip155 columns end to end: the
// reservation satisfies the chain-authorization CHECK with minimal ERC-3009
// metadata + signer address (delegate identity NULL), and the prepare
// transition satisfies the non-terminal-submission CHECK with the signed RLP,
// hash, and account nonce. Any CHECK violation would surface here as a failed
// insert/update — the regression gate for the multichain journal constraints.
#[tokio::test]
async fn evm_reservation_and_prepare_populate_dedicated_columns() -> TestResult {
    let Some(database) = TestDatabase::new().await? else {
        return Ok(());
    };
    let client = ApiClient {
        id: Uuid::new_v4(),
        name: "evm-test-client".to_owned(),
        environment: "testnet".to_owned(),
        daily_budget_yocto_near: CLIENT_LIMIT.to_owned(),
        verify_rate_per_minute: 60,
        settle_rate_per_minute: 10,
    };
    database
        .store
        .create_client(&client, Uuid::new_v4(), "evm-test", &[9; 32])
        .await?;
    let settlement = evm_settlement_for(&client, 9);

    // Reservation writes the single-use anchor, minimal authorization metadata,
    // and signer address; the delegate identity stays NULL.
    let ClaimOutcome::New(reserved) = database.store.claim_settlement(&settlement).await? else {
        return Err(std::io::Error::other("evm reservation was not new").into());
    };
    assert_eq!(reserved.state, SettlementState::Reserved);
    assert_eq!(reserved.chain_kind, ChainKind::Eip155);
    assert_eq!(reserved.anchor_scope, settlement.anchor_scope);
    assert_eq!(reserved.anchor_value, settlement.anchor_value);
    assert_eq!(
        reserved.authorization_metadata,
        settlement.authorization_metadata
    );
    assert_eq!(reserved.signer_address.as_deref(), Some(EVM_SIGNER));
    assert_eq!(reserved.submitted_tx_rlp, None);

    // Prepare: the durable signed transaction into the dedicated columns.
    let prepared = EvmPreparedJournalEntry {
        settlement_id: settlement.id,
        signer_account_nonce: "7".to_owned(),
        submitted_tx_rlp: vec![0x02, 0xf8, 0x6b, 0x01, 0x02],
        submitted_tx_hash: "0xabc0000000000000000000000000000000000000000000000000000000000def"
            .to_owned(),
        required_confirmations: 2,
    };
    database.store.mark_prepared_evm(&prepared).await?;

    let record = database
        .store
        .settlement(settlement.id)
        .await?
        .ok_or_else(|| std::io::Error::other("prepared evm settlement disappeared"))?;
    assert_eq!(record.state, SettlementState::Prepared);
    assert_eq!(
        record.submitted_tx_rlp.as_deref(),
        Some(prepared.submitted_tx_rlp.as_slice())
    );
    assert_eq!(
        record.submitted_tx_hash.as_deref(),
        Some(prepared.submitted_tx_hash.as_str())
    );
    assert_eq!(record.signer_address.as_deref(), Some(EVM_SIGNER));
    assert_eq!(record.signer_account_nonce.as_deref(), Some("7"));
    assert_eq!(record.required_confirmations, Some(2));
    assert_eq!(
        database
            .store
            .raise_required_confirmations(settlement.id, 4)
            .await?,
        4
    );
    assert_eq!(
        database
            .store
            .raise_required_confirmations(settlement.id, 2)
            .await?,
        4
    );
    let raised = database
        .store
        .settlement(settlement.id)
        .await?
        .ok_or_else(|| std::io::Error::other("raised evm settlement disappeared"))?;
    assert_eq!(raised.required_confirmations, Some(4));
    // The NEAR journal columns stay untouched on an EVM row.
    assert_eq!(record.outer_transaction_bytes, None);
    assert_eq!(record.relayer_account_id, None);

    // Re-preparing a non-reserved row is a rejected transition (idempotence guard).
    assert!(matches!(
        database.store.mark_prepared_evm(&prepared).await,
        Err(StoreError::Transition { .. })
    ));

    database.cleanup().await
}

#[tokio::test]
async fn evm_success_requires_depth_and_signer_nonce_is_never_reused() -> TestResult {
    let Some(database) = TestDatabase::new().await? else {
        return Ok(());
    };
    let (client, _) = seeded_store(&database).await?;
    let first = evm_settlement_for(&client, 30);
    assert!(matches!(
        database.store.claim_settlement(&first).await?,
        ClaimOutcome::New(_)
    ));
    let prepared = EvmPreparedJournalEntry {
        settlement_id: first.id,
        signer_account_nonce: "7".to_owned(),
        submitted_tx_rlp: vec![0x02, 0xf8, 0x01],
        submitted_tx_hash: format!("0x{}", hex::encode([0x30; 32])),
        required_confirmations: 2,
    };
    database.store.mark_prepared_evm(&prepared).await?;
    database.store.mark_submitted(first.id).await?;

    let insufficient = TerminalJournalEntry {
        settlement_id: first.id,
        state: SettlementState::Succeeded,
        http_status: 200,
        response_bytes: br#"{"success":true}"#.to_vec(),
        error_code: None,
        error_detail: None,
        gas_burnt: None,
        tokens_burnt: None,
        actual_yocto_near: "7".to_owned(),
        mined_block_number: Some("100".to_owned()),
        mined_block_hash: Some(format!("0x{}", hex::encode([0x31; 32]))),
        confirmations: Some(1),
    };
    assert!(matches!(
        database.store.mark_terminal(&insufficient).await,
        Err(StoreError::Database(_))
    ));
    let still_submitted = database
        .store
        .settlement(first.id)
        .await?
        .ok_or_else(|| std::io::Error::other("depth-rejected settlement disappeared"))?;
    assert_eq!(still_submitted.state, SettlementState::Submitted);
    let usage = database.store.global_sponsorship_usage_today().await?;
    assert_eq!(usage.reserved_yocto_near, RESERVATION);
    assert_eq!(usage.spent_yocto_near, "0");

    let terminal = TerminalJournalEntry {
        confirmations: Some(2),
        ..insufficient
    };
    database.store.mark_terminal(&terminal).await?;

    let second = evm_settlement_for(&client, 31);
    assert!(matches!(
        database.store.claim_settlement(&second).await?,
        ClaimOutcome::New(_)
    ));
    let reused_nonce = EvmPreparedJournalEntry {
        settlement_id: second.id,
        signer_account_nonce: prepared.signer_account_nonce,
        submitted_tx_rlp: vec![0x02, 0xf8, 0x02],
        submitted_tx_hash: format!("0x{}", hex::encode([0x32; 32])),
        required_confirmations: 2,
    };
    assert!(matches!(
        database.store.mark_prepared_evm(&reused_nonce).await,
        Err(StoreError::Database(_))
    ));
    let still_reserved = database
        .store
        .settlement(second.id)
        .await?
        .ok_or_else(|| std::io::Error::other("nonce-rejected settlement disappeared"))?;
    assert_eq!(still_reserved.state, SettlementState::Reserved);

    let mut other_network = evm_settlement_for(&client, 32);
    move_evm_settlement_to_mainnet(&mut other_network);
    assert!(matches!(
        database.store.claim_settlement(&other_network).await?,
        ClaimOutcome::New(_)
    ));
    let same_nonce_other_network = EvmPreparedJournalEntry {
        settlement_id: other_network.id,
        signer_account_nonce: "7".to_owned(),
        submitted_tx_rlp: vec![0x02, 0xf8, 0x03],
        submitted_tx_hash: format!("0x{}", hex::encode([0x33; 32])),
        required_confirmations: 2,
    };
    database
        .store
        .mark_prepared_evm(&same_nonce_other_network)
        .await?;

    database.cleanup().await
}
