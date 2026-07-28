#!/bin/bash
set -euo pipefail

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
metrics_script="$repo_root/deploy/monitoring/x402-near-metrics.sh"
metrics_unit="$repo_root/deploy/monitoring/x402-near-metrics.service"
retired_logrotate="$repo_root/deploy/logrotate/x402-near-facilitator"

fail() {
  echo "monitoring test failed: $*" >&2
  exit 1
}

bash -n "$metrics_script"
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

echo "monitoring RPC fallback, redaction, dead-man, and packaging checks passed"
