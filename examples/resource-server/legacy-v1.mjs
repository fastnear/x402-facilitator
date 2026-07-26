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
