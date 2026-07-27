#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

command -v docker >/dev/null 2>&1 || {
  echo "error: the full check requires Docker" >&2
  exit 1
}
command -v npm >/dev/null 2>&1 || {
  echo "error: the full check requires npm" >&2
  exit 1
}

postgres_image='postgres:16-bookworm@sha256:92620daddcd947f8d5ab5ba66e848702fe443d87fed30c4cea8e389fd78dfc55'
container_name="x402-full-check-$$"

cleanup() {
  docker rm --force "$container_name" >/dev/null 2>&1 || true
  if [ -n "${database_url_file:-}" ]; then
    rm -f "$database_url_file"
  fi
}
trap cleanup EXIT HUP INT TERM

docker run --detach --rm \
  --name "$container_name" \
  --env POSTGRES_DB=facilitator \
  --env POSTGRES_USER=facilitator \
  --env POSTGRES_HOST_AUTH_METHOD=trust \
  --publish 127.0.0.1::5432 \
  "$postgres_image" >/dev/null

postgres_port=$(
  docker inspect \
    --format '{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' \
    "$container_name"
)
database_url="postgres://facilitator@127.0.0.1:${postgres_port}/facilitator"
database_url_file=

attempt=0
until docker exec "$container_name" pg_isready -U facilitator -d facilitator >/dev/null 2>&1; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 30 ]; then
    echo "error: ephemeral PostgreSQL did not become ready" >&2
    exit 1
  fi
  sleep 1
done

database_url_file=$(mktemp)
chmod 600 "$database_url_file"
printf '%s\n' "$database_url" >"$database_url_file"
cargo run --locked --quiet \
  --package x402-near-facilitator \
  --bin x402-near-admin -- \
  migrate --database-url-file "$database_url_file"
rm -f "$database_url_file"
database_url_file=

npm --prefix conformance/http-client ci

X402_FACILITATOR_TEST_DATABASE_URL="$database_url" \
X402_RUN_NODE_CLIENT_CONFORMANCE=1 \
./scripts/check.sh
