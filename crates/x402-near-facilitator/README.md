# x402-near-facilitator service package

Production HTTP, policy, persistence, recovery, and administration boundary for
the shared NEAR and Base x402 facilitator.

The package name predates EVM support and is retained for binary, deployment,
and operator compatibility. It is not a NEAR-only service. The supported
networks and architecture are documented in the workspace
[README](../../README.md).

This crate is a service implementation shared by the
`x402-near-facilitator` and `x402-near-admin` binaries, not a stable
general-purpose Rust library API. Reusable chain mechanisms live in
`x402-chain-near` and `x402-chain-eip155-provider`.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
