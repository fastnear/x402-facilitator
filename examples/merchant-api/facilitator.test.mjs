import assert from "node:assert/strict";
import test from "node:test";

import {
  createFacilitatorProbe,
  withFacilitatorRetries,
  withRetries,
} from "./facilitator.mjs";

function recordingSleep(record) {
  return milliseconds => {
    record.push(milliseconds);
    return Promise.resolve();
  };
}

function manualTimers() {
  const callbacks = [];
  return {
    callbacks,
    setTimeoutImpl(callback) {
      callbacks.push(callback);
      return callback;
    },
    clearTimeoutImpl() {},
  };
}

test("withRetries is bounded and uses the configured delays", async () => {
  const slept = [];
  let calls = 0;
  const result = await withRetries(
    async () => {
      calls += 1;
      if (calls < 3) throw new Error("transient");
      return "ok";
    },
    { retries: 2, delaysMs: [10, 20], sleep: recordingSleep(slept) },
  );
  assert.equal(result, "ok");
  assert.equal(calls, 3);
  assert.deepEqual(slept, [10, 20]);

  await assert.rejects(
    withRetries(
      async () => {
        throw new Error("last");
      },
      { retries: 1, delaysMs: [5], sleep: recordingSleep(slept) },
    ),
    /last/,
  );
});

test("facilitator wrapper retries throws but not resolved protocol failures", async () => {
  const slept = [];
  let verifyCalls = 0;
  let settleCalls = 0;
  const client = {
    async verify() {
      verifyCalls += 1;
      if (verifyCalls === 1) throw new Error("temporarily unavailable");
      return { isValid: false, invalidReason: "insufficient_funds" };
    },
    async settle() {
      settleCalls += 1;
      if (settleCalls < 3) throw new Error("settlement pending");
      return { success: false, errorReason: "invalid_payment" };
    },
  };
  withFacilitatorRetries(client, { sleep: recordingSleep(slept) });

  assert.deepEqual(
    await client.verify({}, {}),
    { isValid: false, invalidReason: "insufficient_funds" },
  );
  assert.deepEqual(
    await client.settle({}, {}),
    { success: false, errorReason: "invalid_payment" },
  );
  assert.equal(verifyCalls, 2);
  assert.equal(settleCalls, 3);
  assert.deepEqual(slept, [1000, 1500, 3000]);
});

test("facilitator wrapper retries only typed transient HTTP errors", async () => {
  for (const { statusCode, calls: expectedCalls, delays } of [
    { statusCode: 400, calls: 1, delays: [] },
    { statusCode: 404, calls: 1, delays: [] },
    { statusCode: 429, calls: 2, delays: [1000] },
    { statusCode: 503, calls: 2, delays: [1000] },
  ]) {
    const slept = [];
    let verifyCalls = 0;
    const typedError = Object.assign(new Error(`HTTP ${statusCode}`), { statusCode });
    const client = {
      async verify() {
        verifyCalls += 1;
        throw typedError;
      },
      async settle() {
        throw typedError;
      },
    };
    withFacilitatorRetries(client, { sleep: recordingSleep(slept) });
    await assert.rejects(() => client.verify({}, {}), error => error === typedError);
    assert.equal(verifyCalls, expectedCalls, `HTTP ${statusCode}`);
    assert.deepEqual(slept, delays, `HTTP ${statusCode}`);
  }
});

test("facilitator wrapper bounds getSupported for startup callers", async () => {
  const timers = manualTimers();
  const client = {
    async verify() {},
    async settle() {},
    async getSupported() {
      return new Promise(() => {});
    },
  };
  withFacilitatorRetries(client, timers);
  const supported = client.getSupported();
  assert.equal(timers.callbacks.length, 1);
  timers.callbacks[0]();
  await assert.rejects(supported, /supported-kinds request timed out/);
});

test("facilitator readiness requires the configured canonical kind", async () => {
  const probe = createFacilitatorProbe({
    network: "eip155:8453",
    facilitatorUrl: "https://facilitator.example",
    client: {
      async getSupported() {
        return {
          kinds: [{
            x402Version: 2,
            scheme: "exact",
            network: "eip155:8453",
          }],
        };
      },
    },
    fetchImpl: async url => {
      assert.equal(url, "https://facilitator.example/readyz");
      return Response.json({ ready: true });
    },
  });
  assert.deepEqual(
    await probe.check(),
    { network: "eip155:8453", ready: true },
  );
});

test("facilitator readiness fails closed on wrong identity or unavailable state", async () => {
  for (const { supported, response, pattern } of [
    {
      supported: { kinds: [{ x402Version: 2, scheme: "exact", network: "near:mainnet" }] },
      response: Response.json({ ready: true }),
      pattern: /does not advertise/,
    },
    {
      supported: { kinds: [{ x402Version: 2, scheme: "exact", network: "eip155:8453" }] },
      response: Response.json({ ready: false }, { status: 503 }),
      pattern: /HTTP 503/,
    },
  ]) {
    const probe = createFacilitatorProbe({
      network: "eip155:8453",
      facilitatorUrl: "https://facilitator.example",
      client: { getSupported: async () => supported },
      fetchImpl: async () => response,
    });
    await assert.rejects(() => probe.check(), pattern);
  }
});

test("facilitator readiness bounds either hanging dependency deterministically", async () => {
  for (const dependency of ["supported", "readyz"]) {
    const timers = manualTimers();
    let readyzSignal;
    const probe = createFacilitatorProbe({
      network: "eip155:8453",
      facilitatorUrl: "https://facilitator.example",
      client: {
        async getSupported() {
          if (dependency === "supported") return new Promise(() => {});
          return {
            kinds: [{
              x402Version: 2,
              scheme: "exact",
              network: "eip155:8453",
            }],
          };
        },
      },
      fetchImpl: async (_url, request) => {
        readyzSignal = request.signal;
        if (dependency === "readyz") return new Promise(() => {});
        return Response.json({ ready: true });
      },
      ...timers,
    });
    const checking = probe.check();
    await Promise.resolve();
    assert.equal(timers.callbacks.length, 2);
    timers.callbacks[dependency === "supported" ? 0 : 1]();
    await assert.rejects(checking, /facilitator readiness timed out/);
    if (dependency === "readyz") assert.equal(readyzSignal?.aborted, true);
  }
});
