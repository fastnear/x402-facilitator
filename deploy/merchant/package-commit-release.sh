#!/bin/sh
set -eu

usage() {
  echo "usage: $0 <merged-40hex-commit> <output-directory>" >&2
  exit 2
}

[ "$#" -eq 2 ] || usage
commit=$1
output_dir=$2
printf '%s\n' "$commit" | grep -Eq '^[0-9a-f]{40}$' || usage
[ -d "$output_dir" ] && [ ! -L "$output_dir" ] || {
  echo "error: output directory is missing or unsafe" >&2
  exit 1
}
output_dir=$(CDPATH= cd -- "$output_dir" && pwd)

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$repo_root"
[ "$(git rev-parse HEAD)" = "$commit" ] || {
  echo "error: commit must be the checked-out HEAD" >&2
  exit 1
}
[ "$(git rev-parse refs/remotes/origin/main)" = "$commit" ] || {
  echo "error: commit must be the fetched origin/main head" >&2
  exit 1
}
git diff --quiet --exit-code
git diff --cached --quiet --exit-code

release_id=git-$commit
name=x402-merchant-$release_id
archive=$output_dir/$name.tar.gz
checksum=$archive.sha256
temporary_tar=
[ ! -e "$archive" ] && [ ! -e "$checksum" ] || {
  echo "error: output already exists for $release_id" >&2
  exit 1
}
complete=false
cleanup() {
  [ -z "$temporary_tar" ] || rm -f "$temporary_tar"
  if [ "$complete" != true ]; then
    rm -f "$archive" "$checksum"
  fi
}
trap cleanup EXIT HUP INT TERM

temporary_tar=$(mktemp "$output_dir/.x402-merchant.XXXXXX")
git archive \
  --format=tar \
  --output="$temporary_tar" \
  --prefix="$name/" \
  "$commit" \
  -- examples/merchant-api
gzip -n -c "$temporary_tar" >"$archive"
rm -f "$temporary_tar"
temporary_tar=
(
  cd "$output_dir"
  sha256sum "$name.tar.gz" >"$name.tar.gz.sha256"
)

tar -tzf "$archive" | grep -q "^$name/examples/merchant-api/package-lock.json$"
if tar -tzf "$archive" | grep -Eq '(^|/)node_modules(/|$)'; then
  echo "error: commit source archive contains node_modules" >&2
  exit 1
fi
if tar -tvzf "$archive" | awk '
  substr($1, 1, 1) != "d" && substr($1, 1, 1) != "-" { found = 1 }
  END { exit found ? 0 : 1 }
'; then
  echo "error: commit source archive contains a link or special file" >&2
  exit 1
fi
(
  cd "$output_dir"
  sha256sum --check "$name.tar.gz.sha256" >/dev/null
)
complete=true

echo "packaged $archive"
echo "packaged $checksum"
