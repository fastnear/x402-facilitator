import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";

const ALLOWED_BROWSER_ORIGIN = "https://js.fastnear.com";
const MAX_PAYMENT_REQUIRED_BYTES = 12_000;

export const deployments = [
  {
    name: "NEAR",
    origin: "https://merchant-near.mikedotexe.com",
    network: "near:mainnet",
    asset: "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1",
    payTo: "count.mike.near",
    identifier: "mike.near",
    accountBody: { accountId: "mike.near" },
    transactionBody: {
      transactionHash: "5dm822stypkWdK7A5s2owV9QBPbh4uZhLPoWou2mw4zs",
      signerId: "mike.near",
    },
  },
  {
    name: "Base",
    origin: "https://merchant-base.mikedotexe.com",
    network: "eip155:8453",
    asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    payTo: "0x7Ff46ab88688D528bCE3e59c470240c6901cF88c",
    identifier: "0xcA202E03f11Aa076c57EAE666Da3f933dCc71CC9",
    accountBody: { address: "0xcA202E03f11Aa076c57EAE666Da3f933dCc71CC9" },
    transactionBody: {
      transactionHash: "0x5376373cceaae0bc078129c61163b3439f1377099ba034e6f4f895c4cb66f28d",
    },
  },
];

export function selectRegressionTargets(argumentsList = []) {
  if (argumentsList.length === 0) return deployments;
  if (
    argumentsList.length !== 2
    || argumentsList[0] !== "--target"
  ) {
    throw new Error("usage: npm run regression [-- --target near|base]");
  }

  switch (argumentsList[1]) {
    case "near":
      return [deployments[0]];
    case "base":
      return [deployments[1]];
    default:
      throw new Error("usage: npm run regression [-- --target near|base]");
  }
}

const routeDefinitions = [
  { method: "POST", path: "/v1/evidence/account", body: deployment => deployment.accountBody },
  { method: "POST", path: "/v1/evidence/transaction", body: deployment => deployment.transactionBody },
  { method: "POST", path: "/v1/activity/search", body: () => ({ query: "transfer", limit: 1 }) },
  {
    method: "POST",
    path: "/v1/routes/usdc/quote",
    body: () => ({
      amountAtomic: "1000000",
      recipient: "mike.near",
      refundTo: "0xcA202E03f11Aa076c57EAE666Da3f933dCc71CC9",
    }),
  },
  {
    method: "GET",
    path: "/v1/entities/{identifier}",
    requestPath: deployment => `/v1/entities/${deployment.identifier}`,
  },
];

