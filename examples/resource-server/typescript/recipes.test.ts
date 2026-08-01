import assert from "node:assert/strict";
import { createServer } from "node:http";
import { after, before, test } from "node:test";

import { listDiscoveryResources } from "./discover.js";
import {
  BASE_MAINNET_USDC,
  createAuthenticatedFacilitatorClient,
} from "./facilitator-client.js";

let origin = "";
const server = createServer((request, response) => {
  if (request.url?.startsWith("/discovery/resources")) {
    response.setHeader("content-type", "application/json");
    response.end(
      JSON.stringify({
        x402Version: 2,
        items: [],
        pagination: { limit: 1, offset: 0, total: 0 },
      }),
    );
    return;
  }
  response.statusCode = 404;
  response.end();
});

before(async () => {
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => resolve());
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("test server did not bind TCP");
  origin = `http://127.0.0.1:${address.port}`;
});

after(async () => {
  await new Promise<void>((resolve, reject) => {
    server.close(error => (error ? reject(error) : resolve()));
  });
});

test("official Bazaar client reads the facilitator catalog", async () => {
  const result = await listDiscoveryResources(origin, { network: "eip155:8453", limit: 1 });
  assert.equal(result.x402Version, 2);
  assert.deepEqual(result.items, []);
  assert.equal(result.pagination.total, 0);
});

test("authenticated client scopes the key to verify and settle", async () => {
  const client = createAuthenticatedFacilitatorClient({
    facilitatorUrl: origin,
    apiKey: "x402_test_recipe_key",
  });
  assert.deepEqual(await client.createAuthHeaders("supported"), { headers: {} });
  assert.deepEqual(await client.createAuthHeaders("bazaar"), { headers: {} });
  assert.deepEqual(await client.createAuthHeaders("verify"), {
    headers: { "X-API-Key": "x402_test_recipe_key" },
  });
  assert.deepEqual(BASE_MAINNET_USDC.extra, { name: "USD Coin", version: "2" });
});

test("recipes reject unsafe URLs and multiline keys", async () => {
  await assert.rejects(() => listDiscoveryResources("http://merchant.example"));
  assert.throws(() =>
    createAuthenticatedFacilitatorClient({
      facilitatorUrl: "https://facilitator.example",
      apiKey: "one\ntwo",
    }),
  );
});
