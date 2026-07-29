# x402 facilitator for NEAR and Base

A production Rust facilitator for x402 `exact` payments in Circle USDC. One
durable settlement engine provides authenticated verification, exactly-once
claims, sponsorship budgets, PostgreSQL journaling, crash recovery, and
chain-specific terminality for:

- **NEAR** — `near:testnet` and `near:mainnet`, using classic NEP-366 delegate
  actions that carry one NEP-141 `ft_transfer`;
- **Base / EVM** — `eip155:84532` and `eip155:8453`, using ERC-3009
  `transferWithAuthorization` and a configured confirmation depth.

x402 v2 is canonical. Every internal value, journal fingerprint, and test uses
v2. An EVM instance may additionally enable the off-by-default `accept_v1`
compatibility gate; legacy v1 requests are strictly translated to canonical v2
at the HTTP boundary and share the same settlement identity.

> **Historical name:** the repository, service binary, admin binary, systemd
> units, and Rust package names retain `x402-near-facilitator` because the
> project launched on NEAR before the shared engine gained EVM support. The
> name is a compatibility identifier, not a statement that the engine is
> NEAR-only.

## Project status

The software supports NEAR and Base mainnet and testnet profiles. Dated
paid-flow evidence exists for NEAR mainnet/testnet and Base mainnet; Base
Sepolia is a configured rollout target and is not claimed as a live public
deployment. This repository deliberately separates software release state from
deployment state: a GitHub release does not imply that any public instance has
been promoted. Dated launch and end-to-end records live in
[`docs/evidence/`](docs/evidence/); the
[v0.5.3 RPC-readiness rollout](docs/evidence/2026-07-27-v053-rpc-readiness-rollout.md)
is the best starting point. Public reference endpoints have no availability
SLA.

The [2026-07-28 x402-list review](docs/evidence/2026-07-28-x402-list-review.md)
found the implementation and submitted evidence technically sound, but did not
find independently attributable Base adoption. Historical canary payments are
not evidence of third-party merchant use. A future facilitator resubmission is
therefore gated on an independent Base resource server, merchant-controlled
recipient, and organic payer activity—not a price change or operator-funded
traffic.

| Network | Reference facilitator status | Wire dialects supported by the software |
| --- | --- | --- |
| `near:mainnet` | `https://x402.mikedotexe.com` | v2 |
| `near:testnet` | `https://test.x402.mikedotexe.com` | v2 |
| `eip155:8453` | `https://base.x402.mikedotexe.com` | v2; gated legacy v1 |
| `eip155:84532` | configured target: `base-test.x402.mikedotexe.com` (not claimed live) | v2; gated legacy v1 |

## Integrate with a reference instance

