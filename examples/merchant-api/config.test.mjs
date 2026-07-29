import assert from "node:assert/strict";
import {
  chmodSync,
  mkdtempSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  NETWORK_PROFILES,
  expectedSystemdCredentialPath,
  formatUsdc,
  isExpectedSystemdAclCredential,
  loadConfig,
  readCredential,
  readInstalledReleaseId,
} from "./config.mjs";

const PAYEES = {
  "near:mainnet": "merchant.near",
  "near:testnet": "merchant.testnet",
  "eip155:8453": "0x1111111111111111111111111111111111111111",
  "eip155:84532": "0x2222222222222222222222222222222222222222",
};

function environment(network, overrides = {}) {
  return {
    NETWORK: network,
    FACILITATOR_URL: "https://facilitator.example",
    FACILITATOR_API_KEY_FILE: "/credential",
    RPC_URL: "https://rpc.example/v1?provider=test",
    RESOURCE_ORIGIN: "https://merchant.example",
    ASSET: NETWORK_PROFILES[network].asset,
    PAY_TO: PAYEES[network],
    ...overrides,
  };
}

const credentialReader = () => "test-credential";
const RELEASE_ID = "git-0123456789abcdef0123456789abcdef01234567";

test("loads each exact supported network profile", () => {
  for (const [network, profile] of Object.entries(NETWORK_PROFILES)) {
    const config = loadConfig(environment(network), { credentialReader });
    assert.equal(config.network, network);
    assert.equal(config.asset, profile.asset);
    assert.equal(config.chainId, profile.chainId);
    assert.equal(config.eip712Name, profile.eip712Name);
    assert.equal(config.eip712Version, profile.eip712Version);
    assert.equal(config.amount, "1000");
    assert.equal(config.priceUsd, "0.001000");
    assert.equal(config.releaseId, undefined);
  }
});

test("records an installed immutable release ID and requires it only in production", () => {
  const releaseIdReader = () => RELEASE_ID;
  const config = loadConfig(
    environment("eip155:8453", {
      MERCHANT_RELEASE_METADATA_REQUIRED: "1",
    }),
    { credentialReader, releaseIdReader },
  );
  assert.equal(config.releaseId, RELEASE_ID);

  assert.throws(
    () => loadConfig(
      environment("eip155:8453", {
        MERCHANT_RELEASE_METADATA_REQUIRED: "1",
      }),
      { credentialReader, releaseIdReader: () => undefined },
    ),
    /installed merchant release metadata is required/,
  );
  assert.throws(
    () => loadConfig(
      environment("eip155:8453", {
        MERCHANT_RELEASE_METADATA_REQUIRED: "true",
      }),
      { credentialReader, releaseIdReader },
    ),
    /MERCHANT_RELEASE_METADATA_REQUIRED must be 1/,
  );
});

test("binds the facilitator key to systemd's exact credential path", () => {
  const credentialsDirectory = "/run/credentials/x402-merchant-api@near.service";
  const apiKeyPath = `${credentialsDirectory}/facilitator-api-key`;
  let observed;
  loadConfig(
    environment("near:mainnet", {
      CREDENTIALS_DIRECTORY: credentialsDirectory,
      FACILITATOR_API_KEY_FILE: apiKeyPath,
    }),
    {
      credentialReader: (...arguments_) => {
        observed = arguments_;
        return "test-credential";
      },
    },
  );
  assert.deepEqual(observed, [
    apiKeyPath,
    "facilitator API key",
    { expectedSystemdPath: apiKeyPath },
  ]);
});

test("rejects unsupported networks and noncanonical network fields", () => {
  const invalid = [
    environment("near:mainnet", { NETWORK: "near:custom" }),
    environment("near:mainnet", { ASSET: "not-usdc" }),
    environment("near:mainnet", { PAY_TO: "INVALID.NEAR" }),
    environment("near:mainnet", { ASSET_EIP712_NAME: "USD Coin" }),
    environment("eip155:8453", { PAY_TO: "0x1234" }),
    environment("eip155:8453", { ASSET_EIP712_NAME: "USDC" }),
    environment("eip155:8453", { ASSET_EIP712_VERSION: "1" }),
  ];
  for (const candidate of invalid) {
    assert.throws(
      () => loadConfig(candidate, { credentialReader }),
      /NETWORK|ASSET|PAY_TO|EIP712/,
    );
  }
});

