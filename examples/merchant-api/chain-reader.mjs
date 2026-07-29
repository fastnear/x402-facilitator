import {
  isEvmAddress,
  isEvmTransactionHash,
  isNearAccountId,
  isNearCryptoHash,
} from "./evidence-input.mjs";

const DEFAULT_TIMEOUT_MS = 12_000;

export class ChainEvidenceError extends Error {
  constructor(code, message, status = 502) {
    super(message);
    this.name = "ChainEvidenceError";
    this.code = code;
    this.status = status;
  }
}

function requireString(value, field) {
  if (typeof value !== "string" || value.length === 0) {
    throw new ChainEvidenceError("invalid_input", `${field} must be a non-empty string`, 400);
  }
  return value;
}

function validateNearAccount(value, field) {
  const account = requireString(value, field);
  if (!isNearAccountId(account)) {
    throw new ChainEvidenceError("invalid_input", `${field} is not a valid NEAR account id`, 400);
  }
  return account;
}

function validateHex(value, field, length) {
  const candidate = requireString(value, field);
  const valid = length === 40
    ? isEvmAddress(candidate)
    : length === 64 && isEvmTransactionHash(candidate);
  if (!valid) {
    throw new ChainEvidenceError("invalid_input", `${field} has an invalid hexadecimal shape`, 400);
  }
  return candidate;
}

function rpcHex(value, field, length) {
  if (
    typeof value !== "string"
    || !(new RegExp(`^0x[0-9a-fA-F]{${length}}$`)).test(value)
  ) {
    throw new ChainEvidenceError(
      "invalid_rpc",
      `${field} had an invalid hexadecimal shape`,
    );
  }
  return value;
}

function validateNearHash(value, field) {
  const hash = requireString(value, field);
  if (!isNearCryptoHash(hash)) {
    throw new ChainEvidenceError(
      "invalid_input",
      `${field} is not a 32-byte NEAR CryptoHash`,
      400,
    );
  }
  return hash;
}

function rpcNearHash(value, field) {
  if (!isNearCryptoHash(value)) {
    throw new ChainEvidenceError(
      "invalid_rpc",
      `${field} had an invalid NEAR CryptoHash`,
    );
  }
  return value;
}

function rpcNearCryptoHash(value, field) {
  if (!isNearCryptoHash(value)) {
    throw new ChainEvidenceError(
      "invalid_rpc",
      `${field} had an invalid NEAR CryptoHash shape`,
    );
  }
  return value;
}

function rpcNearHeight(value, field) {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new ChainEvidenceError(
      "invalid_rpc",
      `${field} was not a nonnegative safe integer`,
    );
  }
  return value;
}

function hexNumber(value, field) {
  if (typeof value !== "string" || !/^0x[0-9a-fA-F]+$/.test(value)) {
    throw new ChainEvidenceError("invalid_rpc", `${field} was not a hexadecimal number`);
  }
  return BigInt(value).toString(10);
}

function block(value, field = "block") {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new ChainEvidenceError("invalid_rpc", `${field} is missing`);
  }
  return value;
}

function nearOutcomeStatus(value, field, { final = false } = {}) {
  const status = block(value, field);
  const keys = Object.keys(status);
  if (keys.length !== 1) {
    throw new ChainEvidenceError("invalid_rpc", `${field} was ambiguous`);
  }
  if (keys[0] === "Failure") {
    if (
      !status.Failure
      || typeof status.Failure !== "object"
      || Array.isArray(status.Failure)
    ) {
      throw new ChainEvidenceError("invalid_rpc", `${field} contained a malformed failure`);
    }
    return { success: false, failure: status.Failure };
  }
  if (keys[0] === "SuccessValue" && typeof status.SuccessValue === "string") {
    return { success: true };
  }
  if (
    !final
    && keys[0] === "SuccessReceiptId"
  ) {
    rpcNearHash(status.SuccessReceiptId, `${field} SuccessReceiptId`);
    return { success: true };
  }
  throw new ChainEvidenceError("invalid_rpc", `${field} was not a canonical terminal status`);
}

function nearFinalStatus(value, field) {
  return nearOutcomeStatus(value, field, { final: true });
}

function nearExecutionStatus(value, field) {
  return nearOutcomeStatus(value, field);
}

