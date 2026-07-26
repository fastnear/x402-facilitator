# Repository instructions

These instructions apply recursively to the entire repository. Do not add
nested `AGENTS.md` files.

## Purpose

Build and operate a production Rust facilitator for x402 `exact` Circle USDC
payments on two chain families — NEAR (`near:testnet`, `near:mainnet`; the
flagship integration) and Base/EVM (`eip155:8453`, `eip155:84532`) — through
one chain-neutral durable settlement engine. Keep the reusable chain
mechanisms separable from the production HTTP, policy, and persistence
boundary.

Two x402 wire dialects exist. **v2 is canonical**: every internal type, the
journal fingerprint, and all tests speak v2. The legacy v1 wire is accepted
only behind the `accept_v1` config gate (eip155 only) and is strictly
translated to canonical v2 at the parse boundary (`src/v1_compat.rs`) before
anything else runs — one settlement identity per payment regardless of
dialect. Never fork behavior on dialect deeper than parse and response
formatting.

The authoritative behavior is, in order:

1. The upstream x402 v2 core specification and the `exact` scheme
   specifications for NEAR and EVM; for the gated legacy dialect, the
   upstream v1 transport specification.
2. Interoperability fixtures generated from the pinned official `@x402/*`
   TypeScript packages.
3. This repository's documented launch policy.

Do not silently diverge from the first two to make a test or partner payload
pass. Record and resolve the incompatibility.

## Repository map

- `crates/x402-near-facilitator` — Axum HTTP boundary (`service.rs`), strict
  request parsing (`protocol.rs`), legacy-v1 translation (`v1_compat.rs`),
  chain-neutral engine and the `ChainProvider` seam (`chain.rs`), PostgreSQL
  journal (`store.rs`), auth, config, leadership, telemetry, and the
  `x402-near-admin` binary.
- `crates/x402-chain-near` — the NEAR mechanism (NEP-366, RPC, receipts).
- `crates/x402-chain-eip155-provider` — the EVM provider (upstream verify +
  durable submit/confirm/reorg reconciliation).
- `migrations/` — forward-only SQL, applied by the admin binary only.
- `deploy/` — systemd/nginx/config templates, release install and promote
  scripts, and the demo resource-server deployment (`deploy/demo/`).
- `examples/resource-server/` — the Node reference workload behind every
  public demo (dual-emit 402, legacy v1 acceptance shim, delivery journal).
- `docs/` — architecture, configuration, runbook, threat model, OpenAPI,
  launch checklist, and dated evidence in `docs/evidence/`.
- `fuzz/` — parser fuzz targets (separate workspace; keep its pins in sync
  with the workspace version).
- `scripts/check.sh` — the full local gate (fmt, clippy `-D warnings`,
  workspace tests, deny/audit, config lint, release guard, docs checks). Run
  it plus `git diff --check` before committing.

The architecture seam is deliberate: the engine speaks neutral value types
and dispatches through the `ChainProvider` **enum** (not trait objects; the
chain set is closed and providers keep rich typed results — rationale in
`docs/evm-v2-design.md`). A new chain is an additive provider crate plus an
enum arm; do not weaken the neutral types to chain-specific ones.

## Scope boundaries

Chain-neutral:

- Scheme `exact` only. One pinned network and one configured Circle USDC
  contract per process. Never infer a network from a signer key or accept a
  client-selected RPC endpoint.
- Permit only exact client/network/asset/payee policy rows. No wildcards.
- API-key authentication precedes body parsing on `/verify` and `/settle`.
- `accept_v1` is the only wire-dialect gate: default off, eip155 only,
  rejected at config validation for NEAR chain kinds. Never emit a fabricated
  v1 shape for networks v1 never covered.

NEAR (`near:*`):

- Classic NEP-366 and NEP-141 only. Reject native NEAR, multiple actions,
  non-`ft_transfer` calls, attached deposit other than 1 yoctoNEAR, gas over
  30 TGas, FunctionCall or gas-key payer permissions, ML-DSA, and DelegateV2.
- Use a full-access Transaction V0 relayer. Gas-key relayers require a
  separate protocol and security review.

EVM (`eip155:*`):

- Base only at launch: `eip155:8453` and `eip155:84532`, each bound to its
  canonical Circle USDC contract at config validation — a testnet deploy can
  never point at mainnet USDC or vice versa.
- ERC-3009 `transferWithAuthorization` only, verified through the pinned
  upstream `x402-chain-eip155` implementation; the EIP-712 domain comes from
  the requirements `extra` and must be the token's real domain (Base mainnet
  USDC is `"USD Coin"`/`"2"`, not the symbol).
- `required_confirmations ≥ 1` is the reorg-safety policy: a settlement is
  terminal only at depth, and a mined transaction that disappears before
  depth returns to nonterminal — never assume finality from one receipt.

## Settlement invariants

Chain-neutral:

- Verify the signature before using the claimed payer for policy, telemetry,
  or responses.
