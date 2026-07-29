# x402 paid-work reference server

This runnable Express service protects `POST /work` with an exact Circle USDC
payment on NEAR or Base. One codebase:

- registers the official NEAR or EVM server scheme selected by `NETWORK`;
- emits canonical v2 `PAYMENT-REQUIRED` requirements;
- accepts canonical v2 `PAYMENT-SIGNATURE` payments;
- optionally accepts legacy v1 `X-PAYMENT` on EVM and translates it before the
  official middleware;
- retries transient facilitator failures with bounded backoff;
- independently deduplicates delivered work when a payment identifier is
  supplied.

The delivery journal is intentionally in-memory. A production resource server
must replace it with durable, transactional storage shared by every instance;
facilitator settlement idempotency does not by itself prevent duplicate
application delivery.

## Configure

Install the pinned dependencies:

```sh
npm ci
```

Set one profile. Values below are placeholders; use the canonical asset and an
exact payee authorized for your facilitator API client.

| Variable | NEAR example | Base example |
| --- | --- | --- |
| `FACILITATOR_URL` | `https://test.x402.example` | `https://base-test.x402.example` |
| `NETWORK` | `near:testnet` | `eip155:84532` |
| `ASSET` | canonical testnet USDC account | canonical Base Sepolia USDC address |
| `PAY_TO` | exact NEAR recipient | exact EVM recipient |
| `AMOUNT` | `1000` | `1000` |
| `ASSET_EIP712_NAME` | unset | `USDC` on Base Sepolia; `USD Coin` on Base mainnet |
| `ASSET_EIP712_VERSION` | unset | `2` |
| `PORT` | `4021` | `4021` |
| `RESOURCE_URL` | public HTTPS `/work` URL | public HTTPS `/work` URL |

Canonical EVM profiles are:

| Network | Circle USDC asset | EIP-712 name / version |
| --- | --- | --- |
| `eip155:84532` (Base Sepolia) | `0x036CbD53842c5426634e7929541eC2318f3dCF7e` | `USDC` / `2` |
| `eip155:8453` (Base mainnet) | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | `USD Coin` / `2` |

The EIP-712 name is the token contract's real domain name, not its display
symbol. The public reference deployment has Base mainnet, but does not
currently claim a live Base Sepolia instance.

The API key must be stored in a mode-0600 regular file:

```sh
export FACILITATOR_URL=https://test.x402.example
export FACILITATOR_API_KEY_FILE=/secure/path/resource-server-api-key
export NETWORK=near:testnet
export ASSET=your-canonical-usdc-asset
export PAY_TO=your-exact-payee
export AMOUNT=1000
export PORT=4021
npm start
```

For EVM, also export the matching `ASSET_EIP712_NAME` and
`ASSET_EIP712_VERSION` from the table above.

`FACILITATOR_API_KEY_FILE` must contain exactly one newline-terminated key. The
service does not accept the key directly in an environment variable and does
not log it. Provision a separate key for every deployed resource-server
instance and environment; never copy a production key into a test deployment.

## Exercise

An unpaid request returns `402 Payment Required`:

```sh
curl -i \
  -H 'Content-Type: application/json' \
  --data '{"input":"hello"}' \
  http://127.0.0.1:4021/work
```

A compatible client signs the advertised requirements and resubmits. A valid
payment returns a deterministic SHA-256 result. Replaying the identical
payment identifier and work returns the stored result without another
settlement; reusing the identifier for different work returns 409.

Legacy v1 is never enabled for NEAR. On EVM, the reference server emits a v1
compatibility body and accepts `X-PAYMENT` only when the configured facilitator
also enables `accept_v1`.

## Tests

```sh
npm run check
```

The suite covers delivery-journal conflicts, bounded retries, the strict v1
translation contract, and import hygiene. It uses no funded credentials and
broadcasts no transaction.
