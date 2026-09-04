import express from "express";
import { paymentMiddlewareFromHTTPServer, x402HTTPResourceServer, x402ResourceServer } from "@x402/express";
import { ExactEvmScheme } from "@x402/evm/exact/server";
import { declareDiscoveryExtension } from "@x402/extensions/bazaar";
import { ExactNearScheme } from "@x402/near/exact/server";

import {
  ACTIVITY_SEARCH_INPUT_SCHEMA,
  ENTITY_IDENTIFIER_SCHEMA,
  ActivityStore,
} from "./activity-store.mjs";
import { ChainEvidenceError, createChainReader } from "./chain-reader.mjs";
import { createCorsMiddleware } from "./cors.mjs";
import {
  NEAR_TRANSACTION_HASH_EXAMPLE,
  PATTERNS,
  evidenceInputSchema,
  nearExampleAccountId,
  validateEvidenceInput,
} from "./evidence-input.mjs";
import {
  BASE_USDC,
  NEAR_USDC,
  USDC_ROUTE_INPUT_SCHEMA,
  UsdcRouteQuoteError,
  createUsdcRouteQuoter,
} from "./usdc-route.mjs";
import { formatUsdc, loadConfig } from "./config.mjs";
import {
  createFacilitatorProbe,
  MerchantFacilitatorClient,
  withFacilitatorRetries,
} from "./facilitator.mjs";
import { createReadinessCache } from "./readiness-cache.mjs";