export async function runRegressionCheck({
  fetchImpl = globalThis.fetch,
  targets = deployments,
  output = console.log,
} = {}) {
  assert.equal(typeof fetchImpl, "function", "fetch implementation is required");
  const results = [];
  for (const deployment of targets) {
    const openApiResponse = await checkedFetch(fetchImpl, `${deployment.origin}/openapi.json`);
    assert.equal(openApiResponse.status, 200, `${deployment.name} OpenAPI must return 200`);
    assert.match(openApiResponse.headers.get("content-type") ?? "", /application\/json/);
    assertSecurityHeaders(openApiResponse, deployment.name);
    const openApi = await openApiResponse.json();
    assert.equal(openApi.openapi, "3.1.0");
    assert.equal(openApi.servers?.[0]?.url, deployment.origin);

    for (const publicPath of ["/", "/llms.txt", "/.well-known/x402", "/healthz"]) {
      const response = await checkedFetch(fetchImpl, `${deployment.origin}${publicPath}`);
      assert.equal(response.status, 200, `${deployment.name} ${publicPath} must return 200`);
      assertSecurityHeaders(response, `${deployment.name} ${publicPath}`);
    }

    const readiness = await checkedFetch(fetchImpl, `${deployment.origin}/readyz`);
    assert.equal(readiness.status, 200, `${deployment.name} /readyz must return exactly 200`);
    assertSecurityHeaders(readiness, `${deployment.name} /readyz`);
    assert.deepEqual(await readiness.json(), {
      ready: true,
      checks: { rpc: "ready", facilitator: "ready", payment: "ready" },
    }, `${deployment.name} /readyz dependencies must be ready`);

    const unknown = await checkedFetch(fetchImpl, `${deployment.origin}/v1/not-a-route`);
    assert.equal(unknown.status, 404, `${deployment.name} unknown routes must return 404`);

    for (const route of routeDefinitions) {
      const requestPath = route.requestPath?.(deployment) ?? route.path;
      const body = route.body?.(deployment);
      const response = await checkedFetch(fetchImpl, `${deployment.origin}${requestPath}`, requestInit(route.method, body));
      const challenge = validateChallenge(response, deployment, route);
      validateBazaar(challenge, openApi, route);

      if (route.method === "POST") {
        const invalid = await checkedFetch(
          fetchImpl,
          `${deployment.origin}${requestPath}`,
          requestInit("POST", {}),
        );
        assert.equal(invalid.status, 402, `${deployment.name} ${route.path} must require payment before application validation`);
      }

      results.push({
        deployment: deployment.name,
        route: `${route.method} ${route.path}`,
        paymentRequiredBytes: response.headers.get("payment-required").length,
      });
    }

    await validateCors(fetchImpl, deployment);
    output(`ok ${deployment.name}: readiness, discovery, 5 unpaid challenges, schemas, and CORS`);
  }
  output(`ok ${results.length} paid routes; no payment signature was created or sent`);
  return results;
}

function requestInit(method, body) {
  if (method === "GET") return { redirect: "error" };
  return {
    method,
    redirect: "error",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  };
}

async function checkedFetch(fetchImpl, url, init = {}) {
  const headers = new Headers(init.headers);
  assert.equal(headers.has("payment-signature"), false);
  assert.equal(headers.has("x-payment"), false);
  return fetchImpl(url, { ...init, headers, redirect: init.redirect ?? "error" });
}

function validateChallenge(response, deployment, route) {
  assert.equal(response.status, 402, `${deployment.name} ${route.path} must return 402`);
  const encoded = response.headers.get("payment-required");
  assert.ok(encoded, `${deployment.name} ${route.path} omitted PAYMENT-REQUIRED`);
  assert.ok(
    encoded.length <= MAX_PAYMENT_REQUIRED_BYTES,
    `${deployment.name} ${route.path} PAYMENT-REQUIRED grew beyond the 12 KB safety threshold`,
  );
  const challenge = decodePaymentRequired(encoded);
  assert.equal(challenge.x402Version, 2);
  assert.equal(challenge.accepts?.length, 1);
  const requirement = challenge.accepts[0];
  assert.equal(requirement.scheme, "exact");
  assert.equal(requirement.network, deployment.network);
  assert.equal(normalize(requirement.asset), normalize(deployment.asset));
  assert.equal(requirement.amount, "1000");
  assert.equal(normalize(requirement.payTo), normalize(deployment.payTo));
  assert.equal(challenge.resource?.url, `${deployment.origin}${route.path}`);
  assert.equal(challenge.resource?.mimeType, "application/json");
  return challenge;
}

export function decodePaymentRequired(encoded) {
  try {
    return JSON.parse(Buffer.from(encoded, "base64").toString("utf8"));
  } catch {
    assert.fail("PAYMENT-REQUIRED is not canonical base64 JSON");
  }
}

