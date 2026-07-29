#!/bin/sh
set -eu

usage() {
  echo "usage: $0 <near|base> <vX.Y.Z[-prerelease]|git-40hex|legacy-YYYYMMDD-id>" >&2
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

require_exact_legacy_npm_link() {
  link=$1
  raw_target=$2
  expected_target=$3

  [ -L "$link" ] || {
    echo "error: merchant legacy release is missing an expected npm link" >&2
    exit 1
  }
  [ "$(readlink -- "$link")" = "$raw_target" ] || {
    echo "error: merchant legacy release has an unsafe npm link target" >&2
    exit 1
  }
  resolved_target=$(readlink -e -- "$link") || {
    echo "error: merchant legacy release has a dangling npm link" >&2
    exit 1
  }
  [ "$resolved_target" = "$expected_target" ] || {
    echo "error: merchant legacy release has an escaping npm link" >&2
    exit 1
  }
  [ -f "$resolved_target" ] && [ ! -L "$resolved_target" ] || {
    echo "error: merchant legacy npm link must resolve to a regular file" >&2
    exit 1
  }
  [ "$(stat -c '%u:%g' -- "$link")" = "0:0" ] || {
    echo "error: merchant legacy npm link must be owned by root:root" >&2
    exit 1
  }
}

require_safe_release_links() {
  release=$1
  version=$2

  # The current installer uses npm --no-bin-links. This tightly scoped
  # compatibility exception is only for the one historical immutable release
  # that predates that policy; arbitrary or escaping symlinks never pass.
  case "$version" in
    20260727-regression-audit-v4)
      if find "$release" -xdev -type l \
        ! -path "$release/node_modules/.bin/mustache" \
        ! -path "$release/node_modules/.bin/node-gyp-build" \
        ! -path "$release/node_modules/.bin/node-gyp-build-optional" \
        ! -path "$release/node_modules/.bin/node-gyp-build-test" \
        -print -quit | grep -q .; then
        echo "error: merchant legacy release contains an unexpected npm link" >&2
        exit 1
      fi

      require_exact_legacy_npm_link \
        "$release/node_modules/.bin/mustache" \
        "../mustache/bin/mustache" \
        "$release/node_modules/mustache/bin/mustache"
      require_exact_legacy_npm_link \
        "$release/node_modules/.bin/node-gyp-build" \
        "../node-gyp-build/bin.js" \
        "$release/node_modules/node-gyp-build/bin.js"
      require_exact_legacy_npm_link \
        "$release/node_modules/.bin/node-gyp-build-optional" \
        "../node-gyp-build/optional.js" \
        "$release/node_modules/node-gyp-build/optional.js"
      require_exact_legacy_npm_link \
        "$release/node_modules/.bin/node-gyp-build-test" \
        "../node-gyp-build/build-test.js" \
        "$release/node_modules/node-gyp-build/build-test.js"
      ;;
    *)
      if find "$release" -xdev -type l -print -quit | grep -q .; then
        echo "error: merchant release contains an unsafe link" >&2
        exit 1
      fi
      ;;
  esac
}

require_safe_release_tree() {
  release=$1
  if find "$release" -xdev ! -type d ! -type f ! -type l -print -quit | grep -q .; then
    echo "error: merchant release contains a special file" >&2
    exit 1
  fi
  require_safe_release_links "$release" "$version"
  if find "$release" -xdev \( ! -user root -o ! -group root \) -print -quit | grep -q .; then
    echo "error: merchant release contains a path not owned by root:root" >&2
    exit 1
  fi
  # Linux reports symlink metadata as 0777 even though it is not an access
  # control mode. Any permitted link was already exact-validated above; its
  # resolved regular file is covered by this immutable-tree scan.
  if find "$release" -xdev ! -type l -perm /022 -print -quit | grep -q .; then
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
  '^(v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?|git-[0-9a-f]{40}|[0-9]{8}-[0-9A-Za-z][0-9A-Za-z.-]*)$' ||
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
