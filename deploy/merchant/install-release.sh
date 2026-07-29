#!/bin/sh
set -eu

usage() {
  echo "usage: $0 <vX.Y.Z[-prerelease]>" >&2
  echo "       $0 <git-40hex> <source.tar.gz> <source.tar.gz.sha256>" >&2
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

require_safe_facilitator_release_tree() {
  release=$1
  if find "$release" -xdev ! -type d ! -type f -print -quit | grep -q .; then
    echo "error: facilitator release contains a link or special file" >&2
    exit 1
  fi
  if find "$release" -xdev \( ! -user root -o ! -group root \) -print -quit | grep -q .; then
    echo "error: facilitator release contains a path not owned by root:root" >&2
    exit 1
  fi
  if find "$release" -xdev -perm /022 -print -quit | grep -q .; then
    echo "error: facilitator release contains a path writable by group or other" >&2
    exit 1
  fi
}

ensure_root_owned_immutable_directory() {
  directory=$1
  description=$2
  if [ -e "$directory" ] || [ -L "$directory" ]; then
    require_root_owned_immutable_directory "$directory" "$description"
    return
  fi
  install -d -o root -g root -m 0755 "$directory"
  require_root_owned_immutable_directory "$directory" "$description"
}

[ "$#" -eq 1 ] || [ "$#" -eq 3 ] || usage
[ "$(id -u)" -eq 0 ] || {
  echo "error: merchant installer must run as root" >&2
  exit 1
}

release_id=$1
merchant_install_root=/opt/x402-merchant
merchant_release_root=$merchant_install_root/releases
destination=$merchant_release_root/$release_id
source_staging=
app_staging=
manifest_files=
source_files=

cleanup() {
  [ -z "$app_staging" ] || rm -rf "$app_staging"
  [ -z "$source_staging" ] || rm -rf "$source_staging"
  [ -z "$manifest_files" ] || rm -f "$manifest_files"
  [ -z "$source_files" ] || rm -f "$source_files"
}
trap cleanup EXIT HUP INT TERM

require_root_owned_immutable_directory /opt "merchant installation root"
ensure_root_owned_immutable_directory \
  "$merchant_install_root" "merchant installation directory"
ensure_root_owned_immutable_directory \
  "$merchant_release_root" "merchant release directory"
[ ! -e "$destination" ] && [ ! -L "$destination" ] || {
  echo "error: merchant release already installed: $destination" >&2
  exit 1
}

if [ "$#" -eq 1 ]; then
  printf '%s\n' "$release_id" |
    grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' || usage

  facilitator_release=/opt/x402-near-facilitator/releases/$release_id
  source_app=$facilitator_release/examples/merchant-api
  asset_manifest=$facilitator_release/examples-assets.sha256
  require_root_owned_immutable_directory /opt "facilitator installation root"
  require_root_owned_immutable_directory \
    /opt/x402-near-facilitator "facilitator installation directory"
  require_root_owned_immutable_directory \
    /opt/x402-near-facilitator/releases "facilitator release directory"
  require_root_owned_immutable_directory \
    "$facilitator_release" "facilitator release"
  require_safe_facilitator_release_tree "$facilitator_release"
  [ -f "$asset_manifest" ] && [ ! -L "$asset_manifest" ] || {
    echo "error: facilitator release has no examples checksum manifest" >&2
    exit 1
  }

  manifest_files=$(mktemp)
  source_files=$(mktemp)
  (
    cd "$facilitator_release"
    sha256sum --check examples-assets.sha256 >/dev/null
    awk '
      {
        path = $2
        sub(/^\*/, "", path)
        if (path ~ /^examples\/merchant-api\//) {
          print path
        }
      }
    ' examples-assets.sha256 | LC_ALL=C sort >"$manifest_files"
    find examples/merchant-api -type f -print |
      LC_ALL=C sort >"$source_files"
  )
  cmp "$manifest_files" "$source_files" >/dev/null || {
    echo "error: merchant application does not match the examples manifest" >&2
    exit 1
  }
else
  printf '%s\n' "$release_id" | grep -Eq '^git-[0-9a-f]{40}$' || usage
  archive=$2
  checksum=$3
  [ -f "$archive" ] && [ ! -L "$archive" ] || {
    echo "error: commit source archive is missing or unsafe" >&2
    exit 1
  }
  [ -f "$checksum" ] && [ ! -L "$checksum" ] || {
    echo "error: commit source checksum is missing or unsafe" >&2
    exit 1
  }

  archive_name=$(basename -- "$archive")
  expected_archive_name=x402-merchant-$release_id.tar.gz
  [ "$archive_name" = "$expected_archive_name" ] || {
    echo "error: expected archive $expected_archive_name, got $archive_name" >&2
    exit 1
  }

  source_staging=$(mktemp -d "$merchant_release_root/.source.XXXXXX")
  archive_copy=$source_staging/$archive_name
  checksum_copy=$source_staging/checksum
  install -m 0600 -- "$archive" "$archive_copy"
  install -m 0600 -- "$checksum" "$checksum_copy"

  checksum_lines=$(awk 'NF { count += 1 } END { print count + 0 }' "$checksum_copy")
  [ "$checksum_lines" -eq 1 ] || {
    echo "error: checksum file must contain exactly one non-empty entry" >&2
    exit 1
  }
  set -- $(awk 'NF { print $1, $2 }' "$checksum_copy")
  [ "$#" -eq 2 ] || {
    echo "error: malformed checksum file" >&2
    exit 1
  }
  expected_hash=$1
  expected_name=${2#\*}
  case "$expected_hash" in
    *[!0-9A-Fa-f]* | "") usage ;;
  esac
  [ "${#expected_hash}" -eq 64 ] &&
    [ "$expected_name" = "$archive_name" ] || {
    echo "error: checksum does not identify the commit source archive" >&2
    exit 1
  }
  actual_hash=$(sha256sum "$archive_copy" | awk '{ print $1 }')
  [ "$actual_hash" = "$expected_hash" ] || {
    echo "error: commit source archive checksum mismatch" >&2
    exit 1
  }

  archive_root=x402-merchant-$release_id
  members=$source_staging/archive-members
  tar -tzf "$archive_copy" >"$members"
  awk -v root="$archive_root/" '
    {
      if ($0 == "" || substr($0, 1, length(root)) != root) {
        exit 1
      }
      relative = substr($0, length(root) + 1)
      if (relative != "" &&
          relative != "examples" &&
          relative != "examples/" &&
          relative != "examples/merchant-api" &&
          relative != "examples/merchant-api/" &&
          substr(relative, 1, 22) != "examples/merchant-api/") {
        exit 1
      }
      count = split($0, parts, "/")
      for (part_number = 1; part_number <= count; part_number += 1) {
        if (parts[part_number] == "..") {
          exit 1
        }
      }
    }
  ' "$members" || {
    echo "error: commit source archive has an unsafe or unexpected path" >&2
    exit 1
  }
  if tar -tvzf "$archive_copy" | awk '
    substr($1, 1, 1) != "d" && substr($1, 1, 1) != "-" { found = 1 }
    END { exit found ? 0 : 1 }
  '; then
    echo "error: commit source archive must not contain links or special files" >&2
    exit 1
  fi

  unpack=$source_staging/unpack
  install -d -m 0700 "$unpack"
  tar -xzf "$archive_copy" -C "$unpack" --no-same-owner --no-same-permissions
  if find "$unpack" ! -type f ! -type d -print -quit | grep -q .; then
    echo "error: commit source archive contains a special file" >&2
    exit 1
  fi
  source_app=$unpack/$archive_root/examples/merchant-api
