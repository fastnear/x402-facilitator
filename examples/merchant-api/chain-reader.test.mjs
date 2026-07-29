import assert from "node:assert/strict";
import test from "node:test";

import {
  ChainEvidenceError,
  JsonRpcTransport,
  createEvmReader,
  createNearReader,
} from "./chain-reader.mjs";
import {
  isNearCryptoHash,
  isNearTransactionHash,
  validateEvidenceInput,
} from "./evidence-input.mjs";

const REQUEST_HASH = `0x${"11".repeat(32)}`;
const BLOCK_HASH = `0x${"22".repeat(32)}`;
const FINAL_HASH = `0x${"33".repeat(32)}`;
const SENDER = `0x${"44".repeat(20)}`;
const NEAR_HASH = "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi";
const NEAR_BLOCK_HASH = "JEKNVnkbo3jma5nREBBJCDoXFVeKkD56V3xKrvRmWxFG";
const NEAR_RECEIPT_ID = "8qbHbw2BbbTHBW1sbeqakYXVKRQM8Ne7pLK7m6CVfeR";
const NEAR_NO_CODE_HASH = "11111111111111111111111111111111";
const MALFORMED_NEAR_HASHES = ["1".repeat(43), "1".repeat(44)];

function fakeRpc(responses) {
  return {
    async request(method, params) {
      const response = responses[method];
      return typeof response === "function" ? response(params) : response;
    },
  };
}

function nearReader(responses) {
  return createNearReader({
    network: "near:testnet",
    chainId: "testnet",
    rpc: fakeRpc(responses),
  });
}

function evmReader(responses) {
  return createEvmReader({
    network: "eip155:84532",
    chainId: "84532",
    asset: "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
    rpc: fakeRpc(responses),
  });
}

function evmResponses(overrides = {}) {
  return {
    eth_getTransactionByHash: {
      hash: REQUEST_HASH,
      from: SENDER,
      to: null,
      blockNumber: "0xf",
      blockHash: BLOCK_HASH,
      ...overrides.transaction,
    },
    eth_getTransactionReceipt: {
      transactionHash: REQUEST_HASH,
      blockNumber: "0xf",
      blockHash: BLOCK_HASH,
      status: "0x1",
      gasUsed: "0x100",
      ...overrides.receipt,
    },
    eth_getBlockByNumber: params => params[0] === "finalized"
      ? { number: "0x10", hash: FINAL_HASH }
      : {
        number: "0xf",
        hash: BLOCK_HASH,
        ...overrides.canonicalBlock,
      },
  };
}

function nearTransaction({
  transaction: transactionOverrides = {},
  transactionOutcome: transactionOutcomeOverrides = {},
  receipts_outcome = [{
    id: NEAR_RECEIPT_ID,
    block_hash: NEAR_BLOCK_HASH,
    outcome: { status: { SuccessReceiptId: NEAR_RECEIPT_ID } },
  }],
  ...resultOverrides
} = {}) {
  return {
    final_execution_status: "FINAL",
    status: { SuccessValue: "" },
    transaction: {
      hash: NEAR_HASH,
      signer_id: "alice.testnet",
      block_hash: NEAR_BLOCK_HASH,
      receiver_id: "merchant.testnet",
      ...transactionOverrides,
    },
    transaction_outcome: {
      id: NEAR_HASH,
      block_hash: NEAR_BLOCK_HASH,
      outcome: { status: { SuccessReceiptId: NEAR_RECEIPT_ID } },
      ...transactionOutcomeOverrides,
    },
    receipts_outcome,
    ...resultOverrides,
  };
}

