const ALLOWED_METHODS = "GET, POST, OPTIONS";
const ALLOWED_HEADERS = "Content-Type, PAYMENT-SIGNATURE, X-PAYMENT";
const EXPOSED_HEADERS = "PAYMENT-REQUIRED, PAYMENT-RESPONSE, X-PAYMENT-RESPONSE";

export function parseAllowedOrigins(value) {
  if (!value) return [];
  const origins = value.split(",").map(origin => origin.trim()).filter(Boolean);
  const unique = new Set();
  for (const origin of origins) {
    if (origin === "*") throw new Error("CORS_ORIGINS must contain exact origins, not *");
    let parsed;
    try {
      parsed = new URL(origin);
    } catch {
      throw new Error(`CORS_ORIGINS contains an invalid origin: ${origin}`);
    }
    if (!["http:", "https:"].includes(parsed.protocol) || parsed.origin !== origin) {
      throw new Error(`CORS_ORIGINS must contain origins without paths: ${origin}`);
    }
    unique.add(origin);
  }
  return [...unique];
}

export function createCorsMiddleware(allowedOrigins) {
  const allowed = new Set(allowedOrigins);
  return (request, response, next) => {
    const origin = request.get?.("origin") ?? request.headers?.origin;
    if (!origin) return next();
    if (!allowed.has(origin)) {
      if (request.method === "OPTIONS") {
        return response.status(403).json({
          error: "cors_origin_denied",
          message: "This browser origin is not allowed",
        });
      }
      return next();
    }

    response.set({
      "Access-Control-Allow-Origin": origin,
      "Access-Control-Allow-Methods": ALLOWED_METHODS,
      "Access-Control-Allow-Headers": ALLOWED_HEADERS,
      "Access-Control-Expose-Headers": EXPOSED_HEADERS,
      "Access-Control-Max-Age": "600",
    });
    response.vary("Origin");
    if (request.method === "OPTIONS") return response.status(204).end();
    return next();
  };
}
