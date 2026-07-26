# 2026-07-26 — multi-chain fleet, legacy v1 wire, third-party Base settle

## Fleet state at end of day

| Instance | Version | Notes |
| --- | --- | --- |
| `x402.mikedotexe.com` (near:mainnet) | v0.3.0 | untouched today; `/readyz` 200 throughout |
| `test.x402.mikedotexe.com` (near:testnet) | v0.3.0 | untouched today; `/readyz` 200 throughout |
| `base.x402.mikedotexe.com` (eip155:8453) | v0.4.0 | promoted today with `accept_v1` enabled |

All three demo resource servers redeployed from `main@60eea76` onto the new
symlink layout (`/opt/x402-demo/app -> /opt/x402-demo/releases/<sha>`).

## Shipped (all squash-merged to `main`)

- **#59** — x402scan registration blockers: explicit https `RESOURCE_URL`
  advertised as the resource URL; discovery `openapi.json` docs gained
  `info.x-guidance`, object-form `x-payment-info.protocols`
  (`[{"x402": {}}]`), and corrected payment-identifier text.
- **#60** — dual-emit: eip155 demos serve the legacy v1 JSON 402 body
  (`network:"base"`, `maxAmountRequired`, the token's true EIP-712 domain)
  alongside the canonical v2 `PAYMENT-REQUIRED` header; NEAR demos serve an
  informational hint body.
- **#61** — legacy v1 payment acceptance at the demos: inbound `X-PAYMENT`
  is strictly translated to the v2 wire before the official middleware;
  settlement responses are mirrored to `X-PAYMENT-RESPONSE` with the legacy
  network alias.
- **#63** — facilitator v0.4.0: gated `accept_v1` wire compatibility on
  `/verify`, `/settle`, and `/supported` (v1 requests strictly translated to
  canonical v2 at the parse boundary; responses echo the legacy network
  alias). Released through the attested pipeline (SSH-signed tag on
  `380de41`, checksum and `gh attestation verify` pass) and promoted to the
  Base instance only.
- **#64** — `verify-deployment.sh` updated for dual-kind `/supported` and
  EVM extension advertising.

## Live proofs

- **Legacy v1 wire accepted in production (read-only):** the same real-signed
  ERC-3009 authorization returned `{"isValid": true, "payer": "0x150B…"}`
  from `POST /verify` on `base.x402.mikedotexe.com` in **both** the canonical
  v2 shape and the legacy v1 shape. Negative control: the identical v1 body
  against `x402.mikedotexe.com` (NEAR, gate off) returned HTTP 400
  `malformed_request`.
- **`/supported` dual-advertises** on Base:
  `{"x402Version": 2, "network": "eip155:8453"}` and
  `{"x402Version": 1, "network": "base"}`.
- **Demo dual-emit and v1 acceptance verified live**: unpaid `POST /work` on
  the Base demo returns the v1 body plus the v2 header with an https
  resource URL; malformed and wrong-network `X-PAYMENT` headers fall through
  to 402 (never 5xx); NEAR demos serve the hint body; a full v1
  `X-PAYMENT → 200 + X-PAYMENT-RESPONSE(network:"base")` flow was proven
  end-to-end against a local mock facilitator.
- **Third-party end-to-end settle on Base mainnet**: an external client
  (payer `0x0377e1a4ea6ce5ba5c2b06d36745e66df27902ba` — not an operator test
  wallet) completed the full x402 flow against
  `https://x402-demo-base.mikedotexe.com/work` — 402 challenge, payment
  authorization, result delivery — settling 1000 atomic USDC ($0.001) in
  transaction
  `0xae6b384dff275344ed14f0f9dff0c260726160f16190b3757ad8795a18383316`
  (verified on-chain via the USDC `Transfer` log to the configured payee).

## Operational notes

- The Base instance's live configuration now sets `accept_v1: true`; any
  rollback to a pre-v0.4.0 binary must remove that key first
  (deny-unknown-fields).
- GitHub's server-side "update branch" rebase re-created a PR commit
  unsigned, making the PR unmergeable under required signatures with all
  checks green (#62); the locally signed twin (#63) replaced it.
- A syntactically well-formed but unrecoverable ERC-3009 signature (invalid
  recovery byte) is classified by the upstream verifier as RPC-ambiguous and
  surfaces as HTTP 503 `rpc_unavailable` rather than `invalid_signature`;
  live probes must use really-signed payloads. Pre-existing behavior, not a
  v0.4.0 change.
