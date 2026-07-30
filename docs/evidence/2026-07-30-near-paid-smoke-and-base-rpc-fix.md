# NEAR paid smoke and Base backup-RPC fix - 2026-07-30

Owner: Mike Purvis

This records two explicitly confirmed, small funded x402 smoke payments through
the public NEAR demo workloads, plus the follow-up Base readiness repair made
during post-checks. These payments were operator canaries from Mike-controlled
payer accounts; they are not third-party adoption volume and must not be used as
independently attributable x402-list settlement evidence.

No payer key, signed NEP-366 delegate, `PAYMENT-SIGNATURE`, API key, or full
credentialed RPC URL was printed or persisted. The proof runner wrote only
sanitized result checkpoints and did not retry either paid request.

## Confirmed envelopes

Immediately before broadcast, the operator confirmed the exact funded payments
shown below.

Testnet:

- network: `near:testnet`
- asset: `3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af`
- amount: 1,000 atomic test-USDC
- payer: `mike.testnet`
- recipient: `merchant.mike.testnet`
- relayer: `x402-relayer.mike.testnet`
- maximum sponsorship: `0.01 NEAR`
- inner-call gas ceiling: 30 Tgas
- resource: `POST https://x402-demo-test.mikedotexe.com/work`
- confirmation token: `396e9b7ec583219013812aab`

Mainnet:

- network: `near:mainnet`
- asset: `17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1`
- amount: 1,000 atomic USDC
- payer: `mike.near`
- recipient: `count.mike.near`
- relayer: `x402-relayer2.mike.near`
- maximum sponsorship: `0.01 NEAR`
- inner-call gas ceiling: 30 Tgas
- resource: `POST https://x402-demo.mikedotexe.com/work`
- confirmation token: `3dbd1ddf26487057318f4e15`

## Settlement results

Both paid requests returned HTTP 200 with a successful settlement header.

| Network | Transaction | Response | Work result | Replayed |
| --- | --- | --- | --- | --- |
| `near:testnet` | [`Ax8kFqgPZZgsvamjwdMeE7RmmBTDhLNVgeB5fFt9E3x1`](https://testnet.nearblocks.io/txns/Ax8kFqgPZZgsvamjwdMeE7RmmBTDhLNVgeB5fFt9E3x1) | 200 | `056ed2a27f83753b3e9d7dd44046ed4a82a5a0921e2d31f56556c97b900142aa` | `false` |
| `near:mainnet` | [`2AzhbSqm1nT5QizVBmas3xFisNiqx9upYnKbGfReWnb5`](https://nearblocks.io/txns/2AzhbSqm1nT5QizVBmas3xFisNiqx9upYnKbGfReWnb5) | 200 | `ebfb7547ab439c8410efdeb5191e3b724cbe3dc79c7fa2b5b906aad9d805aae8` | `false` |

Read-only finality checks through NEAR public RPC reported `SuccessValue` for
both transactions, `SuccessReceiptId` for both outer transaction outcomes, four
receipt outcomes per transaction, and four `SuccessValue` receipt outcomes per
transaction.

| Network | Gas burned | Payer before | Payer after | Recipient before | Recipient after |
| --- | ---: | ---: | ---: | ---: | ---: |
| `near:testnet` | 3,756,264,758,667 | 20,000,000 | 19,999,000 | 0 | 1,000 |
| `near:mainnet` | 3,748,126,687,840 | 501,329 | 500,329 | 437,001 | 438,001 |

Unpaid post-checks against both NEAR demo workloads returned HTTP 402 as
expected.

## Base readiness repair during post-checks

During the post-payment host canary, Base facilitator readiness failed closed.
The NEAR mainnet and testnet paths had already settled and verified
successfully; the Base issue was independent of those paid flows.

Observed Base state before repair:

- `https://base.x402.mikedotexe.com/readyz` returned HTTP 503 with
  `rpc=not_ready` and `relayer=not_ready`.
- Direct checks against the configured Alchemy primary returned Base chain ID,
  block number, and signer balance successfully.
- Direct checks against the configured dRPC backup returned Base chain ID and
  block number, but `eth_getBalance` returned HTTP 408 with a free-plan timeout.

The non-secret Base facilitator config was changed on the production host from
`backup_rpc_url=https://base.drpc.org` to `backup_rpc_url=https://mainnet.base.org`.
The previous config was preserved as:

`/etc/x402-near-facilitator/base.json.20260730T155132Z.bak`

Both candidate replacement endpoints (`mainnet.base.org` and
`base-rpc.publicnode.com`) were tested first for `eth_chainId`,
`eth_blockNumber`, `eth_getTransactionCount`, and `eth_getBalance`; both passed.
`mainnet.base.org` was selected because it was already a known-good public Base
endpoint in this deployment.

An initial edit accidentally tightened `/etc/x402-near-facilitator/base.json`
to `0600 root:root`, which prevented the `x402-near-base` service user from
reading it. The file mode was corrected to the established pattern
`0640 root:x402-near-base`, `systemctl reset-failed` was run for the Base
facilitator and canary units, and only `x402-near-facilitator@base.service` was
restarted.

Final verification:

- `x402-near-facilitator@base.service` active/running since
  `2026-07-30T15:52:38Z`
- five consecutive `https://base.x402.mikedotexe.com/readyz` samples returned
  HTTP 200 with all gates ready
- `https://merchant-base.mikedotexe.com/readyz` returned HTTP 200 with
  `rpc=ready`, `facilitator=ready`, and `payment=ready`
- `https://base.x402.mikedotexe.com/supported` returned HTTP 200
- the full unpaid `x402-canary.service` run at `2026-07-30T15:52:56Z` passed:
  `VerifyCanaryOk`, `DemoWorkOk`, and `MerchantApiOk` were all `1` for their
  configured networks
- scheduled canary runs at `2026-07-30T15:57:24Z` and
  `2026-07-30T16:02:20Z` also passed, including
  `MerchantApiOk network=base value=1`
- CloudWatch alarm `x402-merchant-base-api-canary-failing` entered `OK` at
  `2026-07-30T16:03:37Z` after two healthy `MerchantApiOk{Network=base}`
  datapoints at `15:53Z` and `15:58Z`
- `systemctl list-units --state=failed` showed zero failed units
