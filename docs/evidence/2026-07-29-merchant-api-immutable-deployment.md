# Merchant API immutable deployment

Date: 2026-07-29

Status: deployed from the merged `main` commit; the deployment and all
validation below were unpaid.

## Provenance and rollback gate

- merged source: `0a0b832fed6526d8fa5d51a9de677d66df08ad6f`
  (`feat(merchant): harden listing readiness (#87));
- archive: `x402-merchant-git-0a0b832fed6526d8fa5d51a9de677d66df08ad6f.tar.gz`;
- archive SHA-256:
  `c309d24144fa8e17d5ab2cd7858194701556b007d731961b1a78dc61c91ebff9`; and
- installed immutable release ID:
  `git-0a0b832fed6526d8fa5d51a9de677d66df08ad6f`.

The archive was built in a clean detached worktree where `HEAD` and fetched
`origin/main` both resolved to the merged source above. Its sidecar checksum
was verified locally and again on the host. The host installer copied the
archive into a root-only staging directory, verified the checksum and archive
shape, installed locked production dependencies, and ran all 88 merchant
checks before publishing the immutable directory. Installation did not change
either live pointer or restart a process.

Before promotion, both `current-near` and `current-base` pointed to the
root-owned legacy release `20260727-regression-audit-v4`. The updated promotion
helper successfully selected that same target for each network without a
restart, proving that its narrowly allowlisted legacy npm links are safe for a
rollback. The legacy release's own targeted unpaid regressions also passed.

## Host validation and promotion

The checked-in merchant unit, nginx configuration, install/promotion/rollback
helpers, and canary assets were checksum-matched before installation. The host
then passed `systemd-analyze verify`, `nginx -t`, and an nginx reload. Both
merchant certificate names passed SNI hostname validation.

NEAR was promoted first, then Base only after NEAR met all post-promotion
checks. Each process had a short startup window in which nginx returned a 502
while the new Node process initialized; the bounded readiness gate waited for
three consecutive successful `/readyz` responses before proceeding. No payment
was constructed, signed, verified, or settled during that wait.

Final pointers and service state:

| Instance | Pointer | Service result |
| --- | --- | --- |
| NEAR | `/opt/x402-merchant/releases/git-0a0b832fed6526d8fa5d51a9de677d66df08ad6f` | active, `NRestarts=0`, `ExecMainStatus=0` |
| Base | `/opt/x402-merchant/releases/git-0a0b832fed6526d8fa5d51a9de677d66df08ad6f` | active, `NRestarts=0`, `ExecMainStatus=0` |

Both public merchant `/readyz` endpoints reported `rpc`, `facilitator`, and
`payment` as `ready`. Both underlying facilitator `/readyz` endpoints also
reported every dependency ready.

The targeted NEAR and Base regressions, followed by the full dual-origin
regression, passed from the installed release. They exercised public discovery,
listing surfaces, CORS, and all ten unpaid challenges; the regression runner
reported that no payment signature was created or sent.

## Public listing surface

Both origins now serve HTTP 200 for `/openapi.json`, `/llms.txt`,
`/.well-known/x402`, `/pricing`, `/terms`, `/robots.txt`, and `/readyz`.
OpenAPI reports version `0.3.0`.

The concrete, side-effect-free Base probe
`GET /v1/entities/0x0000000000000000000000000000000000000000` returned HTTP
402 with one canonical x402 v2 `exact` requirement:

- network `eip155:8453`;
- Circle USDC `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`;
- recipient `0x7Ff46ab88688D528bCE3e59c470240c6901cF88c`;
- amount `1000` atomic USDC units; and
- EIP-712 domain `USD Coin` / `2`.

This confirms the deployed service policy only. It is an operator-owned
reference service whose recipient is facilitator-controlled, so neither this
deployment nor its future traffic is evidence of an independently operated
merchant for facilitator relisting.

## Monitoring

The installed `x402-canary` service completed successfully after promotion.
It reported `MerchantApiOk network=mainnet value=1` and
`MerchantApiOk network=base value=1`, in addition to the existing
facilitator and demo checks. The active timer will repeat the no-payment check
every five minutes. The merchant checks require public readiness and decode the
unpaid 402 policy; they do not send a payment header, sign an authorization,
invoke paid application work, or move funds.

The host instance role can publish metrics but intentionally lacks alarm
administration permission. The operator's existing AWS identity created the
two scoped `MerchantApiOk` alarms after the successful canary run:

- `x402-merchant-mainnet-api-canary-failing`; and
- `x402-merchant-base-api-canary-failing`.

Each uses the documented `2 of 3 × 5 minute` threshold and treats missing data
as breaching. Immediately after creation their state was `INSUFFICIENT_DATA`,
which is expected until the next scheduled metric periods arrive. The next two
scheduled canaries again emitted value `1` for both merchant networks, and both
alarms subsequently reached `OK`.

## Boundaries and next step

No paid proof, `/settle` call, payment signature, wallet funding, or on-chain
broadcast was performed for this rollout. This record supports a future
x402-list **service** submission only after it is merged. It does not change
the independently attributable Base-usage requirements recorded in the
[2026-07-28 facilitator review](2026-07-28-x402-list-review.md), nor does it
authorize facilitator resubmission before 2026-08-03.
