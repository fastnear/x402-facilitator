#!/bin/bash
set -euo pipefail

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
metrics_script="$repo_root/deploy/monitoring/x402-near-metrics.sh"
metrics_unit="$repo_root/deploy/monitoring/x402-near-metrics.service"
canary_script="$repo_root/deploy/monitoring/x402-canary.sh"
retired_logrotate="$repo_root/deploy/logrotate/x402-near-facilitator"

fail() {
  echo "monitoring test failed: $*" >&2
  exit 1
}

bash -n "$metrics_script"
bash -n "$canary_script"
if grep -Eq '^[[:space:]]*OnFailure=' "$metrics_unit"; then
  fail "metrics unit must rely on debounced missing-data alarms"
fi
grep -Eq '^TimeoutStartSec=180$' "$metrics_unit" ||
  fail "metrics unit must outlive the bounded three-instance RPC fallback"
if [ -e "$retired_logrotate" ]; then
  fail "overlapping facilitator logrotate rule is still packaged"
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
config_dir="$work/config"
credential_dir="$work/credentials"
cert_dir="$work/certs"
mock_bin="$work/bin"
mkdir -p "$config_dir" "$credential_dir/base" "$cert_dir/x402.example" "$mock_bin"
: >"$cert_dir/x402.example/cert.pem"

printf '%s\n' \
  '{' \
  '  "chain_kind": "eip155",' \
  '  "relayer_account_id": "0x1111111111111111111111111111111111111111",' \
  '  "primary_rpc_url": "https://unused-base-primary.invalid",' \
  '  "backup_rpc_url": "https://unused-base-backup.invalid"' \
  '}' >"$config_dir/base.json"
printf '%s\n' \
  '{' \
  '  "chain_kind": "near",' \
  '  "relayer_account_id": "relayer.example.near",' \
  '  "primary_rpc_url": "https://near-primary.invalid/provider-credential-primary",' \
  '  "backup_rpc_url": "https://near-backup.invalid/provider-credential-backup"' \
  '}' >"$config_dir/mainnet.json"
printf '%s\n' \
  'https://base-primary.invalid/provider-credential-primary' \
  >"$credential_dir/base/primary-rpc-url"
printf '%s\n' \
  'https://base-backup.invalid/provider-credential-backup' \
  >"$credential_dir/base/backup-rpc-url"
chmod 0600 \
  "$credential_dir/base/primary-rpc-url" \
  "$credential_dir/base/backup-rpc-url"

printf '%s\n' \
  '#!/bin/bash' \
  'set -eu' \
  'url=' \
  'body=' \
  'config_source=' \
  'while [ "$#" -gt 0 ]; do' \
  '  case "$1" in' \
  '    http://* | https://*) url=$1 ;;' \
  '    --config)' \
  '      shift' \
  '      config_source=${1-}' \
  '      ;;' \
  '    -d | --data | --data-raw | --data-binary)' \
  '      shift' \
  '      body=${1-}' \
  '      ;;' \
  '  esac' \
  '  shift' \
  'done' \
  'if [ "$config_source" = "-" ]; then' \
  '  IFS= read -r config_line' \
  '  url=${config_line#url = \"}' \
  '  url=${url%\"}' \
  'fi' \
  '[ -n "$url" ] || exit 2' \
  'printf "%s\n" "$url" >>"$METRICS_CURL_CALLS"' \
  'case "$METRICS_SCENARIO:$url" in' \
  '  fail:*)' \
  '    printf "transport failure for %s\n" "$url" >&2' \
  '    exit 22' \
  '    ;;' \
  '  fallback:*base-primary*)' \
  '    printf "%s\n" '\''{"jsonrpc":"2.0","error":{"message":"provider-credential-primary"}}'\''' \
  '    exit 0' \
  '    ;;' \
  '  fallback:*near-primary*)' \
  '    printf "transport failure for %s\n" "$url" >&2' \
  '    exit 22' \
  '    ;;' \
  'esac' \
  'case "$body" in' \
  '  *eth_getBalance*) printf "%s\n" '\''{"jsonrpc":"2.0","result":"0xde0b6b3a7640000"}'\'' ;;' \
  '  *view_account*) printf "%s\n" '\''{"jsonrpc":"2.0","result":{"amount":"2000000000000000000000000"}}'\'' ;;' \
  '  *) exit 2 ;;' \
  'esac' >"$mock_bin/curl"

