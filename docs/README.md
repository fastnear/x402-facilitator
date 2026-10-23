# Documentation

This index separates current software contracts from operator-specific history.
Unless a page is explicitly dated or labeled as a design record, it describes
the current source tree rather than the state of a public deployment.

## Use and integrate

- [Project overview](../README.md) — capabilities, scope, architecture, and
  build entry points.
- [OpenAPI 3.1 contract](openapi.yaml) — canonical v2 NEAR/EVM requests,
  responses, and the gated EVM-only v1 transport.
- [Configuration](configuration.md) — non-secret JSON and credential-file
  contract for one network per process.
- [API-key administration](api-keys.md) — client credentials, exact payee
  policies, budgets, rotation, and revocation.
- [Reference access](reference-access.md) — safe onboarding for the public,
  API-key-gated reference instances.
- [Merchant discovery catalog](catalog.md) — immutable manifest, public API,
  admission evidence, opt-in, removal, and non-claim policy.
- [TypeScript recipes](typescript-recipes.md) — tested public discovery and
  authenticated server-client setup using the pinned official packages.
- [Reference resource server](../examples/resource-server/README.md) — runnable
  Express workload for NEAR or EVM.
- [Agent-facing merchant API](../examples/merchant-api/README.md) — companion
  paid chain-evidence and bounded activity resource server.
- [Merchant API deployment](../deploy/merchant/README.md) — two-process mainnet
  deployment layout and operational sequence.

## Operate

- [Operations runbook](runbook.md) — portable provisioning, readiness,
  incident, upgrade, rollback, and evidence controls.
- [Threat model](threat-model.md) — protected assets, trust boundaries,
  recovery decisions, and review triggers.
- [Launch checklist](launch-checklist.md) — dated assurance record for the
  original deployment.
- [Monitoring assets](../deploy/monitoring/README.md) — host metrics, backups,
  and alerts.

## Understand and extend

- [Architecture](architecture.md) — component boundaries and settlement flows.
- [Adding a chain](adding-a-chain.md) — the complete in-tree provider, schema,
  recovery, fixture, and operations checklist.
- [EVM provider design record](evm-v2-design.md) — why the engine uses a closed
  enum and how EVM was added.
- [Contributor guide](../CONTRIBUTING.md) — local checks, safety, review, and
  pull-request expectations.

## Research and project history

- [NEAR Intents adoption gates](near-intents-adoption-gates.md) and
  [Verifier method design](near-intents-verifier-design.md) — proposed
  mainnet-only transfer method in the existing NEAR provider and service.
- [NEAR Intents 1Click upfront design](near-intents-upfront-design.md) —
  pre-merge implementation boundary and adoption gates for the distinct
  client-submitted cross-chain payment-proof proposal.
- [NEAR Intents research log](near-intents-x402-progress.md) — dated empirical
  work and open decisions.
- [Distribution log](distribution.md) — dated registry and directory outreach.
- [Agent-facing merchant API research](research/agent-facing-merchant-api.md) —
  merchant examples, discovery compatibility, and x402 Scan evidence plan.
- [Registry submissions](registry/README.md) — target-specific historical
  records; never reuse a prior payload for a new submission.
- [Reference deployment runbook snapshot](evidence/2026-07-26-reference-deployment-runbook-snapshot.md)
  — the original operator-specific topology and version procedures, retained
  as history rather than current defaults or go-live evidence.
- [`evidence/`](evidence/) — dated deployment, paid-flow, recovery, and
  hardening evidence. Evidence is never a promise that the same version remains
  deployed today.
- [Agent merchant API deployment](evidence/2026-07-27-agent-merchant-deployment.md)
  — public origins, discovery verification, and paid-flow status.
- [Merchant provenance rollout](evidence/2026-07-29-merchant-provenance-rollout.md)
  — merged-SHA deployment, public provenance checks, canary, and rollback
  checkpoint.
- [v0.6.0 agent discovery rollout](evidence/2026-07-31-v060-agent-discovery-rollout.md)
  — signed release, immutable three-instance promotion, public discovery
  checks, empty catalogs, and monitoring alarms.
- [x402-list Base service submission](evidence/2026-07-29-x402-list-base-service-submission.md)
  — free service submission, five successful unpaid endpoint probes, and
  pending manual review.
- [x402-list facilitator review](evidence/2026-07-28-x402-list-review.md) —
  verified implementation, independent-usage rejection, and evidence gates
  for any resubmission.
