#!/bin/sh
set -eu

usage() {
  echo "usage: $0 <near|base> <previous-vX.Y.Z[-prerelease]|git-40hex>" >&2
  exit 2
}

[ "$#" -eq 2 ] || usage
[ "$(id -u)" -eq 0 ] || {
  echo "error: merchant rollback must run as root" >&2
  exit 1
}

network=$1
version=$2
case "$network" in
  near | base) ;;
  *) usage ;;
esac
printf '%s\n' "$version" | grep -Eq \
  '^(v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?|git-[0-9a-f]{40})$' ||
  usage

root=/opt/x402-merchant
current=$root/current-$network
[ -L "$current" ] || {
  echo "error: merchant $network has no current release pointer" >&2
  exit 1
}
current_target=$(readlink -f "$current")
case "$current_target" in
  "$root"/releases/*) ;;
  *)
    echo "error: current merchant pointer leaves the release root" >&2
    exit 1
    ;;
esac
[ "$current_target" != "$root/releases/$version" ] || {
  echo "error: merchant $network already points to $version" >&2
  exit 1
}

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
"$script_dir/promote-release.sh" "$network" "$version"
echo "rolled merchant $network back from $(basename -- "$current_target") to $version; restart and verify explicitly"
