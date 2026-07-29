# Distribution: registry submissions

Status (2026-07-28): the original x402 Foundation entry **merged** as #2941,
and its NEAR-and-Base follow-up **merged** as #2960.
awesome-agentic-commerce #510, x402facilitators #15, Pay.sh #1020, and
Gold-402 #64 remain open. x402-list declined the facilitator pending
independently attributable settlement activity; the implementation and
identity checks passed. The Base demo resource is registered on x402scan.
NEAR discovery remains tracked in x402scan #1040.
v0.5.3 is deployed across the three live reference instances; its public
landing pages and Base `payment-identifier` advertisement are visible in the
[dated rollout evidence](evidence/2026-07-27-v053-rpc-readiness-rollout.md).
The registry decision and evidence gates are recorded in the
[2026-07-28 review](evidence/2026-07-28-x402-list-review.md).

## Readiness facts registries key off

- The reference deployment has live NEAR mainnet, NEAR testnet, and Base
  mainnet facilitator instances. Base Sepolia remains a configured rollout
  target and must not be presented as live.
- v0.5.3 `/supported` is canonical x402 v2 with `kinds`,
  `extensions: ["payment-identifier"]`, and per-network `signers`; the gated
  legacy v1 kind is advertised only by an EVM instance that enables it.
- The public demo resource server returns a valid 402 with a base64
  `PAYMENT-REQUIRED` requirements header at
  `https://x402-demo.mikedotexe.com/work` (mainnet) and
  `https://x402-demo-test.mikedotexe.com/work` (testnet). Historical
  operator-controlled paid-flow and canary records demonstrate end-to-end
  behavior (see the [real-traffic evidence](evidence/2026-07-23-real-traffic-and-recovery.md)),
  but do not demonstrate independently attributable merchant adoption; the
  [2026-07-28 review](evidence/2026-07-28-x402-list-review.md) controls for
  facilitator-listing purposes.

## Reusable submission identity

- Name: **NEAR x402 Facilitator** (preserve the historical public identity).
- Slug: `near-x402-facilitator`.
- Description: “Open-source, API-key-gated x402 exact-payment facilitator for
  Circle USDC on NEAR and Base, with sponsored gas and durable settlement
  recovery.”
- Website: <https://x402.mikedotexe.com/>.
- Source/docs: <https://github.com/fastnear/x402-facilitator> (repository
  renamed from `x402-near-facilitator` on 2026-07-27; GitHub redirects cover
  every previously submitted link, so already-filed registry entries need no
  amendment).
- Logo: `docs/assets/near-x402-facilitator.svg`; color `#00ec97`.
- Access: gated; `X-API-Key`; use the
  [reference-access process](reference-access.md).
- Base signer: `0x7ff46ab88688d528bce3e59c470240c6901cf88c`;
  canonical Base USDC:
  `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`; first settlement date
  `2026-07-26`.

There is no shared facilitator-registry manifest standard. Keep `/supported`
as the machine contract and use the target-specific material below; do not
publish a made-up `/.well-known` facilitator format.

## Published provider crates

The reusable NEAR and EVM providers are published at v0.5.1:

- <https://crates.io/crates/x402-chain-near/0.5.1>
- <https://crates.io/crates/x402-chain-eip155-provider/0.5.1>

They were published from the signed v0.5.1 tag. Checksums and registry
verification are recorded in the
[dated distribution evidence](evidence/2026-07-26-registry-and-crates-wrap-up.md).
The facilitator service crate remains application-only.

## Targets

### 1. x402scan — resource registration (web form, no PR)

- **Base registration complete:** the `POST`
  `https://x402-demo-base.mikedotexe.com/work` resource is live at
  <https://www.x402scan.com/server/7c1727f6-7b5d-4018-abe9-22276406a685>.
  Do not submit it again.
- The row is active and records x402 v2, Base mainnet, and the fixed `$0.001`
  payment. Its optional acceptance ownership proof is not verified. Adding
  that marker requires a separately approved EIP-191 signature by the
  production payee and is not a registration blocker.
- The NEAR resource remains blocked on upstream NEAR support.
- Original NEAR target: `https://x402-demo.mikedotexe.com/work`.
- Per their discovery spec (`Merit-Systems/x402scan`
  `docs/DISCOVERY.md`, OpenAPI-first), both demo hostnames serve
  `/openapi.json` declaring the paid `POST /work` operation with
  `x-payment-info` (protocol `x402`, fixed `$0.001`), a `402` response,
  and the request-body input schema, plus `/.well-known/x402` for
  compatibility fan-out (sources in `deploy/demo/discovery/`). Runtime 402
  behavior remains authoritative and was validated by the
  [live Base flow](evidence/2026-07-26-legacy-v1-compat-and-base-e2e.md).
