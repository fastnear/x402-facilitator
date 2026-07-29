# Merchant API provenance rollout

Date: 2026-07-29

Status: deployed from merged `main`; every deployment validation below was
unpaid.

## Immutable source and rollback checkpoint

- merged source: `000bf1f7d501a6f3e79ce320165019b4d00ae95a`
  (`feat(merchant): publish immutable release provenance (#89)`), verified by
  GitHub as a signed merge commit;
- archive:
  `x402-merchant-git-000bf1f7d501a6f3e79ce320165019b4d00ae95a.tar.gz`;
- archive SHA-256:
  `16cd02746784b2d3b9061ea3797fc52788fad21924a387e3db56df71293e1190`; and
- installed release ID:
  `git-000bf1f7d501a6f3e79ce320165019b4d00ae95a`.

The archive was packaged from a clean detached worktree whose `HEAD` and
fetched `origin/main` both resolved to the merged source. Its checksum and
archive shape were checked locally and again on the host before installation.

Before promotion, both merchant pointers resolved to the prior immutable
release `git-0a0b832fed6526d8fa5d51a9de677d66df08ad6f`. The newly installed
promotion helper selected each prior target without a service restart, proving
that the precise rollback target remained acceptable before either live
pointer changed.

The installer verified the archive in root-only staging, installed locked
production dependencies, and ran all 94 merchant checks. It then published a
root-owned, non-writable release directory with the installer-created sidecar
`.x402-merchant-release-id` owned by `root:root` with mode `0444`. The archive
did not supply that sidecar.

## Host validation and staged promotion

The root-owned install, promote, and rollback helpers and systemd template
were checksum-matched before installation. The host passed
`systemd-analyze verify` and `nginx -t`; the template requires
`MERCHANT_RELEASE_METADATA_REQUIRED=1`. The nginx configuration was unchanged
and was not reloaded. Both merchant TLS names passed SNI hostname validation.

NEAR was promoted first. Its immediate startup dependency check exited twice
with the generic fail-closed `merchant dependencies are not ready` result, so
systemd restarted it twice. The third start listened successfully. It then
passed three consecutive public `/readyz` checks, matching `/healthz` and
OpenAPI provenance checks, and the targeted unpaid regression. No later
restart occurred during the Base rollout and final verification.

Only after that gate passed, Base was promoted. It started without a restart
and passed the equivalent three public readiness checks, matching provenance
checks, and targeted unpaid regression.

Final state:

| Instance | Pointer | Service state |
| --- | --- | --- |
| NEAR | `git-000bf1f7d501a6f3e79ce320165019b4d00ae95a` | active, `NRestarts=2`, `ExecMainStatus=0` |
| Base | `git-000bf1f7d501a6f3e79ce320165019b4d00ae95a` | active, `NRestarts=0`, `ExecMainStatus=0` |

The full dual-origin regression was then run from the installed release. It
required both public origins to report the same release ID and passed all ten
unpaid challenges, discovery/CORS/schema checks, and listing surfaces. Its
output states that no payment signature was created or sent.

## Public surface and monitoring

Both merchant origins and both underlying facilitator origins reported ready
dependencies. The Base directory-safe probe
`GET /v1/entities/0x0000000000000000000000000000000000000000` returned HTTP
402, and the public Base landing page links that exact unpaid GET route.

The installed `x402-canary` service completed successfully after promotion and
reported both `MerchantApiOk network=mainnet value=1` and
`MerchantApiOk network=base value=1`. Its five-minute timer remains enabled.
The two scoped CloudWatch alarms were read back in `us-east-1` and were both
`OK`:

- `x402-merchant-mainnet-api-canary-failing`; and
- `x402-merchant-base-api-canary-failing`.

## Boundaries and next step

No paid proof, payment-bearing `/verify` or `/settle` request, payment
signature, wallet funding, or on-chain broadcast was performed for this
rollout. The deployed merchant API is suitable for the free x402-list
**service** submission once an intended submitter email is supplied at
submission time.

This remains an operator-owned service with a facilitator-controlled Base
recipient. It is not evidence of independently operated merchant adoption and
does not authorize facilitator resubmission before 2026-08-03 or without the
independent Base settlement evidence required by the
[2026-07-28 facilitator review](2026-07-28-x402-list-review.md).
