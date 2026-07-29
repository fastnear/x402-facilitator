import assert from "node:assert/strict";
import test from "node:test";

import {
  BASE_USDC,
  NEAR_USDC,
  UsdcRouteQuoteError,
  createUsdcRouteQuoter,
} from "./usdc-route.mjs";

const fixedNow = Date.parse("2026-07-27T20:00:00.000Z");
const deadline = "2026-07-27T20:05:00.000Z";
const input = {
  amountAtomic: "1000000",
  recipient: "mike.near",
  refundTo: "0xcA202E03f11Aa076c57EAE666Da3f933dCc71CC9",
};

function providerFixture(overrides = {}) {
  return {
    quote: {
      amountIn: "1000000",
      amountOut: "998898",
      minAmountOut: "988909",
      timeEstimate: 35,
      refundFee: "2400",
      withdrawFee: "0",
      ...overrides.quote,
    },
    quoteRequest: {
      dry: true,
      swapType: "EXACT_INPUT",
      slippageTolerance: 100,
      originAsset: BASE_USDC.assetId,
      depositType: "ORIGIN_CHAIN",
      destinationAsset: NEAR_USDC.assetId,
      amount: "1000000",
      recipient: "mike.near",
      recipientType: "DESTINATION_CHAIN",
      refundTo: "0xcA202E03f11Aa076c57EAE666Da3f933dCc71CC9",
      refundType: "ORIGIN_CHAIN",
      deadline,
      ...overrides.quoteRequest,
    },
    signature: "ed25519:signed-quote",
    timestamp: "2026-07-27T20:00:00.250Z",
    correlationId: "quote-123",
    ...overrides,
  };
}

test("returns a normalized dry quote for canonical Base USDC to canonical NEAR USDC", async () => {
  let observed;
  const quoter = createUsdcRouteQuoter({
    now: () => fixedNow,
    providerJwt: "test-jwt",
    fetchImpl: async (url, request) => {
      observed = { url, request, body: JSON.parse(request.body) };
      return Response.json(providerFixture(), { status: 201 });
    },
  });

  const result = await quoter.quote(input);

  assert.equal(observed.url, "https://1click.chaindefuser.com/v0/quote");
  assert.equal(observed.request.method, "POST");
  assert.equal(observed.request.headers.Authorization, "Bearer test-jwt");
  assert.deepEqual(observed.body, {
    dry: true,
    swapType: "EXACT_INPUT",
    slippageTolerance: 100,
    originAsset: BASE_USDC.assetId,
    depositType: "ORIGIN_CHAIN",
    destinationAsset: NEAR_USDC.assetId,
    amount: "1000000",
    recipient: "mike.near",
    recipientType: "DESTINATION_CHAIN",
    refundTo: input.refundTo,
    refundType: "ORIGIN_CHAIN",
    deadline,
  });
  assert.deepEqual(result, {
    kind: "usdc_route_quote",
    mode: "quote_only",
    fundsMoved: false,
    source: BASE_USDC,
    destination: NEAR_USDC,
    amountInAtomic: "1000000",
    amountOutAtomic: "998898",
    minAmountOutAtomic: "988909",
    recipient: "mike.near",
    refundTo: input.refundTo,
    slippageBasisPoints: 100,
    estimatedSettlementSeconds: 35,
    providerFees: {
      refundFeeAtomic: "2400",
      withdrawFeeAtomic: "0",
    },
    quote: {
      quotedAt: "2026-07-27T20:00:00.250Z",
      expiresAt: deadline,
      correlationId: "quote-123",
      signature: "ed25519:signed-quote",
    },
    provider: {
      name: "NEAR Intents 1Click",
      endpoint: "https://1click.chaindefuser.com/v0/quote",
      status: "live",
    },
  });
});

test("validates input before contacting the route provider", async () => {
  let calls = 0;
  const quoter = createUsdcRouteQuoter({
    fetchImpl: async () => {
      calls += 1;
      return Response.json(providerFixture());
    },
  });

  for (const invalid of [
    null,
    { ...input, amountAtomic: "1.0" },
    { ...input, recipient: "Mike.NEAR" },
    { ...input, refundTo: "0x1234" },
    { ...input, slippageBasisPoints: 1001 },
    { ...input, extra: true },
  ]) {
    await assert.rejects(
      quoter.quote(invalid),
      error => error instanceof UsdcRouteQuoteError
        && error.code === "invalid_route_request"
        && error.status === 400,
    );
  }
  assert.equal(calls, 0);
});

test("fails closed when the provider response conflicts with the requested route", async () => {
  const quoter = createUsdcRouteQuoter({
    now: () => fixedNow,
    fetchImpl: async () => Response.json(providerFixture({
      quoteRequest: { destinationAsset: "nep141:not-canonical.near" },
    })),
  });

  await assert.rejects(
    quoter.quote(input),
    error => error instanceof UsdcRouteQuoteError
      && error.code === "invalid_route_quote"
      && error.status === 503,
  );
});

test("fails closed on malformed amounts and missing provider provenance", async () => {
  for (const fixture of [
    providerFixture({ quote: { amountOut: "0.998898" } }),
    providerFixture({ quote: { amountOut: "10", minAmountOut: "11" } }),
    providerFixture({ signature: "" }),
    providerFixture({ timestamp: "not-a-date" }),
  ]) {
    const quoter = createUsdcRouteQuoter({
      now: () => fixedNow,
      fetchImpl: async () => Response.json(fixture),
    });
    await assert.rejects(
      quoter.quote(input),
      error => error instanceof UsdcRouteQuoteError && error.code === "invalid_route_quote",
    );
  }
});

test("turns provider HTTP and timeout failures into retryable service errors", async () => {
  const rejected = createUsdcRouteQuoter({
    fetchImpl: async () => new Response("busy", { status: 503 }),
  });
  await assert.rejects(
    rejected.quote(input),
    error => error instanceof UsdcRouteQuoteError
      && error.code === "route_provider_unavailable"
      && error.status === 503,
  );

  const timedOut = createUsdcRouteQuoter({
    timeoutMs: 5,
    fetchImpl: async (_url, request) => new Promise((_resolve, reject) => {
      request.signal.addEventListener("abort", () => {
        const error = new Error("aborted");
        error.name = "AbortError";
        reject(error);
      });
    }),
  });
  await assert.rejects(
    timedOut.quote(input),
    error => error instanceof UsdcRouteQuoteError
      && error.code === "route_provider_unavailable"
      && error.message.includes("timed out"),
  );
});
