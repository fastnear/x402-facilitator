# Host monitoring and alerting assets

Version-controlled copies of the operational monitoring assets installed on
the facilitator host. These are operator-installed host assets; they are not
part of the release archive and changing them never requires a new release.

All metrics and alarms live in CloudWatch region `us-east-1`, namespace
`x402near`, colocated with the `/readyz` health-check alarms and the SNS
topic `arn:aws:sns:us-east-1:341982967115:x402-facilitator-alerts`.

## What runs where

| Asset | Installed at | Purpose |
| --- | --- | --- |
| `x402-near-metrics.sh` | `/usr/local/bin/` | Push relayer/signer balances + per-lineage cert expiry every 5 minutes |
| `x402-near-metrics.{service,timer}` | `/etc/systemd/system/` | Drive the metrics push |
| `x402-near-backup.sh` | `/usr/local/bin/` | Nightly dumps, S3 push, `BackupSuccess` signal |
| `x402-near-backup.{service,timer}` | `/etc/systemd/system/` | Drive the nightly backup |
| `x402-near-alert.sh` | `/usr/local/bin/` | Publish a unit failure to SNS |
| `x402-near-alert@.service` | `/etc/systemd/system/` | `OnFailure=` target for units that need immediate notification |
| `certbot-onfailure.conf` | `/etc/systemd/system/certbot.service.d/x402-near-onfailure.conf` | Alert on failed certificate renewal |
| `x402-canary.sh` | `/usr/local/bin/` | Synthetic `/verify`, discovery, demo `/work`, and unpaid merchant canaries every 5 minutes |
| `x402-canary.{service,timer}` | `/etc/systemd/system/` | Drive the canaries (offset 2 minutes from the metrics timer) |
| `x402-ebs-snapshot.sh` | `/usr/local/bin/` | Daily EBS snapshot + prune (installed but disabled until the IAM grant below) |
| `x402-ebs-snapshot.{service,timer}` | `/etc/systemd/system/` | Drive the snapshot |

Install scripts mode 0755 root-owned, units 0644 root-owned, then
`systemctl daemon-reload` and `systemctl enable --now` the timers.

The metrics script makes one bounded request to the effective primary RPC and,
if that response is unavailable or malformed, one to the independent backup.
When an instance has `primary-rpc-url` or `backup-rpc-url` credential files, the
script reads the same files that override the service's public JSON fallbacks.
It passes each URL to curl through stdin and never prints the endpoint or RPC
response because provider URLs may contain credentials. The metrics service
deliberately has no immediate `OnFailure=` notification: balance alarms require
three consecutive missing five-minute datapoints, so one provider interval does
not send an email while a persistent failure still trips the existing dead-man
alarm. Its explicit three-minute unit deadline also accommodates both bounded
RPC attempts for all three currently installed instances during a broad
provider outage.
Backup, canary, snapshot, and certificate-renewal failures retain their
immediate hooks.

## Nginx log rotation

Ubuntu's packaged `/etc/logrotate.d/nginx` rule owns
`/var/log/nginx/*.log`, including the facilitator, demo, and merchant virtual
hosts. Do not install an overlapping per-service rule: logrotate treats a file
matched by two stanzas as a configuration error and may skip the entire run.

For a host that still has the retired
`/etc/logrotate.d/x402-near-facilitator` rule, first confirm that the packaged
Nginx rule contains the wildcard. Move the retired file out of
`/etc/logrotate.d` so it remains recoverable, then validate the complete
configuration without rotating files:

```sh
sudo grep -F '/var/log/nginx/*.log' /etc/logrotate.d/nginx
sudo install -d -m 0700 /root/x402-retired-config
sudo mv /etc/logrotate.d/x402-near-facilitator \
  /root/x402-retired-config/x402-near-facilitator.logrotate
sudo logrotate --debug /etc/logrotate.conf
```

## Canary fixtures

`x402-canary.sh` reads one request body per instance from
`/etc/x402-canary/<instance>-verify.json` (root-owned, not in this
repository because they embed deployment-specific requirements). Each
fixture is deterministic, unsettleable, and never expires:

- NEAR instances: a canonical v2 envelope whose `signedDelegateAction` is
  valid base64 but not Borsh. The chain mechanism rejects it with
  `invalid_exact_near_payload_signed_delegate_action` after the full service
  parse, so the probe exercises auth, strict parsing, and the NEAR scheme
  gate without touching the chain.
- Base: an ERC-3009 authorization validly signed by a throwaway key that
  holds no funds (`validBefore` far in the future). Upstream verification
  reads the payer's USDC balance over RPC and rejects definitively with
  `insufficient_funds`, so the probe exercises the live RPC path. The
  throwaway private key is discarded at generation time; the fixture is not
  sensitive because the authorization can never move funds.

