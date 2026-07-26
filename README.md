# x402 facilitator for NEAR and Base

A production Rust facilitator for x402 `exact` payments in Circle USDC. One
durable, chain-neutral settlement engine — PostgreSQL journal, exactly-once
claims, sponsorship budgets, crash reconciliation — settles through per-chain
providers:

- **NEAR** (`near:testnet`, `near:mainnet`) — the flagship integration and, to
  our knowledge, the first production x402 facilitator for NEAR (the first
  NEAR entry in the x402 Foundation facilitators table). The payer signs a
  classic NEP-366 delegate action; the facilitator sponsors the outer
  transaction through a dedicated relayer and reports success only after the
  inner NEP-141 `ft_transfer` receipt succeeds.
- **Base / EVM** (`eip155:8453`; `eip155:84532` Base Sepolia as the drill
  tier) — the payer signs an ERC-3009 `transferWithAuthorization` (EIP-712);
  the facilitator submits it with sponsored gas and reports success only after
  the transaction holds a configured confirmation depth, with mined-then-
  missing transactions re-evaluated rather than assumed final.

Both x402 wire dialects are spoken. **v2 is canonical everywhere.** eip155
instances may additionally enable `accept_v1` to serve legacy v1 (0.x SDK)
clients: v1 requests are strictly translated to canonical v2 at the parse
boundary — one settlement identity per payment regardless of dialect — and
`/supported` then advertises both kinds.

> **Status: live (2026-07-26).** Three production facilitators run on the
> launch host — NEAR mainnet `x402.mikedotexe.com` and testnet
> `test.x402.mikedotexe.com` (v0.3.0), and Base mainnet
> `base.x402.mikedotexe.com` (v0.4.0 with `accept_v1`) — with real paid
> traffic through the public demo workloads, including a third-party client
> settling USDC on Base end to end. Dated proof lives in
> [docs/evidence/](docs/evidence/) (start with the
> [go-lives](docs/evidence/2026-07-23-mainnet-golive.md) and the
> [multi-chain + legacy-v1 entry](docs/evidence/2026-07-26-legacy-v1-compat-and-base-e2e.md));
> the gate-by-gate record is in [the launch checklist](docs/launch-checklist.md).

## Deployment profile

| Instance | Network | URL | Wire dialects |
| --- | --- | --- | --- |
| NEAR mainnet | `near:mainnet` | `https://x402.mikedotexe.com` | v2 |
| NEAR testnet | `near:testnet` | `https://test.x402.mikedotexe.com` | v2 |
| Base mainnet | `eip155:8453` | `https://base.x402.mikedotexe.com` | v2 + legacy v1 (`accept_v1`) |
| Base Sepolia | `eip155:84532` | scaffolded, not currently deployed | v2 (+ v1 capable) |

Each instance is one process pinned to one network and one Circle USDC
contract, with its own Unix user, PostgreSQL database, credentials, port, and
hostname. Public demo resource servers front each live instance
(`x402-demo.mikedotexe.com`, `x402-demo-test.mikedotexe.com`,
`x402-demo-base.mikedotexe.com`).

The service is intentionally narrow:

- Scheme `exact` only, one configured Circle USDC contract per process.
- NEAR: classic NEP-366 delegate actions (ED25519 and SECP256K1), NEP-141
  `ft_transfer` receipts as the success authority.
- eip155: ERC-3009 `transferWithAuthorization` bound to the chain's canonical
  Circle USDC, terminal only at the configured confirmation depth.
- API-key authentication on `/verify` and `/settle`; exact per-client
  network, asset, and payee allowlists.
- PostgreSQL-backed settlement deduplication, sponsorship budgets, and
  restart reconciliation; chain-enforced single-use anchors (the delegate
  hash on NEAR, the ERC-3009 authorization nonce on eip155).
- Optional `payment-identifier` idempotency, advertised by `/supported`.
- Legacy v1 wire only behind `accept_v1`, only on eip155 (config validation
  rejects it for NEAR, whose networks v1 never covered).

Native NEAR payments, arbitrary NEP-141 assets, non-Base EVM chains,
anonymous settlement, wildcard payees, gas-key relayers, and DelegateV2
remain out of scope.

## Architecture

The workspace builds two binaries and two chain crates around a deliberate
seam: the settlement engine speaks **neutral value types** and dispatches
through a `ChainProvider` enum (`crates/x402-near-facilitator/src/chain.rs`)
— enum dispatch rather than trait objects, chosen so provider contracts can
keep rich typed results for a closed chain set (rationale in
[EVM design](docs/evm-v2-design.md)). Adding a chain means implementing a
provider against the neutral contract and adding an enum arm; the durable
journal, recovery, HTTP, and policy layers do not change.

- `x402-near-facilitator` — the authenticated Axum HTTP boundary, the
  chain-neutral durable engine, and the legacy-v1 wire translation
  (`src/v1_compat.rs`, gated by `accept_v1`).