printf '%s\n' \
  '#!/bin/bash' \
  'set -eu' \
  'printf "%s\n" "$*" >>"$METRICS_AWS_CALLS"' >"$mock_bin/aws"

printf '%s\n' \
  '#!/bin/bash' \
  'printf "%s\n" "notAfter=Jan 1 00:00:00 2035 GMT"' >"$mock_bin/openssl"

printf '%s\n' \
  '#!/bin/bash' \
  'if [ "${1-}" = "-d" ]; then' \
  '  printf "%s\n" 2000000000' \
  'else' \
  '  exec /bin/date "$@"' \
  'fi' >"$mock_bin/date"

chmod 0755 "$mock_bin/curl" "$mock_bin/aws" "$mock_bin/openssl" "$mock_bin/date"

run_metrics() {
  local scenario=$1 stdout=$2 stderr=$3 curl_calls=$4 aws_calls=$5
  METRICS_SCENARIO="$scenario" \
    METRICS_CURL_CALLS="$curl_calls" \
    METRICS_AWS_CALLS="$aws_calls" \
    X402_METRICS_CONFIG_DIR="$config_dir" \
    X402_METRICS_CREDENTIAL_DIR="$credential_dir" \
    X402_METRICS_CERT_LIVE_DIR="$cert_dir" \
    PATH="$mock_bin:$PATH" \
    bash "$metrics_script" >"$stdout" 2>"$stderr"
}

fallback_stdout="$work/fallback.stdout"
fallback_stderr="$work/fallback.stderr"
fallback_curl="$work/fallback.curl"
fallback_aws="$work/fallback.aws"
: >"$fallback_curl"
: >"$fallback_aws"
run_metrics fallback "$fallback_stdout" "$fallback_stderr" \
  "$fallback_curl" "$fallback_aws"

grep -Fq 'SignerBalanceEth network=base balance=1.000000' "$fallback_stdout" ||
  fail "Base backup result was not published"
grep -Fq 'RelayerBalanceNear network=mainnet balance=2.000000' "$fallback_stdout" ||
  fail "NEAR backup result was not published"
grep -Fq -- '--metric-name SignerBalanceEth' "$fallback_aws" ||
  fail "Base signer metric was not sent to CloudWatch"
grep -Fq -- '--metric-name RelayerBalanceNear' "$fallback_aws" ||
  fail "NEAR relayer metric was not sent to CloudWatch"
grep -Fq 'base-primary.invalid' "$fallback_curl" ||
  fail "Base primary RPC was not attempted"
grep -Fq 'base-backup.invalid' "$fallback_curl" ||
  fail "Base backup RPC was not attempted"
if grep -Fq 'unused-base-' "$fallback_curl"; then
  fail "Base JSON fallback was used despite protected RPC credentials"
fi
grep -Fq 'near-primary.invalid' "$fallback_curl" ||
  fail "NEAR primary RPC was not attempted"
grep -Fq 'near-backup.invalid' "$fallback_curl" ||
  fail "NEAR backup RPC was not attempted"

if grep -Fq 'provider-credential-' "$fallback_stdout" "$fallback_stderr"; then
  fail "RPC URL or response detail escaped into metrics output"
fi

healthy_primary_stdout="$work/healthy-primary.stdout"
healthy_primary_stderr="$work/healthy-primary.stderr"
healthy_primary_curl="$work/healthy-primary.curl"
healthy_primary_aws="$work/healthy-primary.aws"
: >"$healthy_primary_curl"
: >"$healthy_primary_aws"
printf '%s\n' \
  'invalid-backup-provider-credential-must-not-escape' \
  >"$credential_dir/base/backup-rpc-url"
