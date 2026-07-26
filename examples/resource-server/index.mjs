import { createHash } from "node:crypto";
import { constants } from "node:fs";
import { open } from "node:fs/promises";

import { HTTPFacilitatorClient } from "@x402/core/server";
import {
  paymentMiddlewareFromHTTPServer,
  x402HTTPResourceServer,
  x402ResourceServer,
} from "@x402/express";
import {
  PAYMENT_IDENTIFIER,
  declarePaymentIdentifierExtension,
  extractPaymentIdentifier,
} from "@x402/extensions/payment-identifier";
import { ExactEvmScheme } from "@x402/evm/exact/server";
import { ExactNearScheme } from "@x402/near/exact/server";
import express from "express";

import {
  DeliveryJournal,
  canonicalJson,
  payloadFingerprint,
  workFingerprint,
} from "./journal.mjs";
import {
  buildUnpaidHintBody,
  buildV1PaymentRequiredBody,
  buildV1Requirements,
} from "./legacy-v1.mjs";

const facilitatorUrl = requiredEnvironment("FACILITATOR_URL");
const apiKey = await readCredential(requiredEnvironment("FACILITATOR_API_KEY_FILE"));
const network = requiredEnvironment("NETWORK");
const asset = requiredEnvironment("ASSET");
const payTo = requiredEnvironment("PAY_TO");
const amount = process.env.AMOUNT ?? "1000";
const port = parsePort(process.env.PORT ?? "4021");

// Public https URL of the paid endpoint, advertised verbatim as the x402
// resource URL. The demo sits behind a TLS-terminating proxy that does not
// forward the original scheme, so deriving the URL per-request would
// advertise http://.
const resourceUrl = process.env.RESOURCE_URL;
if (resourceUrl !== undefined && !/^https:\/\/\S+$/.test(resourceUrl)) {
  throw new Error("RESOURCE_URL must be an https:// URL");
}

const isEvm = network.startsWith("eip155:");
const isNear = network === "near:testnet" || network === "near:mainnet";
if (!isEvm && !isNear) {
  throw new Error("NETWORK must be near:testnet, near:mainnet, or eip155:<chainId>");
}
if (!/^[0-9]+$/.test(amount) || BigInt(amount) < 1000n) {
  throw new Error("AMOUNT must be at least 1000 atomic USDC");
}

// eip155 "exact" is ERC-3009: the client signs an EIP-712 TransferWithAuthorization
// over the token's domain, so the payment requirements must carry the token's
// EIP-712 `name`/`version` in `extra`. These are the token contract's own domain
// values (e.g. Base USDC is name "USD Coin", version "2") — not the symbol.
const eip712Name = process.env.ASSET_EIP712_NAME;
const eip712Version = process.env.ASSET_EIP712_VERSION ?? "2";
if (isEvm && !eip712Name) {
  throw new Error(
    "ASSET_EIP712_NAME is required for eip155 networks (e.g. 'USD Coin' for Base USDC)",
  );
}
const acceptsExtra = isEvm ? { name: eip712Name, version: eip712Version } : undefined;

const facilitator = new HTTPFacilitatorClient({
  url: facilitatorUrl,
  createAuthHeaders: async () => ({
    supported: {},
    verify: { "X-API-Key": apiKey },
    settle: { "X-API-Key": apiKey },
  }),
});

const route = "POST /work";
const routes = {
  [route]: {
    accepts: [
      {
        scheme: "exact",
        price: { asset, amount },
        network,
        payTo,
        ...(acceptsExtra ? { extra: acceptsExtra } : {}),
      },
    ],
    description: "Deterministic paid work with independent delivery deduplication",
    mimeType: "application/json",
    ...(resourceUrl ? { resource: resourceUrl } : {}),
    extensions: {
      // Optional (not required): a client may send a payment-identifier to opt
      // into resource-layer delivery idempotency (replay returns the cached
      // result), but standard x402 clients that omit it are still served.
      [PAYMENT_IDENTIFIER]: declarePaymentIdentifierExtension(false),
    },
  },
};

// Dual-emit: the v2 PAYMENT-REQUIRED header stays authoritative, but legacy
// x402 v1 (0.x SDK) clients only read the 402 JSON body, so eip155 routes
// also serve the v1 shape there. NEAR has no v1 network name; those demos
// serve a hint pointing at the header instead.
const v1Requirements = isEvm
  ? buildV1Requirements({
      network,
      asset,
      payTo,
      amount,
      resourceUrl,
      description: routes[route].description,
      mimeType: routes[route].mimeType,
      extra: acceptsExtra,
    })
  : undefined;
const unpaidBody = v1Requirements
  ? buildV1PaymentRequiredBody(v1Requirements)
  : buildUnpaidHintBody();
