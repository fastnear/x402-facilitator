import assert from "node:assert/strict";
import test from "node:test";

import {
  validateActivitySearchInput,
  validateEntityIdentifier,
} from "./activity-store.mjs";
import { createMerchantApplication, startMerchantServer } from "./app.mjs";
import { validateEvidenceInput } from "./evidence-input.mjs";
import { validateUsdcRouteInput } from "./usdc-route.mjs";

const config = {
  network: "eip155:8453",
  chainId: "8453",
  facilitatorUrl: "https://facilitator.example",
  apiKey: "test-key",
  rpcUrl: "https://rpc.example",
  resourceOrigin: "https://merchant.example",
  asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
  payTo: "0x1111111111111111111111111111111111111111",
  amount: "1000",
  priceUsd: "0.001000",
  port: 4031,
  eip712Name: "USD Coin",
  eip712Version: "2",
  corsOrigins: [],
  oneClickProviderOrigin: "https://1click.chaindefuser.com",
};

const activity = {
  search: () => ({
    items: [],
    nextCursor: null,
    index: { status: "not_yet_indexed", recordCount: 0, indexedAt: null },
  }),
  entity: identifier => ({
    identifier,
    status: "not_yet_indexed",
    records: [],
    index: { status: "not_yet_indexed", recordCount: 0, indexedAt: null },
  }),
};

function dependencies({
  rpcReady = true,
  facilitatorReady = true,
  network = config.network,
} = {}) {
  return {
    reader: {
      async checkIdentity() {
        if (!rpcReady) throw new Error("wrong chain");
      },
      async account(address) {
        return { kind: "account", address };
      },
      async transaction(hash) {
        return { kind: "transaction", hash };
      },
    },
    facilitator: {
      async getSupported() {
        return {
          kinds: [{
            x402Version: 2,
            scheme: "exact",
            network,
          }],
          extensions: [],
          signers: {},
        };
      },
      async verify() {
        return { isValid: false };
      },
      async settle() {
        return { success: false };
      },
    },
    facilitatorProbe: {
      async check() {
        if (!facilitatorReady) throw new Error("not ready");
      },
    },
  };
}

function configForNetwork(network) {
  if (network === "near:mainnet") {
    return {
      ...config,
      network,
      chainId: "mainnet",
      asset: "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1",
      payTo: "merchant.near",
      eip712Name: undefined,
      eip712Version: undefined,
    };
  }
  if (network === "near:testnet") {
    return {
      ...config,
      network,
      chainId: "testnet",
      asset: "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af",
      payTo: "merchant.testnet",
      eip712Name: undefined,
      eip712Version: undefined,
    };
  }
  if (network === "eip155:84532") {
    return {
      ...config,
      network,
      chainId: "84532",
      asset: "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
      payTo: "0x2222222222222222222222222222222222222222",
      eip712Name: "USDC",
      eip712Version: "2",
    };
  }
  return config;
}

async function serve(application) {
  const server = await new Promise((resolve, reject) => {
    const listener = application.app.listen(0, "127.0.0.1", () => resolve(listener));
    listener.once("error", reject);
  });
  const address = server.address();
  const origin = `http://127.0.0.1:${address.port}`;
  return {
    origin,
    close: () => new Promise((resolve, reject) =>
      server.close(error => error ? reject(error) : resolve())),
  };
}

test("health is liveness while readiness reflects both dependencies", async t => {
  const application = await createMerchantApplication({
    config,
    activity,
    ...dependencies({ facilitatorReady: false }),
  });
  const server = await serve(application);
  t.after(server.close);

  const health = await fetch(`${server.origin}/healthz`);
  assert.equal(health.status, 200);
  assert.equal((await health.json()).ok, true);

  const readiness = await fetch(`${server.origin}/readyz`);
  assert.equal(readiness.status, 503);
  assert.equal(readiness.headers.get("retry-after"), "1");
  assert.deepEqual(await readiness.json(), {
    ready: false,
    checks: { rpc: "ready", facilitator: "not_ready", payment: "ready" },
  });
});

test("readiness fails closed when payment initialization is delayed and then fails", async t => {
  let initializationCalls = 0;
  let rejectInitialization;
  let signalInitializationStarted;
  const delayedInitialization = new Promise((_resolve, reject) => {
    rejectInitialization = reject;
  });
  const initializationStarted = new Promise(resolve => {
    signalInitializationStarted = resolve;
  });
  let middlewareSyncOnStart;
  const application = await createMerchantApplication({
    config,
    activity,
    ...dependencies(),
    paymentServerInitializer: () => {
      initializationCalls += 1;
      signalInitializationStarted();
      return delayedInitialization;
    },
    paymentMiddlewareFactory: (...args) => {
      middlewareSyncOnStart = args[3];
      return (_request, _response, next) => next();
    },
  });
  const server = await serve(application);
  t.after(server.close);

  assert.equal(middlewareSyncOnStart, false);
  assert.equal(initializationCalls, 0);

  const readinessRequest = fetch(`${server.origin}/readyz`);
  await initializationStarted;
  assert.equal(initializationCalls, 1);

  rejectInitialization(new Error("facilitator capability sync failed"));
  const readiness = await readinessRequest;
  assert.equal(readiness.status, 503);
  assert.equal(readiness.headers.get("retry-after"), "1");
  assert.deepEqual(await readiness.json(), {
    ready: false,
    checks: { rpc: "ready", facilitator: "ready", payment: "not_ready" },
  });

  await assert.rejects(
    application.checkDependencies(),
    /merchant dependencies are not ready/,
  );
  assert.equal(initializationCalls, 2);
});

