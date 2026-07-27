# Changelog

All notable changes to the x402 facilitator workspace are recorded here. The
format follows [Keep a Changelog](https://keepachangelog.com/) and the project
uses [Semantic Versioning](https://semver.org/).

## Versioning policy

The workspace and its publishable provider crates use one lockstep version.
Before 1.0, a minor release may intentionally change a public Rust API or
operator contract; each break must be called out here with upgrade guidance.
Patch releases remain backward compatible within the current minor line.

## [Unreleased]

## [0.5.1] - 2026-07-26

### Fixed

- Aligned the OCI image description and its release guard on the canonical
  x402 v2 NEAR-and-Base product identity, with regression coverage that catches
  future Dockerfile/guard drift before a tag is created. There are no runtime,
  wire-contract, configuration, or migration changes from `0.5.0`.

## [0.5.0] - 2026-07-26

This release makes the shared NEAR/Base engine ready for broader reuse and
hardens the durable EVM boundary. It records software changes only; it does not
claim that a particular public instance has been upgraded or canaried.

### Added

- Chain-neutral OpenAPI 3.1 documentation for canonical v2 NEAR and EVM
  requests, the gated EVM-only legacy-v1 branch, protocol responses, discovery,
  and readiness.
- A contributor guide, categorized documentation index, security-aware issue
  forms and pull-request checklist, provider-crate READMEs/notices/licenses,
  and an explicit in-tree chain-extension guide.
- Migration `0003_retry_anchors.sql`, which gives every settlement a durable
  scoped single-use anchor and adds an explicit `awaiting_retry` state for
  retryable failures before signed submission bytes exist.
- Exact EVM signed-transaction validation during recovery, including envelope,
  signer, chain, nonce, token, calldata, authorization, payment hash, gas, and
  fee-policy binding.
- Independent EVM durable reads: both configured endpoints must agree on chain
  identity, pending signer nonce, and mined receipt facts before the engine
  advances.
- A required EVM `max_fee_per_gas_wei` ceiling and Base L1 data-fee estimation
  over the exact signed bytes.
- A public facilitator landing page, reference-instance access workflow, and
  target-specific registry submission documentation.

### Changed

- Pre-broadcast retries now release sponsorship budget and active EVM signer
  ownership atomically. A retry of the same canonical payment reacquires
  current policy and budget in one transaction while retaining its replay
  anchor and attempt history.
- The EVM journal retains only the authorization nonce anchor and minimum
  validity metadata before preparation, instead of the full signed ERC-3009
  authorization. Exact signed RLP remains durable once a transaction exists.
- EVM readiness requires the signer balance to cover the configured hard stop
  plus one full reservation. The reservation must exceed
  `gas_limit × max_fee_per_gas_wei` so Base L1 data fees remain covered.
- EVM sponsorship accounting now reconciles the execution fee plus the Base
  receipt's L1 data fee; malformed or conflicting receipt fee evidence fails
  closed.
- A persisted EVM confirmation requirement may be raised by later
  configuration but never silently lowered for an existing submission.
- Sensitive payer, payee, amount, authorization, signer, nonce, transaction,
  and chain-reference values are redacted from shared settlement `Debug`
  output.
- Post-upstream EVM verification accepts only EOA and deployed EIP-1271
  settlement shapes. Counterfactual EIP-6492 is a definitive
  `unsupported_eip6492` rejection, malformed signatures are
  `invalid_signature`, and upstream failures map to stable typed reason codes
  instead of error-text matching.
- EVM `/supported` now advertises `payment-identifier`, and official-client
  conformance is parameterized by the instance's canonical network.
- Project and container metadata now describe the production NEAR and Base
  facilitator. The historical `x402-near-facilitator` package, binary, and
  deployment names remain for compatibility.

### Upgrade notes

- All workspace packages move in lockstep to `0.5.0`. The service package is
  explicitly non-publishable; the two mechanism/provider crates carry their
  standalone package documentation and legal files.
- Before starting `0.5.0`, apply migration `0003` with the privileged admin
  role. It backfills replay anchors, replaces the legacy full EVM
  authorization column with minimal metadata, and adds retry/signer
  constraints. It fails closed if a legacy EVM authorization is malformed or
  existing rows violate the new anchor, signer-ownership, or account-nonce
  uniqueness rules; reconcile those rows before retrying the migration.
  The admin command then performs an out-of-transaction `VACUUM FULL` table
  rewrite to remove the dropped authorization bytes from the current heap and
  associated TOAST storage, then records a completion marker. Production
  startup still never migrates and rejects a pending marker. Pre-migration
  backups and archived WAL remain sensitive until their reviewed retention
  window ends.
- Migration `0003` is a one-way binary rollback boundary: `0.4.x` does not
  understand the new retry state and expects the dropped
  `evm_authorization` column. Do not point a pre-`0.5.0` binary at a migrated
  database. A pre-migration database restore is safe only when operators can
  prove it does not omit any authorization or broadcast the chain may accept;
  otherwise recover forward.
- Every EVM configuration must add a positive decimal-string
  `eip155.max_fee_per_gas_wei` and keep `sponsorship.reservation_yocto_near`
  greater than `eip155.gas_limit × eip155.max_fee_per_gas_wei`. Existing NEAR
  configurations do not gain a new required field.
- EVM instances now require two distinct RPC URLs. Both endpoints must provide
  the durable head/nonce/receipt fields used by the provider, including Base's
  receipt `l1Fee`; preparation also calls the canonical Base GasPriceOracle.

### Public Rust API changes

As permitted by the pre-1.0 minor-version policy:

- EVM provider connection constructors append the required maximum-fee-per-gas
  argument.
- `EvmVerifiedPayment::authorization_json` is replaced by the minimal
  `authorization_identity` projection.
- Stored-transaction validation now takes `ExpectedEvmSubmission` and returns
  the strictly decoded durable facts.
- Reconciliation accepts the confirmation policy persisted for each record,
  and EVM terminal outcomes expose separate execution and L1 fee fields.

## [0.4.1]

Operational hardening after the first third-party paid traffic on Base
(2026-07-26): public RPC endpoints rate-limit under real paid-flow bursts,
and one throttled call previously surfaced as a client-facing 503 through
the engine's fail-closed ambiguity handling.

### Fixed

- **Bounded retry with backoff for read-only EVM RPC operations** (verify,
  signer head/nonce/balance, fee estimation, reconcile receipt lookups): up
  to two short retries absorb burst throttling without masking real
  outages. Broadcast is deliberately not retried — submission recovery
  remains the journaled reconcile loop's job, which rebroadcasts exact
  stored bytes.
- **Chain parity for the same retry on NEAR's read-only verify paths**: the
  `/verify` endpoint, the settle-time verification gate, and the neutral
  provider's NEAR verification retry ambiguous RPC lookups with the same
  bounded backoff before surfacing a 503. NEAR's dual-RPC reconciliation
  and nonce-quarantine machinery is deliberately untouched.
- Chain-neutral wording for the shared verify-timeout, settle re-verify,
  and settlement-worker unavailability messages, which said "NEAR" on
  eip155 instances.
- `deploy/config/base.json.example` now carries the burst-tolerant public
  RPC pair (`base.drpc.org` primary, Blast API backup) matching the live
  Base instance.

## [0.4.0]

Legacy x402 v1 wire compatibility, gated and off by default. Aimed at
merchants still on the 0.x Coinbase SDKs who point their facilitator URL at
this service. NEAR instances are unaffected: the gate is rejected at config
validation for `near` chain kinds, and v1 never covered NEAR networks.

### Added

- `accept_v1` config flag (default `false`; eip155 only). When enabled,
  `/verify` and `/settle` also accept legacy x402 v1 wire requests —
  top-level `x402Version: 1`, `paymentPayload` with `scheme`/`network`
  (legacy aliases `base` / `base-sepolia`) instead of an `accepted` echo, and
  `maxAmountRequired` in place of `amount`. Requests are strictly translated
  (deny-unknown-keys at every level, mirroring the v2 parser) to the
  canonical v2 shape before the normal pipeline runs, so the durable journal
  fingerprint and every downstream check see one settlement identity
  regardless of wire dialect.
- v1-dialect responses: 200 protocol responses to v1 requests echo `network`
  as the legacy alias (all other emitted fields were already a v1-compatible
  superset — `isValid`/`invalidReason`/`payer`,
  `success`/`errorReason`/`transaction`). Non-200 errors are unchanged.
- `/supported` on gated eip155 instances advertises an additional
  `{"x402Version": 1, "scheme": "exact", "network": "base"}` kind, matching
  the dual-advertising precedent of the x402.org hosted facilitator.
- Fuzz coverage: `parse_http_request` now exercises the v1 sniff/translation
  branch, with a legacy v1 request seeded in the corpus.

## [0.3.0]

Multi-chain settlement. The durable engine is now chain-neutral, and a second
chain — **Base (EVM, `eip155`)** — settles through the same hardened journal.
**NEAR behavior is unchanged**, validated by the full DB-backed suite (recovery,
store, leadership, HTTP) at every step.

### Added

- **EVM (Base) settlement** through a new `x402-chain-eip155-provider` built on
  upstream `x402-chain-eip155` 2.0.2: ERC-3009 `transferWithAuthorization`
  verification (EIP-712), durable submission through our journal, and a
  confirmation-depth policy — a settlement becomes terminal only after N
  confirmations and is re-evaluated if a mined transaction disappears
  (reorg-aware). Exactly-once is anchored on the on-chain EIP-3009 authorization
  nonce. Networks: Base mainnet (`eip155:8453`) and Base Sepolia
  (`eip155:84532`).
- A `chain_kind` configuration discriminator (`near` | `eip155`) with
  chain-specific sub-config, defaulting to `near` so existing NEAR host configs
  parse unchanged. `eip155` instances parse, validate, and serve `/verify`,
  `/settle`, `/supported`, and `/readyz`. Relayer-key parsing is chain-conditional
  (ed25519 for NEAR, secp256k1 for EVM); readiness for an EVM instance checks
  `eth_chainId`, block liveness, and the signer gas balance against the hard-stop.
- Migration `0002`: additive, nullable EVM columns on `settlements`
  (authorization JSON, signer address/nonce, submitted-tx RLP/hash, mined
  block/hash, confirmations) plus chain-conditional integrity CHECKs. Existing
  NEAR rows and the prior binary are unaffected; rollback to `0.1.3` is
  schema-safe on the `0002` schema.

### Changed

- The durable settlement engine (`service.rs`) now speaks a chain-neutral
  `ChainProvider` surface (`verify` / `prepare` / `broadcast` / `reconcile_status`
  / `rebroadcast` / `signer_head`) over neutral value types, instead of threading
  NEAR primitives directly. The concrete NEAR logic — receipt-graph
  interpretation, dual-RPC raw-outcome conflict detection, and exact-byte replay —
  now lives inside the NEAR provider. The engine no longer references a NEAR
  primitive on the settlement path.

### Operational

- Monitoring, backup, and deployment scripts no longer assume exactly two NEAR
  environments: metrics iterate installed instance configs (chain-aware), backups
  discover every `x402_*` instance database, and `promote-release.sh` plus
  deployment verification accept the Base (`base`, `base-sepolia`) instances
  alongside NEAR.

### Notes

- NEAR's public HTTP contract is unchanged: `/supported` advertises the NEAR
  `exact` scheme on NEAR instances and the EVM `exact` scheme on Base instances.
- EVM atomic-unit budget columns reuse the existing `*_yocto_near` names to hold
  wei (documented; a rename is deferred).

## [0.1.3]

Initial hardened NEAR-only release lineage (durable journal, dual-RPC
reconciliation, leadership failover, sponsorship budgets). See git history.

[Unreleased]: https://github.com/fastnear/x402-near-facilitator/compare/v0.5.1...HEAD
[0.5.1]: https://github.com/fastnear/x402-near-facilitator/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/fastnear/x402-near-facilitator/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/fastnear/x402-near-facilitator/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/fastnear/x402-near-facilitator/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/fastnear/x402-near-facilitator/compare/v0.1.3...v0.3.0
[0.1.3]: https://github.com/fastnear/x402-near-facilitator/releases/tag/v0.1.3
