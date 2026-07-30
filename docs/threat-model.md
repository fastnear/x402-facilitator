# Threat model

This model covers the shared facilitator and calls out chain-specific controls
where they differ. The original NEAR controls remain the baseline; the
[EVM and legacy-v1 delta](#evm-eip155-and-legacy-v1-delta) extends them to Base
and the gated v1 wire. Deployment-specific residual risks below describe the
dated public reference topology, not requirements for every self-hoster.

## Protected assets and trust boundaries

The facilitator protects:

- the dedicated relayer/signer private key and its native gas balance;
- payer-authorized Circle USDC transfers;
- merchant binding and API client policy;
- the single-settlement and idempotency guarantees;
- PostgreSQL journal integrity and sponsorship accounting;
- API keys, the HMAC pepper, database credentials, and any telemetry
  credential;
- truthful settlement responses and operational telemetry.

The process trusts its checked-in policy logic, environment-specific config,
PostgreSQL, dedicated relayer credential, and its primary and backup RPC
endpoints. It does not trust callers, signed-payload fields before signature
verification, arbitrary RPC error text, forwarded client IP headers, or outer
transaction success by itself.

## Threats and required controls

| Threat | Controls | Required evidence |
| --- | --- | --- |
| Forged payer or modified delegate | Strict Borsh decode, key/signature curve match, NEAR domain-separated verification | ED25519/SECP256K1 oracle fixtures and mutation tests |
| Relayer used for arbitrary calls | One action, `ft_transfer`, fixed asset/payee/amount, 1 yocto, 30 TGas, FullAccess payer, configured relayer | Structural negative matrix |
| Wrong network or token | One pinned network/asset per process; exact API-client policy; no client RPC | Cross-network and wrong-asset rejection |
| Duplicate resource access for one payment | Global delegate dedupe; identifier/fingerprint conflict; resource-server dedupe documented | Concurrent duplicate and identifier tests |
| Relayer nonce races | One active instance; durable nonce uniqueness; mutex spanning final nonce through outcome | High-concurrency settlement test |
| Broadcast accepted but response lost | Persist exact bytes/hash, durably mark `submitted`, then broadcast; query both RPCs; exact-byte rebroadcast only while unexpired | Crash/fault injection after every journal stage |
| False success from asynchronous receipts | Bind final outcome identity to the stored transaction; require the unique inner token receipt `SuccessValue`; keep ambiguous graphs nonterminal | Outer-only, missing, ambiguous, identity-mismatch, and failed receipt fixtures |
| Payer state changes after verify | Full re-verification under the settlement claim and mutex | Nonce/balance/storage race tests |
| Sponsorship drain | API keys, exact payees, rate limits, minimum payment, gas cap, atomic daily budgets, low-balance stop | Quota, balance, and reservation rollback tests |
| API-key database theft | High-entropy one-time key, HMAC-SHA256 digest with separate pepper, constant-time compare, rotation/revocation | Authentication and redaction tests |
| Credential leakage in telemetry | Sensitive-header marking, no request bodies, bounded fields, hashes excluded from metric labels; readiness diagnostics use fixed classes only | Automated log/trace redaction test |
| PostgreSQL split brain | Session advisory leadership lock; readiness false without leadership | Competing-instance test |
| RPC lies, lags, or partitions | Finality, pinned blocks, typed errors, independent backup reconciliation, fail closed; protected readiness events classify only fixed snapshot causes | Failover and disagreement tests |
| Database loss or tampering | Scheduled off-host dumps with a tested restore, least-privileged role, append-oriented journal | Dated restore drill |
| Host compromise | Dedicated unprivileged users, systemd sandboxing, root-only credentials, immutable releases | Unit hardening review and credential-permission check |
| Supply-chain substitution | Locked dependencies, deny/audit checks, checksums, SBOM, build provenance | Green release workflow and verified artifact install |

## Idempotency-specific analysis

The optional `payment-identifier` is scoped by API client and bound to a
fingerprint of the full payment payload, resource, requirements, and delegate
hash. Replaying the same identifier and fingerprint returns the same terminal
response. Reusing an identifier for different work returns HTTP 409.

An identifier is not a payment authorization and does not replace the delegate
hash. Without an identifier, or with a different identifier, the same delegate
still resolves to `duplicate_settlement`. The resource server must bind and
deduplicate the identifier independently before releasing protected work.

## Recovery decision boundary

Recovery distinguishes proof of failure from absence of proof. A typed final
transaction, delegate, reachable-receipt, or token-receipt failure is
definitive. When both RPCs report the stored hash unknown, unchanged relayer
nonces plus a passed delegate expiry height are also definitive: the
authorization can no longer execute and the exact bytes must not be
rebroadcast.

A pending lookup, RPC error or disagreement, missing or ambiguous receipt,
transaction-identity mismatch, or unknown hash paired with an advanced relayer
nonce is not safely attributable to failure. Those cases remain nonterminal
and fail readiness. An advanced nonce with an unknown hash additionally
quarantines the relayer. No recovery path signs replacement bytes.

## Reference deployment residual risks

These observations apply to the reference topology recorded in the
[historical runbook snapshot](evidence/2026-07-26-reference-deployment-runbook-snapshot.md);
that snapshot is not go-live evidence. An independent deployment must perform
its own infrastructure threat review.

- Mainnet and testnet share a host. Host, Nginx, kernel, or network failure can
  affect both.
- Targeted preflight cannot simulate NEAR's asynchronous cross-shard runtime.
  Valid verification can still fail at settlement if state changes.
- PostgreSQL and RPC availability are launch dependencies. Failing closed
  protects funds but reduces availability.
- The primary and backup RPC endpoints are both operated by FastNEAR (the
  regular and archival hosts). This gives infrastructure separation for the
  dual-RPC finality and reconciliation checks but not operator independence,
  so a FastNEAR-wide fault or compromise could affect both. Adopting a
  genuinely independent second provider would restore operator-level
  independence; revisit this if reconciliation trust assumptions change.
- A compromised full-access relayer key can spend the relayer's NEAR balance.
  The deliberately small balance, daily caps, alerts, and recovery key limit
  but do not eliminate this risk.
- API clients can legitimately request many invalid preflights. Rate limits
  bound work but do not make public RPC exhaustion impossible.
- The origin is directly exposed to the public Internet with no CDN or proxy
  tier absorbing floods or TLS-layer attacks. Nginx limits, API-key
  authentication, and fail-closed readiness bound abuse, but volumetric
  denial of service is mitigated only by the host's network.
- Telemetry export is disabled at launch. If an OTLP backend is enabled
  later, field allowlisting and sanitized-event review are required first.
- The pinned official `@x402/near@2.19.0` development/reference dependency
  transitively includes `elliptic`, for which
  [GHSA-848j-6mx2-7j84](https://github.com/advisories/GHSA-848j-6mx2-7j84)
  currently has no patched release. It is not linked into either Rust
  production binary, and the reference resource server neither holds payer
  keys nor signs payments. CI fails on high-severity npm findings and this
  low-severity exception must be re-evaluated whenever `@x402/near` changes.

## EVM (eip155) and legacy-v1 delta

The Base instance reuses the entire chain-neutral control set (API keys,
exact policies, budgets, journal, fail-closed recovery). The additional
assets and threats:

- **Protected assets**: the secp256k1 signer key and its ETH gas balance
  (readiness enforces a hard-stop balance; a compromised signer can spend
  gas but cannot redirect payments, which are bound to the signed ERC-3009
  authorization).
- **Single-use anchor**: the ERC-3009 authorization nonce replaces the
  delegate hash; the token contract enforces it on-chain, and the journal
  enforces it in a network/token/payer scope before broadcast. The full signed
  authorization is not retained before preparation: the journal keeps the
  nonce anchor, canonical settlement fields, and validity window, then stores
  the exact signed transaction bytes required for recovery.
- **Historical authorization retention**: migration `0003` drops the legacy
  full authorization and the admin migration boundary requires a completed
  table rewrite before v0.5 can start. Pre-migration backups and archived WAL
  remain sensitive external copies until the operator's reviewed retention
  window expires; the application cannot erase or attest to those archives.
- **Fee exhaustion**: dynamic EIP-1559 estimates are bounded by a configured
  maximum fee per gas. The budget reservation must exceed that ceiling times
  the gas limit so Base L1 data fees remain covered, and readiness requires the
  signer to hold the hard stop plus one full reservation.
- **Reorg instead of receipt ambiguity**: success requires the stored
  transaction identity to hold the configured confirmation depth; a
  mined-then-missing transaction returns to nonterminal. No recovery path
  signs replacement bytes, matching the NEAR rule.
- **RPC trust**: durable signer-head and receipt decisions require two distinct
  configured readers. Chain-ID, pending-nonce, or receipt disagreement is
  indeterminate; conservative head/balance observations gate progress. A
  readiness transition may identify only a fixed class (reader unavailable,
  chain-ID mismatch, or pending-nonce disagreement); it never exposes a
  provider URL, response, chain value, signer, balance, nonce, or transaction
  value. Operator independence and availability still depend on the endpoints
  a self-hoster chooses.
- **Legacy v1 wire (`accept_v1`)**: adds no new authorization semantics — a
  v1 request is strictly translated (deny-unknown-fields) into the canonical
  v2 shape at the parse boundary, so policy, verification, budgets, and the
  journal fingerprint are dialect-independent, and one payment retried in
  either dialect deduplicates to one settlement. The gate is off by default,
  is rejected at config validation for NEAR chain kinds, and only changes
  parse and response formatting; the API-key boundary is unchanged.

## Security review triggers

Repeat the threat review before enabling any of:

- another token or wildcard payee policy;
- another `assetTransferMethod`, authorization contract, custody ledger, or
  delivery mode on a supported chain;
- a single-use anchor whose scope changes, depends on a recovered signer, or
  can be consumed without proving the requested payment effect;
- settlement success derived from simulation, nonce consumption, an outer
  transaction, or a balance snapshot without exact receipt/log/event evidence;
- an asynchronous transfer-call, callback, refund, or partial-acceptance path;
- native NEAR payments;
- anonymous/public settlement;
- more than one active instance or relayer;
- gas-key relayers, DelegateV2, another signature curve, or another
  multi-standard signature envelope;
- automatic relayer refill;
- partner-controlled webhooks or administrative HTTP endpoints;
- transaction replacement or any recovery path that signs new bytes;
- an additional EVM chain, a non-canonical asset binding, or `accept_v1` on
  any new instance class.
