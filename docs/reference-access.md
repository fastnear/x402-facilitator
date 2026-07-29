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
counterfactual EIP-6492 authorizations as unsupported. Base mainnet USDC's
real EIP-712 domain is name **`USD Coin`**, version **`2`**; do not substitute
the `USDC` token symbol. Base Sepolia uses a different contract and domain and
is not a live public reference instance.

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

- one or more active NEAR mainnet, NEAR testnet, or Base mainnet networks;
- a public HTTPS resource-server URL, repository, or integration document;
- each exact USDC recipient controlled by that resource-server operator;
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

### Base merchant pilot

The [Base merchant-pilot request](https://github.com/fastnear/x402-facilitator/issues/new?template=base_merchant_pilot.yml)
is stricter than a general evaluation request. It is for one public
`eip155:8453` resource-server deployment that can be independently attributed:

- name the exact public HTTPS paid method and path that returns an unpaid 402;
- link public OpenAPI or discovery that names that operation and its `payTo`;
- give the Base mainnet recipient and public evidence that the merchant controls
  it (for example, a service page, public source/configuration, or explorer
  profile); and
- state expected use and accept the default 60 `/verify` and 10 `/settle`
  requests per minute, the existing gas cap, and a zero daily sponsorship
  budget until the verify-first gate completes.

The public request never needs a funded payer, signed authorization, API key,
or transaction. If approved, one per-instance key is delivered exactly once by
an authenticated out-of-band channel. The operator then confirms public
discovery and performs read-only verification before a positive settlement
budget can be enabled.

## Integrate end to end

1. Inspect the chosen instance's `/supported` response and confirm its
   canonical v2 network, signer, and `payment-identifier` extension. Require
   `/readyz` to return HTTP 200 before an integration test.
2. Submit the public access request. The operator creates a dedicated client
   with exact network, canonical asset, and recipient rows. Every deployed
   resource-server instance and environment gets a distinct client and key;
   never share a production credential with staging or another merchant.
   For verify-first onboarding, the client may start with
   `--daily-yocto-near 0`: `/verify` remains read-only, while a valid,
   allowlisted `/settle` returns `429 sponsorship_budget_exhausted` before the
   broadcast phase. Set a positive cap only after the read-only checks and
   settlement-approval gate; the legacy `*_yocto_near` name means the native
   atomic gas unit (wei on Base).
3. Receive the raw key exactly once through an authenticated private channel.
   Store it in a secret manager or mode-0600 credential file and configure the
   resource server, not browser or payer code.
4. Configure canonical payment requirements. For Base mainnet use network
   `eip155:8453`, asset
   `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`, and EIP-712 domain
   `USD Coin` / `2`. Use the exact payee approved in the access request.
5. Start with `/verify`, which never broadcasts. Confirm malformed and invalid
   requests fail closed. The public request never contains a signed
   authorization; any valid verification occurs privately only after the
   out-of-band key delivery and policy review. Do not enable `/settle` until
   that read-only gate is complete.
6. Enable settlement only after delivery idempotency is durable. Retry
   transient facilitator errors with bounded backoff using the byte-identical
   signed payload; never create a replacement payment for an indeterminate
   submission.
7. Monitor `/readyz`, bounded error metrics, settlement outcomes, and the
   merchant's own delivery journal. Rotate or revoke the per-instance key if
   it is exposed or the approved recipient changes.

The runnable [Express resource server](../examples/resource-server/README.md)
shows the environment contract, official middleware, bounded retries, and
payment-identifier delivery behavior. The
[OpenAPI contract](openapi.yaml) defines the exact facilitator request and
response shapes.

## Use the credential

Send the issued secret in `X-API-Key` on `/verify` and `/settle`.
`Authorization: Bearer` is supported as an alternative. If both headers are
present, they must carry the identical value.

Do not put the key in source control, browser code, logs, command-line
arguments, screenshots, or issue text. It authenticates one resource-server
instance to the facilitator; it is not a payer credential and must not be
copied into payer software.

Operators and self-hosters should follow the full
[API-key administration policy](api-keys.md). Teams that need independent
policy, availability, or sponsorship control should
[run their own instance](../README.md#run-your-own-instance).
