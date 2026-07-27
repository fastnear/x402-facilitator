# Reference-instance access

The public reference facilitators are API-key gated. Their public landing page,
`/supported`, liveness, and sanitized readiness endpoints require no
credential; `/verify` and `/settle` require an allowlisted resource-server
client.

Access is manually reviewed and is intended for real x402 integrations,
interoperability work, and bounded evaluation. It is not an anonymous public
relay and carries no availability SLA.

## Public reference instances

| Network | Facilitator URL | Canonical Circle USDC asset |
| --- | --- | --- |
| `near:mainnet` | `https://x402.mikedotexe.com` | `17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1` |
| `near:testnet` | `https://test.x402.mikedotexe.com` | `3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af` |
| `eip155:8453` | `https://base.x402.mikedotexe.com` | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` |

Base Sepolia is a configured rollout target, not a claimed live reference
instance.

The reference instances implement the `exact` scheme and prefer canonical
x402 v2. They charge no facilitator fee; gas sponsorship is bounded by the
client's policy and daily budget. NEAR accepts classic NEP-366 delegated
NEP-141 transfers. Base accepts EOAs and deployed EIP-1271 wallets and rejects
counterfactual EIP-6492 authorizations as unsupported.

Each base URL exposes:

- `GET /supported` for the live network, wire kinds, extensions, and signer;
- `POST /verify` and `POST /settle` for authenticated x402 requests;
- `GET /healthz` for liveness; and
- `GET /readyz` for sanitized operational readiness.

The implementation is Apache-2.0 licensed. See the
[source and API documentation](https://github.com/fastnear/x402-facilitator),
[security policy](https://github.com/fastnear/x402-facilitator/security/policy),
and [dated paid-flow evidence](evidence/).

## Request access

Open a
[reference-access request](https://github.com/fastnear/x402-facilitator/issues/new?template=access_request.yml)
with:

- the NEAR or Base network and environment;
- a public description or repository for the resource server;
- each exact USDC recipient account or address that must be allowed;
- expected verification and settlement rates, daily settlement count, and
  evaluation duration; and
- whether the integration uses canonical x402 v2 or needs the gated EVM v1
  compatibility transport.

The issue is public. Never include a payer or signer private key, API key,
signed payment authorization, credentialed URL, raw transaction bytes, or
other bearer instrument. The operator will arrange one-time credential
delivery outside the issue if the request is approved.

Requests may be declined or assigned conservative rate and sponsorship limits.
Keys expire or are reviewed, may be revoked for abuse or inactivity, and are
valid only for the exact network, canonical USDC asset, and recipient policy
approved for that client.

## Use the credential

Send the issued secret in `X-API-Key` on `/verify` and `/settle`.
`Authorization: Bearer` is supported as an alternative. If both headers are
present, they must carry the identical value.

Store the secret in a secret manager or a mode-0600 credential file. Do not put
it in source control, browser code, logs, command-line arguments, screenshots,
or issue text. The key authenticates the resource server to the facilitator; it
is not a payer credential.

Operators and self-hosters should follow the full
[API-key administration policy](api-keys.md). Teams that need independent
policy, availability, or sponsorship control should
[run their own instance](../README.md#run-your-own-instance).