fi

[ -d "$source_app" ] && [ ! -L "$source_app" ] || {
  echo "error: merchant release has no source application" >&2
  exit 1
}
[ -f "$source_app/package.json" ] &&
  [ -f "$source_app/package-lock.json" ] &&
  [ -f "$source_app/server.mjs" ] || {
  echo "error: merchant application is incomplete" >&2
  exit 1
}
[ ! -e "$source_app/node_modules" ] || {
  echo "error: merchant source must not contain node_modules" >&2
  exit 1
}
if find "$source_app" ! -type d ! -type f -print -quit | grep -q .; then
  echo "error: merchant application contains a link or special file" >&2
  exit 1
fi

app_staging=$(mktemp -d "$merchant_release_root/.install.XXXXXX")
cp -R "$source_app/." "$app_staging/"
diff -qr "$source_app" "$app_staging" >/dev/null

npm --prefix "$app_staging" ci \
  --omit=dev \
  --ignore-scripts \
  --no-bin-links \
  --no-audit \
  --no-fund
if find "$app_staging" ! -type d ! -type f -print -quit | grep -q .; then
  echo "error: npm installed a link or special file into the merchant release" >&2
  exit 1
fi
npm --prefix "$app_staging" run check

chown -R root:root "$app_staging"
chmod -R go-w "$app_staging"
chmod 0755 "$app_staging"
mv "$app_staging" "$destination"
app_staging=

echo "installed merchant $release_id; no network was promoted and no service was restarted"
