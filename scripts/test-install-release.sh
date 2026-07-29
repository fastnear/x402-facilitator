#!/bin/sh
set -eu

[ "$(id -u)" -eq 0 ] || {
  echo "error: installer integration test must run as root" >&2
  exit 1
}

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
version=v999.999.999
archive_root=x402-near-facilitator-$version-x86_64-unknown-linux-gnu
destination=/opt/x402-near-facilitator/releases/$version
merchant_install_root=/opt/x402-merchant
merchant_release_root=$merchant_install_root/releases
merchant_destination=$merchant_release_root/$version
merchant_previous=$merchant_release_root/v999.999.998
unsafe_mode_version=v999.999.997
unsafe_link_version=v999.999.996
unsafe_owner_version=v999.999.995
unsafe_mode_source=/opt/x402-near-facilitator/releases/$unsafe_mode_version
unsafe_link_source=/opt/x402-near-facilitator/releases/$unsafe_link_version
unsafe_owner_source=/opt/x402-near-facilitator/releases/$unsafe_owner_version
unsafe_mode_merchant=$merchant_release_root/$unsafe_mode_version
unsafe_link_merchant=$merchant_release_root/$unsafe_link_version
unsafe_owner_merchant=$merchant_release_root/$unsafe_owner_version
commit=0000000000000000000000000000000000000001
commit_release=git-$commit
merchant_commit_destination=$merchant_release_root/$commit_release
merchant_current=$merchant_install_root/current-near
unsafe_merchant_root_link_created=false

[ ! -e "$destination" ] || {
  echo "error: reserved installer-test destination already exists: $destination" >&2
  exit 1
}
[ ! -e "$merchant_destination" ] &&
  [ ! -e "$merchant_previous" ] &&
  [ ! -e "$merchant_commit_destination" ] &&
  [ ! -e "$merchant_current" ] &&
  [ ! -e "$unsafe_mode_source" ] &&
  [ ! -e "$unsafe_link_source" ] &&
  [ ! -e "$unsafe_owner_source" ] &&
  [ ! -e "$unsafe_mode_merchant" ] &&
  [ ! -e "$unsafe_link_merchant" ] &&
  [ ! -e "$unsafe_owner_merchant" ] || {
  echo "error: reserved merchant installer-test path already exists" >&2
  exit 1
}

work=$(mktemp -d)
cleanup() {
  rm -rf \
    "$work" \
    "$destination" \
    "$merchant_destination" \
    "$merchant_previous" \
    "$merchant_commit_destination" \
    "$unsafe_mode_source" \
    "$unsafe_link_source" \
    "$unsafe_owner_source" \
    "$unsafe_mode_merchant" \
    "$unsafe_link_merchant" \
    "$unsafe_owner_merchant"
  if [ "$unsafe_merchant_root_link_created" = true ] && [ -L "$merchant_install_root" ]; then
    rm "$merchant_install_root"
  fi
  rm -f "$merchant_current"
}
trap cleanup EXIT HUP INT TERM

payload=$work/payload/$archive_root
install -d -m 0755 \
  "$payload/deploy/merchant" \
  "$payload/examples/merchant-api" \
  "$payload/examples/resource-server"
printf '%s\n' '#!/bin/sh' 'exit 0' >"$payload/x402-near-facilitator"
printf '%s\n' '#!/bin/sh' 'exit 0' >"$payload/x402-near-admin"
install -m 0755 \
  "$repo_root/deploy/promote-release.sh" \
  "$payload/deploy/promote-release.sh"
install -m 0755 \
  "$repo_root/deploy/merchant/install-release.sh" \
  "$repo_root/deploy/merchant/package-commit-release.sh" \
  "$repo_root/deploy/merchant/promote-release.sh" \
  "$repo_root/deploy/merchant/rollback-release.sh" \
  "$payload/deploy/merchant/"
