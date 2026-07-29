import assert from "node:assert/strict";
import test from "node:test";

import { createReadinessCache } from "./readiness-cache.mjs";

test("readiness cache coalesces concurrent checks and bounds completed snapshots", async () => {
  let now = 10_000;
  let calls = 0;
  const resolvers = [];
  const cached = createReadinessCache({
    now: () => now,
    ttlMs: 1_000,
    check: () => {
      calls += 1;
      return new Promise(resolve => resolvers.push(resolve));
    },
  });

  const first = cached();
  const second = cached();
  await Promise.resolve();
  assert.equal(calls, 1);

  const result = { ready: false, checks: { rpc: "not_ready" } };
  resolvers.shift()(result);
  assert.equal(await first, result);
  assert.equal(await second, result);

  assert.equal(await cached(), result);
  assert.equal(calls, 1);

  now += 1_000;
  const refreshed = cached();
  await Promise.resolve();
  assert.equal(calls, 2);
  resolvers.shift()({ ready: true, checks: { rpc: "ready" } });
  assert.deepEqual(await refreshed, { ready: true, checks: { rpc: "ready" } });
});

test("readiness cache rejects invalid configuration and does not cache failed checks", async () => {
  assert.throws(
    () => createReadinessCache({ check: async () => {}, ttlMs: -1 }),
    /ttlMs/,
  );
  assert.throws(
    () => createReadinessCache({}),
    /check/,
  );

  let calls = 0;
  const cached = createReadinessCache({
    check: async () => {
      calls += 1;
      throw new Error("dependency exploded");
    },
  });
  await assert.rejects(cached(), /dependency exploded/);
  await assert.rejects(cached(), /dependency exploded/);
  assert.equal(calls, 2);
});
