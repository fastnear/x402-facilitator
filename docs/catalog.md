# Merchant discovery catalog

Each facilitator instance exposes `GET /discovery/resources` using the x402
Bazaar `DiscoveryResourcesResponse` vocabulary. The catalog is a public,
read-only index of independently operated x402 resources that explicitly opted
in. It is not generated from API clients, settlement records, payer history, or
operator telemetry.

The checked-in [`resources.json`](catalog/resources.json) manifest is embedded
in the facilitator binary and included in the signed release archive. Every
entry therefore requires review, a merged commit, and an immutable release.
The service parses and validates the complete manifest before listening, then
serves only entries whose network and canonical Circle USDC asset match that
process. Invalid catalog data fails startup.

## Public contract

The endpoint accepts the official list-client filters `type`, `payTo`,
`scheme`, `network`, and `extensions`, plus `limit` and `offset`. The default
limit is 100 and the maximum is 1,000. Unknown, duplicate, malformed, or
out-of-range parameters return HTTP 400. Results sort by `lastUpdated`
descending and then resource URL ascending. EVM recipient filters ignore
address case; other fields match exactly.

Every response item is x402 v2, `exact`, and tied to one of the repository's
four canonical network/USDC profiles. Base entries carry the real token EIP-712
domain (`USD Coin` / `2` on mainnet and `USDC` / `2` on Sepolia). Provider
admission evidence remains in the manifest for reviewers but is never included
in the public response.

The API deliberately omits semantic search, merchant lookup, MCP proxying,
quality scores, availability rankings, settlement counts, and volume.

## Admission and removal

An independent merchant is eligible only after all of these checks pass:

1. The operator explicitly consents to public catalog publication.
2. The exact public HTTPS method/path returns a canonical unpaid x402 402.
3. Public OpenAPI or Bazaar metadata documents that resource and its payTo.
4. Public evidence ties the payTo to the independent merchant operator.
5. A dedicated per-instance credential is issued with zero settlement budget,
   and a private read-only `/verify` succeeds against the exact network, asset,
   and payTo policy.

No settlement is required for publication. Listing never represents a claim of
activity, availability, quality, or volume. The facilitator operator's demos,
canary wallets, and merchant examples are categorically ineligible.

Admission evidence must be public HTTPS material and must not contain an API
key, private key, signed payment authorization, raw transaction, credentialed
URL, or other bearer instrument. A listing PR records `reviewedAt`,
`optInEvidenceUrl`, and `payToControlEvidenceUrl`; CI validates its Bazaar
`info` against its declared JSON Schema through the pinned official extension
package.

Generate `extensions.bazaar` with the pinned official
`declareDiscoveryExtension` helper used by the resource server, then copy its
JSON output into the manifest. Do not hand-author or relax that schema: CI runs
both the official structural validator and the `info`-against-schema validator.
The Rust startup boundary independently enforces bounded object metadata and
requires object-valued `info` and `schema` fields before the instance listens.

The merchant may request removal at any time through the same public or
existing authenticated private channel. A changed URL, network, asset, payTo,
or Bazaar schema requires a new review rather than an in-place unreviewed edit.