Start by reading the target's public `/supported` and `/readyz` endpoints,
then follow the [end-to-end access guide](docs/reference-access.md). Reference
credentials are manually approved, restricted to an exact network, canonical
USDC asset, and payee, and issued separately for every resource-server
instance and environment. Open a
[public access request](https://github.com/fastnear/x402-facilitator/issues/new?template=access_request.yml)
only after you have an independently operated resource-server URL and payee;
never include credentials or signed payments in the issue. A prospective Base
mainnet merchant should instead use the dedicated
[Base merchant-pilot request](https://github.com/fastnear/x402-facilitator/issues/new?template=base_merchant_pilot.yml), which asks for a live paid method/path, public discovery, and public evidence of payee control without requesting a payment.

The [runnable Express example](examples/resource-server/) shows official x402
middleware, bounded retries, and delivery idempotency. Base mainnet
integrations must use Circle USDC's real EIP-712 domain `USD Coin` / `2`.
Base Sepolia is supported by the software but is not a live public reference
instance.

## Deliberate scope

- Scheme `exact` only; one pinned network and one canonical Circle USDC
  contract per process.
- API-key authentication on `/verify` and `/settle`, with exact per-client
  network, asset, and payee policy rows.
- A chain-enforced single-use anchor for every payment: the domain-prefixed
  signed-delegate hash on NEAR and the ERC-3009 authorization nonce on EVM.
- Exact signed submission bytes and hash persisted before broadcast; ambiguous
  submission is reconciled by the stored hash and is never replaced.
- NEAR succeeds only when the unique inner token receipt reaches
  `SuccessValue`.
- EVM succeeds only at the configured confirmation depth and re-evaluates a
  mined transaction that disappears before terminality.

Native NEAR, arbitrary NEP-141 assets, non-Base EVM networks, wildcard payees,
anonymous settlement, gas-key relayers, and runtime-loaded third-party chain
plugins are out of scope.

## Architecture

The workspace contains a production service and two mechanism/provider crates:

- `x402-near-facilitator` — Axum HTTP and policy boundary, durable engine,
  PostgreSQL store, leadership, recovery, telemetry, and admin CLI;
- `x402-chain-near` — reusable NEAR verification, preflight, signing, RPC, and
  receipt-graph validation;
- `x402-chain-eip155-provider` — upstream EVM verification plus durable
  transaction preparation, submission, confirmation, and reorg reconciliation.

The engine dispatches through a closed `ChainProvider` enum so providers retain
rich typed results. A new chain is an audited in-tree addition, not a runtime
plugin: it requires a provider crate and enum arm **plus** configuration,
canonical parsing, durable schema/store projection, recovery behavior,
fixtures, operational checks, and documentation. See
[architecture](docs/architecture.md) and the decision-complete
[adding-a-chain guide](docs/adding-a-chain.md).

## HTTP API

| Method | Path | Authentication | Purpose |
| --- | --- | --- | --- |
| `GET` | `/` | Public | Identify the project and link capabilities, access onboarding, source, and security policy |
| `GET` | `/supported` | Public | Advertise this instance's network, scheme, dialects, signer, and extensions |
| `GET` | `/healthz` | Public | Process liveness and build version |
| `GET` | `/readyz` | Public | Sanitized database, leadership, recovery, RPC, and signer readiness |
| `POST` | `/verify` | API key | Verify without broadcasting |
| `POST` | `/settle` | API key | Claim, reverify, submit once, and wait for terminal proof |

`X-API-Key` is primary; `Authorization: Bearer` is also accepted. If both are
present they must contain the same key. Expected payment rejection is an HTTP
200 x402 response. Authentication, malformed input, policy, quota,
idempotency conflict, and indeterminate infrastructure use HTTP errors.

The normative wire contract is [OpenAPI 3.1](docs/openapi.yaml). It documents
the canonical v2 NEAR/EVM payload union and the gated EVM-only v1 transport.
Reference instances are manually allowlisted; their
[access-request process](docs/reference-access.md) is separate from the
self-hosting instructions below.

## Build and test

The required Rust toolchain is pinned by `rust-toolchain.toml`.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
```

Those are fast checks: PostgreSQL integration tests and the official Node
client conformance check skip unless their explicit local prerequisites are
present. For the strongest single local gate, which supplies isolated
PostgreSQL and requires official Node client conformance, follow
[CONTRIBUTING.md](CONTRIBUTING.md) and run:

```sh
./scripts/check-full.sh
```

CI remains authoritative for packaging, deployment, OpenAPI, and
dependency-policy jobs. No funded account is needed for development or the
full local gate.

## Run your own instance

1. Choose exactly one supported network and copy its non-secret JSON example
   from `deploy/config/`.
2. Provision a PostgreSQL database, run forward-only migrations with
   `x402-near-admin migrate`, and give the service a DML-only database role.
3. Put the database URLs, dedicated relayer/signer key, and API-key pepper in
   mode-0600 files; pass only their file paths to the process.
4. Create an API client and an exact network/asset/payee policy with
   `x402-near-admin`.
5. Start the service on a loopback address, then require `/readyz` before
   exposing it through a TLS reverse proxy.

The full configuration contract is in
[docs/configuration.md](docs/configuration.md). The portable
[operations runbook](docs/runbook.md) covers provisioning, readiness,
incidents, upgrades, and rollback; dated reference-deployment details remain
separate historical records rather than defaults.

The [Express reference resource server](examples/resource-server/) demonstrates
NEAR and EVM payment requirements, canonical v2 middleware, gated legacy v1
translation on EVM, retry behavior, and independent delivery idempotency.

## Documentation and contributing

Start with the [documentation index](docs/README.md). It separates user,
operator, contributor, design, research, and dated-evidence material.

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md), open a
chain proposal before implementing another chain family, and report
vulnerabilities through the private process in [SECURITY.md](SECURITY.md).

## License and attribution

Licensed under Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE). This
project interoperates with x402-rs, the x402 specifications and official
packages, Circle USDC, NEAR, and Base; those projects and names remain owned by
their respective maintainers. No affiliation or endorsement is implied.
