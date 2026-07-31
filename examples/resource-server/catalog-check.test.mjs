import assert from "node:assert/strict";
import test from "node:test";

import { declareDiscoveryExtension } from "@x402/extensions/bazaar";

import { validateCatalogBazaarExtensions } from "./catalog-check.mjs";

test("accepts an empty audited catalog", () => {
  assert.doesNotThrow(() =>
    validateCatalogBazaarExtensions({ schemaVersion: 1, resources: [] }),
  );
});

test("accepts metadata emitted by the pinned official declaration helper", () => {
  const extensions = declareDiscoveryExtension({
    method: "POST",
    bodyType: "json",
    input: { accountId: "example.near" },
    inputSchema: {
      properties: { accountId: { type: "string" } },
      required: ["accountId"],
    },
    output: { example: { accountId: "example.near", final: true } },
  });
  assert.doesNotThrow(() =>
    validateCatalogBazaarExtensions({
      schemaVersion: 1,
      resources: [{ extensions }],
    }),
  );
});

test("validates Bazaar info against its declared schema", () => {
  assert.throws(
    () =>
      validateCatalogBazaarExtensions({
        schemaVersion: 1,
        resources: [
          {
            extensions: {
              bazaar: {
                info: { input: { type: "http", method: "POST" } },
                schema: {
                  type: "object",
                  required: ["output"],
                  properties: { output: { type: "object" } },
                },
              },
            },
          },
        ],
      }),
    /invalid Bazaar (?:shape|metadata)/u,
  );
});
