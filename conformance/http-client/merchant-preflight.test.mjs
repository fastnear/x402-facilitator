import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { PaymentRequiredV2Schema } from "@x402/core/schemas";
import { inspectMerchantOffer, PreflightError, runPreflight } from "./merchant-preflight.mjs";

const profiles = [
  ["near:testnet", "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af", "merchant.testnet"],
  ["near:mainnet", "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1", "merchant.near"],
  ["eip155:8453", "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", "0x1111111111111111111111111111111111111111", "USD Coin"],
  ["eip155:84532", "0x036CbD53842c5426634e7929541eC2318f3dCF7e", "0x1111111111111111111111111111111111111111", "USDC"],
];
function fixture([network, asset, payTo, name] = profiles[0]) {
  // Unsigned synthetic offers only. No keys, signatures or authorizations.
  const requirements = { scheme: "exact", network, asset, amount: "1000", payTo, maxTimeoutSeconds: 60,
    ...(name ? { extra: { name, version: "2" } } : {}) };
  const expected = { resourceUrl: "https://merchant.example/inventory", requirements };
  const challenge = { x402Version: 2, resource: { url: expected.resourceUrl, description: "Inventory" }, accepts: [structuredClone(requirements)] };
  return { challenge, expected };
}
function rejects(challenge, expected, code) {
  assert.throws(() => inspectMerchantOffer(challenge, expected), error => error instanceof PreflightError && error.code === code);
}

for (const profile of profiles) {
  test(`canonical ${profile[0]} offer passes without any network capability`, () => {
    const { challenge, expected } = fixture(profile);
    const original = globalThis.fetch;
    globalThis.fetch = () => { throw new Error("unexpected network request"); };
    try {
      assert.deepEqual(inspectMerchantOffer(challenge, expected), {
        ok: true, scope: "unsigned_offer_only", network: profile[0], offerIndex: 0,
      });
    } finally { globalThis.fetch = original; }
  });
}

test("unknown requirement fields survive the permissive upstream parser and must be rejected", () => {
  for (const field of ["recipient", "description"]) {
    const { challenge, expected } = fixture();
    challenge.accepts[0][field] = "untrusted metadata";
    assert.equal(PaymentRequiredV2Schema.safeParse(challenge).success, true);
    rejects(challenge, expected, "unsupported_fields");
  }
});

test("approved terms cannot be silently reconstructed from a different offer", () => {
  for (const field of ["amount", "payTo", "maxTimeoutSeconds", "extra"]) {
    const { challenge, expected } = fixture();
    challenge.accepts[0][field] = { amount: "1001", payTo: "other.testnet", maxTimeoutSeconds: 61, extra: { note: "changed" } }[field];
    rejects(challenge, expected, "terms_mismatch");
  }
});

test("resource identity must exactly match the reviewed URL", () => {
  const { challenge, expected } = fixture();
  challenge.resource.url += "?different=1";
  rejects(challenge, expected, "resource_mismatch");
});

test("separate networks in a multirail offer do not authorize a second same-network choice", () => {
  const { challenge, expected } = fixture();
  challenge.accepts.unshift(fixture(profiles[2]).challenge.accepts[0]);
  assert.equal(inspectMerchantOffer(challenge, expected).offerIndex, 1);
  challenge.accepts.push(structuredClone(challenge.accepts[1]));
  rejects(challenge, expected, "ambiguous_offer");
});

test("an explicit reviewed index selects only that offer in a same-network multirail challenge", () => {
  const { challenge, expected } = fixture();
  challenge.accepts.unshift({ ...challenge.accepts[0], amount: "2000" });
  expected.offerIndex = 1;
  assert.equal(inspectMerchantOffer(challenge, expected).offerIndex, 1);
  expected.offerIndex = 0;
  rejects(challenge, expected, "terms_mismatch");
  expected.offerIndex = 99;
  rejects(challenge, expected, "invalid_offer_index");
});

test("a missing selected network is not mistaken for a matching multirail offer", () => {
  const { challenge, expected } = fixture();
  challenge.accepts = fixture(profiles[2]).challenge.accepts;
  rejects(challenge, expected, "missing_offer");
});

test("amounts remain lossless decimal strings above the JavaScript integer range", () => {
  const { challenge, expected } = fixture();
  expected.requirements.amount = "9007199254740993123456789";
  challenge.accepts[0].amount = expected.requirements.amount;
  assert.equal(inspectMerchantOffer(challenge, expected).ok, true);
  for (const value of [1000, "1e3", "1.0", "-1", "", "0"]) {
    challenge.accepts[0].amount = value;
    assert.throws(() => inspectMerchantOffer(challenge, expected), PreflightError);
  }
});

