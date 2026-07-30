# Adding a chain

This facilitator supports a closed, audited set of in-tree chains. It does not
load providers dynamically and does not promise a stable plugin ABI. That is a
security boundary: every provider participates in payment identity,
single-use enforcement, durable submission, recovery, readiness, and
operator-funded gas.

Adding a chain is additive, but it is more than adding one enum arm. The
provider crate keeps chain primitives out of the engine; the service and
superset database still need explicit integration for the new chain's durable
facts.

## First decide: new chain or new transfer method

A new authorization or settlement method on an already supported chain is not
automatically a new chain provider. If it shares the chain's network identity,
RPC/finality model, signer infrastructure, and transaction primitives, prefer
an exhaustive method variant inside the existing provider crate and
`ChainProvider` arm. Keep its payload parser, verified type, preparation,
stored-submission validator, receipt/effect interpreter, and `/supported`
metadata method-specific.

Do not create a sibling provider crate, service binary, or runtime plugin merely
because the new method has a different signature format or settlement
contract. An operator may run a separately configured instance to isolate
availability, custody, or mainnet-only risk while still using the same binary
and implementation.

“Same chain” also does not mean “no migration.” A method with a different
single-use domain, authorization metadata, prepared call, or terminal-effect
proof needs additive typed journal constraints and method-specific recovery
tests. The design proposal must explain:

- whether the request hash and chain-enforced anchor differ from existing
  methods, including the exact anchor scope;
- which fields are closed and which optional fields change execution semantics;
- what preflight state can change before settlement and whether signing locks
  any funds or nonce;
- what exact receipt, log, event, or state transition proves success;
- what authorization is consumed on failure and whether a fresh 402 is
  required;
- how stored bytes are validated and reconciled without confusing methods on
  the same chain.

Open a chain-proposal issue before implementation. The proposal must identify
the authoritative x402 scheme specification, official interoperability oracle,
canonical network and asset policy, chain-enforced single-use anchor,
terminal-success proof, signer model, finality/reorg model, and recovery rules.

## 1. Establish the protocol authority

- Pin the upstream x402 core and exact-scheme specification used by the chain.
- Generate deterministic fixtures from an official implementation. Test keys
  must be public, labeled `DO NOT FUND`, and unusable for live value.
- Define canonical v2 requirements and payload shapes. Legacy dialects, if any,
  translate to v2 only at the parse boundary; downstream code never forks on
  dialect.
- Fix the network/asset allowlist in configuration validation. Never accept a
  client-selected RPC URL or infer the network from signer material.

Acceptance: the provider rejects unknown fields, lossy amounts, wrong
network/asset/payee values, malformed signatures, and unsupported scheme
features before any trusted payer value is emitted.

## 2. Add the mechanism/provider crate

Create one `crates/x402-chain-<name>` crate with no HTTP authentication,
tenant policy, PostgreSQL, deployment, or telemetry dependency. Its public API
must cover:

- offline decoding and signature verification;
- authoritative chain preflight at the required consistency/finality level;
- a deterministic payment identity and chain-enforced single-use anchor;
- signer/head snapshot and readiness checks;
- preparation of exact signed submission bytes and deterministic hash;
- broadcast classification that fails closed on ambiguity;
- stored-byte validation and exact-byte rebroadcast;
- reconciliation to nonterminal, terminal success, terminal failure, conflict,
  or ambiguous evidence.

Keep payer authorizations and signed submissions redacted from `Debug`,
tracing, fixtures, and errors.

## 3. Wire the closed enum and configuration

Add the chain to the service's `ChainKind`, `ChainProvider`,
`VerifiedDetail`, and `PreparedDetail` enums and to provider construction. The
engine-facing values remain chain-neutral strings, byte arrays, atomic-unit
integers, and explicit terminal evidence; conversions to chain primitives stay
inside the provider adapter.

Add a chain-specific configuration block and validation for:

- CAIP-2 network identifiers and deployment tier;
- canonical asset contract;
- signer identity and key encoding;
- independent RPC endpoints;
- gas reservation, warning, and hard-stop units;
- finality or confirmation policy.

Do not make the enum open-ended or replace it with a trait object merely to
avoid match arms. Rich typed results and exhaustive handling are intentional.

## 4. Extend the durable journal

Write a forward-only migration and the matching store projection. A new chain
must durably record, at minimum:

- `chain_kind` and the canonical v2 request fingerprint;
- payment hash plus a precisely scoped, uniquely constrained single-use anchor;
- the minimum authorization facts required for safe replay protection;
- signer identity and consumed nonce/sequence, if applicable;
- exact signed submission bytes and deterministic transaction hash before
  broadcast;
- terminal block/receipt/log evidence and actual sponsored cost.

Use chain-conditional database constraints so nonterminal rows cannot exist
without the facts recovery needs. Never TTL-delete nonterminal records.
Document the forward-only rollback boundary and the required restore or
forward-fix procedure in the migration and changelog. Never imply that an older
binary may run against the upgraded schema.

The settlement engine should consume a neutral authorization projection and a
neutral prepared-submission projection. If adding a chain requires a new
engine branch, explain the invariant that cannot live behind the provider/store
boundary and add cross-chain parity tests.

## 5. Prove recovery and concurrency

Add deterministic tests for:

- concurrent duplicate claims and scoped uniqueness of single-use anchors;
- reserve-budget-and-claim atomicity;
- crash after prepare, after durable submit transition, and during broadcast;
- ambiguous broadcast resolved only by the stored hash and exact bytes;
- restart reconciliation before readiness;
- signer nonce/sequence movement while a transaction is unknown;
- primary/backup RPC disagreement or indeterminate evidence;
- chain reorganization or equivalent finality reversal;
- terminal success at the chain's authoritative inner receipt/log/finality
  locus, not merely outer transaction success.
- for each method, authorization-consumed-without-effect, refund, partial
  acceptance, and other asynchronous failure paths;
- row swapping between methods that share one chain and journal.

Every nonterminal state must have one documented next reconciliation action.
No recovery branch may create replacement bytes unless a separately reviewed
protocol explicitly permits it.

## 6. Complete the public and operational surface

- Add the v2 payload branch to `docs/openapi.yaml` and interoperability fixtures.
- Extend `/supported`, `/readyz`, bounded metrics, and sanitized errors.
- Add non-secret testnet and mainnet configuration examples, deployment
  verification, monitoring thresholds, and operator rollback notes.
- Update the reference resource server or add a focused example.
- Update the threat model, architecture, changelog, crate README, and this
  checklist.
- Run `./scripts/check-full.sh`, then complete testnet funded, restart, reorg or
  fault-injection, and rollback drills before proposing mainnet promotion.

Funded acceptance is an operational action, not a merge prerequisite. It
always requires the repository's fresh human confirmation showing network,
asset, amount, payer, recipient, signer, and maximum sponsored gas.

## Definition of done

A chain is supported only when canonical v2 verify and settle work through the
same durable engine; every ambiguity stays nonterminal and unready; restart
recovery is deterministic; the exact submission is never silently replaced;
the public contract and operator controls have cross-chain parity; and dated
evidence exists for any deployment claim.

The same definition applies to an additional transfer method. It is supported
only when its method discriminator, single-use scope, stored-byte binding,
failure aftermath, and exact terminal effect are independently proven without
weakening another method on that chain.
