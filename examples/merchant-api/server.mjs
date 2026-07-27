import { readFileSync } from "node:fs";

import express from "express";
import { HTTPFacilitatorClient } from "@x402/core/server";
import { paymentMiddlewareFromHTTPServer, x402HTTPResourceServer, x402ResourceServer } from "@x402/express";
import { ExactEvmScheme } from "@x402/evm/exact/server";
import { declareDiscoveryExtension } from "@x402/extensions/bazaar";
import { ExactNearScheme } from "@x402/near/exact/server";

import { ActivityStore } from "./activity-store.mjs";
import { ChainEvidenceError, createChainReader } from "./chain-reader.mjs";
import { createCorsMiddleware, parseAllowedOrigins } from "./cors.mjs";
import {
  BASE_USDC,
  NEAR_USDC,
  UsdcRouteQuoteError,
  createUsdcRouteQuoter,
} from "./usdc-route.mjs";

const config = loadConfig();
const reader = createChainReader(config);
const activity = await ActivityStore.fromFile(process.env.ACTIVITY_INDEX_FILE);
const routeQuoter = createUsdcRouteQuoter({
  providerOrigin: config.oneClickProviderOrigin,
  providerJwt: config.oneClickJwt,
});
const facilitator = new HTTPFacilitatorClient({
  url: config.facilitatorUrl,
  createAuthHeaders: async () => ({
    supported: {},
    verify: { "X-API-Key": config.apiKey },
    settle: { "X-API-Key": config.apiKey },
  }),
});

