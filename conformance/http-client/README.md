# HTTP and merchant conformance

This development-only harness uses the pinned official `@x402/core` package.
It is not part of either production Rust binary. Install and check it with:

```sh
npm --prefix conformance/http-client ci
npm --prefix conformance/http-client run check
```

`check.mjs` is the authenticated HTTP-client integration driver invoked by the
Rust tests. `./scripts/check-full.sh` supplies ephemeral PostgreSQL and runs
those tests against the official client.

## Merchant offer preflight

Before reviewing an access request, capture the decoded `PAYMENT-REQUIRED`
JSON from the exact intended unpaid method/path. Separately record the live
HTTP status (which must be 402), method, source revision and capture time.
The preflight reads two local JSON files; it sends no network request and
never initializes a facilitator or loads applicant code.

Write the independently reviewed terms to a second file, for example:

```json
{
  "resourceUrl": "https://merchant.example/inventory",
  "requirements": {
    "scheme": "exact",
    "network": "near:testnet",
    "asset": "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af",
    "amount": "1000",
    "payTo": "merchant.testnet",
    "maxTimeoutSeconds": 60
  }
}
```

The URL and recipient above are synthetic examples. Obtain real terms from
the access request and independently checked public operator evidence. Do not
copy an untrusted offer into the expectations file and call that a review.
Preserve any reviewed `extra` object exactly, including an explicitly empty
object; do not rewrite the merchant's offer into a more convenient shape.

```sh
npm --prefix conformance/http-client run preflight -- \
  /absolute/path/unsigned-challenge.json /absolute/path/reviewed-terms.json
```

A pass returns JSON with `ok: true`, `scope: "unsigned_offer_only"`, the pinned
network and selected offer index. Failure returns a stable error code and
field path on stderr with exit status 1. Input files are limited to 64 KiB;
errors never echo input values, URLs, filenames or underlying parser messages.
Use only unsigned public offer data. Never supply a key, signed payment,
verification request, settlement response or other bearer instrument.

Checks include the official v2 schema, exact resource URL and reviewed terms,
one unambiguous `exact` offer on the reviewed network, canonical Circle USDC,
integer timeout/atomic-amount encoding and the official static-term matcher.
Unknown fields in the selected requirement or forwarded resource metadata
are rejected before schema parsing can discard them, matching this
facilitator's strict request boundary. Extra metadata must match the reviewed
snapshot in full, even where the upstream matcher permits a subset. Other
networks in a multi-rail challenge remain outside this selection's approval.
If a challenge has several `exact` offers on the reviewed network, add an
explicit zero-based `offerIndex` to the expectations object after reviewing
that entry. The selected entry must still match every pinned term; the tool
never chooses another entry to make a failing selection pass.

NEAR supports the launch classic NEP-366 delegate path; any explicit
`assetTransferMethod` selector is unsupported by this deployment. Both chain
families require authorization flow; an SDK supporting upfront flow does not
extend this deployment's capabilities. Base mainnet requires the real
`USD Coin` / `2` EIP-712 domain; Sepolia requires `USDC` / `2`. Both Base
contracts are checked against their pinned network. Base Sepolia support in
this offline tool is not a claim that a public instance is live.

A pass does not verify the HTTP capture's origin, liveness or method, ownership
of the recipient, token storage, delivery idempotency, a signature, or any
on-chain outcome. Those gates remain in the
[reference access policy](../../docs/reference-access.md) and
[catalog admission policy](../../docs/catalog.md). The checker creates no
client, credential, policy, sponsorship allowance or catalog entry.
