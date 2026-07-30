# v0.5.5 Base readiness-classification rollout — 2026-07-29

Owner: Mike Purvis

Completed 2026-07-30 UTC (2026-07-29 America/Los_Angeles).

This sanitized record covers the production rollout of the secret-free Base
dual-RPC readiness classification from [PR #94](https://github.com/fastnear/x402-facilitator/pull/94).
It records an operational readiness observation, not settlement volume or
third-party adoption evidence. No payment authorization, funding operation,
or transaction broadcast occurred during this rollout.

## Release identity and verification

- Merged source revision:
  [`a9beecbbfec923ea3b9ebd1203ccdf3e78fb764b`](https://github.com/fastnear/x402-facilitator/commit/a9beecbbfec923ea3b9ebd1203ccdf3e78fb764b).
- Signed annotated tag: `v0.5.5`, tag object
  `574c94d93e08daa19bd4778aa7bb3ad6a3ee9712`.
- Published release:
  <https://github.com/fastnear/x402-facilitator/releases/tag/v0.5.5>.
- Successful release workflow:
  <https://github.com/fastnear/x402-facilitator/actions/runs/30513030108>.
- Deployed native archive SHA-256:
  `68e82c5755c955792af15165733895bef66c99997ca55d95e7ee06e8a2d53df1`.
- Immutable OCI reference:
  `ghcr.io/fastnear/x402-facilitator@sha256:7a1cf5300ebaac28f1170de3edd323a3af2a6c44c61fe49a7218ecb352c98110`.

Before installation, the tag signature, tag object, source revision, release
manifest, and exact nine-asset release set agreed. The archive and native
SBOM checksums passed, as did the native archive provenance and CycloneDX SBOM
attestations for the tagged source and release workflow. The packaged deploy,
documentation, and example-application checksum manifests also passed.

## Admission, installation, and promotion

Before each pointer change, the relevant settlement journal had zero rows in
`awaiting_retry`, `reserved`, `prepared`, or `submitted`. The same query was
zero after the final promotion for `x402_near_testnet`, `x402_base`, and
`x402_near_mainnet`.

The verified archive was installed once into the immutable
`/opt/x402-near-facilitator/releases/v0.5.5` directory. Both packaged Linux
binaries reported version `0.5.5`. No admin migration command was run: this
release changes neither database schema nor migrations.

`nginx -t` and concrete instantiated `systemd-analyze verify` checks passed
before promotion. The rollout order was NEAR testnet, Base mainnet, then NEAR
mainnet. Each instance was promoted, restarted, and validated before the next
pointer moved. All three enabled public facilitator pointers now resolve to
v0.5.5. The disabled Base Sepolia instance was deliberately left untouched on
its existing release. Merchant release pointers and configuration were also
unchanged.

## Unpaid regression and readiness observation

All three public facilitator origins passed the deployment helper after their
respective promotion, including `/healthz`, `/readyz`, discovery, landing-page
links, and the unauthenticated `/verify` boundary:

- <https://test.x402.mikedotexe.com/> — `near:testnet`;
- <https://base.x402.mikedotexe.com/> — `eip155:8453`; and
- <https://x402.mikedotexe.com/> — `near:mainnet`.

The full unpaid canary checks facilitator verification, demo 402 challenges,
and merchant readiness plus unpaid evidence challenges. Its first
post-promotion run found a Base merchant readiness response of HTTP 503 while
all facilitator verification and demo checks were successful. This was treated
as a fail-closed observation, not ignored or retried with a payment.

Read-only, secret-free operational diagnostics recorded the fixed Base class
`backup_rpc_unavailable` during that recovery window. The merchant's own
readiness requires its independent RPC check, compatible facilitator
`/supported` discovery, facilitator `/readyz`, and payment initialization, so
the observation does not prove which of those bounded dependencies caused its
one failed probe. It does confirm that no provider URL, signer, account,
credential, authorization, transaction hash, or raw RPC response was exposed.

Immediately afterward, repeated public merchant and facilitator readiness
probes were healthy. A second full unpaid canary passed every facilitator,
demo, NEAR merchant, and Base merchant check; no classified Base readiness
failure was recorded during that run. All three facilitator services remain
active, both canary and metrics timers are active, and no journal row became
active during the regression checks.

## Scope, rollback, and registry posture

The v0.5.5 change preserves the public readiness boolean and fail-closed
dual-reader policy. It adds only bounded internal failure classification for
protected telemetry and structured logs; it does not change `/verify`,
`/settle`, or `/supported` wire formats, settlement-engine behavior, database
schema, migrations, configured price, or payment policy. The prior immutable
release remains installed as the direct rollback target.

This record makes no claim of independently attributable settlement volume.
It neither manufactures activity nor changes the Base adoption requirements or
the 2026-08-03 earliest facilitator-resubmission date documented in the
[2026-07-28 x402-list review](2026-07-28-x402-list-review.md).
