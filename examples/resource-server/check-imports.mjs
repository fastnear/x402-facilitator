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

import {
  V1_NETWORK_NAMES,
  buildUnpaidHintBody,
  buildV1PaymentRequiredBody,
  buildV1Requirements,
  caip2ForV1Network,
  v1NetworkName,
} from "./legacy-v1.mjs";

for (const [name, value] of Object.entries({
  HTTPFacilitatorClient,
  paymentMiddlewareFromHTTPServer,
  x402HTTPResourceServer,
  x402ResourceServer,
  declarePaymentIdentifierExtension,
  extractPaymentIdentifier,
  ExactEvmScheme,
  ExactNearScheme,
  buildUnpaidHintBody,
  buildV1PaymentRequiredBody,
  buildV1Requirements,
  caip2ForV1Network,
  v1NetworkName,
})) {
  if (typeof value !== "function") {
    throw new Error(`${name} is not exported as a function`);
  }
}
if (PAYMENT_IDENTIFIER !== "payment-identifier") {
  throw new Error("unexpected payment-identifier extension key");
}
if (V1_NETWORK_NAMES["eip155:8453"] !== "base") {
  throw new Error("unexpected v1 network name for Base mainnet");
}
