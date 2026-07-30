import assert from "node:assert/strict";
import http from "node:http";
import test from "node:test";

import {
  FacilitatorResponseError,
  SettleError,
  VerifyError,
} from "@x402/core/types";

import {
  createFacilitatorProbe,
  FacilitatorHttpError,
  FacilitatorTimeoutError,
  isRetryableFacilitatorError,
  MerchantFacilitatorClient,
  withFacilitatorRetries,
  withRetries,
} from "./facilitator.mjs";

function recordingSleep(record) {
  return milliseconds => {
    record.push(milliseconds);
    return Promise.resolve();
  };
}

function manualTimers() {
  const callbacks = [];
  return {
    callbacks,
    setTimeoutImpl(callback) {
      callbacks.push(callback);
      return callback;
    },
    clearTimeoutImpl() {},
  };
}

async function serve(handler) {
  const server = http.createServer(handler);
  await new Promise((resolve, reject) => {
    server.listen(0, "127.0.0.1", resolve);
    server.once("error", reject);
  });
  const address = server.address();
  return {
    origin: `http://127.0.0.1:${address.port}`,
    close: () => new Promise((resolve, reject) =>
      server.close(error => error ? reject(error) : resolve())),
  };
}

async function requestBody(request) {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}

function paymentPayload() {
  return {
    x402Version: 2,
    accepted: { scheme: "exact", network: "eip155:8453" },
    payload: { authorization: "signed-payment-bearer" },
  };
}

function paymentRequirements() {
  return {
    scheme: "exact",
    network: "eip155:8453",
    asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    amount: "1000",
    payTo: "0x1111111111111111111111111111111111111111",
  };
}

function merchantFacilitator(options = {}) {
  return new MerchantFacilitatorClient({
    url: "https://facilitator.example",
    createAuthHeaders: async () => ({
      supported: {},
      verify: { "X-API-Key": "merchant-api-key" },
      settle: { "X-API-Key": "merchant-api-key" },
    }),
    ...options,
  });
}

test("withRetries is bounded and uses the configured delays", async () => {
  const slept = [];
  let calls = 0;
  const result = await withRetries(
    async () => {
      calls += 1;
      if (calls < 3) throw new Error("transient");
      return "ok";
    },
    { retries: 2, delaysMs: [10, 20], sleep: recordingSleep(slept) },
  );
  assert.equal(result, "ok");
  assert.equal(calls, 3);
  assert.deepEqual(slept, [10, 20]);

  await assert.rejects(
    withRetries(
      async () => {
        throw new Error("last");
      },
      { retries: 1, delaysMs: [5], sleep: recordingSleep(slept) },
    ),
    /last/,
  );
});

test("merchant facilitator rejects redirects without forwarding credentials or payment bodies", async t => {
  const redirectedRequests = [];
  const redirected = await serve(async (request, response) => {
    redirectedRequests.push({
      headers: request.headers,
      body: await requestBody(request),
      path: request.url,
    });
    response.statusCode = 500;
    response.end();
  });
  t.after(redirected.close);

  const sourceRequests = [];
  const source = await serve(async (request, response) => {
    sourceRequests.push({
      headers: request.headers,
      body: await requestBody(request),
      path: request.url,
    });
    response.writeHead(307, { location: `${redirected.origin}/capture` });
    response.end();
  });
  t.after(source.close);

  const client = merchantFacilitator({ url: source.origin });
  const payload = paymentPayload();
  const requirements = paymentRequirements();

  await assert.rejects(() => client.verify(payload, requirements));
  await assert.rejects(() => client.settle(payload, requirements));

  assert.deepEqual(
    sourceRequests.map(request => ({
      path: request.path,
      apiKey: request.headers["x-api-key"],
      body: JSON.parse(request.body),
    })),
    [
      {
        path: "/verify",
        apiKey: "merchant-api-key",
        body: {
          x402Version: 2,
          paymentPayload: payload,
          paymentRequirements: requirements,
        },
      },
      {
        path: "/settle",
        apiKey: "merchant-api-key",
        body: {
          x402Version: 2,
          paymentPayload: payload,
          paymentRequirements: requirements,
        },
      },
    ],
  );
  assert.deepEqual(redirectedRequests, []);
});

