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
- Resolves the reachable signer advisory using published k256 0.14.0,
  elliptic-curve 0.14.3, ecdsa 0.17.0 and crypto-bigint 0.7.11. Five
  checksum-verified Alloy copies adapt dependency APIs; the small complete
  [patch](../../vendor/alloy-k256.patch),
  [provenance](../../vendor/alloy-k256-provenance.json) and
  [scope/removal instructions](../../vendor/README.md) are checked in.
- Prepares lockstep v0.7.0 because exposed Rust signer types change. External
  consumers must align the k256 types and apply the same Cargo patches; Cargo
  does not propagate patches through a registry dependency.

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

## Signer blocker resolved

The initial #226 commit `d1cb78f4` deliberately retained
[RUSTSEC-2026-0387](https://rustsec.org/advisories/RUSTSEC-2026-0387) on
reachable crypto-bigint 0.5.5, so its first CI run correctly failed. Alloy
2.5.0 still pins the old k256 family. Upstream tracks the issue in
[alloy#4445](https://github.com/alloy-rs/alloy/issues/4445); the core migration
[core#1147](https://github.com/alloy-rs/core/pull/1147) is unmerged as reviewed
on this date. No compatible published Alloy fix was available.

The follow-up applies narrow manifest/API adapters to published Alloy sources,
reaching the published patched RustCrypto stack. It does not implement
cryptographic arithmetic, modify upstream x402 verification or add an advisory
exception. The source guard compares every vendored file with checksum-pinned
original archives, verifies root and fuzz resolution, and rejects vulnerable
crypto-bigint and unreviewed optional signer backends. Both release SBOMs must
contain k256 0.14.0 / crypto-bigint 0.7.11 and no older crypto-bigint.

A detached baseline at `d1cb78f4` recorded 256 deterministic prehash signatures
and one expired, non-authorizing EIP-1559 envelope using the existing unfunded
scalar-one test key. The upgraded signer byte-matches those values. Regression
coverage also checks recovery of the old envelope, high-S parity preservation
on unchecked paths, high-S rejection on checked paths, public-key verification,
wrong-message rejection and invalid-scalar rejection. This establishes tested
functional compatibility, not a timing proof or an independent crypto audit.

The published NEAR 0.38.1 line includes the aws-lc-rs fix we currently vendor,
but near-jsonrpc-client 0.23.0 requires Rust 1.95 and the NEAR line changes
near-account-id from 2.x to 3.x. That compiler/public-type migration is deferred
to a separately scoped release. The current NEAR line accepts the patched
zstd releases used here, so those security fixes do not require that migration.

## Validation

- `./scripts/check-full.sh` exercised ephemeral PostgreSQL migrations,
  Clippy with warnings denied, workspace tests, official TypeScript client
  conformance, rustdoc, locked fuzz metadata, provider packaging, and
  dependency source/license checks. The initial run stopped at the retained
  crypto-bigint advisory. The follow-up `./scripts/check-full.sh` run passes
  end to end with the signer migration: 203 Rust tests, PostgreSQL recovery
  and concurrency tests, official Node HTTP-client conformance, Clippy,
  rustdoc, provider packaging, both lockfile audits, source/license checks,
  configuration, 37 release-guard tests, monitoring and documentation checks.
- The vendor guard passes for both workspaces and a deliberate unrecorded
  source edit is rejected by its positive control. `git diff --check` passes.
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
- Config JSON parsing, release-guard tests, monitoring checks, documentation
  checks, and `git diff --check` are required by the full gate.

No API key, signer, account, policy row, sponsorship allowance, deployed
service, or public catalog entry was created or changed.
