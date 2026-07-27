# NEAR mainnet v0.4.1 regression canary — 2026-07-26

Owner: Mike Purvis

This is sanitized evidence for one explicitly confirmed funded regression
canary against the public NEAR mainnet deployment. It is **not** v0.5.0
rollout evidence: both `/healthz` and the paid flow identified the deployed
facilitator as v0.4.1.

## Confirmed transaction envelope

Immediately before signing and submission, the operator explicitly confirmed:

- network: `near:mainnet`;
- asset:
  `17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1`;
- amount: 1,000 atomic USDC (`$0.001`);
- payer: `mike.near`;
- recipient: `count.mike.near`;
- relayer: `x402-relayer2.mike.near`;
- maximum sponsorship: `0.01 NEAR`;
- inner-call gas ceiling: 30 Tgas; and
- resource: `POST https://x402-demo.mikedotexe.com/work`.

The client used the pinned official `@x402/core` and `@x402/near` 2.19.0
packages. The payer credential was loaded from its mode-0600 credential file;
neither the key nor the signed delegate was printed or persisted.

## Settlement result

The paid request returned a successful settlement for transaction
[`A5MPRMSwAiLXxU3VT6jmFYepvf7d9W8NzWpGxT1qqF3Q`](https://nearblocks.io/txns/A5MPRMSwAiLXxU3VT6jmFYepvf7d9W8NzWpGxT1qqF3Q).

Independent queries through NEAR's public RPC and FastNEAR agreed on:

- final execution status `FINAL`;
- outer transaction status `SuccessValue`;
- four successful receipt outcomes;
- exactly one `SuccessValue` receipt executed by the configured USDC
  contract; and
- no failed receipt.

Balances at finality moved by exactly the confirmed amount:

| Account | Before | After | Delta |
| --- | ---: | ---: | ---: |
| `mike.near` | 503,329 | 502,329 | -1,000 |
| `count.mike.near` | 435,001 | 436,001 | +1,000 |

Total observed gas burn was 3,782,326,687,840 gas and the relayer paid
333,596,156,284,000,000,000 yoctoNEAR (`0.000333596156 NEAR`), below the
confirmed `0.01 NEAR` sponsorship ceiling. Public readiness remained green
after finality.

## Exact replay finding

After the successful response, the client resubmitted the byte-identical HTTP
request with the exact same in-memory `PAYMENT-SIGNATURE`; it did not sign a
replacement authorization. The resource server returned HTTP 402 without a
payment receipt instead of replaying a cached delivery.

Read-only reconciliation after that response showed unchanged balances
(`mike.near` 502,329; `count.mike.near` 436,001), the original transaction
still final on both RPCs, and facilitator readiness still green. Therefore no
second transfer or replacement transaction occurred.

This client did not opt into the resource server's optional
`payment-identifier` extension. The result is evidence for successful
v0.4.1 settlement and chain-level single use, but not for delivery replay
through that optional extension. A future canary should set a stable
payment identifier before signing and require a cached HTTP 200 on the exact
replay.
