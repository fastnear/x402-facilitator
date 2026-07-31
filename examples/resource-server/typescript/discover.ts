import { HTTPFacilitatorClient } from "@x402/core/server";
import { withBazaar, type ListDiscoveryResourcesParams } from "@x402/extensions/bazaar";

function requireFacilitatorUrl(value: string): string {
  const url = new URL(value);
  const localDevelopment =
    url.protocol === "http:" && (url.hostname === "127.0.0.1" || url.hostname === "localhost");
  if (url.protocol !== "https:" && !localDevelopment) {
    throw new Error("facilitator URL must use HTTPS outside loopback development");
  }
  if (url.username || url.password || url.hash) {
    throw new Error("facilitator URL must not contain credentials or a fragment");
  }
  return url.toString().replace(/\/$/, "");
}

/** Build a public, unauthenticated x402 Bazaar catalog client. */
export function createDiscoveryClient(facilitatorUrl: string) {
  return withBazaar(
    new HTTPFacilitatorClient({
      url: requireFacilitatorUrl(facilitatorUrl),
    }),
  );
}

/** List resources without exposing or requiring a facilitator API key. */
export async function listDiscoveryResources(
  facilitatorUrl: string,
  filters: ListDiscoveryResourcesParams = {},
) {
  const client = createDiscoveryClient(facilitatorUrl);
  return client.extensions.bazaar.listResources(filters);
}