run_metrics success "$healthy_primary_stdout" "$healthy_primary_stderr" \
  "$healthy_primary_curl" "$healthy_primary_aws"
grep -Fq 'SignerBalanceEth network=base balance=1.000000' \
  "$healthy_primary_stdout" ||
  fail "invalid Base backup credential suppressed the healthy primary"
grep -Fq 'base-primary.invalid' "$healthy_primary_curl" ||
  fail "healthy Base primary was not attempted with an invalid backup credential"
if grep -Fq 'invalid-backup-provider-credential-must-not-escape' \
  "$healthy_primary_stdout" "$healthy_primary_stderr"; then
  fail "invalid backup credential escaped into metrics output"
fi

invalid_primary_stdout="$work/invalid-primary.stdout"
invalid_primary_stderr="$work/invalid-primary.stderr"
invalid_primary_curl="$work/invalid-primary.curl"
invalid_primary_aws="$work/invalid-primary.aws"
: >"$invalid_primary_curl"
: >"$invalid_primary_aws"
printf '%s\n' \
  'invalid-primary-provider-credential-must-not-escape' \
  >"$credential_dir/base/primary-rpc-url"
printf '%s\n' \
  'https://base-backup.invalid/provider-credential-backup' \
  >"$credential_dir/base/backup-rpc-url"
run_metrics success "$invalid_primary_stdout" "$invalid_primary_stderr" \
  "$invalid_primary_curl" "$invalid_primary_aws"
grep -Fq 'SignerBalanceEth network=base balance=1.000000' \
  "$invalid_primary_stdout" ||
  fail "invalid Base primary credential suppressed the healthy backup"
grep -Fq 'base-backup.invalid' "$invalid_primary_curl" ||
  fail "healthy Base backup was not attempted with an invalid primary credential"
if grep -Fq 'invalid-primary-provider-credential-must-not-escape' \
  "$invalid_primary_stdout" "$invalid_primary_stderr"; then
  fail "invalid primary credential escaped into metrics output"
fi

printf '%s\n' \
  'https://base-primary.invalid/provider-credential-primary' \
  >"$credential_dir/base/primary-rpc-url"

failed_stdout="$work/failed.stdout"
failed_stderr="$work/failed.stderr"
failed_curl="$work/failed.curl"
failed_aws="$work/failed.aws"
: >"$failed_curl"
: >"$failed_aws"
if run_metrics fail "$failed_stdout" "$failed_stderr" "$failed_curl" "$failed_aws"; then
  fail "all-RPC failure must leave the metric missing and exit nonzero"
fi
grep -Fq 'WARN: failed to read signer balance for base' "$failed_stderr" ||
  fail "Base failure did not emit its bounded instance warning"
grep -Fq 'WARN: failed to read relayer balance for mainnet' "$failed_stderr" ||
  fail "NEAR failure did not emit its bounded instance warning"
if grep -Fq -- '--metric-name SignerBalanceEth' "$failed_aws"; then
  fail "Base signer metric was fabricated after both RPCs failed"
fi
if grep -Fq -- '--metric-name RelayerBalanceNear' "$failed_aws"; then
  fail "NEAR relayer metric was fabricated after both RPCs failed"
fi
if grep -Fq 'provider-credential-' "$failed_stdout" "$failed_stderr"; then
  fail "failed RPC URL escaped into metrics output"
fi

# The merchant canary must stay a bounded, unpaid probe. Its mocked transport
# makes the happy path, dependency failure, and a policy-shaped secret
# deterministic without reaching public endpoints or signing a payment.
canary_fixture_dir="$work/canary-fixtures"
canary_credential_dir="$work/canary-credentials"
canary_bin="$work/canary-bin"
mkdir -p \
  "$canary_fixture_dir" \
  "$canary_credential_dir/mainnet" \
  "$canary_credential_dir/testnet" \
  "$canary_credential_dir/base" \
  "$canary_bin"
for network in mainnet testnet base; do
  printf '%s\n' '{}' >"$canary_fixture_dir/$network-verify.json"
  printf '%s\n' 'canary-api-key-must-not-escape' \
    >"$canary_credential_dir/$network/api-key"
