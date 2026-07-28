# v0.5.3 RPC-readiness rollout — 2026-07-27

Owner: Mike Purvis

Completed 2026-07-28 UTC (2026-07-27 America/Los_Angeles).

This sanitized record covers the backward-compatible v0.5.3 rollout to the
three existing public reference instances and the Base primary-RPC cutover.
No database migration was required, no payment authorization was created, and
no funded transaction or replacement transaction was broadcast.

## Release identity and verification

- Source revision:
  [`03189ec2f210190e3c5ed2c9cf3d11d636515f29`](https://github.com/fastnear/x402-facilitator/commit/03189ec2f210190e3c5ed2c9cf3d11d636515f29).
- Signed annotated tag: `v0.5.3`, tag object
  `b41f6a89a6c88416b80b4af1385af36d0b31e11d`.
- Published release:
  <https://github.com/fastnear/x402-facilitator/releases/tag/v0.5.3>.
- Successful release workflow:
  <https://github.com/fastnear/x402-facilitator/actions/runs/30319819053>.
- Deployed native archive SHA-256:
  `64df53f0666b7761b4b314239952578e918d56a1b95dedd6c7618f34fb0e8063`.

The release was independently checked after publication:

- the tag signature, tag object, source commit, exact nine-asset release set,
  and `release-manifest.json` agreed;
- every published checksum passed;
- native archive provenance and CycloneDX SBOM attestations matched the tag,
  source revision, repository, and release workflow;
- the embedded archive SBOM byte-matched the external native SBOM, and all
  packaged deployment and documentation asset checksums passed;
- native and OCI SBOMs contained the locked `x402-types 2.0.2` and
  `near-primitives 0.37.2` dependencies; and
- the immutable OCI image's provenance and SBOM attestations also passed when
  read with registry authentication.

GitHub created the new GHCR package with private visibility. The native release
assets used here are public and unaffected. No package-visibility change was
made during this rollout because making a GitHub package public is
irreversible.

The signed v0.5.2 checkpoint had previously passed source, quality, and fuzz
validation, but its stale `near-primitives` SBOM assertion stopped artifact
creation. It remained unpublished and was not installed. v0.5.3 corrected the
assertion, added a pin-drift regression gate, and completed the same release
pipeline without changing runtime, wire, or schema behavior.

## Admission and installation boundary

Immediately before each pointer change, the relevant PostgreSQL journal had
zero rows in `awaiting_retry`, `reserved`, `prepared`, or `submitted`. The same
post-rollout query returned zero for NEAR testnet, Base mainnet, and NEAR
mainnet.

The verified archive was installed once into the immutable
`/opt/x402-near-facilitator/releases/v0.5.3` directory. Both packaged Linux
binaries reported version `0.5.3`. No admin migration command was run:
v0.5.3 has no schema change from the already deployed v0.5.1.

The rollout order was:

1. NEAR testnet at `2026-07-28 01:45:17 UTC`;
2. Base mainnet at `2026-07-28 01:45:48 UTC`; and
3. NEAR mainnet at `2026-07-28 01:46:29 UTC`.

Each instance was promoted and validated before the next pointer moved.
Transient transfer files were removed from the host after installation.

## RPC topology and credential boundary

- NEAR testnet and mainnet retain independent FastNEAR regular and archival
  endpoints.
- Base mainnet now reads its primary endpoint from the dedicated,
  host-restricted Alchemy credential. The independent configured dRPC endpoint
  remains the backup.
- The Alchemy URL remains in the root-owned mode-`0600` credential source. The
  systemd instance receives it through `LoadCredential`; it is absent from the
  repository, command arguments, ordinary environment values, and this record.
- The installed Base drop-in byte-matches
  `deploy/systemd/x402-rpc-credentials.conf.example` at SHA-256
  `f052cf5529461e9c8fe4fe7d4a98911be72a72dc8a68fbebbd91136e54777e93`.

No credential, full provider URL, RPC response body, authorization, or
transaction hash was printed during rollout validation.

## Public and operator validation

All three services are active, and all three `current-<instance>` pointers
resolve to v0.5.3:

- <https://test.x402.mikedotexe.com/> — `near:testnet`, canonical v2;
- <https://base.x402.mikedotexe.com/> — `eip155:8453` canonical v2 and gated
  legacy v1 `base`; and
- <https://x402.mikedotexe.com/> — `near:mainnet`, canonical v2.

For each endpoint, the exact tagged deployment helper passed the landing page,
`/healthz`, `/readyz`, `/supported`, and unauthenticated `/verify` checks.
Every `/healthz` response reported `0.5.3`. Every `/readyz` response returned
HTTP 200 with the database, leadership, reconciliation, RPC, and relayer checks
ready. Base `/supported` advertised both configured dialects and the
`payment-identifier` extension.

Each service emitted the bounded `readiness_gate_transition` event for the
five expected gate names. Startup leadership and reconciliation observations
briefly moved from `not_ready` to `ready`; no endpoint, account, transaction,
or authorization value appeared in those events.

Post-rollout host and monitoring checks also passed:

- zero failed systemd units;
- `x402-near-metrics.timer` active and the oneshot successful with its
  three-minute deadline;
- `logrotate.service` successful;
- fresh Base signer and both NEAR relayer balance datapoints reached
  CloudWatch at `2026-07-28 01:50:00 UTC`; and
- every alarm with the `x402` prefix was `OK`.

The existing `t3.small` host remains sufficient; no instance resize or new
AWS cost commitment was introduced.

## Rollback and scope

The immediate binary rollback is v0.5.1, which shares the current schema. Each
instance can be stopped, repointed independently with the packaged promotion
tool, and restarted. A Base rollback also moves the new systemd drop-in into
the root-only maintenance backup directory and reloads systemd before
starting v0.5.1. The previous monitoring assets and retired overlapping
logrotate rule remain recoverable under the dated root-only maintenance
backup.

No funded canary was needed for this operations-only patch. Existing dated
NEAR and Base settlement records remain the paid-flow evidence. Base Sepolia
also remains a configured target rather than a claimed live deployment.
