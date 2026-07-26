# Architecture

## System context

```mermaid
flowchart LR
    P["Payer signs a payment authorization:\nNEP-366 delegate (NEAR) or\nERC-3009 authorization (eip155)"]
    R["x402 resource server"]
    F["Facilitator HTTP and policy boundary\n(v2 canonical; gated legacy v1 on eip155)"]
    E["Chain-neutral settlement engine\n(ChainProvider seam)"]
    J[("Instance-specific PostgreSQL")]
    K["Dedicated relayer / signer key"]
    A["Primary chain RPC"]
    B["Independent backup chain RPC"]
    N["Terminal proof:\nNEP-141 receipt (NEAR) or\nN-confirmation depth (eip155)"]
    O["Optional OTLP telemetry backend"]

    P --> R
    R -->|"/verify and /settle with API key"| F
    F --> E
    E -->|"claims, budgets, exact transaction journal"| J
    E -->|"signs the outer submission"| K
    E -->|"final preflight and broadcast"| A
    E -->|"indeterminate-result reconciliation"| B
    A --> N
    B --> N
    F -->|"sanitized telemetry"| O
```

The resource server, not the public payer, is the API-key client. It remains
responsible for binding a payment to one protected operation. The facilitator
prevents duplicate chain settlement and can deduplicate the optional
`payment-identifier`; it cannot stop a resource server from serving multiple
responses for one payment unless that server also deduplicates.

Every instance is one process pinned to one network: separate Unix users,
relayer/signer keys, PostgreSQL databases, configuration files, ports, and
hostnames. All instances currently share one physical host and therefore do
not provide host-level high availability.

## The chain seam

The durable engine (`service.rs`: claim, prepare, broadcast, reconcile,
terminalize) speaks neutral value types and dispatches through the
`ChainProvider` enum in `crates/x402-near-facilitator/src/chain.rs`. Enum
dispatch was chosen over trait objects deliberately: the chain set is closed
and providers keep rich typed results (see
[evm-v2-design.md](evm-v2-design.md) for the full rationale and the design
history of the seam). Adding a chain means implementing a provider against
the neutral contract and adding an enum arm; the journal, recovery, HTTP,
and policy layers do not change.

Wire dialects are handled entirely at the HTTP boundary: `protocol.rs`
parses canonical x402 v2 with deny-unknown-fields at every level, and
`v1_compat.rs` (active only when the instance sets `accept_v1`; eip155 only)
strictly translates a legacy v1 request into canonical v2 before that parser
runs, then formats the protocol response back into the v1 dialect (legacy
network aliases). Everything past the parse boundary — including the journal
fingerprint — sees one canonical shape, so the same payment retried in either
dialect deduplicates to one settlement.

## Components

### `x402-chain-near`

The reusable NEAR mechanism owns:

- strict base64 and Borsh decoding with no trailing bytes;
- classic NEP-366 signature verification through NEAR's domain-separated
  implementation;
- exact delegate-action structure and NEP-141 argument validation;
- block-pinned final state queries for account, access key, code, balance, and
  recipient storage;
- outer Transaction V0 construction and signing;
- final transaction lookup and inner receipt-graph validation.

It has no HTTP authentication, tenant policy, PostgreSQL schema, deployment,
or telemetry dependency.

### `x402-chain-eip155-provider`

The EVM provider owns:

- payment verification reused wholesale from the pinned upstream
  `x402-chain-eip155` implementation (ERC-3009/EIP-712, smart-wallet
  signature support, balance and authorization-state preflight);
- an offline payment hash over the ERC-3009 authorization as the
  chain-enforced single-use anchor;
- durable submission of the signed `transferWithAuthorization` with sponsored
  gas, and status queries that report mined block identity and confirmation
  depth;
- reorg-aware interpretation: terminal success only at the configured
  `required_confirmations`, with mined-then-missing transactions demoted back
  to nonterminal for re-evaluation.

### Service boundary

The service owns:

- canonical x402 `/supported`, `/verify`, and `/settle` response shapes, plus
  the gated legacy-v1 request translation and response formatting;
- content-type and 64 KiB body enforcement;
- API-key authentication and exact policy lookup;
- per-key request limits and sponsorship budgets;
- durable idempotency and settlement state;
- active-instance leadership and startup reconciliation;
- secret redaction, request IDs, and OTLP export;
- health and sanitized readiness (chain-appropriate checks: RPC finality and
  relayer state on NEAR; expected `eth_chainId` and the signer gas hard stop
  on eip155).

Expected payment rejection remains an HTTP 200 protocol result. Malformed
transport, authentication, policy quota, identifier conflict, and unavailable
infrastructure are HTTP errors.

### Administration boundary

`x402-near-admin` uses explicit database or configuration inputs to apply
migrations, manage API clients and exact payee policies, rotate or revoke
credentials, set budgets, and start an operator-directed reconciliation.
It is not exposed over HTTP. Migration credentials are never supplied to the
service process.

## Verify flow

1. Nginx accepts only JSON bodies no larger than 64 KiB and forwards a request
   ID without logging authentication headers or bodies.
2. The service authenticates `X-API-Key`, Bearer, or identical values in both
   forms, then applies the key's verify rate limit. Non-identical dual
   credentials are rejected.
3. The body parses as canonical v2 (a legacy v1 request is first strictly
   translated when `accept_v1` is set). Version, scheme `exact`, the pinned
   network, configured Circle asset, minimum amount, and the client's exact
   payee policy are validated.