- **NEAR status 2026-07-23: blocked upstream.** The probe parses our
  discovery document (title, paid operation, input schema all accepted)
  but rejects registration with `No supported networks. Got:
  [near:mainnet]. Supported: [base, solana]`. Their
  `apps/scan/src/types/chain.ts` `Chain` enum and payment tooling are
  EVM/Solana-only, so NEAR indexing is an upstream feature, not a config
  change. Both warnings from the probe (root `favicon.ico`,
  `info.contact.email`) are fixed on our side so registration is
  turnkey once they add NEAR.
- **Feature request filed 2026-07-23**:
  <https://github.com/Merit-Systems/x402scan/issues/1040> (Support NEAR
  `near:mainnet` resources). Registration is turnkey once it lands.

### 1b. x402scan — facilitator registry (PR, not ready)

- This is separate from resource registration. Current source lives under
  `packages/external/facilitators/`; the repository README and PR template
  still mention an older `facilitators/config.ts` layout.
- A Base entry needs a unique ID, name, logo, docs URL, color, a usable
  API-key-aware config constructor, lowercase signer, canonical USDC token,
  exact first-transaction date, exports, and list registration. Its chain enum
  supports Base, Polygon, and Solana—not NEAR.
- Their PR gate requires at least 10 genuine USDC transfers from the settlement
  address. As of 2026-07-26 the dedicated Base signer has four successful
  `transferWithAuthorization` transactions (nonces 0–3), first on
  2026-07-26. The dashboard additionally suppresses entries below 100
  transactions.
- This transaction-count snapshot is dated and requires revalidation before
  action. Do not open a listing PR merely because a count is reached; require
  independently attributable Base merchant/payee evidence and organic payer
  activity. Never manufacture transfers merely to clear a directory or ranking
  threshold. NEAR support remains tracked in
  <https://github.com/Merit-Systems/x402scan/issues/1040>.

### 2. x402 Foundation repo — facilitators table (PR)

- Upstream: `x402-foundation/x402`, file `docs/dev-tools/facilitators.md`.
- Follow-up branch (based on current upstream `main`):
  <https://github.com/mikedotexe/x402/tree/agent/update-near-base-facilitator-listing>
- **PR merged 2026-07-24:** <https://github.com/x402-foundation/x402/pull/2941>.
  The maintainer accepted the `facilitators.md` entry and asked to drop the
  deprecated `typescript/site/` ecosystem-page files (removed in `b783c830`);
  the merged PR is one table row.
- **Multi-chain follow-up merged 2026-07-27:**
  <https://github.com/x402-foundation/x402/pull/2960>. It changes only that
  existing row to link the human-facing page and describe deployed NEAR and
  Base support. No second Base-only listing or deprecated ecosystem files
  were added.

### 3. x402.org ecosystem page — partner entry (DEPRECATED upstream)

- **Withdrawn 2026-07-24.** The maintainer (phdargen) confirmed on #2941
  that the x402.org ecosystem page (`typescript/site`) is deprecated and
  asked for those files to be removed; the `metadata.json` partner entry
  and logo were dropped from the branch (`b783c830`). No ecosystem-page
  submission path exists at present.

### 4. awesome-agentic-commerce (formerly awesome-x402) — list entry (PR)

- Upstream: `Merit-Systems/awesome-agentic-commerce`, README
  "Facilitators & Networks" section.
- Staged branch:
  <https://github.com/mikedotexe/awesome-agentic-commerce/tree/x402-near-facilitator-listing>
- **PR opened 2026-07-23:** <https://github.com/Merit-Systems/awesome-agentic-commerce/pull/510>
- **Amended 2026-07-26:** the one-line entry, title, and body now describe the
  deployed NEAR-and-Base facilitator and link the v0.5.1 rollout evidence. A
  single concise review ping was left; the PR is awaiting maintainer review.

### 5. x402.watch / @swader/x402facilitators — facilitator directory (PR)

- Chain-neutral community directory (<https://facilitators.x402.watch>,
  npm `@swader/x402facilitators`) that other tools consume as a
  facilitator metadata source. Upstream: `Swader/x402facilitators`.
- Staged branch (adds `Network.NEAR`, the NEP-141 USDC token constant,
  explorer/icon wiring, and our entry):
  <https://github.com/mikedotexe/x402facilitators/tree/x402-near-facilitator-listing>
- **PR opened 2026-07-23:** <https://github.com/Swader/x402facilitators/pull/15>
- **Hardened 2026-07-26:** the gated config is now a typed constructor that
  supplies `X-API-Key` for verify, settle, and supported calls, with a public
  discovery config. An accidental npm lockfile was removed. Bun type-check,
  lint, full build, and a direct auth-header assertion pass.
