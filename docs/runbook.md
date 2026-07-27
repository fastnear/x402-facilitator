# Operations runbook

This runbook describes the controls every self-hosted facilitator needs. It is
deliberately independent of a particular domain, cloud, account, balance, or
release version. The original reference-deployment procedure is preserved as a
[dated historical snapshot](evidence/2026-07-26-reference-deployment-runbook-snapshot.md);
it is not go-live evidence, and its identities or credentials must not be
copied.

Read [configuration.md](configuration.md) and
[threat-model.md](threat-model.md) before operating a funded instance.

## Safety gate

Read-only RPC calls, `/verify`, mock tests, and loopback database tests do not
need a funded-action approval. The following actions require a fresh human
preview and explicit confirmation immediately before execution:

- generating, rotating, adding, or removing a service signing key;
- creating or changing a funded account;
- changing public DNS or TLS;
- issuing a production API key; and
- broadcasting any testnet or mainnet transaction.

For a funded broadcast, the preview must show the network, asset contract,
atomic amount, payer, recipient, relayer or signer, and maximum sponsored gas
in the chain's native unit. Confirmation applies to one exact broadcast only.
An indeterminate result is reconciled by the stored transaction hash and exact
bytes; it is never retried with newly signed replacement bytes.

## Provision an instance

Run one process, database, signer, configuration, and public hostname per
network. Do not share credentials, databases, or policy rows across
environments or other services.

1. Copy the appropriate non-secret example from `deploy/config/` and replace
   all operator-specific identities, URLs, limits, and paths.
2. Provision a PostgreSQL owner/migration role and a separate DML-only service
   role. Keep the database on a private interface.
3. Put the database URLs, dedicated relayer/signer key, and API-key pepper in
   root-owned mode-0600 credential files. Never put their values in JSON, unit
   files, command-line arguments, logs, or source control.
4. Run forward-only migrations with `x402-near-admin migrate` under the
   privileged migration role. Production service startup never migrates.
5. Validate the configuration and credential ownership before starting the
   process.
6. Create an API client and exact network/asset/payee policy. Transfer the raw
   API key once over an authenticated out-of-band channel.
7. Start the service on loopback, require `/readyz` to return 200, and only
   then expose it through a TLS reverse proxy.

The service should run under a dedicated unprivileged account with a
read-only filesystem where practical. The checked-in systemd and Nginx
templates are starting points, not authority to change a host.

## Release installation and upgrade

Use an immutable version directory and an atomic per-instance pointer. Before
promotion:

1. verify the release signature/provenance, checksums, SBOM, and expected
   source revision;
2. inspect the changelog and every migration newer than the installed schema;
3. stop if the release changes the wire contract, database rollback boundary,
   chain policy, credential format, or required operating dependency;
4. install the artifacts without changing the active pointer;
5. run the binary's version and configuration checks on the target host;
6. back up the database and test that the backup is readable;
7. apply required migrations with the privileged admin binary;
8. promote one testnet instance first, require startup reconciliation and
   readiness, then perform the approved acceptance drill; and
9. promote mainnet only after reviewing the dated testnet evidence.

Never describe a promotion or canary as complete without a dated record under
`docs/evidence/`.

Migration `0003` has one deliberate post-transaction maintenance step. After
dropping the legacy full EVM authorization column, `x402-near-admin migrate`
runs `VACUUM (FULL, ANALYZE) settlements` while the service is stopped so the
dropped signature bytes are removed from the current table heap and associated
TOAST storage. It records a completion marker only after the rewrite; v0.5
startup rejects a database whose marker is absent or pending. Allow temporary
disk space for a second copy of the table and do not substitute
`cargo sqlx migrate run`, which cannot complete the out-of-transaction rewrite.

The rewrite cannot erase pre-migration backups or archived WAL. Keep the
pre-migration rollback copy encrypted and access-controlled, do not make a new
post-migration baseline backup until the rewrite completes, and retire the old
backup/WAL at the end of the reviewed rollback-retention window. Record that
external retention decision in the sanitized upgrade evidence.

