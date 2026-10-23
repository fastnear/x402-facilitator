# NEAR Intents 1Click `upfront` method — pre-merge design record

> **Internal status:** implementation scaffold only; disabled and not
> advertised. This record is engineering memory for this repository. It is not
> an upstream proposal, review, or commitment to enable the draft.

This record defines what can safely be built before the proposal merges, what
must remain replaceable, and what must not reach a funded path yet. It does not
change the production behavior or launch policy in this repository.

## Pinned upstream baseline

The conclusions and scaffold in this branch apply to the following exact
snapshot. Mutable upstream references must be re-read through the
[merge-finalization checklist](#merge-finalization-checklist), not silently
treated as equivalent to these pins.

| Contract | Pinned revision | Role in this design |
| --- | --- | --- |
| Current 1Click x402 proposal | [x402-foundation/x402#3370](https://github.com/x402-foundation/x402/pull/3370) at [`708d660f2f80f966db16caebdb38670e16f0bc4b`](https://github.com/x402-foundation/x402/commit/708d660f2f80f966db16caebdb38670e16f0bc4b), reviewed 2026-09-04 | Draft wire, lifecycle, discovery, and refund model. The PR is open and review-blocked. |
| Client-submitted payment-proof family | [#3145](https://github.com/x402-foundation/x402/pull/3145), merged as [`23173acb53825089b7a27cda3774560468c66e33`](https://github.com/x402-foundation/x402/commit/23173acb53825089b7a27cda3774560468c66e33) | Durable claim, retention, finality, amount, and failure-disposition requirements inherited by this method. |
| `upfront` payment flow | [#3053](https://github.com/x402-foundation/x402/pull/3053), merged as [`db5da2e65952e76a4961e0f548d4828c7f37adda`](https://github.com/x402-foundation/x402/commit/db5da2e65952e76a4961e0f548d4828c7f37adda) | Resource middleware calls `/settle` before the handler and skips `/verify`. |
| Historical 1Click proposal | [#2102](https://github.com/x402-foundation/x402/pull/2102) at final head `d33fe0a5da01b8b2aa33eec04a009d4bd48ea277` | Closed, unmerged design history only. It is not a wire or conformance source. |
| Separate NEAR Intents method | [#2948](https://github.com/x402-foundation/x402/pull/2948) at `1573f812014db87422b308276a391b62809c9449` | Distinct same-network `intents-verifier` work; never combine its authorization path with 1Click payment proofs. |
| Official quote verifier | [`@defuse-protocol/one-click-sdk-typescript` 0.1.25 at `ae28ef0348f616dd30c174cb22dd1b1126d8f76b`](https://github.com/defuse-protocol/one-click-sdk-typescript/tree/ae28ef0348f616dd30c174cb22dd1b1126d8f76b) | Signed-field projection, canonical hash, Ed25519 message, and production manager trust root. Version 0.1.24 is insufficient because it did not bind a wet quote's deposit address. |
| Live 1Click provider schema | [`/docs/v0/openapi.yaml`](https://1click.chaindefuser.com/docs/v0/openapi.yaml), fetched 2026-09-04, SHA-256 `57ed3bf994b217e2490f44a6b34494f7c2ecace7067115245c97e4c73dd7e409` | Provider research snapshot. The URL is mutable and the document is not yet a conformance pin. |
| Production quote trust root | `ed25519:reYaWhvwu8Jzo3WUM3zhn6VrhuMEF4eADL17qtRVifc` | Compile-time production key from the pinned official SDK. No key-discovery or rotation contract is published. |

## What the proposal is

The proposal adds `near-intents` as an x402 `exact` **asset transfer method**.
It is not a NEAR-only network scheme. A client can pay on an origin such as
Arbitrum, Base, Solana, NEAR, or Bitcoin; NEAR Intents 1Click swaps that input
and delivers the merchant's exact configured asset and amount on a destination
chain.

The x402 requirements describe the origin leg:

- `network`: origin CAIP-2 network where the client deposits;
- `asset`: origin asset;
- `amount`: quoted origin amount;
- `payTo`: 1Click deposit address;
- `extra.assetTransferMethod`: `near-intents`;
- `extra.paymentFlow`: `upfront`;
- `extra.depositMemo`: conditional memo or destination tag.

The client deposits before asking for the resource and presents only the
origin transaction hash in `payload.txHash`. The resource server calls
`/settle` before its handler and does not call `/verify`. Settlement becomes
successful only after 1Click reports destination delivery to the merchant.
The accepted request does not expose the destination route; the facilitator
must recover it from the exact issued quote record.

The historical
[#2102](https://github.com/x402-foundation/x402/pull/2102) established much of
this direction but closed without merge. The current proposal builds on the
client-submitted payment-proof rules merged in
[#3145](https://github.com/x402-foundation/x402/pull/3145) and the `upfront`
flow merged in [#3053](https://github.com/x402-foundation/x402/pull/3053).

This method is separate from the proposed
[`intents-verifier` method in #2948](https://github.com/x402-foundation/x402/pull/2948).
That method is a same-network, facilitator-submitted NEAR authorization and
belongs inside the existing NEAR provider. The 1Click method is a
client-submitted, potentially cross-chain payment proof and needs a settlement
backend above the existing `ChainProvider::{Near, Evm}` seam.

## Resolved implementation decisions

These are repository decisions for the pre-merge implementation. Revisit them
only when the merged x402 contract or pinned provider evidence directly
requires a change.

1. The method remains entirely disabled: no public route, configuration flag,
   `/supported` entry, 402 requirement, database mutation, signer, or broadcast
   path is added by the scaffold. The preparatory crate is also marked
   `publish = false` until the contract is merged and its reusable API is
   reviewed.
2. 1Click is a settlement backend above the existing closed
   `ChainProvider::{Near, Evm}` enum. It is not another NEAR chain provider and
   does not weaken the chain-neutral payment types.
3. Canonical internal and test values use x402 v2 `exact` with
   `paymentFlow: upfront`. The resource server calls `/settle` before serving
   the resource and does not call `/verify` for this method.
4. Runtime method selection must inspect `scheme`,
   `extra.assetTransferMethod`, and `extra.paymentFlow` before decoding a
   network-specific payload. An EVM-origin `{txHash}` proof must never enter the
   ERC-3009 parser merely because its network is `eip155:*`.
5. The first enabled shape, if adopted, uses one exact configured origin per
   process and a unique wet quote per payable challenge. Shared instruments and
   broad origin menus remain later optimizations.
6. Origin-chain evidence is independent of 1Click: an origin adapter must prove
   transaction identity, finality, asset, recipient, memo, amount, and the
   chain-derived sender. Provider status alone cannot construct that evidence.
7. Success requires both independently verified origin evidence and exact
   provider-reported destination delivery. Origin observation alone never
   advances settlement.
8. Proof and instrument ownership are claimed atomically. Nonterminal attempts
   release their lease while retaining the proof-to-instrument binding;
   terminal outcomes consume both identities.
9. Quote signatures are mandatory before a deposit instrument can be exposed.
   Verification runs on bounded raw JSON using the 0.1.25 signed projection,
   then produces a capability consumed by quote binding. Status binds to the
   authenticated quote hash; unsigned correlation IDs are diagnostic only.
10. The production API origin and production manager key form one trust
    configuration. Tests may inject an explicitly test-only key; production
    construction does not accept an arbitrary runtime key.
11. The signed `timestamp` is retained byte-for-byte and schema-checked, but it
    is not treated as a freshness token. Deposit eligibility comes from the
    quote deadline; paid, nonterminal work is retained and reconciled beyond
    that deadline until terminal.
12. Refund forwarding remains outside the enabled path until attribution,
    custody accounting, fees, signing, durable submission, and recovery are
    specified and reviewed. No funded action is part of this scaffold.

## Proposed lifecycle

1. A resource server asks its facilitator for requirements for one configured
   origin route and merchant destination.
2. The facilitator obtains an authenticated wet 1Click `EXACT_OUTPUT` quote,
   validates it, and durably stores the complete response and exact served
   requirements before returning a 402.
3. The client deposits the quoted origin asset, amount, and memo to the issued
   instrument and retries with the origin transaction hash.
4. `/settle` matches the full request to the issuance record. A crate-owned
   origin adapter independently confirms the transaction identity, finality,
   asset, recipient, memo, amount, and chain-derived sender.
5. In one database transaction, the facilitator binds and leases both the
   global proof and the issued instrument. This prevents two different hashes
   attributed to one aggregate status from serving the resource twice.
6. The facilitator may notify 1Click through `/v0/deposit/submit`, then polls
   `/v0/status` for the exact `(depositAddress, depositMemo)` instrument.
7. A provider-reported `SUCCESS` advances only when the presented origin hash
   is attributed to the quote and exact destination facts match. A nonterminal
   attempt releases its lease but preserves the proof-to-instrument binding so
   only the same proof can retry. A terminal result consumes both identities.
8. Every nonterminal swap and every refund obligation survives restarts and is
   reconciled without a TTL deletion.

The initial implementation should issue a unique quote per payable challenge
and support one explicitly configured origin per process. This avoids the
proposal's shared-address collision path and preserves this repository's
current network-binding and low-cardinality operational model.

The scaffold's curated mapping recognizes reviewed mainnet identifiers for
Ethereum, Optimism, BNB Smart Chain, Gnosis, Polygon, Base, Arbitrum,
Avalanche, NEAR, Solana, and Bitcoin. A route still requires an exact token
snapshot match, including the origin contract or native-asset identity.

## Architecture in this repository

The preparatory code lives in the standalone `x402-near-intents` crate. It
contains no service route, database mutation, signer, or broadcast path. It
currently provides:

- strict draft requirements and proof parsing;
- native transaction-ID canonicalization for explicitly modeled namespaces;
- curated CAIP-2 and asset mappings checked against an exact token snapshot;
- domain-separated proof and `(depositAddress, depositMemo)` identities;
- paired proof/instrument lease transitions with generation fencing;
- a redirect-disabled, bounded, authenticated 1Click client;
- the exact quote-signature projection and Ed25519 verifier pinned to the
  official TypeScript SDK 0.1.25 revision;
- typed quote, submit, and status models;
- verifier-gated request-to-quote binding that retains the complete raw quote
  and authenticated quote hash for recovery;
- independent origin-deposit checks and explicit 1Click status assertions; and
- the indicative `crosschain-swap` discovery model.

The existing facilitator parser recognizes the exact draft method/flow pair
before dispatching by origin network and returns
`unsupported_asset_transfer_method` while the route is disabled. This keeps an
EVM-origin `{txHash}` proof out of the ERC-3009 parser and performs no policy,
database, RPC, or provider work. It does not advertise or execute the method.

When the upstream contract is stable, service routing should select a closed
settlement method from `scheme`, `extra.assetTransferMethod`, and
`extra.paymentFlow` before it interprets a network-specific payload. A higher
level `SettlementBackend` can then dispatch to either the existing chain
engine or the new payment-proof engine. The chain provider enum remains
unchanged because 1Click has no facilitator nonce, sponsored-gas reservation,
prepared chain transaction, or broadcast operation.

The existing `settlements` table also remains unchanged. It structurally
records facilitator-submitted transactions and sponsorship. The eventual
method needs additive tables for issued instruments, proof leases, backend
status, and any refund obligations. The paired state transition in the new
crate specifies the atomic invariant without prematurely freezing that schema.

`/supported`, `/settle`, and every public 402 remain silent for this method
until the adoption gates below are met.

## Safety invariants

- Match the resource and complete canonical payment requirements to the exact
  issuance record; recognizing a deposit address alone is insufficient.
- Globally bind a proof by canonical `<CAIP-2>:<transaction identifier>`.
- Bind an instrument by both deposit address and memo. Address-only keys are
  unsafe for memo/tag networks.
- Claim the proof and instrument atomically. A quote may produce at most one
  resource delivery even when 1Click aggregates multiple deposits.
- Treat the transaction hash as a publicly observable bearer proof. The
  current draft does not prevent another caller from front-running the payer
  for resource delivery, so that limitation must be resolved or accepted
  explicitly upstream.
- Independently verify the exact origin transfer and finality. Aggregate
  1Click status cannot establish which deposit supplied the required amount or
  derive a safe payer/refund identity.
- Require authenticated quote provenance and verify the 1Click response
  signature before exposing a deposit address.
- Treat origin observation as necessary but never sufficient. Only exact
  destination delivery can make the x402 settlement successful. Current
  1Click execution fields are provider assertions carried over authenticated
  HTTPS; unlike the nested quote, they are not signed.
- Keep `KNOWN_DEPOSIT_TX`, `PENDING_DEPOSIT`, `INCOMPLETE_DEPOSIT`, and
  `PROCESSING` nonterminal. Unknown or conflicting status fails closed.
- Separate x402 payment outcome from refund completion. A failed payment may
  still have an outstanding custody obligation, and a successful
  `EXACT_OUTPUT` swap may create a surplus refund.
- Persist every signed refund-forwarding submission before broadcast and
  reconcile only that transaction identity after an indeterminate result.

## Unresolved upstream questions

These are questions in the x402 method contract. A local implementation choice
cannot make them interoperable; each must be answered by the merged
specification or explicitly left unsupported by this facilitator.

| Area | Question to resolve before conformance |
| --- | --- |
| Quote amount | The draft maps x402 `amount` from `quote.maxAmountIn`, while the provider exposes `amountIn` and `minAmountIn`. Which exact field is normative? |
| Deposit threshold | Must the origin transaction transfer the advertised `amountIn`, or may an `EXACT_OUTPUT` payment succeed at any value at or above `minAmountIn`? |
| Successful surplus | 1Click can return unused input after a successful `EXACT_OUTPUT` swap. The method must distinguish that routine surplus from a failed payment refund and define its receipt/accounting treatment. |
| Refund custody | Is facilitator custody and forwarding of every refund part of the method? If so, the spec needs durable accounting, fee/dust behavior, signing, submission, indeterminate recovery, and an eventual completion contract. |
| Refund identity | How is the safe payer/refund recipient derived for smart accounts, relayers, account-abstraction transactions, and UTXO inputs? A generic transaction sender or “first input address” is not sufficient. |
| Shared instruments | When multiple transaction hashes contribute to one aggregate swap, which proof owns the one resource delivery and how are other deposits represented? This implementation defaults to unique quotes and a quote-level claim. |
| Proof ownership | `txHash` is publicly observable and unsigned. The merged method must prevent successful-proof front-running or explicitly define it as a bearer proof. |
| Deadline and retry | A timely deposit may still be processing after quote expiry. The spec must separate the last valid deposit time from the obligation and permission to retry/reconcile already-paid work. |
| Settlement terminality | Define the x402 outcome and proof-consumption rule for every provider state, including `INCOMPLETE_DEPOSIT`, `FAILED`, `REFUNDED`, and `SUCCESS` with surplus. |
| Success receipt | The draft pairs the origin `network` with a destination transaction hash, while the provider can report multiple destination transactions. Define destination network, asset, amount, and deterministic transaction representation. |
| Failure response | The draft example uses `error` and omits `transaction`; core v2 requires `errorReason` and a transaction string. The scheme response must match the final core schema. |
| Discovery envelope | Core v2 requires extension `info` and `schema`; the draft `crosschain-swap` example includes only `info`. One contract must become normative. |
| Quote acquisition | The flow assumes a resource-server-to-facilitator quote request, but no authenticated x402 endpoint or binding contract is standardized. |
| Quote authenticity | The draft authenticates API access but does not normatively require verification of the provider-signed response before exposing `payTo`. |
| Method selection | Define how official clients select `near-intents` when another `exact` method exists for the same origin network and whether `/supported` exposes transfer-method distinctions. |

## Provider-contract uncertainties

These are properties of the live 1Click API and its SDK rather than x402 wire
questions. They remain fail-closed gates even if #3370 merges unchanged.

| Area | Observed contract and remaining requirement |
| --- | --- |
| Amount names | Live OpenAPI exposes `amountIn` and `minAmountIn`; it has no `maxAmountIn`. The scaffold isolates the draft alias at the provider adapter and rejects missing or conflicting values. Remove the alias after the merged spec agrees with a pinned provider schema. |
| Exact-output semantics | Provider documentation says `amountIn` contains an input-side slippage buffer and that a lower value at or above `minAmountIn` may succeed. Lock this behavior with versioned provider fixtures before mapping it to x402 `exact`. |
| Response normalization | A 2026-09-04 dry quote added `depositMode: SIMPLE`, `confidentiality: public`, `quoteWaitingTimeMs: 0`, `insured: false`, and `appFees`, and normalized `...57Z` to `...57.000Z`. Enumerate known defaults, compare timestamps by instant where appropriate, retain exact signed strings, and reject unsupported non-defaults. |
| Signed projection | SDK 0.1.25 signs a selected, flattened field set rather than the complete response. It excludes correlation IDs, app fees, deposit mode, confidentiality, insurance, and other fields. Never use an excluded field for a security decision without independent validation. |
| Key lifecycle | The production manager key is hard-coded. There is no key ID, discovery endpoint, validity interval, or documented rotation procedure. Bind the key to the production origin and require a reviewed release for rotation. |
| Number encoding | `json-stable-stringify` uses ECMAScript number formatting. The scaffold currently accepts only exactly representable safe integers in signed numeric fields. Fractional or unsafe values remain unsupported until differential vectors pin their bytes. |
| Status attribution | Status exposes aggregate deposited/refunded amounts and transaction arrays, but no per-transaction amount or canonical origin sender. It cannot replace an origin-chain adapter or support safe automatic refund forwarding by itself. |
| Status authenticity | The nested quote is signed; later `status`, `swapDetails`, `updatedAt`, and destination transaction hashes are authenticated only by HTTPS/API access. Decide whether to trust this assertion or independently verify destination delivery. |
| Status finality | `FAILED` and `INCOMPLETE_DEPOSIT` do not prove that a refund reached either the facilitator or payer. Payment terminality and refund-obligation terminality must be tracked separately. |
| Transaction cardinality | Status can return multiple origin and destination transaction hashes, while x402 v2 has one `SettlementResponse.transaction` string. A deterministic, evidence-preserving mapping is still required. |
| Memo vocabulary | Quoting uses `depositMode: MEMO`, deposit submission uses `memo`, and status lookup uses `depositMemo`. Versioned fixtures must pin all three shapes. |
| Test environment | [Official NEAR Intents documentation](https://docs.near-intents.org/resources/faqs#is-there-a-testnet-deployment) states that no testnet is available. Keep provider integration mocked until an explicitly approved, small-value mainnet evidence drill. |

## Evidence and fixture pins

These artifacts make the pre-merge conclusions reproducible. Test fixtures are
public, expired or dry, contain no credential, and must never be replaced with
a funded authorization.

| Evidence | Pin and expected result | Purpose |
| --- | --- | --- |
| Official SDK wet fixture | Embedded from the [0.1.25 upstream test](https://github.com/defuse-protocol/one-click-sdk-typescript/blob/ae28ef0348f616dd30c174cb22dd1b1126d8f76b/src/__tests__/quote-signature.test.ts); staging key `ed25519:5J5tkaxyPoR3Q9S8LXfo5bWnXK5Z2bctJ4mB9gENh7co`; quote hash `XS2Ej8EbPHKiDBfxaFY3y6az5pCDb8eh4bSdAErvZy7` | Cross-language parity for the official wet signed projection, including deposit address and deadline. |
| Sanitized production dry quote | [`oneclick-production-dry-exact-output-2026-09-04.json`](../crates/x402-near-intents/tests/fixtures/oneclick-production-dry-exact-output-2026-09-04.json), file SHA-256 `ba669539cf279a1b3fb6e402f43bd8e500d3d869e41acbefdd98743b8a555dee`, quote hash `GYtm1avcPcvNRnPZanKBx1RB541cHnhnLpTSomEmpUn3` | Confirms the production key and captures real default/normalization drift. It is `dry: true` and contains no deposit address or memo. |
| Deterministic wet exact-output quote | [`deterministic-wet-exact-output.json`](../crates/x402-near-intents/tests/fixtures/deterministic-wet-exact-output.json), file SHA-256 `691ccb28687846e70c5878680d468be1990d1ff909d61c50f2cb5f1e10fa56c7`, quote hash `3Nnstyx8CZPxpBMdN2QpPxGH1tNxiud858Z8LBtHVAoL` | Exercises the proposed `EXACT_OUTPUT` plus `refundType: INTENTS` path end to end. Its deterministic test key is labeled `DO NOT FUND` and cannot be constructed as a production verifier. |
| Signature implementation pin | [`signature.rs`](../crates/x402-near-intents/src/signature.rs) with `SIGNATURE_SDK_REVISION = ae28ef0348f616dd30c174cb22dd1b1126d8f76b` | Preserves JavaScript truthiness, request/quote overwrite behavior, Base58 SHA-256 message construction, and strict Ed25519 verification. |
| Mutable provider snapshot | OpenAPI fetched 2026-09-04 with SHA-256 `57ed3bf994b217e2490f44a6b34494f7c2ecace7067115245c97e4c73dd7e409` | Records the source of current DTO and semantic findings. A reviewed, versioned snapshot is still required before enablement. |

## Delivery plan

### Phase 1 — disabled foundation

- [x] Pin the reviewed proposal revision.
- [x] Add strict draft wire and discovery types.
- [x] Add bounded 1Click transport and typed responses.
- [x] Bind issued requirements to the complete quote response.
- [x] Require independent origin evidence before accepting backend success.
- [x] Model atomic proof/instrument leases, nonterminal release, retry, expiry
      recovery, generation fencing, and terminal consumption.
- [x] Add the official 0.1.25 signed-quote projection, production trust root,
      official wet and sanitized production dry fixtures, mutation checks, and
      Ed25519 verification.
- [x] Require verifier-produced capabilities at quote, status, and issuance
      boundaries; retain the raw quote and compare authenticated quote hashes.
- [x] Exercise the authenticated quote-to-status-to-assessment path with a
      deterministic, expired `EXACT_OUTPUT` fixture whose test key is never
      accepted by production construction.
- [ ] Normalize documented live response defaults and pin ECMAScript number
      serialization with differential fixtures before enabling quote issuance.
- [ ] Generate mock fixtures directly from a versioned 1Click OpenAPI snapshot.

### Phase 2 — behavior-preserving service seams

- [x] Route payment methods by the explicit method/flow discriminator before
      network payload parsing, with the new method still disabled.
- [ ] Isolate existing facilitator-submitted settlement behind a higher-level
      backend enum without changing NEAR or EVM behavior.
- [ ] Add extension-capable response construction and remove assumptions that
      response network always comes from process configuration.
- [ ] Define the authenticated resource-server quote interface.

### Phase 3 — merged-spec conformance

- [ ] Update the pinned wire contract, response model, errors, and discovery
      wrapper to the merged specification.
- [ ] Add additive instrument/proof/status migrations and crash recovery.
- [ ] Implement one finality-aware origin adapter and exact payer derivation;
      unsupported or ambiguous origin transactions fail closed.
- [ ] Add official x402 SDK fixtures and an `upfront` resource-server path.
- [ ] Update OpenAPI, configuration, runbook, and threat model.

### Phase 4 — refund and operations

- [ ] Adopt a refund design only after custody, attribution, fees, dust,
      signing, durable broadcasts, and reconciliation receive explicit review.
- [ ] Add readiness, bounded metrics, alerts, backup, and recovery exercises.
- [ ] Advertise the method only after the merged spec and implementation agree.
- [ ] Perform any funded mainnet drill only with the repository's immediate
      human confirmation and record dated evidence.

## Merge-finalization checklist

Run this checklist when #3370 merges or its maintainers declare a review-ready
replacement. A green upstream merge does not by itself enable this method.

### Rebase the contract

- [ ] Record the merged PR, merge commit, merge date, and exact paths of the
      authoritative method and extension specifications.
- [ ] Diff merged text against `708d660f2f80f966db16caebdb38670e16f0bc4b`.
      Classify every normative change to wire fields, method discovery, quote
      construction, amount semantics, proof validation, deadlines, status,
      proof consumption, refunds, and response shape.
- [ ] Re-read the merged core `exact` payment-proof and `upfront` contracts at
      the repository revision containing the new method. Do not assume the
      earlier #3145 and #3053 merge commits remain the complete inherited
      contract.
- [ ] Resolve every row under [unresolved upstream
      questions](#unresolved-upstream-questions), or record the affected route
      as unsupported. No local interpretation may silently fill a normative
      gap.
- [ ] Replace `DRAFT_SPEC_REVISION`, this baseline, comments, wire fixtures,
      error names, and discovery types with the merged contract in one review.
      Remove compatibility aliases that the final contract no longer needs.

### Re-pin the provider

- [ ] Fetch and retain a reviewed versioned 1Click OpenAPI snapshot. Record its
      content digest, API origin, authentication schemes, and retrieval date.
- [ ] Check the latest official TypeScript SDK and npm artifact against
      `0.1.25`/`ae28ef0348f616dd30c174cb22dd1b1126d8f76b`. Diff the signed
      projection, algorithm, production key, key lifecycle, and fixtures.
- [ ] Confirm that the production API still verifies against the reviewed key
      using a sanitized dry quote. Never check in a live wet instrument or
      credential.
- [ ] Resolve every row under [provider-contract
      uncertainties](#provider-contract-uncertainties), including live
      defaults, timestamp normalization, signed numeric encoding, memo names,
      status attribution, and transaction cardinality.
- [ ] Generate deterministic mock quote, submit, and status fixtures for every
      recognized provider state from the pinned schema. Run the official SDK
      oracle and Rust verifier over the same signed values.

### Enablement gates

- [ ] **U1 — upstream contract merged.** The final method, amount mapping,
      response, extension schema, deadline, status, and refund rules are
      reviewed and merged.
- [ ] **U2 — 1Click contract pinned.** Versioned OpenAPI and signed-response
      fixtures cover quote, memo, submit, every status, bounds, and errors.
- [ ] **U3 — quote provenance verified.** A mutated address, amount, deadline,
      or destination fails signature verification before a 402 is served.
- [ ] **U4 — durable dual claim proven.** Concurrent instances cannot use one
      proof twice or serve two proofs for one instrument; crashes and expired
      leases cannot strand or revive terminal work.
- [ ] **U5 — origin adapter proven.** Finality, asset, recipient, memo, amount,
      and payer/refund identity are exact for each advertised origin.
- [ ] **U6 — destination evidence proven.** Success binds the issued quote,
      presented origin transaction, exact merchant output, and canonical
      destination receipt.
- [ ] **U7 — refund protocol proven.** Every normal surplus and failure path is
      durably attributed and reconciled without guessing or double broadcast.
- [ ] **U8 — SDK integration proven.** The pinned official client selects the
      intended method and honors `upfront`; `/supported` and discovery are not
      ambiguous with another `exact` method on the same network.
- [ ] **U9 — operations proven.** Startup reconciliation gates readiness and
      dated evidence covers the approved mainnet path, recovery, and rollback.

### Final repository review

- [ ] Update service routing, configuration, migrations, OpenAPI, resource
      server integration, runbook, threat model, launch checklist, and recovery
      documentation together for the final enabled scope.
- [ ] Keep all unsupported origins and ambiguous provider responses fail-closed
      and absent from `/supported` and `crosschain-swap` discovery.
- [ ] Run the complete local gate, `git diff --check`, concurrency/restart
      regression tests, official SDK interoperability tests, and configuration
      validation.
- [ ] Review the final diff for credentials, live payment instruments,
      unbounded labels, transaction hashes, and refund identifiers before
      committing evidence.
- [ ] Require the repository's immediate human confirmation before the first
      funded broadcast, then capture dated evidence for success, failure,
      restart reconciliation, refund recovery, and rollback.
- [ ] Advertise the method only after every applicable gate above passes. If a
      gate remains unresolved, ship the scaffold disabled and record the exact
      blocker here.
