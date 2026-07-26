# EVM v2 engineering design — the provider seam

> **Status: implemented and shipped.** Phase 0 landed as v0.2.0, the EVM
> provider and Base instances as v0.3.0, and the gated legacy-v1 wire as
> v0.4.0. This document is the design record; current behavior is described
> in [architecture.md](architecture.md) and the dated entries under
> [evidence/](evidence/).

Executable design for Phase 0 of the EVM-support plan (see the approved plan
and `docs/near-intents-adoption-gates.md` for scope). This turns "introduce a
provider seam and route EVM through the hardened journal" into concrete types
and a safe increment sequence. **Phase 0 changes NEAR behavior in no way; it is
a pure refactor plus additive schema, shipped as v0.2.0 and drilled before any
EVM code (Phase 1) lands.**

## Why an enum, not `dyn`

The settlement engine (`service.rs` `run_new_settlement`, `finalize_outcome`,
`reconcile_prepared`, `validate_stored_transaction`) currently calls inherent
methods on a concrete `Arc<NearChainProvider>` and threads NEAR primitives
(`AccountId`, `PublicKey`, `CryptoHash`, `SignedDelegateAction`,
`FinalExecutionOutcomeView`, `TransactionLookup`). To make EVM ride the same
engine, the engine must speak **neutral value types**. A single erased provider
would need those types to be concrete (object safety rules out associated
types), and the chain set is closed (NEAR, eip155). Upstream x402-rs uses the
same shape (a `ChainProvider` enum in its `facilitator/chain.rs`). So:

```rust
// crates/x402-near-facilitator/src/chain.rs (new)
pub enum ChainProvider {
    Near(x402_chain_near::NearChainProvider),
    // Evm(x402_chain_eip155_provider::Eip155Provider)  // added in Phase 1
}
```

`AppState.provider: Arc<ChainProvider>`. Every engine call goes through inherent
`ChainProvider` methods that return the neutral types below and `match` inward.

## Neutral value types (engine boundary)

Replacing the NEAR-specific returns. Fields chosen to satisfy both the journal
(`PreparedJournalEntry`, `SettlementRecord`) and the reconcile logic.

```rust
pub struct VerifiedPayment {
    pub payer: String,                 // NEAR account id / EVM 0x from
    pub payment_hash: [u8; 32],        // canonical per-chain payload hash
    pub requirements: VerifiedRequirements, // asset, pay_to, amount, network (neutral strings + u128)
    pub detail: VerifiedDetail,        // enum { Near(near::VerifiedPayment), Evm(evm::Verified) }
}

pub struct SignerHead {
    pub chain_block_height: u64,       // NEAR block height / EVM block number
    pub chain_block_ref: String,       // NEAR block hash / EVM "" (unused)
    pub signer_nonce: u128,            // NEAR access-key nonce / EVM account nonce
    pub signer_id: String,             // relayer account / EVM signer 0x
    pub signer_balance_atomic: u128,   // yoctoNEAR / wei (gas balance)
}

pub struct Prepared {
    pub submit_bytes: Vec<u8>,         // Borsh SignedTransaction / RLP signed tx
    pub submit_hash: String,           // NEAR CryptoHash / EVM 0x tx hash
    pub signer_id: String,
    pub signer_public_key: String,     // NEAR ed25519 pubkey / EVM "" (address is signer_id)
    pub signer_nonce: u128,
    pub detail: PreparedDetail,        // chain-specific durable extras
}

pub enum BroadcastOutcome {
    Terminal(TerminalOutcome),         // NEAR fast-finality success in one shot
    Rejected(String),                  // deterministic on-chain/relayer rejection
    Pending,                           // EVM always lands here → reconcile confirms
}

pub struct StatusOutcome {             // from query_status during reconcile
    pub state: StatusState,            // Unknown | Pending | Mined{confs,block} | Final
    pub terminal: Option<TerminalOutcome>,
}

pub struct TerminalOutcome {
    pub success: bool,
    pub tx_hash: String,
    pub recipient_delta_atomic: Option<u128>, // for evidence
    pub fee_atomic: u128,                      // gas_burnt/tokens_burnt (NEAR) / gas*price (EVM)
    pub evidence: TerminalEvidence,            // chain-specific receipt/log locus
}
```

