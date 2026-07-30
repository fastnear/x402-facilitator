# NEAR Intents Verifier method — in-tree design sketch (DRAFT)

> **Status: draft, pending gate G1.** This sketches the proposed
> `assetTransferMethod: "intents-verifier"` amendment to x402 `exact` on
> `near:mainnet`. Upstream
> [x402-foundation/x402#2948](https://github.com/x402-foundation/x402/pull/2948)
> is still under review, so names and wire fields are not frozen. Nothing here
> is implemented or advertised. The living gates are in
> [near-intents-adoption-gates.md](near-intents-adoption-gates.md); historical
> research and dated corrections are in
> [near-intents-x402-progress.md](near-intents-x402-progress.md).

## Why this is an in-tree method

`intents-verifier` changes how an `exact` NEAR payment is authorized and
submitted, but it does not introduce another chain. It uses the same CAIP-2
network, Circle USDC policy, NEAR RPC/finality model, facilitator relayer, and
durable settlement engine as `delegate`. The correct seam is a typed
transfer-method variant inside `x402-chain-near` and the closed
`ChainProvider::Near` integration.

The mainnet-only method may be enabled in a separately configured process or
hostname to isolate Verifier availability and custody risk. That is an
operational deployment choice: both instances use the same binary, provider
implementation, migrations, recovery logic, and conformance tests. Do not
create a copied sibling service, dynamic plugin ABI, or speculative shared-core
crate.

The existing delegate route remains structurally closed. Its full-access payer
key, one `ft_transfer`, one-yocto deposit, and gas constraints are not relaxed;
the parse boundary selects a distinct verified type before either mechanism
runs.

## Method boundary

- **Serves:** x402 v2 `exact` on `near:mainnet`, proposed
  `extra.assetTransferMethod: "intents-verifier"`, with wallet delivery through
  `ft_withdraw`.
- **Does not serve:** `near:testnet` (no Verifier deployment), cross-asset
  `token_diff`, the client-settled 1Click flow, `exact-agent`, or the proposed
  `internal` delivery mode. This repository will reject and not advertise
  `internal` until a separate exact-effect and operational gate passes.
- **Custody:** payer funds already live in the shared `intents.near` Verifier
  ledger. The facilitator never holds those funds; it signs and sponsors the
  outer `execute_intents` transaction. Verifier custody and availability are
  therefore explicit trust dependencies.
- **Payer identity:** the canonical signer recovered and authorized under the
  Verifier's signature rules, which may be a NEAR account or an implicit
  identity such as an ERC-191-derived address. It is not the outer transaction
  signer.

## In-tree provider shape

The implementation remains in `x402-chain-near`:

- parsing dispatches on the exact transfer-method discriminator;
- a typed NEAR verified-detail enum keeps delegate and Verifier values
  separate;
- both variants produce the neutral `PaymentIdentity` and
  `DurableSubmission` contracts consumed by the service;
- stored-submission validation and receipt interpretation are provider-owned
  and method-specific;
- the facilitator service retains one exhaustive, compile-time
  `ChainProvider::Near` arm.

An additive migration may still be necessary. “Same chain” does not mean
“same durable facts”: the Verifier signer, nonce scope, deadline, method, and
outer call binding differ from NEP-366 delegate metadata.

## Payment identity and sensitive persistence

The two durable identities have different jobs:

- **Request hash:** a domain-separated hash using a reviewed,
  standard-specific serialization. It binds the exact original signed
  application string and every signature-bound envelope field, key, and
  signature, while outer JSON object key order is semantically irrelevant. The
  serialization and equivalent-JSON fixtures are finalized at G2 before code
  is written. This hash identifies the bearer payload and participates in the
  canonical-v2 request fingerprint.
- **Single-use anchor:** the raw 32-byte Verifier nonce, uniquely constrained
  in a scope containing the canonical network, Verifier contract, and recovered
  signer. The exact serialized scope is finalized with the upstream signer and
  nonce rules. The contract consumes nonces per signer, so a global nonce-only
  constraint would be wrong.

At reservation, store only the request hash, scoped anchor, canonical signer,
signature standard, deadline, and other minimum facts required to validate a
future prepared submission. Do not retain the raw signed intent merely to make
pre-prepare retries convenient; an authenticated retry must supply it again.

After preparation, persist the exact signed NEAR transaction and hash before
broadcast. Its `execute_intents` calldata necessarily contains the signed
authorization, so it receives the same sensitive-data handling and `Debug`/
telemetry redaction as every other durable signed submission. Recovery reuses
only those exact bytes and never creates a replacement.

## Verification pipeline

The method has its own ordered verifier:

1. Require canonical x402 v2 `exact`, `near:mainnet`, the configured Circle
   USDC asset and payee policy, and the exact transfer-method discriminator.
2. Deny unknown fields at every method-specific wire level and allow only the
   configured signature standards.
3. Parse each signature standard in its real shape:
   - NEP-413 carries an envelope whose `message` is a string; parse the JSON
     content of that string strictly. There is no `message.intents` field.
   - ERC-191 has no envelope `message` field. Verify and recover the signer from
     the flat signed intent representation defined by the proposed upstream
     spec.
4. Recompute and verify the signature locally before using the claimed signer.
   For named accounts, check the Verifier key registry; for implicit identities,
   apply the Verifier's canonical derivation rules. NEAR access keys are not the
   authority for this method.
5. Require `is_nonce_used(canonical_signer, nonce) == false` at the pinned
   preflight state.
6. Enforce the final upstream two-sided deadline and configured clock-skew
   policy.
7. Require exactly one wallet-delivery intent:
   `ft_withdraw{token == asset, receiver_id == payTo, amount == amount}`.
   The field set is closed. In particular, `msg` MUST be absent—not `null`,
   empty, or ignored—because its presence changes delivery to
   `ft_transfer_call`, whose receiver may partially accept and refund the
   remainder.
8. Require the pinned Verifier balance to cover the amount and the configured
   wallet storage precondition to hold.
9. Run `simulate_intents` as supplemental fail-closed preflight. The documented
   simulator validates signatures, and the provider still performs step 4
   independently; simulation never replaces local verification.
10. Compare only evidence simulation actually produces. External asynchronous
    wallet receipts are not executed by simulation, so it cannot prove that
    `payTo` received funds. For each supported delivery mode, specify the
    synchronous payer debit and DIP-4/event outputs that must equal the signed
    intent; reject missing or unexplained output.
11. Apply fee semantics from pinned Verifier source and measured vectors.
    `state.fee` being nonzero does not by itself establish whether
    `ft_withdraw` is charged. The current expectation that protocol fees apply
    to `token_diff` matching must be source-verified before it becomes policy.

Verification is a snapshot, not a reservation. A signed intent does not lock
the payer's Verifier balance. Another authorization, storage change, deadline
transition, or contract-state change may make settlement fail after a valid
preflight. `/verify` must not promise otherwise, and `/settle` re-verifies after
owning the durable claim.

## Preparation and submission

The facilitator signs one NEAR Transaction V0 calling
`intents.near::execute_intents` with the verified signed authorization and the
method's exact gas/deposit policy. Before trusting any RPC evidence, recovery
strictly decodes the complete stored transaction and binds:

- outer signer, public key, account nonce, receiver, method, gas, deposit, and
  transaction hash;
- the embedded signed authorization, recovered payer, deadline, signature
  standard, and scoped nonce anchor;
- the exact asset, payee, amount, and closed `ft_withdraw` fields journaled at
  claim time.

Row-swapped, trailing, malformed, or inconsistent bytes are journal corruption
and fail readiness without broadcast.

## Settlement success and refund evidence

Outer `execute_intents` success and nonce consumption are both insufficient for
payment success.

For wallet delivery, the provider must prove the exact asynchronous effect:

- the stored transaction executed the journaled signed authorization;
- the Verifier initiated the bound withdrawal;
- the configured token's delivery receipt completed successfully; and
- method-specific receipt/event evidence binds the configured recipient and
  full atomic amount, with no partial-acceptance path.

A wallet token receipt that both RPCs bind to the stored submission and agree
has failed is authoritative terminal payment failure. The nonce remains
consumed, so the same payload is dead: the client must receive a fresh 402 and
sign a fresh authorization.

The Verifier resolver should restore the debit to the payer's internal balance.
That refund is a separate custody invariant, not payment-success evidence. A
missing, failed, or ambiguous refund quarantines the method's sponsorship and
keeps readiness false until reconciled, but it does not turn an authoritatively
failed merchant delivery back into a nonterminal payment. Missing or
conflicting payment-effect receipts and ambiguous recipient/amount evidence
remain nonterminal. The service does not infer either payment or refund success
from a later balance snapshot.

## Reconciliation and recovery

The stored outer transaction hash remains the first recovery identity. Primary
and backup RPCs must agree on any final outcome before the method-specific
receipt interpreter terminalizes it.

If both RPCs report the exact stored hash unknown:

- an unused authorization nonce, unchanged relayer nonce, and unexpired
  deadline permit rebroadcast of the exact stored bytes;
- an advanced relayer nonce with an unknown stored hash quarantines the
  relayer;
- a consumed authorization nonce proves only that an authorization in that
  signer's nonce domain executed. It prevents rebroadcast but does **not**
  prove that this signed payload produced the exact merchant effect.

The last case requires an independently trustworthy path to the exact
transaction/effect evidence. Until that evidence is found and bound to the
journaled request, the row stays nonterminal and readiness remains false. A
third party's ability to submit the bearer payload is not permission to equate
`is_nonce_used` with settlement success.

Every prepared or submitted branch either reaches method-specific terminal
evidence, waits without mutating the transaction, or fails readiness. No branch
re-signs, changes calldata, advances fees by replacement, or converts a
post-execution failure into a pre-prepare retry.

## Operational posture

- The method is mainnet-only. Dust canaries and failure drills require the
  repository's fresh funded-broadcast confirmation every time.
- A separately configured instance may isolate Verifier readiness and custody
  exposure, but it deploys the same binary and is upgraded in lockstep.
- Keep payer Verifier balances deliberately small and prefer just-in-time
  deposits. Sponsorship budgets cover the facilitator's NEAR gas, not payer
  custody risk.
- Readiness includes Verifier views, both NEAR RPCs, the relayer hard stop, and
  complete journal reconciliation.
- Monitoring distinguishes preflight rejection, retryable infrastructure
  failure, authorization consumed without effect evidence, wallet-delivery
  failure/refund, and terminal exact success without placing signer, nonce,
  payload, or transaction hashes in metric labels.

## Open questions before G1 closes

- Final discriminator and delivery-mode vocabulary.
- Canonical signer serialization and the exact anchor-scope string.
- The pinned source version defining signature, fee, event, and refund
  behavior.
- The exact standard-specific request-identity serialization and
  equivalent-JSON retry fixtures.
- The trustworthy lookup path for a third-party submission when the stored
  outer hash is unknown but the signer nonce is consumed.
- Whether any internal-ledger delivery mode can supply exact, portable effect
  evidence suitable for x402 `exact`.