done

canary_verify_near='{"isValid":false,"invalidReason":"invalid_exact_near_payload_signed_delegate_action"}'
canary_verify_base='{"isValid":false,"invalidReason":"insufficient_funds"}'
canary_demo_mainnet='{"accepts":[{"network":"near:mainnet"}]}'
canary_demo_testnet='{"accepts":[{"network":"near:testnet"}]}'
canary_demo_base='{"accepts":[{"network":"eip155:8453"}]}'
canary_ready_healthy='{"ready":true,"checks":{"rpc":"ready","facilitator":"ready","payment":"ready"}}'
canary_ready_string='{"ready":"true","checks":{"rpc":"ready","facilitator":"ready","payment":"ready"},"debug":"merchant-response-secret"}'
canary_ready_missing_payment='{"ready":true,"checks":{"rpc":"ready","facilitator":"ready"}}'
canary_near_policy='{"x402Version":2,"accepts":[{"scheme":"exact","network":"near:mainnet","asset":"17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1","payTo":"count.mike.near","amount":"1000","extra":{}}]}'
canary_base_policy='{"x402Version":2,"accepts":[{"scheme":"exact","network":"eip155:8453","asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913","payTo":"0x7Ff46ab88688D528bCE3e59c470240c6901cF88c","amount":"1000","extra":{"name":"USD Coin","version":"2"}}]}'
canary_base_policy_extra='{"x402Version":2,"accepts":[{"scheme":"exact","network":"eip155:8453","asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913","payTo":"0x7Ff46ab88688D528bCE3e59c470240c6901cF88c","amount":"1000","extra":{"name":"USD Coin","version":"2","unexpected":"merchant-response-secret"}}]}'
canary_base_policy_two_accepts=$(printf '%s' "$canary_base_policy" |
  jq -c '.accepts += [.accepts[0]]')
canary_demo_mainnet_header=$(printf '%s' "$canary_demo_mainnet" | base64 | tr -d '\n')
canary_demo_testnet_header=$(printf '%s' "$canary_demo_testnet" | base64 | tr -d '\n')
canary_demo_base_header=$(printf '%s' "$canary_demo_base" | base64 | tr -d '\n')
canary_near_policy_header=$(printf '%s' "$canary_near_policy" | base64 | tr -d '\n')
canary_base_policy_header=$(printf '%s' "$canary_base_policy" | base64 | tr -d '\n')
canary_base_policy_extra_header=$(printf '%s' "$canary_base_policy_extra" | base64 | tr -d '\n')
canary_base_policy_two_accepts_header=$(printf '%s' "$canary_base_policy_two_accepts" | base64 | tr -d '\n')
canary_discovery_empty='{"x402Version":2,"items":[],"pagination":{"limit":100,"offset":0,"total":0}}'
canary_discovery_bad='{"x402Version":2,"items":[{"resource":"https://merchant.example/work","type":"http","x402Version":2,"accepts":[{"scheme":"exact","network":"eip155:84532","asset":"0x036CbD53842c5426634e7929541eC2318f3dCF7c","amount":"1000","payTo":"0x1111111111111111111111111111111111111111","maxTimeoutSeconds":300,"extra":{"name":"USDC","version":"2"}}],"lastUpdated":"2026-07-31T12:00:00Z"}],"pagination":{"limit":100,"offset":0,"total":1}}'
canary_openapi=$(printf '%s\n' \
  'openapi: 3.1.0' \
  'paths:' \
  '  /openapi.yaml:' \
  '  /llms.txt:' \
  '  /discovery/resources:')
canary_llms_mainnet=$(printf '%s\n' \
  '- Network: near:mainnet' \
  '- Canonical asset: 17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1' \
  '- Facilitator fee: 0')
canary_llms_testnet=$(printf '%s\n' \
  '- Network: near:testnet' \
  '- Canonical asset: 3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af' \
  '- Facilitator fee: 0')
canary_llms_base=$(printf '%s\n' \
  '- Network: eip155:8453' \
  '- Canonical asset: 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913' \
  '- Facilitator fee: 0')