## Startup and readiness

Startup must remain unready until the service:

- validates the configured network, canonical asset, signer identity, and
  policy;
- validates every migration checksum and the completed authorization-scrub
  maintenance marker;
- connects through both database URLs and acquires session-pinned leadership;
- confirms primary and backup RPC identity and liveness;
- confirms the signer balance is above the hard stop;
- loads at least one active client and exact recipient policy; and
- reconciles every nonterminal settlement.

NEAR additionally checks the configured FullAccess key, final chain state, and
relayer quarantine. EVM additionally checks `eth_chainId`, live head, fee
policy, and confirmation depth.

`/healthz` proves only process liveness. Route paid traffic only while
`/readyz` returns 200.

## Routine checks

At least daily, and after every restart or provider incident:

- check `/readyz` and bounded error/settlement metrics;
- confirm both RPC endpoints report the configured network;
- compare signer balance with warning and hard-stop thresholds;
- inspect counts and ages of nonterminal states without exposing payer,
  authorization, transaction, or API-key material;
- verify database backups and the age of the last restore drill;
- review API-client budgets, policy changes, and credential rotations; and
- check certificate expiry and the reverse proxy's authentication-header
  redaction.

Alert labels must stay low-cardinality. Account IDs, addresses, payment IDs,
authorization hashes, and transaction hashes belong only in access-controlled
investigation data, never metric labels.

## Incident decisions

### Readiness fails

Remove the instance from paid traffic. Determine whether the failure is
configuration, leadership, database, RPC identity/liveness, signer balance,
startup reconciliation, or a chain-specific quarantine. Do not override
readiness or settle manually.

### Settlement is indeterminate

Keep the journal row nonterminal. Query the stored transaction identity through
the configured independent evidence paths. If the submission was prepared,
rebroadcast only its exact stored bytes when the provider's recovery rule
permits it. Never create a replacement transaction merely because an RPC
timed out.

For NEAR, if the relayer nonce advanced while the stored transaction is unknown
on both independent RPCs, keep the relayer quarantined and the instance
unready. For EVM, a mined transaction that disappears before the required
confirmation depth returns to nonterminal reconciliation.

### Database is unavailable or restored

Stop paid traffic until leadership, schema compatibility, journal integrity,
and reconciliation are re-established. After a restore, prove that the restore
point does not omit a broadcast that the chain may still accept. If that
cannot be proven, keep the instance unready and escalate; do not create fresh
submissions for uncertain rows.

### RPCs disagree

Treat disagreement as ambiguous evidence. Keep the affected row nonterminal
and the relevant readiness check failed until independent observations agree
or an operator can establish the authoritative result.

### Signing key is suspected compromised

Stop the instance and paid traffic. Preserve the journal and sanitized audit
evidence. Inventory pending authorizations and stored submissions without
printing them. Rotate or revoke the chain credential only through the safety
gate, then review every nonterminal row before restoring readiness.

### API key or pepper is suspected compromised

Revoke affected clients immediately, disable sponsorship budgets if needed,
and review recent request and settlement identifiers. Rotate the server pepper
as a coordinated all-client credential migration. In-flight settlement
recovery continues by journal identity, not by API-key validity.

## Rollback

Prefer rolling back the binary through the immutable version pointer. Before
doing so, verify that the older binary accepts the current configuration and
database schema. Forward-only migrations are not automatically reversed;
their comments and the release changelog define the safe rollback boundary.

After rollback, restart one instance, require full reconciliation and
readiness, and verify that no nonterminal state lacks a recovery path. Record
the reason, source revision, schema version, observations, and outcome in
dated evidence.

## Evidence record

An operational evidence entry should state:

- date and operator;
- exact source revision and artifact digest;
- network and sanitized instance identity;
- migration and rollback status;
- checks performed and their results;
- transaction links only when the broadcast was explicitly approved; and
- unresolved risks or follow-up actions.

Never include raw credentials, signed payment authorizations, signed
transaction bytes, credentialed URLs, or private telemetry data.