function sameHex(left, right) {
  return left.toLowerCase() === right.toLowerCase();
}

export class JsonRpcTransport {
  constructor(url, fetchImpl = fetch, timeoutMs = DEFAULT_TIMEOUT_MS) {
    this.url = url;
    this.fetchImpl = fetchImpl;
    this.timeoutMs = timeoutMs;
    this.nextRequestId = 0;
  }

  async request(method, params) {
    const id = `${method}:${this.nextRequestId += 1}`;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);
    try {
      const response = await this.fetchImpl(this.url, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id, method, params }),
        signal: controller.signal,
      });
      if (!response.ok) {
        throw new ChainEvidenceError("rpc_unavailable", `RPC returned HTTP ${response.status}`);
      }
      let body;
      try {
        body = await response.json();
      } catch {
        throw new ChainEvidenceError("invalid_rpc", "RPC returned invalid JSON");
      }
      if (!body || typeof body !== "object" || Array.isArray(body)) {
        throw new ChainEvidenceError("invalid_rpc", "RPC response envelope was invalid");
      }
      if (body.jsonrpc !== "2.0") {
        throw new ChainEvidenceError("invalid_rpc", "RPC response had an invalid JSON-RPC version");
      }
      if (body.id !== id) {
        throw new ChainEvidenceError("invalid_rpc", "RPC response id did not match the request");
      }
      const hasResult = Object.hasOwn(body, "result");
      const hasError = Object.hasOwn(body, "error");
      if (hasResult === hasError) {
        throw new ChainEvidenceError("invalid_rpc", "RPC response must contain exactly one result or error");
      }
      if (hasError) {
        if (
          !body.error
          || typeof body.error !== "object"
          || Array.isArray(body.error)
          || !Number.isInteger(body.error.code)
          || typeof body.error.message !== "string"
        ) {
          throw new ChainEvidenceError("invalid_rpc", "RPC response error was malformed");
        }
        throw new ChainEvidenceError("rpc_error", "RPC returned an error");
      }
      return body.result;
    } catch (error) {
      if (error instanceof ChainEvidenceError) throw error;
      if (error?.name === "AbortError") {
        throw new ChainEvidenceError("rpc_timeout", "RPC request timed out");
      }
      throw new ChainEvidenceError("rpc_unavailable", "RPC request failed");
    } finally {
      clearTimeout(timer);
    }
  }
}

function nearExplorer(explorerBaseUrl, hash, signerId) {
  if (!explorerBaseUrl) return undefined;
  return `${explorerBaseUrl}/txns/${encodeURIComponent(hash)}/${encodeURIComponent(signerId)}`;
}

function evmExplorer(explorerBaseUrl, hash) {
  if (!explorerBaseUrl) return undefined;
  return `${explorerBaseUrl}/tx/${encodeURIComponent(hash)}`;
}

function normalizeExplorerBaseUrl(value) {
  if (value === undefined) return undefined;
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new ChainEvidenceError("invalid_config", "explorer URL was invalid", 500);
  }
  if (
    url.protocol !== "https:"
    || url.username
    || url.password
    || url.hash
    || url.search
  ) {
    throw new ChainEvidenceError(
      "invalid_config",
      "explorer URL must use HTTPS without credentials, a fragment, or a query",
      500,
    );
  }
  return url.href.replace(/\/$/, "");
}

