# Configuration

Each process loads one non-secret JSON file using `--config` and reads secrets
from files. Checked-in examples live in `deploy/config/`.
Production files are installed as:

```text
/etc/x402-near-facilitator/mainnet.json
/etc/x402-near-facilitator/testnet.json
/etc/x402-near-facilitator/base.json
/etc/x402-near-facilitator/base-sepolia.json
```

The `x402-near-facilitator` directory and environment-variable prefix are
historical compatibility names retained from the original NEAR-only launch.
They apply equally to EVM instances.

The service must reject startup when a required key is unknown, a number is
out of range, a network does not match its Circle asset, a public bind address
is configured for the native deployment, or a secret value is supplied inline.

## Secret file inputs

| Variable | Credential filename | Contents |
| --- | --- | --- |
| `DATABASE_URL_FILE` | `database-url` | PostgreSQL service-role URL for this environment |
| `DATABASE_DIRECT_URL_FILE` | `database-direct-url` | Direct PostgreSQL URL for session leadership; may equal the application URL only when it is already direct |
| `RELAYER_KEY_FILE` | `relayer-key` | Dedicated relayer/signer service key; never an operator recovery key |
| `API_KEY_PEPPER_FILE` | `api-key-pepper` | Random HMAC pepper independent of all API keys |
| `PRIMARY_RPC_URL_FILE` | `primary-rpc-url` | Optional complete HTTPS RPC URL when the provider embeds a credential in its path or query |
| `BACKUP_RPC_URL_FILE` | `backup-rpc-url` | Optional complete HTTPS backup RPC URL when it embeds a credential |
| `OTEL_EXPORTER_OTLP_HEADERS_FILE` | `otel-headers` | Optional OTLP authorization header; omit when telemetry export is disabled |

Files must contain only the value, end with a newline, be owned by root, and be
mode 0600 before systemd imports them. The service should trim one terminal
newline, but no other whitespace. Secret values must never be accepted through
CLI arguments.

The JSON configuration continues to carry non-secret fallback RPC URLs.
Authenticated provider URLs override either corresponding JSON value only when
`PRIMARY_RPC_URL_FILE` or `BACKUP_RPC_URL_FILE` is set, and the complete
configuration is validated again after the override. Install an instance-local
systemd drop-in based on
`deploy/systemd/x402-rpc-credentials.conf.example`; do not add authenticated
URLs to checked-in JSON, an `Environment=` value, or a command line. The two
effective endpoints must still use HTTPS and different hosts.

The checked-in examples disable telemetry export. If an OTLP backend is
adopted, set its HTTPS endpoint, resource attributes
for `service.name=x402-near-facilitator`, `deployment.environment.name`, and
`service.version`, and repeat the sanitized-event verification before
production use. Never put a dataset name or API key in source-controlled
examples if it identifies a private environment.

## NEAR environment isolation

The two checked-in NEAR examples intentionally differ in every value that can
prevent a cross-network mistake. The account IDs below describe the public
reference profile; self-hosters must use dedicated identities they control:

| Setting | Testnet | Mainnet |
| --- | --- | --- |
| Network | `near:testnet` | `near:mainnet` |
| Bind address | `127.0.0.1:8403` | `127.0.0.1:8402` |
| Relayer | `x402-relayer.mike.testnet` | `x402-relayer2.mike.near` |
| Primary RPC | `rpc.testnet.fastnear.com` | `rpc.mainnet.fastnear.com` |
| Backup RPC | `archival-rpc.testnet.fastnear.com` | `archival-rpc.mainnet.fastnear.com` |
| Global daily cap | 2 NEAR | 0.50 NEAR |
| Default client cap | 1 NEAR | 0.10 NEAR |
| Balance warning | 2 NEAR | 1 NEAR |
| Hard stop | 0.50 NEAR | 0.25 NEAR |

All NEAR-denominated configuration is expressed as decimal yoctoNEAR strings,
not floating point. Circle USDC quantities are decimal atomic-unit strings.
Configuration validation requires at least 1,000 atomic USDC.

NEAR readiness independently requires both configured RPC readers to report
the expected `status.chain_id` and a final block. Protected telemetry and
structured logs classify a degraded pair with only the fixed codes
`primary_rpc_unavailable`, `backup_rpc_unavailable`,
`both_rpc_unavailable`, or `chain_id_mismatch`. They never include an RPC URL,
credential, provider response, or observed chain value. Public `/readyz`
remains limited to its sanitized boolean gate states.

## EVM (eip155) instances

An instance selects its chain family with `chain_kind`; it defaults to `near`
when absent, so the NEAR examples above need no new key. An `eip155` instance
(Base) adds a required `eip155` block and reuses the same top-level keys with
chain-appropriate values:

| Key | NEAR (`near`) | Base (`eip155`) |
| --- | --- | --- |
| `chain_kind` | `near` (default) | `eip155` |
| `network` | `near:<network>` | `eip155:<chain-id>` (`eip155:84532`, `eip155:8453`) |
| `relayer_account_id` | NEAR account ID | the signer's `0x` secp256k1 address |
| `asset` | NEAR USDC account ID | the chain's canonical Circle USDC `0x` address |
| `max_inner_gas` | NEAR gas ceiling | `0`; unused, EVM gas comes from the `eip155` block |
| `RELAYER_KEY_FILE` | ED25519 key | secp256k1 key (`0x`-hex, 32 bytes), same file contract |

