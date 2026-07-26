# Changelog

All notable changes to the x402 NEAR facilitator are recorded here. The format
follows [Keep a Changelog](https://keepachangelog.com/) and the project uses
[Semantic Versioning](https://semver.org/).

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