printf '%s\n' \
  '#!/bin/bash' \
  'set -eu' \
  '[ -n "${CANARY_CURL_CALLS:-}" ] && printf "%s\\n" "$*" >>"$CANARY_CURL_CALLS"' \
  'output=' \
  'headers=' \
  'url=' \
  'max_time=' \
  'connect_time=' \
  'while [ "$#" -gt 0 ]; do' \
  '  case "$1" in' \
  '    -o)' \
  '      output=${2-}' \
  '      shift 2' \
  '      ;;' \
  '    -D)' \
  '      headers=${2-}' \
  '      shift 2' \
  '      ;;' \
  '    -w)' \
  '      shift 2' \
  '      ;;' \
  '    --max-time)' \
  '      max_time=${2-}' \
  '      shift 2' \
  '      ;;' \
  '    --connect-timeout)' \
  '      connect_time=${2-}' \
  '      shift 2' \
  '      ;;' \
  '    --config)' \
  '      [ "${2-}" = "-" ] || exit 96' \
  '      IFS= read -r _config_line || exit 96' \
  '      [ "${_config_line#header = \"X-API-Key: }" != "$_config_line" ] || exit 96' \
  '      [ "${_config_line%\"}" != "$_config_line" ] || exit 96' \
  '      shift 2' \
  '      ;;' \
  '    -H)' \
  '      case ${2-} in' \
  '        PAYMENT-SIGNATURE:*) exit 97 ;;' \
  '      esac' \
  '      shift 2' \
  '      ;;' \
  '    -X | --data | --data-binary)' \
  '      shift 2' \
  '      ;;' \
  '    --get)' \
  '      shift' \
  '      ;;' \
  '    --data-urlencode)' \
  '      case ${2-} in' \
  '        network=*) url="${url}?network=${2#network=}&limit=100&offset=0" ;;' \
  '      esac' \
  '      shift 2' \
  '      ;;' \
  '    http://* | https://*)' \
  '      url=$1' \
  '      shift' \
  '      ;;' \
  '    *)' \
  '      shift' \
  '      ;;' \
  '  esac' \
  'done' \
  'write_body() {' \
  '  [ "$output" = /dev/null ] || printf "%s\\n" "$1" >"$output"' \
  '}' \
  'write_header() {' \
  '  [ -z "$headers" ] || printf "PAYMENT-REQUIRED: %s\\r\\n" "$1" >"$headers"' \
  '}' \
  'case "$url" in' \
  '  https://merchant-near.mikedotexe.com/* | https://merchant-base.mikedotexe.com/*)' \
  '    [ "$max_time" = 20 ] && [ "$connect_time" = 5 ] || exit 98' \
  '    ;;' \
  'esac' \
  'case "$url" in' \
  '  https://x402.mikedotexe.com/discovery/resources?network=near:mainnet\&limit=100\&offset=0 | https://test.x402.mikedotexe.com/discovery/resources?network=near:testnet\&limit=100\&offset=0)' \
  '    write_body "$CANARY_DISCOVERY_EMPTY"' \
  '    printf 200' \
  '    ;;' \
  '  https://base.x402.mikedotexe.com/discovery/resources?network=eip155:8453\&limit=100\&offset=0)' \
  '    if [ "${CANARY_SCENARIO:-success}" = base-discovery-bad ]; then' \
  '      write_body "$CANARY_DISCOVERY_BAD"' \
  '    else' \
  '      write_body "$CANARY_DISCOVERY_EMPTY"' \
  '    fi' \
  '    printf 200' \
  '    ;;' \
  '  https://x402.mikedotexe.com/llms.txt)' \
  '    write_body "$CANARY_LLMS_MAINNET"' \
  '    printf 200' \
  '    ;;' \
  '  https://test.x402.mikedotexe.com/llms.txt)' \
  '    write_body "$CANARY_LLMS_TESTNET"' \
  '    printf 200' \
  '    ;;' \
  '  https://base.x402.mikedotexe.com/llms.txt)' \
  '    write_body "$CANARY_LLMS_BASE"' \
  '    printf 200' \
  '    ;;' \
  '  https://x402.mikedotexe.com/openapi.yaml | https://test.x402.mikedotexe.com/openapi.yaml | https://base.x402.mikedotexe.com/openapi.yaml)' \
  '    write_body "$CANARY_OPENAPI"' \
  '    printf 200' \
  '    ;;' \
  '  https://x402.mikedotexe.com/verify | https://test.x402.mikedotexe.com/verify)' \
  '    write_body "$CANARY_VERIFY_NEAR"' \
  '    printf 200' \
  '    ;;' \
  '  https://base.x402.mikedotexe.com/verify)' \
  '    write_body "$CANARY_VERIFY_BASE"' \
  '    printf 200' \
  '    ;;' \
  '  https://x402-demo.mikedotexe.com/work)' \
  '    write_header "$CANARY_DEMO_MAINNET_HEADER"' \
  '    printf 402' \
  '    ;;' \
  '  https://x402-demo-test.mikedotexe.com/work)' \
  '    write_header "$CANARY_DEMO_TESTNET_HEADER"' \
  '    printf 402' \
  '    ;;' \
  '  https://x402-demo-base.mikedotexe.com/work)' \
  '    write_header "$CANARY_DEMO_BASE_HEADER"' \
  '    printf 402' \
  '    ;;' \
  '  https://merchant-near.mikedotexe.com/readyz)' \
  '    write_body "$CANARY_READY_HEALTHY"' \
  '    printf 200' \
  '    ;;' \
  '  https://merchant-base.mikedotexe.com/readyz)' \
  '    if [ "${CANARY_SCENARIO:-success}" = base-ready-string ]; then' \
  '      write_body "$CANARY_READY_STRING"' \
  '    elif [ "${CANARY_SCENARIO:-success}" = base-payment-missing ]; then' \
  '      write_body "$CANARY_READY_MISSING_PAYMENT"' \
  '    else' \
  '      write_body "$CANARY_READY_HEALTHY"' \
  '    fi' \
  '    printf 200' \
  '    ;;' \
  '  https://merchant-near.mikedotexe.com/v1/evidence/account)' \
  '    write_header "$CANARY_NEAR_POLICY_HEADER"' \
  '    printf 402' \
  '    ;;' \
  '  https://merchant-base.mikedotexe.com/v1/evidence/account)' \
  '    if [ "${CANARY_SCENARIO:-success}" = base-extra-field ]; then' \
  '      write_header "$CANARY_BASE_POLICY_EXTRA_HEADER"' \
  '    elif [ "${CANARY_SCENARIO:-success}" = base-two-accepts ]; then' \
  '      write_header "$CANARY_BASE_POLICY_TWO_ACCEPTS_HEADER"' \
  '    else' \
  '      write_header "$CANARY_BASE_POLICY_HEADER"' \
  '    fi' \
  '    printf 402' \
  '    ;;' \
  '  *)' \
  '    exit 2' \
  '    ;;' \
  'esac' >"$canary_bin/curl"
