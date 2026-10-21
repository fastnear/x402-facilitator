# Vendored crates

Each directory here is a byte-for-byte copy of a crate published on crates.io
plus one documented upstream change, applied through `[patch.crates-io]` in the
root workspace and in `fuzz/Cargo.toml`. A vendored crate exists only until the
published crate carries the same change; `scripts/check.sh` refuses to let the
pinned version move while the copy is still present.

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
- Why: every published `near-crypto` 0.37.x and 0.38.0 release candidate still
  pins `aws-lc-rs =1.16.2`, while the `rustls` and `rustls-webpki` releases that
  fix RUSTSEC-2026-0285, -0346, -0348, -0349 and -0382 require `aws-lc-rs ^1.18`.
  Cargo allows one `aws-lc-rs` 1.x in the graph, so the pin blocked those fixes.
- Scope of risk: `near-crypto` uses `aws-lc-rs` only for ML-DSA-65, which this
  facilitator rejects for payer keys; ed25519 and secp256k1 use other crates.
- Remove when a published `near-crypto` that this workspace can pin carries the
  change: delete this directory, both `[patch.crates-io]` entries, and the
  guard in `scripts/check.sh`, then bump the NEAR pins.
