#!/bin/bash
# Publish operational metrics for the x402 NEAR facilitator host.
#
# Every run pushes to CloudWatch (region us-east-1, namespace x402near,
# colocated with the readyz alarms and the SNS alert topic):
#   - RelayerBalanceNear{Network=mainnet|testnet}: NEAR-instance relayer
#     account balance in NEAR, read from the same RPC endpoint the service uses.
#   - SignerBalanceEth{Network=<instance>}: eip155-instance signer native-gas
#     balance in ETH, read from the same RPC endpoint the service uses.
#   - CertDaysRemaining{Host=<lineage>}: days until each Let's Encrypt
#     certificate lineage expires (one datapoint per lineage).
#
# The companion CloudWatch alarms treat missing data as breaching, so a
# host, timer, or credential failure that stops these pushes raises the
# same alert as a low balance.
set -euo pipefail

readonly REGION=us-east-1
readonly NAMESPACE=x402near
readonly CONFIG_DIR=/etc/x402-near-facilitator
readonly CERT_LIVE_DIR=/etc/letsencrypt/live

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

relayer_balance_near() {
  local config=$1
  local account rpc response amount
  account=$(jq -r .relayer_account_id "$config")
  rpc=$(jq -r .primary_rpc_url "$config")
  response=$(curl -sS --fail --max-time 20 "$rpc" \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":"metrics","method":"query","params":{"request_type":"view_account","finality":"final","account_id":"'"$account"'"}}')
  amount=$(jq -er .result.amount <<<"$response")
  # yoctoNEAR -> NEAR; float precision loss is irrelevant at alert scale.
  awk -v y="$amount" 'BEGIN { printf "%.6f", y / 1e24 }'
}

signer_balance_eth() {
  local config=$1
  local account rpc response hex
  account=$(jq -r .relayer_account_id "$config")
  rpc=$(jq -r .primary_rpc_url "$config")
  response=$(curl -sS --fail --max-time 20 "$rpc" \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":"metrics","method":"eth_getBalance","params":["'"$account"'","latest"]}')
  hex=$(jq -er .result <<<"$response")
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
