const DEFAULT_TIMEOUT_MS = 45_000;
const MAX_RESPONSE_TEXT = 64_000;

export class PaidFlowError extends Error {
  constructor(stage, message, details = {}) {
    super(message);
    this.name = "PaidFlowError";
    this.stage = stage;
    this.details = details;
  }
}

export function summarizePaymentRequired(paymentRequired) {
  if (paymentRequired?.x402Version !== 2) {
    throw new PaidFlowError("payment_required", "merchant did not return canonical x402 v2");
  }
  if (!Array.isArray(paymentRequired.accepts) || paymentRequired.accepts.length !== 1) {
    throw new PaidFlowError("payment_required", "merchant must return exactly one payment requirement");
  }
  const accepted = paymentRequired.accepts[0];
  for (const field of ["scheme", "network", "asset", "amount", "payTo"]) {
    if (typeof accepted?.[field] !== "string" || accepted[field].length === 0) {
      throw new PaidFlowError("payment_required", `payment requirement is missing ${field}`);
    }
  }
  return {
    x402Version: 2,
    scheme: accepted.scheme,
    network: accepted.network,
    asset: accepted.asset,
    amount: accepted.amount,
    payTo: accepted.payTo,
  };
}

export function assertExpectedRequirement(actual, expected) {
  for (const field of ["network", "asset", "amount", "payTo"]) {
    if (typeof expected?.[field] !== "string" || expected[field].length === 0) {
      throw new PaidFlowError("configuration", `expected ${field} is required`);
    }
    const caseInsensitive = field === "asset" || field === "payTo";
    const left = caseInsensitive ? actual[field].toLowerCase() : actual[field];
    const right = caseInsensitive ? expected[field].toLowerCase() : expected[field];
    if (left !== right) {
      throw new PaidFlowError("requirement_mismatch", `${field} did not match the approved value`, {
        field,
        expected: expected[field],
        actual: actual[field],
      });
    }
  }
}

export async function discoverPaymentRequired({
  fetchImpl = fetch,
  httpClient,
  request,
  timeoutMs = DEFAULT_TIMEOUT_MS,
}) {
  let response;
  try {
    response = await fetchWithTimeout(fetchImpl, request, timeoutMs);
  } catch (error) {
    throw new PaidFlowError("unpaid_request", "unpaid discovery request failed", {
      reason: safeError(error),
    });
  }

  const responseBody = await readResponseBodyWithTimeout(response, timeoutMs);
  if (response.status !== 402) {
    throw new PaidFlowError("unpaid_request", `expected HTTP 402, received ${response.status}`, {
      status: response.status,
      body: responseBody.body,
    });
  }

  let paymentRequired;
  try {
    paymentRequired = httpClient.getPaymentRequiredResponse(
      name => response.headers.get(name),
      responseBody.body,
    );
  } catch (error) {
    throw new PaidFlowError("payment_required", "could not decode PAYMENT-REQUIRED", {
      reason: safeError(error),
    });
  }

  return {
    paymentRequired,
    requirement: summarizePaymentRequired(paymentRequired),
    unpaidResponse: {
      status: response.status,
      body: responseBody.body,
      bodyTruncated: responseBody.truncated,
    },
  };
}

