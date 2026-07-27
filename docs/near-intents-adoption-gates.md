# NEAR Intents Verifier adoption: decisions and engineering gates

Living record of whether and how the proposed `intents-verifier` asset
transfer method (x402 `exact` on NEAR settled through the `intents.near`
Verifier) is adopted by this repository. The frozen background record and its
dated errata are in
[near-intents-x402-progress.md](near-intents-x402-progress.md). The launch
boundaries in [AGENTS.md](../AGENTS.md) remain authoritative and are not
changed by anything here.

The discriminator is still proposed vocabulary while upstream
[x402-foundation/x402#2948](https://github.com/x402-foundation/x402/pull/2948)
is under review. Do not advertise it until the upstream contract is stable and
the implementation exists.

## Decisions (updated 2026-07-27)

1. **Implementation: one in-tree method, one binary.** `intents-verifier` is
   another transfer method for the existing `exact` NEAR scheme, not another
   chain and not a runtime plugin. It belongs in `x402-chain-near`, the closed
   `ChainProvider` integration, and the same durable settlement engine. A
   mainnet operator may run a separately configured instance or hostname that
   advertises only this method for blast-radius isolation, but that instance
   must use the same binary, provider code, migrations, and conformance suite.
   There will be no copied sibling implementation or speculative core-crate
   extraction.
2. **The delegate method remains closed.** Adding `intents-verifier` must not
   relax the existing delegate route's full-access-key requirement,
   single-action `ft_transfer` binding, one-yocto deposit, or gas cap. Parsing,
   verified types, durable validation, and receipt interpretation dispatch by
   an explicit transfer-method variant.
3. **Upstream spec first.** Implementation follows the merged successor of
   #2948, including the final discriminator, signature envelopes, intent
   binding, and failure semantics. The complementary
   [#2102](https://github.com/x402-foundation/x402/pull/2102) design is
   client-settled, cross-chain prepayment and is oriented toward the
   `upfront` scheme pending
   [#2520](https://github.com/x402-foundation/x402/pull/2520); it is not a
   second `exact` Verifier implementation.
4. **`/supported` stays silent until support is real.** Unmerged vocabulary,
   reference vectors, or a standalone experiment are not production
   capabilities.

## Gates before any Verifier method ships

- [ ] **G1 — Spec frozen.** #2948 is merged, or a maintainer-directed
      successor is merged. The final method name, payload shapes, delivery
      modes, error semantics, and verification requirements are stable.
- [ ] **G2 — Wire and signature conformance pinned.** Deterministic,
      `DO NOT FUND` fixtures cover every supported signature standard. In
      particular:
  - a NEP-413 envelope's `message` is a string whose JSON content is parsed
    strictly; it is not addressed as `message.intents`;
  - the ERC-191 form has no envelope `message` field; its signed intent fields
    are verified in the standard's actual wire shape;
  - the recovered canonical signer is bound before it is used for policy,
    telemetry, nonce lookup, or a response.
- [ ] **G3 — Intent binding is closed and exact.** Wallet delivery accepts
      exactly one `ft_withdraw` with the configured token, `payTo`, and atomic
      amount. `msg` MUST be absent—not `null`, empty, or ignored—because its
      presence selects `ft_transfer_call` semantics and permits partial
      acceptance. Every optional or unknown field is rejected unless the
      merged spec explicitly gives it exact semantics. `token_diff` and
      cross-asset fills remain excluded from `exact`.
- [ ] **G4 — Simulation and fees are characterized without overclaiming.**
      The documented simulator validates signatures, and the facilitator also
      verifies them independently; simulation never replaces local
      verification. Simulation does not execute external asynchronous
      receipts, so each delivery mode names the synchronous debit/event
      evidence it can compare; it never claims to have simulated wallet
      receipt finality. The Verifier fee path is confirmed from pinned source
      and dust measurements. A reported `state.fee` does not by itself prove
      that `ft_withdraw` is charged or exempt; exact delivery fails closed on
      any unexplained delta.
- [ ] **G5 — Scoped single-use identity is durable.** The request hash uses a
      reviewed, standard-specific serialization that binds the exact signed
      application string plus every signature-bound envelope field, key, and
      signature while ignoring semantically irrelevant outer JSON key order.
      The chain anchor is the raw Verifier nonce in a scope that includes the
      network, Verifier contract, and canonical signer, matching the contract's
      per-signer nonce domain. Tests prove:
  - the same signer and nonce cannot reserve two different payments;
  - different signers may use the same nonce;
  - an idempotent retry with reordered outer JSON maps to the same request and
    anchor without reserializing the signed application string;
  - claim and sponsorship reservation remain atomic under concurrency.
      The reservation stores only the minimum typed identity metadata, never
      the raw signed bearer authorization. Once prepared, the exact signed
      outer transaction is retained because exact-byte recovery requires it.
- [ ] **G6 — TOCTOU and failure aftermath are explicit.** A signed intent does
      not lock the payer's Verifier balance. Verification can therefore pass
      and settlement can later fail for insufficient balance, storage changes,
      deadline movement, or another state race. A failed wallet delivery
      consumes the authorization nonce even when the Verifier refunds the debit
      to the payer's internal balance. That row is terminal failure: the same
      authorization is dead, and the client must obtain a fresh 402 and sign a
      fresh payload. Tests distinguish pre-prepare retryable infrastructure
      failures from post-submission protocol effects.
- [ ] **G7 — Exact effect, not nonce consumption, proves success.** A consumed
      nonce proves only that some authorization in that signer's nonce domain
      executed. It does not prove which signed payload executed or that the
      merchant received the exact effect. Reconciliation binds the stored
      signed transaction to its journal row and requires method-specific
      receipt/event evidence for the configured token, recipient, and amount.
      Missing, conflicting, or unattributable effect evidence remains
      nonterminal and fails readiness. No recovery branch signs replacement
      bytes.
- [ ] **G8 — Mainnet-only operational parity.** Because there is no testnet
      Verifier, explicitly confirmed dust-scale mainnet drills cover success,
      storage and balance races, wallet-delivery failure and refund,
      response-loss recovery, stored-byte tampering, exact-byte rebroadcast,
      concurrent anchor claims, and a consumed nonce with missing effect
      evidence. The method reaches the same alerting, backup, readiness, and
      rollback standard as the delegate route before it is advertised.

The implementation shape for G2–G8 is maintained in
[near-intents-verifier-design.md](near-intents-verifier-design.md).

## Non-goals

- **Runtime provider plugins or a second service codebase.** Optional process
  isolation is configuration of the same binary, not a new implementation.
- **`exact-agent`.** The least-privilege payment-agent track remains separate
  and testnet-proven only.
- **`token_diff` or cross-asset settlement.** Those quote-dependent flows
  belong to the #2102 → `upfront` direction.
- **Testnet Verifier support.** No testnet Verifier deployment exists. This is
  a hard deployment constraint, not permission to weaken the mainnet evidence
  gate.
