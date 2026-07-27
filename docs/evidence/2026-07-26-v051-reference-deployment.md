# v0.5.1 reference deployment rollout — 2026-07-26

Owner: Mike Purvis

Completed 2026-07-27 UTC (2026-07-26 America/Los_Angeles).

This record covers the forward-only v0.5.1 deployment to the three existing
public reference instances. No payment authorization was created and no funded
transaction was broadcast.

## Release identity

- Source revision:
  [`48dcfa62df40373eb25be5a33b140f9724cac122`](https://github.com/fastnear/x402-near-facilitator/commit/48dcfa62df40373eb25be5a33b140f9724cac122).
- Signed annotated tag: `v0.5.1`, tag object
  `a8650d92f1a51892fa0be7b526bd12a032433b46`.
- Published release:
  <https://github.com/fastnear/x402-near-facilitator/releases/tag/v0.5.1>.
- Successful release workflow:
  <https://github.com/fastnear/x402-near-facilitator/actions/runs/30233087880>.
- Deployed native archive SHA-256:
  `f738dd3d2043471d906a59101f1543baf48cbfba14743b8b69455fcd4a1ac59d`.

The archive was installed into the immutable
`/opt/x402-near-facilitator/releases/v0.5.1` release directory. All three live
deployment pointers now resolve to that directory.

## Backup and migration proof

A sanitized configuration snapshot was retained at
`/var/backups/x402-near/config-pre-v0.5.1-20260727T031758Z`.

Immediately before migration, custom-format database dumps were retained in a
root-only local directory and copied successfully to the private off-host
backup prefix `pre-v0.5.1-20260727T031859Z`:

- NEAR testnet:
  `0445e458e23b4a0a7ae5efebac5e9a327f9048befd09a54f94f7acd5be28d6eb`;
- NEAR mainnet:
  `2258ea1fa4e54bbc15cbb19701fc4004467bdef4686a787e4fc64fb9aa972291`;
- Base mainnet:
  `c282de32d6d4ac278cf7c11688df3051f3b1a1d08c70846c63280b21bac94efe`.

Before touching a live database, representative NEAR and Base backups were
restored into scratch databases. Each contained eight settlement rows before
and after migration. Both reached successful migration versions 1, 2, and 3
and the maintenance marker
`x402-maintenance:0003-authorization-scrub:complete`.

The drill also confirmed that the legacy authorization column was absent, the
minimal authorization-metadata column was present, and all eight Base rows had
authorization metadata and chain anchors.

The live migrations preserved their settlement journals:

| Instance | Rows before and after | Final states |
| --- | ---: | --- |
| NEAR testnet | 8 → 8 | 2 failed, 6 succeeded |
| NEAR mainnet | 4 → 4 | 4 succeeded |
| Base mainnet | 8 → 8 | 4 failed, 4 succeeded |

All three live databases report migrations 1, 2, and 3 successful and carry
the completed authorization-scrub marker. There were no active nonterminal
settlements at the migration boundaries.

Because migration 0003 is forward-only, an older v0.4 binary must not be run
against these databases. The verified pre-migration backups remain the
recovery boundary.

## Base sponsorship configuration

The Base mainnet instance was promoted with:

- maximum fee per gas: `1,000,000,000` wei;
- gas limit: `120,000`;
- maximum reservation: `200,000,000,000,000` wei; and
- required confirmation depth: 2.

This bounds the EIP-1559 liability while retaining room in the reservation for
Base's separately estimated L1 data fee.

## Public validation

The rollout order was NEAR testnet, NEAR mainnet, then Base mainnet. Each
service is active and points to v0.5.1.

The public landing page, `/healthz`, `/readyz`, and `/supported` checks passed
for:

- <https://test.x402.mikedotexe.com/> — `near:testnet`, canonical v2;
- <https://x402.mikedotexe.com/> — `near:mainnet`, canonical v2;
- <https://base.x402.mikedotexe.com/> — `eip155:8453` canonical v2 and the
  gated legacy v1 `base` transport.

Each `/healthz` response identified version `0.5.1`. Each `/readyz` response
returned HTTP 200 with database, leadership, reconciliation, RPC, and relayer
checks ready. Each root served the human-facing NEAR-and-Base project page
with source, access, and security links.

All three `/supported` responses advertise the `payment-identifier` extension.
In particular, the Base instance now exposes it for both clients and registry
validation.

The deployment smoke helper passed against all three public instances. It ran
from the exact tagged v0.5.1 source tree; the helper is operator tooling and is
not included in the native release archive.

## Scope and remaining boundary

No funded transaction or replacement transaction was created during this
deployment. Existing dated NEAR and Base canary records remain the paid-flow
evidence.

Base Sepolia remains intentionally absent: it has no live public DNS,
deployment pointer, or enabled service and is not claimed as a reference
deployment.
