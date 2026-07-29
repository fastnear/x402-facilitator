export const PATTERNS = Object.freeze({
  nearAccountId: "^(?=.{2,64}$)[a-z0-9]+(?:[._-][a-z0-9]+)*$",
  // OpenAPI can express the base58 alphabet and its possible rendered
  // lengths; `isNearCryptoHash` below also verifies the decoded 32-byte
  // length. The all-zero account code hash is rendered as 32 `1`s.
  nearCryptoHash: "^[1-9A-HJ-NP-Za-km-z]{32,44}$",
  nearTransactionHash: "^[1-9A-HJ-NP-Za-km-z]{43,44}$",
  evmAddress: "^0x[0-9a-fA-F]{40}$",
  evmTransactionHash: "^0x[0-9a-fA-F]{64}$",
  atomicUsdcAmount: "^[1-9][0-9]{0,15}$",
});

const nearAccountIdPattern = new RegExp(PATTERNS.nearAccountId);
const nearCryptoHashPattern = new RegExp(PATTERNS.nearCryptoHash);
const nearTransactionHashPattern = new RegExp(PATTERNS.nearTransactionHash);
const evmAddressPattern = new RegExp(PATTERNS.evmAddress);
const evmTransactionHashPattern = new RegExp(PATTERNS.evmTransactionHash);
const atomicUsdcAmountPattern = new RegExp(PATTERNS.atomicUsdcAmount);

export function isNearAccountId(value) {
  return typeof value === "string" && nearAccountIdPattern.test(value);
}

export function isNearCryptoHash(value) {
  if (typeof value !== "string" || !nearCryptoHashPattern.test(value)) {
    return false;
  }

  // A NEAR CryptoHash is the base58 rendering of exactly 32 bytes. Leading
  // zero bytes are represented by `1`, so length alone is insufficient.
  let leadingZeroBytes = 0;
  while (value[leadingZeroBytes] === "1") leadingZeroBytes += 1;

  let decoded = 0n;
  const alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
  for (let index = leadingZeroBytes; index < value.length; index += 1) {
    decoded = (decoded * 58n) + BigInt(alphabet.indexOf(value[index]));
  }

  let decodedBytes = 0;
  while (decoded > 0n) {
    decoded >>= 8n;
    decodedBytes += 1;
  }
  return leadingZeroBytes + decodedBytes === 32;
}

export function isNearTransactionHash(value) {
  return typeof value === "string" && nearTransactionHashPattern.test(value);
}

export function isEvmAddress(value) {
  return typeof value === "string" && evmAddressPattern.test(value);
}

export function isEvmTransactionHash(value) {
  return typeof value === "string" && evmTransactionHashPattern.test(value);
}

export function isAtomicUsdcAmount(value) {
  return typeof value === "string" && atomicUsdcAmountPattern.test(value);
}

export function evidenceInputSchema(network, kind) {
  const near = network.startsWith("near:");
  if (kind === "account") {
    return {
      type: "object",
      additionalProperties: false,
      required: [near ? "accountId" : "address"],
      properties: near
        ? {
          accountId: {
            type: "string",
            minLength: 2,
            maxLength: 64,
            pattern: PATTERNS.nearAccountId,
          },
        }
        : {
          address: { type: "string", pattern: PATTERNS.evmAddress },
        },
    };
  }
  if (kind === "transaction") {
    return {
      type: "object",
      additionalProperties: false,
      required: near ? ["transactionHash", "signerId"] : ["transactionHash"],
      properties: near
        ? {
          transactionHash: {
            type: "string",
            minLength: 43,
            maxLength: 44,
            pattern: PATTERNS.nearTransactionHash,
          },
          signerId: {
            type: "string",
            minLength: 2,
            maxLength: 64,
            pattern: PATTERNS.nearAccountId,
          },
        }
        : {
          transactionHash: {
            type: "string",
            pattern: PATTERNS.evmTransactionHash,
          },
        },
    };
  }
  throw new Error(`unsupported evidence input kind: ${kind}`);
}

export function validateEvidenceInput(network, kind, value, invalid = defaultInvalid) {
  const schema = evidenceInputSchema(network, kind);
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw invalid("request body must be an object");
  }

  const fields = Object.keys(schema.properties);
  for (const key of Object.keys(value)) {
    if (!fields.includes(key)) {
      throw invalid(`unexpected request field: ${key}`);
    }
  }
  for (const field of schema.required) {
    if (!(field in value)) {
      throw invalid(`${field} is required`);
    }
  }
  for (const [field, fieldSchema] of Object.entries(schema.properties)) {
    const candidate = value[field];
    if (typeof candidate !== "string") {
      throw invalid(`${field} must be a string`);
    }
    if (candidate.length < (fieldSchema.minLength ?? 0)) {
      throw invalid(`${field} is too short`);
    }
    if (fieldSchema.maxLength !== undefined && candidate.length > fieldSchema.maxLength) {
      throw invalid(`${field} is too long`);
    }
    if (!new RegExp(fieldSchema.pattern).test(candidate)) {
      throw invalid(`${field} has an invalid shape`);
    }
  }
  return value;
}

export function nearExampleAccountId(network) {
  if (network === "near:mainnet") return "alice.near";
  if (network === "near:testnet") return "alice.testnet";
  throw new Error(`network is not a supported NEAR network: ${network}`);
}

export const NEAR_TRANSACTION_HASH_EXAMPLE =
  "11111111111111111111111111111111111111111111";

function defaultInvalid(message) {
  return new TypeError(message);
}
