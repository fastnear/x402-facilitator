import assert from "node:assert/strict";
import test from "node:test";

import { createCorsMiddleware, parseAllowedOrigins } from "./cors.mjs";

function mockResponse() {
  return {
    headers: {},
    statusCode: 200,
    body: undefined,
    ended: false,
    set(values) {
      Object.assign(this.headers, values);
      return this;
    },
    vary(value) {
      this.headers.Vary = value;
      return this;
    },
    status(value) {
      this.statusCode = value;
      return this;
    },
    json(value) {
      this.body = value;
      this.ended = true;
      return this;
    },
    end() {
      this.ended = true;
      return this;
    },
  };
}

test("CORS origin parsing requires exact HTTP origins", () => {
  assert.deepEqual(
    parseAllowedOrigins("https://js.fastnear.com, http://localhost:8000,https://js.fastnear.com"),
    ["https://js.fastnear.com", "http://localhost:8000"],
  );
  assert.throws(() => parseAllowedOrigins("*"), /exact origins/);
  assert.throws(() => parseAllowedOrigins("https://js.fastnear.com/path"), /without paths/);
});

test("allowed browser request exposes canonical x402 response headers", () => {
  const middleware = createCorsMiddleware(["https://js.fastnear.com"]);
  const response = mockResponse();
  let nextCalled = false;
  middleware(
    {
      method: "POST",
      get: name => name === "origin" ? "https://js.fastnear.com" : undefined,
    },
    response,
    () => { nextCalled = true; },
  );
  assert.equal(nextCalled, true);
  assert.equal(response.headers["Access-Control-Allow-Origin"], "https://js.fastnear.com");
  assert.match(response.headers["Access-Control-Expose-Headers"], /PAYMENT-REQUIRED/);
  assert.match(response.headers["Access-Control-Expose-Headers"], /PAYMENT-RESPONSE/);
  assert.equal(response.headers.Vary, "Origin");
});

test("allowed preflight is free and terminates before route middleware", () => {
  const middleware = createCorsMiddleware(["https://js.fastnear.com"]);
  const response = mockResponse();
  let nextCalled = false;
  middleware(
    {
      method: "OPTIONS",
      get: name => name === "origin" ? "https://js.fastnear.com" : undefined,
    },
    response,
    () => { nextCalled = true; },
  );
  assert.equal(nextCalled, false);
  assert.equal(response.statusCode, 204);
  assert.equal(response.ended, true);
  assert.match(response.headers["Access-Control-Allow-Headers"], /PAYMENT-SIGNATURE/);
});

test("disallowed browser preflight fails explicitly", () => {
  const middleware = createCorsMiddleware(["https://js.fastnear.com"]);
  const response = mockResponse();
  let nextCalled = false;
  middleware(
    {
      method: "OPTIONS",
      get: name => name === "origin" ? "https://attacker.invalid" : undefined,
    },
    response,
    () => { nextCalled = true; },
  );
  assert.equal(nextCalled, false);
  assert.equal(response.statusCode, 403);
  assert.equal(response.body.error, "cors_origin_denied");
  assert.equal(response.headers["Access-Control-Allow-Origin"], undefined);
});