const exactScheme = config.network.startsWith("near:") ? new ExactNearScheme() : new ExactEvmScheme();
const resourceServer = new x402ResourceServer(facilitator).register(config.network, exactScheme);
const price = process.env.AMOUNT ?? "1000";
const resourceOrigin = config.resourceOrigin;
const favicon = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=", "base64");
const usdcRouteInputSchema = {
  type: "object",
  additionalProperties: false,
  required: ["amountAtomic", "recipient", "refundTo"],
  properties: {
    amountAtomic: { type: "string", pattern: "^[1-9][0-9]{0,15}$", description: "Base USDC amount in 6-decimal atomic units" },
    recipient: { type: "string", minLength: 2, maxLength: 64, pattern: "^[a-z0-9]+(?:[._-][a-z0-9]+)*$", description: "Destination NEAR account id" },
    refundTo: { type: "string", pattern: "^0x[0-9a-fA-F]{40}$", description: "Base address for a failed-route refund" },
    slippageBasisPoints: { type: "integer", minimum: 0, maximum: 1000, default: 100 },
  },
};
const routeAssetSchema = {
  type: "object",
  additionalProperties: false,
  required: ["network", "asset", "assetId", "symbol", "decimals"],
  properties: {
    network: { type: "string" },
    asset: { type: "string" },
    assetId: { type: "string" },
    symbol: { type: "string", const: "USDC" },
    decimals: { type: "integer", const: 6 },
  },
};
const usdcRouteOutputSchema = {
  type: "object",
  additionalProperties: false,
  required: [
    "kind",
    "mode",
    "fundsMoved",
    "source",
    "destination",
    "amountInAtomic",
    "amountOutAtomic",
    "minAmountOutAtomic",
    "recipient",
    "refundTo",
    "slippageBasisPoints",
    "estimatedSettlementSeconds",
    "providerFees",
    "quote",
    "provider",
  ],
  properties: {
    kind: { type: "string", const: "usdc_route_quote" },
    mode: { type: "string", const: "quote_only" },
    fundsMoved: { type: "boolean", const: false },
    source: routeAssetSchema,
    destination: routeAssetSchema,
    amountInAtomic: { type: "string", pattern: "^[0-9]+$" },
    amountOutAtomic: { type: "string", pattern: "^[0-9]+$" },
    minAmountOutAtomic: { type: "string", pattern: "^[0-9]+$" },
    recipient: { type: "string" },
    refundTo: { type: "string" },
    slippageBasisPoints: { type: "integer" },
    estimatedSettlementSeconds: { type: "integer", minimum: 0 },
    providerFees: {
      type: "object",
      additionalProperties: false,
      required: ["refundFeeAtomic", "withdrawFeeAtomic"],
      properties: {
        refundFeeAtomic: { type: "string", pattern: "^[0-9]+$" },
        withdrawFeeAtomic: { type: "string", pattern: "^[0-9]+$" },
      },
    },
    quote: {
      type: "object",
      additionalProperties: false,
      required: ["quotedAt", "expiresAt", "correlationId", "signature"],
      properties: {
        quotedAt: { type: "string", pattern: "^\\d{4}-\\d{2}-\\d{2}T" },
        expiresAt: { type: "string", pattern: "^\\d{4}-\\d{2}-\\d{2}T" },
        correlationId: { type: "string" },
        signature: { type: "string" },
      },
    },
    provider: {
      type: "object",
      additionalProperties: false,
      required: ["name", "endpoint", "status"],
      properties: {
        name: { type: "string", const: "NEAR Intents 1Click" },
        endpoint: { type: "string", pattern: "^https://" },
        status: { type: "string", const: "live" },
      },
    },
  },
};
const usdcRouteOutputExample = {
  kind: "usdc_route_quote",
  mode: "quote_only",
  fundsMoved: false,
  source: BASE_USDC,
  destination: NEAR_USDC,
  amountInAtomic: "1000000",
  amountOutAtomic: "998898",
  minAmountOutAtomic: "988909",
  recipient: "mike.near",
  refundTo: "0xcA202E03f11Aa076c57EAE666Da3f933dCc71CC9",
  slippageBasisPoints: 100,
  estimatedSettlementSeconds: 35,
  providerFees: { refundFeeAtomic: "2400", withdrawFeeAtomic: "0" },
  quote: {
    quotedAt: "2026-07-27T20:00:00.250Z",
    expiresAt: "2026-07-27T20:05:00.000Z",
    correlationId: "quote-123",
    signature: "provider-signature",
  },
  provider: {
    name: "NEAR Intents 1Click",
    endpoint: "https://1click.chaindefuser.com/v0/quote",
    status: "live",
  },
};
const routes = {
  "POST /v1/evidence/account": paidRoute({
    path: "/v1/evidence/account",
    description: "Final account evidence from the configured chain",
    input: config.network.startsWith("near:")
      ? { accountId: "alice.testnet" }
      : { address: "0x0000000000000000000000000000000000000000" },
    inputSchema: config.network.startsWith("near:")
      ? { type: "object", additionalProperties: false, required: ["accountId"], properties: { accountId: { type: "string" } } }
      : { type: "object", additionalProperties: false, required: ["address"], properties: { address: { type: "string", pattern: "^0x[0-9a-fA-F]{40}$" } } },
    outputExample: { network: config.network, kind: "account", observedFinality: "finalized", account: {}, source: { status: "final" } },
    outputSchema: { type: "object", required: ["network", "kind", "observedFinality", "account", "source"] },
  }),
  "POST /v1/evidence/transaction": paidRoute({
    path: "/v1/evidence/transaction",
    description: "Final or nonterminal transaction evidence from the configured chain",
    input: config.network.startsWith("near:")
      ? { transactionHash: "0x0000000000000000000000000000000000000000000000000000000000000000", signerId: "alice.testnet" }
      : { transactionHash: "0x0000000000000000000000000000000000000000000000000000000000000000" },
    inputSchema: config.network.startsWith("near:")
      ? { type: "object", additionalProperties: false, required: ["transactionHash", "signerId"], properties: { transactionHash: { type: "string" }, signerId: { type: "string" } } }
      : { type: "object", additionalProperties: false, required: ["transactionHash"], properties: { transactionHash: { type: "string", pattern: "^0x[0-9a-fA-F]{64}$" } } },
    outputExample: { network: config.network, kind: "transaction", observedFinality: "finalized", transaction: {}, source: { status: "final" } },
    outputSchema: { type: "object", required: ["network", "kind", "observedFinality", "transaction", "source"] },
  }),
  "POST /v1/activity/search": paidRoute({
    path: "/v1/activity/search",
    description: "Search the bounded final activity index",
    input: { query: "transfer", limit: 25 },
    inputSchema: { type: "object", additionalProperties: false, properties: { query: { type: "string" }, account: { type: "string" }, contract: { type: "string" }, limit: { type: "integer", minimum: 1, maximum: 100 }, cursor: { type: "string" } } },
    outputExample: { items: [], nextCursor: null, index: { status: "ready" } },
    outputSchema: { type: "object", required: ["items", "nextCursor", "index"] },
  }),
  "POST /v1/routes/usdc/quote": paidRoute({
    path: "/v1/routes/usdc/quote",
    description: "Quote canonical Base USDC to canonical NEAR USDC without moving funds",
    input: {
      amountAtomic: "1000000",
      recipient: "mike.near",
      refundTo: "0xcA202E03f11Aa076c57EAE666Da3f933dCc71CC9",
      slippageBasisPoints: 100,
    },
    inputSchema: usdcRouteInputSchema,
    outputExample: usdcRouteOutputExample,
    outputSchema: usdcRouteOutputSchema,
  }),
  "GET /v1/entities/:identifier": paidRoute({
    path: "/v1/entities/{identifier}",
    description: "Inspect indexed activity for one account, contract, or entity",
    input: {},
    inputSchema: { type: "object", additionalProperties: false, properties: {} },
    outputExample: { identifier: "alice.testnet", status: "not_yet_indexed", records: [], index: { status: "not_yet_indexed" } },
    outputSchema: { type: "object", required: ["identifier", "status", "records", "index"] },
    method: "GET",
    bodyType: undefined,
    pathParams: { identifier: "alice.testnet" },
    pathParamsSchema: { type: "object", required: ["identifier"], properties: { identifier: { type: "string" } } },
  }),
};

