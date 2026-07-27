export const BASE_USDC = Object.freeze({
  network: "eip155:8453",
  asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
  assetId: "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near",
  symbol: "USDC",
  decimals: 6,
});

export const NEAR_USDC = Object.freeze({
  network: "near:mainnet",
  asset: "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1",
  assetId: "nep141:17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1",
  symbol: "USDC",
  decimals: 6,
});

const DEFAULT_PROVIDER_ORIGIN = "https://1click.chaindefuser.com";
const DEFAULT_SLIPPAGE_BASIS_POINTS = 100;
const QUOTE_LIFETIME_MS = 5 * 60 * 1000;
const PROVIDER_TIMEOUT_MS = 12 * 1000;
const NEAR_ACCOUNT_PATTERN = /^(?=.{2,64}$)[a-z0-9]+(?:[._-][a-z0-9]+)*$/;
const BASE_ADDRESS_PATTERN = /^0x[0-9a-fA-F]{40}$/;
const ATOMIC_AMOUNT_PATTERN = /^[1-9][0-9]{0,15}$/;

export class UsdcRouteQuoteError extends Error {
  constructor(code, message, status = 503) {
    super(message);
    this.name = "UsdcRouteQuoteError";
    this.code = code;
    this.status = status;
  }
}

export function createUsdcRouteQuoter({
  fetchImpl = globalThis.fetch,
  now = () => Date.now(),
  providerOrigin = DEFAULT_PROVIDER_ORIGIN,
  providerJwt,
  timeoutMs = PROVIDER_TIMEOUT_MS,
} = {}) {
  if (typeof fetchImpl !== "function") throw new Error("fetchImpl must be a function");
  const quoteUrl = `${providerOrigin.replace(/\/$/, "")}/v0/quote`;

  return {
    async quote(input) {
      const normalized = normalizeInput(input);
      const deadline = new Date(now() + QUOTE_LIFETIME_MS).toISOString();
      const providerRequest = {
        dry: true,
        swapType: "EXACT_INPUT",
        slippageTolerance: normalized.slippageBasisPoints,
        originAsset: BASE_USDC.assetId,
        depositType: "ORIGIN_CHAIN",
        destinationAsset: NEAR_USDC.assetId,
        amount: normalized.amountAtomic,
        recipient: normalized.recipient,
        recipientType: "DESTINATION_CHAIN",
        refundTo: normalized.refundTo,
        refundType: "ORIGIN_CHAIN",
        deadline,
      };

      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), timeoutMs);
      let response;
      try {
        response = await fetchImpl(quoteUrl, {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            ...(providerJwt ? { Authorization: `Bearer ${providerJwt}` } : {}),
          },
          body: JSON.stringify(providerRequest),
          signal: controller.signal,
        });
      } catch (error) {
        const message = error?.name === "AbortError"
          ? "The route provider timed out"
          : "The route provider could not be reached";
        throw new UsdcRouteQuoteError("route_provider_unavailable", message);
      } finally {
        clearTimeout(timeout);
      }

      if (!response.ok) {
        throw new UsdcRouteQuoteError(
          "route_provider_unavailable",
          `The route provider returned HTTP ${response.status}`,
        );
      }

      let providerResponse;
      try {
        providerResponse = await response.json();
      } catch {
        throw new UsdcRouteQuoteError(
          "invalid_route_quote",
          "The route provider returned invalid JSON",
        );
      }

      return normalizeProviderQuote(providerResponse, providerRequest, quoteUrl);
    },
  };
}