test("NEAR transaction hashes decode to exactly 32 bytes", () => {
  for (const hash of [
    NEAR_HASH,
    NEAR_BLOCK_HASH,
    NEAR_RECEIPT_ID,
    NEAR_NO_CODE_HASH,
  ]) {
    assert.equal(isNearCryptoHash(hash), true);
    assert.equal(isNearTransactionHash(hash), true);
    assert.doesNotThrow(() => validateEvidenceInput("near:testnet", "transaction", {
      transactionHash: hash,
      signerId: "alice.testnet",
    }));
  }

  for (const hash of MALFORMED_NEAR_HASHES) {
    assert.equal(isNearCryptoHash(hash), false);
    assert.equal(isNearTransactionHash(hash), false);
    assert.throws(
      () => validateEvidenceInput("near:testnet", "transaction", {
        transactionHash: hash,
        signerId: "alice.testnet",
      }),
      /32-byte NEAR CryptoHash/,
    );
  }
});

test("NEAR account evidence is pinned to one final block", async () => {
  let queryParams;
  const reader = nearReader({
    block: { header: { height: 42, hash: NEAR_BLOCK_HASH } },
    query: params => {
      queryParams = params;
      return {
        amount: "1000",
        locked: "0",
        storage_usage: 12,
        code_hash: NEAR_NO_CODE_HASH,
        block_hash: NEAR_BLOCK_HASH,
        block_height: 42,
      };
    },
  });
  const result = await reader.account("alice.testnet");
  assert.equal(result.observedFinality, "final");
  assert.equal(result.block.height, 42);
  assert.equal(result.account.amountYoctoNear, "1000");
  assert.equal(queryParams.block_id, NEAR_BLOCK_HASH);
  assert.equal(queryParams.finality, undefined);
});

test("NEAR account evidence requires the exact pinned query block identity", async () => {
  const response = {
    amount: "1000",
    locked: "0",
    storage_usage: 12,
    code_hash: NEAR_NO_CODE_HASH,
    block_hash: NEAR_BLOCK_HASH,
    block_height: 42,
  };
  const missingHash = { ...response };
  delete missingHash.block_hash;
  const missingHeight = { ...response };
  delete missingHeight.block_height;

  for (const query of [
    missingHash,
    missingHeight,
    { ...response, block_hash: NEAR_RECEIPT_ID },
    { ...response, block_height: 41 },
    { ...response, code_hash: "111" },
    { ...response, code_hash: "0".repeat(44) },
  ]) {
    const reader = nearReader({
      block: { header: { height: 42, hash: NEAR_BLOCK_HASH } },
      query,
    });
    await assert.rejects(
      () => reader.account("alice.testnet"),
      error => error.code === "invalid_rpc",
    );
  }
});

test("NEAR final and transaction block headers require canonical identities", async () => {
  for (const header of [
    { height: -1, hash: NEAR_BLOCK_HASH },
    { height: 1.5, hash: NEAR_BLOCK_HASH },
    { height: Number.MAX_SAFE_INTEGER + 1, hash: NEAR_BLOCK_HASH },
    { height: 1, hash: "not-a-valid-near-hash" },
    ...MALFORMED_NEAR_HASHES.map(hash => ({ height: 1, hash })),
  ]) {
    const accountReader = nearReader({
      block: { header },
      query: assert.fail,
    });
    await assert.rejects(
      () => accountReader.account("alice.testnet"),
      error => error.code === "invalid_rpc",
    );

    const transactionReader = nearReader({
      block: { header },
      tx: nearTransaction(),
    });
    await assert.rejects(
      () => transactionReader.transaction(NEAR_HASH, "alice.testnet"),
      error => error.code === "invalid_rpc",
    );
  }
});

test("NEAR account input is rejected without an RPC call", async () => {
  const reader = createNearReader({
    network: "near:testnet",
    chainId: "testnet",
    rpc: { request: async () => assert.fail("RPC must not be called") },
  });
  await assert.rejects(
    () => reader.account("not valid"),
    error => error instanceof ChainEvidenceError && error.status === 400,
  );
});

