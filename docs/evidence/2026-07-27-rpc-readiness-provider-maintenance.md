# RPC readiness and provider maintenance — 2026-07-27

This sanitized record covers the Base/NEAR RPC incident review and the
host-only maintenance completed on the reference deployment. It does not claim
that the v0.5.2 service binary was deployed; all three facilitator instances
remained on the attested v0.5.1 release while the software patch entered its
normal review and release gate.

## Incident finding

- The reference host remained healthy on its existing `t3.small` instance;
  CPU, memory, disk, and service state did not justify an infrastructure
  resize.
- Intermittent Base readiness failures correlated with rate limiting and
  timeouts from the public Base RPC pair. The durable journal, signer balance,
  PostgreSQL service, and NEAR instances remained healthy.
- NEAR mainnet and testnet were already configured with FastNEAR regular and
  archival endpoints. No NEAR provider cutover was necessary.
- The Base service remained on its existing public primary/backup readers
  pending the v0.5.2 binary rollout. No settlement transaction was broadcast.

## Authenticated Base provider

- A dedicated Alchemy app named `x402 Facilitator Base Mainnet` was created
  with only the Node API service selected.
- The automatically created, unused first-app credential became visible during
  dashboard inspection. It was treated as compromised and revoked immediately
  by deleting that unused app before the credential was installed or used.
- The replacement app key is restricted to the production host's Elastic IP.
  The full endpoint was transferred directly from the authenticated dashboard
  into
  `/etc/x402-near-facilitator/credentials/base/primary-rpc-url`; it was not
  placed in the repository, a shell argument, an ordinary environment
  variable, or operator output.
- The credential source is `0600 root:root`. A host-originated, sanitized
  preflight confirmed Base chain ID `8453` and a usable block head without
  printing the endpoint or response body.

## Monitoring change

The following reviewed assets were installed directly because monitoring
assets are host-managed and are not part of the facilitator release archive:

| Asset | SHA-256 |
| --- | --- |
| `/usr/local/bin/x402-near-metrics.sh` | `26d079634c75da5146c3230a4e98390691d7f6a343caa05064a7a40923ec6505` |
| `/etc/systemd/system/x402-near-metrics.service` | `d16e4dca5d14d93c79837835d5cc0b2fb83e7d19aa13f58ecc1d9ea9296c6532` |

The script now reads the effective protected RPC credential when present,
tries the primary once and then the independent backup once, and suppresses
provider URLs, curl diagnostics, and RPC bodies. The metrics service no longer
fires an immediate `OnFailure=` email for one failed interval; the existing
three-period, five-minute, missing-data-breaching alarms remain the debounced
dead-man path. The oneshot has a three-minute start deadline, long enough for
both bounded RPC attempts across all three installed instances.

Post-install checks:

- `x402-near-metrics.service`: `Result=success`, `ExecMainStatus=0`;
- `x402-near-metrics.timer`: active;
- fresh `SignerBalanceEth{Network=base}` datapoints reached CloudWatch through
  17:15 PDT;
- the Base balance, sustained-unhealthy, and hourly-flapping alarms were `OK`;
- the five-minute metrics journal contained no `alchemy`, provider host, or
  `/v2/` endpoint text.

The previous metrics script and unit remain recoverable under
`/root/x402-maintenance-backup/20260727-rpc-readiness/`; the intermediate
first-stage script is preserved there as
`x402-near-metrics.sh.pre-partial-credential-fix`.
The intermediate unit is likewise preserved as
`x402-near-metrics.service.pre-timeout-fix`.

## Log rotation repair

- Ubuntu's `/etc/logrotate.d/nginx` was confirmed to own
  `/var/log/nginx/*.log`.
- The overlapping
  `/etc/logrotate.d/x402-near-facilitator` file was moved, not deleted, to the
  root-only maintenance backup directory.
- `logrotate --debug /etc/logrotate.conf` succeeded.
- `logrotate.service` was reset and run successfully:
  `Result=success`, `ExecMainStatus=0`.

## Service health after host maintenance

The public Base mainnet, NEAR mainnet, and NEAR testnet `/readyz` endpoints
each returned `ready: true` with database, leadership, reconciliation, RPC,
and relayer gates ready. The Base service was still on v0.5.1 at this
checkpoint; therefore its service traffic had not yet switched to the
protected Alchemy endpoint.

## Remaining v0.5.2 gate

Before the Base service cutover:

1. pass the complete local and PostgreSQL/conformance gates;
2. merge and produce the signed, attested v0.5.2 release;
3. prove there are no active nonterminal Base settlements;
4. install the protected-RPC systemd drop-in, promote v0.5.2, and restart only
   the Base instance;
5. verify `/`, `/supported`, `/readyz`, the bounded readiness-transition
   journal, exact provider behavior, and CloudWatch telemetry;
6. retain v0.5.1 plus the prior config/unit assets as the immediate rollback.
