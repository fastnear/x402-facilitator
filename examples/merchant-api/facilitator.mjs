import {
  FacilitatorResponseError,
  SettleError,
  VerifyError,
} from "@x402/core/types";
import { z } from "@x402/core/schemas";

const READY_TIMEOUT_MS = 12_000;
// Three settlement attempts plus the prescribed 1.5 s and 3 s waits take at
// most 25.5 s. Keep that below nginx's 30 s upstream timeout so a merchant
// client never receives a gateway timeout while this process keeps retrying.
const FACILITATOR_REQUEST_TIMEOUT_MS = 7_000;
const MAX_FACILITATOR_RESPONSE_BYTES = 64 * 1024;

const verifyResponseSchema = z.object({
  isValid: z.boolean(),
  invalidReason: z.string().nullish().transform(value => value ?? undefined),
  invalidMessage: z.string().nullish().transform(value => value ?? undefined),
  payer: z.string().nullish().transform(value => value ?? undefined),
  extensions: z.record(z.string(), z.unknown()).nullish().transform(value => value ?? undefined),
  extra: z.record(z.string(), z.unknown()).nullish().transform(value => value ?? undefined),
});
const settleResponseSchema = z.object({
  success: z.boolean(),
  errorReason: z.string().nullish().transform(value => value ?? undefined),
  errorMessage: z.string().nullish().transform(value => value ?? undefined),
  payer: z.string().nullish().transform(value => value ?? undefined),
  transaction: z.string(),
  network: z.custom(value => typeof value === "string"),
  amount: z.string().nullish().transform(value => value ?? undefined),
  extensions: z.record(z.string(), z.unknown()).nullish().transform(value => value ?? undefined),
  extra: z.record(z.string(), z.unknown()).nullish().transform(value => value ?? undefined),
});
const supportedKindSchema = z.object({
  x402Version: z.number(),
  scheme: z.string(),
  network: z.custom(value => typeof value === "string"),
  extra: z.record(z.string(), z.unknown()).nullish().transform(value => value ?? undefined),
});
const supportedResponseSchema = z.object({
  kinds: z.array(supportedKindSchema),
  extensions: z.array(z.string()).default([]),
  signers: z.record(z.string(), z.array(z.string())).default({}),
});

export class FacilitatorTimeoutError extends Error {
  constructor(message) {
    super(message);
    this.name = "FacilitatorTimeoutError";
  }
}

export class FacilitatorHttpError extends Error {
  constructor(operation, statusCode) {
    super(`Facilitator ${operation} failed (${statusCode})`);
    this.name = "FacilitatorHttpError";
    this.statusCode = statusCode;
  }
}

// The upstream client follows redirects for payment-bearing POSTs. This
// implementation intentionally owns the merchant's facilitator transport so
// credentials and signed payment payloads are never replayed to a redirect
// target. It otherwise mirrors the pinned client wire shape and result types.
export class MerchantFacilitatorClient {
  constructor({
    url,
    createAuthHeaders,
    fetchImpl = globalThis.fetch,
    requestTimeoutMs = FACILITATOR_REQUEST_TIMEOUT_MS,
    maxResponseBytes = MAX_FACILITATOR_RESPONSE_BYTES,
    setTimeoutImpl = setTimeout,
    clearTimeoutImpl = clearTimeout,
  } = {}) {
    if (typeof url !== "string" || url.length === 0) {
      throw new Error("facilitator URL must be a non-empty string");
    }
    if (createAuthHeaders !== undefined && typeof createAuthHeaders !== "function") {
      throw new Error("createAuthHeaders must be a function");
    }
    if (typeof fetchImpl !== "function") {
      throw new Error("fetchImpl must be a function");
    }
    if (!Number.isSafeInteger(requestTimeoutMs) || requestTimeoutMs <= 0) {
      throw new Error("facilitator request timeout must be a positive integer");
    }
    if (!Number.isSafeInteger(maxResponseBytes) || maxResponseBytes <= 0) {
      throw new Error("facilitator response limit must be a positive integer");
    }

    this.url = url.replace(/\/+$/, "");
    this._createAuthHeaders = createAuthHeaders;
    this.fetchImpl = fetchImpl;
    this.requestTimeoutMs = requestTimeoutMs;
    this.maxResponseBytes = maxResponseBytes;
    this.setTimeoutImpl = setTimeoutImpl;
    this.clearTimeoutImpl = clearTimeoutImpl;
  }

  async verify(paymentPayload, paymentRequirements) {
    const { response, text } = await this.requestPayment(
      "verify",
      paymentPayload,
      paymentRequirements,
    );
    if (!response.ok) {
      const data = parseJson(text);
      if (isRecord(data) && "isValid" in data) {
        throw new VerifyError(response.status, data);
      }
      throw new FacilitatorHttpError("verify", response.status);
    }
    return parseSuccessResponse(text, verifyResponseSchema, "verify");
  }

