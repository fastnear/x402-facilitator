import { createHash, randomUUID } from "node:crypto";
import { chmod, readFile, rename, writeFile } from "node:fs/promises";

import { x402Client, x402HTTPClient } from "@x402/core/client";
import { ExactEvmScheme } from "@x402/evm/exact/client";
import { createClientNearSigner } from "@x402/near";
import { ExactNearScheme } from "@x402/near/exact/client";
import { privateKeyToAccount } from "viem/accounts";

import {
  PaidFlowError,
  assertExpectedRequirement,
  discoverPaymentRequired,
  submitPaidRequest,
} from "./paid-flow.mjs";

const attemptId = randomUUID();
const resultFile = process.env.PROOF_RESULT_FILE;
const record = resultFile ? createRecorder(resultFile) : async () => {};

try {
  const config = loadConfig();
  const request = {
    url: config.url,
    method: config.method,
    headers: {
      accept: "application/json",
      ...(config.body === undefined ? {} : { "content-type": "application/json" }),
    },
    ...(config.body === undefined ? {} : { body: JSON.stringify(config.body) }),
  };
  const previewHttp = new x402HTTPClient(new x402Client());
  const discovered = await discoverPaymentRequired({
    httpClient: previewHttp,
    request,
    timeoutMs: config.timeoutMs,
  });
  assertExpectedRequirement(discovered.requirement, config.expected);

  const preview = {
    endpoint: config.url,
    method: config.method,
    requestBodySha256: hash(request.body ?? ""),
    network: discovered.requirement.network,
    asset: discovered.requirement.asset,
    atomicAmount: discovered.requirement.amount,
    payer: config.payer,
    recipient: discovered.requirement.payTo,
    facilitatorSigner: config.facilitatorSigner,
    maximumSponsoredGas: config.maximumSponsoredGas,
  };
  const confirmationToken = hash(JSON.stringify(preview)).slice(0, 24);

  if (process.env.PROOF_CONFIRMATION_TOKEN !== confirmationToken) {
    const result = {
      attemptId,
      outcome: "confirmation_required",
      paidRequestSent: false,
      preview,
      confirmationToken,
    };
    await record(result);
    emit(result);
    process.exitCode = 3;
  } else {
    if (!resultFile) {
      throw new PaidFlowError(
        "configuration",
        "PROOF_RESULT_FILE is required for a funded proof",
      );
    }
    const signer = await loadSigner(config);
    const core = new x402Client().register(
      config.network,
      config.network.startsWith("near:")
        ? new ExactNearScheme(signer)
        : new ExactEvmScheme(signer),
    );
    const result = await submitPaidRequest({
      attemptId,
      httpClient: new x402HTTPClient(core),
      paymentRequired: discovered.paymentRequired,
      preview,
      record,
      request,
      timeoutMs: config.timeoutMs,
    });
    emit(result);
    process.exitCode = exitCode(result.outcome);
  }
} catch (error) {
  const result = {
    attemptId,
    outcome: "preflight_failed",
    paidRequestSent: false,
    stage: error instanceof PaidFlowError ? error.stage : "unknown",
    error: safeError(error),
    ...(error instanceof PaidFlowError && Object.keys(error.details).length > 0
      ? { details: error.details }
      : {}),
  };
  await record(result).catch(() => {});
  emit(result);
  process.exitCode = 1;
}

function loadConfig() {
  const network = required("PROOF_EXPECTED_NETWORK");
  const method = (process.env.PROOF_METHOD ?? "POST").toUpperCase();
  if (!["GET", "POST"].includes(method)) {
    throw new PaidFlowError("configuration", "PROOF_METHOD must be GET or POST");
  }
  let body;
  if (process.env.PROOF_BODY_JSON !== undefined) {
    try {
      body = JSON.parse(process.env.PROOF_BODY_JSON);
    } catch {
      throw new PaidFlowError("configuration", "PROOF_BODY_JSON must be valid JSON");
    }
  }
  const timeoutMs = Number(process.env.PROOF_TIMEOUT_MS ?? 45_000);
  if (!Number.isInteger(timeoutMs) || timeoutMs < 1_000 || timeoutMs > 120_000) {
    throw new PaidFlowError(
      "configuration",
      "PROOF_TIMEOUT_MS must be between 1000 and 120000",
    );
  }
  return {
    network,
    method,
    body,
    timeoutMs,
    url: required("PROOF_URL"),
    payer: required("PROOF_PAYER"),
    payerKeyFile: required("PROOF_PAYER_KEY_FILE"),
    rpcUrl: required("PROOF_RPC_URL"),
    facilitatorSigner: required("PROOF_FACILITATOR_SIGNER"),
    maximumSponsoredGas: required("PROOF_MAX_SPONSORED_GAS"),
    expected: {
      network,
      asset: required("PROOF_EXPECTED_ASSET"),
      amount: required("PROOF_EXPECTED_AMOUNT"),
      payTo: required("PROOF_EXPECTED_PAY_TO"),
    },
  };
}

async function loadSigner(config) {
  const credential = (await readFile(config.payerKeyFile, "utf8")).trim();
  if (!credential) {
    throw new PaidFlowError(
      "credential",
      "payer credential file must not be empty",
    );
  }

  if (config.network.startsWith("near:")) {
    let secretKey = credential;
    if (credential.startsWith("{")) {
      let parsed;
      try {
        parsed = JSON.parse(credential);
      } catch {
        throw new PaidFlowError("credential", "NEAR payer credential JSON is invalid");
      }
      secretKey = parsed.private_key;
      if (parsed.account_id && parsed.account_id !== config.payer) {
        throw new PaidFlowError(
          "credential",
          "NEAR credential account does not match PROOF_PAYER",
        );
      }
    }
    if (typeof secretKey !== "string") {
      throw new PaidFlowError("credential", "NEAR payer credential has no private_key");
    }
    return createClientNearSigner({
      accountId: config.payer,
      secretKey,
      rpcUrls: { [config.network]: config.rpcUrl },
      gas: 30_000_000_000_000n,
    });
  }

  if (!config.network.startsWith("eip155:")) {
    throw new PaidFlowError("configuration", "unsupported proof network");
  }
  if (credential.includes("\n") || credential.includes("\r")) {
    throw new PaidFlowError("credential", "EVM payer credential must contain one key");
  }
  const account = privateKeyToAccount(credential);
  if (account.address.toLowerCase() !== config.payer.toLowerCase()) {
    throw new PaidFlowError(
      "credential",
      "EVM credential address does not match PROOF_PAYER",
    );
  }
  return account;
}

function required(name) {
  const value = process.env[name];
  if (!value) throw new PaidFlowError("configuration", `${name} is required`);
  return value;
}

function createRecorder(path) {
  return async value => {
    const temporary = `${path}.${process.pid}.tmp`;
    await writeFile(temporary, `${JSON.stringify(value, null, 2)}\n`, {
      encoding: "utf8",
      mode: 0o600,
    });
    await chmod(temporary, 0o600);
    await rename(temporary, path);
  };
}

function hash(value) {
  return createHash("sha256").update(value).digest("hex");
}

function emit(value) {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}

function exitCode(outcome) {
  if (outcome === "settled") return 0;
  if (outcome === "settled_resource_error") return 4;
  if (outcome === "indeterminate") return 2;
  return 1;
}

function safeError(error) {
  if (typeof error?.message === "string" && error.message.length > 0) {
    return error.message.slice(0, 500);
  }
  return "unknown error";
}
