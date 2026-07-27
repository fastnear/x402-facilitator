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

The public API serves `/openapi.json`, `/llms.txt`, `/.well-known/x402`, the
paid evidence/activity routes, and the quote-only
`POST /v1/routes/usdc/quote` route. That route makes an outbound HTTPS request
to NEAR Intents 1Click but never returns a deposit address or moves funds. The
nginx configuration expects a single certificate lineage named
`merchant-near.mikedotexe.com` covering both hostnames.

## Operational sequence

1. Create the two Route 53 A/AAAA records and preview the change batch.
2. Create the certificate with the existing ACME webroot.
3. Create `x402-merchant-near` and `x402-merchant-base` users and credential
   directories.
4. Create dedicated facilitator clients and exact payee policies.
5. Install the application release and run `npm ci --omit=dev`.
6. Install the systemd unit and nginx site, validate both, then enable the two
   merchant services and reload nginx.
7. Verify discovery, unpaid 402 responses, and paid flows. A funded test
   must display the network, asset, amount, payer, payee, relayer/signer, and
   maximum sponsored gas immediately before broadcast.

Set `CORS_ORIGINS` to the exact production browser origins that may invoke the
merchant, for example `https://js.fastnear.com`. Do not use `*`. Verify an
allowed OPTIONS request returns 204 without payment and exposes the canonical
x402 request/response headers; verify an unlisted origin receives 403 on
preflight.

For repeatable paid-flow evidence, create a per-network directory under
`/var/lib/x402-merchant-proof/`, owned by the corresponding merchant service
account and mode 0700. Run `npm run proof` as that account with
`PROOF_RESULT_FILE` inside the directory. The proof runner records a sanitized
pre-broadcast checkpoint and final result atomically; it does not run as part
of the long-lived API service and it never stores a signed payment
authorization.
