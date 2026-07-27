import test from "node:test";
import assert from "node:assert/strict";

import { ChainEvidenceError, JsonRpcTransport, createEvmReader, createNearReader } from "./chain-reader.mjs";

function fakeRpc(responses) {
  return { request: async method => responses[method] };
}

test("NEAR account evidence is pinned to a final block", async () => {
  const reader = createNearReader({
    network: "near:testnet",
    rpc: fakeRpc({
      block: { header: { height: 42, hash: "block-hash" } },
      query: { amount: "1000", locked: "0", storage_usage: 12, code_hash: "111" },
    }),
  });
  const result = await reader.account("alice.testnet");
  assert.equal(result.observedFinality, "final");
  assert.equal(result.block.height, 42);
  assert.equal(result.account.amountYoctoNear, "1000");
});

test("NEAR account input is rejected without an RPC call", async () => {
  const reader = createNearReader({
    network: "near:testnet",
    rpc: { request: async () => assert.fail("RPC must not be called") },
  });
  await assert.rejects(() => reader.account("not valid"), ChainEvidenceError);
});

test("NEAR transaction evidence accepts base58 hashes", async () => {
  const reader = createNearReader({
    network: "near:testnet",
    rpc: fakeRpc({
      tx: {
        final_execution_status: "FINAL",
        status: { SuccessValue: "" },
        transaction: { block_hash: "block-hash", receiver_id: "merchant.testnet" },
        receipts_outcome: [],
      },
    }),
  });
  const result = await reader.transaction("11111111111111111111111111111111111111111111", "alice.testnet");
  assert.equal(result.transaction.status, "succeeded");
  assert.equal(result.observedFinality, "final");
});

test("EVM transaction evidence reports pending and finalized states", async () => {
  const reader = createEvmReader({
    network: "eip155:84532",
    asset: "0x0000000000000000000000000000000000000000",
    rpc: fakeRpc({
      eth_getTransactionByHash: { from: "0x1111111111111111111111111111111111111111", to: null },
      eth_getTransactionReceipt: null,
      eth_getBlockByNumber: { number: "0x10", hash: "0x" + "22".repeat(32) },
    }),
  });
  const result = await reader.transaction("0x" + "11".repeat(32));
  assert.equal(result.transaction.status, "pending");
  assert.equal(result.observedFinality, "nonterminal");
});

test("RPC failures become typed unavailable errors", async () => {
  const rpc = new JsonRpcTransport("https://rpc.invalid", async () => ({ ok: false, status: 504 }), 20);
  await assert.rejects(() => rpc.request("block", {}), error => error instanceof ChainEvidenceError && error.code === "rpc_unavailable");
});

test("EVM missing transactions are not presented as empty evidence", async () => {
  const reader = createEvmReader({
    network: "eip155:84532",
    asset: "0x0000000000000000000000000000000000000000",
    rpc: fakeRpc({
      eth_getTransactionByHash: null,
      eth_getTransactionReceipt: null,
      eth_getBlockByNumber: { number: "0x10", hash: "0x" + "22".repeat(32) },
    }),
  });
  await assert.rejects(() => reader.transaction("0x" + "11".repeat(32)), error => error.code === "not_found" && error.status === 404);
});