cp "$repo_root"/examples/merchant-api/*.mjs \
  "$repo_root"/examples/merchant-api/package.json \
  "$repo_root"/examples/merchant-api/package-lock.json \
  "$payload/examples/merchant-api/"
printf '%s\n' \
  '{"name":"resource-server-install-fixture","private":true}' \
  >"$payload/examples/resource-server/package.json"
(
  cd "$payload"
  find examples -type f -print0 |
    LC_ALL=C sort -z |
    xargs -0 sha256sum >examples-assets.sha256
)
chmod 0755 "$payload/x402-near-facilitator" "$payload/x402-near-admin"

archive=$work/$archive_root.tar.gz
tar -czf "$archive" -C "$work/payload" "$archive_root"
(
  cd "$work"
  sha256sum "$archive_root.tar.gz" >"$archive_root.tar.gz.sha256"
)

"$repo_root/deploy/install-release.sh" \
  "$archive" \
  "$archive.sha256"

test -x "$destination/x402-near-facilitator"
test -x "$destination/x402-near-admin"
test -x "$destination/deploy/promote-release.sh"
test -x "$destination/deploy/merchant/install-release.sh"
test -x "$destination/deploy/merchant/package-commit-release.sh"
test -f "$destination/examples/merchant-api/package-lock.json"
test -f "$destination/examples/resource-server/package.json"
(
  cd "$destination"
  sha256sum --check examples-assets.sha256 >/dev/null
)
test ! -e /opt/x402-near-facilitator/current-mainnet
test ! -e /opt/x402-near-facilitator/current-testnet

# The finished release directory must be root-owned and traversable by the
# unprivileged service users, without granting write to group or other.
test "$(stat -c '%U:%G %a' "$destination")" = "root:root 755"

if [ ! -e "$merchant_install_root" ] && [ ! -L "$merchant_install_root" ]; then
  install -d -m 0755 "$work/unsafe-merchant-root"
  ln -s "$work/unsafe-merchant-root" "$merchant_install_root"
  unsafe_merchant_root_link_created=true
  if "$destination/deploy/merchant/install-release.sh" \
    "$version" >"$work/unsafe-merchant-root.out" 2>&1; then
    echo "error: merchant installer accepted a linked release root" >&2
    exit 1
  fi
  grep -Fq 'merchant installation directory is missing or unsafe' \
    "$work/unsafe-merchant-root.out"
  rm "$merchant_install_root"
  unsafe_merchant_root_link_created=false
fi

"$destination/deploy/merchant/install-release.sh" "$version"
test -f "$merchant_destination/server.mjs"
test -d "$merchant_destination/node_modules"
if find "$merchant_destination" -type l -print -quit | grep -q .; then
  echo "error: merchant installer published a linked dependency" >&2
  exit 1
fi
test "$(stat -c '%U:%G %a' "$merchant_destination")" = "root:root 755"
test ! -L "$merchant_install_root"
test ! -L "$merchant_release_root"
test "$(stat -c '%U:%G' "$merchant_install_root")" = "root:root"
test "$(stat -c '%U:%G' "$merchant_release_root")" = "root:root"

chown 65534:65534 "$merchant_destination/server.mjs"
if "$destination/deploy/merchant/promote-release.sh" \
  near "$version" >"$work/non-root-merchant-child.out" 2>&1; then
  echo "error: merchant promotion accepted a non-root-owned child path" >&2
  exit 1
fi
grep -Fq 'merchant release contains a path not owned by root:root' \
  "$work/non-root-merchant-child.out"
test ! -e "$merchant_current"
chown root:root "$merchant_destination/server.mjs"

# The tag installer must trust only an immutable native release. These checks
# occur before it reads the manifest, so a locally writable path, link, or
# non-root file cannot become its source application.
cp -a "$destination" "$unsafe_mode_source"
chmod g+w "$unsafe_mode_source/examples/merchant-api/server.mjs"
if "$unsafe_mode_source/deploy/merchant/install-release.sh" \
  "$unsafe_mode_version" >"$work/unsafe-mode.out" 2>&1; then
  echo "error: merchant installer accepted a group-writable facilitator source" >&2
  exit 1
fi
grep -Fq 'facilitator release contains a path writable by group or other' \
  "$work/unsafe-mode.out"
test ! -e "$unsafe_mode_merchant"

cp -a "$destination" "$unsafe_link_source"
mv "$unsafe_link_source/examples/merchant-api/server.mjs" \
  "$unsafe_link_source/examples/merchant-api/server.mjs.real"
ln -s server.mjs.real "$unsafe_link_source/examples/merchant-api/server.mjs"
if "$unsafe_link_source/deploy/merchant/install-release.sh" \
  "$unsafe_link_version" >"$work/unsafe-link.out" 2>&1; then
  echo "error: merchant installer accepted a linked facilitator source" >&2
  exit 1
fi
grep -Fq 'facilitator release contains a link or special file' \
  "$work/unsafe-link.out"
test ! -e "$unsafe_link_merchant"

cp -a "$destination" "$unsafe_owner_source"
chown 65534:65534 "$unsafe_owner_source/examples/merchant-api/server.mjs"
if "$unsafe_owner_source/deploy/merchant/install-release.sh" \
  "$unsafe_owner_version" >"$work/unsafe-owner.out" 2>&1; then
  echo "error: merchant installer accepted a non-root facilitator source" >&2
  exit 1
fi
grep -Fq 'facilitator release contains a path not owned by root:root' \
  "$work/unsafe-owner.out"
test ! -e "$unsafe_owner_merchant"

commit_archive_root=$work/commit-source/x402-merchant-$commit_release
install -d -m 0755 "$commit_archive_root/examples"
cp -a "$payload/examples/merchant-api" "$commit_archive_root/examples/"
commit_archive=$work/x402-merchant-$commit_release.tar.gz
tar -czf "$commit_archive" \
  -C "$work/commit-source" \
  "x402-merchant-$commit_release"
(
  cd "$work"
  sha256sum "x402-merchant-$commit_release.tar.gz" \
    >"x402-merchant-$commit_release.tar.gz.sha256"
)
"$destination/deploy/merchant/install-release.sh" \
  "$commit_release" \
  "$commit_archive" \
  "$commit_archive.sha256"
test -f "$merchant_commit_destination/server.mjs"
test -d "$merchant_commit_destination/node_modules"
if find "$merchant_commit_destination" -type l -print -quit | grep -q .; then
  echo "error: commit installer published a linked dependency" >&2
  exit 1
fi

cp -a "$merchant_destination" "$merchant_previous"
"$destination/deploy/merchant/promote-release.sh" near v999.999.998
test "$(readlink -f "$merchant_current")" = "$merchant_previous"
"$destination/deploy/merchant/promote-release.sh" near "$commit_release"
test "$(readlink -f "$merchant_current")" = "$merchant_commit_destination"
"$destination/deploy/merchant/rollback-release.sh" near v999.999.998
test "$(readlink -f "$merchant_current")" = "$merchant_previous"

echo "facilitator and merchant installer integration tests passed"