test("NEAR transaction input rejects malformed base58 hashes without an RPC call", async () => {
  const reader = createNearReader({
    network: "near:testnet",
    chainId: "testnet",
    rpc: { request: async () => assert.fail("RPC must not be called") },
  });
  for (const hash of MALFORMED_NEAR_HASHES) {
    await assert.rejects(
      () => reader.transaction(hash, "alice.testnet"),
      error => error instanceof ChainEvidenceError
        && error.code === "invalid_input"
        && error.status === 400,
    );
  }
});

test("chain readers reject explorer base URLs with query strings", () => {
  assert.throws(
    () => createNearReader({
      network: "near:testnet",
      chainId: "testnet",
      rpc: fakeRpc({}),
      explorerBaseUrl: "https://nearblocks.io/?untrusted=query",
    }),
    error => error instanceof ChainEvidenceError && error.code === "invalid_config",
  );
  assert.throws(
    () => createEvmReader({
      network: "eip155:84532",
      chainId: "84532",
      asset: "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
      rpc: fakeRpc({}),
      explorerBaseUrl: "https://basescan.org/?untrusted=query",
    }),
    error => error instanceof ChainEvidenceError && error.code === "invalid_config",
  );
});

test("NEAR RPC identity must match the configured network", async () => {
  const matching = nearReader({ status: { chain_id: "testnet" } });
  assert.deepEqual(
    await matching.checkIdentity(),
    { network: "near:testnet", chainId: "testnet" },
  );

  const wrong = nearReader({ status: { chain_id: "mainnet" } });
  await assert.rejects(
    () => wrong.checkIdentity(),
    error => error.code === "wrong_chain" && error.status === 503,
  );
});

test("NEAR readiness requires identity and a canonical final block", async () => {
  const ready = nearReader({
    status: { chain_id: "testnet" },
    block: { header: { height: 42, hash: NEAR_BLOCK_HASH } },
  });
  assert.deepEqual(
    await ready.checkReadiness(),
    { network: "near:testnet", chainId: "testnet" },
  );

  for (const block of [
    undefined,
    { header: { height: -1, hash: NEAR_BLOCK_HASH } },
    { header: { height: 42, hash: MALFORMED_NEAR_HASHES[0] } },
  ]) {
    await assert.rejects(
      () => nearReader({ status: { chain_id: "testnet" }, block }).checkReadiness(),
      error => error.code === "invalid_rpc",
    );
  }
});

test("NEAR transaction evidence accepts only canonical terminal statuses", async () => {
  const reader = nearReader({
    block: { header: { height: 43, hash: NEAR_BLOCK_HASH } },
    tx: nearTransaction(),
  });
  const result = await reader.transaction(NEAR_HASH, "alice.testnet");
  assert.equal(result.transaction.status, "succeeded");
  assert.equal(result.observedFinality, "final");
  assert.deepEqual(result.block, { height: 43, hash: NEAR_BLOCK_HASH });
});

test("NEAR transaction evidence reports explicit failures", async () => {
  const reader = nearReader({
    block: { header: { height: 43, hash: NEAR_BLOCK_HASH } },
    tx: nearTransaction({
      receipts_outcome: [{
        id: NEAR_RECEIPT_ID,
        block_hash: NEAR_BLOCK_HASH,
        outcome: { status: { Failure: { ActionError: "failed" } } },
      }],
    }),
  });
  const result = await reader.transaction(NEAR_HASH, "alice.testnet");
  assert.equal(result.transaction.status, "failed");
  assert.equal(result.transaction.success, false);
  assert.equal(result.transaction.failures.length, 1);
});

test("NEAR transaction outcome failure is included in failed evidence", async () => {
  const failure = { ActionError: "transaction failed" };
  const reader = nearReader({
    block: { header: { height: 43, hash: NEAR_BLOCK_HASH } },
    tx: nearTransaction({
      transactionOutcome: { outcome: { status: { Failure: failure } } },
    }),
  });

  const result = await reader.transaction(NEAR_HASH, "alice.testnet");
  assert.equal(result.transaction.status, "failed");
  assert.equal(result.transaction.success, false);
  assert.deepEqual(result.transaction.failures, [failure]);
});

