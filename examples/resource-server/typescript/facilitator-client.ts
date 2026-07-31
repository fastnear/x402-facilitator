import { HTTPFacilitatorClient } from "@x402/core/server";

export interface AuthenticatedFacilitatorConfig {
  /** Public facilitator base URL. Production integrations must use HTTPS. */
  facilitatorUrl: string;
  /** Server-side secret loaded from a secret manager or protected credential file. */
  apiKey: string;
}

/**
 * Construct the official server-side client with per-operation authentication.
 * Never call this module from browser or payer code.
 */
export function createAuthenticatedFacilitatorClient({
  facilitatorUrl,
  apiKey,
}: AuthenticatedFacilitatorConfig): HTTPFacilitatorClient {
  const url = new URL(facilitatorUrl);
  const localDevelopment =
    url.protocol === "http:" && (url.hostname === "127.0.0.1" || url.hostname === "localhost");
  if (url.protocol !== "https:" && !localDevelopment) {
    throw new Error("facilitator URL must use HTTPS outside loopback development");
  }
  if (url.username || url.password || url.hash) {
    throw new Error("facilitator URL must not contain credentials or a fragment");
  }
  if (!apiKey || /[\r\n]/u.test(apiKey)) {
    throw new Error("apiKey must be one non-empty line");
  }
  const baseUrl = url.toString().replace(/\/$/, "");
  return new HTTPFacilitatorClient({
    url: baseUrl,
    createAuthHeaders: async () => ({
      supported: {},
      verify: { "X-API-Key": apiKey },
      settle: { "X-API-Key": apiKey },
      bazaar: {},
    }),
  });
}

/** Canonical Base mainnet values for an exact Circle USDC resource route. */
export const BASE_MAINNET_USDC = Object.freeze({
  network: "eip155:8453" as const,
  asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
  extra: Object.freeze({ name: "USD Coin", version: "2" }),
});
