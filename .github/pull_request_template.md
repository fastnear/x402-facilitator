## Outcome

<!-- What invariant or user/operator outcome changes? -->

## Design and failure modes

<!-- Include ambiguity, recovery, concurrency, reorg/finality, and rollback impact where relevant. -->

## Validation

- [ ] Fast Rust checks pass.
- [ ] `./scripts/check-full.sh` passes, or the omitted gates and reason are stated below.
- [ ] New concurrency or recovery behavior has a deterministic regression test.
- [ ] No test or fixture contains funded credentials or a live signed payment.

Commands and omitted gates:

```text

```

## Compatibility and operations

- [ ] Canonical v2 behavior remains authoritative; any legacy handling stays at the boundary.
- [ ] Cross-chain parity was preserved, or the non-applicable behavior is explained.
- [ ] Public API / pre-1.0 breakage is called out.
- [ ] Migration, startup compatibility, rollback, and reconciliation impact are documented.
- [ ] No deployment, canary, funded transaction, alert, or partner integration is claimed without dated evidence.

## Documentation

- [ ] OpenAPI and examples are updated for wire-visible changes.
- [ ] Configuration, runbook, threat model, and changelog are updated where relevant.
- [ ] Dependency changes include fixture/oracle and advisory review.
