# v0.5.4 NEAR transfer-method rollout — 2026-07-29

Owner: Mike Purvis

Completed 2026-07-30 UTC (2026-07-29 America/Los_Angeles).

This sanitized record covers the v0.5.4 deployment of the NEAR
transfer-method guard from [PR #80](https://github.com/fastnear/x402-facilitator/pull/80)
to the three existing public facilitator instances. No database migration was
required; no payment authorization, funding, or transaction broadcast occurred.

## Release identity and verification

- Merged source revision:
  [`855c2081f24df5ba3ec5d056bb4fa86bf4754022`](https://github.com/fastnear/x402-facilitator/commit/855c2081f24df5ba3ec5d056bb4fa86bf4754022).
- Signed annotated tag: `v0.5.4`, tag object
  `4916d46afc9e066c1d55f9de35b9b0dbb9bfc062`.
- Published release:
  <https://github.com/fastnear/x402-facilitator/releases/tag/v0.5.4>.
- Successful release workflow:
  <https://github.com/fastnear/x402-facilitator/actions/runs/30505286185>.
- Deployed native archive SHA-256:
  `c9a03048fc0dd75b638149d475d382c89b82983e5605ef00c52a8a5254d31a1f`.
- Immutable OCI reference:
  `ghcr.io/fastnear/x402-facilitator@sha256:b6e81b41033c4037f9cd01a5902c4fe12e3d23b968ed5ba6c020e6e8b684f40a`.

After publication, the tag signature, tag object, source commit, release
manifest, and exact nine-asset release set agreed. Every published checksum
passed. Native archive provenance and CycloneDX SBOM attestations matched the
tag, source revision, repository, and release workflow. The packaged deploy,
documentation, and example-application manifests also passed under their
intended root-owned verification boundary.

## Admission, installation, and promotion

Immediately before every pointer change, the relevant journal had zero rows in
`awaiting_retry`, `reserved`, `prepared`, or `submitted`. The same query was
zero after all three promotions for NEAR testnet, Base mainnet, and NEAR
mainnet.

The verified archive was installed once into the immutable
`/opt/x402-near-facilitator/releases/v0.5.4` directory. Both packaged Linux
binaries reported version `0.5.4`. No admin migration command was run: this
release has no schema change from v0.5.3.

The rollout order was NEAR testnet, then Base mainnet, then NEAR mainnet. Each
instance was promoted, restarted, and validated before the next pointer moved.
All three `current-<instance>` pointers now resolve to v0.5.4; v0.5.3 remains
installed as the direct binary rollback target. The merchant release pointers
were deliberately unchanged: their already-deployed immutable release did not
depend on this facilitator-only code change.

Before promotion, `nginx -t` and concrete instantiated `systemd-analyze
verify` checks passed. The packaged promotion path preserved the prior
rollback targets and did not alter configuration, credentials, database
schema, or payment policy.

## Public and protected regression checks

All three public facilitator origins passed the exact v0.5.4 deployment helper
after promotion. Each reported `/healthz` version `0.5.4`, a ready `/readyz`,
and the expected `/supported` discovery document:

- <https://test.x402.mikedotexe.com/> — `near:testnet`;
- <https://base.x402.mikedotexe.com/> — `eip155:8453`; and
- <https://x402.mikedotexe.com/> — `near:mainnet`.

The NEAR testnet and NEAR mainnet instances also received an authenticated,
unpaid `/verify` regression request with an explicitly unsupported
`assetTransferMethod` in both the accepted and requirements objects. Each
returned HTTP 200 with `isValid: false` and
`invalidReason: unsupported_asset_transfer_method`. The protected credential
was supplied through curl standard input rather than a command argument; it is
not recorded here. No `/settle` request was made, no payment authorization was
created, and the active-journal query was zero both before and after each
request.

Both merchant origins were checked as facilitator dependencies and reported
ready. Their immutable pointers remain the previously recorded merchant
release, so this evidence does not claim a merchant redeployment.

## Monitoring and transient dependency observation

After the rollout, the scheduled and manually invoked unpaid canary both
completed successfully. The final manual run recorded successful facilitator
verify and demo-work checks for NEAR mainnet, NEAR testnet, and Base, plus
successful merchant API checks for the NEAR and Base origins. The canary and
metrics timers are active, and the latest metrics run succeeded. A read-only
operator check found all 24 `x402` CloudWatch alarms in `OK` state.

For completeness, a pre-rollout Base merchant-readiness probe returned 503 at
01:42 UTC. Read-only log review shows the Base facilitator's `rpc` and
`relayer` readiness gates became not ready at 01:42:07 UTC and recovered at
01:42:36 UTC. The merchant canary correctly failed closed because it requires
facilitator readiness; the merchant did not restart or create a journal row,
and the facilitator had no restart, settlement, database, or leadership event
in that window. The next scheduled run at 01:47 UTC and the final post-rollout
manual run were successful.

The available logs do not distinguish a transient provider transport failure
from a conservative dual-reader disagreement. This is not treated as a
deployment failure or a relaxation of readiness semantics. A future hardening
item is bounded, secret-free failure classification at readiness transitions,
without logging provider URLs, account addresses, credentials, authorizations,
or transaction identifiers.

## Scope and rollback

This release rejects an unimplemented caller-supplied NEAR asset transfer
method before mechanism parsing. It does not change `/verify`, `/settle`, or
`/supported` wire formats; settlement-engine behavior; database schema; or
migrations.

No paid canary was needed for this guard. Existing dated settlement evidence
remains historical only. This deployment neither manufactures third-party
activity nor changes the independent Base-adoption gate or the 2026-08-03
earliest date for a future facilitator resubmission described in the
[2026-07-28 x402-list review](2026-07-28-x402-list-review.md).
