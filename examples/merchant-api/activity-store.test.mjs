import test from "node:test";
import assert from "node:assert/strict";

import { ActivityStore } from "./activity-store.mjs";

const records = [
  { id: "a", network: "near:testnet", account: "alice.testnet", kind: "transfer", indexedAt: "2026-07-27T00:00:00Z" },
  { id: "b", network: "near:testnet", contract: "token.testnet", kind: "contract_call", indexedAt: "2026-07-27T00:00:00Z" },
];

test("activity search returns bounded pages and cursors", () => {
  const store = new ActivityStore(records);
  const first = store.search({ limit: 1 });
  assert.equal(first.items.length, 1);
  assert.ok(first.nextCursor);
  const second = store.search({ limit: 1, cursor: first.nextCursor });
  assert.equal(second.items[0].id, "b");
  assert.equal(second.nextCursor, null);
});

test("empty activity index reports not_yet_indexed", () => {
  const store = new ActivityStore();
  assert.equal(store.search({}).index.status, "not_yet_indexed");
  assert.equal(store.entity("alice.testnet").status, "not_yet_indexed");
});

test("invalid cursors fail closed", () => {
  const store = new ActivityStore(records);
  assert.throws(() => store.search({ cursor: "bad" }), /cursor is invalid/);
});

test("malformed search input and duplicate record ids fail closed", () => {
  const store = new ActivityStore(records);
  assert.throws(() => store.search({ limit: 0 }), error => error.status === 400);
  assert.throws(() => new ActivityStore([records[0], records[0]]), /duplicated/);
});