const favicon = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=", "base64");
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
        signature: {
          type: "string",
          description: "Provider-supplied provenance preserved verbatim; this service does not claim cryptographic verification",
        },
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
function createRoutes(config) {
  const apiSchemas = createApiSchemas(config);
  const resourceOrigin = config.resourceOrigin;
  const price = config.amount;
  const near = config.network.startsWith("near:");
  const nearExampleAccount = near ? nearExampleAccountId(config.network) : undefined;
  const entityExample = near
    ? nearExampleAccount
    : "0x0000000000000000000000000000000000000000";
  return {
  "POST /v1/evidence/account": paidRoute({
    path: "/v1/evidence/account",
    description: "Final account evidence from the configured chain",
    input: near
      ? { accountId: nearExampleAccount }
      : { address: "0x0000000000000000000000000000000000000000" },
    inputSchema: evidenceInputSchema(config.network, "account"),
    outputExample: {
      network: config.network,
      kind: "account",
      observedFinality: near ? "final" : "finalized",
      observedAt: "2026-07-27T20:00:00.000Z",
      block: { height: near ? 123456789 : "34567890", hash: near ? NEAR_TRANSACTION_HASH_EXAMPLE : `0x${"11".repeat(32)}` },
      account: near
        ? { accountId: nearExampleAccount, amountYoctoNear: "1000000000000000000000000", lockedYoctoNear: "0", storageUsage: 100, codeHash: NEAR_TRANSACTION_HASH_EXAMPLE }
        : { address: "0x0000000000000000000000000000000000000000", balanceWei: "0", isContract: false, asset: config.asset },
      source: { type: near ? "near-jsonrpc" : "evm-jsonrpc", status: near ? "final" : "finalized" },
    },
    outputSchema: apiSchemas.accountOutput,
  }),
  "POST /v1/evidence/transaction": paidRoute({
    path: "/v1/evidence/transaction",
    description: "Final or nonterminal transaction evidence from the configured chain",
    input: near
      ? { transactionHash: NEAR_TRANSACTION_HASH_EXAMPLE, signerId: nearExampleAccount }
      : { transactionHash: "0x0000000000000000000000000000000000000000000000000000000000000000" },
    inputSchema: evidenceInputSchema(config.network, "transaction"),
    outputExample: {
      network: config.network,
      kind: "transaction",
      observedFinality: near ? "final" : "finalized",
      observedAt: "2026-07-27T20:00:00.000Z",
      block: { height: near ? 123456789 : "34567890", hash: near ? NEAR_TRANSACTION_HASH_EXAMPLE : `0x${"11".repeat(32)}` },
      transaction: near
        ? { hash: NEAR_TRANSACTION_HASH_EXAMPLE, signerId: nearExampleAccount, receiverId: config.network === "near:mainnet" ? "token.near" : "token.testnet", blockHash: NEAR_TRANSACTION_HASH_EXAMPLE, success: true, status: "succeeded", receiptCount: 1, failures: [] }
        : { hash: `0x${"22".repeat(32)}`, from: "0x0000000000000000000000000000000000000000", to: config.asset, blockNumber: "34567889", confirmationDepth: "1", success: true, status: "succeeded", gasUsed: "65000" },
      explorerUrl: near ? "https://nearblocks.io/txns/example" : "https://basescan.org/tx/example",
      source: { type: near ? "near-jsonrpc" : "evm-jsonrpc", status: near ? "final" : "finalized" },
    },
    outputSchema: apiSchemas.transactionOutput,
  }),
  "POST /v1/activity/search": paidRoute({
    path: "/v1/activity/search",
    description: "Search the bounded final activity index",
    input: { query: "transfer", limit: 25 },
    inputSchema: ACTIVITY_SEARCH_INPUT_SCHEMA,
    outputExample: { items: [], nextCursor: null, index: { status: "not_yet_indexed", recordCount: 0, indexedAt: null } },
    outputSchema: apiSchemas.activityOutput,
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
    inputSchema: USDC_ROUTE_INPUT_SCHEMA,
    outputExample: usdcRouteOutputExample,
    outputSchema: usdcRouteOutputSchema,
  }),
  "GET /v1/entities/:identifier": paidRoute({
    path: "/v1/entities/{identifier}",
    description: "Inspect indexed activity for one account, contract, or entity",
    input: {},
    inputSchema: { type: "object", additionalProperties: false, properties: {} },
    outputExample: { identifier: entityExample, status: "not_yet_indexed", records: [], index: { status: "not_yet_indexed", recordCount: 0, indexedAt: null } },
    outputSchema: apiSchemas.entityOutput,
    method: "GET",
    bodyType: undefined,
    pathParams: { identifier: entityExample },
    pathParamsSchema: { type: "object", additionalProperties: false, required: ["identifier"], properties: { identifier: ENTITY_IDENTIFIER_SCHEMA } },
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
}

export async function createMerchantApplication({
  config = loadConfig(),
  reader = createChainReader(config),
  activity,
  routeQuoter,
  facilitator,
  facilitatorProbe,
  paymentServerInitializer,
  paymentMiddlewareFactory = paymentMiddlewareFromHTTPServer,
  fetchImpl = globalThis.fetch,
  logger = console,
  readinessCacheMs = 1_000,
  now = () => Date.now(),
} = {}) {
  const activityStore = activity
    ?? await ActivityStore.fromFile(config.activityIndexFile);
  const quoter = routeQuoter ?? createUsdcRouteQuoter({
    providerOrigin: config.oneClickProviderOrigin,
    providerJwt: config.oneClickJwt,
  });
  const rawFacilitator = facilitator ?? new MerchantFacilitatorClient({
    url: config.facilitatorUrl,
    createAuthHeaders: async () => ({
      supported: {},
      verify: { "X-API-Key": config.apiKey },
      settle: { "X-API-Key": config.apiKey },
    }),
    fetchImpl,
  });
  const retryingFacilitator = withFacilitatorRetries(rawFacilitator);
  const probe = facilitatorProbe ?? createFacilitatorProbe({
    client: retryingFacilitator,
    facilitatorUrl: config.facilitatorUrl,
    network: config.network,
    fetchImpl,
  });
  const exactScheme = config.network.startsWith("near:")
    ? new ExactNearScheme()
    : new ExactEvmScheme();
  const resourceServer = new x402ResourceServer(retryingFacilitator)
    .register(config.network, exactScheme);
  const routes = createRoutes(config);
  const httpServer = new x402HTTPResourceServer(resourceServer, routes);
  const initializeHttpServer = paymentServerInitializer
    ?? (() => httpServer.initialize());
  let paymentInitialized = false;
  let paymentInitialization;

  async function initializePaymentServer() {
    if (paymentInitialized) return;
    if (!paymentInitialization) {
      paymentInitialization = (async () => {
        try {
          await initializeHttpServer();
          paymentInitialized = true;
        } catch (error) {
          paymentInitialization = undefined;
          throw error;
        }
      })();
    }
    return paymentInitialization;
  }

  const checkDependencies = createReadinessCache({
    check: () =>
      dependencyReadiness(reader, probe, initializePaymentServer, logger),
    ttlMs: readinessCacheMs,
    now,
  });

  const app = express();

  app.disable("x-powered-by");
  app.use(createCorsMiddleware(config.corsOrigins));

  app.get("/", (_request, response) =>
    response.type("html").send(landing(config)));
  app.get("/openapi.json", (_request, response) =>
    response.json(openApi(config, routes)));
  app.get("/llms.txt", (_request, response) =>
    response.type("text/plain").send(llms(config)));
  app.get("/pricing", (_request, response) =>
    response.type("html").send(pricing(config, routes)));
  app.get(["/terms", "/terms-of-service"], (_request, response) =>
    response.type("html").send(terms(config)));
  app.get("/robots.txt", (_request, response) =>
    response.type("text/plain").send("User-agent: *\nAllow: /\n"));
  app.get("/favicon.ico", (_request, response) =>
    response.type("image/png").send(favicon));
  app.get("/.well-known/x402", (_request, response) => response.json({
    version: 1,
    resources: Object.keys(routes).map(route =>
      `${config.resourceOrigin}${route.split(" ")[1].replace(":identifier", "{identifier}")}`),
  }));
  app.get("/healthz", (_request, response) => response.json({
    ok: true,
    network: config.network,
    activityIndex: activityStore.indexMetadata(),
    ...(config.releaseId ? { release: { id: config.releaseId } } : {}),
  }));
  app.get("/readyz", async (_request, response) => {
    const readiness = await checkDependencies();
    if (!readiness.ready) response.set("Retry-After", "1");
    response.status(readiness.ready ? 200 : 503).json(readiness);
  });

  // Initialization is owned above so a failed or delayed facilitator sync is
  // visible to startup and readiness rather than becoming an untracked
  // middleware promise.
  app.use(paymentMiddlewareFactory(httpServer, undefined, undefined, false));
  // The pinned x402 middleware and Bazaar extension inspect protected requests
  // through method, path, and headers only. Keep parsing after payment so an
  // unpaid malformed or oversized body receives its canonical 402 before
  // application validation. Review this ordering if a body-aware hook is ever
  // added to the protected-route path.
  app.use(express.json({ limit: "16kb", strict: true }));

  app.post("/v1/evidence/account", async (request, response, next) => {
    try {
      const body = validateEvidenceInput(
        config.network,
        "account",
        request.body,
        invalidInput,
      );
      const result = config.network.startsWith("near:")
        ? await reader.account(body.accountId)
        : await reader.account(body.address);
      response.json(result);
    } catch (error) {
      next(error);
    }
  });
  app.post("/v1/evidence/transaction", async (request, response, next) => {
    try {
      const body = validateEvidenceInput(
        config.network,
        "transaction",
        request.body,
        invalidInput,
      );
      const result = config.network.startsWith("near:")
        ? await reader.transaction(body.transactionHash, body.signerId)
        : await reader.transaction(body.transactionHash);
      response.json(result);
    } catch (error) {
      next(error);
    }
  });
  app.post("/v1/activity/search", (request, response, next) => {
    try {
      response.json(activityStore.search(request.body));
    } catch (error) {
      next(error);
    }
  });
  app.post("/v1/routes/usdc/quote", async (request, response, next) => {
    try {
      response.json(await quoter.quote(request.body));
    } catch (error) {
      next(error);
    }
  });
  app.get("/v1/entities/:identifier", (request, response) =>
    response.json(activityStore.entity(request.params.identifier)));

  app.use((error, _request, response, _next) => {
    if (error instanceof ChainEvidenceError) {
      return response.status(error.status).json({
        error: error.code,
        message: error.message,
      });
    }
    if (error instanceof UsdcRouteQuoteError) {
      return response.status(error.status).json({
        error: error.code,
        message: error.message,
      });
    }
    if (error?.type === "entity.too.large" || error?.status === 413) {
      return response.status(413).json({
        error: "payload_too_large",
        message: "request body exceeds the 16 KiB limit",
      });
    }
    if (error?.type === "entity.parse.failed") {
      return response.status(400).json({
        error: "invalid_input",
        message: "request body must be valid JSON",
      });
    }
    if (error?.status === 400) {
      return response.status(400).json({
        error: "invalid_input",
        message: error.message,
      });
    }
    logger.error(error);
    return response.status(503).json({
      error: "unavailable",
      message: "The merchant API could not produce authoritative evidence",
    });
  });

  return {
    app,
    config,
    routes,
    checkDependencies: async () => {
      const readiness = await checkDependencies();
      if (!readiness.ready) {
        throw new Error("merchant dependencies are not ready");
      }
      return readiness;
    },
  };
}

export async function startMerchantServer(options = {}) {
  const application = await createMerchantApplication(options);
  await application.checkDependencies();
  const server = await new Promise((resolve, reject) => {
    const listener = application.app.listen(
      application.config.port,
      "127.0.0.1",
      () => resolve(listener),
    );
    listener.once("error", reject);
  });
  console.log(
    `x402 merchant API listening on http://127.0.0.1:${application.config.port}`,
  );
  return { ...application, server };
}

async function dependencyReadiness(
  reader,
  facilitatorProbe,
  initializePaymentServer,
  logger = console,
) {
  const checkRpc = typeof reader.checkReadiness === "function"
    ? () => reader.checkReadiness()
    : () => reader.checkIdentity();
  const [rpc, facilitator, payment] = await Promise.allSettled([
    Promise.resolve().then(checkRpc),
    Promise.resolve().then(() => facilitatorProbe.check()),
    Promise.resolve().then(() => initializePaymentServer()),
  ]);
  const checks = {
    rpc: rpc.status === "fulfilled" ? "ready" : "not_ready",
    facilitator: facilitator.status === "fulfilled" ? "ready" : "not_ready",
    payment: payment.status === "fulfilled" ? "ready" : "not_ready",
  };
  logReadinessDependencyFailures(logger, { rpc, facilitator, payment });
  return {
    ready: checks.rpc === "ready"
      && checks.facilitator === "ready"
      && checks.payment === "ready",
    checks,
  };
}

function logReadinessDependencyFailures(logger, results) {
  for (const [dependency, result] of Object.entries(results)) {
    if (result.status !== "rejected") continue;
    logger?.warn?.({
      event: "merchant_readiness_dependency_failure",
      dependency,
      dependencyError: classifyDependencyError(result.reason),
      httpStatus: Number.isInteger(result.reason?.statusCode)
        ? result.reason.statusCode
        : 0,
    });
  }
}

function classifyDependencyError(error) {
  if (error?.name === "AbortError") return "aborted";
  if (Number.isInteger(error?.statusCode)) {
    if (error.statusCode === 429) return "http_rate_limited";
    if (error.statusCode === 503) return "http_temporarily_unavailable";
    if (error.statusCode >= 400 && error.statusCode <= 499) {
      return "http_client_error";
    }
    if (error.statusCode >= 500 && error.statusCode <= 599) {
      return "http_server_error";
    }
    return "http_status";
  }
  if (error?.name === "FacilitatorTimeoutError") return "timeout";
  if (error?.name === "FacilitatorResponseError") return "invalid_response";
  if (error instanceof ChainEvidenceError) return error.code;
  return "dependency_error";
}

function invalidInput(message) {
  const error = new Error(message);
  error.status = 400;
  return error;
}

function createApiSchemas(config) {
  const near = config.network.startsWith("near:");
  const blockSchema = {
    type: "object",
    additionalProperties: false,
    required: ["height", "hash"],
    properties: {
      height: near ? { type: "integer", minimum: 0 } : { type: "string", pattern: "^[0-9]+$" },
      hash: { type: "string", pattern: near ? PATTERNS.nearTransactionHash : PATTERNS.evmTransactionHash },
    },
  };
  const sourceSchema = {
    type: "object",
    additionalProperties: false,
    required: ["type", "status"],
    properties: {
      type: { type: "string", const: near ? "near-jsonrpc" : "evm-jsonrpc" },
      status: { type: "string", enum: ["final", "finalized", "nonterminal"] },
    },
  };
  const evidenceBase = {
    type: "object",
    additionalProperties: false,
    required: ["network", "kind", "observedFinality", "observedAt", "block", "source"],
    properties: {
      network: { type: "string", const: config.network },
      kind: { type: "string" },
      observedFinality: { type: "string", enum: ["final", "finalized", "nonterminal"] },
      observedAt: { type: "string", pattern: "^\\d{4}-\\d{2}-\\d{2}T" },
      block: blockSchema,
      source: sourceSchema,
      explorerUrl: { type: "string", pattern: "^https://" },
    },
  };
  const account = near
    ? {
      type: "object",
      additionalProperties: false,
      required: ["accountId", "amountYoctoNear", "lockedYoctoNear", "storageUsage", "codeHash"],
      properties: {
        accountId: {
          type: "string",
          minLength: 2,
          maxLength: 64,
          pattern: PATTERNS.nearAccountId,
        },
        amountYoctoNear: { type: "string", pattern: "^[0-9]+$" },
        lockedYoctoNear: { type: "string", pattern: "^[0-9]+$" },
        storageUsage: { type: "integer", minimum: 0 },
        codeHash: { type: "string", pattern: PATTERNS.nearCryptoHash },
      },
    }
    : {
      type: "object",
      additionalProperties: false,
      required: ["address", "balanceWei", "isContract", "asset"],
      properties: {
        address: { type: "string", pattern: PATTERNS.evmAddress },
        balanceWei: {
          type: "string",
          pattern: "^[0-9]+$",
          description: "Native ETH balance in wei at the finalized block; it is not a USDC balance.",
        },
        isContract: { type: "boolean" },
        asset: {
          type: "string",
          const: config.asset,
          description: "Configured USDC payment asset; it does not denominate balanceWei.",
        },
      },
    };
  const transaction = near
    ? {
      type: "object",
      additionalProperties: false,
      required: ["hash", "signerId", "blockHash", "success", "status", "receiptCount", "failures"],
      properties: {
        hash: { type: "string", pattern: PATTERNS.nearTransactionHash },
        signerId: {
          type: "string",
          minLength: 2,
          maxLength: 64,
          pattern: PATTERNS.nearAccountId,
        },
        receiverId: {
          type: "string",
          minLength: 2,
          maxLength: 64,
          pattern: PATTERNS.nearAccountId,
        },
        blockHash: { type: "string", pattern: PATTERNS.nearTransactionHash },
        success: { type: "boolean" },
        status: { type: "string", enum: ["succeeded", "failed"] },
        receiptCount: { type: "integer", minimum: 0 },
        failures: { type: "array", items: {} },
      },
    }
    : {
      type: "object",
      additionalProperties: false,
      required: ["hash", "from", "confirmationDepth", "status"],
      properties: {
        hash: { type: "string", pattern: PATTERNS.evmTransactionHash },
        from: { type: "string", pattern: PATTERNS.evmAddress },
        to: {
          type: ["string", "null"],
          description: "Transaction-envelope recipient. ERC-3009 USDC transfers normally target the token contract, not the payTo address.",
        },
        blockNumber: { type: "string", pattern: "^[0-9]+$" },
        confirmationDepth: { type: "string", pattern: "^[0-9]+$" },
        success: { type: "boolean" },
        status: { type: "string", enum: ["succeeded", "failed", "pending"] },
        gasUsed: { type: "string", pattern: "^[0-9]+$" },
      },
    };
  const accountOutput = { ...evidenceBase, required: [...evidenceBase.required, "account"], properties: { ...evidenceBase.properties, kind: { type: "string", const: "account" }, account } };
  const transactionOutput = { ...evidenceBase, required: [...evidenceBase.required, "transaction"], properties: { ...evidenceBase.properties, kind: { type: "string", const: "transaction" }, transaction } };
  const indexSchema = {
    type: "object",
    additionalProperties: false,
    required: ["status", "recordCount", "indexedAt"],
    properties: {
      status: { type: "string", enum: ["ready", "not_yet_indexed"] },
      recordCount: { type: "integer", minimum: 0 },
      indexedAt: { type: ["string", "null"] },
    },
  };
  const activityRecord = {
    type: "object",
    additionalProperties: false,
    required: ["id", "kind"],
    properties: {
      id: { type: "string" },
      network: { type: "string" },
      kind: { type: "string" },
      account: { type: "string" },
      contract: { type: "string" },
      entity: { type: "string" },
      block: {},
      timestamp: { type: "string" },
      summary: {},
      indexedAt: { type: "string" },
    },
  };
  const activityOutput = {
    type: "object",
    additionalProperties: false,
    required: ["items", "nextCursor", "index"],
    properties: {
      items: { type: "array", items: activityRecord },
      nextCursor: { type: ["string", "null"] },
      index: indexSchema,
    },
  };
  const entityOutput = {
    type: "object",
    additionalProperties: false,
    required: ["identifier", "status", "records", "index"],
    properties: {
      identifier: ENTITY_IDENTIFIER_SCHEMA,
      status: { type: "string", enum: ["indexed", "not_yet_indexed"] },
      records: { type: "array", items: activityRecord },
      index: indexSchema,
    },
  };
  return { accountOutput, transactionOutput, activityOutput, entityOutput };
}

function openApi(config, routes) {
  const near = config.network.startsWith("near:");
  const priceUsd = formatUsdc(config.amount);
  const accountInputSchema = evidenceInputSchema(config.network, "account");
  const transactionInputSchema = evidenceInputSchema(config.network, "transaction");
  const apiSchemas = createApiSchemas(config);
  const { accountOutput, transactionOutput, activityOutput, entityOutput } = apiSchemas;
  const outputExample = route => routes[route].extensions.bazaar.info.output.example;
  const inputExample = route => routes[route].extensions.bazaar.info.input;
  return {
    openapi: "3.1.0",
    info: {
      title: `${near ? "NEAR" : "Base"} Agent Evidence & Route API`,
      version: "0.3.0",
      ...(config.releaseId
        ? { "x-x402-merchant-release-id": config.releaseId }
        : {}),
      contact: config.contactEmail
        ? { email: config.contactEmail }
        : { url: "https://mikedotexe.com" },
      termsOfService: `${config.resourceOrigin}/terms`,
      description: "Paid, machine-readable chain evidence and an activity index whose availability is reported in every response.",
      "x-guidance": `Use POST /v1/evidence/account for ${near ? "a NEAR account id" : "an EVM address"}, POST /v1/evidence/transaction for ${near ? "a transaction hash plus signer account" : "an EVM transaction hash"}, or POST /v1/routes/usdc/quote for a dry Base-USDC-to-NEAR-USDC route quote. An unpaid request returns HTTP 402 with canonical x402 v2 requirements.`,
    },
    servers: [{ url: config.resourceOrigin }],
    paths: {
      "/v1/evidence/account": operation("Inspect final account evidence", accountInputSchema, inputExample("POST /v1/evidence/account").body, outputExample("POST /v1/evidence/account"), accountOutput),
      "/v1/evidence/transaction": operation("Inspect transaction evidence", transactionInputSchema, inputExample("POST /v1/evidence/transaction").body, outputExample("POST /v1/evidence/transaction"), transactionOutput),
      "/v1/activity/search": operation("Search the bounded final activity index", ACTIVITY_SEARCH_INPUT_SCHEMA, inputExample("POST /v1/activity/search").body, outputExample("POST /v1/activity/search"), activityOutput),
      "/v1/routes/usdc/quote": operation("Quote Base USDC to NEAR USDC", USDC_ROUTE_INPUT_SCHEMA, inputExample("POST /v1/routes/usdc/quote").body, outputExample("POST /v1/routes/usdc/quote"), usdcRouteOutputSchema),
      "/v1/entities/{identifier}": { get: { ...operation("Inspect an indexed entity", { type: "object", additionalProperties: false, properties: {} }, undefined, outputExample("GET /v1/entities/:identifier"), entityOutput).get, parameters: [{ name: "identifier", in: "path", required: true, schema: ENTITY_IDENTIFIER_SCHEMA, example: inputExample("GET /v1/entities/:identifier").pathParams.identifier }] } },
    },
  };

  function operation(summary, schema, requestExample, example, responseSchema) {
    const isGet = summary.startsWith("Inspect an indexed entity");
    return { [isGet ? "get" : "post"]: { summary, operationId: summary.toLowerCase().replace(/[^a-z]+/g, "_"), ...(isGet ? {} : { requestBody: { required: true, content: { "application/json": { schema, example: requestExample } } } }), "x-payment-info": { price: { mode: "fixed", currency: "USD", amount: priceUsd }, protocols: [{ x402: {} }] }, responses: { "200": { description: "Successful response", content: { "application/json": { schema: { ...responseSchema, example } } } }, "402": { description: "Payment Required" } } } };
  }
}

function llms(config) {
  return `# ${config.network} Agent Evidence & Route API\n\nPaid x402 API for authoritative chain evidence and route intelligence.\n\n- POST /v1/evidence/account — inspect a finalized account.\n- POST /v1/evidence/transaction — inspect final or nonterminal transaction evidence.\n- POST /v1/activity/search — search the bounded activity index; inspect \`index.status\` because records may not yet be available.\n- GET /v1/entities/{identifier} — inspect indexed activity for an entity.\n- POST /v1/routes/usdc/quote — request a quote-only route with provider-supplied signature metadata from canonical Base USDC to canonical NEAR USDC. The signature is preserved as provenance; this service does not claim to verify it cryptographically. This dry route never returns a deposit address and never moves funds.\n- GET /pricing — read the exact configured price, network, asset, and recipient.\n- GET /terms — read the operational terms for this reference service.\n\nCall a route without payment first. Read the PAYMENT-REQUIRED header, sign the exact requirement, and retry with PAYMENT-SIGNATURE. Prices are decimal USD in OpenAPI and atomic USDC units at runtime.\n`;
}

function pricing(config, routes) {
  const priceUsd = formatUsdc(config.amount);
  const eip712 = config.eip712Name
    ? `<dt>EIP-712 domain</dt><dd><code>${escapeHtml(config.eip712Name)}</code> / <code>${escapeHtml(config.eip712Version)}</code></dd>`
    : "";
  const routeRows = Object.entries(routes).map(([route, definition]) => {
    const requirement = definition.accepts[0];
    return `<tr><td><code>${escapeHtml(route)}</code></td><td><code>$${escapeHtml(priceUsd)}</code></td><td><code>${escapeHtml(requirement.network)}</code></td></tr>`;
  }).join("\n");
  return publicPage(
    `${config.network} pricing`,
    `Payment policy for ${config.network}`,
    `<p>Each listed resource costs <strong>$${escapeHtml(priceUsd)}</strong> (${escapeHtml(config.amount)} atomic USDC) when its x402 requirement is accepted.</p>
  <table>
    <thead><tr><th>Resource</th><th>Price</th><th>Network</th></tr></thead>
    <tbody>${routeRows}</tbody>
  </table>
  <h2>Exact payment policy</h2>
  <dl>
    <dt>Asset</dt><dd><code>${escapeHtml(config.asset)}</code></dd>
    <dt>Recipient (payTo)</dt><dd><code>${escapeHtml(config.payTo)}</code></dd>
    ${eip712}
  </dl>
  <p>Retrieve a resource without a payment header first. The resulting HTTP 402 response is the authoritative exact requirement; do not sign a requirement whose network, asset, recipient, or amount differs from this page.</p>`,
  );
}

function terms(config) {
  return publicPage(
    `${config.network} operational terms`,
    `Operational terms for ${config.network}`,
    `<p>This is a reference x402 service for machine-readable chain evidence and quote-only routing on <code>${escapeHtml(config.network)}</code>.</p>
  <ul>
    <li>Each paid request is governed by the exact payment requirement returned in that resource's HTTP 402 response.</li>
    <li>The service has no availability commitment. Chain, RPC, and facilitator outcomes are returned only when the service can observe authoritative evidence.</li>
    <li>Never send API keys, private keys, or signed payment authorizations to public support channels.</li>
    <li>Use the <a href="/openapi.json">OpenAPI contract</a>, <a href="/pricing">pricing</a>, and <a href="/.well-known/x402">x402 discovery document</a> before integrating.</li>
  </ul>
  <p>Security reports belong in the repository's <a href="https://github.com/fastnear/x402-facilitator/security/policy">private reporting process</a>.</p>`,
  );
}

function publicPage(title, description, content) {
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>${escapeHtml(title)}</title>
  <meta name="description" content="${escapeHtml(description)}">
  <style>
    :root { color-scheme: dark; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
    body { max-width: 58rem; margin: 0 auto; padding: 4rem 1.5rem; background: #0b0d10; color: #e8edf2; line-height: 1.6; }
    a { color: #7dd3fc; }
    code { color: #a7f3d0; overflow-wrap: anywhere; }
    table { width: 100%; border-collapse: collapse; margin: 1.5rem 0; }
    th, td { border: 1px solid #2d3640; padding: .75rem; text-align: left; vertical-align: top; }
    dt { color: #a7f3d0; font-weight: bold; margin-top: .75rem; }
    dd { margin-left: 0; }
  </style>
</head>
<body>
  <p><a href="/">mikedotexe.com / x402</a></p>
  <h1>${escapeHtml(title)}</h1>
  ${content}
</body>
</html>`;
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, character => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[character]);
}

function landing(config) {
  const chain = config.network.startsWith("near:") ? "NEAR" : "Base";
  const priceUsd = formatUsdc(config.amount);
  const entityProbe = config.network.startsWith("near:")
    ? `${config.resourceOrigin}/v1/entities/mike.near`
    : `${config.resourceOrigin}/v1/entities/0x0000000000000000000000000000000000000000`;
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
  <p>Machine-readable chain evidence and route intelligence on <code>${config.network}</code>, priced at <code>$${priceUsd}</code> per request and settled through x402.</p>
  <div class="card">
    <p><strong>Agent discovery</strong></p>
    <ul>
      <li><a href="/openapi.json">OpenAPI contract</a></li>
      <li><a href="/llms.txt">llms.txt guidance</a></li>
      <li><a href="/.well-known/x402">x402 discovery</a></li>
      <li><a href="/pricing">pricing and payment policy</a></li>
      <li><a href="/terms">operational terms</a></li>
    </ul>
  </div>
  <p>Start with <code>POST /v1/evidence/account</code>, <code>POST /v1/evidence/transaction</code>, or the quote-only <code>POST /v1/routes/usdc/quote</code>. An unpaid valid request returns canonical x402 v2 payment requirements.</p>
  <p>Directory-safe unpaid probe: <code>GET <a href="${escapeHtml(entityProbe)}">${escapeHtml(entityProbe)}</a></code>. It returns the canonical x402 requirement without a payment.</p>
</body>
</html>`;
}
