import assert from "node:assert/strict";
import test from "node:test";

import { withFacilitatorRetries, withRetries } from "./retry.mjs";

function recordingSleep(record) {
  return ms => {
    record.push(ms);
    return Promise.resolve();
  };
}

test("withRetries returns the first success without sleeping", async () => {
  const slept = [];
  let calls = 0;
  const result = await withRetries(
    async () => {
      calls += 1;
      return "ok";
    },
    { retries: 2, delaysMs: [10, 20], sleep: recordingSleep(slept) },
  );
  assert.equal(result, "ok");
  assert.equal(calls, 1);
  assert.deepEqual(slept, []);
});

test("withRetries retries through transient throws with the configured backoff", async () => {
  const slept = [];
  let calls = 0;
  const result = await withRetries(
    async () => {
      calls += 1;
      if (calls < 3) {
        throw new Error("transient");
      }
      return calls;
    },
    { retries: 2, delaysMs: [10, 20], sleep: recordingSleep(slept) },
  );
  assert.equal(result, 3);
  assert.deepEqual(slept, [10, 20]);
});

test("withRetries is bounded and rethrows the last error", async () => {
  const slept = [];
  let calls = 0;
  await assert.rejects(
    withRetries(
      async () => {
        calls += 1;
        throw new Error(`attempt ${calls}`);
      },
      { retries: 2, delaysMs: [10, 20], sleep: recordingSleep(slept) },
    ),
    /attempt 3/,
  );
  assert.equal(calls, 3);
});

test("withFacilitatorRetries retries settle throws but never protocol failures", async () => {
  const slept = [];
  let settleCalls = 0;
  let verifyCalls = 0;
  const client = {
    verify(payload, requirements) {
      verifyCalls += 1;
      assert.equal(requirements, "requirements");
      if (verifyCalls === 1) {
        throw new Error("rpc_unavailable");
      }
      return Promise.resolve({ isValid: false, invalidReason: "insufficient_funds" });
    },
    settle() {
      settleCalls += 1;
      if (settleCalls < 3) {
        throw new Error("settlement_pending");
      }
      return Promise.resolve({ success: true, transaction: "0xabc" });
    },
  };
  withFacilitatorRetries(client, { sleep: recordingSleep(slept) });

  // A resolved protocol rejection is a result, not a throw: returned as-is
  // after the one transient retry, never retried itself.
  const verdict = await client.verify("payload", "requirements");
  assert.deepEqual(verdict, { isValid: false, invalidReason: "insufficient_funds" });
  assert.equal(verifyCalls, 2);

  const settled = await client.settle("payload", "requirements");
  assert.deepEqual(settled, { success: true, transaction: "0xabc" });
  assert.equal(settleCalls, 3);
  assert.deepEqual(slept, [1000, 1500, 3000]);
});