test("merchant facilitator preserves typed protocol failures and bounds untrusted response bodies", async () => {
  const responses = [
    Response.json({ isValid: false, invalidReason: "invalid_payment" }, { status: 400 }),
    Response.json({
      success: false,
      errorReason: "invalid_payment",
      transaction: "",
      network: "eip155:8453",
    }, { status: 409 }),
    new Response("untrusted remote detail", { status: 503 }),
    new Response("x".repeat(17), { status: 200 }),
  ];
  const client = merchantFacilitator({
    fetchImpl: async () => responses.shift(),
  });

  await assert.rejects(
    () => client.verify(paymentPayload(), paymentRequirements()),
    error => error instanceof VerifyError
      && error.statusCode === 400
      && error.invalidReason === "invalid_payment",
  );
  await assert.rejects(
    () => client.settle(paymentPayload(), paymentRequirements()),
    error => error instanceof SettleError
      && error.statusCode === 409
      && error.errorReason === "invalid_payment",
  );
  await assert.rejects(
    () => client.verify(paymentPayload(), paymentRequirements()),
    error => error instanceof FacilitatorHttpError
      && error.statusCode === 503
      && !error.message.includes("untrusted remote detail"),
  );
  const bodyLimitedClient = merchantFacilitator({
    fetchImpl: async () => responses.shift(),
    maxResponseBytes: 16,
  });
  await assert.rejects(
    () => bodyLimitedClient.verify(paymentPayload(), paymentRequirements()),
    error => error instanceof FacilitatorResponseError
      && /exceeded 16 byte limit/.test(error.message),
  );
  assert.equal(
    isRetryableFacilitatorError(
      new FacilitatorResponseError("malformed response"),
    ),
    false,
  );
});

test("merchant facilitator aborts a hanging per-attempt request at its deadline", async () => {
  const timers = manualTimers();
  let signal;
  const client = merchantFacilitator({
    fetchImpl: async (_url, request) => {
      signal = request.signal;
      return new Promise(() => {});
    },
    ...timers,
  });

  const verifying = client.verify(paymentPayload(), paymentRequirements());
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(timers.callbacks.length, 1);
  timers.callbacks[0]();
  await assert.rejects(
    verifying,
    error => error instanceof FacilitatorTimeoutError
      && error.message === "facilitator verify request timed out",
  );
  assert.equal(signal.aborted, true);
});

test("merchant facilitator aborts a hanging supported request at its deadline", async () => {
  const timers = manualTimers();
  let signal;
  const client = merchantFacilitator({
    fetchImpl: async (_url, request) => {
      signal = request.signal;
      return new Promise(() => {});
    },
    ...timers,
  });

  const checking = client.getSupported();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(timers.callbacks.length, 1);
  timers.callbacks[0]();
  await assert.rejects(
    checking,
    error => error instanceof FacilitatorTimeoutError
      && error.message === "facilitator supported request timed out",
  );
  assert.equal(signal.aborted, true);
});

test("merchant facilitator aborts a supported request when its caller cancels", async () => {
  const controller = new AbortController();
  let signal;
  let calls = 0;
  let signalFetchStarted;
  const fetchStarted = new Promise(resolve => {
    signalFetchStarted = resolve;
  });
  const client = merchantFacilitator({
    fetchImpl: async (_url, request) => {
      calls += 1;
      signal = request.signal;
      signalFetchStarted();
      return new Promise((resolve, reject) => {
        request.signal.addEventListener("abort", () => {
          const error = new Error("request aborted");
          error.name = "AbortError";
          reject(error);
        }, { once: true });
      });
    },
  });

  const checking = client.getSupported({ signal: controller.signal });
  await fetchStarted;
  controller.abort();
  await assert.rejects(checking, error => error?.name === "AbortError");
  assert.equal(calls, 1);
  assert.equal(signal.aborted, true);
});