- `x402-chain-near` — the reusable NEAR mechanism: NEP-366 verification,
  block-pinned RPC preflight, outer-transaction signing, and final
  receipt-graph validation, built on the extension traits from
  [`x402-rs`](https://github.com/x402-rs/x402-rs).
- `x402-chain-eip155-provider` — the EVM provider: upstream
  `x402-chain-eip155` verification reused wholesale, plus our durable
  submit, confirmation-depth terminality, and reorg-aware reconciliation.
- `x402-near-admin` — migrations and administrative operations without
  exposing secrets through the public service.

The upstream x402 v2 specifications (core, `exact` NEAR, `exact` EVM) and the
official TypeScript packages are the protocol authority; the legacy v1 wire
format follows the upstream v1 transport specification. See
[architecture](docs/architecture.md) for boundaries and flows,
[EVM design](docs/evm-v2-design.md) for the multi-chain design record, and
[threat model](docs/threat-model.md) for trust and failure analysis.

## HTTP interface

| Method | Path | Authentication | Purpose |
| --- | --- | --- | --- |
| `GET` | `/supported` | Public | Advertise this instance's network(s), scheme, wire dialects, signer, and extensions |
| `GET` | `/healthz` | Public | Process liveness and version |
| `GET` | `/readyz` | Public | Sanitized database, leadership, reconciliation, RPC, and relayer readiness |
| `POST` | `/verify` | API key | Verify without broadcasting |
| `POST` | `/settle` | API key | Claim, reverify, submit once, and wait for the chain's terminal proof |

The primary credential header is `X-API-Key`; `Authorization: Bearer` is also
accepted. A request may send both forms only when they contain the identical
key; conflicting values are rejected with 401. Expected payment rejection is
an HTTP 200 x402 response. Authentication, malformed input, policy limits,
idempotency conflicts, and unavailable or indeterminate infrastructure use
HTTP errors. On `accept_v1` instances, `/verify` and `/settle` also accept the
legacy v1 request shape and echo `network` as the legacy alias (`base`,
`base-sepolia`) in protocol responses. The normative wire contract is in
[OpenAPI](docs/openapi.yaml).

## Development

Rust 1.93 is pinned by `rust-toolchain.toml`.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
```

With `cargo-deny` and `cargo-audit` installed, the complete local check is:

```sh
./scripts/check.sh
```

Parser fuzz targets cover standard-base64/Borsh delegate decoding, strict
NEP-141 transfer JSON, and the canonical HTTP request boundary in both wire
dialects:

```sh
cargo install cargo-fuzz --version 0.13.2 --locked
rustup toolchain install nightly-2026-07-01 --profile minimal
cargo +nightly-2026-07-01 fuzz run decode_signed_delegate
cargo +nightly-2026-07-01 fuzz run decode_ft_transfer_args
cargo +nightly-2026-07-01 fuzz run parse_http_request
```

For local-only configuration, use `.env.example` as a variable inventory and
export file paths explicitly; the binary does not implicitly load `.env`.
Use unfunded or testnet credentials. Production uses JSON configuration and
systemd credentials, never an environment file. The complete configuration
contract and non-secret examples are in
[configuration](docs/configuration.md) and `deploy/config/`.

The [runnable Express reference resource server](examples/resource-server/)
serves every live demo from one codebase: it registers the official NEAR or
EVM server scheme by network, dual-emits the 402 (canonical v2
`PAYMENT-REQUIRED` header everywhere, plus a legacy v1 JSON body on eip155
and an informational hint body on NEAR), accepts legacy v1 `X-PAYMENT`
payments on eip155 by translating them to v2 before the official middleware,
treats `payment-identifier` as optional, and independently deduplicates paid
work delivery.

## Operations

Production runs as hardened per-instance systemd services
(`x402-near-facilitator@{mainnet,testnet,base}`) behind Nginx on a single
personal host. Releases are installed under
`/opt/x402-near-facilitator/releases/<version>` and selected through atomic
per-instance `current-<instance>` symlinks, so each instance is promoted or
rolled back independently — the NEAR fleet runs v0.3.0 while Base runs
v0.4.0. Installation never changes a pointer: the packaged promotion tool
first runs an on-host `--version` ABI smoke check, then promotes one named
instance. An OCI image is published as a portable artifact, but it is not the
production runtime. Because the launch policy requires a loopback bind, run
the image with host networking or an equivalent loopback-only network
boundary; it deliberately exposes no bridge-network port.

Start with:

1. [API key administration](docs/api-keys.md)
2. [Operations runbook](docs/runbook.md)
3. [Configuration contract](docs/configuration.md)
4. [Launch checklist](docs/launch-checklist.md)

Publishing a GitHub release does not deploy it. The drill tier (NEAR testnet,
Base Sepolia) must pass funded, restart, and fault-injection acceptance before
a mainnet promote. Every funded launch or provisioning broadcast, including
testnet, requires an immediate human confirmation of the network, asset,
amount, payer, recipient, relayer/signer, and maximum sponsored gas. Account
and access-key changes follow the same fresh-preview gate; see the runbook.

## License and attribution

Licensed under Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE). This
repository depends on and follows the modular shape of x402-rs; it does not
claim affiliation with or endorsement by x402-rs, Circle, or the x402
Foundation. Report vulnerabilities through the private process in
[SECURITY.md](SECURITY.md).
