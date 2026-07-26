// Legacy x402 v1 (0.x SDK) interop helpers. v1 clients read payment
// requirements from the 402 JSON body ({x402Version: 1, error, accepts})
// rather than the v2 PAYMENT-REQUIRED header, name networks "base" /
// "base-sepolia" rather than CAIP-2, and call the amount maxAmountRequired
// rather than amount. Everything here is pure: build the bodies once at
// startup and serve frozen objects.

export const V1_NETWORK_NAMES = Object.freeze({
  "eip155:8453": "base",
  "eip155:84532": "base-sepolia",
});

const V1_NETWORK_CAIP2 = Object.freeze(
  Object.fromEntries(Object.entries(V1_NETWORK_NAMES).map(([caip2, name]) => [name, caip2])),
);

export function v1NetworkName(caip2Network) {
  return V1_NETWORK_NAMES[caip2Network];
}

export function caip2ForV1Network(name) {
  return V1_NETWORK_CAIP2[name];
}

// v1 PaymentRequirements for the "exact" scheme. `extra` must carry the
// token's true EIP-712 domain (Base mainnet USDC is name "USD Coin",
// version "2") — a v1 client derives its signing domain from it, so a wrong
// name produces signatures that can never verify. Returns undefined when the
// network has no v1 name (v1 never covered NEAR) or when no public resource
// URL is configured: v1 requires `resource`, and advertising a guessed URL
// is worse than advertising no v1 entry.
export function buildV1Requirements({
  network,
  asset,
  payTo,
  amount,
  resourceUrl,
  description,
  mimeType,
  maxTimeoutSeconds = 300,
  extra,
}) {
  const name = v1NetworkName(network);
  if (!name || !resourceUrl) {
    return undefined;
  }
  return Object.freeze({
    scheme: "exact",
    network: name,
    maxAmountRequired: amount,
    resource: resourceUrl,
    description,
    mimeType,
    payTo,
    maxTimeoutSeconds,
    asset,
    ...(extra ? { extra: Object.freeze({ ...extra }) } : {}),
  });
}

export function buildV1PaymentRequiredBody(requirements, error = "Payment required") {
  return Object.freeze({
    x402Version: 1,
    error,
    accepts: Object.freeze([requirements]),
  });
}

// For networks v1 never covered, an informational body beats an empty {}:
// it tells body-reading clients where the real requirements live without
// impersonating a v1 response.
export function buildUnpaidHintBody() {
  return Object.freeze({
    error: "Payment required",
    hint: "x402 v2 payment requirements are base64-encoded in the PAYMENT-REQUIRED response header; pay by resubmitting with a signed PAYMENT-SIGNATURE header.",
  });
}

// A v1 X-PAYMENT payload carries scheme/network at the top level and no
// echo of the accepted requirements; v2 nests the identical inner
// {signature, authorization} under `payload` and echoes the requirements as
// `accepted` (which the v2 server deep-equals against what it computed for
// the route). Translating therefore needs the route's own v2 requirements,
// not anything from the client. Returns undefined for anything that is not
// a v1 exact payment for this route's network.
export function translateV1PaymentToV2(v1Payload, { requirementsV2, resourceObject }) {
  if (
    v1Payload === null ||
    typeof v1Payload !== "object" ||
    Array.isArray(v1Payload) ||
    v1Payload.x402Version !== 1 ||
    v1Payload.scheme !== requirementsV2.scheme ||
    caip2ForV1Network(v1Payload.network) !== requirementsV2.network ||
    v1Payload.payload === null ||
    typeof v1Payload.payload !== "object" ||
    Array.isArray(v1Payload.payload)
  ) {
    return undefined;
  }
  return {
    x402Version: 2,
    ...(resourceObject ? { resource: resourceObject } : {}),
    accepted: requirementsV2,
    payload: v1Payload.payload,
  };
}

// v1 clients read settlement results from an X-PAYMENT-RESPONSE header with
// the v1 network name; v2 sets PAYMENT-RESPONSE with CAIP-2. Same base64
// JSON otherwise. Throws on undecodable input — callers treat that as
// "leave only the v2 header".
export function translateSettleHeaderToV1(paymentResponseHeader) {
  const decoded = JSON.parse(Buffer.from(String(paymentResponseHeader), "base64").toString("utf8"));
  const name = v1NetworkName(decoded.network);
  return Buffer.from(JSON.stringify(name ? { ...decoded, network: name } : decoded)).toString(
    "base64",
  );
}