test("merchant facilitator defaults stay inside the nginx retry envelope", () => {
  assert.equal(merchantFacilitator().requestTimeoutMs, 7_000);
  assert.ok(3 * 7_000 + 1_500 + 3_000 < 30_000);
});

test("merchant facilitator does not retry a rate-limited supported check", async () => {
  let calls = 0;
  const client = merchantFacilitator({
    // The pre-hardening client used this hook for its 429 retry delay. Keep it
    // immediate so this test detects any later reintroduction of that retry.
    sleep: () => Promise.resolve(),
    fetchImpl: async url => {
      assert.match(url, /\/supported$/);
      calls += 1;
      return new Response("", { status: 429, headers: { "retry-after": "30" } });
    },
  });

  await assert.rejects(
    () => client.getSupported(),
    error => error instanceof FacilitatorHttpError && error.statusCode === 429,
  );
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(calls, 1);
});

test("facilitator wrapper retries throws but not resolved protocol failures", async () => {
  const slept = [];
  let verifyCalls = 0;
  let settleCalls = 0;
  const client = {
    async verify() {
      verifyCalls += 1;
      if (verifyCalls === 1) throw new Error("temporarily unavailable");
      return { isValid: false, invalidReason: "insufficient_funds" };
    },
    async settle() {
      settleCalls += 1;
      if (settleCalls < 3) throw new Error("settlement pending");
      return { success: false, errorReason: "invalid_payment" };
    },
  };
  withFacilitatorRetries(client, { sleep: recordingSleep(slept) });

  assert.deepEqual(
    await client.verify({}, {}),
    { isValid: false, invalidReason: "insufficient_funds" },
  );
  assert.deepEqual(
    await client.settle({}, {}),
    { success: false, errorReason: "invalid_payment" },
  );
  assert.equal(verifyCalls, 2);
  assert.equal(settleCalls, 3);
  assert.deepEqual(slept, [1000, 1500, 3000]);
});

test("facilitator wrapper retries only typed transient HTTP errors", async () => {
  for (const { statusCode, calls: expectedCalls, delays } of [
    { statusCode: 400, calls: 1, delays: [] },
    { statusCode: 404, calls: 1, delays: [] },
    { statusCode: 429, calls: 2, delays: [1000] },
    { statusCode: 503, calls: 2, delays: [1000] },
  ]) {
    const slept = [];
    let verifyCalls = 0;
    const typedError = Object.assign(new Error(`HTTP ${statusCode}`), { statusCode });
    const client = {
      async verify() {
        verifyCalls += 1;
        throw typedError;
      },
      async settle() {
        throw typedError;
      },
    };
    withFacilitatorRetries(client, { sleep: recordingSleep(slept) });
    await assert.rejects(() => client.verify({}, {}), error => error === typedError);
    assert.equal(verifyCalls, expectedCalls, `HTTP ${statusCode}`);
    assert.deepEqual(slept, delays, `HTTP ${statusCode}`);
  }
});

test("facilitator wrapper bounds getSupported for startup callers", async () => {
  const timers = manualTimers();
  const client = {
    async verify() {},
    async settle() {},
    async getSupported() {
      return new Promise(() => {});
    },
  };
  withFacilitatorRetries(client, timers);
  const supported = client.getSupported();
  assert.equal(timers.callbacks.length, 1);
  timers.callbacks[0]();
  await assert.rejects(supported, /supported-kinds request timed out/);
});

test("facilitator readiness requires the configured canonical kind", async () => {
  const probe = createFacilitatorProbe({
    network: "eip155:8453",
    facilitatorUrl: "https://facilitator.example",
    client: {
      async getSupported() {
        return {
          kinds: [{
            x402Version: 2,
            scheme: "exact",
            network: "eip155:8453",
          }],
        };
      },
    },
    fetchImpl: async url => {
      assert.equal(url, "https://facilitator.example/readyz");
      return Response.json({ ready: true });
    },
  });
  assert.deepEqual(
    await probe.check(),
    { network: "eip155:8453", ready: true },
  );
});