printf '%s\n' \
  '#!/bin/bash' \
  'set -eu' \
  'printf "%s\\n" "$*" >>"$CANARY_AWS_CALLS"' >"$canary_bin/aws"
chmod 0755 "$canary_bin/curl" "$canary_bin/aws"

run_canary() {
  local scenario=$1 stdout=$2 stderr=$3 aws_calls=$4 curl_calls=${5:-/dev/null}
  CANARY_SCENARIO="$scenario" \
    CANARY_AWS_CALLS="$aws_calls" \
    CANARY_CURL_CALLS="$curl_calls" \
    CANARY_VERIFY_NEAR="$canary_verify_near" \
    CANARY_VERIFY_BASE="$canary_verify_base" \
    CANARY_DEMO_MAINNET_HEADER="$canary_demo_mainnet_header" \
    CANARY_DEMO_TESTNET_HEADER="$canary_demo_testnet_header" \
    CANARY_DEMO_BASE_HEADER="$canary_demo_base_header" \
    CANARY_READY_HEALTHY="$canary_ready_healthy" \
    CANARY_READY_STRING="$canary_ready_string" \
    CANARY_READY_MISSING_PAYMENT="$canary_ready_missing_payment" \
    CANARY_NEAR_POLICY_HEADER="$canary_near_policy_header" \
    CANARY_BASE_POLICY_HEADER="$canary_base_policy_header" \
    CANARY_BASE_POLICY_EXTRA_HEADER="$canary_base_policy_extra_header" \
    CANARY_BASE_POLICY_TWO_ACCEPTS_HEADER="$canary_base_policy_two_accepts_header" \
    CANARY_DISCOVERY_EMPTY="$canary_discovery_empty" \
    CANARY_DISCOVERY_BAD="$canary_discovery_bad" \
    CANARY_OPENAPI="$canary_openapi" \
    CANARY_LLMS_MAINNET="$canary_llms_mainnet" \
    CANARY_LLMS_TESTNET="$canary_llms_testnet" \
    CANARY_LLMS_BASE="$canary_llms_base" \
    X402_CANARY_FIXTURE_DIR="$canary_fixture_dir" \
    X402_CANARY_CREDENTIAL_DIR="$canary_credential_dir" \
    PATH="$canary_bin:$PATH" \
    bash "$canary_script" >"$stdout" 2>"$stderr"
}

