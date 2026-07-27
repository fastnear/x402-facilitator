# Agent merchant API mainnet deployment

Date: 2026-07-27

Status: deployed and discovery-verified; NEAR and Base mainnet paid-flow
evidence complete.

## Public origins

- [NEAR merchant API](https://merchant-near.mikedotexe.com/)
- [Base merchant API](https://merchant-base.mikedotexe.com/)

Both origins run on the existing `x402-facilitator` EC2 host. They are separate
systemd processes because each process is bound to one x402 network:

| Origin | Network | Local service | Port | Facilitator |
| --- | --- | --- | ---: | --- |
| `merchant-near.mikedotexe.com` | `near:mainnet` | `x402-merchant-api@near` | 4034 | `x402.mikedotexe.com` |
| `merchant-base.mikedotexe.com` | `eip155:8453` | `x402-merchant-api@base` | 4035 | `base.x402.mikedotexe.com` |

The deployment uses dedicated facilitator clients with exact payee policies and
small daily sponsorship budgets. No API key material, private key, or database
credential is included here.

## Discovery and unpaid-flow checks

Verified from outside the host:

- DNS A/AAAA records resolve to the existing host and the certificate covers
  both names;
- `/openapi.json`, `/llms.txt`, and `/.well-known/x402` return HTTP 200;
- all five routes are discoverable: account evidence, transaction evidence,
  activity search, entity lookup, and Base-USDC-to-NEAR-USDC route quote;
- `@agentcash/discovery` reports no schema or payment warnings on either
  origin and every route;
- valid unpaid account requests return HTTP 402 before chain application work;
- NEAR emits v2 `near:mainnet` with the canonical NEAR USDC asset,
  `count.mike.near`, and amount `1000`; and
- Base emits v2 `eip155:8453` with the canonical Base USDC asset,
  `0x7Ff46ab88688D528bCE3e59c470240c6901cF88c`, and amount `1000`.

The runtime 402 examples include the official Bazaar method, JSON input, and
output metadata. The empty activity index intentionally reports
`not_yet_indexed` until a finality-aware index is installed.

## Paid-flow evidence

### NEAR mainnet

On 2026-07-27, the explicitly confirmed proof payment completed end to end:

- network: `near:mainnet`;
- asset: Circle USDC NEP-141
  `17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1`;
- amount: `1000` atomic units (`$0.001000`);
- payer: `mike.near`;
- payee: `count.mike.near`;
- facilitator relayer: `x402-relayer2.mike.near`;
- sponsored execution cap: 30 TGas, within the configured 0.01 NEAR daily
  sponsorship budget.

The client received HTTP 402, created one NEP-366 payment authorization, and
retried the same request with the x402 v2 payment header. The merchant then
returned HTTP 200 with final account evidence for `mike.near`, including block
height `208801546` and block hash
`5SRSREzDSaLUA5QV8UKTHAXP8y5diLfHc7MK6Joe6fQc`. The facilitator returned
`success: true` and transaction
`5dm822stypkWdK7A5s2owV9QBPbh4uZhLPoWou2mw4zs`.

Independent `near tx-status` reconciliation at finality showed successful
receipt execution and the NEP-141 event transferring exactly `1000` units from
`mike.near` to `count.mike.near`:

<https://nearblocks.io/txns/5dm822stypkWdK7A5s2owV9QBPbh4uZhLPoWou2mw4zs>

No private key, signed delegate, or payment header was recorded. A duplicate
replay test was not attempted because the original signed authorization was
intentionally not retained.

### Base mainnet

On 2026-07-27, a dedicated proof payer was created for this merchant and
funded with 1 USDC on Base. The private key is retained only in the EC2 host's
root-owned merchant credential store and is not included in this repository.

The explicitly confirmed proof payment used:

- network: `eip155:8453` / Base mainnet;
- asset: Circle USDC
  `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`;
- amount: `1000` atomic units (`$0.001000`);
- payer: `0xcA202E03f11Aa076c57EAE666Da3f933dCc71CC9`;
- payee and facilitator signer:
  `0x7Ff46ab88688D528bCE3e59c470240c6901cF88c`;
- maximum sponsored execution: 120,000 gas at the configured 1 gwei cap
  (`0.00012 ETH`).

The payer balance decreased from 1,000,000 to 999,000 atomic USDC. Independent
Base RPC reconciliation found the matching USDC `Transfer` event and a
successful receipt (`status: 0x1`):

- transaction: `0x5376373cceaae0bc078129c61163b3439f1377099ba034e6f4f895c4cb66f28d`;
- block: `0x2eeaee1`;
- block hash:
  `0xbf314e5f9a91bd3174f5fe33fc1ec01fc587212f6300f5c98f1a99cfd7176ebd`;
- gas used: `0x14ed8`.

<https://basescan.org/tx/0x5376373cceaae0bc078129c61163b3439f1377099ba034e6f4f895c4cb66f28d>

The original one-off remote proof runner did not emit its post-request JSON
summary, so the HTTP response status was not captured in that terminal
transcript. The successful on-chain transfer and receipt establish settlement;
this was a runner observability gap, separate from the merchant and facilitator
services.

No private key, signed authorization, or payment header was recorded. The
persistent payer credential is intended for later low-value demonstrations;
future payments still require an immediate confirmation and should use a new
authorization each time.

## Post-deployment proof-runner hardening

Later on 2026-07-27, immutable release
`20260727-runner-hardening` replaced the one-off paid-flow script with the
checked-in `examples/merchant-api/paid-flow-proof.mjs` operator runner. It:

- validates the live 402 network, asset, amount, and payee against explicit
  expected values before signing;
- prints an exact confirmation preview and requires its derived confirmation
  token;
- requires a durable result path for funded runs;
- atomically records a sanitized `broadcasting` checkpoint before sending;
- emits a structured settlement or failure result without payment payloads or
  headers; and
- treats timeout, missing or malformed settlement headers, and final-result
  persistence failure as requiring reconciliation before retry.

The release passed 19 merchant tests locally and on the EC2 host. A deployed
no-broadcast Base preview returned `confirmation_required`, exited with the
documented code, and wrote a mode-0600 result owned by the dedicated Base
merchant account. No new authorization was signed and no additional payment
was made for this validation.

Both public origins now return HTTP 200 at `/` with links to their discovery
documents. Public checks confirmed HTTP 200 for `/openapi.json`, `/llms.txt`,
`/.well-known/x402`, and `/healthz`, plus canonical v2 HTTP 402 responses for
valid unpaid requests. The current `@agentcash/discovery` package discovered
all four routes per origin and `check` completed without warnings for each of
the eight concrete route URLs. Both services remained active with zero
automatic restarts after promotion.

Release `20260727-browser-cors` then added an exact browser-origin allowlist
for `https://js.fastnear.com`. Live validation on both origins showed:

- allowed OPTIONS preflight returns HTTP 204 before payment middleware;
- `PAYMENT-SIGNATURE` is accepted as a browser request header;
- `PAYMENT-REQUIRED`, `PAYMENT-RESPONSE`, and the legacy response alias are
  exposed to browser JavaScript;
- an actual unpaid request still returns canonical HTTP 402 with those CORS
  headers; and
- an unlisted origin receives HTTP 403 on preflight.

The CORS release passed 23 merchant tests locally and on the host. It did not
sign an authorization or move funds.

## Quote-only Base USDC to NEAR USDC route

Immutable release `20260727-usdc-route-v2` added
`POST /v1/routes/usdc/quote` to both payment origins. The capability is fixed
to:

- source: Base mainnet canonical USDC
  `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`;
- destination: NEAR mainnet canonical USDC
  `17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1`;
- provider: NEAR Intents 1Click; and
- behavior: signed dry quote only, with `fundsMoved: false` and no deposit
  address.

The official live 1Click token registry reported the corresponding asset IDs:

- `nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near`;
- `nep141:17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1`.

A direct dry provider quote for 1,000,000 atomic Base USDC returned:

- expected output: 998,898 atomic NEAR USDC;
- minimum output: 988,909 atomic NEAR USDC;
- estimated settlement: 35 seconds;
- refund-fee field: 2,400 atomic units; and
- withdraw-fee field: 0.

These values are a dated provider response, not a promised execution result.
The merchant validates the provider's signed route, amount, assets, recipient,
refund address, deadline, and slippage fields before returning a normalized
response. Conflicting, malformed, timed-out, and non-success provider
responses fail closed.

The first public challenge exposed an nginx integration defect:
schema-complete Bazaar metadata produced a roughly 6.7 KB encoded
`PAYMENT-REQUIRED` header, larger than the platform-default upstream response
header buffer. nginx returned HTTP 502 even though the merchant correctly
returned HTTP 402 on localhost. The checked-in deployment config now uses a
16 KB upstream header buffer with bounded 16 KB proxy buffers. After nginx
validation and reload, both public origins returned canonical HTTP 402 with
the full header.

Final validation showed:

- both services active with zero automatic restarts on
  `20260727-usdc-route-v2`;
- 28 local merchant tests and the route test subset on the EC2 host passed;
- AgentCash `discover` found five routes per origin;
- AgentCash `check` completed without warnings for all ten concrete route
  URLs;
- valid and malformed unpaid quote requests both reached HTTP 402 before
  application validation;
- runtime Bazaar and OpenAPI input schemas were identical;
- runtime Bazaar and OpenAPI success-output schemas were identical; and
- browser CORS exposed the complete challenge to
  `https://js.fastnear.com`.

No x402 authorization was signed for this release, no paid quote call was
made, no deposit address was created, and no USDC or gas asset moved.
