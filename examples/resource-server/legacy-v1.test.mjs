import assert from "node:assert/strict";
import test from "node:test";

import {
  buildUnpaidHintBody,
  buildV1PaymentRequiredBody,
  buildV1Requirements,
  caip2ForV1Network,
  translateSettleHeaderToV1,
  translateV1PaymentToV2,
  v1NetworkName,
} from "./legacy-v1.mjs";

const baseInputs = Object.freeze({
  network: "eip155:8453",
  asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
  payTo: "0x7Ff46ab88688D528bCE3e59c470240c6901cF88c",
  amount: "1000",
  resourceUrl: "https://x402-demo-base.mikedotexe.com/work",
  description: "Deterministic paid work with independent delivery deduplication",
  mimeType: "application/json",
  extra: { name: "USD Coin", version: "2" },
});

test("v1 network names round-trip for the supported eip155 chains", () => {
  assert.equal(v1NetworkName("eip155:8453"), "base");
  assert.equal(v1NetworkName("eip155:84532"), "base-sepolia");
  assert.equal(caip2ForV1Network("base"), "eip155:8453");
  assert.equal(caip2ForV1Network("base-sepolia"), "eip155:84532");
  assert.equal(v1NetworkName("near:mainnet"), undefined);
  assert.equal(caip2ForV1Network("eip155:8453"), undefined);
});

test("buildV1Requirements emits the exact legacy Base-mainnet shape", () => {
  assert.deepEqual(buildV1Requirements(baseInputs), {
    scheme: "exact",
    network: "base",
    maxAmountRequired: "1000",
    resource: "https://x402-demo-base.mikedotexe.com/work",
    description: "Deterministic paid work with independent delivery deduplication",
    mimeType: "application/json",
    payTo: "0x7Ff46ab88688D528bCE3e59c470240c6901cF88c",
    maxTimeoutSeconds: 300,
    asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    extra: { name: "USD Coin", version: "2" },
  });
});

test("buildV1Requirements refuses networks without a v1 name or a missing resource URL", () => {
  assert.equal(buildV1Requirements({ ...baseInputs, network: "near:mainnet" }), undefined);
  assert.equal(buildV1Requirements({ ...baseInputs, network: "near:testnet" }), undefined);
  assert.equal(buildV1Requirements({ ...baseInputs, resourceUrl: undefined }), undefined);
});

test("v1 payment-required bodies are frozen and v1-shaped", () => {
  const requirements = buildV1Requirements(baseInputs);
  const body = buildV1PaymentRequiredBody(requirements);
  assert.deepEqual(Object.keys(body), ["x402Version", "error", "accepts"]);
  assert.equal(body.x402Version, 1);
  assert.equal(body.error, "Payment required");
  assert.equal(body.accepts.length, 1);
  assert.equal(body.accepts[0], requirements);
  assert.ok(Object.isFrozen(body));
  assert.ok(Object.isFrozen(body.accepts));
  assert.ok(Object.isFrozen(requirements));
  assert.ok(Object.isFrozen(requirements.extra));
  assert.equal(buildV1PaymentRequiredBody(requirements, "settle_failed").error, "settle_failed");
});

test("the hint body points at the v2 header and never claims to be v1", () => {
  const body = buildUnpaidHintBody();
  assert.ok(Object.isFrozen(body));
  assert.equal(body.x402Version, undefined);
  assert.equal(body.error, "Payment required");
  assert.match(body.hint, /PAYMENT-REQUIRED/);
  assert.match(body.hint, /PAYMENT-SIGNATURE/);
});

// The v2 requirements object exactly as index.mjs injects it as `accepted`,
// and as @x402/core@2.19.0 buildPaymentRequirements computes it for the
// route (server/index.js:1080-1092): core keys scheme, network, amount,
// asset, payTo, maxTimeoutSeconds — deep-equality checked by the matcher at
// server/index.js:1847 — plus extra as a superset of the route's extra.
const requirementsV2 = Object.freeze({
  scheme: "exact",
  network: "eip155:8453",
  amount: "1000",
  asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
  payTo: "0x7Ff46ab88688D528bCE3e59c470240c6901cF88c",
  maxTimeoutSeconds: 300,
  extra: Object.freeze({ name: "USD Coin", version: "2" }),
});

const resourceObject = Object.freeze({
  url: "https://x402-demo-base.mikedotexe.com/work",
  description: "Deterministic paid work with independent delivery deduplication",
  mimeType: "application/json",
});

const v1Payment = Object.freeze({
  x402Version: 1,
  scheme: "exact",
  network: "base",
  payload: Object.freeze({
    signature: `0x${"ab".repeat(65)}`,
    authorization: Object.freeze({
      from: "0x150B4b68F0Aa687a70d2383A88A5294E6077296E",
      to: "0x7Ff46ab88688D528bCE3e59c470240c6901cF88c",
      value: "1000",
      validAfter: "1753500000",
      validBefore: "1753500900",
      nonce: `0x${"cd".repeat(32)}`,
    }),
  }),
});

test("translateV1PaymentToV2 emits the v2 wire shape with the inner payload verbatim", () => {
  const translated = translateV1PaymentToV2(v1Payment, { requirementsV2, resourceObject });
  assert.deepEqual(translated, {
    x402Version: 2,
    resource: resourceObject,
    accepted: requirementsV2,
    payload: v1Payment.payload,
  });
  assert.equal(translated.payload, v1Payment.payload);
  assert.equal(translated.accepted, requirementsV2);
  assert.equal("extensions" in translated, false);
});

test("translateV1PaymentToV2 omits resource when none is configured", () => {
  const translated = translateV1PaymentToV2(v1Payment, {
    requirementsV2,
    resourceObject: undefined,
  });
  assert.equal("resource" in translated, false);
});

test("translateV1PaymentToV2 rejects anything that is not a v1 exact payment for the route", () => {
  const reject = payload =>
    assert.equal(translateV1PaymentToV2(payload, { requirementsV2, resourceObject }), undefined);
  reject(null);
  reject("payment");
  reject([]);
  reject({ ...v1Payment, x402Version: 2 });
  reject({ ...v1Payment, scheme: "permit2" });
  reject({ ...v1Payment, network: "base-sepolia" });
  reject({ ...v1Payment, network: "solana" });
  reject({ ...v1Payment, network: "eip155:8453" });
  reject({ ...v1Payment, payload: undefined });
  reject({ ...v1Payment, payload: null });
  reject({ ...v1Payment, payload: [] });
});

test("translateSettleHeaderToV1 rewrites the network to the v1 name", () => {
  const settle = {
    success: true,
    transaction: `0x${"12".repeat(32)}`,
    network: "eip155:8453",
    payer: "0x150B4b68F0Aa687a70d2383A88A5294E6077296E",
  };
  const translated = JSON.parse(
    Buffer.from(
      translateSettleHeaderToV1(Buffer.from(JSON.stringify(settle)).toString("base64")),
      "base64",
    ).toString("utf8"),
  );
  assert.deepEqual(translated, { ...settle, network: "base" });
});

test("translateSettleHeaderToV1 leaves unknown networks untouched and throws on garbage", () => {
  const settle = { success: false, errorReason: "settle_failed", network: "near:mainnet" };
  const translated = JSON.parse(
    Buffer.from(
      translateSettleHeaderToV1(Buffer.from(JSON.stringify(settle)).toString("base64")),
      "base64",
    ).toString("utf8"),
  );
  assert.deepEqual(translated, settle);
  assert.throws(() => translateSettleHeaderToV1("!!!not-base64!!!"));
});