function paidRoute({ description, input, inputSchema, outputExample, outputSchema, method = "POST", bodyType, path, pathParams, pathParamsSchema }) {
  const effectiveBodyType = bodyType ?? (method === "POST" ? "json" : undefined);
  return {
    accepts: [{ scheme: "exact", price: { asset: config.asset, amount: price }, network: config.network, payTo: config.payTo, ...(config.eip712Name ? { extra: { name: config.eip712Name, version: config.eip712Version } } : {}) }],
    description,
    mimeType: "application/json",
    resource: `${resourceOrigin}${path}`,
    extensions: {
      ...declareDiscoveryExtension({ method, ...(effectiveBodyType ? { bodyType: effectiveBodyType } : {}), input, inputSchema, pathParams, pathParamsSchema, output: { example: outputExample, schema: outputSchema } }),
    },
  };
}

const httpServer = new x402HTTPResourceServer(resourceServer, routes);
const app = express();
app.disable("x-powered-by");
app.use(createCorsMiddleware(config.corsOrigins));
app.use(express.json({ limit: "16kb", strict: true }));

app.get("/", (_request, response) => response.type("html").send(landing(config)));
app.get("/openapi.json", (_request, response) => response.json(openApi(config)));
app.get("/llms.txt", (_request, response) => response.type("text/plain").send(llms(config)));
app.get("/favicon.ico", (_request, response) => response.type("image/png").send(favicon));
app.get("/.well-known/x402", (_request, response) => response.json({ version: 1, resources: Object.keys(routes).map(route => `${resourceOrigin}${route.split(" ")[1].replace(":identifier", "{identifier}")}`) }));
app.get("/healthz", (_request, response) => response.json({ ok: true, network: config.network, activityIndex: activity.search({}).index }));

app.use(paymentMiddlewareFromHTTPServer(httpServer));

app.post("/v1/evidence/account", async (request, response, next) => {
  try {
    const body = exactBody(request.body, config.network.startsWith("near:") ? ["accountId"] : ["address"]);
    const result = config.network.startsWith("near:") ? await reader.account(body.accountId) : await reader.account(body.address);
    response.json(result);
  } catch (error) { next(error); }
});
app.post("/v1/evidence/transaction", async (request, response, next) => {
  try {
    const body = exactBody(request.body, config.network.startsWith("near:") ? ["transactionHash", "signerId"] : ["transactionHash"]);
    const result = config.network.startsWith("near:")
      ? await reader.transaction(body.transactionHash, body.signerId)
      : await reader.transaction(body.transactionHash);
    response.json(result);
  } catch (error) { next(error); }
});
app.post("/v1/activity/search", (request, response, next) => {
  try { response.json(activity.search(request.body)); } catch (error) { next(error); }
});
app.post("/v1/routes/usdc/quote", async (request, response, next) => {
  try {
    response.json(await routeQuoter.quote(request.body));
  } catch (error) { next(error); }
});
app.get("/v1/entities/:identifier", (request, response) => response.json(activity.entity(request.params.identifier)));

app.use((error, _request, response, _next) => {
  if (error instanceof ChainEvidenceError) return response.status(error.status).json({ error: error.code, message: error.message });
  if (error instanceof UsdcRouteQuoteError) return response.status(error.status).json({ error: error.code, message: error.message });
  if (error?.status === 400) return response.status(400).json({ error: "invalid_input", message: error.message });
  console.error(error);
  return response.status(503).json({ error: "unavailable", message: "The merchant API could not produce authoritative evidence" });
});

app.listen(config.port, "127.0.0.1", () => console.log(`x402 merchant API listening on http://127.0.0.1:${config.port}`));

