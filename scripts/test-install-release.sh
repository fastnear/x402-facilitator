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
merchant_legacy_previous=$merchant_release_root/20260727-regression-audit-v4
unsafe_mode_version=v999.999.997
unsafe_link_version=v999.999.996
unsafe_owner_version=v999.999.995
unsafe_metadata_version=v999.999.994
unsafe_mode_source=/opt/x402-near-facilitator/releases/$unsafe_mode_version
unsafe_link_source=/opt/x402-near-facilitator/releases/$unsafe_link_version
unsafe_owner_source=/opt/x402-near-facilitator/releases/$unsafe_owner_version
unsafe_metadata_source=/opt/x402-near-facilitator/releases/$unsafe_metadata_version
unsafe_mode_merchant=$merchant_release_root/$unsafe_mode_version
unsafe_link_merchant=$merchant_release_root/$unsafe_link_version
unsafe_owner_merchant=$merchant_release_root/$unsafe_owner_version
unsafe_metadata_merchant=$merchant_release_root/$unsafe_metadata_version
commit=0000000000000000000000000000000000000001
commit_release=git-$commit
merchant_commit_destination=$merchant_release_root/$commit_release
merchant_current=$merchant_install_root/current-near
unsafe_merchant_root_link_created=false
restore_opt_mode=false
original_opt_mode=