- Leave this NEAR-enum PR narrowly scoped unless its maintainer responds. The
  directory's single-config URL does not cleanly represent the separate NEAR
  and Base endpoints, and it is lower priority than the active targets above.

### 6. NEAR Catalog — ecosystem directory (web form)

- Submit at <https://submit.nearcatalog.xyz/> (requires NEAR-account
  login; editorial review). The NEAR-native directory — strongest
  audience fit. Suggested category: infrastructure/payments; link the
  repo, both facilitator endpoints, and the demo workload.

### 7. near/awesome-near — official curated list (PR)

- Staged branch (entry in "AI and Cloud Services"):
  <https://github.com/mikedotexe/awesome-near/tree/x402-near-facilitator-listing>
- Open the PR from
  <https://github.com/near/awesome-near/compare/main...mikedotexe:awesome-near:x402-near-facilitator-listing>

### 8. x402-list.com — facilitator registry (adoption gate)

- Submit through <https://x402-list.com/submit> or
  `POST https://x402-list.com/api/v1/submit`.
- Required fields are `type: "facilitator"`, submitter `email`,
  `facilitator_name`, an own-domain `website_url`, one to 25
  `settler_addresses`, and one to 25 `networks`. Optional fields are
  `description`, `facilitator_id_slug`, `token_claims`,
  `claimed_volume_usd`, and `notes`.
- The historical submission record is
  [`registry/x402-list-submission.json`](registry/x402-list-submission.json).
  It was revalidated against the live OpenAPI immediately before the 2026-07-26
  submission; never reuse it for a future submission.
- **Submitted once on 2026-07-26:** ID
  `925e62da-75e7-49f5-adca-57762b835966`. The automatic Base probe found the
  submitted settler with four transactions and no errors.
- The body declares both `base` and `near`; the NEAR named account is in
  `notes` because the automatic address probe accepts EVM and Solana formats.
  Every entry is manually reviewed.
- **Declined on 2026-07-28:** the registry verified the implementation,
  evidence, and operator identity, but found no independently attributable
  settlement activity. Its Base view showed four `$0.001` transfers with
  canary-shaped payer histories; NEAR is outside its measurement registry.
  See the [dated review](evidence/2026-07-28-x402-list-review.md).

#### Service submission

The Base merchant evidence API is a separate service-discovery candidate:

- service: **Base Agent Evidence & Route API**;
- URL and website: <https://merchant-base.mikedotexe.com/>;
- category: **Blockchain**;
- discovery: `/openapi.json`, `/llms.txt`, `/.well-known/x402`, `/pricing`,
  `/terms`, and `/robots.txt`;
- first directory probe path:
  `/v1/entities/0x0000000000000000000000000000000000000000` (the public,
  paid `GET` entity operation, which returns a canonical 402 without a
  payment); the OpenAPI and Bazaar metadata advertise the additional paid
  `POST /v1/evidence/*`, `POST /v1/activity/search`, and
  `POST /v1/routes/usdc/quote` operations.
- payment: canonical Base mainnet USDC, fixed `1000` atomic units, to
  `0x7Ff46ab88688D528bCE3e59c470240c6901cF88c`, with the required
  `USD Coin` / `2` EIP-712 domain.
- description: paid, finality-aware Base account/transaction evidence,
  bounded activity lookup, and dry Base-USDC-to-NEAR-USDC route quotes.

Submit it only after the merchant changes are merged, promoted from that
merged commit, and the unpaid production regression passes. Use the service
submission form or API fields `url`, `email`, `service_name`, `description`,
`website_url`, `category`, `endpoints`, and `notes`. Confirm the intended
email immediately before sending and do not commit it in a reusable template.
The registry accepts endpoint **paths** (not methods) and may issue a `GET`.
Until it confirms method-aware probing, submit only this concrete unpaid `GET`
operation; put the POST operations and body shapes in `notes` and OpenAPI:

```text
/v1/entities/0x0000000000000000000000000000000000000000
```

Every listed path must return a canonical unpaid 402 when the registry probes
it; do not include discovery, liveness, or readiness routes. The form's
first-time service submission is free. A service resubmission rejected within
14 days may instead challenge for payment; stop rather than pay, wait for the
free window, and record the response in a new dated evidence document. The
service and facilitator cooldowns are distinct, but each is still one
submission per email in seven days.

The zero-payment preflight is the deployed release's `npm run regression`: it
checks both `/readyz` dependencies, the page/openapi/llms/robots/terms
surfaces, the fixed six-decimal price, Base `USD Coin`/`2`, and the concrete
entity-route 402. Record the merged SHA, immutable release pointer, archive
checksum, public `/healthz.release.id`, matching OpenAPI
`info.x-x402-merchant-release-id`, and preflight output before asserting that
the service is current.
Do not create `/.well-known/x402list.txt` speculatively: if the registry later
issues an ownership token, serve its exact one-line value from a root-owned
file through nginx and record the registry request that authorized it.