function loadConfig() {
  const network = required("NETWORK");
  const facilitatorUrl = required("FACILITATOR_URL");
  const apiKeyFile = required("FACILITATOR_API_KEY_FILE");
  const rpcUrl = required("RPC_URL");
  const resourceOrigin = required("RESOURCE_ORIGIN").replace(/\/$/, "");
  const apiKey = readCredential(apiKeyFile);
  const asset = required("ASSET");
  const payTo = required("PAY_TO");
  const eip712Name = process.env.ASSET_EIP712_NAME;
  if (network.startsWith("eip155:") && !eip712Name) throw new Error("ASSET_EIP712_NAME is required for EVM networks");
  const oneClickProviderOrigin = process.env.ONE_CLICK_PROVIDER_ORIGIN ?? "https://1click.chaindefuser.com";
  if (!oneClickProviderOrigin.startsWith("https://")) throw new Error("ONE_CLICK_PROVIDER_ORIGIN must use https");
  const oneClickJwt = process.env.ONE_CLICK_JWT_FILE
    ? readCredential(process.env.ONE_CLICK_JWT_FILE)
    : undefined;
  return { network, facilitatorUrl, apiKey, rpcUrl, resourceOrigin, asset, payTo, port: Number(process.env.PORT ?? 4031), explorerBaseUrl: process.env.EXPLORER_BASE_URL, eip712Name, eip712Version: process.env.ASSET_EIP712_VERSION ?? "2", corsOrigins: parseAllowedOrigins(process.env.CORS_ORIGINS), oneClickProviderOrigin, oneClickJwt };
}

function required(name) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function readCredential(path) {
  const value = readFileSync(path, "utf8").trim();
  if (!value || /\r|\n/.test(value)) throw new Error("facilitator credential file must contain one key");
  return value;
}

function exactBody(value, required) {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw invalidInput("request body must be an object");
  const allowed = new Set(required);
  for (const key of Object.keys(value)) if (!allowed.has(key)) throw invalidInput(`unexpected request field: ${key}`);
  for (const key of required) if (!(key in value)) throw invalidInput(`${key} is required`);
  return value;
}

function invalidInput(message) {
  const error = new Error(message);
  error.status = 400;
  return error;
}

function openApi(config) {
  const near = config.network.startsWith("near:");
  const accountInput = near ? { accountId: { type: "string" } } : { address: { type: "string", pattern: "^0x[0-9a-fA-F]{40}$" } };
  const transactionInput = near ? { transactionHash: { type: "string" }, signerId: { type: "string" } } : { transactionHash: { type: "string", pattern: "^0x[0-9a-fA-F]{64}$" } };
  const evidenceBase = {
    type: "object",
    additionalProperties: false,
    required: ["network", "kind", "observedFinality", "observedAt", "block", "source"],
    properties: {
      network: { type: "string" },
      kind: { type: "string" },
      observedFinality: { type: "string", enum: ["final", "finalized", "nonterminal"] },
      observedAt: { type: "string", format: "date-time" },
      block: { type: "object" },
      source: { type: "object" },
      explorerUrl: { type: "string", format: "uri" },
    },
  };
  const accountOutput = { ...evidenceBase, required: [...evidenceBase.required, "account"], properties: { ...evidenceBase.properties, account: { type: "object" } } };
  const transactionOutput = { ...evidenceBase, required: [...evidenceBase.required, "transaction"], properties: { ...evidenceBase.properties, transaction: { type: "object" } } };
  const activityOutput = {
    type: "object",
    additionalProperties: false,
    required: ["items", "nextCursor", "index"],
    properties: {
      items: { type: "array", items: { type: "object" } },
      nextCursor: { type: ["string", "null"] },
      index: { type: "object" },
    },
  };
  const entityOutput = {
    type: "object",
    additionalProperties: false,
    required: ["identifier", "status", "records", "index"],
    properties: {
      identifier: { type: "string" },
      status: { type: "string", enum: ["indexed", "not_yet_indexed"] },
      records: { type: "array", items: { type: "object" } },
      index: { type: "object" },
    },
  };
  return {
    openapi: "3.1.0",
    info: {
      title: `${near ? "NEAR" : "Base"} Agent Evidence & Route API`,
      version: "0.2.0",
      contact: { email: process.env.CONTACT_EMAIL ?? "support@example.invalid" },
      description: "Paid, machine-readable chain evidence and bounded activity intelligence.",
      "x-guidance": `Use POST /v1/evidence/account for ${near ? "a NEAR account id" : "an EVM address"}, POST /v1/evidence/transaction for ${near ? "a transaction hash plus signer account" : "an EVM transaction hash"}, or POST /v1/routes/usdc/quote for a dry Base-USDC-to-NEAR-USDC route quote. An unpaid request returns HTTP 402 with canonical x402 v2 requirements.`,
    },
    servers: [{ url: config.resourceOrigin }],
    paths: {
      "/v1/evidence/account": operation("Inspect final account evidence", { type: "object", additionalProperties: false, required: Object.keys(accountInput), properties: accountInput }, { network: config.network, kind: "account", observedFinality: "finalized", account: {}, source: {} }, accountOutput),
      "/v1/evidence/transaction": operation("Inspect transaction evidence", { type: "object", additionalProperties: false, required: Object.keys(transactionInput), properties: transactionInput }, { network: config.network, kind: "transaction", observedFinality: "finalized", transaction: {}, source: {} }, transactionOutput),
      "/v1/activity/search": operation("Search the bounded final activity index", { type: "object", additionalProperties: false, properties: { query: { type: "string" }, account: { type: "string" }, contract: { type: "string" }, limit: { type: "integer", minimum: 1, maximum: 100 }, cursor: { type: "string" } } }, { items: [], nextCursor: null, index: {} }, activityOutput),
      "/v1/routes/usdc/quote": operation("Quote Base USDC to NEAR USDC", usdcRouteInputSchema, usdcRouteOutputExample, usdcRouteOutputSchema),
      "/v1/entities/{identifier}": { get: { ...operation("Inspect an indexed entity", { type: "object", additionalProperties: false, properties: {} }, { identifier: "string", status: "string", records: [], index: {} }, entityOutput).get, parameters: [{ name: "identifier", in: "path", required: true, schema: { type: "string" } }] } },
    },
  };

  function operation(summary, schema, example, responseSchema) {
    const isGet = summary.startsWith("Inspect an indexed entity");
    return { [isGet ? "get" : "post"]: { summary, operationId: summary.toLowerCase().replace(/[^a-z]+/g, "_"), ...(isGet ? {} : { requestBody: { required: true, content: { "application/json": { schema } } } }), "x-payment-info": { price: { mode: "fixed", currency: "USD", amount: "0.001000" }, protocols: [{ x402: {} }] }, responses: { "200": { description: "Successful response", content: { "application/json": { schema: { ...responseSchema, example } } } }, "402": { description: "Payment Required" } } } };
  }
}