export function createNearReader({ network, chainId, rpc, explorerBaseUrl }) {
  const explorer = normalizeExplorerBaseUrl(explorerBaseUrl);
  async function finalBlock() {
    const result = await rpc.request("block", { finality: "final" });
    const header = block(result?.header, "final block header");
    return {
      height: rpcNearHeight(header.height, "final block height"),
      hash: rpcNearHash(header.hash, "final block hash"),
    };
  }

  async function blockByHash(hash) {
    const result = await rpc.request("block", { block_id: hash });
    const header = block(result?.header, "transaction block header");
    const height = rpcNearHeight(header.height, "transaction block height");
    const headerHash = rpcNearHash(header.hash, "transaction block hash");
    if (headerHash !== hash) {
      throw new ChainEvidenceError("invalid_rpc", "transaction block conflicted with the transaction");
    }
    return { height, hash: headerHash };
  }

  async function checkIdentity() {
    const status = await rpc.request("status", []);
    if (status?.chain_id !== chainId) {
      throw new ChainEvidenceError(
        "wrong_chain",
        `NEAR RPC chain identity did not match ${chainId}`,
        503,
      );
    }
    return { network, chainId };
  }

  return {
    checkIdentity,

    async checkReadiness() {
      const identity = await checkIdentity();
      await finalBlock();
      return identity;
    },

    async account(accountId) {
      const account = validateNearAccount(accountId, "accountId");
      const final = await finalBlock();
      const result = await rpc.request("query", {
        request_type: "view_account",
        block_id: final.hash,
        account_id: account,
      });
      if (
        !result
        || typeof result.amount !== "string"
        || !/^[0-9]+$/.test(result.amount)
        || typeof result.locked !== "string"
        || !/^[0-9]+$/.test(result.locked)
        || !Number.isInteger(result.storage_usage)
        || result.storage_usage < 0
      ) {
        throw new ChainEvidenceError("invalid_rpc", "account response was incomplete");
      }
      const codeHash = rpcNearCryptoHash(result.code_hash, "account response code hash");
      const responseBlockHash = rpcNearHash(
        result.block_hash,
        "account response block hash",
      );
      const responseBlockHeight = rpcNearHeight(
        result.block_height,
        "account response block height",
      );
      if (
        responseBlockHash !== final.hash
        || responseBlockHeight !== final.height
      ) {
        throw new ChainEvidenceError("invalid_rpc", "account response conflicted with the pinned block");
      }
      return {
        network,
        kind: "account",
        observedFinality: "final",
        observedAt: new Date().toISOString(),
        block: final,
        account: {
          accountId: account,
          amountYoctoNear: result.amount,
          lockedYoctoNear: result.locked,
          storageUsage: result.storage_usage,
          codeHash,
        },
        source: { type: "near-jsonrpc", status: "final" },
      };
    },

    async transaction(transactionHash, signerId) {
      const hash = validateNearHash(transactionHash, "transactionHash");
      const signer = validateNearAccount(signerId, "signerId");
      const result = await rpc.request("tx", {
        tx_hash: hash,
        sender_account_id: signer,
        wait_until: "FINAL",
      });
      if (!result || typeof result.status !== "object") {
        throw new ChainEvidenceError("invalid_rpc", "transaction response was incomplete");
      }
      if (result.final_execution_status !== "FINAL") {
        throw new ChainEvidenceError("invalid_rpc", "transaction did not reach requested finality");
      }
      const transaction = block(result.transaction, "transaction");
      const returnedTransactionHash = rpcNearHash(transaction.hash, "transaction hash");
      if (returnedTransactionHash !== hash) {
        throw new ChainEvidenceError("invalid_rpc", "transaction hash conflicted with the request");
      }
      if (transaction.signer_id !== signer) {
        throw new ChainEvidenceError("invalid_rpc", "transaction signer conflicted with the request");
      }
      const transactionOutcome = block(
        result.transaction_outcome,
        "transaction outcome",
      );
      const returnedTransactionOutcomeId = rpcNearHash(
        transactionOutcome.id,
        "transaction outcome id",
      );
      if (returnedTransactionOutcomeId !== hash) {
        throw new ChainEvidenceError("invalid_rpc", "transaction outcome conflicted with the request");
      }
      const outcomeBlockHash = rpcNearHash(
        transactionOutcome.block_hash,
        "transaction outcome block hash",
      );
      const transactionBlockHash = transaction.block_hash === undefined
        ? undefined
        : rpcNearHash(transaction.block_hash, "transaction block hash");
      if (
        transactionBlockHash !== undefined
        && transactionBlockHash !== outcomeBlockHash
      ) {
        throw new ChainEvidenceError("invalid_rpc", "transaction block identities conflicted");
      }
      const topStatus = nearFinalStatus(result.status, "transaction status");
      const transactionOutcomeStatus = nearExecutionStatus(
        block(transactionOutcome.outcome, "transaction outcome execution").status,
        "transaction outcome status",
      );
      if (!Array.isArray(result.receipts_outcome)) {
        throw new ChainEvidenceError("invalid_rpc", "transaction response omitted receipt outcomes");
      }
      const failures = [];
      if (!transactionOutcomeStatus.success) {
        failures.push(transactionOutcomeStatus.failure);
      }
      for (const [index, outcome] of result.receipts_outcome.entries()) {
        const receipt = block(outcome, `receipt outcome ${index}`);
        rpcNearHash(receipt.id, `receipt outcome ${index} id`);
        rpcNearHash(receipt.block_hash, `receipt outcome ${index} block hash`);
        const receiptStatus = nearExecutionStatus(
          receipt.outcome?.status,
          `receipt outcome ${index} status`,
        );
        if (!receiptStatus.success) failures.push(receiptStatus.failure);
      }
      if (!topStatus.success) failures.unshift(topStatus.failure);
      const success = topStatus.success
        && transactionOutcomeStatus.success
        && failures.length === 0;
      const transactionBlock = await blockByHash(outcomeBlockHash);
      return {
        network,
        kind: "transaction",
        observedFinality: "final",
        observedAt: new Date().toISOString(),
        block: transactionBlock,
        transaction: {
          hash,
          signerId: signer,
          receiverId: transaction.receiver_id,
          blockHash: outcomeBlockHash,
          success,
          status: success ? "succeeded" : "failed",
          receiptCount: Array.isArray(result.receipts_outcome) ? result.receipts_outcome.length : 0,
          failures,
        },
        explorerUrl: nearExplorer(explorer, hash, signer),
        source: { type: "near-jsonrpc", status: "final" },
      };
    },
  };
}

