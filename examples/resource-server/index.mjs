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
  translateSettleHeaderToV1,
  translateV1PaymentToV2,
} from "./legacy-v1.mjs";
import { withFacilitatorRetries } from "./retry.mjs";

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

// Facilitator throws (transient 503s like rpc_unavailable, or a settlement
// still reaching its terminal state) are retried with short backoff: verify
// is read-only and settle is idempotent at the facilitator (a repeat replays
// the journaled terminal result), while the middleware would otherwise
// surface one transient throw as a client-facing failure even after the
// settlement succeeds.
const facilitator = withFacilitatorRetries(
  new HTTPFacilitatorClient({
    url: facilitatorUrl,
    createAuthHeaders: async () => ({
      supported: {},
      verify: { "X-API-Key": apiKey },
      settle: { "X-API-Key": apiKey },
    }),
  }),
);

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

// The v2 requirements this route resolves to, for injecting into translated
// legacy v1 payments as `accepted`. The middleware deep-equals the accepted
// core (scheme, network, amount, asset, payTo, maxTimeoutSeconds) against
// what it computes from `routes` and requires its extra to be a subset of
// this one (@x402/core@2.19.0 server/index.js:1080-1092, :1847), so this
// object is built from the same constants as `routes` and must stay
// byte-identical to the accepts entry in the emitted PAYMENT-REQUIRED
// header.
const requirementsV2 = Object.freeze({
  scheme: "exact",
  network,
  amount,
  asset,
  payTo,
  maxTimeoutSeconds: 300,
  ...(acceptsExtra ? { extra: Object.freeze({ ...acceptsExtra }) } : {}),
});
const resourceObject = resourceUrl
  ? Object.freeze({
      url: resourceUrl,
      description: routes[route].description,
      mimeType: routes[route].mimeType,
    })
  : undefined;

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
app.use(legacyV1PaymentShim);
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
  console.log(`x402 reference workload listening on http://127.0.0.1:${port}`);
});

// Legacy v1 payment acceptance. The @x402 middleware reads payments only
// from the payment-signature header, so a v1 client's X-PAYMENT would be
// ignored and answered with another 402. When a well-formed v1 exact
// payment for this route's network arrives, rewrite it into the v2 shape on
// the request, and mirror the middleware's settlement/rejection responses
// back into the v1 dialect (X-PAYMENT-RESPONSE header, v1 402 body). On any
// decode or shape problem the request is left untouched and falls through
// to the normal unpaid 402, which dual-emits the v1 body.
function legacyV1PaymentShim(request, response, next) {
  if (!isEvm || request.headers["payment-signature"]) {
    next();
    return;
  }
  const header = request.headers["x-payment"];
  if (typeof header !== "string" || header === "") {
    next();
    return;
  }
  let v1Payload;
  try {
    v1Payload = JSON.parse(Buffer.from(header, "base64").toString("utf8"));
  } catch {
    next();
    return;
  }
  const translated = translateV1PaymentToV2(v1Payload, { requirementsV2, resourceObject });
  if (!translated) {
    next();
    return;
  }
  request.headers["payment-signature"] = Buffer.from(JSON.stringify(translated)).toString("base64");
  request.x402LegacyV1 = true;
  // The middleware sets PAYMENT-RESPONSE via setHeader after settling,
  // while the handler's output is still buffered, so wrapping setHeader is
  // early enough to mirror it.
  const setHeader = response.setHeader.bind(response);
  response.setHeader = (name, value) => {
    if (typeof name === "string" && name.toLowerCase() === "payment-response") {
      try {
        setHeader("X-PAYMENT-RESPONSE", translateSettleHeaderToV1(value));
      } catch {
        // Undecodable settle header: leave only the v2 header in place.
      }
    }
    return setHeader(name, value);
  };
  // Post-payment 402s (requirements mismatch, verify failure) carry a `{}`
  // body plus the refreshed PAYMENT-REQUIRED header; a v1 client needs the
  // v1 body there too, with the error the header carries.
  const json = response.json.bind(response);
  response.json = body => {
    if (
      response.statusCode === 402 &&
      v1Requirements &&
      (body === undefined ||
        body === null ||
        (typeof body === "object" && !Array.isArray(body) && body.x402Version === undefined))
    ) {
      let error = "payment_rejected";
      const headerValue = response.getHeader("PAYMENT-REQUIRED");
      if (typeof headerValue === "string") {
        try {
          const decoded = JSON.parse(Buffer.from(headerValue, "base64").toString("utf8"));
          if (typeof decoded.error === "string" && decoded.error !== "") {
            error = decoded.error;
          }
        } catch {
          // Keep the generic error when the header cannot be decoded.
        }
      }
      return json(buildV1PaymentRequiredBody(v1Requirements, error));
    }
    return json(body);
  };
  next();
}

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
