# Base merchant canary incident investigation — 2026-09-03

Status: recovered; primary RPC failure class confirmed, underlying provider
error unresolved.

This investigation examined AWS account `341982967115`, live CloudWatch
metrics and alarm history, EC2 status, public readiness responses, sanitized
host logs, and the installed canary and merchant implementation. Collection
occurred around 21:55–22:05 UTC. The user authorized temporary SSH ingress
from the workstation's current IPv4 address; that exact rule was subsequently
removed and verified absent. Host inspection was read-only. No service,
provider configuration, credentials, or payment state was changed. This
document is the only repository change.

## Finding

The Base merchant canary failed eight consecutive readiness probes over
approximately 40 minutes, each returning HTTP 503. These were published zero
values, not missing-data substitutions. Host logs additionally show 22 Base
facilitator primary RPC failure-and-recovery cycles from 15:28:10 through
16:08:39 UTC. Both the merchant RPC and facilitator primary use the same
Alchemy endpoint; equality was checked in memory without printing credentials.

The evidence strongly supports intermittent failure of this shared upstream
RPC dependency. The facilitator's primary could not complete its required
snapshot while its independent backup could. The retained logs do not identify
the failing RPC method, HTTP status, or provider error, so throttling, timeout,
transport failure, and other upstream causes cannot be distinguished.

The initial CloudWatch-only view understated the scope: Route 53's health
state remained 100% healthy despite individual HTTP 503 responses. EC2 and
host evidence does not indicate a host outage or resource exhaustion.