test("validates URLs, amount, and port before startup", () => {
  const invalid = [
    { FACILITATOR_URL: "http://facilitator.example" },
    { FACILITATOR_URL: "https://user:secret@facilitator.example" },
    { RESOURCE_ORIGIN: "https://merchant.example/path" },
    { RPC_URL: "https://rpc.example/#fragment" },
    { ONE_CLICK_PROVIDER_ORIGIN: "https://quotes.example/v1" },
    { EXPLORER_BASE_URL: "javascript:alert(1)" },
    { EXPLORER_BASE_URL: "https://basescan.org/?tracking=untrusted" },
    { AMOUNT: "0" },
    { AMOUNT: "1.0" },
    { AMOUNT: "12345678901234567" },
    { PORT: "0" },
    { PORT: "65536" },
    { PORT: "4031.5" },
  ];
  for (const overrides of invalid) {
    assert.throws(
      () => loadConfig(
        environment("eip155:8453", overrides),
        { credentialReader },
      ),
    );
  }
});

test("formats atomic USDC without floating point conversion", () => {
  assert.equal(formatUsdc("1"), "0.000001");
  assert.equal(formatUsdc("1000"), "0.001000");
  assert.equal(formatUsdc("1000000"), "1.000000");
  assert.equal(formatUsdc("123456789"), "123.456789");
  assert.throws(() => formatUsdc("1.5"), /positive atomic/);
});