export async function submitPaidRequest({
  attemptId,
  fetchImpl = fetch,
  httpClient,
  paymentRequired,
  preview,
  record = async () => {},
  request,
  timeoutMs = DEFAULT_TIMEOUT_MS,
}) {
  let paymentPayload;
  try {
    paymentPayload = await httpClient.createPaymentPayload(paymentRequired);
  } catch (error) {
    const result = {
      attemptId,
      outcome: "payment_creation_failed",
      paidRequestSent: false,
      preview,
      error: safeError(error),
    };
    await record(result);
    return result;
  }

  let paymentHeaders;
  try {
    paymentHeaders = httpClient.encodePaymentSignatureHeader(paymentPayload);
  } catch (error) {
    const result = {
      attemptId,
      outcome: "payment_encoding_failed",
      paidRequestSent: false,
      preview,
      error: safeError(error),
    };
    await record(result);
    return result;
  }

  const checkpoint = {
    attemptId,
    outcome: "broadcasting",
    paidRequestSent: "indeterminate",
    preview,
    reconcileBeforeRetry: true,
    updatedAt: new Date().toISOString(),
  };
  await record(checkpoint);

  let response;
  try {
    response = await fetchWithTimeout(
      fetchImpl,
      {
        ...request,
        headers: { ...request.headers, ...paymentHeaders },
      },
      timeoutMs,
    );
  } catch (error) {
    const result = {
      attemptId,
      outcome: "indeterminate",
      paidRequestSent: "indeterminate",
      preview,
      error: safeError(error),
      reconcileBeforeRetry: true,
    };
    await record(result);
    return result;
  } finally {
    paymentPayload = undefined;
    paymentHeaders = undefined;
  }

  const settlementHeaderPresent = Boolean(
    response.headers.get("payment-response") ?? response.headers.get("x-payment-response"),
  );
  let settlement;
  let settlementDecodeError;
  if (settlementHeaderPresent) {
    try {
      settlement = sanitizeSettlement(
        httpClient.getPaymentSettleResponse(name => response.headers.get(name)),
      );
      if (settlement.network !== preview.network) {
        throw new Error("settlement network did not match the approved network");
      }
    } catch (error) {
      settlementDecodeError = safeError(error);
    }
  }

  const responseBody = await readResponseBodyWithTimeout(response, timeoutMs).catch(error => ({
    body: null,
    truncated: false,
    error: safeError(error),
  }));
  const outcome = classifyOutcome(response.status, settlement, settlementDecodeError);
  const result = {
    attemptId,
    outcome,
    paidRequestSent: true,
    preview,
    response: {
      status: response.status,
      body: responseBody.body,
      bodyTruncated: responseBody.truncated,
      ...(responseBody.error ? { bodyReadError: responseBody.error } : {}),
    },
    settlementHeaderPresent,
    ...(settlement ? { settlement } : {}),
    ...(settlementDecodeError ? { settlementDecodeError } : {}),
    ...(outcome === "indeterminate" ? { reconcileBeforeRetry: true } : {}),
  };
  try {
    await record(result);
  } catch (error) {
    result.resultPersistenceError = safeError(error);
    result.reconcileBeforeRetry = true;
  }
  return result;
}

function classifyOutcome(status, settlement, settlementDecodeError) {
  if (settlement?.success === true) {
    return status >= 200 && status < 300 ? "settled" : "settled_resource_error";
  }
  if (settlement?.success === false || status === 402) return "payment_failed";
  if (settlementDecodeError || !settlement) return "indeterminate";
  return "indeterminate";
}

function sanitizeSettlement(value) {
  if (!value || typeof value !== "object") {
    throw new Error("settlement response was not an object");
  }
  if (typeof value.success !== "boolean") {
    throw new Error("settlement response did not contain a boolean success field");
  }
  if (value.success && (
    typeof value.network !== "string"
    || value.network.length === 0
    || typeof value.transaction !== "string"
    || value.transaction.length === 0
  )) {
    throw new Error("successful settlement response was incomplete");
  }
  return {
    success: value.success,
    ...(typeof value.network === "string" ? { network: value.network } : {}),
    ...(typeof value.transaction === "string" ? { transaction: value.transaction } : {}),
    ...(typeof value.errorReason === "string" ? { errorReason: value.errorReason } : {}),
    ...(typeof value.payer === "string" ? { payer: value.payer } : {}),
  };
}

async function fetchWithTimeout(fetchImpl, request, timeoutMs) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetchImpl(request.url, {
      method: request.method,
      headers: request.headers,
      body: request.body,
      signal: controller.signal,
    });
  } finally {
    clearTimeout(timer);
  }
}

async function readResponseBody(response) {
  const text = await response.text();
  const truncated = text.length > MAX_RESPONSE_TEXT;
  const bounded = truncated ? text.slice(0, MAX_RESPONSE_TEXT) : text;
  if (bounded.length === 0) return { body: null, truncated };
  try {
    return { body: JSON.parse(bounded), truncated };
  } catch {
    return { body: bounded, truncated };
  }
}

async function readResponseBodyWithTimeout(response, timeoutMs) {
  let timer;
  try {
    return await Promise.race([
      readResponseBody(response),
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error("response body timed out")), timeoutMs);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

function safeError(error) {
  if (error?.name === "AbortError") return "request timed out";
  if (typeof error?.message === "string" && error.message.length > 0) {
    return error.message.slice(0, 500);
  }
  return "unknown error";
}
