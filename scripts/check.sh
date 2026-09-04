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
  # rust_decimal's serde-with-arbitrary-precision support declares an inactive
  # rkyv edge in Cargo.lock via x402-types. Refuse the exception if rkyv ever
  # becomes reachable by production workspace or fuzz targets.
  if cargo tree --workspace --edges normal,build -i rkyv 2>/dev/null | grep -q .; then
    echo "error: rkyv became reachable; remove the RUSTSEC-2026-0235 exception" >&2
    exit 1
  fi
  if cargo tree --manifest-path fuzz/Cargo.toml --edges normal,build -i rkyv 2>/dev/null | grep -q .; then
    echo "error: rkyv became reachable in fuzz; remove the RUSTSEC-2026-0235 exception" >&2
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