  async settle(paymentPayload, paymentRequirements) {
    const { response, text } = await this.requestPayment(
      "settle",
      paymentPayload,
      paymentRequirements,
    );
    if (!response.ok) {
      const data = parseJson(text);
      if (isRecord(data) && "success" in data) {
        throw new SettleError(response.status, data);
      }
      throw new FacilitatorHttpError("settle", response.status);
    }
    return parseSuccessResponse(text, settleResponseSchema, "settle");
  }

  async getSupported({ signal } = {}) {
    // Readiness and startup must not create background retry chains after the
    // caller's bounded probe has timed out. Only payment-bearing verify and
    // settle operations use the prescribed retry policy; a later readiness
    // probe can retry this one bounded discovery request.
    const { response, text } = await this.request(
      "supported",
      async () => ({
        method: "GET",
        headers: await this.headersFor("supported"),
      }),
      { signal },
    );
    if (!response.ok) {
      throw new FacilitatorHttpError("getSupported", response.status);
    }
    return parseSuccessResponse(text, supportedResponseSchema, "supported");
  }

  async createAuthHeaders(path) {
    if (!this._createAuthHeaders) return { headers: {} };
    const authHeaders = await this._createAuthHeaders();
    return { headers: authHeaders[path] ?? {} };
  }

  toJsonSafe(object) {
    return JSON.parse(JSON.stringify(
      object,
      (_key, value) => typeof value === "bigint" ? value.toString() : value,
    ));
  }

  async requestPayment(operation, paymentPayload, paymentRequirements) {
    return this.request(operation, async () => ({
      method: "POST",
      headers: await this.headersFor(operation),
      body: JSON.stringify({
        x402Version: paymentPayload.x402Version,
        paymentPayload: this.toJsonSafe(paymentPayload),
        paymentRequirements: this.toJsonSafe(paymentRequirements),
      }),
    }));
  }

  async headersFor(operation) {
    const authHeaders = await this.createAuthHeaders(operation);
    return {
      "Content-Type": "application/json",
      ...authHeaders.headers,
    };
  }

  async request(operation, createInit, { signal } = {}) {
    const controller = new AbortController();
    let removeAbortListener;
    if (signal) {
      const abort = () => controller.abort();
      if (signal.aborted) {
        abort();
      } else {
        signal.addEventListener("abort", abort, { once: true });
        removeAbortListener = () => signal.removeEventListener("abort", abort);
      }
    }
    try {
      return await withDeadline(
        async () => {
          const init = await createInit();
          const response = await this.fetchImpl(`${this.url}/${operation}`, {
            ...init,
            redirect: "error",
            signal: controller.signal,
          });
          return {
            response,
            text: await readBoundedResponseText(
              response,
              this.maxResponseBytes,
              operation,
            ),
          };
        },
        {
          timeoutMs: this.requestTimeoutMs,
          message: `facilitator ${operation} request timed out`,
          onTimeout: () => controller.abort(),
          setTimeoutImpl: this.setTimeoutImpl,
          clearTimeoutImpl: this.clearTimeoutImpl,
        },
      );
    } finally {
      removeAbortListener?.();
      controller.abort();
    }
  }
}

export async function withRetries(
  operation,
  {
    retries,
    delaysMs,
    sleep = defaultSleep,
    shouldRetry = isRetryableFacilitatorError,
  },
) {
  let lastError;
  for (let attempt = 0; attempt <= retries; attempt += 1) {
    try {
      return await operation();
    } catch (error) {
      lastError = error;
      if (attempt === retries || !shouldRetry(error)) break;
      await sleep(delaysMs[Math.min(attempt, delaysMs.length - 1)]);
    }
  }
  throw lastError;
}

export function isRetryableFacilitatorError(error) {
  if (error instanceof FacilitatorResponseError) return false;
  if (!Number.isInteger(error?.statusCode)) return true;
  return error.statusCode === 429
    || (error.statusCode >= 500 && error.statusCode <= 599);
}

