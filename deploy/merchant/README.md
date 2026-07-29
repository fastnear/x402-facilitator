# Agent-facing merchant API deployment

This deployment runs the companion resource server from
`examples/merchant-api/` on the existing facilitator host, with one process
per mainnet network:

| Origin | Network | Local port | Facilitator |
| --- | --- | ---: | --- |
| `merchant-near.mikedotexe.com` | `near:mainnet` | 4034 | `x402.mikedotexe.com` |
| `merchant-base.mikedotexe.com` | `eip155:8453` | 4035 | `base.x402.mikedotexe.com` |

The two services use separate facilitator API clients with exact payee rows,
small daily sponsorship budgets, and rate limits. Do not reuse the demo
credentials. The service itself is read-only: it queries chain RPC and never
creates keys or broadcasts transactions.

Credential source files are root-owned, mode 0600 regular files. systemd
delivers each one through `LoadCredential`; on systemd v255 its named-user ACL
can appear as root-owned mode 0440 to `stat(2)`. The application recognizes
only the exact `CREDENTIALS_DIRECTORY/facilitator-api-key` path with that
metadata as systemd's ACL mask. Every other credential path remains strictly
owner-only.

The public API serves `/openapi.json`, `/llms.txt`, `/.well-known/x402`,
`/pricing`, `/terms`, `/robots.txt`, the paid evidence/activity routes, and the quote-only
`POST /v1/routes/usdc/quote` route. That route makes an outbound HTTPS request
to NEAR Intents 1Click but never returns a deposit address or moves funds. The
nginx configuration expects a single certificate lineage named
`merchant-near.mikedotexe.com` covering both hostnames.

Before enabling the Base TLS server, and after every certificate renewal or
nginx reload, verify the certificate presented by each public origin. This
uses SNI and the system trust store only; it sends no payment or credential:

```sh
set -euo pipefail
for host in merchant-near.mikedotexe.com merchant-base.mikedotexe.com; do
  printf '' |
    openssl s_client -connect "$host:443" -servername "$host" \
      -verify_hostname "$host" -verify_return_error 2>/dev/null |
    openssl x509 -noout -checkhost "$host"
done
```

Both checks must succeed before the unpaid regression gate or any directory
preflight. A certificate valid only for the lineage's near hostname is not
sufficient for the Base origin.

Each merchant-to-facilitator HTTP attempt has a seven-second deadline. The
bounded verify retry can take at most fifteen seconds; the three settle
attempts and prescribed 1.5/3-second waits can take at most 25.5 seconds.
That envelope intentionally remains below nginx's 30-second upstream timeout.
Do not raise either limit independently.

Tagged native releases contain both `examples/resource-server/` and
`examples/merchant-api/`, plus an `examples-assets.sha256` manifest. The
merchant installer verifies that manifest, installs production dependencies
and runs the application checks in a private staging directory, then publishes
a root-owned immutable directory under `/opt/x402-merchant/releases/`.

An immediate post-merge deployment does not require an unrelated facilitator
release. From a clean checkout whose `HEAD` and fetched `origin/main` are the
same merged commit, package the exact Git object:

```sh
commit=$(git rev-parse HEAD)
mkdir -p dist/merchant
deploy/merchant/package-commit-release.sh "$commit" dist/merchant
```

The packager uses `git archive`, so untracked files and `node_modules` cannot
enter the bundle. It produces `x402-merchant-git-$commit.tar.gz` and an exact
SHA-256 sidecar. Transfer those two files and the checked-in merchant deployment
scripts over the operator's authenticated channel. Install the scripts
root-owned before invoking them, then install and select the immutable commit:

```sh
release_id="git-$commit"
sudo install -d -m 0755 /usr/local/libexec/x402-merchant
sudo install -o root -g root -m 0755 \
  deploy/merchant/install-release.sh \
  deploy/merchant/promote-release.sh \
  deploy/merchant/rollback-release.sh \
  /usr/local/libexec/x402-merchant/
sudo /usr/local/libexec/x402-merchant/install-release.sh \
  "$release_id" \
  "x402-merchant-$release_id.tar.gz" \
  "x402-merchant-$release_id.tar.gz.sha256"
```

The installer copies the untrusted inputs into a root-only staging directory,
checks the exact filename and one-line checksum, rejects unexpected paths,
links, special files, and bundled dependencies, runs the locked application
checks, and only then publishes the release directory.

For an upgrade of an already live instance, use this unpaid gate one instance
at a time. `release_id` must be `git-$commit` from the merged `origin/main`
SHA above. The pointer check therefore verifies the exact deployed commit
without relying on an editable checkout.

