const DEFAULT_TTL_MS = 1_000;

export function createReadinessCache({
  check,
  now = () => Date.now(),
  ttlMs = DEFAULT_TTL_MS,
} = {}) {
  if (typeof check !== "function") throw new TypeError("check must be a function");
  if (!Number.isInteger(ttlMs) || ttlMs < 0) {
    throw new TypeError("ttlMs must be a non-negative integer");
  }

  let cached;
  let inFlight;

  return async function checkReadiness() {
    const timestamp = now();
    if (cached && timestamp < cached.expiresAt) return cached.value;

    if (!inFlight) {
      inFlight = Promise.resolve()
        .then(check)
        .then(value => {
          cached = { value, expiresAt: now() + ttlMs };
          return value;
        })
        .finally(() => {
          inFlight = undefined;
        });
    }
    return inFlight;
  };
}
