import assert from "node:assert/strict";
import test from "node:test";

import {
  buildUnpaidHintBody,
  buildV1PaymentRequiredBody,
  buildV1Requirements,
  caip2ForV1Network,
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