- Fail closed on unknown or ambiguous RPC results, on both dialects and both
  chains. Indeterminate evidence keeps a row nonterminal and readiness false;
  callers get a retryable 503, never a guessed x402 result.
- `/settle` performs verification again after it owns the durable settlement
  claim.
- Every payment has a chain-enforced single-use anchor recorded in the
  journal: the domain-prefixed hash of the exact decoded signed delegate
  bytes on NEAR, the ERC-3009 authorization nonce on eip155. It is globally
  single-use in the settlement journal.
- Reserve sponsorship budget and claim settlement in the same database
  transaction.
- Persist the exact signed submission bytes and hash before broadcast. Never
  create a replacement transaction for an indeterminate submission — recovery
  reconciles by the stored hash only.
- Never TTL-delete nonterminal settlement records. Reconcile them on startup
  before readiness becomes true.
- The journal fingerprint is computed over the canonical v2 request value, so
  a payment retried in either dialect deduplicates to one settlement.

NEAR:

- Read chain state at finality and pin a final block across preflight queries
  where the RPC permits it.
- Serialize the relayer from the final nonce read through terminal outcome.
- Success requires the unique inner token receipt to finish with
  `SuccessValue`. Outer transaction or delegate-receipt success is
  insufficient.
- If the relayer nonce advanced while the stored transaction is unknown on
  both independent RPCs, quarantine the relayer and fail readiness.

EVM:

- Terminality requires the configured confirmation depth against the stored
  transaction identity; reconciliation re-queries depth and demotes
  mined-then-missing transactions to nonterminal.
- Readiness includes the expected `eth_chainId` and the signer's gas balance
  hard stop.

## Security and privacy

- Never commit, print, log, trace, snapshot, fixture, or paste a real API
  key, funded private key (ED25519 or secp256k1), credentialed database URL,
  telemetry key, live signed delegate or ERC-3009 authorization, or funded
  wallet credential.
- The sole key-material exception is the checked-in interoperability fixture
  generator: its deterministic public test keys must be labeled `DO NOT
  FUND`, used only for impossible/expired fixture accounts, and never reused
  outside fixture generation.
- Treat signed payment authorizations (NEP-366 delegates, ERC-3009
  authorizations) as sensitive bearer instruments even after they expire.
  Persist only fields required for safe replay protection and
  reconciliation.
- Mark authentication headers as sensitive before tracing middleware runs.
- Metric labels must be bounded and low-cardinality. Account IDs, addresses,
  payment identifiers, transaction hashes, and authorization hashes are not
  labels.
- Show raw API key material exactly once at creation. Store an HMAC-SHA256
  digest using a separately provisioned server pepper and compare in
  constant time.
- Production secrets enter through systemd `LoadCredential` or an equivalent
  secret file. Production `.env` files are prohibited.
- Migrations use a separate privileged database role. The service role
  cannot create or alter schema.
- Do not reuse credentials, DNS or cloud API tokens, databases, relayer
  keys, or EVM signer keys from any other service.

## Network and funds safety

Local tests, mocked RPC tests, read-only RPC calls (including `/verify`
against a live instance), and test database work are allowed. Do not create
accounts or keys, alter DNS, deploy services, issue production API keys, or
broadcast a transaction merely because a command or script exists in this
repository.

Before every funded broadcast, require an explicit human confirmation
showing:

- network;
- asset contract and atomic amount;
- payer;
- recipient;
- relayer or signer;
- expected maximum sponsored gas (NEAR or ETH).

Mainnet confirmation must occur immediately before broadcast and cannot be
reused for a retry. An indeterminate broadcast is reconciled by its stored
hash; it is never retried by signing a new transaction.

## Engineering conventions

- Keep `#![forbid(unsafe_code)]` and workspace lints intact.
- Use typed protocol/RPC errors. Do not determine account, key, method, or
  receipt status by substring matching.
- Parse decimal token amounts as integers and reject lossy or permissive
  JSON. The HTTP boundary denies unknown fields at every level, in both
  dialects.
- Keep protocol rejection separate from malformed HTTP, authentication,
  policy, quota, and infrastructure errors.
- Write forward-only SQL migrations. Production startup must not migrate.
- Every concurrency or recovery fix needs a deterministic regression test.
- Keep generated TypeScript oracle tooling in development/test scope; the
  production binary must not depend on Node.
- The demo's legacy-v1 acceptance injects an `accepted` object that must
  byte-match what the pinned `@x402` middleware computes; treat any
  `@x402/*` version bump as requiring the matcher-contract check in
  `examples/resource-server/legacy-v1.test.mjs` plus a live 402 comparison.
- Update OpenAPI, configuration examples, runbook, and threat model when an
  externally visible behavior or operational dependency changes.

Run `./scripts/check.sh` and `git diff --check` before committing. Do not
describe a deployment, transaction, alert, or partner integration as
complete without a dated evidence link under `docs/evidence/`.
