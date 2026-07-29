import {
  closeSync,
  constants as fsConstants,
  fstatSync,
  openSync,
  readFileSync,
} from "node:fs";

import { parseAllowedOrigins } from "./cors.mjs";
import {
  isAtomicUsdcAmount,
  isEvmAddress,
  isNearAccountId,
} from "./evidence-input.mjs";

const MAX_CREDENTIAL_BYTES = 4096;

export const NETWORK_PROFILES = Object.freeze({
  "near:mainnet": Object.freeze({
    asset:
      "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1",
    chainId: "mainnet",
  }),
  "near:testnet": Object.freeze({
    asset:
      "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af",
    chainId: "testnet",
  }),
  "eip155:8453": Object.freeze({
    asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    chainId: "8453",
    eip712Name: "USD Coin",
    eip712Version: "2",
  }),
  "eip155:84532": Object.freeze({
    asset: "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
    chainId: "84532",
    eip712Name: "USDC",
    eip712Version: "2",
  }),
});

export function loadConfig(
  environment = process.env,
  { credentialReader = readCredential } = {},
) {
  const network = required(environment, "NETWORK");
  const profile = NETWORK_PROFILES[network];
  if (!profile) {
    throw new Error(
      "NETWORK must be near:mainnet, near:testnet, eip155:8453, or eip155:84532",
    );
  }

  const asset = required(environment, "ASSET");
  if (asset !== profile.asset) {
    throw new Error(`ASSET must be canonical Circle USDC for ${network}`);
  }

  const payTo = required(environment, "PAY_TO");
  if (
    network.startsWith("near:")
      ? !isNearAccountId(payTo)
      : !isEvmAddress(payTo)
  ) {
    throw new Error(`PAY_TO is invalid for ${network}`);
  }

  const configuredName = environment.ASSET_EIP712_NAME;
  const configuredVersion = environment.ASSET_EIP712_VERSION;
  if (
    configuredName !== undefined
    && configuredName !== profile.eip712Name
  ) {
    throw new Error(`ASSET_EIP712_NAME does not match canonical USDC on ${network}`);
  }
  if (
    configuredVersion !== undefined
    && configuredVersion !== profile.eip712Version
  ) {
    throw new Error(
      `ASSET_EIP712_VERSION does not match canonical USDC on ${network}`,
    );
  }
  if (
    network.startsWith("near:")
    && (configuredName !== undefined || configuredVersion !== undefined)
  ) {
    throw new Error("ASSET_EIP712_NAME/VERSION are only valid for EVM networks");
  }

  const amount = environment.AMOUNT ?? "1000";
  if (!isAtomicUsdcAmount(amount)) {
    throw new Error("AMOUNT must be a positive atomic USDC integer of at most 16 digits");
  }

  const portText = environment.PORT ?? "4031";
  if (!/^[1-9][0-9]{0,4}$/.test(portText)) {
    throw new Error("PORT must be an integer from 1 through 65535");
  }
  const port = Number(portText);
  if (port > 65535) {
    throw new Error("PORT must be an integer from 1 through 65535");
  }

  const facilitatorUrl = httpsUrl(
    required(environment, "FACILITATOR_URL"),
    "FACILITATOR_URL",
    { originOnly: true },
  );
  const rpcUrl = httpsUrl(required(environment, "RPC_URL"), "RPC_URL");
  const resourceOrigin = httpsUrl(
    required(environment, "RESOURCE_ORIGIN"),
    "RESOURCE_ORIGIN",
    { originOnly: true },
  );
  const oneClickProviderOrigin = httpsUrl(
    environment.ONE_CLICK_PROVIDER_ORIGIN
      ?? "https://1click.chaindefuser.com",
    "ONE_CLICK_PROVIDER_ORIGIN",
    { originOnly: true },
  );
  const explorerBaseUrl = environment.EXPLORER_BASE_URL === undefined
    ? undefined
    : httpsUrl(environment.EXPLORER_BASE_URL, "EXPLORER_BASE_URL", {
      allowQuery: false,
    });

  const apiKey = credentialReader(
    required(environment, "FACILITATOR_API_KEY_FILE"),
    "facilitator API key",
  );
  const oneClickJwt = environment.ONE_CLICK_JWT_FILE
    ? credentialReader(environment.ONE_CLICK_JWT_FILE, "1Click JWT")
    : undefined;

  return {
    network,
    chainId: profile.chainId,
    facilitatorUrl,
    apiKey,
    rpcUrl,
    resourceOrigin,
    asset,
    payTo,
    amount,
    priceUsd: formatUsdc(amount),
    port,
    explorerBaseUrl,
    eip712Name: profile.eip712Name,
    eip712Version: profile.eip712Version,
    corsOrigins: parseAllowedOrigins(environment.CORS_ORIGINS),
    oneClickProviderOrigin,
    oneClickJwt,
    activityIndexFile: environment.ACTIVITY_INDEX_FILE,
    contactEmail: environment.CONTACT_EMAIL,
  };
}

export function formatUsdc(amountAtomic) {
  if (!isAtomicUsdcAmount(amountAtomic)) {
    throw new Error("amountAtomic must be a positive atomic USDC integer");
  }
  const padded = amountAtomic.padStart(7, "0");
  return `${padded.slice(0, -6)}.${padded.slice(-6)}`;
}

export function readCredential(path, label = "credential") {
  let descriptor;
  try {
    descriptor = openSync(
      path,
      fsConstants.O_RDONLY | (fsConstants.O_NOFOLLOW ?? 0),
    );
    const metadata = fstatSync(descriptor);
    if (!metadata.isFile()) {
      throw new Error(`${label} file must be a regular file`);
    }
    if ((metadata.mode & 0o077) !== 0) {
      throw new Error(`${label} file must not be accessible by group or others`);
    }
    if (metadata.size < 1 || metadata.size > MAX_CREDENTIAL_BYTES) {
      throw new Error(
        `${label} file must contain from 1 through ${MAX_CREDENTIAL_BYTES} bytes`,
      );
    }
    const contents = readFileSync(descriptor, "utf8");
    if (
      contents.includes("\0")
      || !/^[^\r\n]+(?:\n)?$/.test(contents)
    ) {
      throw new Error(`${label} file must contain exactly one nonempty line`);
    }
    const value = contents.endsWith("\n") ? contents.slice(0, -1) : contents;
    if (value !== value.trim()) {
      throw new Error(`${label} file must not contain surrounding whitespace`);
    }
    return value;
  } catch (error) {
    if (error?.code === "ELOOP") {
      throw new Error(`${label} file must not be a symbolic link`);
    }
    throw error;
  } finally {
    if (descriptor !== undefined) closeSync(descriptor);
  }
}

function required(environment, name) {
  const value = environment[name];
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${name} is required`);
  }
  return value;
}

function httpsUrl(value, name, { originOnly = false, allowQuery = true } = {}) {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new Error(`${name} must be a valid HTTPS URL`);
  }
  if (
    url.protocol !== "https:"
    || url.username
    || url.password
    || url.hash
    || (!allowQuery && url.search)
  ) {
    throw new Error(
      `${name} must use HTTPS without credentials or a fragment${
        allowQuery ? "" : " or a query"
      }`,
    );
  }
  if (
    originOnly
    && (url.pathname !== "/" || url.search)
  ) {
    throw new Error(`${name} must be an HTTPS origin without a path or query`);
  }
  return originOnly ? url.origin : url.href.replace(/\/$/, "");
}