The `eip155` block carries the chain-specific settlement parameters:

| Field | Meaning |
| --- | --- |
| `chain_id` | must equal the numeric suffix of `network` |
| `required_confirmations` | confirmation depth (≥ 1) a mined transaction must reach before the journal marks it terminal — the reorg-safety margin |
| `gas_limit` | per-settlement gas cap for the `transferWithAuthorization` call |
| `max_fee_per_gas_wei` | positive decimal-string ceiling for the EIP-1559 maximum fee per gas; the service never signs above it |

`primary_rpc_url` and `backup_rpc_url` must identify distinct EVM readers.
Durable head, pending-nonce, balance, and receipt decisions consult both;
identity or receipt disagreement is indeterminate and fails closed. Both
endpoints must return Base's `l1Fee` receipt quantity, and preparation must be
able to call the canonical Base GasPriceOracle predeploy over the exact signed
transaction bytes.

When the dual-reader signer-head readiness snapshot fails, protected telemetry
and structured service logs use only fixed codes such as
`primary_rpc_unavailable`, `chain_id_mismatch`, or
`pending_nonce_disagreement`. They never include either RPC URL, a provider
response, observed chain/head values, signer address, balance, or nonce.
Public `/readyz` remains limited to its sanitized boolean gate states.

The service binds `network` to its canonical Circle USDC and refuses a
mismatch, exactly as the NEAR branch binds a network to its USDC account:

- Base mainnet `eip155:8453` → `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`
- Base Sepolia `eip155:84532` → `0x036CbD53842c5426634e7929541eC2318f3dCF7e`

The sponsorship budgets keep their `*_yocto_near` names but hold the chain's
native atomic gas unit — **wei** for eip155. In the Base Sepolia example the
`10000000000000000` hard stop is 0.01 ETH and the `50000000000000000` warning
is 0.05 ETH. `reservation_yocto_near` must be greater than
`gas_limit × max_fee_per_gas_wei`; the remaining reservation covers Base's L1
data fee. EVM readiness requires the signer balance to cover both the hard stop
and one complete reservation.

An eip155 instance may additionally set `"accept_v1": true` to accept legacy
x402 v1 wire requests on `/verify` and `/settle` (translated internally to
the canonical v2 shape; responses echo `network` as the legacy alias `base` /
`base-sepolia`) and to advertise an `x402Version: 1` kind on `/supported`.
The flag defaults to `false` and is rejected at validation for `near`
configs — x402 v1 never covered NEAR networks.

The checked-in Base profiles are isolated as follows. These are non-secret
configuration examples, not public deployment status:

| Setting | Base Sepolia | Base mainnet |
| --- | --- | --- |
| Network | `eip155:84532` | `eip155:8453` |
| Bind address | `127.0.0.1:8404` | `127.0.0.1:8405` |
| Signer | placeholder secp256k1 address | placeholder secp256k1 address |
| Primary RPC | `https://sepolia.base.org` | `https://base.drpc.org` |
| Backup RPC | `https://base-sepolia-rpc.publicnode.com` | `https://base-mainnet.public.blastapi.io` |
| Canonical USDC | `0x036CbD53842c5426634e7929541eC2318f3dCF7e` | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` |
| USDC EIP-712 domain | `USDC` / `2` | `USD Coin` / `2` |
| Balance warning | 0.05 ETH | 0.005 ETH |
| Hard stop | 0.01 ETH | 0.002 ETH |
| Legacy v1 gate | off | on in the example; opt-in only |

## Database roles

Create an independent database for every network in a private or loopback-only
PostgreSQL cluster. Each environment has:

- an owner/migration role used only by `x402-near-admin migrate`;
- a service role with connect and DML privileges on the facilitator schema,
  but no schema creation, alteration, role management, or cross-database
  access;
- an operator-observer role with column-level read access only to sanitized
  settlement state/timestamps/reasons and global sponsorship totals. It has no
  access to client/account identities, hashes, payload or transaction bytes,
  terminal response bodies, or API-key data.

Both URL files may contain the same direct localhost URL: there is no
connection pooler, so the application connection already satisfies the
session-pinned leadership requirement. Do not reuse a database or role from
any other service.

## Validation before service start

The effective configuration check must confirm:

- config and each credential file are readable by the service;
- the database schema version is compatible, without applying migrations, and
  the v0.5 authorization-scrub table rewrite is marked complete;
- the advisory leadership connection can remain session-pinned;
- primary and backup RPCs report the configured network and final blocks;
- configured asset, relayer, and minimum amount match the environment;
- the relayer key belongs to the configured account and is FullAccess;
- at least one API client is active;
- recipient policies exist for every enabled API client;
- the relayer is not quarantined and its balance is above the hard stop;
- nonterminal settlement reconciliation has completed.

Only then may `/readyz` return 200.

For an `eip155` instance the network-specific checks read in chain-native
terms: both RPCs report the configured `chain_id` and a live head; the
configured asset and minimum match the network; the signer address matches
`relayer_account_id` and its native-gas balance is above the hard stop. An
eip155 instance has no FullAccess-key or nonce-quarantine analog, so those two
NEAR checks are simply not part of its readiness.