function parseJson(text) {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

function parseSuccessResponse(text, schema, operation) {
  const data = parseJson(text);
  if (data === undefined) {
    throw new FacilitatorResponseError(
      `Facilitator ${operation} returned invalid JSON`,
    );
  }
  const parsed = schema.safeParse(data);
  if (!parsed.success) {
    throw new FacilitatorResponseError(
      `Facilitator ${operation} returned invalid data`,
    );
  }
  return parsed.data;
}

function isRecord(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

async function readBoundedResponseText(response, maxResponseBytes, operation) {
  const declaredLength = response.headers.get("content-length");
  if (declaredLength && /^\d+$/.test(declaredLength)
    && Number(declaredLength) > maxResponseBytes) {
    if (response.body) await response.body.cancel().catch(() => {});
    throw new FacilitatorResponseError(
      `Facilitator ${operation} response exceeded ${maxResponseBytes} byte limit`,
    );
  }
  if (!response.body) return "";

  const reader = response.body.getReader();
  const chunks = [];
  let length = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      length += value.byteLength;
      if (length > maxResponseBytes) {
        await reader.cancel().catch(() => {});
        throw new FacilitatorResponseError(
          `Facilitator ${operation} response exceeded ${maxResponseBytes} byte limit`,
        );
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }

  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return new TextDecoder().decode(bytes);
}

export function withFacilitatorRetries(
  client,
  {
    sleep,
    getSupportedTimeoutMs = READY_TIMEOUT_MS,
    setTimeoutImpl = setTimeout,
    clearTimeoutImpl = clearTimeout,
  } = {},
) {
  const verify = client.verify.bind(client);
  const settle = client.settle.bind(client);
  client.verify = (...callArguments) =>
    withRetries(
      () => verify(...callArguments),
      { retries: 1, delaysMs: [1000], sleep },
    );
  client.settle = (...callArguments) =>
    withRetries(
      () => settle(...callArguments),
      { retries: 2, delaysMs: [1500, 3000], sleep },
    );
  if (typeof client.getSupported === "function") {
    const getSupported = client.getSupported.bind(client);
    client.getSupported = (...callArguments) => withDeadline(
      () => getSupported(...callArguments),
      {
        timeoutMs: getSupportedTimeoutMs,
        message: "facilitator supported-kinds request timed out",
        setTimeoutImpl,
        clearTimeoutImpl,
      },
    );
  }
  return client;
}

export function createFacilitatorProbe({
  client,
  facilitatorUrl,
  network,
  fetchImpl = globalThis.fetch,
  timeoutMs = READY_TIMEOUT_MS,
  setTimeoutImpl = setTimeout,
  clearTimeoutImpl = clearTimeout,
}) {
  if (typeof client?.getSupported !== "function") {
    throw new Error("facilitator client must implement getSupported()");
  }
  if (typeof fetchImpl !== "function") {
    throw new Error("fetchImpl must be a function");
  }

  return {
    async check() {
      const controller = new AbortController();
      try {
        const [supported] = await Promise.all([
          withDeadline(
            () => client.getSupported({ signal: controller.signal }),
            {
              timeoutMs,
              message: "facilitator readiness timed out",
              onTimeout: () => controller.abort(),
              setTimeoutImpl,
              clearTimeoutImpl,
            },
          ),
          withDeadline(
            async () => {
              const response = await fetchImpl(`${facilitatorUrl}/readyz`, {
                method: "GET",
                redirect: "error",
                signal: controller.signal,
              });
              if (!response.ok) {
                throw new Error(`facilitator readiness returned HTTP ${response.status}`);
              }
              const body = await response.json();
              if (body?.ready !== true) {
                throw new Error("facilitator reported not ready");
              }
              return body;
            },
            {
              timeoutMs,
              message: "facilitator readiness timed out",
              onTimeout: () => controller.abort(),
              setTimeoutImpl,
              clearTimeoutImpl,
            },
          ),
        ]);
        const kind = supported?.kinds?.find(
          candidate =>
            candidate?.x402Version === 2
            && candidate?.scheme === "exact"
            && candidate?.network === network,
        );
        if (!kind) {
          throw new Error(
            `facilitator does not advertise exact x402 v2 for ${network}`,
          );
        }
        return { network, ready: true };
      } catch (error) {
        if (
          error instanceof FacilitatorTimeoutError
          || error?.name === "AbortError"
        ) {
          throw new Error("facilitator readiness timed out");
        }
        throw error;
      } finally {
        controller.abort();
      }
    },
  };
}

function withDeadline(
  operation,
  {
    timeoutMs,
    message,
    onTimeout,
    setTimeoutImpl,
    clearTimeoutImpl,
  },
) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const timer = setTimeoutImpl(() => {
      if (settled) return;
      settled = true;
      onTimeout?.();
      reject(new FacilitatorTimeoutError(message));
    }, timeoutMs);
    Promise.resolve()
      .then(operation)
      .then(
        value => {
          if (settled) return;
          settled = true;
          clearTimeoutImpl(timer);
          resolve(value);
        },
        error => {
          if (settled) return;
          settled = true;
          clearTimeoutImpl(timer);
          reject(error);
        },
      );
  });
}

function defaultSleep(milliseconds) {
  return new Promise(resolve => setTimeout(resolve, milliseconds));
}