canary_success_stdout="$work/canary-success.stdout"
canary_success_stderr="$work/canary-success.stderr"
canary_success_aws="$work/canary-success.aws"
canary_success_curl="$work/canary-success.curl"
: >"$canary_success_aws"
: >"$canary_success_curl"
run_canary success "$canary_success_stdout" "$canary_success_stderr" \
  "$canary_success_aws" "$canary_success_curl"
grep -Fq 'MerchantApiOk network=mainnet value=1' "$canary_success_stdout" ||
  fail "healthy NEAR merchant policy did not publish success"
grep -Fq 'MerchantApiOk network=base value=1' "$canary_success_stdout" ||
  fail "healthy Base merchant policy and EIP-712 domain did not publish success"
grep -Fq -- '--metric-name MerchantApiOk' "$canary_success_aws" ||
  fail "merchant success did not publish its CloudWatch metric"
grep -Fq 'FacilitatorDiscoveryOk network=mainnet value=1' "$canary_success_stdout" ||
  fail "healthy NEAR mainnet discovery did not publish success"
grep -Fq 'FacilitatorDiscoveryOk network=testnet value=1' "$canary_success_stdout" ||
  fail "healthy NEAR testnet discovery did not publish success"
grep -Fq 'FacilitatorDiscoveryOk network=base value=1' "$canary_success_stdout" ||
  fail "healthy Base discovery did not publish success"
grep -Fq -- '--metric-name FacilitatorDiscoveryOk' "$canary_success_aws" ||
  fail "discovery success did not publish its CloudWatch metric"
grep -Fq -- '--config -' "$canary_success_curl" ||
  fail "verify canary did not pass credentials through stdin curl configuration"
if grep -Fq 'canary-api-key-must-not-escape' \
  "$canary_success_curl" "$canary_success_stdout" \
  "$canary_success_stderr" "$canary_success_aws"; then
  fail "credential detail escaped curl arguments, canary output, or metric logs"
fi

canary_ready_failure_stdout="$work/canary-ready-failure.stdout"
canary_ready_failure_stderr="$work/canary-ready-failure.stderr"
canary_ready_failure_aws="$work/canary-ready-failure.aws"
: >"$canary_ready_failure_aws"
if run_canary base-ready-string "$canary_ready_failure_stdout" \
  "$canary_ready_failure_stderr" "$canary_ready_failure_aws"; then
  fail "string readiness must not satisfy the merchant readiness gate"
fi
grep -Fq 'MerchantApiOk network=mainnet value=1' "$canary_ready_failure_stdout" ||
  fail "a Base readiness failure stopped the independent NEAR merchant probe"
grep -Fq 'MerchantApiOk network=base value=0' "$canary_ready_failure_stdout" ||
  fail "invalid Base readiness did not publish MerchantApiOk=0"