test("startup refuses to listen when payment initialization fails", async () => {
  let initializationCalls = 0;

  await assert.rejects(
    startMerchantServer({
      config: { ...config, port: 0 },
      activity,
      ...dependencies(),
      paymentServerInitializer: async () => {
        initializationCalls += 1;
        throw new Error("facilitator capability sync failed");
      },
      paymentMiddlewareFactory: () => (_request, _response, next) => next(),
    }),
    /merchant dependencies are not ready/,
  );
  assert.equal(initializationCalls, 1);
});

test("discovery derives its price from the exact atomic amount", async t => {
  const application = await createMerchantApplication({
    config: { ...config, amount: "1234567", priceUsd: "999.999999" },
    activity,
    ...dependencies(),
  });
  await application.checkDependencies();
  const server = await serve(application);
  t.after(server.close);

  const landing = await (await fetch(`${server.origin}/`)).text();
  assert.match(landing, /\$1\.234567/);
  const openApi = await (await fetch(`${server.origin}/openapi.json`)).json();
  assert.equal(
    openApi.paths["/v1/evidence/account"].post["x-payment-info"].price.amount,
    "1.234567",
  );

  const challengeResponse = await fetch(
    `${server.origin}/v1/evidence/account`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        address: "0x2222222222222222222222222222222222222222",
      }),
    },
  );
  assert.equal(challengeResponse.status, 402);
  const challenge = JSON.parse(
    Buffer.from(
      challengeResponse.headers.get("payment-required"),
      "base64",
    ).toString("utf8"),
  );
  assert.equal(challenge.accepts[0].amount, "1234567");
  assert.deepEqual(challenge.accepts[0].extra, {
    name: "USD Coin",
    version: "2",
  });
});

test("application handlers are independently testable after payment authorization", async t => {
  const application = await createMerchantApplication({
    config,
    activity,
    ...dependencies(),
    paymentMiddlewareFactory: () => (_request, _response, next) => next(),
  });
  const server = await serve(application);
  t.after(server.close);

  const readiness = await fetch(`${server.origin}/readyz`);
  assert.equal(readiness.status, 200);
  assert.deepEqual(await readiness.json(), {
    ready: true,
    checks: { rpc: "ready", facilitator: "ready", payment: "ready" },
  });

  const response = await fetch(`${server.origin}/v1/evidence/account`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      address: "0x2222222222222222222222222222222222222222",
    }),
  });
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), {
    kind: "account",
    address: "0x2222222222222222222222222222222222222222",
  });
});

test("every advertised discovery input passes its shared pre-RPC validation", async t => {
  for (const network of [
    "near:mainnet",
    "near:testnet",
    "eip155:8453",
    "eip155:84532",
  ]) {
    const configured = configForNetwork(network);
    const application = await createMerchantApplication({
      config: configured,
      activity,
      ...dependencies({ network }),
    });
    const server = await serve(application);
    t.after(server.close);
    const openApi = await (await fetch(`${server.origin}/openapi.json`)).json();

    for (const [route, definition] of Object.entries(application.routes)) {
      const discovery = definition.extensions.bazaar.info.input;
      const [method, path] = route.split(" ");
      const openApiPath = path.replace(":identifier", "{identifier}");
      const operation = openApi.paths[openApiPath]?.[method.toLowerCase()];
      assert.ok(operation, `${network} ${route} is absent from OpenAPI`);

      if (route === "POST /v1/evidence/account") {
        assert.doesNotThrow(() =>
          validateEvidenceInput(network, "account", discovery.body));
      } else if (route === "POST /v1/evidence/transaction") {
        assert.doesNotThrow(() =>
          validateEvidenceInput(network, "transaction", discovery.body));
      } else if (route === "POST /v1/activity/search") {
        assert.doesNotThrow(() => validateActivitySearchInput(discovery.body));
      } else if (route === "POST /v1/routes/usdc/quote") {
        assert.doesNotThrow(() => validateUsdcRouteInput(discovery.body));
      } else if (route === "GET /v1/entities/:identifier") {
        assert.doesNotThrow(() =>
          validateEntityIdentifier(discovery.pathParams.identifier));
        assert.equal(
          operation.parameters?.[0]?.example,
          discovery.pathParams.identifier,
        );
        continue;
      } else {
        assert.fail(`missing validation coverage for ${route}`);
      }

      assert.deepEqual(
        operation.requestBody.content["application/json"].example,
        discovery.body,
        `${network} ${route} OpenAPI and Bazaar examples diverged`,
      );
    }
  }
});

test("strict discovery input validation rejects malformed NEAR evidence before RPC", async t => {
  const near = configForNetwork("near:testnet");
  const application = await createMerchantApplication({
    config: near,
    activity,
    ...dependencies({ network: near.network }),
    reader: {
      async checkIdentity() {},
      async account() {
        assert.fail("malformed account id must not reach RPC reader");
      },
      async transaction() {
        assert.fail("malformed transaction must not reach RPC reader");
      },
    },
    paymentMiddlewareFactory: () => (_request, _response, next) => next(),
  });
  const server = await serve(application);
  t.after(server.close);

  const account = await fetch(`${server.origin}/v1/evidence/account`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ accountId: "wrong..testnet" }),
  });
  assert.equal(account.status, 400);

  const transaction = await fetch(`${server.origin}/v1/evidence/transaction`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      transactionHash: `0x${"11".repeat(32)}`,
      signerId: "alice.testnet",
    }),
  });
  assert.equal(transaction.status, 400);
});
