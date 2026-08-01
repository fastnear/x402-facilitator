# v0.6.0 agent discovery rollout — 2026-07-31

Owner: Mike Purvis

Completed 2026-08-01 UTC (2026-07-31 America/Los_Angeles).

This sanitized record covers the production rollout of the agent-ready
onboarding bundle and opt-in merchant catalog from
[PR #105](https://github.com/fastnear/x402-facilitator/pull/105), released
through [PR #106](https://github.com/fastnear/x402-facilitator/pull/106).
It records software and operational readiness, not settlement volume,
merchant quality, or independently attributable adoption. No payment
authorization, API credential, `/settle` request, funded action, or
transaction broadcast was created for this rollout.

## Release identity and verification

- Feature merge:
  [`b3c9942fd89703e7c07f0f64471dfb89b00a1192`](https://github.com/fastnear/x402-facilitator/commit/b3c9942fd89703e7c07f0f64471dfb89b00a1192).
- Released source revision:
  [`55d4c45f63a907df0b252254c551e46b3c538640`](https://github.com/fastnear/x402-facilitator/commit/55d4c45f63a907df0b252254c551e46b3c538640).
- Signed annotated tag: `v0.6.0`, tag object
  `bf21beff6a076e3ee1c1da3c02720c6e0845f4e6`.
- Published immutable release:
  <https://github.com/fastnear/x402-facilitator/releases/tag/v0.6.0>.
- Successful release workflow:
  <https://github.com/fastnear/x402-facilitator/actions/runs/30676748379>.
- Deployed native archive SHA-256:
  `c314f3597b9c07eca1c84125092e245ac0bf309580754ca28898c22f551cb84a`.
- Immutable OCI reference:
  `ghcr.io/fastnear/x402-facilitator@sha256:79138fe51981170908d389362067045cc932f8c513473b579fdb69b84d4e0045`.

The signed source checkpoint, release tag, release target, asset digests, and
release manifest agreed. All published checksum sidecars passed. The native
archive's deploy, documentation, and example-application checksum manifests
also passed after extraction and again on the production host. The release
contains both example applications, the tested TypeScript discovery and
facilitator-client recipes, the OpenAPI document, and the empty embedded
merchant manifest.

GitHub attestations independently verified provenance and CycloneDX SBOMs for
both the native archive and the immutable OCI image against the released
source, tag, signer, and pinned release workflow. The feature and release
pull requests passed their required repository, PostgreSQL, documentation,
dependency-review, TypeScript-oracle, fuzz, and production-container checks.

## Preflight and immutable promotion

Immediately before installation, the v0.5.6 public instances were healthy and
ready, `nginx -t` and concrete `systemd-analyze verify` checks passed, the
canary and metrics timers were active, and disk use was 21 percent. Each of
the three production databases had zero settlements in `awaiting_retry`,
`reserved`, `prepared`, or `submitted`.

The verified archive was installed once into the immutable
`/opt/x402-near-facilitator/releases/v0.6.0` directory. Installation did not
change a live pointer or restart a service. Both packaged binaries reported
version `0.6.0`. No admin migration command ran; this release does not change
the database schema or migrations.

Promotion proceeded one instance at a time in the planned order: NEAR
testnet, Base mainnet, then NEAR mainnet. Each pointer was moved, its service
restarted, and its loopback and public surfaces validated before the next
instance changed. Final state:

| Public origin | Network | Pointer | Started (UTC) |
| --- | --- | --- | --- |
| <https://test.x402.mikedotexe.com/> | `near:testnet` | `v0.6.0` | 2026-08-01 01:26:08 |
| <https://base.x402.mikedotexe.com/> | `eip155:8453` | `v0.6.0` | 2026-08-01 01:26:40 |
| <https://x402.mikedotexe.com/> | `near:mainnet` | `v0.6.0` | 2026-08-01 01:27:48 |

All three services are active with successful main processes. A final query
again found zero active nonterminal settlements in all three journals. The
disabled Base Sepolia target was not promoted or advertised. The immutable
v0.5.6 release remains installed as the direct application rollback target.

## Public discovery and edge validation

For each origin, the release's deployment verifier passed the landing page,
`/healthz`, `/readyz`, `/supported`, `/llms.txt`, `/openapi.yaml`, and
`/discovery/resources` checks plus an unauthenticated `/verify` request that
must return 401. The live OpenAPI response was byte-identical to the release
document. The generated agent guide reported the configured network,
canonical asset, zero facilitator fee, access and discovery links, and, on
Base mainnet, the required `USD Coin`/`2` EIP-712 domain.

All three discovery responses returned `x402Version: 2` with zero items and a
pagination total of zero. The empty state is intentional: no operator-owned
demo is cataloged, and no independent merchant has yet completed the catalog
admission evidence gate. Publication would not by itself imply usage,
quality, or settlement volume.

The signed release Nginx configurations were staged, validated, installed
atomically, and reloaded only after all three application instances passed.
The enabled targets have these SHA-256 digests:

- Base: `8d861caddcadf4a87df6feab58a317c784fa5983ce4868c5e5aba0940a0e8659`;
- NEAR: `2a8494e863044a4943c207181ad6d0ca26f46d8ca7e4246a661f029e2ec660c6`.

The previous edge configurations remain as
`/etc/nginx/sites-available/x402-base.pre-v0.6.0` and
`/etc/nginx/sites-available/x402-near-facilitator.pre-v0.6.0`. Final
`nginx -t` and public all-origin verification passed after reload.

## Monitoring

The release's secret-free canary was installed at
`/usr/local/bin/x402-canary.sh` after preserving the previous version as
`/usr/local/bin/x402-canary.sh.pre-v0.6.0`. Its syntax check and immediate
unpaid run passed. The scheduled run then emitted healthy
`FacilitatorDiscoveryOk` samples for `mainnet`, `testnet`, and `base`, while
the existing verify, demo-work, and merchant checks remained healthy. The
canary and `x402-near-metrics` timers are active and the host has zero failed
units.

Three scoped CloudWatch alarms were created in `us-east-1` with a five-minute
minimum statistic, two of three datapoints required, threshold below one,
missing data treated as breaching, and the existing facilitator alert topic
for both alarm and recovery notifications:

- `x402-mainnet-discovery-canary-failing`;
- `x402-testnet-discovery-canary-failing`; and
- `x402-base-discovery-canary-failing`.

After naturally scheduled canary samples, all three alarms reached `OK`.
Both public merchant readiness endpoints were also healthy at the final
checkpoint.

## Boundaries and next gates

This release adds public, read-only discovery and onboarding surfaces. It does
not change `/verify`, `/settle`, or `/supported` wire formats, the settlement
engine, payment policy, database schema, or migrations. The deployment did
not issue a client credential, add a merchant, make a payment, or produce an
adoption or volume claim.

Catalog admission remains opt-in and evidence-gated. Facilitator relisting
also remains gated on independently attributable Base settlement evidence and
the earliest resubmission date recorded in the
[2026-07-28 x402-list review](2026-07-28-x402-list-review.md). This rollout is
not evidence that either gate has been met.
