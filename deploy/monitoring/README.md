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
| `x402-near-alert@.service` | `/etc/systemd/system/` | `OnFailure=` target for any monitored unit |
| `certbot-onfailure.conf` | `/etc/systemd/system/certbot.service.d/x402-near-onfailure.conf` | Alert on failed certificate renewal |
| `x402-canary.sh` | `/usr/local/bin/` | Synthetic `/verify` + demo `/work` canaries every 5 minutes |
| `x402-canary.{service,timer}` | `/etc/systemd/system/` | Drive the canaries (offset 2 minutes from the metrics timer) |
| `x402-ebs-snapshot.sh` | `/usr/local/bin/` | Daily EBS snapshot + prune (installed but disabled until the IAM grant below) |
| `x402-ebs-snapshot.{service,timer}` | `/etc/systemd/system/` | Drive the snapshot |

Install scripts mode 0755 root-owned, units 0644 root-owned, then
`systemctl daemon-reload` and `systemctl enable --now` the timers.

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
new secrets.

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
| `x402-<inst>-readyz-flapping` | `HealthCheckPercentageHealthy` (Average, 1 h) | `< 90` % | 1 × 1 h |

The metrics script emits one `CertDaysRemaining` datapoint per Let's
Encrypt lineage; when a new lineage is issued (for example a demo
workload's hostnames), create a matching alarm on its `Host` dimension.

The readyz-flapping alarms exist because the Route53 `HealthCheckStatus`
alarms only fire on sustained failure (3 consecutive per checker):
2026-07-27 showed thousands of intermittent `/readyz` 503s per day on the
base instance (free-tier RPC flakiness) without a single status alarm.
Flapping alarms use `TreatMissingData: notBreaching` since a fully dead
check already fires the status alarm.

The balance thresholds sit above the configured service warning thresholds,
so the operator is paged with refill headroom before the facilitator itself
starts warning, and well before the hard-stop halts settlement.

## Failure-path coverage

- Relayer balance low → balance alarm (before the service's own warning).
- Metrics push broken (timer, RPC, credentials, host down) → missing-data
  on every 5-minute metric → balance/cert alarms.
- Nightly backup fails, including a failed S3 push → unit exits nonzero →
  `OnFailure=` SNS alert, and no `BackupSuccess` datapoint → dead-man alarm
  within a day.
- Certificate renewal failing → `certbot.service` `OnFailure=` alert
  immediately, and `CertDaysRemaining` decays toward the 21-day alarm as a
  backstop.
- Service unhealthy → the existing Route 53 `/readyz` health-check alarms.
- Verify path broken while `/readyz` stays green (auth, parser, chain
  mechanism, RPC, or a crashed demo) → `VerifyCanaryOk` / `DemoWorkOk` drop
  to 0 → canary alarms within 15 minutes; a stopped canary timer breaches
  via missing data.
- `/readyz` flapping without sustained failure → hourly
  `HealthCheckPercentageHealthy` alarms.
- Demo endpoints down at the static layer → the Route 53 demo `/` checks.
