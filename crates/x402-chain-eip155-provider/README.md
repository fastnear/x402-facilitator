# x402-chain-eip155-provider

Durable EVM provider support for x402 v2 `exact` Circle USDC payments on Base.

The crate reuses the pinned upstream `x402-chain-eip155` verification
implementation and adds the durable submit path required by the shared
facilitator engine:

- ERC-3009 `transferWithAuthorization` verification over the token's real
  EIP-712 domain;
- deterministic EIP-1559 preparation at a pinned signer nonce and configured
  maximum fee per gas;
- exact signed RLP and transaction-hash validation;
- raw broadcast without blind replacement;
- independent primary/backup durable reads;
- Base execution plus L1 data-fee accounting; and
- confirmation-depth terminality and mined-then-missing reorg handling.

The ERC-3009 authorization nonce is the chain-enforced single-use anchor.
Broadcast is never assumed terminal; reconciliation evaluates the stored
transaction identity against the configured confirmation depth.

## Public API

The crate exposes typed preparation, provider, and settlement modules plus the
upstream `V2Eip155Exact` and `Eip155ChainProvider` building blocks. The durable
provider is designed for an audited in-tree integration and is not a runtime
plugin ABI.

EOA and deployed EIP-1271 smart-wallet signatures can be prepared for
settlement. Counterfactual EIP-6492 signatures may be understood by upstream
verification but are rejected by this submit path because settlement would
require deployment and authorization in one transaction.

Payment authorizations, payer signatures, signer keys, and signed RLP are
sensitive bearer or key material. They must never appear in logs, snapshots,
or public fixtures.

## Compatibility

The workspace publishes provider crates on one lockstep version. Before 1.0, a
minor release may intentionally change the public Rust API; such changes are
listed in the workspace
[changelog](https://github.com/fastnear/x402-facilitator/blob/main/CHANGELOG.md).
Patch releases remain backward compatible.

The minimum supported Rust version is declared in the workspace manifest and
pinned by `rust-toolchain.toml`.

## Development

From the workspace root:

```sh
cargo test --package x402-chain-eip155-provider --all-features --locked
```

Provider tests use mocks and deterministic unfunded keys; they do not broadcast
a transaction.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
