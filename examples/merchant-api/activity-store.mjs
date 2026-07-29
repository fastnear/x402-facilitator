import { readFile } from "node:fs/promises";

export const ACTIVITY_SEARCH_INPUT_SCHEMA = Object.freeze({
  type: "object",
  additionalProperties: false,
  properties: {
    query: { type: "string" },
    account: { type: "string" },
    contract: { type: "string" },
    limit: { type: "integer", minimum: 1, maximum: 100 },
    cursor: { type: "string" },
  },
});

export const ENTITY_IDENTIFIER_SCHEMA = Object.freeze({
  type: "string",
  minLength: 1,
  maxLength: 256,
});

export class ActivityStore {
  constructor(records = []) {
    const ids = new Set();
    this.records = records.map(record => {
      const normalized = normalizeRecord(record);
      if (ids.has(normalized.id)) throw new Error(`activity record id is duplicated: ${normalized.id}`);
      ids.add(normalized.id);
      return normalized;
    });
  }

  static async fromFile(path) {
    if (!path) return new ActivityStore();
    const parsed = JSON.parse(await readFile(path, "utf8"));
    if (!Array.isArray(parsed)) throw new Error("ACTIVITY_INDEX_FILE must contain a JSON array");
    return new ActivityStore(parsed);
  }

  search(input = {}) {
    const validated = validateActivitySearchInput(input);
    const { query, account, contract, limit, cursor } = validated;
    const normalizedQuery = typeof query === "string" ? query.toLowerCase() : undefined;
    const offset = decodeCursor(cursor);
    const filtered = this.records.filter(record => {
      if (account && record.account !== account) return false;
      if (contract && record.contract !== contract) return false;
      if (!normalizedQuery) return true;
      return JSON.stringify(record).toLowerCase().includes(normalizedQuery);
    });
    const page = filtered.slice(offset, offset + Math.min(limit, 100));
    const nextOffset = offset + page.length;
    return {
      items: page,
      nextCursor: nextOffset < filtered.length ? encodeCursor(nextOffset) : null,
      index: {
        status: this.records.length === 0 ? "not_yet_indexed" : "ready",
        recordCount: this.records.length,
        indexedAt: this.records.length === 0 ? null : this.records[0].indexedAt ?? null,
      },
    };
  }

  entity(identifier) {
    const validated = validateEntityIdentifier(identifier);
    const match = this.records.filter(record =>
      record.account === validated || record.contract === validated || record.entity === validated,
    );
    return {
      identifier: validated,
      status: match.length === 0 ? "not_yet_indexed" : "indexed",
      records: match.slice(0, 100),
      index: {
        status: match.length === 0 ? "not_yet_indexed" : "ready",
        recordCount: this.records.length,
        indexedAt: this.records.length === 0 ? null : this.records[0].indexedAt ?? null,
      },
    };
  }
}

export function validateActivitySearchInput(input = {}) {
  if (!input || typeof input !== "object" || Array.isArray(input)) {
    throw invalidInput("request body must be an object");
  }
  const allowed = new Set(Object.keys(ACTIVITY_SEARCH_INPUT_SCHEMA.properties));
  for (const key of Object.keys(input)) {
    if (!allowed.has(key)) throw invalidInput(`unexpected request field: ${key}`);
  }
  const { query, account, contract, limit = 25, cursor } = input;
  if (query !== undefined && typeof query !== "string") throw invalidInput("query must be a string");
  if (account !== undefined && typeof account !== "string") throw invalidInput("account must be a string");
  if (contract !== undefined && typeof contract !== "string") throw invalidInput("contract must be a string");
  if (!Number.isInteger(limit) || limit < 1 || limit > 100) throw invalidInput("limit must be an integer from 1 to 100");
  if (cursor !== undefined && typeof cursor !== "string") throw invalidInput("cursor must be a string");
  return {
    query,
    account,
    contract,
    limit,
    cursor,
  };
}

export function validateEntityIdentifier(identifier) {
  if (
    typeof identifier !== "string"
    || identifier.length < ENTITY_IDENTIFIER_SCHEMA.minLength
    || identifier.length > ENTITY_IDENTIFIER_SCHEMA.maxLength
  ) {
    throw invalidInput("identifier must be a nonempty string of at most 256 characters");
  }
  return identifier;
}

function invalidInput(message) {
  const error = new Error(message);
  error.status = 400;
  return error;
}

function normalizeRecord(record) {
  if (!record || typeof record !== "object" || typeof record.id !== "string") {
    throw new Error("activity records require an id");
  }
  return {
    id: record.id,
    network: record.network,
    kind: record.kind ?? "activity",
    account: record.account,
    contract: record.contract,
    entity: record.entity,
    block: record.block,
    timestamp: record.timestamp,
    summary: record.summary,
    indexedAt: record.indexedAt,
  };
}

function encodeCursor(offset) {
  return Buffer.from(JSON.stringify({ offset }), "utf8").toString("base64url");
}

function decodeCursor(cursor) {
  if (!cursor) return 0;
  try {
    const decoded = JSON.parse(Buffer.from(cursor, "base64url").toString("utf8"));
    if (!Number.isInteger(decoded.offset) || decoded.offset < 0) throw new Error();
    return decoded.offset;
  } catch {
    const error = new Error("cursor is invalid");
    error.status = 400;
    throw error;
  }
}
