#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked
# The fuzz workspace is excluded from the root workspace but pins the same
# local provider crates. Resolve it here so a lockstep version bump cannot
# wait for the separate fuzz workflow to reveal a stale exact dependency.
cargo metadata --manifest-path fuzz/Cargo.toml --locked --format-version 1 >/dev/null
cargo package --locked --allow-dirty -p x402-chain-near
cargo package --locked --allow-dirty -p x402-chain-eip155-provider
python3 -m json.tool docs/catalog/resources.json >/dev/null

# vendor/near-crypto is crates.io near-crypto 0.37.4 plus nearcore#16420/#16617
# (aws-lc-rs 1.18 without the `unstable` feature), applied through
# [patch.crates-io] so rustls and rustls-webpki can take the aws-lc-rs ^1.18
# security releases (RUSTSEC-2026-0285/0346/0348/0349/0382). Drop the directory
# and both [patch] entries as soon as a published near-crypto carries that
# change. This guard refuses a pin bump that leaves the copy behind and refuses
# a copy that the resolver is not actually using.
if [ -d vendor/near-crypto ]; then
  vendored=$(awk '/^\[package\]/ { p = 1 } p && /^version = / { gsub(/"/, "", $3); print $3; exit }' vendor/near-crypto/Cargo.toml)
  pinned=$(sed -n 's/^near-crypto = "=\(.*\)"$/\1/p' Cargo.toml)
  if [ -z "$vendored" ] || [ "$vendored" != "$pinned" ]; then
    echo "error: near-crypto is pinned to '$pinned' but vendor/near-crypto is '$vendored'; remove the vendored patch (vendor/README.md) or re-vendor" >&2
    exit 1
  fi
  grep -q '^near-crypto = { path = "vendor/near-crypto" }$' Cargo.toml || {
    echo "error: the root [patch.crates-io] entry for vendor/near-crypto is missing" >&2
    exit 1
  }
  grep -q '^near-crypto = { path = "../vendor/near-crypto" }$' fuzz/Cargo.toml || {
    echo "error: the fuzz [patch.crates-io] entry for vendor/near-crypto is missing" >&2
    exit 1
  }
  root_aws=$(cargo tree --workspace --edges normal,build -i aws-lc-rs --depth 0 --locked 2>/dev/null | sed -n 's/^aws-lc-rs v//p' | head -n 1)
  fuzz_aws=$(cargo tree --manifest-path fuzz/Cargo.toml --edges normal,build -i aws-lc-rs --depth 0 --locked 2>/dev/null | sed -n 's/^aws-lc-rs v//p' | head -n 1)
  for selected in "root:$root_aws" "fuzz:$fuzz_aws"; do
    version=${selected#*:}
    if [ -z "$version" ] || [ "$(printf '%s\n1.18.0\n' "$version" | sort -V | head -n 1)" != "1.18.0" ]; then
      echo "error: ${selected%%:*} workspace resolves aws-lc-rs '$version'; the vendored near-crypto patch is not in effect" >&2
      exit 1
    fi
  done
fi

if command -v cargo-deny >/dev/null 2>&1; then
  # RustSec is checked by cargo-audit below. Keeping the advisory scan out of
  # cargo-deny also avoids making this gate depend on cargo-deny's support for
  # the newest CVSS document version.
  cargo deny check bans licenses sources
else
  echo "note: cargo-deny is not installed; CI must run this gate" >&2
fi

if command -v cargo-audit >/dev/null 2>&1; then
  # sqlx's facade declares every optional database driver, so Cargo.lock
  # contains the inactive sqlx-mysql -> rsa edge. Refuse the exception if rsa
  # ever becomes reachable by a production workspace target.
  if cargo tree --workspace --edges normal,build -i rsa 2>/dev/null | grep -q .; then
    echo "error: rsa became reachable; remove the RUSTSEC-2023-0071 exception" >&2
    exit 1
  fi
  # rust_decimal still lists optional rkyv 0.7 metadata in Cargo.lock. Keep the
  # RustSec exception only while rkyv is absent from normal/build edges in both
  # the production workspace and the fuzz workspace.
  if cargo tree --workspace --edges normal,build --target all -i rkyv 2>/dev/null | grep -q '^rkyv v'; then
    echo "error: rkyv became reachable; remove the RUSTSEC-2026-0235 exception" >&2
    exit 1
  fi
  if cargo tree --manifest-path fuzz/Cargo.toml --edges normal,build --target all -i rkyv 2>/dev/null | grep -q '^rkyv v'; then
    echo "error: fuzz rkyv became reachable; remove the RUSTSEC-2026-0235 exception" >&2
    exit 1
  fi
  cargo audit --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0235
  cargo audit --file fuzz/Cargo.lock --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0235
else
  echo "note: cargo-audit is not installed; CI must run this gate" >&2
fi

python3 -m json.tool deploy/config/mainnet.json.example >/dev/null
python3 -m json.tool deploy/config/testnet.json.example >/dev/null
python3 -m json.tool deploy/config/base-sepolia.json.example >/dev/null
python3 -m json.tool deploy/config/base.json.example >/dev/null
python3 -B scripts/test_release_guard.py
bash scripts/test-monitoring.sh
python3 scripts/check-docs.py

git diff --check
