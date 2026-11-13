import { open } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";
import { PaymentRequiredV2Schema } from "@x402/core/schemas";
import { x402ResourceServer } from "@x402/core/server";

const MAX_FILE_BYTES = 64 * 1024;
const REQUIREMENT_FIELDS = ["scheme", "network", "asset", "amount", "payTo", "maxTimeoutSeconds", "extra"];
const RESOURCE_FIELDS = ["url", "description", "mimeType", "serviceName", "tags", "iconUrl"];
const PROFILES = Object.freeze({
  "near:testnet": { asset: "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af" },
  "near:mainnet": { asset: "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1" },
  "eip155:8453": { asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", name: "USD Coin" },
  "eip155:84532": { asset: "0x036CbD53842c5426634e7929541eC2318f3dCF7e", name: "USDC" },
});

export class PreflightError extends Error {
  constructor(code, path) {
    // Never reflect file contents, URLs, values or upstream exception messages.
    super(`${code} at ${path}`);
    this.code = code;
    this.path = path;
  }
}

function requireCondition(condition, code, path) {
  if (!condition) throw new PreflightError(code, path);
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function requireFields(value, allowed, path) {
  requireCondition(isObject(value), "object_required", path);
  requireCondition(Object.keys(value).every(key => allowed.includes(key)), "unsupported_fields", path);
}

function requireHttpsUrl(value, path) {
  let url;
  try { url = new URL(value); } catch { throw new PreflightError("https_url_required", path); }
  requireCondition(typeof value === "string" && url.protocol === "https:" && !url.username && !url.password && !url.hash,
    "https_url_required", path);
}

function checkRequirements(value, path) {
  requireFields(value, REQUIREMENT_FIELDS, path);
  for (const field of ["scheme", "network", "asset", "amount", "payTo"]) {
    requireCondition(typeof value[field] === "string" && value[field].length > 0, "string_required", `${path}.${field}`);
  }
  requireCondition(value.scheme === "exact", "unsupported_scheme", `${path}.scheme`);
  requireCondition(Object.hasOwn(PROFILES, value.network), "unsupported_network", `${path}.network`);
  const profile = PROFILES[value.network];
  const evm = value.network.startsWith("eip155:");
  requireCondition(evm ? value.asset.toLowerCase() === profile.asset.toLowerCase() : value.asset === profile.asset,
    "noncanonical_asset", `${path}.asset`);
  requireCondition(/^[0-9]+$/.test(value.amount) && BigInt(value.amount) > 0n, "positive_atomic_amount_required", `${path}.amount`);
  requireCondition(Number.isSafeInteger(value.maxTimeoutSeconds) && value.maxTimeoutSeconds > 0,
    "positive_integer_timeout_required", `${path}.maxTimeoutSeconds`);
  requireCondition(evm ? /^0x[0-9a-fA-F]{40}$/.test(value.payTo) :
    value.payTo.length >= 2 && value.payTo.length <= 64 && /^[a-z0-9]+(?:[-_.][a-z0-9]+)*$/.test(value.payTo),
    "invalid_payee_shape", `${path}.payTo`);
  if (Object.hasOwn(value, "extra")) {
    requireCondition(isObject(value.extra), "object_required", `${path}.extra`);
  }
  // This deployment accepts authorization-based NEP-366 / ERC-3009 only.
  // A newer SDK's other flows do not extend this facilitator's launch policy.
  requireCondition(value.extra?.paymentFlow === undefined || value.extra.paymentFlow === "authorization",
    "unsupported_payment_flow", `${path}.extra.paymentFlow`);
  if (evm) {
    requireCondition(value.extra?.name === profile.name && value.extra?.version === "2",
      "noncanonical_usdc_domain", `${path}.extra`);
  } else {
    requireCondition(!Object.hasOwn(value.extra ?? {}, "assetTransferMethod"),
      "unsupported_near_transfer_selector", `${path}.extra.assetTransferMethod`);
  }
}

/** Check a captured unsigned offer against independently reviewed exact terms. */
export function inspectMerchantOffer(challenge, expected) {
  requireFields(expected, ["resourceUrl", "requirements", "offerIndex"], "expected");
  requireHttpsUrl(expected.resourceUrl, "expected.resourceUrl");
  checkRequirements(expected.requirements, "expected.requirements");
  requireFields(challenge, ["x402Version", "error", "resource", "accepts", "extensions"], "challenge");
  requireCondition(PaymentRequiredV2Schema.safeParse(challenge).success, "invalid_x402_v2_offer", "challenge");
  requireFields(challenge.resource, RESOURCE_FIELDS, "resource");
  requireHttpsUrl(challenge.resource.url, "resource.url");
  for (const field of ["description", "mimeType", "serviceName", "iconUrl", "tags"]) {
    requireCondition(challenge.resource[field] !== null, "null_resource_field", `resource.${field}`);
  }
  if (Object.hasOwn(challenge, "extensions")) {
    requireCondition(isObject(challenge.extensions), "object_required", "extensions");
  }
  requireCondition(challenge.resource.url === expected.resourceUrl, "resource_mismatch", "resource.url");
  if (Object.hasOwn(expected, "offerIndex")) {
    requireCondition(Number.isSafeInteger(expected.offerIndex) && expected.offerIndex >= 0 && expected.offerIndex < challenge.accepts.length,
      "invalid_offer_index", "expected.offerIndex");
  }
  const candidates = challenge.accepts.map((offer, index) => ({ offer, index }))
    .filter(({ offer, index }) => offer.network === expected.requirements.network && offer.scheme === "exact" &&
      (expected.offerIndex === undefined || index === expected.offerIndex));
  requireCondition(candidates.length === 1, candidates.length ? "ambiguous_offer" : "missing_offer", "accepts");
  const { offer, index } = candidates[0];
  checkRequirements(offer, `accepts[${index}]`);
  // Inspect original objects. The official schema can strip unknown keys;
  // using its parsed result would conceal fields rejected by protocol.rs.
  requireCondition(isDeepStrictEqual(offer, expected.requirements), "terms_mismatch", `accepts[${index}]`);
  const offlineOnly = {
    async getSupported() { throw new Error("network disabled"); },
    async verify() { throw new Error("network disabled"); },
    async settle() { throw new Error("network disabled"); },
  };
  const server = new x402ResourceServer(offlineOnly);
  const match = server.findMatchingRequirements([offer], {
    x402Version: 2, accepted: expected.requirements, payload: {},
  });
  requireCondition(match === offer, "upstream_matcher_rejected", `accepts[${index}]`);
  return { ok: true, scope: "unsigned_offer_only", network: offer.network, offerIndex: index };
}

async function readBoundedJson(path) {
  // Bound reads even when the input is a pipe, changes size, or lacks a useful stat.
  const file = await open(path, "r");
  try {
    const bytes = Buffer.alloc(MAX_FILE_BYTES + 1);
    let length = 0;
    while (length < bytes.length) {
      const { bytesRead } = await file.read(bytes, length, bytes.length - length, null);
      if (!bytesRead) break;
      length += bytesRead;
    }
    requireCondition(length <= MAX_FILE_BYTES, "input_too_large", "input");
    return JSON.parse(bytes.subarray(0, length).toString("utf8"));
  } finally { await file.close(); }
}

export async function runPreflight(args, output = process.stdout, errors = process.stderr) {
  try {
    requireCondition(args.length === 2, "usage_challenge_json_expected_json", "arguments");
    const challenge = await readBoundedJson(args[0]);
    const expected = await readBoundedJson(args[1]);
    output.write(`${JSON.stringify(inspectMerchantOffer(challenge, expected))}\n`);
    return 0;
  } catch (error) {
    const report = error instanceof PreflightError ? { code: error.code, path: error.path } :
      { code: "invalid_or_unreadable_json", path: "input" };
    errors.write(`${JSON.stringify({ ok: false, ...report })}\n`);
    return 1;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await runPreflight(process.argv.slice(2));
}
