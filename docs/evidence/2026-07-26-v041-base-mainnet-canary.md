# Base mainnet v0.4.1 regression canary — 2026-07-26

Owner: Mike Purvis

This is sanitized evidence for one explicitly confirmed funded regression
canary against the public Base mainnet deployment. It is **not** v0.5.0
rollout evidence: `/healthz` identified the deployed facilitator as v0.4.1,
and its `/supported` response still omitted the `payment-identifier`
extension added by v0.5.0.

## Confirmed transaction envelope

Immediately before authorization and submission, the operator explicitly
confirmed:

- network: `eip155:8453`;
- asset: `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`;
- amount: 1,000 atomic USDC (`$0.001`);
- payer: `0x11B1cb965c64A8005953c1622a67C2030bEB7987`;
- recipient: `0x7Ff46ab88688D528bCE3e59c470240c6901cF88c`;
- facilitator signer: `0x7Ff46ab88688D528bCE3e59c470240c6901cF88c`;
- gas limit: 120,000;
- maximum sponsorship: 200,000,000,000,000 wei (`0.0002 ETH`);
- required confirmations: 2; and
- resource: `POST https://x402-demo-base.mikedotexe.com/work`.

Preflight showed both RPCs at the same Base mainnet head, a funded payer, a
ready facilitator, signer pending nonce 3, and signer balance
9,998,449,354,333,699 wei.

## Client safety controls

The client used the pinned official `@x402/core`, `@x402/evm`, and
`@x402/extensions` 2.19.0 packages. It validated the canonical v2 challenge,
including Circle's `"USD Coin"` / `"2"` EIP-712 domain, and opted into the
resource server's optional `payment-identifier`.

The local signer was hard-capped at one ERC-3009 authorization. The client
also permitted only one initial paid submission. The private key was loaded
from its external mode-0600 credential file, while the signed authorization
and `PAYMENT-SIGNATURE` remained in memory and were never printed or
persisted.

## Settlement result

The paid request returned a successful settlement for
[transaction `0x6f02…be7b`](https://basescan.org/tx/0x6f020f60a35c4f614207ac89ad969947e0dd5bafa2c0c44b11c276379ca1be7b).

Independent Base public RPC and PublicNode queries agreed on:

- successful receipt status at block 49,161,728;
- the same receipt block hash and transaction identity;
- facilitator signer, canonical USDC destination, and zero ETH value;
- exactly one USDC `Transfer` log from the payer to the configured recipient
  for 1,000 atomic units; and
- 25 conservative confirmations at the recorded observation, above the
  required depth of 2.

Balances moved by exactly the confirmed amount:

| Account | Before | After | Delta |
| --- | ---: | ---: | ---: |
| Canary payer | 1,000,000 | 999,000 | -1,000 |
| Configured recipient | 3,000 | 4,000 | +1,000 |

The facilitator signer nonce moved exactly once, from 3 to 4. Receipt gas was
85,728 at an effective gas price of 5,097,635 wei, with an L1 data fee of
1,814,987,756 wei. Total observed sponsorship was 438,825,041,036 wei
(`0.000000438825041036 ETH`), below the confirmed
200,000,000,000,000-wei maximum. Public readiness remained green.

## Exact replay result

Only after the initial HTTP 200, successful settlement response, confirmed
receipt, and exact balance deltas, the client resubmitted the byte-identical
request with the exact same in-memory `PAYMENT-SIGNATURE`. It did not create a
new authorization.

The resource server returned HTTP 200 with `replayed: true`, proving delivery
from the payment-identifier journal. Subsequent independent reconciliation
showed unchanged balances (payer 999,000; recipient 4,000), signer nonce still
4, the original receipt final on both RPCs, and readiness still green. No
replacement or second transaction was created.
