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
  # rustls 0.23.42 and rustls-webpki 0.103.13 carry five open advisories whose
  # fixes (rustls >= 0.23.46, rustls-webpki >= 0.103.16) all require
  # aws-lc-rs ^1.18: RUSTSEC-2026-0285 (handshake messages accepted across an
  # encryption-level change; the transcript stays authenticated),
  # RUSTSEC-2026-0346 (derived Debug on client session values prints
  # resumption secrets; no custom ClientSessionStore formats them here),
  # RUSTSEC-2026-0348 (Brotli certificate decompression; the brotli feature is
  # not enabled), RUSTSEC-2026-0349 (an unauthenticated ServerHello can pin a
  # classical key exchange for later connections to that name), and
  # RUSTSEC-2026-0382 (certificate paths through distrusted emSign
  # intermediates whose private keys are public are still accepted when the
  # trust anchors allow them; the bundled webpki root stores are on 1.0.10 and
  # the platform verifier depends on the host's OS trust store).
  # near-crypto 0.37.x (and 0.38.0-rc.5) pins aws-lc-rs =1.16.2 while reqwest
  # 0.13's default TLS path (alloy, opentelemetry-http) activates rustls's
  # aws-lc-rs backend, so neither lockfile can take the fixes yet. Keep the
  # exceptions only while the resolver still selects that pinned version; the
  # moment it moves, bump rustls and rustls-webpki and delete this block.
  selected=$(cargo tree --workspace --edges normal,build -i aws-lc-rs --depth 0 --locked 2>/dev/null | head -n 1)
  if [ "$selected" != "aws-lc-rs v1.16.2" ]; then
    echo "error: workspace resolves '$selected'; bump rustls and rustls-webpki and remove the aws-lc-rs-blocked RustSec exceptions" >&2
    exit 1
  fi
  selected=$(cargo tree --manifest-path fuzz/Cargo.toml --edges normal,build -i aws-lc-rs --depth 0 --locked 2>/dev/null | head -n 1)
  if [ "$selected" != "aws-lc-rs v1.16.2" ]; then
    echo "error: fuzz workspace resolves '$selected'; bump rustls and rustls-webpki and remove the aws-lc-rs-blocked RustSec exceptions" >&2
    exit 1
  fi
  aws_lc_rs_blocked='--ignore RUSTSEC-2026-0285 --ignore RUSTSEC-2026-0346 --ignore RUSTSEC-2026-0348 --ignore RUSTSEC-2026-0349 --ignore RUSTSEC-2026-0382'
  # shellcheck disable=SC2086
  cargo audit --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0235 $aws_lc_rs_blocked
  # shellcheck disable=SC2086
  cargo audit --file fuzz/Cargo.lock --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0235 $aws_lc_rs_blocked
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
