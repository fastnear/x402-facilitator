# Distribution: registry submissions

Status (2026-07-26): the x402 Foundation facilitators-table entry **merged**
(#2941); awesome-agentic-commerce #510 and x402facilitators #15 remain open;
and the x402scan NEAR feature request remains open as #1040. Base mainnet is
now live with authentic settlement activity, which makes x402-list actionable
and creates a future x402scan facilitator path. Do not submit updated
multi-chain claims until v0.5.1 is deployed and its public landing page and
Base `payment-identifier` advertisement are visible.

## Readiness facts registries key off

- The reference deployment has live NEAR mainnet, NEAR testnet, and Base
  mainnet facilitator instances. Base Sepolia remains a configured rollout
  target and must not be presented as live.
- v0.5.1 `/supported` is canonical x402 v2 with `kinds`,
  `extensions: ["payment-identifier"]`, and per-network `signers`; the gated
  legacy v1 kind is advertised only by an EVM instance that enables it.
- The public demo resource server returns a valid 402 with a base64
  `PAYMENT-REQUIRED` requirements header at
  `https://x402-demo.mikedotexe.com/work` (mainnet) and
  `https://x402-demo-test.mikedotexe.com/work` (testnet), and settles
  real payments end to end (see the real-traffic evidence entry).

## Reusable submission identity

- Name: **NEAR x402 Facilitator** (preserve the historical public identity).
- Slug: `near-x402-facilitator`.
- Description: “Open-source, API-key-gated x402 exact-payment facilitator for
  Circle USDC on NEAR and Base, with sponsored gas and durable settlement
  recovery.”
- Website: `https://x402.mikedotexe.com/` after the v0.5 landing page is
  deployed.
- Source/docs: <https://github.com/fastnear/x402-near-facilitator>.
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

## Targets

### 1. x402scan — resource registration (web form, no PR)

- Submit at <https://www.x402scan.com/resources/register>.
- **URL to submit now: `https://x402-demo-base.mikedotexe.com/work`** (Base
  mainnet — the network their validator supports). As of 2026-07-26 every
  requirement their validator checks is in place: https resource URL in the
  402 header, `info.x-guidance`, object-form `x-payment-info.protocols`,
  input/output schemas, 402-before-validation, and a third-party client has
  settled through the endpoint end to end (see
  [evidence](evidence/2026-07-26-legacy-v1-compat-and-base-e2e.md)).
- The NEAR resource below remains blocked on their upstream NEAR support.
- Original NEAR target: `https://x402-demo.mikedotexe.com/work`.
- Per their discovery spec (`Merit-Systems/x402scan`
  `docs/DISCOVERY.md`, OpenAPI-first), both demo hostnames serve
  `/openapi.json` declaring the paid `POST /work` operation with
  `x-payment-info` (protocol `x402`, fixed `$0.001`), a `402` response,
  and the request-body input schema, plus `/.well-known/x402` for
  compatibility fan-out (sources in `deploy/demo/discovery/`). Runtime
  402 behavior remains authoritative and was validated by the live paid
  flows.
- Optionally also register the testnet resource URL if the form accepts
  non-mainnet resources (their spec notes testnets are not indexed).
- **Status 2026-07-23: blocked upstream.** The probe now parses our
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
- Wait for six more authentic settlements before opening the PR. Never
  manufacture transfers merely to clear a directory or ranking threshold.
  NEAR support remains tracked in
  <https://github.com/Merit-Systems/x402scan/issues/1040>.

### 2. x402 Foundation repo — facilitators table (PR)

- Upstream: `x402-foundation/x402`, file `docs/dev-tools/facilitators.md`.
- Staged branch (based on clean upstream `main`):
  <https://github.com/mikedotexe/x402/tree/x402-near-facilitator-listing>
- **PR merged 2026-07-24:** <https://github.com/x402-foundation/x402/pull/2941>.
  The maintainer accepted the `facilitators.md` entry and asked to drop the
  deprecated `typescript/site/` ecosystem-page files (removed in `b783c830`);
  the merged PR is the single table entry below.
- Entry added (alphabetical position):
  `| [NEAR x402 Facilitator](https://x402.mikedotexe.com/supported) |
  Independent facilitator for NEAR mainnet and testnet; NEP-141 USDC
  settled through NEP-366 signed delegates with relayer-sponsored gas |`
- After v0.5.1 and the Base promotion are publicly evidenced, update this
  existing row in a small follow-up PR to say NEAR and Base and link the
  human-facing root page. Do not create a second Base-only listing.

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
- The open entry still describes NEAR only. After v0.5.1 is deployed, amend
  that PR to the reusable NEAR-and-Base description before it merges.

### 5. x402.watch / @swader/x402facilitators — facilitator directory (PR)

- Chain-neutral community directory (<https://facilitators.x402.watch>,
  npm `@swader/x402facilitators`) that other tools consume as a
  facilitator metadata source. Upstream: `Swader/x402facilitators`.
- Staged branch (adds `Network.NEAR`, the NEP-141 USDC token constant,
  explorer/icon wiring, and our entry; `tsc --noEmit` clean):
  <https://github.com/mikedotexe/x402facilitators/tree/x402-near-facilitator-listing>
- **PR opened 2026-07-23:** <https://github.com/Swader/x402facilitators/pull/15>
- The entry's logo references
  `docs/assets/near-x402-facilitator.svg` in this repository (must be on
  `main` before the PR is opened).
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

### 8. x402-list.com — facilitator registry (ready after v0.5 deploy)

- Submit through <https://x402-list.com/submit> or
  `POST https://x402-list.com/api/v1/submit`.
- Required fields are `type: "facilitator"`, submitter `email`,
  `facilitator_name`, an own-domain `website_url`, one to 25
  `settler_addresses`, and one to 25 `networks`. Optional fields are
  `description`, `facilitator_id_slug`, `token_claims`,
  `claimed_volume_usd`, and `notes`.
- The prepared body is
  [`registry/x402-list-submission.json`](registry/x402-list-submission.json).
  Revalidate it against the live OpenAPI immediately before submission.
- Submit the lowercase Base signer and declare both `base` and `near`. Put the
  NEAR named account in `notes`: the address validator accepts only EVM and
  Solana formats, while non-measured networks are resolved in manual review.
- The automatic probe scans 30 days of EVM USDC activity and is advisory;
  every entry is manually reviewed. One facilitator submission per email is
  allowed every seven days, so do not send a placeholder or speculative body.

### 9. Pay.sh awesome-x402 — hosted-facilitator list (PR)

- Upstream: `xpaysh/awesome-x402`, section “Hosted Facilitators.”
- Submit one bullet at the bottom of the section:
  `[NEAR x402 Facilitator](https://x402.mikedotexe.com/) - Open-source,
  API-key-gated facilitator for exact Circle USDC payments on NEAR and Base.`
- Wait until the own-domain landing page is live. Their contribution rules
  require an active, documented, production-ready HTTPS destination.

### 10. Gold-402 — hosted-facilitator list (PR)

- Upstream: `Haustorium12/gold-402`, `directory/facilitators.md`.
- Add one linked name and up to three factual sentences. Include the Base and
  NEAR paid-flow evidence because the authenticated endpoints cannot be probed
  without a key; arrange a narrowly scoped reviewer credential out of band if
  the maintainer requests a live verification.
- Submit after v0.5.1 is live so the homepage and `/supported` agree with the
  multi-chain description.

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

## Housekeeping

- Mike's GitHub fork of the foundation repo was temporarily renamed by
  tooling during fork setup and has been restored to `mikedotexe/x402`.
  A leftover empty duplicate may exist as `mikedotexe/x402-foundation`;
  delete it if present (it is not referenced by anything).
