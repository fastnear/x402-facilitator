# Contributing

Thank you for helping make the facilitator safer and easier to reuse. Payment
code rewards small, reviewable changes with explicit invariants and
deterministic tests.

## Before opening a change

- Use a normal bug or documentation issue for scoped improvements.
- Use the chain-proposal form before implementing a new chain family or
  settlement mechanism.
- Report suspected vulnerabilities through the private process in
  [SECURITY.md](SECURITY.md), not a public issue.
- Never paste a live signed payment, funded private key, API key, credentialed
  database URL, or telemetry secret into an issue, test, log, or pull request.

The contributor certificate is the Apache-2.0 contribution language in
[LICENSE](LICENSE): by intentionally submitting a contribution, you represent
that you are entitled to do so under the project license.

## Prerequisites

- the Rust toolchain pinned by `rust-toolchain.toml`;
- PostgreSQL 16 for database, leadership, HTTP, and recovery integration tests;
- Node.js 22.17 and npm for official x402 fixtures, client conformance, and the
  reference resource server;
- Python 3;
- Docker for the packaged installer, container, and Nginx validation;
- pinned `cargo-deny` and `cargo-audit` versions shown in CI when running their
  dependency-policy gates manually.

Use only a loopback PostgreSQL test URL. The tests create isolated temporary
schemas and reject non-loopback database hosts.

## Fast checks

For an ordinary edit:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
```

These commands are intentionally convenient, but they are not the complete CI
gate. Without explicit environment variables, PostgreSQL integration tests and
the official Node client conformance check print a skip message.

## Full local gate

```sh
./scripts/check-full.sh
```

The full script requires Docker and npm, starts an isolated loopback PostgreSQL
16 container, exercises the privileged admin migration and physical scrub,
installs the pinned official-client conformance dependencies, and runs
`scripts/check.sh` with database tests and Node client conformance required.
The inner gate covers Rust formatting, Clippy, workspace tests, configuration
parsing, release-guard tests, documentation links, the tracked-secret guard,
and `git diff --check`. It also runs `cargo-deny` and `cargo-audit` when
installed; CI remains authoritative for missing dependency-policy tools and
its additional packaging and deployment checks.

No funded RPC call or broadcast belongs in the local or CI gate.

## Design rules

- v2 is canonical internally. Legacy v1 is EVM-only, gated, and translated at
  the parse boundary.
- Verify the signature before trusting the claimed payer.
- Fail closed on unknown or conflicting RPC evidence.
- Persist exact signed submission bytes and hash before broadcast.
- Never replace an indeterminate transaction with newly signed bytes.
- Never delete nonterminal journal rows on a timer.
- Keep chain primitives in provider crates and neutral values at the engine
  boundary.
- Give resilience, messages, metrics, and operational controls cross-chain
  parity or document why a behavior cannot apply.

Read [docs/architecture.md](docs/architecture.md),
[docs/threat-model.md](docs/threat-model.md), and
[docs/adding-a-chain.md](docs/adding-a-chain.md) before changing settlement or
recovery behavior.

## Tests and documentation

- Every concurrency or recovery fix needs a deterministic regression test.
- A parser or wire change needs strict unknown-field tests, OpenAPI changes,
  and official-package interoperability coverage.
- A dependency bump under `@x402/*` requires regenerating/checking the pinned
  fixtures and the legacy matcher contract.
- An externally visible or operational change needs the relevant README,
  configuration, runbook, threat model, and changelog update.
- Do not describe a deployment, funded transaction, canary, alert, or partner
  integration as complete without a dated entry under `docs/evidence/`.

## Pull requests

Keep commits reviewable and explain:

1. the invariant or user outcome being changed;
2. the failure modes considered;
3. tests run, including whether the full gate ran;
4. migration and rollback impact;
5. protocol, documentation, and cross-chain parity impact.

The workspace uses one lockstep version. Before 1.0, a minor release may
contain an intentional public-API break, but it must be called out in the
changelog and migration/upgrade notes. Patch releases remain backward
compatible.

## Publishing provider crates

`x402-chain-near` and `x402-chain-eip155-provider` are public reusable crates.
The facilitator service crate is application-only and must remain
`publish = false`.

For each release:

1. work from a clean detached checkout of the verified signed release tag;
2. confirm the intended versions are not already present on crates.io;
3. run `cargo publish --locked --dry-run` for both provider packages;
4. publish only those two exact tagged packages;
5. use `cargo info` from outside the workspace to prove registry resolution;
   and
6. record the crate URLs and crates.io checksums in dated evidence.

Never publish from a dirty worktree, rebuild a historical version from a
different commit, or use the service crate as a transitive public API.