function llms(config) {
  return `# ${config.network} Agent Evidence & Route API\n\nPaid x402 API for authoritative chain evidence and route intelligence.\n\n- POST /v1/evidence/account — inspect a finalized account.\n- POST /v1/evidence/transaction — inspect final or nonterminal transaction evidence.\n- POST /v1/activity/search — search the bounded final activity index.\n- GET /v1/entities/{identifier} — inspect indexed activity for an entity.\n- POST /v1/routes/usdc/quote — request a signed, quote-only route from canonical Base USDC to canonical NEAR USDC. This dry route never returns a deposit address and never moves funds.\n\nCall a route without payment first. Read the PAYMENT-REQUIRED header, sign the exact requirement, and retry with PAYMENT-SIGNATURE. Prices are decimal USD in OpenAPI and atomic USDC units at runtime.\n`;
}

function landing(config) {
  const chain = config.network.startsWith("near:") ? "NEAR" : "Base";
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>${chain} Agent Evidence &amp; Route API</title>
  <meta name="description" content="Paid x402 chain evidence for ${config.network}.">
  <style>
    :root { color-scheme: dark; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
    body { max-width: 52rem; margin: 0 auto; padding: 4rem 1.5rem; background: #0b0d10; color: #e8edf2; line-height: 1.6; }
    h1 { line-height: 1.15; }
    a { color: #7dd3fc; }
    code { color: #a7f3d0; }
    .card { border: 1px solid #2d3640; border-radius: 12px; padding: 1.25rem; margin: 1.5rem 0; background: #11161c; }
  </style>
</head>
<body>
  <p>mikedotexe.com / x402</p>
  <h1>${chain} Agent Evidence &amp; Route API</h1>
  <p>Machine-readable chain evidence and route intelligence on <code>${config.network}</code>, priced at <code>$0.001</code> per request and settled through x402.</p>
  <div class="card">
    <p><strong>Agent discovery</strong></p>
    <ul>
      <li><a href="/openapi.json">OpenAPI contract</a></li>
      <li><a href="/llms.txt">llms.txt guidance</a></li>
      <li><a href="/.well-known/x402">x402 discovery</a></li>
    </ul>
  </div>
  <p>Start with <code>POST /v1/evidence/account</code>, <code>POST /v1/evidence/transaction</code>, or the quote-only <code>POST /v1/routes/usdc/quote</code>. An unpaid valid request returns canonical x402 v2 payment requirements.</p>
</body>
</html>`;
}
