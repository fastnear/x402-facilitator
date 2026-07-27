#!/bin/bash
# Synthetic end-to-end canaries for the x402 facilitator fleet and its demo
# resource servers. Complements x402-near-metrics.sh (balances, certificates)
# and the Route53 /readyz checks, which probe reported readiness rather than
# the request path a paying client actually exercises.
#
# Every run pushes to CloudWatch (region us-east-1, namespace x402near):
#   - VerifyCanaryOk{Network=mainnet|testnet|base}: 1 when an authenticated
#     POST /verify with the stored deterministic fixture returns HTTP 200 with
#     the expected definitive rejection, else 0. The fixtures are intentionally
#     unsettleable (see deploy/monitoring/README.md) so the probe proves the
#     full parse -> chain-mechanism -> RPC path without moving funds:
#       * NEAR instances: a syntactically valid envelope whose
#         signedDelegateAction is not Borsh -> the chain mechanism rejects it
#         (invalid_exact_near_payload_signed_delegate_action).
#       * Base: a validly signed ERC-3009 authorization from an unfunded
#         throwaway key -> upstream reads the payer balance over RPC and
#         rejects definitively (insufficient_funds).
#   - DemoWorkOk{Network=mainnet|testnet|base}: 1 when an unpaid POST /work
#     against the demo returns HTTP 402 whose PAYMENT-REQUIRED header decodes
#     to the expected accepts[0].network, else 0.
#
# The companion alarms treat missing data as breaching, so a host, timer, or
# credential failure that stops these pushes raises the same alert as a
# failing canary.
set -euo pipefail

readonly REGION=us-east-1
readonly NAMESPACE=x402near
readonly FIXTURE_DIR=/etc/x402-canary
readonly CREDENTIAL_DIR=/etc/x402-demo/credentials
readonly CURL_MAX_TIME=20

fail=0

publish() {
  local metric=$1 network=$2 value=$3
  aws cloudwatch put-metric-data \
    --region "$REGION" \
    --namespace "$NAMESPACE" \
    --metric-name "$metric" \
    --dimensions "Network=$network" \
    --value "$value" \
    --unit None
}

# POST the stored fixture to the facilitator's /verify with the demo's API key
# and require the exact deterministic rejection. Anything else (5xx, timeout,
# auth failure, unexpected reason) fails the canary. The API key is read into
# the curl argument list only; it is never echoed.
verify_canary() {
  local network=$1 base_url=$2 expected_reason=$3
  local fixture="$FIXTURE_DIR/$network-verify.json"
  local key_file="$CREDENTIAL_DIR/$network/api-key"
  local body status
  if [ ! -r "$fixture" ] || [ ! -r "$key_file" ]; then
    echo "WARN: verify canary $network: missing fixture or credential" >&2
    return 1
  fi
  body=$(mktemp)
  status=$(curl -sS -o "$body" -w "%{http_code}" --max-time "$CURL_MAX_TIME" \
    -X POST "$base_url/verify" \
    -H "Content-Type: application/json" \
    -H "X-API-Key: $(tr -d '\n' <"$key_file")" \
    --data-binary @"$fixture" || echo "000")
  local reason is_valid
  is_valid=$(jq -r '.isValid' "$body" 2>/dev/null || echo parse-error)
  reason=$(jq -r '.invalidReason' "$body" 2>/dev/null || echo parse-error)
  rm -f "$body"
  if [ "$status" = "200" ] && [ "$is_valid" = "false" ] && [ "$reason" = "$expected_reason" ]; then
    return 0
  fi
  echo "WARN: verify canary $network: status=$status isValid=$is_valid reason=$reason (expected $expected_reason)" >&2
  return 1
}

# Unpaid POST /work must produce a 402 whose PAYMENT-REQUIRED header decodes
# to the demo's network. This proves nginx, the Node app, and the x402
# middleware end to end (the Route53 checks only reach static files).
demo_canary() {
  local network=$1 url=$2 expected_network=$3
  local headers decoded got status
  headers=$(mktemp)
  status=$(curl -sS -o /dev/null -D "$headers" -w "%{http_code}" \
    --max-time "$CURL_MAX_TIME" \
    -X POST "$url/work" \
    -H "Content-Type: application/json" \
    --data '{"canary":"x402-canary"}' || echo "000")
  decoded=$(awk 'tolower($1) == "payment-required:" {print $2}' "$headers" | tr -d '\r' | base64 -d 2>/dev/null || true)
  rm -f "$headers"
  got=$(jq -r '.accepts[0].network' <<<"$decoded" 2>/dev/null || echo parse-error)
  if [ "$status" = "402" ] && [ "$got" = "$expected_network" ]; then
    return 0
  fi
  echo "WARN: demo canary $network: status=$status accepts[0].network=$got (expected $expected_network)" >&2
  return 1
}

run_verify() {
  local network=$1 base_url=$2 expected_reason=$3
  local ok=1
  if verify_canary "$network" "$base_url" "$expected_reason"; then ok=1; else
    ok=0
    fail=1
  fi
  publish VerifyCanaryOk "$network" "$ok"
  echo "VerifyCanaryOk network=$network value=$ok"
}

run_demo() {
  local network=$1 url=$2 expected_network=$3
  local ok=1
  if demo_canary "$network" "$url" "$expected_network"; then ok=1; else
    ok=0
    fail=1
  fi
  publish DemoWorkOk "$network" "$ok"
  echo "DemoWorkOk network=$network value=$ok"
}

run_verify mainnet https://x402.mikedotexe.com invalid_exact_near_payload_signed_delegate_action
run_verify testnet https://test.x402.mikedotexe.com invalid_exact_near_payload_signed_delegate_action
run_verify base https://base.x402.mikedotexe.com insufficient_funds

run_demo mainnet https://x402-demo.mikedotexe.com near:mainnet
run_demo testnet https://x402-demo-test.mikedotexe.com near:testnet
run_demo base https://x402-demo-base.mikedotexe.com eip155:8453

exit "$fail"