test("reads an owner-only, bounded, single-line credential", t => {
  const directory = mkdtempSync(join(tmpdir(), "x402-merchant-config-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const path = join(directory, "credential");
  writeFileSync(path, "secret-value\n", { mode: 0o600 });
  assert.equal(readCredential(path, "test credential"), "secret-value");

  chmodSync(path, 0o640);
  assert.throws(
    () => readCredential(path, "test credential"),
    /group or others/,
  );

  chmodSync(path, 0o440);
  assert.throws(
    () => readCredential(path, "test credential", { expectedSystemdPath: path }),
    /group or others/,
  );
});

test("accepts only the expected systemd LoadCredential ACL-mask metadata", () => {
  const directory = "/run/credentials/x402-merchant-api@near.service";
  const expectedPath = expectedSystemdCredentialPath(
    directory,
    "facilitator-api-key",
  );
  const metadata = {
    mode: 0o100440,
    uid: 0,
    gid: 0,
  };

  assert.equal(
    expectedPath,
    "/run/credentials/x402-merchant-api@near.service/facilitator-api-key",
  );
  assert.equal(
    expectedSystemdCredentialPath("/tmp/credentials", "facilitator-api-key"),
    undefined,
  );
  assert.equal(expectedSystemdCredentialPath(directory, ""), undefined);
  assert.equal(expectedSystemdCredentialPath(directory, ".."), undefined);
  assert.equal(expectedSystemdCredentialPath(directory, "nested/key"), undefined);
  assert.equal(
    isExpectedSystemdAclCredential(expectedPath, expectedPath, metadata),
    true,
  );
  assert.equal(
    isExpectedSystemdAclCredential(
      "/tmp/facilitator-api-key",
      "/tmp/facilitator-api-key",
      metadata,
    ),
    false,
  );

  for (const candidate of [
    { path: "/tmp/facilitator-api-key" },
    { expectedSystemdPath: undefined },
    { metadata: { ...metadata, mode: 0o100444 } },
    { metadata: { ...metadata, mode: 0o100460 } },
    { metadata: { ...metadata, mode: 0o100640 } },
    { metadata: { ...metadata, uid: 1000 } },
    { metadata: { ...metadata, gid: 1000 } },
  ]) {
    const candidateExpectedSystemdPath = Object.hasOwn(
      candidate,
      "expectedSystemdPath",
    )
      ? candidate.expectedSystemdPath
      : expectedPath;
    assert.equal(
      isExpectedSystemdAclCredential(
        candidate.path ?? expectedPath,
        candidateExpectedSystemdPath,
        candidate.metadata ?? metadata,
      ),
      false,
    );
  }
});

test("rejects credential symlinks, extra lines, whitespace, and oversized files", t => {
  const directory = mkdtempSync(join(tmpdir(), "x402-merchant-config-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const target = join(directory, "target");
  const link = join(directory, "link");
  writeFileSync(target, "secret\n", { mode: 0o600 });
  symlinkSync(target, link);
  assert.throws(() => readCredential(link, "test credential"), /symbolic link/);

  for (const [contents, pattern] of [
    ["first\nsecond\n", /exactly one/],
    [" secret\n", /surrounding whitespace/],
    ["x".repeat(4097), /4096/],
  ]) {
    writeFileSync(target, contents, { mode: 0o600 });
    assert.throws(() => readCredential(target, "test credential"), pattern);
  }
});

test("reads only a root-owned immutable release metadata sidecar", () => {
  const contents = `${RELEASE_ID}\n`;
  const metadata = {
    isFile: () => true,
    uid: 0,
    gid: 0,
    mode: 0o100444,
    size: Buffer.byteLength(contents),
  };
  let openedPath;
  let closed = false;
  assert.equal(
    readInstalledReleaseId({
      moduleUrl: "file:///unused/config.mjs",
      moduleUrlToPath: () => "/opt/x402-merchant/releases/current/config.mjs",
      openFile: path => {
        openedPath = path;
        return 7;
      },
      fstat: () => metadata,
      readFile: () => contents,
      close: descriptor => {
        assert.equal(descriptor, 7);
        closed = true;
      },
    }),
    RELEASE_ID,
  );
  assert.equal(
    openedPath,
    "/opt/x402-merchant/releases/current/.x402-merchant-release-id",
  );
  assert.equal(closed, true);
});

test("fails closed on malformed, unsafe, or missing required release metadata", () => {
  const safeMetadata = {
    isFile: () => true,
    uid: 0,
    gid: 0,
    mode: 0o100444,
    size: Buffer.byteLength(`${RELEASE_ID}\n`),
  };
  const reader = ({ metadata = safeMetadata, contents = `${RELEASE_ID}\n`, openError } = {}) => () => readInstalledReleaseId({
    moduleUrl: "file:///unused/config.mjs",
    moduleUrlToPath: () => "/opt/x402-merchant/releases/current/config.mjs",
    openFile: () => {
      if (openError) throw openError;
      return 7;
    },
    fstat: () => metadata,
    readFile: () => contents,
    close: () => {},
  });

  const missing = new Error("missing");
  missing.code = "ENOENT";
  assert.equal(reader({ openError: missing })(), undefined);

  const symlink = new Error("link");
  symlink.code = "ELOOP";
  assert.throws(reader({ openError: symlink }), /symbolic link/);

  for (const [metadata, contents, pattern] of [
    [{ ...safeMetadata, isFile: () => false }, `${RELEASE_ID}\n`, /regular file/],
    [{ ...safeMetadata, uid: 1000 }, `${RELEASE_ID}\n`, /root-owned and immutable/],
    [{ ...safeMetadata, gid: 1000 }, `${RELEASE_ID}\n`, /root-owned and immutable/],
    [{ ...safeMetadata, mode: 0o100644 }, `${RELEASE_ID}\n`, /root-owned and immutable/],
    [{ ...safeMetadata, mode: 0o100664 }, `${RELEASE_ID}\n`, /root-owned and immutable/],
    [{ ...safeMetadata, size: 129 }, `${RELEASE_ID}\n`, /1 through 128 bytes/],
    [safeMetadata, "git-not-a-commit\n", /invalid release ID/],
    [safeMetadata, `${RELEASE_ID}\nextra\n`, /exactly one nonempty line/],
    [safeMetadata, ` ${RELEASE_ID}\n`, /invalid release ID/],
  ]) {
    assert.throws(reader({ metadata, contents }), pattern);
  }
});