The `*Detail` enums carry chain-specific state the same provider reconstructs
(e.g. NEAR's `SignedDelegateAction`, EVM's EIP-3009 authorization + `log_index`)
without leaking primitives to the engine.

## ChainProvider method surface (what the engine needs)

Derived from `run_new_settlement` (`service.rs:1144-1294`) and reconcile
(`1620-1868`):

- `verify(&VerifyRequest, &policy) -> Result<VerifiedPayment, VerifyFailure>`
  where `VerifyFailure` exposes `reason() -> &str`, `payer_attributable()`, and
  an `is_rpc_ambiguous()` classifier (today `verification_is_rpc_ambiguous`).
- `signer_head() -> Result<SignerHead, ChainError>` (replaces `relayer_status`
  + `RelayerHead`; carries balance for readiness).
- `prepare(&VerifiedPayment, &SignerHead) -> Result<Prepared, ChainError>`.
- `broadcast(&Prepared) -> Result<BroadcastOutcome, ChainError>`.
- `query_status(submit_hash, signer_id, signer_nonce, min_confs) -> Result<StatusOutcome, ChainError>`
  (primary) and a `_backup` variant (NEAR dual-RPC; EVM may reuse primary).
- `validate_stored_submit(bytes, expected_hash) -> Result<(), ChainError>`
  (the "exact bytes, deterministic hash" reconcile guard).
- `readiness_probe() -> RpcReadiness` (chain-id match + liveness; NEAR checks
  both RPCs, EVM checks `eth_chainId`/`eth_blockNumber`).
- `signer_addresses()`, `chain_id()` (already the generic `ChainProviderOps`).

The NEAR-specific **nonce-quarantine** (`service.rs:1222-1243`) and
**block-height-expiry** reconcile branches become methods that the NEAR impl
implements and the EVM impl no-ops (EVM exactly-once is the on-chain EIP-3009
nonce; reorg is handled by confirmation depth, below).

### As built (Phase 0, increments 1–2)

The neutral seam shipped as a `ChainProvider` enum (not `dyn`), and the engine in
`service.rs` now speaks only these methods — the `as_near()` bridge is gone from
the engine (it remains solely as a test accessor for staging journal fixtures):

- `verify(&VerifyRequest, &VerificationPolicy) -> Result<VerifiedPayment, VerifyRejection>`;
  `VerifyRejection { reason: &'static str, rpc_ambiguous: bool }`.
- `signer_head()` / `backup_signer_head() -> Result<SignerHead, NearRpcError>`
  (backup carries height + nonce only; balance is unused there).
- `prepare(&VerifiedPayment, &SignerHead) -> Result<Prepared, PrepareError>`.
- `broadcast(&Prepared, &VerifiedPayment) -> BroadcastOutcome`.
- `reconcile_status(submit_hash, signer, payer, asset) -> ReconcileStatus`
  and `rebroadcast(bytes, submit_hash, payer, asset) -> BroadcastOutcome`.
- `readiness_probe() -> bool`, `signer_account_id()`, `signer_public_key()`.

**Decision — dual-RPC + conflict live in the provider.** Rather than the engine
calling `query_status` twice and comparing, `reconcile_status` performs both RPC
queries, compares the two *raw* final outcomes for integrity, and interprets the
receipt graph, returning one neutral `ReconcileStatus { verdict, rpc_failover }`
(`verdict`: Terminal | Indeterminate | Pending | Unknown | Conflict | Ambiguous).
Rationale: NEAR's byte-for-byte raw-outcome equality is the integrity check we
must preserve exactly in Phase 0, and comparing *interpreted* neutral outcomes
would subtly weaken it; EVM's cross-check is confirmation-depth, also provider
-specific. `validate_stored_transaction` stays in the engine for now (it is a
pure function over the record + bytes, not a bridge user); it moves behind the
provider when the EVM RLP/EIP-3009 validator lands in Phase 1.

The identity-mismatch and receipt-indeterminate tracing events collapse to a
single `*_indeterminate` event carrying the reason; the settlement stays
submitted and the outer reconcile loop recomputes readiness from the remaining
nonterminal set, so the final readiness state is unchanged.

## EVM settlement specifics (Phase 1)

