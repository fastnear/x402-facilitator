# x402 agent merchant API

This companion resource server exposes paid chain evidence and a bounded
activity index. It consumes the facilitator through the official x402 server
middleware; it does not add merchant or data-product routes to the facilitator.

The same application runs as a separate NEAR or Base deployment. It accepts
only these exact network/asset profiles:

| Network | Circle USDC asset | EIP-712 domain |
| --- | --- | --- |
| `near:mainnet` | `17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1` | n/a |
| `near:testnet` | `3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af` | n/a |
| `eip155:8453` | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | `USD Coin` / `2` |
| `eip155:84532` | `0x036CbD53842c5426634e7929541eC2318f3dCF7e` | `USDC` / `2` |

The first two paid operations are:

- `POST /v1/evidence/account`
- `POST /v1/evidence/transaction`

The indexed expansion adds `POST /v1/activity/search` and
`GET /v1/entities/:identifier`.

The route-intelligence example adds:

- `POST /v1/routes/usdc/quote`

It requests a dry quote from NEAR Intents 1Click for canonical Base USDC to
canonical NEAR USDC and preserves the provider-supplied signature as
provenance. This service validates the returned route fields but does not claim
to verify that signature cryptographically because the provider does not
publish a verification scheme. It does not return a deposit address, sign a
transaction, or move funds. Execution and status monitoring are intentionally
separate future phases.

## Configure

Install the pinned dependencies with `npm ci`. Set the facilitator API key in a
mode-0600 file and provide a public origin for the resource metadata.

```sh
export FACILITATOR_URL=https://test.x402.example
export FACILITATOR_API_KEY_FILE=/secure/path/resource-server-api-key
export NETWORK=near:testnet
export RPC_URL=https://rpc.testnet.near.org
export ASSET=3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af
export PAY_TO=merchant.testnet
export RESOURCE_ORIGIN=https://merchant.example
export CORS_ORIGINS=https://your-browser-app.example
export AMOUNT=1000
export PORT=4031
npm start
```

For Base mainnet, set the canonical asset above. The application derives the
required `USD Coin` / `2` domain itself; optional
`ASSET_EIP712_NAME`/`ASSET_EIP712_VERSION` settings are accepted only when
they exactly match the selected profile. `AMOUNT` defaults to `1000` atomic
USDC ($0.001000), and the landing page, OpenAPI document, and runtime payment
requirements are all derived from that same integer.

All remote endpoints must use HTTPS. `RESOURCE_ORIGIN`,
`FACILITATOR_URL`, and `ONE_CLICK_PROVIDER_ORIGIN` must be origins without a
path. RPC URLs may include paths and query strings; explorer base URLs may
include paths but not queries. Neither may contain credentials or fragments.
Payees, ports, and amounts are validated before any listener starts.

Credential paths must resolve directly to owner-only regular files of at most
4 KiB containing exactly one nonempty line. Symbolic links and
group/world-readable files are rejected. The sole deployment exception is
systemd v255's `LoadCredential` ACL representation: the exact
`CREDENTIALS_DIRECTORY/facilitator-api-key` path may appear root-owned mode
0440 because its group bit is the named-service-user ACL mask, not group
access. No other mode-0440 credential is accepted. Give each resource-server
instance a separate facilitator API key restricted to its exact network,
asset, and payee.

`ONE_CLICK_PROVIDER_ORIGIN` defaults to
`https://1click.chaindefuser.com`. `ONE_CLICK_JWT_FILE` is optional and, when
set, must point to a single-line credential file. The JWT is never accepted
directly through an environment variable.

`ACTIVITY_INDEX_FILE` is optional. When provided, it must contain a JSON array
of final, normalized activity records. An empty index is valid and reports
`not_yet_indexed` rather than inventing activity.

## Discovery

The server exposes `/openapi.json`, `/llms.txt`, `/.well-known/x402`,
`/pricing`, `/terms`, and `/robots.txt`. `/pricing` derives the exact
six-decimal USD display price, network, asset, recipient, and (on EVM) EIP-712
domain from the active configuration; `/terms` is a fetchable operational
terms page linked from OpenAPI. Protected routes declare the official Bazaar
discovery extension in their runtime x402 requirements as well as their
OpenAPI schemas.

