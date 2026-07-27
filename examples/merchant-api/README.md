# x402 agent merchant API

This companion resource server exposes paid chain evidence and a bounded
activity index. It consumes the facilitator through the official x402 server
middleware; it does not add merchant or data-product routes to the facilitator.

The same application runs as a separate NEAR or Base deployment by setting
`NETWORK`, `RPC_URL`, `ASSET`, and `PAY_TO`. The first two paid operations are:

- `POST /v1/evidence/account`
- `POST /v1/evidence/transaction`

The indexed expansion adds `POST /v1/activity/search` and
`GET /v1/entities/:identifier`.

The route-intelligence example adds:

- `POST /v1/routes/usdc/quote`

It requests a signed, dry quote from NEAR Intents 1Click for canonical Base
USDC to canonical NEAR USDC. It does not return a deposit address, sign a
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
export ASSET=your-canonical-usdc-asset
export PAY_TO=merchant.testnet
export RESOURCE_ORIGIN=https://merchant.example
export CORS_ORIGINS=https://your-browser-app.example
export PORT=4031
npm start
```

`ONE_CLICK_PROVIDER_ORIGIN` defaults to
`https://1click.chaindefuser.com`. `ONE_CLICK_JWT_FILE` is optional and, when
set, must point to a single-line credential file. The JWT is never accepted
directly through an environment variable.

`ACTIVITY_INDEX_FILE` is optional. When provided, it must contain a JSON array
of final, normalized activity records. An empty index is valid and reports
`not_yet_indexed` rather than inventing activity.

## Discovery

The server exposes `/openapi.json`, `/llms.txt`, and `/.well-known/x402`.
Protected routes declare the official Bazaar discovery extension in their
runtime x402 requirements as well as their OpenAPI schemas.

`GET /` provides a small human-readable service page linking those discovery
documents. It is informational only; paid operations remain under `/v1/`.

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

## Safety

The server performs read-only chain RPC calls and dry route-quote requests. It
never creates keys, returns a route deposit address, signs payer
authorizations, or broadcasts a transaction. A live paid-flow test still
requires an explicitly approved payment under the repository network and funds
policy. The proof runner is a separate operator command and is never started by
the merchant service.
