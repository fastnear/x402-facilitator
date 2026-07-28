#!/bin/bash
# Publish operational metrics for the x402 NEAR facilitator host.
#
# Every run pushes to CloudWatch (region us-east-1, namespace x402near,
# colocated with the readyz alarms and the SNS alert topic):
#   - RelayerBalanceNear{Network=mainnet|testnet}: NEAR-instance relayer
#     account balance in NEAR, read through the same primary/backup RPC pair
#     the service uses.
#   - SignerBalanceEth{Network=<instance>}: eip155-instance signer native-gas
#     balance in ETH, read through the same primary/backup RPC pair the service
#     uses.
#   - CertDaysRemaining{Host=<lineage>}: days until each Let's Encrypt
#     certificate lineage expires (one datapoint per lineage).
#
# The companion CloudWatch alarms treat missing data as breaching, so a
# host, timer, or credential failure that stops these pushes raises the
# same alert as a low balance.
set -euo pipefail

readonly REGION=us-east-1
readonly NAMESPACE=x402near
readonly CONFIG_DIR=${X402_METRICS_CONFIG_DIR:-/etc/x402-near-facilitator}
readonly CREDENTIAL_DIR=${X402_METRICS_CREDENTIAL_DIR:-/etc/x402-near-facilitator/credentials}
readonly CERT_LIVE_DIR=${X402_METRICS_CERT_LIVE_DIR:-/etc/letsencrypt/live}

fail=0

publish() {
  # put-metric-data takes shorthand key=value dimensions (e.g.
  # "Network=mainnet"), unlike put-metric-alarm's Name=/Value= structure.
  local metric=$1 dimensions=$2 value=$3
  aws cloudwatch put-metric-data \
    --region "$REGION" \
    --namespace "$NAMESPACE" \
    --metric-name "$metric" \
    --dimensions "$dimensions" \
    --value "$value" \
    --unit None
}

read_rpc_credential() {
  local path=$1 value
  [ -r "$path" ] || return 1
  [ "$(wc -c <"$path")" -le 65536 ] || return 1
  [ "$(wc -l <"$path")" -eq 1 ] || return 1
  IFS= read -r value <"$path" || return 1
  value=${value%$'\r'}
  case "$value" in
    https://*) ;;
    *) return 1 ;;
  esac
  case "$value" in
    *[[:space:]]*) return 1 ;;
  esac
  printf '%s' "$value"
}

rpc_url() {
  local config=$1 role=$2
  local instance credential
  instance=$(basename "$config" .json)
  credential="$CREDENTIAL_DIR/$instance/$role-rpc-url"
  if [ -e "$credential" ]; then
    read_rpc_credential "$credential"
  else
    jq -er \
      ".${role}_rpc_url | select(type == \"string\" and startswith(\"https://\") and (test(\"[[:space:]]\") | not))" \
      "$config" 2>/dev/null
  fi
}