To regenerate, follow the fixture shapes above against the live 402 of each
demo (`accepts[0]` supplies the exact requirements object). Do not use an
invalid signature for the Base fixture: the upstream x402-chain-eip155 crate
maps the resulting simulation revert to `OnchainFailure`, which this service
classifies as ambiguous and answers with 503 `rpc_unavailable` rather than a
definitive rejection (tracked as a known defect).

The API keys come from the demo credentials already on the host
(`/etc/x402-demo/credentials/<instance>/api-key`); the canary introduces no
new secrets. The two merchant checks send valid account-evidence request
bodies without a payment header. They first require `/readyz` to confirm the
configured chain RPC, facilitator dependency, and x402 payment-server
initialization, then require one canonical v2 `exact` acceptance with the exact
production network, asset, payee, and 1,000-atomic-unit amount. NEAR must
carry an empty `extra` object; the Base acceptance must carry only Circle
USDC's `USD Coin`/`2` domain. They do not sign an authorization, invoke
application work, or move funds.

The three facilitator discovery checks are fully public and secret-free. Each
requires `/llms.txt`, `/openapi.yaml`, and a network-filtered
`/discovery/resources` response to agree on the instance network and canonical
asset. Every returned item must remain x402 v2 HTTP metadata for that profile;
an empty catalog is explicitly healthy and makes no activity or volume claim.

## Instance-role policy

The host authenticates with the `x402-near-backup-role` instance role
(IMDSv2 temporary credentials, no static key). Inline policy
`s3-backup-put`, least-privilege:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": "s3:PutObject",
      "Resource": "arn:aws:s3:::x402-near-backups-341982967115/dumps/*"
    },
    {
      "Effect": "Allow",
      "Action": "cloudwatch:PutMetricData",
      "Resource": "*",
      "Condition": { "StringEquals": { "cloudwatch:namespace": "x402near" } }
    },
    {
      "Effect": "Allow",
      "Action": "sns:Publish",
      "Resource": "arn:aws:sns:us-east-1:341982967115:x402-facilitator-alerts"
    }
  ]
}
```

The host can write dumps, metrics in the `x402near` namespace, and alert
messages to the one topic — nothing else: no list, read, delete, or any
other namespace/topic.

Enabling `x402-ebs-snapshot.timer` additionally requires this statement on
the instance role (until it is granted, the unit stays installed but
disabled and the volume is covered by manual snapshots only):

```json
{
  "Effect": "Allow",
  "Action": [
    "ec2:CreateSnapshot",
    "ec2:CreateTags",
    "ec2:DescribeSnapshots",
    "ec2:DeleteSnapshot"
  ],
  "Resource": "*"
}
```

## Alarms

All alarms notify the SNS topic on both ALARM and OK. `TreatMissingData:
breaching` makes every alarm double as a dead-man switch: a stopped timer,
broken credential, or dead host raises the same alert as the condition
itself.

| Alarm | Metric | Threshold | Periods |
| --- | --- | --- | --- |
| `x402-mainnet-relayer-balance-low` | `RelayerBalanceNear{Network=mainnet}` | `< 2` NEAR | 3 × 5 min |
| `x402-testnet-relayer-balance-low` | `RelayerBalanceNear{Network=testnet}` | `< 3` NEAR | 3 × 5 min |
| `x402-base-signer-balance-low` | `SignerBalanceEth{Network=base}` | `< 0.005` ETH | 3 × 5 min |
| `x402-cert-expiry-soon` | `CertDaysRemaining{Host=x402.mikedotexe.com}` | `< 21` days | 3 × 5 min |
| `x402-demo-cert-expiry-soon` | `CertDaysRemaining{Host=x402-demo.mikedotexe.com}` | `< 21` days | 3 × 5 min |
| `x402-demo-base-cert-expiry-soon` | `CertDaysRemaining{Host=x402-demo-base.mikedotexe.com}` | `< 21` days | 1 × 5 min |
| `x402-backup-missing` | `BackupSuccess` (Sum, 1-day period) | `< 1` | 1 × 1 day |
| `x402-<inst>-verify-canary-failing` | `VerifyCanaryOk{Network=<inst>}` | `< 1` | 2 of 3 × 5 min |
| `x402-demo-<inst>-work-canary-failing` | `DemoWorkOk{Network=<inst>}` | `< 1` | 2 of 3 × 5 min |
| `x402-merchant-<inst>-api-canary-failing` | `MerchantApiOk{Network=<inst>}` | `< 1` | 2 of 3 × 5 min |
| `x402-<inst>-discovery-canary-failing` | `FacilitatorDiscoveryOk{Network=<inst>}` | `< 1` | 2 of 3 × 5 min |
| `x402-<inst>-readyz-flapping` | `HealthCheckPercentageHealthy` (Average, 1 h) | `< 90` % | 1 × 1 h |

Install or refresh the two merchant alarms after the updated canary is active:

```sh
for network in mainnet base; do
  aws cloudwatch put-metric-alarm \
    --region us-east-1 \
    --alarm-name "x402-merchant-${network}-api-canary-failing" \
    --namespace x402near \
    --metric-name MerchantApiOk \
    --dimensions "Name=Network,Value=${network}" \
    --statistic Minimum \
    --period 300 \
    --evaluation-periods 3 \
    --datapoints-to-alarm 2 \
    --threshold 1 \
    --comparison-operator LessThanThreshold \
    --treat-missing-data breaching \
    --alarm-actions arn:aws:sns:us-east-1:341982967115:x402-facilitator-alerts \
    --ok-actions arn:aws:sns:us-east-1:341982967115:x402-facilitator-alerts
