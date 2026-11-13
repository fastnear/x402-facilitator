# Vendored crates

Each directory here is a byte-for-byte copy of a crate published on crates.io
plus a documented patch, applied through `[patch.crates-io]` in the
root workspace and in `fuzz/Cargo.toml`. A vendored crate exists only until the
published crate carries the same change; `scripts/check.sh` refuses to let the
resolver bypass a copy or silently move its reviewed version.

## `near-crypto` 0.37.4

- Source: `near-crypto-0.37.4.crate` from crates.io, SHA-256
  `76f1b80805c09154f284d3eec797d700d742a1fb42a8e4dfd561091725d9d9f3` (the
  checksum recorded in `Cargo.lock` before this patch). `Cargo.toml.orig`,
  `.cargo_vcs_info.json`, and the crate's own `Cargo.lock` are omitted; `src`,
  `benches`, and the license files are unchanged except as listed below.
- Applied change: nearcore PR [#16420](https://github.com/near/nearcore/pull/16420)
  (merged 2026-09-16) and [#16617](https://github.com/near/nearcore/pull/16617)
  (merged 2026-10-21). `aws-lc-rs` moves from `=1.16.2` with the `unstable`
  feature to `1.18.2`, and `src/signature.rs` imports the ML-DSA items from
  `aws_lc_rs::signature` instead of `aws_lc_rs::unstable::signature`. Upstream
  states the API is otherwise identical.
- Why: at the time this copy was introduced, every published `near-crypto`
  0.37.x and 0.38.0 release candidate still
  pins `aws-lc-rs =1.16.2`, while the `rustls` and `rustls-webpki` releases that
  fix RUSTSEC-2026-0285, -0346, -0348, -0349 and -0382 require `aws-lc-rs ^1.18`.
  Cargo allows one `aws-lc-rs` 1.x in the graph, so the pin blocked those fixes.
- Scope of risk: `near-crypto` uses `aws-lc-rs` only for ML-DSA-65, which this
  facilitator rejects for payer keys; ed25519 and secp256k1 use other crates.
- Remove when a published `near-crypto` that this workspace can pin carries the
  change: delete this directory, both `[patch.crates-io]` entries, and the
  guard in `scripts/check.sh`, then bump the NEAR pins.

The 2026-11-13 review found the fix in published NEAR 0.38.1. Moving the full
NEAR RPC family also requires Rust 1.95 and near-account-id 3.x, so removal is
deferred to that compiler/public-type migration. The current NEAR pins and
vendored copy remain paired; both lockfiles can take the patched zstd releases
without that migration. See the
[dependency review](../docs/evidence/2026-11-13-dependency-updates.md).

## Temporary Alloy k256 0.14 migration

RUSTSEC-2026-0387 affects reachable crypto-bigint 0.5.5 in the published Alloy
signer graph. Its patched line starts at 0.7.6; no compatible 0.5 fix exists.
Published k256 0.14.0 and elliptic-curve 0.14.3 resolve crypto-bigint 0.7.11.
Alloy tracks the unreleased migration in
[alloy#4445](https://github.com/alloy-rs/alloy/issues/4445) and
[core#1147](https://github.com/alloy-rs/core/pull/1147) (reviewed head
`af6831ffe1ebf4cbf64998363afa266fb6b447fe`, still unmerged on 2026-11-13).
The latter supplies the public-point API rename; the remaining adapters here
are local changes against the published RustCrypto APIs, not merged Alloy fixes.

Five published crates are copied with their exact normalized manifests,
sources, tests and other files, omitting only `Cargo.toml.orig`,
`.cargo_vcs_info.json`, and their own `Cargo.lock`:

| Crate | Version | Changes |
| --- | --- | --- |
| alloy-primitives | 1.7.3 | k256 pin; SEC1 public-point API rename |
| alloy-signer | 2.5.0 | k256/elliptic-curve pins; same point API rename |
| alloy-signer-local | 2.5.0 | k256/rand pins; published `Generate` and `rand::rng` APIs; point API rename |
| alloy-consensus | 2.5.0 | k256 pin; infallible recoverable-signing result; low-S normalization result and recovery parity; point API rename |
| alloy-eip7702 | 0.6.3 | k256 pin only |

Two published trailing-whitespace lines (in `alloy-primitives/src/hex_literal.rs`
and `alloy-consensus/src/transaction/envelope.rs`) are trimmed to satisfy the
repository's diff check; these nonfunctional changes are recorded too.

The complete applied diff is [alloy-k256.patch](alloy-k256.patch).
[alloy-k256-provenance.json](alloy-k256-provenance.json) records each original
crates.io archive SHA-256 and the before/after hashes of every changed file.
`scripts/check-vendored-alloy.py` verifies every file against those original
archives (Cargo cache first, checksum-verified crates.io download otherwise),
rejects extra/omitted/modified files outside that record, checks both resolvers
actually select the copies, and rejects vulnerable crypto-bigint. It runs in
`scripts/check.sh`; release SBOM assertions require the patched crypto versions.
No cryptographic arithmetic, secret-dependent algorithm, x402 verifier, or
advisory exception is changed locally.

Supported scope is the production workspace's deterministic private-key signer
and its existing features. Optional mnemonic, keystore, HSM and alternate
signer backends have different dependency contracts and are not migrated; the
resolution guard rejects their activation. The workspace's `--all-features`
gate covers its own features, not every optional backend of dependencies.

The provider tests compare 256 RFC-6979 signatures and an expired synthetic
EIP-1559 envelope with values recorded using the previous published signer.
They exercise recovery of those old bytes, public-key verification, unchecked
high-S recovery with flipped parity, checked high-S rejection, wrong-message
rejection and zero-scalar rejection. Existing ERC-3009 domain, durable recovery,
and both-chain tests remain required. This is functional compatibility evidence,
not a timing proof or an independent audit of RustCrypto.

Remove the five directories, this patch/provenance pair, both manifest patch
sets and the verification helper once published Alloy crates carry a reviewed
compatible migration. Update both lockfiles and repeat the compatibility tests
and full gate. Cargo patches do not propagate to downstream registry consumers;
production builds must use this source workspace until then.

The published Alloy archives omit license files. The shared
[Apache license](ALLOY-LICENSE-APACHE) and [MIT license](ALLOY-LICENSE-MIT)
are copied from alloy-rs/core at `87080853083c909b34f8c8cff3622570c114985d`
and match alloy-rs/alloy at `59174912873b337ea5868c295cefa70997596f29`.
The [EIPs MIT license](EIPS-LICENSE-MIT) comes from alloy-rs/eips at
`6e469eff444ae73b07dc7c93536db0948f480cfe`; its Apache text is identical.
These are the source revisions embedded in the published archives; each
package's repository/revision is retained in the provenance JSON. The release
NOTICE attributes the copies and selects Apache-2.0 for this distribution.
