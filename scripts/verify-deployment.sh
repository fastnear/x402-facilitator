#!/bin/sh
set -eu

usage() {
  echo "usage: $0 <base-url> [api-key-file]" >&2
  exit 2
}

[ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage
base_url=${1%/}
key_file=${2-}

case "$base_url" in
  https://*) ;;
  *)
    echo "error: public deployment checks require an https URL" >&2
    exit 1
    ;;
esac

curl_common="--fail --silent --show-error --max-time 15"

# shellcheck disable=SC2086
curl $curl_common "$base_url/healthz" >/dev/null
# shellcheck disable=SC2086
curl $curl_common "$base_url/readyz" >/dev/null
# shellcheck disable=SC2086
landing=$(curl $curl_common "$base_url/")
# shellcheck disable=SC2086
supported=$(curl $curl_common "$base_url/supported")
# shellcheck disable=SC2086
llms=$(curl $curl_common "$base_url/llms.txt")
# shellcheck disable=SC2086
openapi=$(curl $curl_common "$base_url/openapi.yaml")
# shellcheck disable=SC2086
discovery=$(curl $curl_common "$base_url/discovery/resources?type=http&limit=100&offset=0")

printf '%s' "$landing" | python3 -c '
import sys

body = sys.stdin.read()
assert "<title>x402 facilitator for NEAR and Base</title>" in body
assert "href=\"/supported\"" in body
assert "href=\"/readyz\"" in body
assert "href=\"/llms.txt\"" in body
assert "href=\"/openapi.yaml\"" in body
assert "href=\"/discovery/resources\"" in body
assert "issues/new?template=access_request.yml" in body
assert "docs/reference-access.md" in body
assert "examples/resource-server" in body
'

printf '%s' "$supported" | LANDING="$landing" python3 -c '
import os
import json
import sys

body = json.load(sys.stdin)
kinds = body["kinds"]
assert 1 <= len(kinds) <= 2
kind = kinds[0]
assert kind["x402Version"] == 2
assert kind["scheme"] == "exact"
assert kind["network"] in {
    "near:testnet", "near:mainnet",   # NEAR instances
    "eip155:84532", "eip155:8453",    # Base Sepolia / Base mainnet (eip155)
}
if len(kinds) == 2:
    # accept_v1 eip155 instances additionally advertise the legacy v1 kind.
    legacy = kinds[1]
    assert legacy["x402Version"] == 1
    assert legacy["scheme"] == "exact"
    assert legacy["network"] in {"base", "base-sepolia"}
assert "payment-identifier" in body["extensions"]
assert isinstance(body["signers"], dict)

assets = {
    "near:mainnet": "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1",
    "near:testnet": "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af",
    "eip155:8453": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "eip155:84532": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
}
landing = os.environ["LANDING"]
network = kind["network"]
assert f"id=\"instance-network\">{network}</code>" in landing
assert f"id=\"instance-asset\">{assets[network]}</code>" in landing
'

printf '%s' "$discovery" | SUPPORTED="$supported" LLMS="$llms" python3 -c '
import json
import os
import sys

catalog = json.load(sys.stdin)
supported = json.loads(os.environ["SUPPORTED"])
network = supported["kinds"][0]["network"]
assets = {
    "near:mainnet": "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1",
    "near:testnet": "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af",
    "eip155:8453": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
    "eip155:84532": "0x036cbd53842c5426634e7929541ec2318f3dcf7e",
}
asset = assets[network]
assert catalog["x402Version"] == 2
assert catalog["pagination"]["limit"] == 100
assert catalog["pagination"]["offset"] == 0
assert catalog["pagination"]["total"] == len(catalog["items"])
for item in catalog["items"]:
    assert item["type"] == "http"
    assert item["x402Version"] == 2
    assert item["resource"].startswith("https://")
    assert item["accepts"]
    for accepted in item["accepts"]:
        assert accepted["scheme"] == "exact"
        assert accepted["network"] == network
        if network.startswith("eip155:"):
            assert accepted["asset"].lower() == asset
        else:
            assert accepted["asset"] == asset
llms = os.environ["LLMS"]
assert f"Network: {network}" in llms
assert "Facilitator fee: 0" in llms
assert "/discovery/resources" in llms
'

printf '%s' "$openapi" | python3 -c '
import sys

body = sys.stdin.read()
assert body.startswith("openapi: 3.1.0\n")
assert "  /openapi.yaml:\n" in body
assert "  /llms.txt:\n" in body
assert "  /discovery/resources:\n" in body
'

unauthenticated_status=$(
  curl --silent --output /dev/null --write-out '%{http_code}' \
    --max-time 15 \
    --header 'Content-Type: application/json' \
    --data '{}' \
    "$base_url/verify"
)
[ "$unauthenticated_status" = "401" ] || {
  echo "error: unauthenticated /verify returned $unauthenticated_status, expected 401" >&2
  exit 1
}

if [ -n "$key_file" ]; then
  [ -f "$key_file" ] || {
    echo "error: API key file not found: $key_file" >&2
    exit 1
  }
  python3 -c '
import os
import sys

if os.stat(sys.argv[1]).st_mode & 0o077:
    raise SystemExit(1)
' "$key_file" || {
    echo "error: API key file has group or world permissions" >&2
    exit 1
  }

  authenticated_status=$(
    {
      printf 'header = "X-API-Key: '
      tr -d '\r\n' <"$key_file"
      printf '"\n'
    } | curl --config - \
      --silent --output /dev/null --write-out '%{http_code}' \
      --max-time 15 \
      --header 'Content-Type: application/json' \
      --data '{}' \
      "$base_url/verify"
  )
  [ "$authenticated_status" = "400" ] || {
    echo "error: authenticated malformed /verify returned $authenticated_status, expected 400" >&2
    exit 1
  }
fi

echo "deployment smoke checks passed for $base_url"