test("NEAR transaction requires a canonical final and transaction-outcome status", async () => {
  const cases = [
    nearTransaction({ transactionOutcome: { outcome: undefined } }),
    nearTransaction({
      transactionOutcome: { outcome: { status: { Unknown: "" } } },
    }),
    nearTransaction({
      transactionOutcome: {
        outcome: { status: { SuccessReceiptId: MALFORMED_NEAR_HASHES[0] } },
      },
    }),
    nearTransaction({
      receipts_outcome: [{
        id: NEAR_RECEIPT_ID,
        block_hash: NEAR_BLOCK_HASH,
        outcome: { status: { SuccessReceiptId: MALFORMED_NEAR_HASHES[1] } },
      }],
    }),
    nearTransaction({ status: { SuccessReceiptId: NEAR_RECEIPT_ID } }),
  ];

  for (const tx of cases) {
    const reader = nearReader({
      block: { header: { height: 43, hash: NEAR_BLOCK_HASH } },
      tx,
    });
    await assert.rejects(
      () => reader.transaction(NEAR_HASH, "alice.testnet"),
      error => error.code === "invalid_rpc",
    );
  }
});

test("NEAR transaction evidence requires exact transaction and outcome identities", async () => {
  const cases = [
    nearTransaction({ transaction: { hash: undefined } }),
    nearTransaction({ transaction: { hash: MALFORMED_NEAR_HASHES[0] } }),
    nearTransaction({ transaction: { signer_id: undefined } }),
    nearTransaction({ transactionOutcome: { id: undefined } }),
    nearTransaction({ transactionOutcome: { id: MALFORMED_NEAR_HASHES[1] } }),
    nearTransaction({ transactionOutcome: { id: "44444444444444444444444444444444444444444444" } }),
    nearTransaction({ transactionOutcome: { block_hash: undefined } }),
    nearTransaction({ transactionOutcome: { block_hash: MALFORMED_NEAR_HASHES[0] } }),
    nearTransaction({ transaction: { block_hash: "44444444444444444444444444444444444444444444" } }),
    nearTransaction({ transaction: { block_hash: MALFORMED_NEAR_HASHES[1] } }),
    nearTransaction({
      receipts_outcome: [{
        block_hash: NEAR_BLOCK_HASH,
        outcome: { status: { SuccessReceiptId: NEAR_RECEIPT_ID } },
      }],
    }),
    nearTransaction({
      receipts_outcome: [{
        id: MALFORMED_NEAR_HASHES[0],
        block_hash: NEAR_BLOCK_HASH,
        outcome: { status: { SuccessReceiptId: NEAR_RECEIPT_ID } },
      }],
    }),
    nearTransaction({
      receipts_outcome: [{
        id: NEAR_RECEIPT_ID,
        block_hash: MALFORMED_NEAR_HASHES[1],
        outcome: { status: { SuccessReceiptId: NEAR_RECEIPT_ID } },
      }],
    }),
  ];
  for (const tx of cases) {
    const reader = nearReader({
      block: { header: { height: 43, hash: NEAR_BLOCK_HASH } },
      tx,
    });
    await assert.rejects(
      () => reader.transaction(NEAR_HASH, "alice.testnet"),
      error => error.code === "invalid_rpc",
    );
  }
});

test("NEAR transaction evidence fails closed on unknown or conflicting state", async () => {
  for (const tx of [
    nearTransaction({
      status: { Unknown: "" },
    }),
    nearTransaction({
      status: { Failure: null },
    }),
    nearTransaction({
      receipts_outcome: [{
        id: NEAR_RECEIPT_ID,
        block_hash: NEAR_BLOCK_HASH,
        outcome: { status: { Unknown: "" } },
      }],
    }),
    nearTransaction({
      transactionOutcome: { block_hash: "44444444444444444444444444444444444444444444" },
    }),
  ]) {
    const reader = nearReader({
      block: { header: { height: 43, hash: NEAR_BLOCK_HASH } },
      tx,
    });
    await assert.rejects(
      () => reader.transaction(NEAR_HASH, "alice.testnet"),
      error => error.code === "invalid_rpc",
    );
  }
});