export function createEvmReader({ network, chainId, rpc, asset, explorerBaseUrl }) {
  const explorer = normalizeExplorerBaseUrl(explorerBaseUrl);
  async function finalBlock() {
    const result = await rpc.request("eth_getBlockByNumber", ["finalized", false]);
    const value = block(result, "finalized block");
    return {
      height: hexNumber(value.number, "finalized block number"),
      hash: rpcHex(value.hash, "finalized block hash", 64),
    };
  }

  async function checkIdentity() {
    const actual = hexNumber(await rpc.request("eth_chainId", []), "chain id");
    if (actual !== chainId) {
      throw new ChainEvidenceError(
        "wrong_chain",
        `EVM RPC chain identity did not match ${chainId}`,
        503,
      );
    }
    return { network, chainId };
  }

  return {
    checkIdentity,

    async checkReadiness() {
      const identity = await checkIdentity();
      await finalBlock();
      return identity;
    },

    async account(address) {
      const account = validateHex(address, "address", 40);
      const final = await finalBlock();
      const blockTag = `0x${BigInt(final.height).toString(16)}`;
      const [balance, code] = await Promise.all([
        rpc.request("eth_getBalance", [account, blockTag]),
        rpc.request("eth_getCode", [account, blockTag]),
      ]);
      if (typeof code !== "string" || !/^0x[0-9a-fA-F]*$/.test(code)) {
        throw new ChainEvidenceError("invalid_rpc", "account code was not hexadecimal");
      }
      return {
        network,
        kind: "account",
        observedFinality: "finalized",
        observedAt: new Date().toISOString(),
        block: final,
        account: {
          address: account,
          balanceWei: hexNumber(balance, "account balance"),
          isContract: typeof code === "string" && code !== "0x",
          asset,
        },
        source: { type: "evm-jsonrpc", status: "finalized" },
      };
    },

    async transaction(transactionHash) {
      const hash = validateHex(transactionHash, "transactionHash", 64);
      const [tx, receipt, final] = await Promise.all([
        rpc.request("eth_getTransactionByHash", [hash]),
        rpc.request("eth_getTransactionReceipt", [hash]),
        finalBlock(),
      ]);
      if (!tx) {
        if (receipt) {
          throw new ChainEvidenceError(
            "invalid_rpc",
            "receipt existed without its transaction",
          );
        }
        throw new ChainEvidenceError("not_found", "transaction was not found", 404);
      }
      const transactionIdentity = rpcHex(tx.hash, "transaction hash", 64);
      if (!sameHex(transactionIdentity, hash)) {
        throw new ChainEvidenceError("invalid_rpc", "transaction hash conflicted with the request");
      }
      if (
        !Object.hasOwn(tx, "blockNumber")
        || !Object.hasOwn(tx, "blockHash")
      ) {
        throw new ChainEvidenceError("invalid_rpc", "transaction block identity was missing");
      }
      const pendingTransaction = tx.blockNumber === null && tx.blockHash === null;
      if ((tx.blockNumber === null) !== (tx.blockHash === null)) {
        throw new ChainEvidenceError("invalid_rpc", "transaction block identity was incomplete");
      }
      const txBlockNumber = pendingTransaction
        ? undefined
        : hexNumber(tx.blockNumber, "transaction block");
      const txBlockHash = pendingTransaction
        ? undefined
        : rpcHex(tx.blockHash, "transaction block hash", 64);

      const status = receipt
        ? hexNumber(receipt.status, "receipt status")
        : "pending";
      const blockNumber = receipt
        ? hexNumber(receipt.blockNumber, "receipt block")
        : undefined;
      if (receipt && status !== "0" && status !== "1") {
        throw new ChainEvidenceError("invalid_rpc", "receipt status was not canonical");
      }
      if (!receipt && (txBlockNumber !== undefined || txBlockHash !== undefined)) {
        throw new ChainEvidenceError(
          "invalid_rpc",
          "mined transaction was missing its receipt",
        );
      }
      if (blockNumber && BigInt(blockNumber) > BigInt(final.height)) {
        throw new ChainEvidenceError("invalid_rpc", "receipt block was newer than finalized chain state");
      }
      let canonicalBlock;
      if (receipt) {
        const receiptTransactionHash = rpcHex(
          receipt.transactionHash,
          "receipt transaction hash",
          64,
        );
        const receiptBlockHash = rpcHex(
          receipt.blockHash,
          "receipt block hash",
          64,
        );
        if (!sameHex(receiptTransactionHash, hash)) {
          throw new ChainEvidenceError("invalid_rpc", "receipt transaction hash conflicted with the request");
        }
        if (
          txBlockNumber !== blockNumber
          || txBlockHash === undefined
          || !sameHex(txBlockHash, receiptBlockHash)
        ) {
          throw new ChainEvidenceError("invalid_rpc", "transaction and receipt block identities conflicted");
        }
        const canonicalResult = await rpc.request(
          "eth_getBlockByNumber",
          [receipt.blockNumber, false],
        );
        canonicalBlock = block(canonicalResult, "receipt block");
        const canonicalNumber = hexNumber(
          canonicalBlock.number,
          "canonical receipt block number",
        );
        const canonicalHash = rpcHex(
          canonicalBlock.hash,
          "canonical receipt block hash",
          64,
        );
        if (
          canonicalNumber !== blockNumber
          || !sameHex(canonicalHash, receiptBlockHash)
        ) {
          throw new ChainEvidenceError(
            "invalid_rpc",
            "receipt block was not canonical",
          );
        }
      }
      const from = rpcHex(tx.from, "transaction sender", 40);
      const to = tx.to === null ? null : rpcHex(tx.to, "transaction recipient", 40);
      const depth = blockNumber
        ? (BigInt(final.height) - BigInt(blockNumber) + 1n).toString(10)
        : "0";
      const observedFinality = receipt ? "finalized" : "nonterminal";
      return {
        network,
        kind: "transaction",
        observedFinality,
        observedAt: new Date().toISOString(),
        block: final,
        transaction: {
          hash,
          from,
          to,
          blockNumber,
          confirmationDepth: depth,
          success: receipt ? status === "1" : undefined,
          status: receipt ? (status === "1" ? "succeeded" : "failed") : "pending",
          gasUsed: receipt?.gasUsed ? hexNumber(receipt.gasUsed, "receipt gas") : undefined,
        },
        explorerUrl: evmExplorer(explorer, hash),
        source: { type: "evm-jsonrpc", status: observedFinality },
      };
    },
  };
}

export function createChainReader({
  network,
  chainId,
  rpcUrl,
  asset,
  explorerBaseUrl,
  fetchImpl,
}) {
  const rpc = new JsonRpcTransport(rpcUrl, fetchImpl);
  if (network.startsWith("near:")) {
    return createNearReader({ network, chainId, rpc, explorerBaseUrl });
  }
  if (network.startsWith("eip155:")) {
    return createEvmReader({ network, chainId, rpc, asset, explorerBaseUrl });
  }
  throw new ChainEvidenceError("invalid_config", "NETWORK must be a supported NEAR or EVM network", 500);
}