- **verify**: reuse `x402-chain-eip155`'s `V2Eip155Exact` handler wholesale
  (EIP-712 domain, EIP-1271/6492, balance). It gives us verify; we own submit.
- **prepare/broadcast**: build + sign the `transferWithAuthorization` tx via
  `alloy` from our funded EOA at a pinned account nonce; `submit_bytes` = RLP,
  `submit_hash` = tx hash. `broadcast` submits and returns `Pending` (never
  Terminal — we do not trust 1 confirmation).
- **confirmation depth / reorg**: reconcile calls `query_status`; a terminal
  `succeeded` is written **only at ≥ N confirmations** (config `confirmations`,
  Base default e.g. 5). `Mined{confs<N}` stays `submitted`. A tx that was
  `Mined` then returns `Unknown` (reorged out) stays `submitted` and is
  re-broadcast-safe only via the **same** signed bytes (nonce unchanged) — never
  re-signed. Revert → `Rejected`.
- **exactly-once**: the EIP-3009 authorization nonce is single-use on-chain, so
  a re-submit of the same bytes is idempotent at the token contract.

### As built — increment 5b (durable submit core)

`crates/x402-chain-eip155-provider/src/prepare.rs` is the offline, deterministic
heart of the durable path (no RPC, no clock, golden-tested):

- **calldata**: `transfer_with_authorization_{bytes,vrs}_calldata` encode the two
  ERC-3009 overloads via a local `sol!` binding — `_0` (opaque `bytes` signature,
  for EIP-1271 / EIP-6492 wallets) and `_1` (split `v,r,s`, for EOA payers),
  matching upstream's variant numbering. A local binding (vs. reaching into
  upstream's generated items) keeps the surface stable; the encoding is
  byte-identical because the ABI signature is the ERC-3009 standard, and two
  selector golden tests lock that to `keccak256(sig)[..4]`.
- **overload selection** (`settle.rs`, 5b-ii): `settlement_calldata` classifies
  the payer signature with upstream's `StructuredSignature` — so settle uses the
  *same* interpretation verify did — and maps it: EOA → `(v,r,s)` `_1`; deployed
  smart wallet (EIP-1271) → `bytes` `_0`; counterfactual EIP-6492 → **rejected**
  (`UnsupportedSignature::CounterfactualWallet`), a documented v1 boundary
  (settling it needs an in-tx wallet deploy the durable path does not yet build).
  `eip712_transfer_hash` (the digest the payer signed) doubles as the payment's
  canonical identity / idempotency anchor and the classifier prehash. Golden
  tests: EOA→vrs, opaque→bytes, 6492-shaped→refused, digest determinism +
  domain separation.