test("EVM RPC identity must match the configured chain id", async () => {
  assert.deepEqual(
    await evmReader({ eth_chainId: "0x14a34" }).checkIdentity(),
    { network: "eip155:84532", chainId: "84532" },
  );
  await assert.rejects(
    () => evmReader({ eth_chainId: "0x2105" }).checkIdentity(),
    error => error.code === "wrong_chain" && error.status === 503,
  );
});

test("EVM readiness requires identity and a canonical finalized block", async () => {
  const ready = evmReader({
    eth_chainId: "0x14a34",
    eth_getBlockByNumber: { number: "0x10", hash: FINAL_HASH },
  });
  assert.deepEqual(
    await ready.checkReadiness(),
    { network: "eip155:84532", chainId: "84532" },
  );

  for (const finalBlock of [
    undefined,
    { number: "0x10", hash: "0x1234" },
    { number: "not-a-number", hash: FINAL_HASH },
  ]) {
    await assert.rejects(
      () => evmReader({
        eth_chainId: "0x14a34",
        eth_getBlockByNumber: finalBlock,
      }).checkReadiness(),
      error => error.code === "invalid_rpc",
    );
  }
});

test("EVM transaction evidence reports pending state only for an unmined transaction", async () => {
  const reader = evmReader({
    eth_getTransactionByHash: {
      hash: REQUEST_HASH,
      from: SENDER,
      to: null,
      blockNumber: null,
      blockHash: null,
    },
    eth_getTransactionReceipt: null,
    eth_getBlockByNumber: { number: "0x10", hash: FINAL_HASH },
  });
  const result = await reader.transaction(REQUEST_HASH);
  assert.equal(result.transaction.status, "pending");
  assert.equal(result.observedFinality, "nonterminal");
  assert.equal(result.transaction.confirmationDepth, "0");
});

test("EVM pending evidence requires explicit paired null transaction block fields", async () => {
  const pending = transaction => ({
    eth_getTransactionByHash: transaction,
    eth_getTransactionReceipt: null,
    eth_getBlockByNumber: { number: "0x10", hash: FINAL_HASH },
  });
  const base = {
    hash: REQUEST_HASH,
    from: SENDER,
    to: null,
    blockNumber: null,
    blockHash: null,
  };
  const missingNumber = { ...base };
  delete missingNumber.blockNumber;
  const missingHash = { ...base };
  delete missingHash.blockHash;

  for (const transaction of [
    missingNumber,
    missingHash,
    { ...base, blockNumber: "0xf" },
    { ...base, blockHash: BLOCK_HASH },
  ]) {
    await assert.rejects(
      () => evmReader(pending(transaction)).transaction(REQUEST_HASH),
      error => error.code === "invalid_rpc",
    );
  }
});

test("EVM transaction evidence correlates transaction, receipt, and canonical finalized block", async () => {
  const result = await evmReader(evmResponses()).transaction(REQUEST_HASH);
  assert.equal(result.transaction.status, "succeeded");
  assert.equal(result.observedFinality, "finalized");
  assert.equal(result.transaction.blockNumber, "15");
  assert.equal(result.transaction.confirmationDepth, "2");
});

test("EVM transaction evidence fails closed on identity and canonicality conflicts", async () => {
  const cases = [
    evmResponses({ transaction: { hash: `0x${"99".repeat(32)}` } }),
    evmResponses({ receipt: { transactionHash: `0x${"99".repeat(32)}` } }),
    evmResponses({ receipt: { blockHash: `0x${"99".repeat(32)}` } }),
    evmResponses({ canonicalBlock: { hash: `0x${"99".repeat(32)}` } }),
    evmResponses({ receipt: { blockNumber: "0x11" } }),
  ];
  for (const responses of cases) {
    await assert.rejects(
      () => evmReader(responses).transaction(REQUEST_HASH),
      error => error.code === "invalid_rpc",
    );
  }
});