done
```

Install or refresh the three public discovery alarms after the updated canary
is active:

```sh
for network in mainnet testnet base; do
  aws cloudwatch put-metric-alarm \
    --region us-east-1 \
    --alarm-name "x402-${network}-discovery-canary-failing" \
    --namespace x402near \
    --metric-name FacilitatorDiscoveryOk \
    --dimensions "Name=Network,Value=${network}" \
    --statistic Minimum \
    --period 300 \
    --evaluation-periods 3 \
    --datapoints-to-alarm 2 \
    --threshold 1 \
    --comparison-operator LessThanThreshold \
    --treat-missing-data breaching \
    --alarm-actions arn:aws:sns:us-east-1:341982967115:x402-facilitator-alerts \
    --ok-actions arn:aws:sns:us-east-1:341982967115:x402-facilitator-alerts
done
```

The metrics script emits one `CertDaysRemaining` datapoint per Let's
Encrypt lineage; when a new lineage is issued (for example a demo
workload's hostnames), create a matching alarm on its `Host` dimension.

The readyz-flapping alarms exist because the Route53 `HealthCheckStatus`
alarms only fire on sustained failure (3 consecutive per checker):
2026-07-27 showed thousands of intermittent `/readyz` 503s per day on the
base instance (free-tier RPC flakiness) without a single status alarm.
Flapping alarms use `TreatMissingData: notBreaching` since a fully dead
check already fires the status alarm.

The facilitator's protected OpenTelemetry stream also records
`x402_readiness_failure_transitions_total` and a matching structured event when
a fixed readiness-failure class appears or changes. This is diagnosis, not a
new public health surface: `/readyz` remains sanitized and the metric labels
contain only `chain_family`, `component`, and a fixed reason code. Never add a
provider hostname, URL, credential, RPC response, signer, balance, nonce, or
transaction value as a CloudWatch or OpenTelemetry dimension.

The counter increments only for a newly observed class; recovery emits the
bounded `chain_readiness_failure_cleared` event without incrementing it. This
keeps the counter useful for recurring failures without turning ordinary
recovery into a false failure signal.

The balance thresholds sit above the configured service warning thresholds,
so the operator is paged with refill headroom before the facilitator itself
starts warning, and well before the hard-stop halts settlement.

## Failure-path coverage

- Relayer balance low → balance alarm (before the service's own warning).
- Metrics push broken after primary/backup fallback (timer, both RPCs,
  credentials, host down) → missing-data on every affected five-minute metric
  → balance/cert alarms. The metrics unit omits an immediate `OnFailure=` hook
  so the balance alarms' three-period evaluation debounces one bad interval.
- Nightly backup fails, including a failed S3 push → unit exits nonzero →
  `OnFailure=` SNS alert, and no `BackupSuccess` datapoint → dead-man alarm
  within a day.
- Certificate renewal failing → `certbot.service` `OnFailure=` alert
  immediately, and `CertDaysRemaining` decays toward the 21-day alarm as a
  backstop.
- Service unhealthy → the existing Route 53 `/readyz` health-check alarms.
- Verify path broken while `/readyz` stays green (auth, parser, chain
  mechanism, RPC, or a crashed demo) → `VerifyCanaryOk` / `DemoWorkOk` drop
  to 0 → canary alarms within 15 minutes.
- Merchant origin, nginx response-header handling, payment middleware, or
  configured production policy broken → `MerchantApiOk` drops to 0 without
  making a payment. A stopped canary timer breaches every canary alarm via
  missing data.
- Facilitator OpenAPI, agent onboarding text, proxy allowlisting, or catalog
  metadata broken → `FacilitatorDiscoveryOk` drops to 0. A valid empty catalog
  remains healthy and communicates no activity claim.
- `/readyz` flapping without sustained failure → hourly
  `HealthCheckPercentageHealthy` alarms.
- Demo endpoints down at the static layer → the Route 53 demo `/` checks.
