-- Migration 0003: durable retry ownership and chain-enforced payment anchors.
--
-- This migration intentionally removes the full ERC-3009 authorization JSON.
-- It contains a bearer signature and duplicates values already represented by
-- canonical settlement columns. Recovery needs only the signed submission RLP;
-- the authorization's validity window remains as non-sensitive audit metadata.
--
-- Operational rollback boundary: 0.4.x binaries expect the removed column and
-- old state constraint, so they must be stopped before this migration runs.
-- Binary rollback requires restoring a pre-migration database snapshot and
-- separately reconciling every submission made after that snapshot.

ALTER TABLE settlements
    ADD COLUMN anchor_scope TEXT,
    ADD COLUMN anchor_value BYTEA,
    ADD COLUMN authorization_metadata JSONB,
    ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 1
        CHECK (attempt_count >= 1),
    ADD COLUMN retry_code TEXT;

-- Fail loudly before decoding a malformed legacy nonce. All EVM rows written by
-- migration 0002's store boundary contain these three string fields.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM settlements
        WHERE chain_kind = 'eip155'
          AND (
              jsonb_typeof(evm_authorization) IS DISTINCT FROM 'object'
              OR COALESCE(
                  evm_authorization->>'nonce'
                      ~ '^0x[[:xdigit:]]{64}$',
                  false
              ) = false
              OR jsonb_typeof(evm_authorization->'validAfter')
                    IS DISTINCT FROM 'string'
              OR jsonb_typeof(evm_authorization->'validBefore')
                    IS DISTINCT FROM 'string'
          )
    ) THEN
        RAISE EXCEPTION
            'cannot backfill EVM anchor/metadata from malformed evm_authorization';
    END IF;
END $$;

UPDATE settlements
SET anchor_scope = 'near',
    anchor_value = payment_hash
WHERE chain_kind = 'near';

UPDATE settlements
SET anchor_scope =
        network || ':' || lower(asset) || ':' || lower(payer),
    anchor_value =
        decode(substring(evm_authorization->>'nonce' FROM 3), 'hex'),
    authorization_metadata = jsonb_build_object(
        'version', 2,
        'validAfter', evm_authorization->>'validAfter',
        'validBefore', evm_authorization->>'validBefore'
    )
WHERE chain_kind = 'eip155';

ALTER TABLE settlements
    ALTER COLUMN anchor_scope SET NOT NULL,
    ALTER COLUMN anchor_value SET NOT NULL,
    ADD CONSTRAINT settlements_anchor_value_check
        CHECK (octet_length(anchor_value) = 32),
    ADD CONSTRAINT settlements_anchor_scope_check CHECK (
        (
            chain_kind = 'near'
            AND anchor_scope = 'near'
            AND anchor_value = payment_hash
        )
        OR (
            chain_kind = 'eip155'
            AND anchor_scope =
                network || ':' || lower(asset) || ':' || lower(payer)
        )
    ),
    ADD CONSTRAINT settlements_anchor_unique
        UNIQUE (anchor_scope, anchor_value);

-- Replace the old authorization constraint before removing its sensitive
-- column. The retained JSON is deliberately limited to exactly three keys.
ALTER TABLE settlements
    DROP CONSTRAINT settlements_chain_authorization_check,
    ADD CONSTRAINT settlements_chain_authorization_check CHECK (
        (
            chain_kind = 'near'
            AND delegate_public_key IS NOT NULL
            AND delegate_nonce IS NOT NULL
            AND delegate_max_block_height IS NOT NULL
            AND authorization_metadata IS NULL
        )
        OR (
            chain_kind = 'eip155'
            AND signer_address IS NOT NULL
            AND jsonb_typeof(authorization_metadata) = 'object'
            AND authorization_metadata->'version' = '2'::jsonb
            AND jsonb_typeof(authorization_metadata->'validAfter') = 'string'
            AND jsonb_typeof(authorization_metadata->'validBefore') = 'string'
            AND authorization_metadata
                    - ARRAY['version', 'validAfter', 'validBefore']::text[]
                = '{}'::jsonb
        )
    ),
    DROP COLUMN evm_authorization;

-- Awaiting-retry rows have relinquished both their sponsorship reservation and
-- active signer ownership. They are intentionally dormant: only an explicit
-- retry claim can return one to `reserved`.
ALTER TABLE settlements
    DROP CONSTRAINT settlements_state_check,
    ADD CONSTRAINT settlements_state_check CHECK (
        state IN (
            'reserved',
            'awaiting_retry',
            'prepared',
            'submitted',
            'succeeded',
            'failed'
        )
    ),
    DROP CONSTRAINT settlements_nonterminal_submission_check,
    DROP CONSTRAINT settlements_required_confirmations_check,
    ADD CONSTRAINT settlements_nonterminal_submission_check CHECK (
        state IN ('reserved', 'awaiting_retry', 'failed')
        OR (
            chain_kind = 'near'
            AND relayer_account_id IS NOT NULL
            AND relayer_public_key IS NOT NULL
            AND relayer_nonce IS NOT NULL
            AND outer_transaction_bytes IS NOT NULL
            AND outer_transaction_hash IS NOT NULL
        )
        OR (
            chain_kind = 'eip155'
            AND signer_address IS NOT NULL
            AND signer_account_nonce IS NOT NULL
            AND submitted_tx_rlp IS NOT NULL
            AND submitted_tx_hash IS NOT NULL
            AND required_confirmations IS NOT NULL
        )
    ),
    ADD CONSTRAINT settlements_awaiting_retry_check CHECK (
        state <> 'awaiting_retry'
        OR (
            retry_code IS NOT NULL
            AND reserved_yocto_near = 0
            AND relayer_account_id IS NULL
            AND relayer_public_key IS NULL
            AND relayer_nonce IS NULL
            AND outer_transaction_bytes IS NULL
            AND outer_transaction_hash IS NULL
            AND signer_account_nonce IS NULL
            AND submitted_tx_rlp IS NULL
            AND submitted_tx_hash IS NULL
        )
    ),
    ADD CONSTRAINT settlements_required_confirmations_check CHECK (
        required_confirmations IS NULL OR required_confirmations >= 1
    ),
    ADD CONSTRAINT settlements_evm_success_confirmations_check CHECK (
        NOT (chain_kind = 'eip155' AND state = 'succeeded')
        OR (
            confirmations IS NOT NULL
            AND required_confirmations IS NOT NULL
            AND confirmations >= required_confirmations
        )
    );

-- A process has one configured EVM signer per network. Only one settlement may
-- own it while preparing or reconciling a transaction; dormant retry rows do
-- not.
CREATE UNIQUE INDEX settlements_evm_active_signer_idx
    ON settlements (network, lower(signer_address))
    WHERE chain_kind = 'eip155'
      AND state IN ('reserved', 'prepared', 'submitted');

-- A signer account nonce is never reusable on its network, including after
-- terminalization.
CREATE UNIQUE INDEX settlements_evm_signer_nonce_idx
    ON settlements (network, lower(signer_address), signer_account_nonce)
    WHERE chain_kind = 'eip155'
      AND signer_account_nonce IS NOT NULL;

-- DROP COLUMN removes the legacy authorization from SQL visibility but
-- PostgreSQL retains dropped-column bytes until a table rewrite. The admin
-- migration command rewrites the heap and associated TOAST storage with
-- VACUUM FULL after SQLx commits this migration, then changes this marker to
-- `complete`. Service startup rejects a database while the marker is pending,
-- including after a crash between the transactional migration and the
-- out-of-transaction rewrite.
COMMENT ON TABLE settlements IS
    'x402-maintenance:0003-authorization-scrub:pending';