This operator-owned service can attract organic buyers, but its own payments
do not establish an independent merchant for facilitator review.

#### Independent Base pilot

Re-engage the known external integrator only through the operator's existing
private channel. Before provisioning anything, require the public HTTPS paid
method/path that returns an unpaid 402, public OpenAPI or discovery naming that
operation and its `payTo`, an independently controlled Base recipient with
public operator-control evidence, expected usage, and an operational contact.
Use the [Base merchant-pilot issue template](../.github/ISSUE_TEMPLATE/base_merchant_pilot.yml)
for the public non-secret record. Create one dedicated Base mainnet client with
exact `eip155:8453`, canonical USDC, and recipient policy, default limits of 60
verify requests and 10 settle requests per minute, and the existing
conservative gas cap. Do not raise sponsorship limits without a separate
review.

Start that client with `--daily-yocto-near 0` for verify-first onboarding.
`/verify` remains read-only; a valid, allowlisted `/settle` then returns
`429 sponsorship_budget_exhausted` before the broadcast phase. Raise the cap
only with `client set-budget --daily-yocto-near <atomic-gas-cap>` after the
out-of-band review and read-only verification have succeeded. The legacy
`*_yocto_near` name denotes wei for this Base client.

Deliver the raw key once out of band. Confirm `/supported`, `/readyz`, public
discovery, and read-only `/verify` before settlement is enabled. The public
request requires no funded payer, signature, or transaction. The merchant, not
the facilitator operator, must initiate and fund any real payments. Never fund
payer wallets, self-pay, split payments, or raise prices to create registry
volume.

#### Facilitator resubmission

Do not resubmit before August 3, 2026, the conservative end of the seven-day
per-email cooldown. Even then, require an independent public Base
resource-server and recipient, multiple organic settlements from established
payer histories, and public evidence that ties the merchant and transactions
together. Prefer activity above the reviewer's approximately `$2.80`
comparison point, but do not treat it as a guaranteed threshold or alter
prices to reach it. Omit unsupported claimed volume and describe NEAR
settlements as supplementary because this registry does not currently measure
them.

### 9. Pay.sh awesome-x402 — hosted-facilitator list (PR)

- Upstream: `xpaysh/awesome-x402`, section “Hosted Facilitators.”
- Submit one bullet at the bottom of the section:
  `[NEAR x402 Facilitator](https://x402.mikedotexe.com/) - Open-source,
  API-key-gated facilitator for exact Circle USDC payments on NEAR and Base.`
- The own-domain landing page is live. Their contribution rules require an
  active, documented, production-ready HTTPS destination.
- **PR opened 2026-07-26:**
  <https://github.com/xpaysh/awesome-x402/pull/1020>.

### 10. Gold-402 — hosted-facilitator list (PR)

- Upstream: `Haustorium12/gold-402`, `directory/facilitators.md`.
- Add one linked name and up to three factual sentences. Include the Base and
  NEAR paid-flow evidence because the authenticated endpoints cannot be probed
  without a key; arrange a narrowly scoped reviewer credential out of band if
  the maintainer requests a live verification.
- v0.5.3 is live, and the homepage and `/supported` agree with the multi-chain
  description.
- **PR opened 2026-07-26:**
  <https://github.com/Haustorium12/gold-402/pull/64>. No reviewer credential
  was created or disclosed.

### 11. x402dev monitor — skip until its gated-facilitator model is fixed

- Its JSON list can record name, URL, API-key requirement, and comments, but
  the monitor skips API-key facilitators other than its special Coinbase path.
  A listing would appear unusable or unmonitored. Propose the upstream
  authentication model first rather than misrepresenting availability.

### 12. x402dir.com — editorial contact only

- No public submission schema, form, or source repository was found. Treat it
  as optional outreach after the structured targets above; confirm fields with
  the maintainer before sending operator or address details.

### 13. Bazaar — reference only

- Coinbase's Bazaar discovery layer
  (<https://docs.cdp.coinbase.com/x402/bazaar>) indexes resources behind
  the CDP facilitator; as a self-hosted facilitator we are out of scope.
  x402scan (target 1) is the discovery surface that applies.

## Client integration note (learned from the live paid flows)

Clients talking to this facilitator through the official middleware must
send the `payment-identifier` extension in its full canonical envelope —
`{"payment-identifier": {"info": {"required": true, "id": "…"}, "schema":
{…}}}` — echoing the `schema` object from the 402 requirements. An
`info`-only entry is rejected as non-canonical (the facilitator validates
extension entries against `additionalProperties: false`). Replays must
resend the byte-identical signed payload: the reference workload binds
each payment identifier to the exact payload fingerprint, so a re-signed
payment with a reused identifier is a `409` conflict by design.
