const READY_TIMEOUT_MS = 12_000;

export class FacilitatorTimeoutError extends Error {
  constructor(message) {
    super(message);
    this.name = "FacilitatorTimeoutError";
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
  if (!Number.isInteger(error?.statusCode)) return true;
  return error.statusCode === 429
    || (error.statusCode >= 500 && error.statusCode <= 599);
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
            () => client.getSupported(),
            {
              timeoutMs,
              message: "facilitator readiness timed out",
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