function validateBazaar(challenge, openApi, route) {
  const bazaar = challenge.extensions?.bazaar;
  assert.ok(bazaar?.info?.input, `${route.path} omitted Bazaar input metadata`);
  assert.ok(bazaar?.info?.output, `${route.path} omitted Bazaar output metadata`);
  assert.ok(bazaar?.schema?.properties?.input, `${route.path} omitted Bazaar input schema`);
  assert.ok(bazaar?.schema?.properties?.output, `${route.path} omitted Bazaar output schema`);
  assert.equal(bazaar.info.input.method, route.method);

  const operation = openApi.paths?.[route.path]?.[route.method.toLowerCase()];
  assert.ok(operation, `${route.path} is missing from OpenAPI`);
  assert.ok(operation["x-payment-info"], `${route.path} omitted x-payment-info`);
  assert.equal(operation["x-payment-info"].price.amount, "0.001000");
  assert.ok(operation.responses?.["402"], `${route.path} omitted its OpenAPI 402 response`);

  if (route.method === "POST") {
    assert.deepEqual(
      bazaar.schema.properties.input.properties.body,
      operation.requestBody.content["application/json"].schema,
      `${route.path} runtime and OpenAPI input schemas diverged`,
    );
  } else {
    const runtimePath = bazaar.schema.properties.input.properties.pathParams;
    for (const parameter of operation.parameters ?? []) {
      assert.equal(parameter.in, "path");
      assert.deepEqual(runtimePath.properties[parameter.name], parameter.schema);
      assert.ok(runtimePath.required.includes(parameter.name));
    }
  }

  const openApiOutput = structuredClone(operation.responses["200"].content["application/json"].schema);
  delete openApiOutput.example;
  assert.deepEqual(
    bazaar.schema.properties.output.properties.example,
    openApiOutput,
    `${route.path} runtime and OpenAPI output schemas diverged`,
  );
}

async function validateCors(fetchImpl, deployment) {
  const url = `${deployment.origin}/v1/evidence/account`;
  const allowed = await checkedFetch(fetchImpl, url, {
    method: "OPTIONS",
    headers: {
      origin: ALLOWED_BROWSER_ORIGIN,
      "access-control-request-method": "POST",
      "access-control-request-headers": "content-type,payment-signature",
    },
  });
  assert.equal(allowed.status, 204);
  assert.equal(allowed.headers.get("access-control-allow-origin"), ALLOWED_BROWSER_ORIGIN);
  assert.match(allowed.headers.get("access-control-allow-headers") ?? "", /PAYMENT-SIGNATURE/i);

  const denied = await checkedFetch(fetchImpl, url, {
    method: "OPTIONS",
    headers: {
      origin: "https://unlisted.invalid",
      "access-control-request-method": "POST",
    },
  });
  assert.equal(denied.status, 403);
  assert.equal(denied.headers.get("access-control-allow-origin"), null);

  const actual = await checkedFetch(fetchImpl, url, {
    ...requestInit("POST", deployment.accountBody),
    headers: { "content-type": "application/json", origin: ALLOWED_BROWSER_ORIGIN },
  });
  assert.equal(actual.status, 402);
  assert.match(actual.headers.get("access-control-expose-headers") ?? "", /PAYMENT-REQUIRED/i);
}

function assertSecurityHeaders(response, label) {
  assert.equal(response.headers.get("x-content-type-options"), "nosniff", `${label} omitted nosniff`);
  assert.equal(response.headers.get("referrer-policy"), "no-referrer", `${label} omitted referrer policy`);
  assert.match(response.headers.get("cache-control") ?? "", /no-store/, `${label} must not be cached`);
  assert.match(response.headers.get("strict-transport-security") ?? "", /max-age=/, `${label} omitted HSTS`);
}

function normalize(value) {
  return typeof value === "string" && value.startsWith("0x") ? value.toLowerCase() : value;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  let targets;
  try {
    targets = selectRegressionTargets(process.argv.slice(2));
  } catch (error) {
    console.error(`regression check failed: ${error.message}`);
    process.exitCode = 1;
  }

  if (targets) runRegressionCheck({ targets }).catch(error => {
    console.error(`regression check failed: ${error.message}`);
    process.exitCode = 1;
  });
}
