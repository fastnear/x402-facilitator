#!/bin/sh
set -eu

usage() {
  echo "usage: $0 <near|base> <vX.Y.Z[-prerelease]|git-40hex>" >&2
  exit 2
}

require_root_owned_immutable_directory() {
  directory=$1
  description=$2
  [ -d "$directory" ] && [ ! -L "$directory" ] || {
    echo "error: $description is missing or unsafe: $directory" >&2
    exit 1
  }
  [ "$(stat -c '%u:%g' "$directory")" = "0:0" ] || {
    echo "error: $description must be owned by root:root" >&2
    exit 1
  }
  if find "$directory" -maxdepth 0 -perm /022 -print -quit | grep -q .; then
    echo "error: $description must not be writable by group or other" >&2
    exit 1
  fi
}

require_safe_release_tree() {
  release=$1
  if find "$release" -xdev ! -type d ! -type f -print -quit | grep -q .; then
    echo "error: merchant release contains a link or special file" >&2
    exit 1
  fi
  if find "$release" -xdev \( ! -user root -o ! -group root \) -print -quit | grep -q .; then
    echo "error: merchant release contains a path not owned by root:root" >&2
    exit 1
  fi
  if find "$release" -xdev -perm /022 -print -quit | grep -q .; then
    echo "error: merchant release is writable by group or other" >&2
    exit 1
  fi
}

[ "$#" -eq 2 ] || usage
[ "$(id -u)" -eq 0 ] || {
  echo "error: merchant promotion must run as root" >&2
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
destination=$root/releases/$version
server=$destination/server.mjs
require_root_owned_immutable_directory /opt "merchant installation root"
require_root_owned_immutable_directory "$root" "merchant installation directory"
require_root_owned_immutable_directory "$root/releases" "merchant release directory"
require_root_owned_immutable_directory "$destination" "merchant release"
require_safe_release_tree "$destination"
[ -f "$server" ] && [ ! -L "$server" ] || {
  echo "error: merchant release has no safe server entrypoint" >&2
  exit 1
}

# Fail before changing the pointer when Node cannot parse the selected build.
/usr/bin/node --check "$server"

current=$root/current-$network
temporary=$root/.current-$network.new
rm -f "$temporary"
ln -s "$destination" "$temporary"
mv -Tf "$temporary" "$current"

echo "promoted merchant $network to $version; the service was not restarted"
