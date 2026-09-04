# NEAR Intents 1Click `upfront` method — pre-merge design record

> **Status:** implementation scaffold only; disabled and not advertised.
> **Upstream snapshot:**
> [x402-foundation/x402#3370](https://github.com/x402-foundation/x402/pull/3370)
> at
> [`708d660f2f80f966db16caebdb38670e16f0bc4b`](https://github.com/x402-foundation/x402/commit/708d660f2f80f966db16caebdb38670e16f0bc4b),
> reviewed 2026-09-04. The proposal is open and still requires review.

This record defines what can safely be built before the proposal merges, what
must remain replaceable, and what must not reach a funded path yet. It does not
change the production behavior or launch policy in this repository.

The provider boundary is pinned independently. The quote-signature algorithm
and production manager key come from
[`@defuse-protocol/one-click-sdk-typescript` 0.1.25 at `ae28ef0`](https://github.com/defuse-protocol/one-click-sdk-typescript/tree/ae28ef0348f616dd30c174cb22dd1b1126d8f76b).
The live 1Click OpenAPI fetched on 2026-09-04 had SHA-256
`57ed3bf994b217e2490f44a6b34494f7c2ecace7067115245c97e4c73dd7e409`;
because that URL is mutable, it is research input rather than a conformance
pin. A sanitized, instrument-free production dry quote is retained as a
signature fixture.

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

## Open contract and operational issues

These items must be resolved before runtime conformance is claimed.

| Area | Current ambiguity | Required resolution |
| --- | --- | --- |
| Quote amount | The proposal maps `amount` from `quote.maxAmountIn`; the live API exposes `amountIn` and `minAmountIn`, with no `maxAmountIn`. | Correct the normative mapping. The scaffold isolates both spellings at one adapter and rejects conflicts. |
| Deposit threshold | The proposal requires at least advertised `amount`, while 1Click describes `minAmountIn` as sufficient for `EXACT_OUTPUT`. | Define the exact valid origin amount and underpayment behavior. |
| Routine surplus | 1Click may refund unused input slippage on a successful `EXACT_OUTPUT` swap. | Define `SUCCESS` with surplus separately from terminal `REFUNDED`; specify fees, dust, and receipt data. |
| Refund custody | The revised draft sends refunds to a facilitator-controlled Intents account for forwarding. | Specify a separate durable accounting, signing, fee, submission, reconciliation, and recovery protocol. |
| Refund recipient | Status reports transaction hashes and aggregate amounts, but no canonical origin sender. Transaction sender is insufficient for smart accounts and relayers; a UTXO transaction has no safe generic “first input address.” | Define and test a payer/refund identity rule for each enabled origin or cryptographically bind a refund address. |
| Shared quote | Different transaction hashes can be aggregated under one address and all appear in a successful status. | Require a quote-level claim in addition to the proof claim. Prefer unique quotes until provider allocation behavior is contractual. |
| Proof ownership | `txHash` is unsigned and publicly observable. | Resolve successful-proof front-running or explicitly document the bearer-proof security model. |
| Deadline | A timely deposit may remain processing after quote expiry, while the draft restricts retry to the deadline. | Separate “may deposit” from “must finish reconciling” and retain paid work until terminal. |
| Status lifecycle | `FAILED` and `INCOMPLETE_DEPOSIT` do not prove that a refund reached the facilitator or payer. | Define payment terminality and refund terminality for every live status. |
| Receipt | Draft success pairs the origin `network` with a destination transaction hash; destination status may contain multiple hashes. | Define destination network/asset/amount fields and deterministic transaction selection. |
| Destination trust | 1Click signs the quote but not later `status`, `swapDetails`, `updatedAt`, or destination transaction hashes. | Decide whether the merged method explicitly trusts that provider assertion or independently confirm destination delivery before returning x402 success. |
| Failure response | The example uses `error` and omits `transaction`, while core v2 requires `errorReason` and a transaction string. | Align the scheme response with the merged core schema. |
| Discovery wrapper | Core v2 says an extension has `info` and `schema`; the draft `crosschain-swap` example has only `info`. | Add the required schema or revise the core extension contract. |
| Quote API | The flow assumes resource-server-to-facilitator quote acquisition, but x402 standardizes no such endpoint. | Define an authenticated private API or standard surface and bind all merchant pricing/destination inputs. |
| Quote signature | The draft requires authenticated API calls but does not normatively require verification of the signed quote response. | The pure verifier is pinned to [`@defuse-protocol/one-click-sdk-typescript` 0.1.25 at `ae28ef0`](https://github.com/defuse-protocol/one-click-sdk-typescript/tree/ae28ef0348f616dd30c174cb22dd1b1126d8f76b). Keep runtime issuance closed until normalized live responses can enter the typed model only through that verified capability. |
| Testing network | [Official NEAR Intents documentation](https://docs.near-intents.org/resources/faqs#is-there-a-testnet-deployment) states that no testnet is available. | Keep integration mocked until an explicitly approved, small-value mainnet evidence drill. |

The live 1Click API also distinguishes the memo field by operation:
`depositMode: MEMO` when quoting, `memo` in `/v0/deposit/submit`, and
`depositMemo` in the `/v0/status` query. Conformance fixtures must lock down all
three shapes.

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

- [ ] Route payment methods by the explicit method/flow discriminator before
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

## Adoption gates

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
