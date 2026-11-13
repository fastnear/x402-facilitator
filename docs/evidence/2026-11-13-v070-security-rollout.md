# v0.7.0 dependency security rollout — 2026-11-13

Status: completed. v0.7.0 is published and deployed to all eight enabled
workload processes, with post-promotion validation and monitoring checks passed.

## Source and gates

- Merged [PR #226](https://github.com/fastnear/x402-facilitator/pull/226).
- Released source:
  [`3d144fe15169b9824eb7e464332fc76de75b8a05`](https://github.com/fastnear/x402-facilitator/commit/3d144fe15169b9824eb7e464332fc76de75b8a05).
- Signed annotated tag `v0.7.0`, object
  `8d26023e7d23537f082e6dc009766723dc3ca04a`; local SSH verification and
  GitHub tag and source-commit verification passed.
- [PR CI](https://github.com/fastnear/x402-facilitator/actions/runs/43482221650),
  [PR fuzzing](https://github.com/fastnear/x402-facilitator/actions/runs/43482221637),
  and [merged-main CI](https://github.com/fastnear/x402-facilitator/actions/runs/43484783789)
  passed, including the production container build.
- [Release workflow](https://github.com/fastnear/x402-facilitator/actions/runs/43484860696).
- [Dependency review and compatibility evidence](2026-11-13-dependency-updates.md).

The fix reaches published k256 0.14.0 / elliptic-curve 0.14.3 /
crypto-bigint 0.7.11 through five source-verified temporary Alloy API patches.
No exception was added for RUSTSEC-2026-0387. The exact recorded patch
reproduces every vendored byte from the checksum-pinned published archives.
The 256 deterministic signature vectors, old synthetic transaction recovery,
low/high-S policy and invalid-signature regression tests pass. Local full-gate
validation includes 203 Rust tests and PostgreSQL/official-client conformance.

## Host and rollback preflight

At 2026-11-13 18:39:49 UTC, the three enabled facilitators were active on
v0.6.0, with zero nonterminal settlements in all three journals and successful
migrations 1–3. Public facilitator smoke tests, both merchants' ten unpaid
challenges, and all three demos' official-package matcher comparisons passed.
Nginx configuration and concrete service-unit validation passed; root disk
usage was 22 percent.

Rollback checkpoints:

| Workload | Previous immutable target |
| --- | --- |
| Three facilitators | `/opt/x402-near-facilitator/releases/v0.6.0` |
| NEAR and Base merchants | `/opt/x402-merchant/releases/git-cdb2d003d3eb6f98f9bcf714723581628f057505` |
| Three demos | `/opt/x402-demo/releases/ed59d9f` |

The existing backup service completed successfully at checkpoint
`20261113T184148Z`, including its off-host uploads. All four instance database
dumps were root-owned mode 0600 and passed complete decoding with
`pg_restore --file=/dev/null`. Canary, metrics and backup timers were active.
A stale failed transient cleanup unit from 2026-10-23 referred to an already
absent temporary file; its failure status was cleared without reading or
changing any credential. The host then had zero failed units.

## Artifact admission and promotions

Published [v0.7.0](https://github.com/fastnear/x402-facilitator/releases/tag/v0.7.0)
passed the complete release workflow. Independent admission verified the exact
nine-asset set, all checksum sidecars, source/tag/manifest agreement, and native
and OCI provenance plus CycloneDX SBOM attestations against the pinned release
workflow and source. Both SBOMs contain k256 0.14.0 and crypto-bigint 0.7.11
with no older crypto-bigint component.

- Native archive SHA-256:
  `f0424f723da7ab3293f277c369096e7d98931a3a9c16f99f8063809c43395b76`.
- Immutable image:
  `ghcr.io/fastnear/x402-facilitator@sha256:19fb7719217c1eb89420d890873c6b3f154e85a82372d0e873ad09bc46c2312e`.

The native archive and installer were copied into root-only staging and checked
against independently verified hashes before execution. The immutable install
passed both binary version checks and all packaged deployment/document/example
checksum manifests before any pointer changed. No database migration ran.

NEAR testnet was promoted and started at 2026-11-13 19:03:42 UTC. Its loopback
readiness and full public deployment verifier passed with `NRestarts=0` and
`ExecMainStatus=0`. Three further public readiness samples passed across a
refresh cycle and the testnet journal again had zero nonterminal settlements.
This testnet checkpoint was reviewed before either mainnet promotion.

Base mainnet started on v0.7.0 at 2026-11-13 19:04:40 UTC with
`NRestarts=0` / `ExecMainStatus=0`. Loopback readiness, the public deployment
verifier and three further readiness samples passed before the NEAR mainnet
promotion.

NEAR mainnet started on v0.7.0 at 2026-11-13 19:05:03 UTC with
`NRestarts=0` / `ExecMainStatus=0`; loopback readiness and the full public
verifier passed. All three facilitator pointers resolve to
`/opt/x402-near-facilitator/releases/v0.7.0`.

## Application rollout

The packaged merchant installer verified its source manifest, installed locked
production dependencies with lifecycle scripts disabled, and passed all 97
merchant checks before publishing `/opt/x402-merchant/releases/v0.7.0`.
NEAR was promoted first, then Base after its independent gate passed. The
installed regression checker verified each public origin's release ID and
OpenAPI provenance, readiness, schemas, CORS and all ten unpaid challenges.
Both public origins report release ID `v0.7.0`.

The demo source was copied from the verified release into private staging,
compared byte-for-byte, and installed with locked production dependencies and
lifecycle scripts disabled. All 20 runtime JavaScript tests and import checks
passed under the host's Node.js 22.23.1 before publishing the immutable
`/opt/x402-demo/releases/v0.7.0` directory. The shared `app` pointer was moved
atomically; processes were restarted one at a time in testnet, Base, mainnet
order, with an unpaid challenge gate between promotions. Each running process's
actual working directory resolves to the new immutable release.

| Workload | Process start (UTC) | New immutable target |
| --- | --- | --- |
| NEAR testnet facilitator | 19:03:42 | facilitator `v0.7.0` |
| Base mainnet facilitator | 19:04:40 | facilitator `v0.7.0` |
| NEAR mainnet facilitator | 19:05:03 | facilitator `v0.7.0` |
| NEAR merchant | 19:06:26 | merchant `v0.7.0` |
| Base merchant | 19:07:06 | merchant `v0.7.0` |
| NEAR testnet demo | 19:07:54 | demo `v0.7.0` |
| Base mainnet demo | 19:08:13 | demo `v0.7.0` |
| NEAR mainnet demo | 19:08:23 | demo `v0.7.0` |

## Final validation

At 2026-11-13 19:08:52 UTC, all eight processes were active with
`NRestarts=0` and `ExecMainStatus=0`. Running facilitator executable paths and
Node working directories matched the admitted immutable releases. The three
enabled facilitator journals again contained zero nonterminal settlements.
Nginx validation passed and the host had zero failed units.

All three public deployment verifiers passed `/healthz`, `/readyz`, landing,
`/supported`, agent guide, OpenAPI, discovery and unauthenticated `/verify`
rejection. Further readiness samples passed across refresh cycles. All three
demos returned the configured unpaid 402 challenge; each `accepts` entry
matched the pinned official 2.30.0 requirements builder and matcher and the
pre-upgrade public contract. Base retained the correct `USD Coin` / `2`
domain and legacy-v1 challenge. The merchant dual-origin regression passed
after both promotions.

Canary, metrics and backup timers were active. All 27 scoped CloudWatch alarms
were `OK`. The first naturally scheduled canary after the last demo promotion
started at 19:12:20 UTC and completed successfully at 19:12:40 UTC with
`Result=success` and `ExecMainStatus=0`. The host still had zero failed units.

SSH preflight had found the operator allowlist bound to an older workstation
address. A temporary single-address `/32` SSH rule was used for this rollout
and removed after the final canary. The original allowlist was preserved and
its restored state verified. Public transfer files and root-only installer
staging were cleaned up.

Validation used public, unpaid requests; it created no payment signature,
API credential or funded broadcast. The disabled Base Sepolia workload was not
promoted. Database schema, configuration, policy, budgets, keys and DNS were
unchanged. Prior immutable targets remain available for rollback.