[ ! -e "$destination" ] || {
  echo "error: reserved installer-test destination already exists: $destination" >&2
  exit 1
}
[ ! -e "$merchant_destination" ] &&
  [ ! -e "$merchant_previous" ] &&
  [ ! -e "$merchant_legacy_previous" ] &&
  [ ! -e "$merchant_commit_destination" ] &&
  [ ! -e "$merchant_current" ] &&
  [ ! -e "$unsafe_mode_source" ] &&
  [ ! -e "$unsafe_link_source" ] &&
  [ ! -e "$unsafe_owner_source" ] &&
  [ ! -e "$unsafe_metadata_source" ] &&
  [ ! -e "$unsafe_mode_merchant" ] &&
  [ ! -e "$unsafe_link_merchant" ] &&
  [ ! -e "$unsafe_owner_merchant" ] &&
  [ ! -e "$unsafe_metadata_merchant" ] || {
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
    "$merchant_legacy_previous" \
    "$merchant_commit_destination" \
    "$unsafe_mode_source" \
    "$unsafe_link_source" \
    "$unsafe_owner_source" \
    "$unsafe_metadata_source" \
    "$unsafe_mode_merchant" \
    "$unsafe_link_merchant" \
    "$unsafe_owner_merchant" \
    "$unsafe_metadata_merchant"
  if [ "$unsafe_merchant_root_link_created" = true ] && [ -L "$merchant_install_root" ]; then
    rm "$merchant_install_root"
  fi
  rm -f "$merchant_current"
  if [ "$restore_opt_mode" = true ]; then
    chmod "$original_opt_mode" /opt
  fi
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
  "$repo_root/deploy/merchant/x402-merchant-api@.service" \
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
grep -Fx \
  'LoadCredential=facilitator-api-key:/etc/x402-merchant/credentials/%i/api-key' \
  "$destination/deploy/merchant/x402-merchant-api@.service"
grep -Fx \
  'Environment=FACILITATOR_API_KEY_FILE=%d/facilitator-api-key' \
  "$destination/deploy/merchant/x402-merchant-api@.service"
grep -Fx \
  'Environment=MERCHANT_RELEASE_METADATA_REQUIRED=1' \
  "$destination/deploy/merchant/x402-merchant-api@.service"
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
echo "validated immutable facilitator package fixture"

# GitHub's Ubuntu runner deliberately makes /opt group-writable for tooling.
# Verify the production guard rejects that parent, then temporarily restore the
# root-owned production mode so the remaining installer integration checks can
# exercise a safe fixture. The EXIT trap restores the runner's exact mode.
if find /opt -maxdepth 0 -perm /022 -print -quit | grep -q .; then
  if "$destination/deploy/merchant/install-release.sh" \
    "$version" >"$work/unsafe-opt.out" 2>&1; then
    echo "error: merchant installer accepted a writable /opt parent" >&2
    exit 1
  fi
  if ! grep -Fq 'merchant installation root must not be writable by group or other' \
    "$work/unsafe-opt.out"; then
    echo "error: merchant installer rejected writable /opt unexpectedly" >&2
    sed -n '1,40p' "$work/unsafe-opt.out" >&2
    exit 1
  fi
  original_opt_mode=$(stat -c '%a' /opt)
  chmod go-w /opt
  restore_opt_mode=true
fi

if [ ! -e "$merchant_install_root" ] && [ ! -L "$merchant_install_root" ]; then
  install -d -m 0755 "$work/unsafe-merchant-root"
  ln -s "$work/unsafe-merchant-root" "$merchant_install_root"
  unsafe_merchant_root_link_created=true
  if "$destination/deploy/merchant/install-release.sh" \
    "$version" >"$work/unsafe-merchant-root.out" 2>&1; then
    echo "error: merchant installer accepted a linked release root" >&2
    exit 1
  fi
  if ! grep -Fq 'merchant installation directory is missing or unsafe' \
    "$work/unsafe-merchant-root.out"; then
    echo "error: merchant installer rejected the linked release root unexpectedly" >&2
    sed -n '1,40p' "$work/unsafe-merchant-root.out" >&2
    exit 1
  fi
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
test "$(cat "$merchant_destination/.x402-merchant-release-id")" = "$version"
test "$(stat -c '%U:%G %a' "$merchant_destination/.x402-merchant-release-id")" = "root:root 444"
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

# The installer owns this reserved sidecar. A source archive must not preseed
# a plausible provenance value, even when its enclosing release and manifest
# otherwise pass the native-release integrity checks.
cp -a "$destination" "$unsafe_metadata_source"
install -o root -g root -m 0444 /dev/null \
  "$unsafe_metadata_source/examples/merchant-api/.x402-merchant-release-id"
printf '%s\n' 'git-ffffffffffffffffffffffffffffffffffffffff' \
  >"$unsafe_metadata_source/examples/merchant-api/.x402-merchant-release-id"
(
  cd "$unsafe_metadata_source"
  find examples -type f -print0 |
    LC_ALL=C sort -z |
    xargs -0 sha256sum >examples-assets.sha256
)
if "$unsafe_metadata_source/deploy/merchant/install-release.sh" \
  "$unsafe_metadata_version" >"$work/unsafe-metadata.out" 2>&1; then
  echo "error: merchant installer accepted source-provided release metadata" >&2
  exit 1
fi
if ! grep -Fq 'merchant source must not contain installer release metadata' \
  "$work/unsafe-metadata.out"; then
  echo "error: merchant installer rejected the source metadata fixture unexpectedly" >&2
  sed -n '1,40p' "$work/unsafe-metadata.out" >&2
  exit 1
fi
test ! -e "$unsafe_metadata_merchant"

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
test "$(cat "$merchant_commit_destination/.x402-merchant-release-id")" = "$commit_release"
test "$(stat -c '%U:%G %a' "$merchant_commit_destination/.x402-merchant-release-id")" = "root:root 444"
if find "$merchant_commit_destination" -type l -print -quit | grep -q .; then
  echo "error: commit installer published a linked dependency" >&2
  exit 1
fi

cp -a "$merchant_destination" "$merchant_previous"
cp -a "$merchant_destination" "$merchant_legacy_previous"
install -d -m 0755 \
  "$merchant_legacy_previous/node_modules/.bin" \
  "$merchant_legacy_previous/node_modules/mustache/bin" \
  "$merchant_legacy_previous/node_modules/node-gyp-build"
for target in \
  "$merchant_legacy_previous/node_modules/mustache/bin/mustache" \
  "$merchant_legacy_previous/node_modules/node-gyp-build/bin.js" \
  "$merchant_legacy_previous/node_modules/node-gyp-build/optional.js" \
  "$merchant_legacy_previous/node_modules/node-gyp-build/build-test.js"; do
  : >"$target"
  chmod 0555 "$target"
done
ln -s ../mustache/bin/mustache \
  "$merchant_legacy_previous/node_modules/.bin/mustache"
ln -s ../node-gyp-build/bin.js \
  "$merchant_legacy_previous/node_modules/.bin/node-gyp-build"
ln -s ../node-gyp-build/optional.js \
  "$merchant_legacy_previous/node_modules/.bin/node-gyp-build-optional"
ln -s ../node-gyp-build/build-test.js \
  "$merchant_legacy_previous/node_modules/.bin/node-gyp-build-test"
"$destination/deploy/merchant/promote-release.sh" near v999.999.998
test "$(readlink -f "$merchant_current")" = "$merchant_previous"
"$destination/deploy/merchant/promote-release.sh" near "$commit_release"
test "$(readlink -f "$merchant_current")" = "$merchant_commit_destination"
"$destination/deploy/merchant/rollback-release.sh" near v999.999.998
test "$(readlink -f "$merchant_current")" = "$merchant_previous"
"$destination/deploy/merchant/promote-release.sh" near 20260727-regression-audit-v4
test "$(readlink -f "$merchant_current")" = "$merchant_legacy_previous"
"$destination/deploy/merchant/rollback-release.sh" near "$commit_release"
test "$(readlink -f "$merchant_current")" = "$merchant_commit_destination"
"$destination/deploy/merchant/rollback-release.sh" near 20260727-regression-audit-v4
test "$(readlink -f "$merchant_current")" = "$merchant_legacy_previous"

# A legacy compatibility exception must not become a general link allowance.
cp -a "$merchant_destination" "$unsafe_link_merchant"
ln -s server.mjs "$unsafe_link_merchant/server-link"
if "$destination/deploy/merchant/promote-release.sh" \
  near "$unsafe_link_version" >"$work/unsafe-merchant-link.out" 2>&1; then
  echo "error: merchant promotion accepted a link in a non-legacy release" >&2
  exit 1
fi
grep -Fq 'merchant release contains an unsafe link' \
  "$work/unsafe-merchant-link.out"
test "$(readlink -f "$merchant_current")" = "$merchant_legacy_previous"
rm "$unsafe_link_merchant/server-link"

ln -s ../node-gyp-build/bin.js \
  "$merchant_legacy_previous/node_modules/.bin/unexpected"
if "$destination/deploy/merchant/promote-release.sh" \
  near 20260727-regression-audit-v4 >"$work/unexpected-legacy-link.out" 2>&1; then
  echo "error: merchant promotion accepted an extra legacy npm link" >&2
  exit 1
fi
grep -Fq 'merchant legacy release contains an unexpected npm link' \
  "$work/unexpected-legacy-link.out"
test "$(readlink -f "$merchant_current")" = "$merchant_legacy_previous"
rm "$merchant_legacy_previous/node_modules/.bin/unexpected"

legacy_mustache_link=$merchant_legacy_previous/node_modules/.bin/mustache
rm "$legacy_mustache_link"
if "$destination/deploy/merchant/promote-release.sh" \
  near 20260727-regression-audit-v4 >"$work/missing-legacy-link.out" 2>&1; then
  echo "error: merchant promotion accepted a legacy release missing an npm link" >&2
  exit 1
fi
grep -Fq 'merchant legacy release is missing an expected npm link' \
  "$work/missing-legacy-link.out"
test "$(readlink -f "$merchant_current")" = "$merchant_legacy_previous"
ln -s ../mustache/bin/mustache "$legacy_mustache_link"

legacy_link=$merchant_legacy_previous/node_modules/.bin/node-gyp-build
rm "$legacy_link"
ln -s /etc/passwd "$legacy_link"
if "$destination/deploy/merchant/promote-release.sh" \
  near 20260727-regression-audit-v4 >"$work/escaping-legacy-link.out" 2>&1; then
  echo "error: merchant promotion accepted an escaping legacy npm link" >&2
  exit 1
fi
grep -Fq 'merchant legacy release has an unsafe npm link target' \
  "$work/escaping-legacy-link.out"
test "$(readlink -f "$merchant_current")" = "$merchant_legacy_previous"
rm "$legacy_link"
ln -s ../node-gyp-build/optional.js "$legacy_link"
if "$destination/deploy/merchant/promote-release.sh" \
  near 20260727-regression-audit-v4 >"$work/wrong-legacy-link.out" 2>&1; then
  echo "error: merchant promotion accepted a wrong legacy npm link target" >&2
  exit 1
fi
grep -Fq 'merchant legacy release has an unsafe npm link target' \
  "$work/wrong-legacy-link.out"
test "$(readlink -f "$merchant_current")" = "$merchant_legacy_previous"
rm "$legacy_link"
ln -s ../node-gyp-build/bin.js "$legacy_link"
legacy_target=$merchant_legacy_previous/node_modules/node-gyp-build/bin.js
mv "$legacy_target" "$legacy_target.real"
if "$destination/deploy/merchant/promote-release.sh" \
  near 20260727-regression-audit-v4 >"$work/dangling-legacy-link.out" 2>&1; then
  echo "error: merchant promotion accepted a dangling legacy npm link" >&2
  exit 1
fi
grep -Fq 'merchant legacy release has a dangling npm link' \
  "$work/dangling-legacy-link.out"
test "$(readlink -f "$merchant_current")" = "$merchant_legacy_previous"
mv "$legacy_target.real" "$legacy_target"
chown -h 65534:65534 "$legacy_link"
if "$destination/deploy/merchant/promote-release.sh" \
  near 20260727-regression-audit-v4 >"$work/non-root-legacy-link.out" 2>&1; then
  echo "error: merchant promotion accepted a non-root legacy npm link" >&2
  exit 1
fi
grep -Fq 'merchant legacy npm link must be owned by root:root' \
  "$work/non-root-legacy-link.out"
test "$(readlink -f "$merchant_current")" = "$merchant_legacy_previous"
chown -h root:root "$legacy_link"
chmod g+w "$legacy_target"
if "$destination/deploy/merchant/promote-release.sh" \
  near 20260727-regression-audit-v4 >"$work/writable-legacy-target.out" 2>&1; then
  echo "error: merchant promotion accepted a writable legacy npm target" >&2
  exit 1
fi
grep -Fq 'merchant release is writable by group or other' \
  "$work/writable-legacy-target.out"
test "$(readlink -f "$merchant_current")" = "$merchant_legacy_previous"
chmod 0555 "$legacy_target"
"$destination/deploy/merchant/promote-release.sh" near 20260727-regression-audit-v4
test "$(readlink -f "$merchant_current")" = "$merchant_legacy_previous"

echo "facilitator and merchant installer integration tests passed"
