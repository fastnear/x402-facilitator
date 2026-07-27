# x402-chain-near

Reusable NEAR support for x402 v2 `exact` Circle USDC payments.

The crate verifies classic NEP-366 signed delegate actions carrying exactly one
NEP-141 `ft_transfer`, performs block-pinned chain preflight, prepares and signs
the facilitator's outer Transaction V0, submits exact bytes, and validates the
final receipt graph. Outer transaction or delegate-receipt success alone is not
accepted: the unique inner token receipt must finish with `SuccessValue`.

It intentionally has no HTTP authentication, tenant policy, PostgreSQL,
deployment, or telemetry dependency. Production orchestration and durable
recovery live in the sibling `x402-near-facilitator` package.

## Public API

The supported surface is re-exported from `lib.rs`:

- strict delegate and transfer decoders;
- `V2NearExact` and the x402-rs facilitator integration;
- `NearChainProvider` and typed verify/prepare/broadcast/reconcile values;
- the `NearRpc` seam and JSON-RPC implementation;
- terminal receipt-graph interpretation and identity validation.

All payment authorizations and signed transactions are bearer instruments.
Callers must redact them from logs and persist only the data required for safe
replay protection and reconciliation.

## Compatibility

The workspace publishes provider crates on one lockstep version. Before 1.0, a
minor release may intentionally change the public Rust API; such changes are
listed in the workspace
[changelog](https://github.com/fastnear/x402-near-facilitator/blob/main/CHANGELOG.md).
Patch releases remain backward compatible.

The minimum supported Rust version is declared in the workspace manifest and
pinned by `rust-toolchain.toml`.

## Development

From the workspace root:

```sh
cargo test --package x402-chain-near --all-features --locked
npm --prefix crates/x402-chain-near/fixtures ci
npm --prefix crates/x402-chain-near/fixtures run check
```

The fixture keys are deterministic public test keys marked `DO NOT FUND`.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