// These hooks must never throw: @x402/core awaits them uncaught, so a throw
// would turn the 402 into a 500. Both close over frozen precomputed objects.
routes[route].unpaidResponseBody = () => ({
  contentType: "application/json",
  body: unpaidBody,
});
routes[route].settlementFailedResponseBody = (context, failure) => ({
  contentType: "application/json",
  body: v1Requirements
    ? buildV1PaymentRequiredBody(v1Requirements, failure?.errorReason ?? "settlement_failed")
    : { ...unpaidBody, error: failure?.errorReason ?? "settlement_failed" },
});

// Development-only journal. Production must use durable transactional storage.
const deliveries = new DeliveryJournal();
const resourceServer = new x402ResourceServer(facilitator)
  .register(network, isEvm ? new ExactEvmScheme() : new ExactNearScheme())
  .onAfterSettle(async ({ paymentPayload, result }) => {
    if (!result.success) {
      return;
    }
    const identifier = extractPaymentIdentifier(paymentPayload);
    if (!identifier) {
      // payment-identifier is optional: a payment without one is a
      // payment-per-request settlement with no delivery-journal entry to mark.
      return;
    }
    if (!deliveries.markSettled(identifier, payloadFingerprint(paymentPayload))) {
      throw new Error("settlement succeeded without a matching delivery-journal entry");
    }
  });

const httpServer = new x402HTTPResourceServer(resourceServer, routes).onProtectedRequest(
  async context => {
    if (!context.paymentHeader) {
      return;
    }
    let paymentPayload;
    try {
      paymentPayload = JSON.parse(Buffer.from(context.paymentHeader, "base64").toString("utf8"));
    } catch {
      // Leave malformed payload handling to the official x402 middleware.
      return;
    }
    const identifier = extractPaymentIdentifier(paymentPayload);
    if (!identifier) {
      return;
    }
    const request = context.adapter.req;
    request.x402PaymentIdentifier = identifier;
    request.x402PaymentFingerprint = payloadFingerprint(paymentPayload);
    request.x402WorkFingerprint = workFingerprint(paymentPayload, request);
    const observed = deliveries.prepare(
      identifier,
      request.x402PaymentFingerprint,
      request.x402WorkFingerprint,
      () => ({
        result: createHash("sha256").update(canonicalJson(request.body)).digest("hex"),
      }),
    );
    if (observed.status === "conflict") {
      request.x402PaymentConflict = true;
      return { grantAccess: true };
    }
    if (observed.status === "settled") {
      request.x402PaymentReplay = true;
      return { grantAccess: true };
    }
    if (observed.status === "new") {
      request.x402DeliveryWasNew = true;
    }
    if (observed.status === "pending") {
      request.x402PaymentRetry = true;
    }
  },
);

const app = express();
app.use(express.json({ limit: "16kb", strict: true }));
app.use(paymentMiddlewareFromHTTPServer(httpServer));
app.post("/work", (request, response) => {
  const work = () => ({
    result: createHash("sha256").update(canonicalJson(request.body)).digest("hex"),
  });
  const identifier = request.x402PaymentIdentifier;
  if (!identifier) {
    // payment-identifier is optional: with no identifier this is a
    // payment-per-request delivery and there is no journal entry to dedup.
    response.json({ ...work(), replayed: false });
    return;
  }
  if (request.x402PaymentConflict) {
    response.status(409).json({ error: "payment identifier already used for other work" });
    return;
  }
  const prepared = deliveries.prepare(
    identifier,
    request.x402PaymentFingerprint,
    request.x402WorkFingerprint,
    work,
  );
  if (prepared.status === "conflict") {
    response.status(409).json({ error: "payment identifier already used for other work" });
    return;
  }
  response.json({
    ...prepared.entry.response,
    replayed: !request.x402DeliveryWasNew || Boolean(request.x402PaymentReplay),
  });
});

app.listen(port, "127.0.0.1", () => {
  console.log(`NEAR x402 reference workload listening on http://127.0.0.1:${port}`);
});

function requiredEnvironment(name) {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} is required`);
  }
  return value;
}

async function readCredential(path) {
  const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const metadata = await handle.stat();
    if (!metadata.isFile() || (metadata.mode & 0o077) !== 0) {
      throw new Error("facilitator API key file must be a mode-0600 regular file");
    }
    const value = await handle.readFile("utf8");
    if (!value.endsWith("\n") || value.endsWith("\n\n")) {
      throw new Error("facilitator API key file must end with exactly one newline");
    }
    const key = value.slice(0, -1);
    if (
      key.trim() !== key ||
      !/^x402_(?:test|live)_[0-9a-f]{24}\.[0-9a-f]{64}$/.test(key)
    ) {
      throw new Error("facilitator API key file has an invalid value");
    }
    return key;
  } finally {
    await handle.close();
  }
}

function parsePort(value) {
  if (!/^[0-9]+$/.test(value)) {
    throw new Error("PORT must be an integer");
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 1 || parsed > 65535) {
    throw new Error("PORT is out of range");
  }
  return parsed;
}