4. The chain mechanism verifies the signed authorization before trusting the
   payer identity — NEP-366 signature on NEAR, EIP-712/ERC-3009 recovery on
   eip155.
5. Chain preflight runs against final state: block-pinned account, access-key,
   balance, and storage checks on NEAR; balance and authorization-state
   checks on eip155.
6. A valid or invalid x402 response is returned (in the request's dialect).
   No relayer nonce is consumed and no transaction is broadcast.

Verification is a current-state preflight, not a reservation. Settlement
repeats it because payer state can change.

## Settle flow

Chain-neutral spine (both chains):

1. Authenticate, enforce policy, derive the chain-enforced payment anchor
   (delegate hash / ERC-3009 authorization hash), validate the optional
   payment identifier, and begin one PostgreSQL transaction.
2. Claim the payment, persist the normalized request and policy snapshot, and
   reserve the conservative sponsorship amount atomically.
3. Acquire active-instance leadership, reverify against live chain state, and
   construct exactly one outer submission.
4. Persist the signer identity, account nonce, exact signed bytes, and
   transaction hash as `prepared` before submission; durably mark `submitted`
   before broadcasting the already-stored bytes.
5. Wait for the chain's terminal proof, bind the returned transaction
   identity to the exact stored hash, and persist the exact terminal
   response; reconcile reserved sponsorship to actual cost.

Chain-specific terminal proof:

- **NEAR** — wait for `FINAL`, then walk the receipt graph
  (transaction → payer delegate receipt → exactly one configured token
  receipt) and require that token receipt to be `SuccessValue`; the relayer
  mutex spans the final nonce read through terminal outcome.
- **eip155** — wait for the stored transaction to hold
  `required_confirmations` of depth with matching block identity; a mined
  transaction that disappears before depth returns to nonterminal, and the
  terminal row records the mined block, confirmation count, and reorg-audit
  columns.

Concurrent calls for the same payment join or return the recorded result. A
different identifier cannot make the same authorization payable twice.

## Journal and recovery

The source of truth is PostgreSQL. The minimum logical records are:

| Record | Purpose |
| --- | --- |
| API client | Public identifier/prefix, HMAC digest, status, rate and sponsorship policy |
| Payee policy | Exact client, network, asset, and `pay_to` tuple |
| Settlement | Chain-enforced payment anchor, identifier/fingerprint, policy snapshot, lifecycle, exact outer transaction, chain-specific authorization columns (delegate identity on NEAR; ERC-3009 authorization, signer, mined block, confirmations on eip155), terminal response |
| Daily sponsorship ledger | Atomic reservation and actual sponsored cost by instance/client/day |

Settlement states are monotonic:

```mermaid
stateDiagram-v2
    [*] --> reserved
    reserved --> prepared
    reserved --> failed
    prepared --> submitted: durable before broadcast
    prepared --> failed: only after authoritative reconciliation
    submitted --> succeeded: NEAR receipt proof or eip155 confirmation depth
    submitted --> failed: definitive final outcome
```

Nonterminal rows are never expired by retention jobs. On startup, the process
holds a session advisory lock and keeps readiness false while reconciling:

- stale `reserved` rows without an outer transaction can release their budget;
- `prepared` and `submitted` hashes are queried on primary, then backup RPC;
- a final result is terminal only when it matches the stored transaction
  identity and satisfies the chain's proof (unique inner token receipt on
  NEAR; confirmation depth on eip155) or a typed on-chain execution failure;
- pending, missing, structurally inconsistent, identity-mismatched, or
  RPC-ambiguous evidence is indeterminate: the row remains nonterminal,
  readiness stays false, and callers receive a retryable 503 rather than a
  guessed x402 result;
- on NEAR, when both RPCs report the hash unknown, relayer nonces are
  unchanged, and the delegate expiry height has passed, the settlement is a
  definitive failure and is never rebroadcast; a consumed relayer nonce with
  no known stored transaction quarantines the key and keeps readiness false;
- exact stored bytes may be rebroadcast only while the authorization remains
  valid and only after both providers' checks — never by signing a new
  transaction.

## Operational topology

Nginx terminates public TLS and routes:

- `x402.mikedotexe.com` to `127.0.0.1:8402` (NEAR mainnet);
- `test.x402.mikedotexe.com` to `127.0.0.1:8403` (NEAR testnet);
- `base.x402.mikedotexe.com` to `127.0.0.1:8405` (Base mainnet;
  `127.0.0.1:8404` is reserved for Base Sepolia when deployed).

Route 53 records point the names directly at the host; no CDN or proxy tier
fronts the origin. Publicly trusted certificates cover exactly these names,
deny-by-default virtual hosts refuse unknown Host and SNI values, and the
API-key boundary plus Nginx method, body-size, and timeout limits face the
public Internet directly. Each systemd unit reads non-secret JSON from
`/etc/x402-near-facilitator/<instance>.json` and receives the database URL,
relayer/signer credential, API-key pepper, and OTLP headers through
`LoadCredential`.

Releases are immutable version directories. The per-instance
`current-<instance>` symlinks are the only deployment pointers, so each
instance is promoted or rolled back without changing the others (the NEAR
fleet and the Base instance intentionally run different versions). Database
migrations remain forward-only and compatible with the previous binary. Note
for rollback: a config that sets `accept_v1` must drop that key before
promoting a pre-v0.4.0 binary, whose configuration parser rejects unknown
fields.
