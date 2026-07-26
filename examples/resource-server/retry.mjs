// Bounded retry for facilitator calls. The facilitator answers transient
// conditions (RPC throttling, a settlement still reaching its terminal
// state) with retryable HTTP errors; the official middleware surfaces any
// settle throw as a client-facing failure even when the settlement journal
// later records success (the 2026-07-26 paid-but-undelivered incident).
// Retrying is safe on both operations: verify is read-only, and settle is
// idempotent at the facilitator — a repeat of the same payment replays the
// journaled terminal result rather than settling twice.

export async function withRetries(operation, { retries, delaysMs, sleep = defaultSleep }) {
  let lastError;
  for (let attempt = 0; attempt <= retries; attempt += 1) {
    if (attempt > 0) {
      await sleep(delaysMs[Math.min(attempt - 1, delaysMs.length - 1)]);
    }
    try {
      return await operation();
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError;
}

function defaultSleep(ms) {
  return new Promise(resolve => {
    setTimeout(resolve, ms);
  });
}

// Wrap a facilitator client so verify throws retry once and settle throws
// retry twice. Protocol-level outcomes (isValid: false, success: false) are
// resolved values, not throws, and are never retried.
export function withFacilitatorRetries(client, { sleep } = {}) {
  const verify = client.verify.bind(client);
  const settle = client.settle.bind(client);
  client.verify = (...callArguments) =>
    withRetries(() => verify(...callArguments), { retries: 1, delaysMs: [1000], sleep });
  client.settle = (...callArguments) =>
    withRetries(() => settle(...callArguments), { retries: 2, delaysMs: [1500, 3000], sleep });
  return client;
}