`GET /` provides a small human-readable service page linking those discovery
documents. It is informational only; paid operations remain under `/v1/`.
`GET /healthz` is process liveness and never depends on an upstream.
When installed by the production merchant installer, `/healthz.release.id` and
OpenAPI `info.x-x402-merchant-release-id` publish the same validated immutable
release ID. Local checkouts intentionally omit both fields. The identifier is
deployment provenance, not an integrity signature; compare it with the merged
commit, archived SHA-256, immutable release pointer, and dated rollout record.
`GET /readyz` checks the configured RPC chain identity and a canonical
final/finalized block, the facilitator's readiness plus advertised canonical
x402 v2 network, and successful payment server initialization. Concurrent probes share a one-second completed
snapshot, including a failed result, so readiness monitoring cannot amplify
upstream RPC/facilitator work. It returns HTTP 503 with `Retry-After: 1` if any
check fails. Startup performs those same checks before listening.

The merchant-owned facilitator transport rejects redirects for every request,
including payment-bearing `/verify` and `/settle` calls. It uses a bounded
seven-second per-attempt deadline and 64 KiB response limit; the prescribed
retry envelope remains within the reverse proxy timeout.

`CORS_ORIGINS` is an optional comma-separated exact-origin allowlist for
browser clients. Allowed preflights terminate with HTTP 204 before payment
middleware, and browser responses expose canonical `PAYMENT-REQUIRED` and
`PAYMENT-RESPONSE` headers. Wildcard origins and origins containing paths are
rejected at startup.

## Paid-flow proof runner

`npm run proof` is an operator tool for live evidence collection. It first
makes an unpaid request, validates the returned network, asset, amount, and
payee against explicit expected values, and prints an exact confirmation
preview. It will not sign until `PROOF_CONFIRMATION_TOKEN` matches the token in
that preview.

For a confirmed run, `PROOF_RESULT_FILE` is mandatory. The runner writes a
sanitized `broadcasting` checkpoint before sending the paid request and then
atomically replaces it with the final result. A timeout, missing settlement
header, or invalid settlement header is `indeterminate` and must be reconciled
before another authorization is created. The runner never retries a paid
request and never writes the payer key, payment payload, or payment header.

Required settings:

```text
PROOF_URL
PROOF_BODY_JSON
PROOF_EXPECTED_NETWORK
PROOF_EXPECTED_ASSET
PROOF_EXPECTED_AMOUNT
PROOF_EXPECTED_PAY_TO
PROOF_PAYER
PROOF_PAYER_KEY_FILE
PROOF_RPC_URL
PROOF_FACILITATOR_SIGNER
PROOF_MAX_SPONSORED_GAS
```

Run once without `PROOF_CONFIRMATION_TOKEN` to obtain the exact preview. After
the repository's fresh human confirmation, rerun with that token and a
root-owned or service-owned `PROOF_RESULT_FILE`.

## Public regression check

`npm run regression` exercises both production origins without signing or
broadcasting a payment. It verifies public discovery and listing-surface
documents, all ten unpaid 402 challenges, canonical network/asset/payee/amount
fields, the 12 KB upstream header safety margin, Bazaar/OpenAPI schema parity,
validation ordering, the production browser CORS allowlist, and (when run from
an installed release) matching public release provenance. Run that default full
check before and after a complete promotion.

During a one-instance-at-a-time rollout, use the exact target switch for the
post-promotion gate so the newly promoted instance is not coupled to the other
instance before it is upgraded:

```sh
npm run regression -- --target near
npm run regression -- --target base
```

Only `near` and `base` are accepted. Each scoped check remains entirely unpaid;
run the default no-argument command after both instances are promoted.

## Safety

The server performs read-only chain RPC calls and dry route-quote requests. It
never creates keys, returns a route deposit address, signs payer
authorizations, or broadcasts a transaction. A live paid-flow test still
requires an explicitly approved payment under the repository network and funds
policy. The proof runner is a separate operator command and is never started by
the merchant service.
