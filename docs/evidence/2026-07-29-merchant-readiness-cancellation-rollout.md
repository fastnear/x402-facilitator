# Merchant readiness-cancellation rollout — 2026-07-29

Owner: Mike Purvis

Completed 2026-07-30 UTC (2026-07-29 America/Los_Angeles).

This sanitized record covers the production rollout of
[PR #100](https://github.com/fastnear/x402-facilitator/pull/100). It records
operational readiness, not settlement volume or independently attributable
adoption.

## Source and immutable merchant release

- Merged source revision:
  [`cdb2d003d3eb6f98f9bcf714723581628f057505`](https://github.com/fastnear/x402-facilitator/commit/cdb2d003d3eb6f98f9bcf714723581628f057505).
- GitHub reports a valid signature for the squash-merge commit. The submitted
  source commit was also locally verified as signed.
- Immutable merchant release ID:
  `git-cdb2d003d3eb6f98f9bcf714723581628f057505`.
- Source archive SHA-256:
  `9bc3d5a0931327cccead483a85ab1e9a52eb8ed1aa1ffca959c0042b810df5c1`.

The archive was made by `deploy/merchant/package-commit-release.sh` from a
clean checkout whose `HEAD` exactly matched fetched `origin/main`. The
packager rejected bundled dependencies, links, and special files; the installer
separately verified the checksum and rejected unsafe paths. The archive
contains only the merchant source tree; it is not a new native facilitator tag
or container release.

The merchant package's locked install and full `npm run check` completed with
96 passing tests both before host admission and during the root-owned immutable
installation. `npm audit --audit-level=high` passed. It reported the existing
low-severity `elliptic` advisory inherited through `@x402/near`; the advisory
has no available upstream patch and is tracked separately by Dependabot.

The installer revalidated the archive checksum and paths, installed production
dependencies in a private staging directory, reran the complete merchant test
suite, and published a root-owned immutable release with its installer-owned
provenance sidecar. Installation changed no live pointer and restarted no
service. The checked-in installer, promotion, and rollback helper hashes
already matched their root-owned deployed copies, so no deployment helper,
Nginx configuration, credential, or service-unit change was needed.

## Pre-promotion gate

Immediately before promotion, both `current-near` and `current-base` pointed
to `git-000bf1f7d501a6f3e79ce320165019b4d00ae95a`; both merchant services
were active and Nginx validation passed. TLS hostname validation passed for
both public merchant origins. Their public `/readyz` endpoints were ready, and
the full dual-origin regression passed all ten unpaid challenges, discovery,
schemas, and CORS checks without creating or sending a payment signature.

## Staged promotion and observations

NEAR was promoted first. Its pointer, local and public readiness, health and
OpenAPI provenance, target regression, and service state all passed. It held
at zero restart attempts. The first public probes were intentionally launched
in parallel with the restart and saw HTTP 502 while the Node process had not
yet begun listening. Those probes were not accepted as a rollout pass; the
subsequent sequential readiness and provenance gate passed.

Base was promoted only after that NEAR gate passed. Its first three cold-start
attempts failed closed because the startup dependency snapshot was not ready.
Systemd's configured five-second `on-failure` retry started the successful
process on the fourth attempt. The service did not listen until that dependency
gate passed. The sequential Base gate then showed:

- Base local and public `/readyz` ready, with RPC, facilitator, and payment
  checks all ready;
- matching immutable release ID in public `/healthz` and OpenAPI;
- Base target regression passing all five unpaid challenges; and
- three later local readiness samples with the Base restart counter fixed at
  three.

The final dual-origin regression passed all ten unpaid challenges. Both
merchant pointers now resolve to
`/opt/x402-merchant/releases/git-cdb2d003d3eb6f98f9bcf714723581628f057505`.
Both processes are active; NEAR has zero restart attempts and Base has the
three bounded cold-start attempts above with no further restart. `nginx -t`
passed, and both public facilitator readiness endpoints remained ready.
The prior immutable merchant release remains installed as the direct rollback
target.

The standard canary was not manually invoked because it includes an
authenticated Base `/verify` fixture. Its next naturally scheduled run finished
successfully. This timer result is operational monitoring, not payment or
adoption evidence.

## Scope and listing posture

This change propagates readiness cancellation to the merchant's single,
non-retrying `/supported` discovery request. A peer readiness failure or the
probe deadline now aborts that in-flight discovery request. It does not change
`/verify`, `/settle`, or `/supported` wire formats, settlement behavior,
pricing, policy, database schema, or migrations.

The promotion and regression commands did not create or rotate credentials,
fund wallets, sign a new authorization, call `/settle`, run `npm run proof`,
or broadcast a transaction. The natural canary is intentionally excluded from
that statement as described above. This rollout creates no third-party
settlement evidence and does not change the independent Base-adoption gate or
the 2026-08-03 earliest date for a future facilitator resubmission recorded in
the [2026-07-28 x402-list review](2026-07-28-x402-list-review.md).
