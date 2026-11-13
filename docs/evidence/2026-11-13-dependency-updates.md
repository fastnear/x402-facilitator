# November dependency maintenance — 2026-11-13

This is local dependency and read-only interoperability evidence. It is not a
release, deployment, funded payment, or completed partner integration.

## Changes

- Consolidates the intent of Dependabot PRs #197, #198, #200–#205, #207,
  #213–#216, #219, #220, #222, and #223. The older 2.29.0 proposals are
  superseded by the coordinated 2.30.0 update.
- Pins every direct `@x402/*` dependency to 2.30.0 in both example apps,
  the HTTP conformance harness, and the NEAR fixture generator. Each lockfile
  has only the 2.30.0 core and Zod 4.6.7; neither app retains an older
  nested core tree.
- Both apps resolve Express 5.3.0, body-parser 2.4.0, and qs 6.17.1. The
  merchant viem pin is 2.57.13; resource-server test tooling moves to
  `@types/node` 26.6.5 and tsx 4.23.15.
- Updates the OpenTelemetry API, SDK, HTTP/proto layers, and OTLP exporters
  together to 0.33.1 with tracing-opentelemetry 0.34.0. All Alloy 2.x
  packages move together to 2.5.0.
- Both Cargo lockfiles resolve ctutils 0.4.8, zstd 0.13.5, zstd-safe 7.4.1,
  zstd-sys 2.1.3+zstd.1.5.7, and quinn-udp 0.5.16. Their package/version
  sets match apart from the fuzz harness and libfuzzer-sys.

The canonical v2 contract, v1 gate, database schema, configuration, production
Node independence, and Rust 1.94.1 minimum remain unchanged. A rollback is the
previous source commit and its paired lockfiles; no data migration is needed.

## Security review

- [Zod GHSA-mrhx-c7xc-m3w5](https://github.com/advisories/GHSA-mrhx-c7xc-m3w5)
  is fixed by the coordinated x402/Zod update.
- [body-parser GHSA-27r3-jxmh-p828](https://github.com/advisories/GHSA-27r3-jxmh-p828)
  is fixed in both apps. The resource demo parses JSON before payment
  middleware, so the compressed-body process-crash fix is relevant even to
  unsigned requests.
- [ctutils RUSTSEC-2026-0385](https://rustsec.org/advisories/RUSTSEC-2026-0385),
  the zstd family findings RUSTSEC-2026-0391/-0398/-0403/-0410, and quinn-udp
  RUSTSEC-2026-0538 are removed from both lockfiles' audit results.
- The optional SQLx SQLite driver is absent from normal/build dependencies
  in both workspaces on all targets. Its RUSTSEC-2026-0459 exception is
  guarded by those reachability checks, and a failed tree query fails the
  gate. PostgreSQL remains the only enabled SQLx driver.
- npm's high-severity audit gate passes for all four directories. The
  previously monitored low-severity transitive elliptic finding remains;
  npm reports no compatible fix.
- npm verifies registry signatures for all 176 installed resource-server
  packages and all 124 merchant packages, plus 58 and 29 provenance
  attestations respectively. The fixture installation verifies 35 registry
  signatures and 10 attestations. These checks establish registry integrity
  and published provenance, not an audit of every upstream source line.

## Release blocker retained

[RUSTSEC-2026-0387](https://rustsec.org/advisories/RUSTSEC-2026-0387) remains
on reachable crypto-bigint 0.5.5. Alloy 2.5.0 requires elliptic-curve/k256
0.13, whose crypto-bigint requirement is `^0.5`; the advisory's patched
line is `>=0.7.6`. This is not an optional-driver-only dependency. Both audit
gates continue to fail on it, and no exception is added for it. A compatible
upstream fix or a separately reviewed cryptographic dependency migration is
required before treating this maintenance branch as release-ready.

The published NEAR 0.38.1 line includes the aws-lc-rs fix we currently vendor,
but near-jsonrpc-client 0.23.0 requires Rust 1.95 and the NEAR line changes
near-account-id from 2.x to 3.x. That compiler/public-type migration is deferred
to a separately scoped release. The current NEAR line accepts the patched
zstd releases used here, so those security fixes do not require that migration.

## Validation

- `./scripts/check-full.sh` exercised ephemeral PostgreSQL migrations,
  Clippy with warnings denied, workspace tests, official TypeScript client
  conformance, rustdoc, locked fuzz metadata, provider packaging, and
  dependency source/license checks. Those stages passed. It then stopped
  at the retained crypto-bigint advisory; the full gate did **not** pass.
- Both example `npm run check` suites pass: 97 merchant tests and 23 resource
  tests, plus TypeScript checking, import checking, and catalog validation.
- `npm run check` passes for the NEAR fixtures and HTTP harness. The NEAR
  signed-delegate fixtures are unchanged.
- The added Base-mainnet/Sepolia contract test builds requirements through
  the pinned official server, compares the serialized `accepted` value,
  invokes its matcher, and checks rejection of a changed amount.
- On 2026-11-13, unsigned POST requests with `{}` returned 402 from
  [NEAR mainnet](https://x402-demo.mikedotexe.com/work),
  [NEAR testnet](https://x402-demo-test.mikedotexe.com/work), and
  [Base mainnet](https://x402-demo-base.mikedotexe.com/work).
  Each live `accepts` entry deep-matched the requirements built by the new
  2.30.0 packages and was accepted by their matcher. No payment signature,
  credential, verification request, settlement, or broadcast was sent.
- Config JSON parsing, 37 release-guard tests, monitoring checks, documentation
  checks, and `git diff --check` are run separately because the retained
  advisory stops `check.sh` before its final non-Rust checks.

No API key, signer, account, policy row, sponsorship allowance, deployed
service, or public catalog entry was created or changed.