- **live provider** (`provider.rs`, 5b-ii-B): `EvmChainProvider` wraps upstream's
  `Eip155ChainProvider` (built via `connect`, which validates the required x402
  contracts on-chain) plus the facilitator `PrivateKeySigner`. `verify` reuses
  upstream's authoritative `verify_eip3009_payment` (domain, balance, simulation),
  guards `asset == configured`, and returns a durable `EvmVerifiedPayment`
  (payer, `payment_hash`, authorization, signature) for the submit path.
  `account_nonce` / `gas_balance_wei` snapshot the signer via RPC; `broadcast_raw`
  performs `eth_sendRawTransaction` and drops the pending-tx watcher (confirmation
  is the reconcile loop's job, never awaited inline → always `Pending`);
  `readiness_probe` checks chain-id + live head. These RPC paths are proven in the
  5e Base Sepolia drills; the offline `classify_verify_error` mapping
  (on-chain failure → ambiguous/503 vs. verification error → definitive reject) is
  unit-tested. **Boundary:** EIP-6492 counterfactual signatures are refused at
  settle; the demo/typical EOA + deployed-wallet paths are covered.
- **prepare + reconcile** (`provider.rs`, completes the settlement logic):
  `prepare` ties the halves together — `settlement_calldata` (offline) + a fresh
  account-nonce snapshot + an EIP-1559 fee estimate (`estimate_eip1559_fees`) +
  the configured `gas_limit` → `sign_settlement_transaction` (offline core) →
  durable `EvmPrepared`. Gas cap is configured (over-provisioning is free; only
  the `gas_limit * max_fee` balance reservation matters); the fee cap is the
  immutable one (caveat above). `reconcile(tx_hash)` looks up the receipt and
  applies the confirmation-depth policy via the pure `classify_confirmations`:
  ≥ `required_confirmations` → `Terminal` (reorg-safe, success or definitive
  revert); `< N` → `Mined`; no receipt → `Unknown` (reorged-out / dropped → engine
  keeps the submission and rebroadcasts the same bytes; the ERC-3009 nonce makes
  that idempotent). `classify_confirmations` is unit-tested at the N boundary and
  for a confirmed revert; the RPC lookups are 5e-live.

### As built — increment 5d-A (engine wiring)

`chain.rs` now dispatches to `ChainProvider::Evm(Box<EvmChainProvider>)` (boxed —
it wraps the alloy stack). Every neutral method has an EVM arm mapping the
provider's types to the neutral vocabulary: `verify`→`VerifiedPayment`/
`VerifiedDetail::Evm` (asset-guarded; `payment_hash` = EIP-712 digest; caip2
network), `prepare` (now `async`, Option A) reuses the journaled head's nonce +
fetches fees, `broadcast`/`rebroadcast`→`Pending`, `reconcile_status` maps
confirmation-depth verdicts (`Terminal`/`Mined`→`Pending`/`Unknown`),
`signer_head`/`backup_signer_head`→one `head()` snapshot (EVM has no backup RPC).
Neutralized for multi-chain: `VerifyRejection.reason` and `PrepareError::Provider`
are now owned `String`; `signer_head` returns a neutral `SignerHeadError`;
`terminal_protocol_failure`/`record_settlement_result` take `&str`. EVM
`signer_public_key` = the address, so the store's relayer-policy keys stay
consistent. All error types are discarded at the engine's call sites
(`map_err(|_| …)`), so neutralizing them is behavior-neutral for NEAR (55 lib
tests unchanged). **Deferred to 5d-B** (needs the Postgres recovery gate): the
`/verify` **reservation** path (`NewSettlement` writes NEAR delegate columns —
EVM must populate the eip155 journal columns) and the **recovery** path
(`reconcile_prepared`/`validate_stored_transaction` are NEAR-coupled: `CryptoHash`/
`AccountId` parse, Borsh decode + sig verify, delegate-height expiry,
nonce-quarantine — EVM needs RLP-decode + signer-recover, no quarantine/expiry).
Until 5d-C wires config/construction, EVM is not constructable, so both deferred
branches are unreachable and guarded (the `/verify` one returns a clear error).

### As built — increment 5d-B (recovery path)

`reconcile_prepared` gains an early-return EVM branch right after the neutral
identity check; the NEAR body below is byte-identical (recovery suite: 13 recovery
+ 4 store + leadership tests unchanged and green). `reconcile_prepared_evm`
validates the journaled RLP via the provider crate's offline
`validate_signed_transaction` (decode + hash-match + secp256k1 signer-recover — the
EVM analog of NEAR's Borsh + signature validation), then resolves by the neutral
verdict: `Terminal`→finalize; `Pending` (mined `< N`, or mempool)→wait;
`Unknown`→re-submit the exact bytes (idempotent via the ERC-3009 nonce;
confirmation depth guards reorg); `Ambiguous`→unready + retry. **No NEAR
nonce-quarantine or delegate-expiry.** Boundary: on `Unknown` we always
rebroadcast (never terminal-fail on nonce-advance) — safe against transient RPC
false-negatives; the degenerate "nonce genuinely consumed by another tx" case
retries benignly (a single serialized signer makes it unreachable in normal
operation). `validate_signed_transaction` is golden-tested (accepts our bytes;
rejects wrong hash / wrong signer / tampering).

### As built — increment 5d-C (durable EVM journal + config + construction)

EVM is now fully constructable and runnable end to end (three commits).

**Design fork (Mike's call): dedicated EVM columns.** The durable journal honors
migration 0002's dedicated eip155 columns (`signer_address`, `submitted_tx_rlp`,
`submitted_tx_hash`, `signer_account_nonce`, `evm_authorization`) as the source of
truth, rather than overloading NEAR's `outer_transaction_*`/`relayer_*` columns.
This keeps NEAR's columns pristine and needs no new migration (0002 was built for
it); the 5d-A/5d-B code that had read the NEAR columns is corrected here. NEAR's
`String` delegate fields stay populated via `COALESCE(delegate_*, '')` in the
SELECTs, so every NEAR read is unchanged while EVM rows carry harmless empties
(never read on the NEAR path — a single DB per instance means the provider kind,
not a per-row flag, selects the reconcile path).

- **store**: `SettlementRecord` gains `signer_address`/`submitted_tx_*`;
  `NewSettlement` gains `chain_kind` + `evm_authorization` + `signer_address` and
  makes `delegate_*` `Option`; `claim_settlement` writes the eip155 authorization
  identity (satisfying 0002's `chain_authorization_check`); new `mark_prepared_evm`
  writes the signed RLP + nonce + `required_confirmations` (satisfying the eip155
  `nonterminal_submission_check`). A DB round-trip test
  (`evm_reservation_and_prepare_populate_dedicated_columns`) is the regression gate
  for the conditional CHECKs.
- **engine**: the reservation write branches the authorization identity by chain
  (NEAR delegate vs EVM ERC-3009 authorization + signer). `settle_prepared_evm` is
  the EVM forward tail — journal → `mark_submitted` → broadcast, with leadership
  re-checked before the durable transition and before the broadcast, but **no NEAR
  nonce recheck/quarantine** (idempotent via the single-use ERC-3009 nonce).
  `reconcile_prepared` takes its EVM branch *before* the NEAR relayer-identity guard
  (which reads NEAR-only columns) and guards the journaled `signer_address` itself.
- **config**: `validate_eip155` + `Eip155Config { chain_id, required_confirmations,
  gas_limit }` (an optional, defaulted block, so NEAR configs parse unchanged). It
  binds the deployment tier to a specific Base chain (mainnet 8453 / Sepolia 84532),
  the canonical Circle USDC per chain, `network == eip155:<chain_id>`, a `0x` signer
  address, `required_confirmations >= 1`, a sane `gas_limit` band, and
  `max_inner_gas == 0` (a NEAR-only ceiling, sentinel for EVM).
- **binary**: `main.rs` branches construction on `chain_kind`. EVM parses the
  secp256k1 signer (a mode-0600 credential, **never logged** — a parse failure
  never carries key material) and the `0x` asset via
  `EvmChainProvider::connect_from_config`, then `ChainProvider::Evm(Box::new(..))`.
  The relayer/signer-identity upsert is unified through the neutral
  `signer_account_id`/`signer_public_key` accessors. The `ChainKind::Near` build
  guard is removed.

**HTTP surface — why EVM does not register a `FacilitatorLocal`.** The upstream
`V2Eip155Exact` scheme builder is *generic* (`impl<P> X402SchemeFacilitatorBuilder<P>
for V2Eip155Exact where P: Eip155MetaTransactionProvider + ChainProviderOps +
'static`). The `SchemeBlueprints`/`and_register` assembly requires
`for<'a> X402SchemeFacilitatorBuilder<&'a P>`, i.e. `&'a P: … + 'static` under a
higher-ranked lifetime — which is unsatisfiable for any concrete provider (a
`&'a P` is not `'static`). NEAR works only because our local `x402-chain-near`
provides a *concrete* `impl X402SchemeFacilitatorBuilder<&NearChainProvider> for
V2NearExact` with no `'static` bound. Rather than fight this (a newtype hits the
same `'static` wall), EVM serves its read-only surface through the neutral
provider: `AppState.facilitator` is `Option` (`None` for EVM); `/supported`
synthesizes the single eip155 exact kind + signer address (`evm_supported`);
`/verify` runs `provider.verify` (`evm_verify`); and the settle verify-gate (NEAR's
scheme routing, extracted verbatim into `facilitator_verify_gate`) is skipped —
the neutral `provider.verify` already performs the real ERC-3009 verification.

Gate: NEAR byte-identical — 60 lib + 2 admin + 12 provider tests green, clippy
pedantic + deny unwrap/expect/panic clean on both crates. **Deferred to 5e**
(needs live RPC, so not unit-testable): `base-sepolia.json`, the funded secp256k1
signer key file, and the live Base Sepolia integration drills.
- **signing**: `sign_settlement_transaction` builds a `TxEip1559` around the
  calldata and signs it (`signature_hash` → `sign_hash_sync` → `into_signed` →
  `TxEnvelope::Eip1559`), returning `EvmPrepared { tx_hash, signer_address,
  account_nonce, signed_tx_rlp }`. RFC-6979 determinism makes the RLP + hash a
  stable function of the inputs, so the journaled submission and its recovery
  replay are byte-identical; the tx is **never re-signed**. A golden test decodes
  the RLP back and recovers the signer, end-to-end.
- **fee immutability caveat (new)**: an EIP-1559 tx pins `max_fee_per_gas`, so a
  base-fee spike *past the cap* after preparation **strands** the tx (it waits
  for the market to recede) rather than failing it. The cap must be provisioned
  with generous headroom. Fee-bump replacement (re-sign the *same account nonce*
  at a higher cap, superseding the stranded tx) is a deliberate later refinement;
  until then the operational lever is a generous cap plus the existing
  reconcile/rebroadcast loop. This is the one EVM failure mode with no NEAR
  analog (NEAR meta-tx finality does not price gas at submission).

## Journal / migration 0002

One superset schema (one binary → one migration checksum across all instances;
each instance uses its chain's subset). Additive, nullable:

- `settlements`: add `evm_authorization JSONB`, `signer_address TEXT`,
  `signer_account_nonce NUMERIC(20,0)`, `submitted_tx_rlp BYTEA`,
  `submitted_tx_hash TEXT`, `mined_block_number NUMERIC(20,0)`,
  `mined_block_hash TEXT`, `confirmations INTEGER`, `required_confirmations INTEGER`.
- Relax the non-terminal CHECK (`0001` lines 124-133) to be **chain-conditional**:
  NEAR rows require the `delegate_*`/`relayer_*`/`outer_transaction_*` set; EVM
  rows require `signer_address`/`signer_account_nonce`/`submitted_tx_*`.
- Widen `api_clients.environment` CHECK beyond `('mainnet','testnet')`.
- Keep `*_yocto_near` budget column names (documented as chain-native atomic
  units; `NUMERIC(40,0)` already fits wei). Rename deferred.
- Migration applied only by `x402-near-admin migrate` (checksum-gated; the
  service refuses to start on mismatch). Test against the restore drill.

## Config generalization

`ServiceConfig` gains `chain_kind: ChainKind` (`near`|`eip155`) and a
chain-specific block; `validate()` branches. `Environment` (today `{Mainnet,
Testnet}`) generalizes to a `(chain_kind, caip2_network)` identity used for the
api-key label, DB name, and readiness. Relayer-key parse is chain-conditional
(ed25519 vs secp256k1). `deny_unknown_fields` → the struct changes, not just JSON.

## Safe increment sequence (each: `cargo check` + relevant tests green)

1. Add `chain.rs` with neutral types + `ChainProvider::Near` wrapping
   `NearChainProvider` (conversions NEAR→neutral). `#[allow(dead_code)]` until
   step 2 wires it (single transient allow, removed in step 2).
2. Flip `AppState.provider` to `Arc<ChainProvider>`; migrate the engine
   functions to neutral types **in one compilable move** (`run_new_settlement`,
   `finalize_outcome`, `reconcile_prepared`, `validate_stored_transaction`,
   `refresh_chain_readiness`, `fresh_relayer_status`). NEAR behavior identical.
3. Update the NEAR test doubles (`service_recovery_tests.rs` uses `MockRpc`;
   keep the NEAR provider path, adapt call sites to neutral types).
4. Config generalization + migration 0002 + widen 2-env scripts.
5. Full `cargo test` (needs Postgres for store/recovery/leadership suites) +
   CI; testnet drills; v0.2.0; mainnet promote with drilled rollback.

## Regression gate

The NEAR path must stay byte-identical. Gates: `crates/x402-chain-near`
unit/oracle tests, `service_recovery_tests.rs` (crash/recovery matrix),
`store_postgres_tests.rs` (idempotency/budget), `leadership_postgres_tests.rs`,
and the on-host testnet drills (promote/rollback/recovery) before any mainnet
promote. Rollback target stays v0.1.3.