```sh
# First prove the currently serving immutable release is healthy.
npm --prefix /opt/x402-merchant/current-near run regression

# Move only NEAR, then restart the process that still has the old code mapped.
sudo /usr/local/libexec/x402-merchant/promote-release.sh near "$release_id"
sudo systemctl restart x402-merchant-api@near
sudo systemctl is-active --quiet x402-merchant-api@near
test "$(readlink -f /opt/x402-merchant/current-near)" = \
  "/opt/x402-merchant/releases/$release_id"
curl --fail --silent --show-error https://merchant-near.mikedotexe.com/readyz |
  jq -e '.ready == true and .checks.rpc == "ready" and .checks.facilitator == "ready" and .checks.payment == "ready"'
npm --prefix /opt/x402-merchant/current-near run regression -- --target near
sudo journalctl -u x402-merchant-api@near --since "-5 min" --no-pager
```

Repeat the same sequence for `base`, replacing `near` with `base`,
`merchant-near.mikedotexe.com` with `merchant-base.mikedotexe.com`, and the
final regression command with `npm --prefix /opt/x402-merchant/current-base run regression -- --target base`. Do not promote the second instance until the
first post-promotion readiness, regression, pointer, and journal checks are
satisfactory. After both instances pass their independent gates, run the
default full dual-origin check:

```sh
npm --prefix /opt/x402-merchant/current-base run regression
```

For a first installation there is no pre-promotion release to test; enable each
unit after its pointer is selected, then run the same post-promotion checks.

## Operational sequence

1. Create the two Route 53 A/AAAA records and preview the change batch.
2. Create the certificate with the existing ACME webroot.
3. Create `x402-merchant-near` and `x402-merchant-base` users and credential
   directories.
4. Create dedicated facilitator clients and exact payee policies.
5. Copy `near.conf.example` and `base.conf.example` to root-owned, mode-0640
   `/etc/x402-merchant/near.conf` and `/etc/x402-merchant/base.conf`. Review
   every public setting against the intended deployment. Keep all keys out of
   these files and deliver them only through `LoadCredential`.
6. For a later tagged rollout, install a previously verified facilitator
   release's merchant application:

   ```sh
   sudo /opt/x402-near-facilitator/releases/vX.Y.Z/deploy/merchant/install-release.sh vX.Y.Z
   ```

7. Atomically select that immutable release for each process:

   ```sh
   sudo /opt/x402-near-facilitator/releases/vX.Y.Z/deploy/merchant/promote-release.sh near vX.Y.Z
   sudo /opt/x402-near-facilitator/releases/vX.Y.Z/deploy/merchant/promote-release.sh base vX.Y.Z
   ```

   Promotion changes only the `current-near` or `current-base` symlink. It
   deliberately does not restart a process, so the per-instance restart and
   post-promotion unpaid gate above are mandatory for an upgrade.
8. Install the systemd unit and nginx site, validate both, then enable the two
   merchant services and reload nginx.
9. Run `npm run regression` from the release to verify both public origins,
   discovery schemas, CORS, and unpaid x402 challenges without signing or
   broadcasting anything. This rollout stops at that unpaid gate. Do not run
   `npm run proof` as a promotion or deployment check.

For rollback, select an already-installed prior version, restart only the
affected instance, and rerun the unpaid regression gate. The helper accepts
semantic tags, immutable `git-<sha>` IDs, and existing legacy `YYYYMMDD-...`
release IDs. Current installs contain no symlinks. The only compatibility
exception is the four root-owned, in-tree npm command shims in the historical
`20260727-regression-audit-v4` release; the helper verifies each exact link
and target before it can be selected.

```sh
sudo /opt/x402-near-facilitator/releases/vX.Y.Z/deploy/merchant/rollback-release.sh near vPREVIOUS
sudo systemctl restart x402-merchant-api@near
npm --prefix /opt/x402-merchant/current-near run regression -- --target near
```

The rollback tool refuses an uninstalled target, a non-root-owned or writable
release, an unsafe pointer, or a target whose server entrypoint does not parse.
Use the same sequence with `base` for the Base instance.

Set `CORS_ORIGINS` to the exact production browser origins that may invoke the
merchant, for example `https://js.fastnear.com`. Do not use `*`. Verify an
allowed OPTIONS request returns 204 without payment and exposes the canonical
x402 request/response headers; verify an unlisted origin receives 403 on
preflight.

## Optional paid proof — separate authorization required

Paid proof is not part of deployment, promotion, rollback, or the unpaid
regression gate above. This guide authorizes no funded broadcast. If evidence
is needed later, obtain a new human confirmation immediately before each
broadcast; it must show the network, asset contract and atomic amount, payer,
recipient/payee, relayer or signer, and maximum sponsored gas. A confirmation
cannot be reused for a retry or a later proof.

Only after that separate confirmation, create a per-network directory under
`/var/lib/x402-merchant-proof/`, owned by the corresponding merchant service
account and mode 0700, then run `npm run proof` as that account with
`PROOF_RESULT_FILE` inside the directory. The proof runner records a sanitized
pre-broadcast checkpoint and final result atomically; it does not run as part
of the long-lived API service and it never stores a signed payment
authorization.