grep -Fq 'readiness check failed (status=200)' "$canary_ready_failure_stderr" ||
  fail "Base readiness failure was not classified safely"

canary_payment_failure_stdout="$work/canary-payment-failure.stdout"
canary_payment_failure_stderr="$work/canary-payment-failure.stderr"
canary_payment_failure_aws="$work/canary-payment-failure.aws"
: >"$canary_payment_failure_aws"
if run_canary base-payment-missing "$canary_payment_failure_stdout" \
  "$canary_payment_failure_stderr" "$canary_payment_failure_aws"; then
  fail "missing payment initialization must not satisfy merchant readiness"
fi
grep -Fq 'MerchantApiOk network=base value=0' "$canary_payment_failure_stdout" ||
  fail "missing payment initialization did not publish MerchantApiOk=0"

canary_policy_failure_stdout="$work/canary-policy-failure.stdout"
canary_policy_failure_stderr="$work/canary-policy-failure.stderr"
canary_policy_failure_aws="$work/canary-policy-failure.aws"
: >"$canary_policy_failure_aws"
if run_canary base-extra-field "$canary_policy_failure_stdout" \
  "$canary_policy_failure_stderr" "$canary_policy_failure_aws"; then
  fail "Base EIP-712 domain with an unexpected field must fail closed"
fi
grep -Fq 'MerchantApiOk network=base value=0' "$canary_policy_failure_stdout" ||
  fail "invalid Base payment policy did not publish MerchantApiOk=0"
grep -Fq 'unexpected unpaid challenge policy (status=402)' \
  "$canary_policy_failure_stderr" ||
  fail "Base payment-policy failure was not classified safely"

canary_two_accepts_stdout="$work/canary-two-accepts.stdout"
canary_two_accepts_stderr="$work/canary-two-accepts.stderr"
canary_two_accepts_aws="$work/canary-two-accepts.aws"
: >"$canary_two_accepts_aws"
if run_canary base-two-accepts "$canary_two_accepts_stdout" \
  "$canary_two_accepts_stderr" "$canary_two_accepts_aws"; then
  fail "a second x402 acceptance must fail the canonical merchant policy gate"
fi
grep -Fq 'MerchantApiOk network=base value=0' "$canary_two_accepts_stdout" ||
  fail "multiple Base acceptances did not publish MerchantApiOk=0"
if grep -Fq 'merchant-response-secret\|canary-api-key-must-not-escape' \
  "$canary_ready_failure_stdout" "$canary_ready_failure_stderr" \
  "$canary_policy_failure_stdout" "$canary_policy_failure_stderr"; then
  fail "merchant response or credential detail escaped canary output"
fi

canary_discovery_failure_stdout="$work/canary-discovery-failure.stdout"
canary_discovery_failure_stderr="$work/canary-discovery-failure.stderr"
canary_discovery_failure_aws="$work/canary-discovery-failure.aws"
: >"$canary_discovery_failure_aws"
if run_canary base-discovery-bad "$canary_discovery_failure_stdout" \
  "$canary_discovery_failure_stderr" "$canary_discovery_failure_aws"; then
  fail "wrong-network Base discovery metadata must fail closed"
fi
grep -Fq 'FacilitatorDiscoveryOk network=mainnet value=1' \
  "$canary_discovery_failure_stdout" ||
  fail "a Base discovery failure stopped the independent mainnet probe"
grep -Fq 'FacilitatorDiscoveryOk network=testnet value=1' \
  "$canary_discovery_failure_stdout" ||
  fail "a Base discovery failure stopped the independent testnet probe"
grep -Fq 'FacilitatorDiscoveryOk network=base value=0' \
  "$canary_discovery_failure_stdout" ||
  fail "wrong-network Base discovery did not publish FacilitatorDiscoveryOk=0"
grep -Fq 'public discovery contract check failed' \
  "$canary_discovery_failure_stderr" ||
  fail "Base discovery failure was not classified safely"

echo "monitoring RPC fallback, redaction, dead-man, merchant, and discovery canary checks passed"