rpc_result_with_fallback() {
  local config=$1 request=$2 result_filter=$3
  local role rpc curl_url response result

  # One bounded attempt per independent endpoint is enough for a five-minute
  # metric. Pass the URL through curl's stdin config rather than argv, and
  # suppress curl/jq diagnostics, because provider URLs may contain credentials.
  # Resolve each role independently: a malformed or unreadable credential for
  # one endpoint must not suppress a healthy endpoint in the other role. The
  # caller emits only the bounded instance name when neither yields a result.
  for role in primary backup; do
    if ! rpc=$(rpc_url "$config" "$role"); then
      continue
    fi
    curl_url=${rpc//\\/\\\\}
    curl_url=${curl_url//\"/\\\"}
    if response=$(
      printf 'url = "%s"\n' "$curl_url" |
        curl -sS --fail --max-time 20 --config - \
          -H 'Content-Type: application/json' \
          -d "$request" 2>/dev/null
    ) &&
      result=$(jq -er "$result_filter" <<<"$response" 2>/dev/null); then
      printf '%s' "$result"
      return 0
    fi
  done
  return 1
}

relayer_balance_near() {
  local config=$1
  local account request amount
  account=$(jq -er \
    '.relayer_account_id | select(type == "string" and length > 0)' \
    "$config" 2>/dev/null) || return 1
  request=$(jq -cn --arg account "$account" \
    '{jsonrpc:"2.0",id:"metrics",method:"query",params:{request_type:"view_account",finality:"final",account_id:$account}}') ||
    return 1
  amount=$(rpc_result_with_fallback "$config" "$request" \
    '.result.amount | select(type == "string" and test("^[0-9]+$"))') ||
    return 1
  # yoctoNEAR -> NEAR; float precision loss is irrelevant at alert scale.
  awk -v y="$amount" 'BEGIN { printf "%.6f", y / 1e24 }'
}

signer_balance_eth() {
  local config=$1
  local account request hex
  account=$(jq -er \
    '.relayer_account_id | select(type == "string" and length > 0)' \
    "$config" 2>/dev/null) || return 1
  request=$(jq -cn --arg account "$account" \
    '{jsonrpc:"2.0",id:"metrics",method:"eth_getBalance",params:[$account,"latest"]}') ||
    return 1
  hex=$(rpc_result_with_fallback "$config" "$request" \
    '.result | select(type == "string" and test("^0x[0-9a-fA-F]+$"))') ||
    return 1
  # eth_getBalance returns 0x-hex wei. Parse it in awk so the conversion needs
  # no gawk/bc/python: awk doubles carry every magnitude without the 64-bit
  # overflow bash arithmetic hits above ~9.2 ETH, and sub-wei rounding is
  # irrelevant at the sub-0.1-ETH alert scale.
  awk -v h="${hex#0x}" 'BEGIN {
    n = 0
    h = tolower(h)
    if (length(h) == 0) { exit 2 }
    for (i = 1; i <= length(h); i++) {
      d = index("0123456789abcdef", substr(h, i, 1)) - 1
      if (d < 0) { exit 2 }
      n = n * 16 + d
    }
    printf "%.6f", n / 1e18
  }'
}

# Iterate the installed instance configs rather than a fixed mainnet/testnet
# list, so a new instance is covered as soon as its config lands. The balance
# metric is chain-specific: NEAR instances publish RelayerBalanceNear; the EVM
# (eip155) signer-balance metric arrives with that provider.
found_config=0
for config in "$CONFIG_DIR"/*.json; do
  [ -e "$config" ] || continue
  found_config=1
  network=$(basename "$config" .json)
  chain_kind=$(jq -r '.chain_kind // "near"' "$config")
  case "$chain_kind" in
    near)
      if balance=$(relayer_balance_near "$config"); then
        publish RelayerBalanceNear "Network=$network" "$balance"
        echo "RelayerBalanceNear network=$network balance=$balance"
      else
        echo "WARN: failed to read relayer balance for $network" >&2
        fail=1
      fi
      ;;
    eip155)
      if balance=$(signer_balance_eth "$config"); then
        publish SignerBalanceEth "Network=$network" "$balance"
        echo "SignerBalanceEth network=$network balance=$balance"
      else
        echo "WARN: failed to read signer balance for $network" >&2
        fail=1
      fi
      ;;
    *)
      # A chain_kind with no balance metric yet: warn but do not fail, so a
      # future instance can land its config before its metric ships.
      echo "WARN: no balance metric for chain_kind=$chain_kind ($network); skipping" >&2
      ;;
  esac
done
if [ "$found_config" -eq 0 ]; then
  echo "WARN: no instance configs found under $CONFIG_DIR" >&2
  fail=1
fi

found_cert=0
for cert in "$CERT_LIVE_DIR"/*/cert.pem; do
  [ -e "$cert" ] || continue
  found_cert=1
  host=$(basename "$(dirname "$cert")")
  if end_date=$(openssl x509 -enddate -noout -in "$cert" 2>/dev/null); then
    end_epoch=$(date -d "${end_date#notAfter=}" +%s)
    days=$(( (end_epoch - $(date +%s)) / 86400 ))
    publish CertDaysRemaining "Host=$host" "$days"
    echo "CertDaysRemaining host=$host days=$days"
  else
    echo "WARN: failed to read certificate expiry for $host" >&2
    fail=1
  fi
done
if [ "$found_cert" -eq 0 ]; then
  echo "WARN: no certificate lineages found under $CERT_LIVE_DIR" >&2
  fail=1
fi

exit "$fail"
