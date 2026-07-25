#!/bin/sh
set -eu

usage() {
  echo "usage: $0 <mainnet|testnet|base-sepolia|base> <vX.Y.Z>" >&2
  exit 2
}

[ "$#" -eq 2 ] || usage
[ "$(id -u)" -eq 0 ] || {
  echo "error: promote-release must run as root" >&2
  exit 1
}

environment=$1
version=$2
# The argument is the systemd instance name (%i) and the current-<name>
# pointer, not a NEAR environment: mainnet/testnet are the NEAR instances,
# base-sepolia/base the eip155 ones. Enumerated rather than pattern-matched so
# an unexpected token can never reach the pointer paths below.
case "$environment" in
  mainnet | testnet | base-sepolia | base) ;;
  *) usage ;;
esac
case "$version" in
  v[0-9]*.[0-9]*.[0-9]*) ;;
  *) usage ;;
esac

root=/opt/x402-near-facilitator
destination=$root/releases/$version
binary=$destination/x402-near-facilitator
[ -d "$destination" ] && [ ! -L "$destination" ] || {
  echo "error: release is not installed: $destination" >&2
  exit 1
}
[ -x "$binary" ] && [ ! -L "$binary" ] || {
  echo "error: release binary is missing or unsafe: $binary" >&2
  exit 1
}

# This is also the on-host ABI smoke test. Promotion fails before changing the
# pointer when the native binary cannot execute on the production host.
"$binary" --version >/dev/null

temporary=$root/.current-$environment.new
rm -f "$temporary"
ln -s "$destination" "$temporary"
mv -Tf "$temporary" "$root/current-$environment"

echo "promoted $environment to $version; the service was not restarted"