test("EVM mined transaction without a receipt is indeterminate", async () => {
  const responses = evmResponses();
  responses.eth_getTransactionReceipt = null;
  await assert.rejects(
    () => evmReader(responses).transaction(REQUEST_HASH),
    error => error.code === "invalid_rpc",
  );
});

test("EVM missing transactions are not presented as empty evidence", async () => {
  await assert.rejects(
    () => evmReader({
      eth_getTransactionByHash: null,
      eth_getTransactionReceipt: null,
      eth_getBlockByNumber: { number: "0x10", hash: FINAL_HASH },
    }).transaction(REQUEST_HASH),
    error => error.code === "not_found" && error.status === 404,
  );
});

test("EVM orphan receipts are treated as conflicting RPC evidence", async () => {
  const responses = evmResponses();
  responses.eth_getTransactionByHash = null;
  await assert.rejects(
    () => evmReader(responses).transaction(REQUEST_HASH),
    error => error.code === "invalid_rpc",
  );
});

test("RPC failures become typed unavailable errors", async () => {
  const rpc = new JsonRpcTransport(
    "https://rpc.invalid",
    async () => ({ ok: false, status: 504 }),
    20,
  );
  await assert.rejects(
    () => rpc.request("block", {}),
    error =>
      error instanceof ChainEvidenceError
      && error.code === "rpc_unavailable",
  );
});

test("JSON-RPC transport rejects malformed or mismatched response envelopes", async () => {
  for (const body of [
    { jsonrpc: "2.0", result: {} },
    { jsonrpc: "2.0", id: "different", result: {} },
    { jsonrpc: "1.0", id: "status:1", result: {} },
    {
      jsonrpc: "2.0",
      id: "status:1",
      result: {},
      error: { code: -32000, message: "conflicting" },
    },
  ]) {
    const rpc = new JsonRpcTransport(
      "https://rpc.invalid",
      async () => Response.json(body),
      20,
    );
    await assert.rejects(
      () => rpc.request("status", []),
      error => error instanceof ChainEvidenceError && error.code === "invalid_rpc",
    );
  }
});

test("JSON-RPC transport accepts an exact result envelope", async () => {
  const rpc = new JsonRpcTransport(
    "https://rpc.invalid",
    async () => Response.json({
      jsonrpc: "2.0",
      id: "status:1",
      result: { chain_id: "testnet" },
    }),
    20,
  );
  assert.deepEqual(await rpc.request("status", []), { chain_id: "testnet" });
});

test("JSON-RPC transport correlates concurrent requests with unique ids", async () => {
  const requests = [];
  const rpc = new JsonRpcTransport(
    "https://rpc.invalid",
    async (_url, request) => new Promise(resolve => {
      requests.push({ body: JSON.parse(request.body), resolve });
    }),
    20,
  );
  const first = rpc.request("status", []);
  const second = rpc.request("status", []);
  assert.equal(requests.length, 2);
  assert.notEqual(requests[0].body.id, requests[1].body.id);

  requests[0].resolve(Response.json({
    jsonrpc: "2.0",
    id: requests[1].body.id,
    result: { chain_id: "testnet" },
  }));
  requests[1].resolve(Response.json({
    jsonrpc: "2.0",
    id: requests[0].body.id,
    result: { chain_id: "testnet" },
  }));

  await assert.rejects(
    first,
    error => error instanceof ChainEvidenceError && error.code === "invalid_rpc",
  );
  await assert.rejects(
    second,
    error => error instanceof ChainEvidenceError && error.code === "invalid_rpc",
  );
});
