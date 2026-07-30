# v0.5.6 readiness startup-refresh rollout — 2026-07-29

Owner: Mike Purvis

Completed 2026-07-30 UTC (2026-07-29 America/Los_Angeles).

This sanitized record covers the production rollout of the merchant readiness
startup-refresh fix from [PR #97](https://github.com/fastnear/x402-facilitator/pull/97),
released through [PR #98](https://github.com/fastnear/x402-facilitator/pull/98).
It records operational readiness, not settlement volume or independently
attributable adoption. No payment authorization, API credential, `/settle`
request, funded action, or transaction broadcast was created for this rollout.

## Release identity and verification

- Released source revision:
  [`975ffd81b192738340775dcffdfb7148b85fe1a2`](https://github.com/fastnear/x402-facilitator/commit/975ffd81b192738340775dcffdfb7148b85fe1a2).
- Signed annotated tag: `v0.5.6`, tag object
  `d3c5109c6d1eb5bef856f148b8cade06dfa711d4`.
- Published immutable release:
  <https://github.com/fastnear/x402-facilitator/releases/tag/v0.5.6>.
- Successful release workflow:
  <https://github.com/fastnear/x402-facilitator/actions/runs/30517753945>.
- Deployed native archive SHA-256:
  `44ca59af723702cd13845cd4745775eb1ddd8f2966e1bef529be7e80379706b7`.
- Immutable OCI reference:
  `ghcr.io/fastnear/x402-facilitator@sha256:7e945d2e8231ee38e05d75c8207ef8d767594357e35ab898c975d9a271f3f6c7`.

The signed local tag, GitHub tag verification, GitHub merge-commit
verification, release target, release manifest, and exact nine-asset set
agreed. All four release-side checksum files passed. The native archive and
OCI image each passed provenance and CycloneDX SBOM attestation verification
for the tagged source and pinned release workflow. Both SBOMs contain the
expected `x402-types` 2.0.2 and `near-primitives` 0.37.2 components.

Before host admission, the archive member paths and types were rejected unless
safe, the archive's embedded SBOM matched the published native SBOM, and the
embedded deploy, documentation, and example-application checksum manifests
passed. The checked-in release workflow also independently passed its signed
source checkpoint, locked quality gate, parser fuzz gate, native build,
immutable OCI build, and final publication verification.

## Installation and staged promotion

Immediately before promotion, each database had zero settlements in
`awaiting_retry`, `reserved`, `prepared`, or `submitted`:
`x402_near_testnet`, `x402_base`, and `x402_near_mainnet`. The same query was
zero again after the final promotion.

The verified native archive was installed once into the immutable
`/opt/x402-near-facilitator/releases/v0.5.6` directory. Both packaged Linux
binaries reported version `0.5.6`, and the packaged checksum manifests passed
again on the host. Installation did not change a pointer or restart a service.
No admin migration command ran; this release does not change the database
schema or migrations.

`nginx -t` and concrete instantiated `systemd-analyze verify` checks passed.
The rollout order was NEAR testnet, Base mainnet, then NEAR mainnet. Each
instance was promoted, restarted, and validated before the next pointer moved.
All three enabled public facilitator pointers now resolve to v0.5.6 and all
three services are active. v0.5.5 remains installed as the direct rollback
target. The disabled Base Sepolia facilitator and the separately versioned
merchant deployments were deliberately left unchanged.

## Authorization-free regression checks

For each promotion, the tagged deployment helper was run without an API-key
file. It checked `/healthz`, `/readyz`, discovery, landing-page links, and an
unauthenticated `/verify` request that must return 401. It does not send a
payment header or call `/settle`. Three consecutive public readiness samples
passed before and after the Base promotion, and three consecutive samples
passed after the testnet and mainnet promotions. The final all-origin sweep
also passed:

- <https://test.x402.mikedotexe.com/> — `near:testnet`;
- <https://base.x402.mikedotexe.com/> — `eip155:8453`; and
- <https://x402.mikedotexe.com/> — `near:mainnet`.

The two merchant APIs were tested using only their public readiness endpoints
and unauthenticated evidence requests. Each returned its configured 1,000
atomic-USDC 402 challenge, including Base mainnet's `USD Coin`/`2` domain;
no payment authorization was sent. The three reference demos likewise
returned their expected unpaid 402 challenges. These checks did not settle,
fund, or broadcast a payment.

The standard scheduled canary was not manually invoked because its Base
`/verify` fixture is a stored, unfunded signed authorization. Its naturally
scheduled latest run was successful. Both the canary and metrics timers remain
active.

## Scope, reliability posture, and rollback

The v0.5.6 code consumes Tokio's immediate interval tick after the synchronous
startup readiness snapshot, so the next background refresh occurs after the
configured 15-second interval instead of duplicating the startup probe. It
does not change `/verify`, `/settle`, or `/supported` wire formats,
settlement-engine behavior, payment policy, configuration, database schema,
or migrations.

Base retained the fail-closed independent dual-RPC readiness policy during the
entire rollout. The previously observed, recovered
`backup_rpc_unavailable` condition is not cured by this scheduling fix and no
reader was removed or relaxed. Base was ready in every pre-promotion,
post-promotion, and final rollout sample, but selection and installation of a
vetted independent backup provider remains an operator decision before a
claim of sustained reliability is appropriate.

This rollout does not create third-party settlement evidence and does not
change the independent Base-adoption gate or the 2026-08-03 earliest date for
a future facilitator resubmission recorded in the
[2026-07-28 x402-list review](2026-07-28-x402-list-review.md).
