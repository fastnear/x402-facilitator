# Base mainnet canary payer provisioning — 2026-07-26

Owner: Mike Purvis

This record covers offline creation and funding of a dedicated, low-value Base
mainnet payer for x402 canaries. It is credential and funding evidence only;
no x402 payment or v0.5.0 deployment is claimed here.

## Credential handling

- Network: `eip155:8453`.
- Payer: `0x11B1cb965c64A8005953c1622a67C2030bEB7987`.
- Purpose: dedicated Base mainnet x402 canary payer; do not reuse it as the
  facilitator signer or for another service.
- Local credential:
  `/Users/mikepurvis/.local/share/x402-near-facilitator/credentials/base-mainnet-canary-payer.json`.
- The credential directory is mode 0700 and the credential is mode 0600,
  both owned by the workstation user.
- The stored private key was independently re-derived to the recorded address.
  It was not printed, logged, placed in an environment variable, or added to
  the repository.

## Funding evidence

The payer received exactly 1,000,000 atomic units (1 USDC) of canonical Base
USDC,
`0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`, in
[transaction `0x4939…3daa`](https://basescan.org/tx/0x4939bf13613d0915911a0e0365ef49e481fd42e7adcc27c2c9dfa6bdde523daa).

At observation:

- the transaction receipt status was successful;
- the transfer was at block 49,161,498;
- the conservative confirmation count was 34;
- Base's public RPC and PublicNode both reported chain ID 8453, head
  49,161,531, and a payer balance of exactly 1,000,000 atomic USDC; and
- the payer held zero ETH, as expected for an ERC-3009 payer whose facilitator
  sponsors settlement gas.

Before any funded x402 canary, refresh the balance, challenge, signer balance,
fee envelope, and both RPC heads, then obtain the required transaction-specific
confirmation. Do not reuse a confirmation after an ambiguous submission.
