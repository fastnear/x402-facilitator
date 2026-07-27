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
  if (account.length > 64 || !/^[a-z0-9._-]+$/.test(account)) {
    throw new ChainEvidenceError("invalid_input", `${field} is not a valid NEAR account id`, 400);
  }
  return account;
}

function validateHex(value, field, length) {
  const candidate = requireString(value, field);
  const pattern = new RegExp(`^0x[0-9a-fA-F]{${length}}$`);
  if (!pattern.test(candidate)) {
    throw new ChainEvidenceError("invalid_input", `${field} has an invalid hexadecimal shape`, 400);
  }
  return candidate;
}

function validateNearHash(value, field) {
  const hash = requireString(value, field);
  if (!/^[1-9A-HJ-NP-Za-km-z]{43,44}$/.test(hash)) {
    throw new ChainEvidenceError("invalid_input", `${field} is not a valid NEAR hash`, 400);
  }
  return hash;
}

function hexNumber(value, field) {
  const candidate = requireString(value, field);
  if (!/^0x[0-9a-fA-F]+$/.test(candidate)) {
    throw new ChainEvidenceError("invalid_rpc", `${field} was not a hexadecimal number`);
  }
  return BigInt(candidate).toString(10);
}

function block(value, field = "block") {
  if (!value || typeof value !== "object") {
    throw new ChainEvidenceError("invalid_rpc", `${field} is missing`);
  }
  return value;
}

export class JsonRpcTransport {
  constructor(url, fetchImpl = fetch, timeoutMs = DEFAULT_TIMEOUT_MS) {
    this.url = url;
    this.fetchImpl = fetchImpl;
    this.timeoutMs = timeoutMs;
  }

  async request(method, params) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);
    try {
      const response = await this.fetchImpl(this.url, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: method, method, params }),
        signal: controller.signal,
      });
      if (!response.ok) {
        throw new ChainEvidenceError("rpc_unavailable", `RPC returned HTTP ${response.status}`);
      }
      const body = await response.json();
      if (body.error) {
        throw new ChainEvidenceError("rpc_error", "RPC returned an error");
      }
      if (!("result" in body)) {
        throw new ChainEvidenceError("invalid_rpc", "RPC response has no result");
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
  return `${explorerBaseUrl.replace(/\/$/, "")}/txns/${encodeURIComponent(hash)}/${encodeURIComponent(signerId)}`;
}

function evmExplorer(explorerBaseUrl, hash) {
  if (!explorerBaseUrl) return undefined;
  return `${explorerBaseUrl.replace(/\/$/, "")}/tx/${encodeURIComponent(hash)}`;
}

export function createNearReader({ network, rpc, explorerBaseUrl }) {
  async function finalBlock() {
    const result = await rpc.request("block", { finality: "final" });
    const header = block(result?.header, "final block header");
    if (typeof header.height !== "number" || typeof header.hash !== "string") {
      throw new ChainEvidenceError("invalid_rpc", "final block was incomplete");
    }
    return { height: header.height, hash: header.hash };
  }

  return {
    async account(accountId) {
      const account = validateNearAccount(accountId, "accountId");
      const final = await finalBlock();
      const result = await rpc.request("query", {
        request_type: "view_account",
        finality: "final",
        block_id: final.hash,
        account_id: account,
      });
      if (!result || typeof result.amount !== "string") {
        throw new ChainEvidenceError("invalid_rpc", "account response was incomplete");
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
          codeHash: result.code_hash,
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
      const transaction = result.transaction ?? {};
      const status = result.status;
      const failures = [];
      for (const outcome of result.receipts_outcome ?? []) {
        if (outcome?.outcome?.status?.Failure) failures.push(outcome.outcome.status.Failure);
      }
      const success = !status.Failure && failures.length === 0;
      const blockHash = transaction.block_hash ?? result.transaction_outcome?.block_hash;
      return {
        network,
        kind: "transaction",
        observedFinality: result.final_execution_status === "FINAL" ? "final" : "nonterminal",
        observedAt: new Date().toISOString(),
        transaction: {
          hash,
          signerId: signer,
          receiverId: transaction.receiver_id,
          blockHash,
          success,
          status: success ? "succeeded" : "failed",
          receiptCount: Array.isArray(result.receipts_outcome) ? result.receipts_outcome.length : 0,
          failures,
        },
        explorerUrl: blockHash ? nearExplorer(explorerBaseUrl, hash, signer) : undefined,
        source: { type: "near-jsonrpc", status: "final" },
      };
    },
  };
}

export function createEvmReader({ network, rpc, asset, explorerBaseUrl }) {
  async function finalBlock() {
    const result = await rpc.request("eth_getBlockByNumber", ["finalized", false]);
    const value = block(result, "finalized block");
    return {
      height: hexNumber(value.number, "finalized block number"),
      hash: validateHex(value.hash, "finalized block hash", 64),
    };
  }

  return {
    async account(address) {
      const account = validateHex(address, "address", 40);
      const final = await finalBlock();
      const [balance, code] = await Promise.all([
        rpc.request("eth_getBalance", [account, "finalized"]),
        rpc.request("eth_getCode", [account, "finalized"]),
      ]);
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
        throw new ChainEvidenceError("not_found", "transaction was not found", 404);
      }
      const status = receipt ? hexNumber(receipt.status, "receipt status") : "pending";
      const blockNumber = receipt?.blockNumber ? hexNumber(receipt.blockNumber, "receipt block") : undefined;
      const depth = blockNumber ? (BigInt(final.height) - BigInt(blockNumber)).toString(10) : "0";
      const observedFinality = receipt && BigInt(depth) >= 1n ? "finalized" : "nonterminal";
      return {
        network,
        kind: "transaction",
        observedFinality,
        observedAt: new Date().toISOString(),
        block: final,
        transaction: {
          hash,
          from: tx.from,
          to: tx.to,
          blockNumber,
          confirmationDepth: depth,
          success: receipt ? status === "1" : undefined,
          status: receipt ? (status === "1" ? "succeeded" : "failed") : "pending",
          gasUsed: receipt?.gasUsed ? hexNumber(receipt.gasUsed, "receipt gas") : undefined,
        },
        explorerUrl: evmExplorer(explorerBaseUrl, hash),
        source: { type: "evm-jsonrpc", status: observedFinality },
      };
    },
  };
}

export function createChainReader({ network, rpcUrl, asset, explorerBaseUrl, fetchImpl }) {
  const rpc = new JsonRpcTransport(rpcUrl, fetchImpl);
  if (network.startsWith("near:")) {
    return createNearReader({ network, rpc, explorerBaseUrl });
  }
  if (network.startsWith("eip155:")) {
    return createEvmReader({ network, rpc, asset, explorerBaseUrl });
  }
  throw new ChainEvidenceError("invalid_config", "NETWORK must be a supported NEAR or EVM network", 500);
}
