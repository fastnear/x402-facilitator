# Registry and crates.io wrap-up — 2026-07-26

Owner: Mike Purvis

Completed 2026-07-27 UTC (2026-07-26 America/Los_Angeles).

This record closes the public-package work promised by v0.5.1 and records the
same-day registry submissions. No payment authorization was created and no
funded transaction was broadcast.

## Provider crates

Both reusable provider crates were published from the verified signed
`v0.5.1` tag at source revision
[`48dcfa62df40373eb25be5a33b140f9724cac122`](https://github.com/fastnear/x402-near-facilitator/commit/48dcfa62df40373eb25be5a33b140f9724cac122):

| Crate | Version | crates.io checksum |
| --- | --- | --- |
| [`x402-chain-near`](https://crates.io/crates/x402-chain-near/0.5.1) | 0.5.1 | `476a858d105e71a14cbbfbad31c6d790e0bae0eda3997c55dd9974c4a22abc89` |
| [`x402-chain-eip155-provider`](https://crates.io/crates/x402-chain-eip155-provider/0.5.1) | 0.5.1 | `b1bcd2f5907bac0425c5770a4473f2915334c26eb82591c155ba53647149ea9f` |

Before publication, each exact package passed `cargo publish --locked
--dry-run`, including registry-style package verification and compilation.
After publication, `cargo info` from outside the workspace downloaded and
resolved both 0.5.1 packages from crates.io. Each package contains its
crate-local README plus the workspace LICENSE and NOTICE. The facilitator
service crate remains private to the application release and was not
published.

## Registry actions

### x402-list

The facilitator payload was revalidated against the live OpenAPI schema and
submitted once to `POST https://x402-list.com/api/v1/submit`.

- Submission ID: `925e62da-75e7-49f5-adca-57762b835966`.
- Status: `pending` manual review.
- Automatic Base probe: the lowercase settler was found with four
  transactions and no probe errors.
- NEAR remains a declared claim for manual review because the automatic
  address probe measures supported EVM/Solana formats.

This is not yet a published x402-list entry and must not be described as one
until review completes.

### x402scan

The Base demo resource was already registered, so it was not submitted again:

- <https://www.x402scan.com/server/7c1727f6-7b5d-4018-abe9-22276406a685>
- `POST https://x402-demo-base.mikedotexe.com/work`
- x402 v2, Base mainnet, fixed `$0.001` payment

Its optional ownership-proof marker remains unverified. Adding that proof
would require a separate, explicitly approved non-transaction signature from
the production payee; it was intentionally left out of this wrap-up.

The Base facilitator listing still requires ten genuine outgoing USDC
settlements. The dedicated signer has four, leaving six organic settlements.
No traffic was manufactured for this threshold. NEAR resource support remains
tracked in
[x402scan issue #1040](https://github.com/Merit-Systems/x402scan/issues/1040).

### Directory pull requests

| Target | Action | Status |
| --- | --- | --- |
| x402 Foundation | [#2960](https://github.com/x402-foundation/x402/pull/2960) updates the existing row for NEAR and Base | Open |
| awesome-agentic-commerce | [#510](https://github.com/Merit-Systems/awesome-agentic-commerce/pull/510) amended in place for NEAR and Base | Open |
| Pay.sh awesome-x402 | [#1020](https://github.com/xpaysh/awesome-x402/pull/1020) adds one hosted-facilitator entry | Open |
| Gold-402 | [#64](https://github.com/Haustorium12/gold-402/pull/64) adds one evidence-backed hosted-facilitator entry | Open |
| x402.watch package | [#15](https://github.com/Swader/x402facilitators/pull/15) fixed to construct `X-API-Key` headers and drop an accidental npm lockfile | Open |

The x402.watch branch passed Bun type-checking, linting, the full build, and a
direct assertion over the generated verify, settle, and supported auth
headers.

## Readiness observation

One transient Base `/readyz` response reported the RPC and relayer checks
unready during the audit. The service remained active without restarting, all
three public instances subsequently returned ready, and six consecutive Base
samples at five-second intervals returned HTTP 200 with `ready: true`. No
operator mutation was needed.
