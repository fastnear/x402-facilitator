import assert from "node:assert/strict";
import test from "node:test";

import {
  PaidFlowError,
  assertExpectedRequirement,
  discoverPaymentRequired,
  submitPaidRequest,
  summarizePaymentRequired,
} from "./paid-flow.mjs";

const paymentRequired = {
  x402Version: 2,
  accepts: [{
    scheme: "exact",
    network: "eip155:8453",
    asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    amount: "1000",
    payTo: "0x7Ff46ab88688D528bCE3e59c470240c6901cF88c",
  }],
};

function fakeHttp(settlement = {
  success: true,
  network: "eip155:8453",
  transaction: "0x" + "11".repeat(32),
}) {
  return {
    getPaymentRequiredResponse: () => paymentRequired,
    createPaymentPayload: async () => ({ sensitive: "not-output" }),
    encodePaymentSignatureHeader: () => ({ "PAYMENT-SIGNATURE": "sensitive" }),
    getPaymentSettleResponse: () => settlement,
  };
}

function jsonResponse(status, body, headers = {}) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...headers },
  });
}

test("payment requirement summary is strict canonical v2", () => {
  assert.deepEqual(summarizePaymentRequired(paymentRequired), {
    x402Version: 2,
    scheme: "exact",
    network: "eip155:8453",
    asset: paymentRequired.accepts[0].asset,
    amount: "1000",
    payTo: paymentRequired.accepts[0].payTo,
  });
  assert.throws(
    () => summarizePaymentRequired({ ...paymentRequired, x402Version: 1 }),
    PaidFlowError,
  );
});

test("expected requirement comparison permits EVM checksum differences", () => {
  const summary = summarizePaymentRequired(paymentRequired);
  assert.doesNotThrow(() => assertExpectedRequirement(summary, {
    network: "eip155:8453",
    asset: paymentRequired.accepts[0].asset.toLowerCase(),
    amount: "1000",
    payTo: paymentRequired.accepts[0].payTo.toLowerCase(),
  }));
  assert.throws(
    () => assertExpectedRequirement(summary, {
      network: "eip155:8453",
      asset: paymentRequired.accepts[0].asset,
      amount: "2000",
      payTo: paymentRequired.accepts[0].payTo,
    }),
    error => error.stage === "requirement_mismatch",
  );
});

test("unpaid discovery requires 402 and decodes the requirement", async () => {
  const result = await discoverPaymentRequired({
    fetchImpl: async () => jsonResponse(402, { error: "payment required" }),
    httpClient: fakeHttp(),
    request: { url: "https://merchant.example/resource", method: "POST" },
  });
  assert.equal(result.requirement.amount, "1000");
  assert.equal(result.unpaidResponse.status, 402);
});

test("successful paid request records settlement and response body", async () => {
  const records = [];
  let calls = 0;
  const result = await submitPaidRequest({
    attemptId: "attempt-1",
    fetchImpl: async (_url, init) => {
      calls += 1;
      assert.equal(init.headers["PAYMENT-SIGNATURE"], "sensitive");
      return jsonResponse(200, { network: "eip155:8453", kind: "account" }, {
        "payment-response": "encoded",
      });
    },
    httpClient: fakeHttp(),
    paymentRequired,
    preview: { network: "eip155:8453" },
    record: async value => records.push(value),
    request: {
      url: "https://merchant.example/resource",
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    },
  });
  assert.equal(calls, 1);
  assert.equal(result.outcome, "settled");
  assert.equal(result.response.status, 200);
  assert.equal(result.settlement.success, true);
  assert.deepEqual(records.map(value => value.outcome), ["broadcasting", "settled"]);
  assert.equal(JSON.stringify(records).includes("not-output"), false);
  assert.equal(JSON.stringify(records).includes("PAYMENT-SIGNATURE"), false);
});

test("transport failure after checkpoint is indeterminate and is never retried", async () => {
  const records = [];
  let calls = 0;
  const result = await submitPaidRequest({
    attemptId: "attempt-2",
    fetchImpl: async () => {
      calls += 1;
      throw new Error("connection closed");
    },
    httpClient: fakeHttp(),
    paymentRequired,
    preview: { network: "eip155:8453" },
    record: async value => records.push(value),
    request: { url: "https://merchant.example/resource", method: "POST" },
  });
  assert.equal(calls, 1);
  assert.equal(result.outcome, "indeterminate");
  assert.equal(result.reconcileBeforeRetry, true);
  assert.deepEqual(records.map(value => value.outcome), ["broadcasting", "indeterminate"]);
});

test("missing settlement header is indeterminate even when resource returns 200", async () => {
  const result = await submitPaidRequest({
    attemptId: "attempt-3",
    fetchImpl: async () => jsonResponse(200, { ok: true }),
    httpClient: fakeHttp(),
    paymentRequired,
    preview: { network: "eip155:8453" },
    request: { url: "https://merchant.example/resource", method: "POST" },
  });
  assert.equal(result.outcome, "indeterminate");
  assert.equal(result.response.status, 200);
  assert.equal(result.settlementHeaderPresent, false);
});

test("settled payment with a resource failure is not safe to retry", async () => {
  const result = await submitPaidRequest({
    attemptId: "attempt-4",
    fetchImpl: async () => jsonResponse(503, { error: "rpc unavailable" }, {
      "payment-response": "encoded",
    }),
    httpClient: fakeHttp(),
    paymentRequired,
    preview: { network: "eip155:8453" },
    request: { url: "https://merchant.example/resource", method: "POST" },
  });
  assert.equal(result.outcome, "settled_resource_error");
  assert.equal(result.settlement.success, true);
});

test("final result persistence failure leaves an explicit reconciliation warning", async () => {
  let writes = 0;
  const result = await submitPaidRequest({
    attemptId: "attempt-5",
    fetchImpl: async () => jsonResponse(200, { ok: true }, {
      "payment-response": "encoded",
    }),
    httpClient: fakeHttp(),
    paymentRequired,
    preview: { network: "eip155:8453" },
    record: async () => {
      writes += 1;
      if (writes === 2) throw new Error("disk full");
    },
    request: { url: "https://merchant.example/resource", method: "POST" },
  });
  assert.equal(result.outcome, "settled");
  assert.equal(result.resultPersistenceError, "disk full");
  assert.equal(result.reconcileBeforeRetry, true);
});

test("malformed successful settlement is indeterminate", async () => {
  const result = await submitPaidRequest({
    attemptId: "attempt-6",
    fetchImpl: async () => jsonResponse(200, { ok: true }, {
      "payment-response": "encoded",
    }),
    httpClient: fakeHttp({ success: true, network: "eip155:8453" }),
    paymentRequired,
    preview: { network: "eip155:8453" },
    request: { url: "https://merchant.example/resource", method: "POST" },
  });
  assert.equal(result.outcome, "indeterminate");
  assert.match(result.settlementDecodeError, /incomplete/);
});