The alarm
[`x402-merchant-base-api-canary-failing`](https://us-east-1.console.aws.amazon.com/cloudwatch/deeplink.js?region=us-east-1#alarmsV2:alarm/x402-merchant-base-api-canary-failing)
uses `x402near/MerchantApiOk`, `Network=base`, Minimum, 300-second periods,
`< 1`, two of three periods, with missing data treated as breaching.

## Timeline

All timestamps below are UTC on 2026-09-03. Metric timestamps are one-minute
CloudWatch bucket timestamps, not exact request start times.

| Observation | UTC | America/Los_Angeles (PDT) |
| --- | --- | --- |
| Last healthy sample before failure | 15:27 | 08:27 |
| First failed sample | 15:32 | 08:32 |
| Alarm entered ALARM | 15:39:39.294 | 08:39:39.294 |
| Last failed sample | 16:08 | 09:08 |
| First recovered sample | 16:12 | 09:12 |
| Second recovered sample | 16:17 | 09:17 |
| Alarm entered OK; supplied email | 16:19:39.292 | 09:19:39.292 |

The eight zero samples occurred at 15:32, 15:38, 15:42, 15:47, 15:52, 15:57,
16:02, and 16:08. CloudWatch's alarm evaluation uses five-minute buckets
offset from these one-minute buckets: the email's recovered periods beginning
16:09 and 16:14 contain the successful 16:12 and 16:17 samples.

From 2026-09-02 22:00 UTC to the initial AWS-only collection around 21:55 UTC,
the metric returned 287 populated one-minute buckets, only eight of which were
zero. The latest sample in that query was healthy at 21:52 UTC. The only
`x402*` alarm state changes returned for
that interval were this alarm's ALARM and OK transitions. All 27 current
`x402*` alarms were OK at inspection.

## Correlated AWS evidence

The correlation window was 14:30–17:30 UTC. Each five-minute series below
contained 36 populated periods. The Route 53 series contained 180 one-minute
periods.

| Signal | Observed range/result |
| --- | --- |
| Base merchant `MerchantApiOk` | Eight zero periods; other periods one |
| NEAR mainnet merchant `MerchantApiOk` | Always one |
| Base `FacilitatorDiscoveryOk` | Always one |
| Base `VerifyCanaryOk` | Always one |
| Base `DemoWorkOk` | Always one |
| Base signer balance | Constant 0.069998 ETH |
| Base facilitator Route 53 `HealthCheckStatus` | Always one |
| Base facilitator Route 53 `HealthCheckPercentageHealthy` | Always 100% |
| EC2 `CPUUtilization`, Maximum | 7.47–11.53% |
| EC2 `CPUCreditBalance`, Minimum | Constant 576 |
| EC2 `CPUSurplusCreditBalance`, Maximum | Zero |
| EC2 `StatusCheckFailed`, Instance, and System, Maximum | All zero |

The host was verified as `i-0537770b34b04b820`, `x402-facilitator`, `t3.small`,
in `us-west-2a`, with Elastic IP `100.23.147.163`. Its instance, system, and
attached EBS reachability checks were currently OK, with no scheduled event
returned. Monitoring metrics and alarms live separately in `us-east-1`.
At host inspection, uptime was 43 days, root filesystem usage was 22%, and
available memory was approximately 1,110 MiB of 1,907 MiB. Historical memory
and filesystem-usage samples were not collected. The incident kernel journal
had no matching OOM, I/O-error, segfault, or hung-task classifications.

At collection time, public `/readyz` requests returned HTTP 200 with every
reported dependency ready for the Base and NEAR merchants and their respective
facilitators. Base merchant readiness took approximately 0.27 seconds in this
single request; this is a current observation, not incident latency evidence.

## Host log findings

The host incident window was 15:20–16:25 UTC.

- All eight canary warnings were exactly the bounded classification
  `merchant canary base: readiness check failed (status=503)`.
- The Base merchant Nginx access log confirms all eight readiness HTTP 503s;
  its error log contains zero entries in the incident window. Failed canary
  runs stopped before the unpaid account-evidence challenge.
- Seven failed readiness responses had 84-byte bodies. The 16:02:40 response
  had 88 bytes; healthy responses had 79 bytes. These are body sizes, not
  retained response JSON.
- The Base merchant journal contained no entries in the incident window.
  Its process had been running since July 30 at 13:24:36 UTC with
  `NRestarts=0`. The Base facilitator had been running since August 1 at
  01:26:40 UTC with `NRestarts=0`.
- The Base facilitator journal recorded 22 `chain_readiness_failure` events,
  all `component=head`, `reason=primary_rpc_unavailable`, and 22 corresponding
  clear events. The RPC and relayer readiness gates followed those
  transitions. Intervals between failure and clear events ranged from 9.53
  to 52.24 seconds; these log intervals are not a request-level downtime SLA.
- The shared facilitator access log recorded 202 Route 53-agent `/readyz`
  HTTP 503 responses and one Node-agent HTTP 503, at 16:02:31. Node-agent
  `/supported` requests all returned HTTP 200. The Node failures and successes
  align with the merchant probe times; the shared log does not label networks.
- The same day's broader Base journal showed three earlier
  `backup_rpc_unavailable` transitions in the 13:00 UTC hour. The 22 primary
  failures were confined to the incident; no later Base readiness failure
  transition appeared before the approximately 22:04 UTC inspection.

`component=head` names the aggregate EVM snapshot, not a particular method.
The [provider implementation](../../crates/x402-chain-eip155-provider/src/provider.rs)
requires chain ID, block number, pending signer nonce, and gas balance from
each endpoint. The `primary_rpc_unavailable` class means the primary snapshot
failed while the backup snapshot succeeded. It does not establish a
provider-wide outage or a failing block-number request specifically.

The effective primary was Alchemy; the facilitator backup was
`mainnet.base.org`. The merchant used exactly the same endpoint as the
facilitator primary. No credentialed URL was printed or retained in evidence.

## Why the Route 53 metric stayed green

The live Base health check uses HTTPS `/readyz`, a 30-second request interval,
and a failure threshold of three. Route 53 changes a checker's health state
after consecutive failures, resetting the counter when responses recover;
it does not expose the percentage of individual successful HTTP requests as
`HealthCheckPercentageHealthy`. See the
[AWS health-check behavior](https://docs.aws.amazon.com/Route53/latest/DeveloperGuide/welcome-health-checks.html)
and [checker aggregation](https://docs.aws.amazon.com/Route53/latest/DeveloperGuide/dns-failover-determining-health-of-endpoints.html).

The short recorded failure/clear cycles and actual 503s are consistent with
individual checkers recovering before their three-failure threshold. Thus the
100% health-state metric is compatible with this intermittent failure and
must not be described as 100% HTTP request success. The merchant canary
provided a useful independent signal.

## What the merchant canary covers

The checked-in [canary](../../deploy/monitoring/x402-canary.sh) first requires
merchant `/readyz` HTTP 200 with `rpc`, `facilitator`, and `payment` ready. It
then requires a valid unpaid account-evidence request to return the exact
configured canonical v2 Base USDC payment challenge. It does not pay or execute
the requested paid work. Host logs now identify readiness as the failed stage,
so challenge validation was not the cause of these eight failures.

Merchant RPC readiness checks chain identity and a finalized block. Merchant
facilitator readiness checks `/supported` and `/readyz`. The merchant's payment
initialization remains ready after successful startup. With the verified
unchanged process and matching source, the observed body sizes are consistent
with one failed dependency in seven probes and both RPC and facilitator
dependencies failing at 16:02:40. This is an inference from compact JSON sizes
and code, not captured response bodies; it assumes no body transformation.
The corresponding facilitator Node-agent 503 at 16:02:31 independently
supports the latter observation. Sizes alone cannot identify which dependency
failed in the seven single-failure responses.

The readiness code collapses exceptions to bounded dependency states, while
the canary's readiness warning records HTTP status without those states.
The elapsed time between the preceding mainnet canary completion and the Base
readiness warning was approximately 10.5 seconds for seven failures and 9.4
seconds for the other. These are inferred timings, not recorded server
request-duration fields. All returned HTTP 503 before the canary's 20-second
deadline; the evidence does not show canary-side timeouts.

The installed Base merchant release was
`git-cdb2d003d3eb6f98f9bcf714723581628f057505`. SHA-256 comparisons confirmed
that installed `app.mjs`, `chain-reader.mjs`, `facilitator.mjs`, and
`/usr/local/bin/x402-canary.sh` exactly matched their checkout counterparts.

## Follow-up opportunities

The remaining provider-level cause requires additional historical diagnostics,
such as Alchemy-side request/error records for approximately 15:28–16:09 UTC.
No such records were available in this investigation. Recovered live requests
cannot establish the historical HTTP or RPC error.

The most direct observability improvement is to retain bounded merchant
dependency states and elapsed times on failed probes, with equivalent NEAR
and Base behavior. Bounded provider failure categories and readiness-transition
metrics would distinguish throttling, timeout, transport, and invalid-response
failures without exposing provider URLs or payment data. These improvements
were identified but not implemented or deployed. Existing fail-closed
settlement and readiness behavior should be preserved.

## Temporary access and cleanup

The original SSH connection timed out because security group
`sg-015857c4ae084d397` did not include the workstation's current public IPv4
address. After explicit user authorization, temporary rule
`sgr-05193f2ed871b4cb6` allowed only that address as a `/32` on TCP 22.
The rule was revoked and verified absent at **22:04:46 UTC**. The original
SSH ingress rule remained; no existing ingress rule was replaced.

The current AWS identity was denied `logs:DescribeLogGroups` and
`ssm:DescribeInstanceInformation`. No IAM permissions were modified. SSH
provided the authorized host inspection path. Logs were parsed on the host
and only bounded classifications, timestamps, counts, and sizes were emitted.
No raw request bodies, credentials, or signed authorizations were retrieved.

After inspection, loopback merchant `/healthz` and `/readyz`, public merchant
`/readyz`, and public Base facilitator `/readyz` all returned HTTP 200; all
reported readiness dependencies were ready. No service restart or provider
cutover was performed.

## Read-only data sources

- `cloudwatch describe-alarms`, prefix `x402` and exact merchant alarm.
- `cloudwatch describe-alarm-history`, StateUpdate, incident and preceding
  day, parsed to UTC.
- `cloudwatch get-metric-statistics`, merchant Base metric, Minimum and
  SampleCount, 60-second periods.
- `cloudwatch get-metric-data`, correlated custom metrics in `us-east-1`,
  Route 53 health check `b5232894-cbd5-4189-b5dc-9a74e9796073`, and EC2 metrics
  in `us-west-2`.
- `ec2 describe-instances`, `describe-instance-status`, and
  `describe-security-groups` for the verified host.
- `route53 get-health-check` for the Base readiness configuration.
- Host `systemctl show`, bounded `journalctl` queries, sanitized Nginx log
  parsing, file hashes, and in-memory endpoint comparison.
- Single unauthenticated HTTPS readiness requests to the two merchant and
  two facilitator public origins.

No deployment, provider repair, funded transaction, or underlying provider-error
resolution is claimed.
