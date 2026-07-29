# Agent-facing merchant API

Status: implemented and deployed as separate NEAR and Base mainnet companion
resource servers.

Date: 2026-07-27

> **Historical deployment note:** the deployment and paid-flow observations in
> this dated research record describe the July 27 release only. They do not
> prove that a later source revision is live. Re-run the unpaid regression and
> record new evidence after every immutable promotion; see the
> [x402-list review](../evidence/2026-07-28-x402-list-review.md) for the
> independent-adoption boundary.

## Purpose

The facilitator is settlement infrastructure. The merchant product belongs in a
separate resource server that an agent can discover, pay, and invoke. The same
capability should run as separate NEAR and Base deployments because the current
facilitator is pinned to one network per process.

This gives x402 Scan a meaningful NEAR merchant resource instead of another
synthetic payment demo. x402 Scan's current NEAR limitation is upstream: its
open support issue reports that the index currently accepts Base and Solana.

- [x402 Scan NEAR support issue #1040](https://github.com/Merit-Systems/x402scan/issues/1040)
- [x402 Scan discovery specification](https://www.x402scan.com/discovery/spec)

## Implemented low-lift API

`examples/merchant-api/` provides a read-only, paid chain-evidence service:

| Operation | NEAR input | Base input |
| --- | --- | --- |
| `POST /v1/evidence/account` | `{ "accountId": "alice.near" }` | `{ "address": "0x..." }` |
| `POST /v1/evidence/transaction` | `{ "transactionHash": "...", "signerId": "alice.near" }` | `{ "transactionHash": "0x..." }` |

Responses include the configured network, finality/terminality, block identity,
normalized account or transaction evidence, source status, freshness timestamp,
and an explorer URL when configured. RPC timeouts, malformed evidence, and
unknown results fail closed with an explicit unavailable/error response.

The default price is `1000` atomic USDC units, represented as `$0.001000` in
OpenAPI. The implementation never creates keys, signs payer authorizations, or
broadcasts transactions.

## Implemented bounded indexed layer

The same service also exposes:

- `POST /v1/activity/search`
- `GET /v1/entities/{identifier}`

The index is loaded from an operator-provided JSON file and is intentionally
bounded. Empty or incomplete data returns `status: "not_yet_indexed"`; the
service never presents an empty index as authoritative history. This is the
safe first contract for a later finality-aware NEAR/Base ingestion worker.

Each indexed response includes records, cursors, index status, record count, and
index freshness. A future ingestion worker must only publish final evidence and
must retain block provenance for every record.

## Implemented cross-chain route intelligence

The same companion API now exposes:

- `POST /v1/routes/usdc/quote`

This operation is fixed to canonical Base mainnet USDC as the source and
canonical NEAR mainnet USDC as the destination. The request supplies an atomic
USDC amount, NEAR recipient, Base refund address, and optional slippage limit.
The response normalizes a signed NEAR Intents 1Click dry quote: expected and
minimum output, fee fields, estimated settlement time, expiry, provider
signature, and provenance.

The safety boundary is explicit: the response says `mode: "quote_only"` and
`fundsMoved: false`; it never returns a deposit address, signs a source
transaction, or broadcasts funds. The route is useful paid intelligence, not a
custodial bridge.

This is the correct first rail because NEAR Intents supports both Base and
NEAR, and its official flow is token discovery → quote → origin-chain deposit
→ status polling. Circle CCTP supports Base but does not currently list NEAR as
a domain, so this must not be described as a direct CCTP transfer.

- [NEAR Intents 1Click request flow](https://docs.near-intents.org/integration/distribution-channels/1click-api/quickstart/making-a-request)
- [NEAR Intents supported chains](https://docs.near-intents.org/resources/chain-support)
- [Circle CCTP supported domains](https://developers.circle.com/cctp/concepts/supported-chains-and-domains)

The live 1Click token registry on 2026-07-27 identified:

- Base USDC:
  `nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near`;
- NEAR USDC:
  `nep141:17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1`.

A live dry quote for 1,000,000 atomic Base USDC returned 998,898 atomic NEAR
USDC, a 988,909 minimum, and a 35-second estimate. Those values are dated
evidence, not guarantees; callers must use the fresh signed quote returned by
the API.

The next safe expansion is non-custodial preparation plus monitoring:

1. an explicitly executable quote that returns the provider deposit address
   only after surfacing amount, minimum output, fees, deadline, recipient, and
   refund address;
2. a free or paid status route keyed by deposit address and source transaction
   hash; and
3. no server-side fund movement until a separate custody, authorization,
   idempotency, refund, and broadcast-risk review is complete.

## Discovery contract

The companion server serves:

- `/` — a small human-readable service page linking the discovery surfaces;
- `/openapi.json` — canonical OpenAPI 3.1 contract with `x-payment-info`,
  decimal USD pricing, request and response schemas, `402` responses, contact
  metadata, `info.termsOfService`, and `info.x-guidance`;
- `/llms.txt` — concise agent instructions;
- `/pricing` — exact configured six-decimal price, network, asset, recipient,
  and EIP-712 domain where applicable;
- `/terms` — fetchable operational terms page;
- `/robots.txt` — crawler policy; and
- `/.well-known/x402` — compatibility fan-out for x402 Scan.

Runtime payment requirements declare the official Bazaar discovery extension
from the pinned `@x402/extensions` package. This matters because OpenAPI alone
was insufficient for the current AgentCash validator: the existing synthetic
demo exposed static schemas but omitted the runtime Bazaar input/output
metadata.

Validate each deployed origin with the current AgentCash discovery tools:

```sh
npx -y @agentcash/discovery@latest discover https://merchant.example
npx -y @agentcash/discovery@latest check https://merchant.example/v1/evidence/account
```

The AgentCash merchant guidance recommends OpenAPI-first discovery and aligned
runtime `402` behavior. [AgentCash discovery guidance](https://agentcash.dev/docs/discovery)

AgentCash currently advertises wallet settlement on Base, Solana, and Tempo,
not NEAR. NEAR readiness therefore means that the merchant is discoverable and
speaks canonical x402 correctly; an AgentCash wallet must still add NEAR before
it can pay the NEAR instance directly. [AgentCash overview](https://agentcash.dev/learn/agentic-commerce)

MPP is deliberately not added in this first implementation. StableFeedback is
used as a compatibility benchmark, not copied as a payment dependency.

The deployed merchant also has an exact-origin browser CORS allowlist for
`https://js.fastnear.com`. Allowed OPTIONS preflights terminate before payment
middleware, and browser code may read `PAYMENT-REQUIRED` and
`PAYMENT-RESPONSE`. Wildcard origins are rejected.

## StableFeedback comparison and feedback loop

StableFeedback is a useful reference for agent-facing API design: free public
submission/read routes, wallet-identity routes, `/llms.txt`, complete OpenAPI
schemas, and a dynamically priced x402 resolve route.

- [StableFeedback OpenAPI](https://stablefeedback.dev/openapi.json)
- [StableFeedback agent instructions](https://stablefeedback.dev/llms.txt)

Compatibility defects found during validation should be submitted to
StableFeedback against the merchant origin and repository. Record the returned
feed URL in dated evidence; do not include credentials, signed payment data, or
private RPC details.

The schema-rich-header proxy compatibility finding was submitted after the
fix:

- [StableFeedback: schema-rich x402 402 headers need proxy buffer guidance](https://stablefeedback.dev/feedback/cms3qsrkg0000kq04fdk7ww0z)

## x402 Scan evidence packet

The dated mainnet evidence packet contains:

1. the NEAR and Base `/openapi.json` URLs;
2. decoded v2 `PAYMENT-REQUIRED` examples with atomic amounts;
3. runtime Bazaar `info`/`schema` examples;
4. successful Base and NEAR mainnet paid-flow evidence;
5. retry/indeterminate behavior in the hardened proof runner;
6. the exact x402 Scan NEAR upstream changes still required: chain enum,
   payment tooling, indexer, and UI support.

This separates merchant-side readiness from x402 Scan feature completion. No
synthetic transfers should be created to satisfy registry thresholds.

The partner packet should also state the exact upstream work x402 Scan must do:

- add `near:testnet` and `near:mainnet` to its supported chain model and
  validation enum;
- add NEAR payment requirement decoding and wallet/signing support to its
  paid-flow tooling;
- permit NEAR resources through discovery ingestion and indexing; and
- expose NEAR in the catalog and UI filters/details.

The merchant can supply the OpenAPI and runtime evidence, but cannot make a
Base/Solana-only index accept NEAR by changing its own discovery files.

## Current validation record

On 2026-07-27:

- `https://merchant-near.mikedotexe.com` and
  `https://merchant-base.mikedotexe.com` were deployed on the existing EC2
  host as separate one-network processes;
- current AgentCash `discover` found all five routes per origin and `check`
  completed without warnings for all ten concrete route URLs;
- `npm run regression` provides a no-payment production gate across both
  origins, all ten challenges, schema parity, validation ordering, CORS, and
  the reverse-proxy header-size safety margin;
- valid unpaid requests returned canonical v2 HTTP 402 requirements for
  `near:mainnet` and `eip155:8453`, each priced at 1,000 atomic USDC units and
  carrying Bazaar input/output metadata;
- explicitly confirmed paid account-evidence requests settled successfully on
  both mainnets;
- the hardened proof runner passed tests for structured success, timeout,
  missing/malformed settlement headers, persistence failure, and the
  no-retry-on-indeterminate invariant;
- exact-origin CORS preflight returned HTTP 204 for
  `https://js.fastnear.com`, exposed the canonical x402 headers, and rejected
  an unlisted origin with HTTP 403;
- the quote-only Base-USDC-to-NEAR-USDC operation emitted matching OpenAPI and
  Bazaar schemas on both payment origins, while malformed unpaid bodies still
  reached HTTP 402 before application validation;
- all five operations now share complete success-output schemas between
  OpenAPI and runtime Bazaar metadata, and NEAR transaction evidence returns
  and verifies its top-level block identity;
- nginx's upstream response buffer was raised from the platform default to
  accommodate the 6.7 KB schema-rich `PAYMENT-REQUIRED` header; and
- mocked chain/index tests covered finality pinning, pending/missing data,
  RPC failure, malformed and unknown input fields, conflicting block/receipt
  finality, pagination, duplicate identifiers, conflicting route quotes,
  provider failure, and provider timeout.

See the dated
[deployment and paid-flow evidence](../evidence/2026-07-27-agent-merchant-deployment.md).
The StableFeedback feed URL is recorded above. An x402 Scan submission is not
yet claimed.

## FastNear x402 landing-page save point

`js-example-berryclub/public/x402.html` now leads with “x402 on NEAR is live,”
mainnet/v2/exact/USDC status, and the two verified settlement links. It includes
an agent → merchant → facilitator → delivery flow, equivalent NEAR/Base
deployment cards, the new quote-only Base-USDC-to-NEAR-USDC operation, and a
clear merchant-ready versus x402-Scan-upstream split.

The page's free browser challenge inspector sends only unpaid valid requests
and decodes the resulting `PAYMENT-REQUIRED` headers. It has no wallet or
signing capability. Production CORS is restricted to
`https://js.fastnear.com`, so local preview correctly cannot call the live
origins; mocked browser interaction tests and direct production-origin CORS
checks cover the two halves independently.

The package quickstarts and safety constraints remain below the proof surface.
Any future paid browser demo must preserve explicit user action and show a
complete payment preview before asking a wallet to sign.

## Acceptance criteria

- The local service passes syntax and unit tests without funded credentials.
- AgentCash discovery reports no input/output schema or payment warnings.
- Valid unpaid requests reach HTTP `402` before application work runs.
- Runtime requirements decode as canonical v2 with correct network, asset,
  payee, atomic amount, and Bazaar metadata.
- OpenAPI and runtime schemas describe the same inputs and outputs.
- NEAR and Base deployments expose equivalent evidence semantics with
  network-specific identifiers.
- Tests cover malformed identifiers, RPC timeout/error, missing transaction,
  pending/finalized evidence, empty indexes, pagination, invalid cursors, and
  deterministic response shapes, plus malformed/conflicting route quotes,
  provider errors, and provider timeout.
- Live paid testing remains subject to the repository's explicit funded
  broadcast confirmation gate.