test("fractional and unsafe timeouts accepted by upstream are rejected at our boundary", () => {
  for (const value of [0.5, Number.MAX_SAFE_INTEGER + 1]) {
    const { challenge, expected } = fixture();
    challenge.accepts[0].maxTimeoutSeconds = value;
    assert.equal(PaymentRequiredV2Schema.safeParse(challenge).success, true);
    rejects(challenge, expected, "positive_integer_timeout_required");
  }
});

test("canonical mainnet and testnet assets cannot be swapped even in approved input", () => {
  for (const [network, asset, payTo, name] of profiles) {
    const { challenge, expected } = fixture([network, asset, payTo, name]);
    expected.requirements.asset = network.startsWith("near:") ? profiles[network === "near:mainnet" ? 0 : 1][1] : profiles[network === "eip155:8453" ? 3 : 2][1];
    rejects(challenge, expected, "noncanonical_asset");
  }
});

test("Base mainnet uses the token's real EIP-712 domain, not its symbol", () => {
  for (const [name, version] of [["USDC", "2"], ["USD Coin", "1"]]) {
    const { challenge, expected } = fixture(profiles[2]);
    expected.requirements.extra = { name, version };
    rejects(challenge, expected, "noncanonical_usdc_domain");
  }
});

test("upfront flow is outside this deployment on both chain families", () => {
  for (const profile of [profiles[0], profiles[2]]) {
    const { challenge, expected } = fixture(profile);
    expected.requirements.extra = { ...expected.requirements.extra, paymentFlow: "upfront" };
    rejects(challenge, expected, "unsupported_payment_flow");
  }
});

test("any explicit NEAR transfer selector stays outside the classic delegate policy", () => {
  for (const selector of ["delegate", "default", "intents"]) {
    const { challenge, expected } = fixture();
    expected.requirements.extra = { assetTransferMethod: selector };
    rejects(challenge, expected, "unsupported_near_transfer_selector");
  }
});

test("nullable and unknown forwarded metadata are not stripped into a passing offer", () => {
  for (const mutate of [c => { c.accepts[0].extra = null; }, c => { c.resource.description = null; },
    c => { c.resource.recipient = "other.testnet"; }, c => { c.extensions = null; }]) {
    const { challenge, expected } = fixture(); mutate(challenge);
    assert.equal(PaymentRequiredV2Schema.safeParse(challenge).success, true);
    assert.throws(() => inspectMerchantOffer(challenge, expected), PreflightError);
  }
});

test("a v2 challenge must actually contain its required resource object", () => {
  const { challenge, expected } = fixture();
  delete challenge.resource;
  rejects(challenge, expected, "invalid_x402_v2_offer");
});

test("credentialed URLs, legacy v1 and payment-bearing input fail closed", () => {
  const { challenge, expected } = fixture();
  expected.resourceUrl = "https://user:PRIVATE_SENTINEL@merchant.example/work";
  rejects(challenge, expected, "https_url_required");
  const valid = fixture();
  rejects({ ...valid.challenge, x402Version: 1 }, valid.expected, "invalid_x402_v2_offer");
  rejects({ ...valid.challenge, paymentPayload: { signature: "PRIVATE_SENTINEL" } }, valid.expected, "unsupported_fields");
});

test("CLI errors never echo untrusted values, file paths or parser exceptions", async () => {
  const dir = await mkdtemp(join(tmpdir(), "x402-offer-"));
  const path = join(dir, "PRIVATE_SENTINEL.json");
  const writes = [];
  const sink = { write(value) { writes.push(value); } };
  try {
    await writeFile(path, '{"PRIVATE_SENTINEL": invalid');
    assert.equal(await runPreflight([path, path], sink, sink), 1);
    assert.equal(writes.join("").includes("PRIVATE_SENTINEL"), false);
    assert.equal(JSON.parse(writes.join("")).code, "invalid_or_unreadable_json");
    writes.length = 0;
    await writeFile(path, " ".repeat(64 * 1024 + 1));
    assert.equal(await runPreflight([path, path], sink, sink), 1);
    assert.equal(JSON.parse(writes.join("")).code, "input_too_large");
    writes.length = 0;
    const { challenge, expected } = fixture();
    await writeFile(path, JSON.stringify(challenge));
    const approval = join(dir, "expected.json");
    await writeFile(approval, JSON.stringify(expected));
    assert.equal(await runPreflight([path, approval], sink, sink), 0);
    assert.equal(JSON.parse(writes.join("")).scope, "unsigned_offer_only");
  } finally { await rm(dir, { recursive: true, force: true }); }
});