function normalizeInput(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw invalidInput("request body must be an object");
  }

  const allowed = new Set(["amountAtomic", "recipient", "refundTo", "slippageBasisPoints"]);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) throw invalidInput(`unexpected request field: ${key}`);
  }

  if (typeof value.amountAtomic !== "string" || !ATOMIC_AMOUNT_PATTERN.test(value.amountAtomic)) {
    throw invalidInput("amountAtomic must be a positive USDC atomic-unit string of at most 16 digits");
  }
  if (typeof value.recipient !== "string" || !NEAR_ACCOUNT_PATTERN.test(value.recipient)) {
    throw invalidInput("recipient must be a valid lowercase NEAR account id");
  }
  if (typeof value.refundTo !== "string" || !BASE_ADDRESS_PATTERN.test(value.refundTo)) {
    throw invalidInput("refundTo must be a 20-byte Base address");
  }

  const slippageBasisPoints = value.slippageBasisPoints ?? DEFAULT_SLIPPAGE_BASIS_POINTS;
  if (!Number.isInteger(slippageBasisPoints) || slippageBasisPoints < 0 || slippageBasisPoints > 1000) {
    throw invalidInput("slippageBasisPoints must be an integer from 0 through 1000");
  }

  return {
    amountAtomic: value.amountAtomic,
    recipient: value.recipient,
    refundTo: value.refundTo,
    slippageBasisPoints,
  };
}

function normalizeProviderQuote(value, expectedRequest, quoteUrl) {
  const quote = value?.quote;
  const request = value?.quoteRequest;
  if (!quote || !request || typeof value.signature !== "string" || !value.signature) {
    throw invalidQuote("The route provider response omitted signed quote metadata");
  }

  const exactFields = [
    ["dry", true],
    ["swapType", "EXACT_INPUT"],
    ["originAsset", BASE_USDC.assetId],
    ["depositType", "ORIGIN_CHAIN"],
    ["destinationAsset", NEAR_USDC.assetId],
    ["amount", expectedRequest.amount],
    ["recipient", expectedRequest.recipient],
    ["recipientType", "DESTINATION_CHAIN"],
    ["refundTo", expectedRequest.refundTo],
    ["refundType", "ORIGIN_CHAIN"],
    ["deadline", expectedRequest.deadline],
    ["slippageTolerance", expectedRequest.slippageTolerance],
  ];
  for (const [field, expected] of exactFields) {
    if (request[field] !== expected) {
      throw invalidQuote(`The route provider response conflicted on ${field}`);
    }
  }

  for (const field of ["amountIn", "amountOut", "minAmountOut", "refundFee", "withdrawFee"]) {
    if (typeof quote[field] !== "string" || !/^\d+$/.test(quote[field])) {
      throw invalidQuote(`The route provider response omitted ${field}`);
    }
  }
  if (quote.amountIn !== expectedRequest.amount) {
    throw invalidQuote("The route provider response conflicted on amountIn");
  }
  if (BigInt(quote.minAmountOut) > BigInt(quote.amountOut)) {
    throw invalidQuote("The route provider returned an invalid minimum output");
  }
  if (!Number.isInteger(quote.timeEstimate) || quote.timeEstimate < 0) {
    throw invalidQuote("The route provider returned an invalid time estimate");
  }
  if (
    typeof value.timestamp !== "string"
    || Number.isNaN(Date.parse(value.timestamp))
    || typeof value.correlationId !== "string"
    || !value.correlationId
  ) {
    throw invalidQuote("The route provider response omitted provenance");
  }

  return {
    kind: "usdc_route_quote",
    mode: "quote_only",
    fundsMoved: false,
    source: BASE_USDC,
    destination: NEAR_USDC,
    amountInAtomic: quote.amountIn,
    amountOutAtomic: quote.amountOut,
    minAmountOutAtomic: quote.minAmountOut,
    recipient: request.recipient,
    refundTo: request.refundTo,
    slippageBasisPoints: request.slippageTolerance,
    estimatedSettlementSeconds: quote.timeEstimate,
    providerFees: {
      refundFeeAtomic: quote.refundFee,
      withdrawFeeAtomic: quote.withdrawFee,
    },
    quote: {
      quotedAt: value.timestamp,
      expiresAt: request.deadline,
      correlationId: value.correlationId,
      signature: value.signature,
    },
    provider: {
      name: "NEAR Intents 1Click",
      endpoint: quoteUrl,
      status: "live",
    },
  };
}

function invalidInput(message) {
  return new UsdcRouteQuoteError("invalid_route_request", message, 400);
}

function invalidQuote(message) {
  return new UsdcRouteQuoteError("invalid_route_quote", message);
}