test("facilitator readiness fails closed on wrong identity or unavailable state", async () => {
  for (const { supported, response, pattern } of [
    {
      supported: { kinds: [{ x402Version: 2, scheme: "exact", network: "near:mainnet" }] },
      response: Response.json({ ready: true }),
      pattern: /does not advertise/,
    },
    {
      supported: { kinds: [{ x402Version: 2, scheme: "exact", network: "eip155:8453" }] },
      response: Response.json({ ready: false }, { status: 503 }),
      pattern: /HTTP 503/,
    },
  ]) {
    const probe = createFacilitatorProbe({
      network: "eip155:8453",
      facilitatorUrl: "https://facilitator.example",
      client: { getSupported: async () => supported },
      fetchImpl: async () => response,
    });
    await assert.rejects(() => probe.check(), pattern);
  }
});

test("facilitator readiness has no background retry after supported is rate-limited", async () => {
  let supportedCalls = 0;
  const fetchImpl = async url => {
    if (url.endsWith("/supported")) {
      supportedCalls += 1;
      return new Response("", { status: 429, headers: { "retry-after": "30" } });
    }
    assert.equal(url, "https://facilitator.example/readyz");
    return Response.json({ ready: true });
  };
  const probe = createFacilitatorProbe({
    network: "eip155:8453",
    facilitatorUrl: "https://facilitator.example",
    client: merchantFacilitator({
      fetchImpl,
      sleep: () => Promise.resolve(),
    }),
    fetchImpl,
  });

  await assert.rejects(
    () => probe.check(),
    error => error instanceof FacilitatorHttpError && error.statusCode === 429,
  );
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(supportedCalls, 1);
});

test("facilitator readiness bounds either hanging dependency deterministically", async () => {
  for (const dependency of ["supported", "readyz"]) {
    const timers = manualTimers();
    let readyzSignal;
    let supportedSignal;
    const probe = createFacilitatorProbe({
      network: "eip155:8453",
      facilitatorUrl: "https://facilitator.example",
      client: {
        async getSupported({ signal } = {}) {
          supportedSignal = signal;
          if (dependency === "supported") return new Promise(() => {});
          return {
            kinds: [{
              x402Version: 2,
              scheme: "exact",
              network: "eip155:8453",
            }],
          };
        },
      },
      fetchImpl: async (_url, request) => {
        readyzSignal = request.signal;
        if (dependency === "readyz") return new Promise(() => {});
        return Response.json({ ready: true });
      },
      ...timers,
    });
    const checking = probe.check();
    await Promise.resolve();
    assert.equal(timers.callbacks.length, 2);
    timers.callbacks[dependency === "supported" ? 0 : 1]();
    await assert.rejects(checking, /facilitator readiness timed out/);
    if (dependency === "supported") assert.equal(supportedSignal?.aborted, true);
    if (dependency === "readyz") assert.equal(readyzSignal?.aborted, true);
  }
});

test("facilitator readiness cancels supported discovery when /readyz fails", async () => {
  let supportedSignal;
  let supportedStarted;
  const started = new Promise(resolve => {
    supportedStarted = resolve;
  });
  const client = merchantFacilitator({
    fetchImpl: async (_url, request) => {
      supportedSignal = request.signal;
      supportedStarted();
      return new Promise((resolve, reject) => {
        request.signal.addEventListener("abort", () => {
          const error = new Error("request aborted");
          error.name = "AbortError";
          reject(error);
        }, { once: true });
      });
    },
  });
  const probe = createFacilitatorProbe({
    network: "eip155:8453",
    facilitatorUrl: "https://facilitator.example",
    client,
    fetchImpl: async () => {
      await started;
      return new Response("", { status: 503 });
    },
  });

  await assert.rejects(() => probe.check(), /facilitator readiness returned HTTP 503/);
  assert.equal(supportedSignal?.aborted, true);
});
