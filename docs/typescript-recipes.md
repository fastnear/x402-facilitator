# TypeScript integration recipes

The reference resource-server package includes two tested TypeScript recipes
using the repository's pinned official x402 packages directly. They are
examples, not a new wrapper SDK.

## Browse the public catalog

[`discover.ts`](../examples/resource-server/typescript/discover.ts) wraps an
official `HTTPFacilitatorClient` with `withBazaar` and calls
`extensions.bazaar.listResources()`. Catalog reads are public and must not carry
an API key.

```ts
import { listDiscoveryResources } from "./discover.js";

const catalog = await listDiscoveryResources(
  "https://base.x402.mikedotexe.com",
  { network: "eip155:8453", scheme: "exact", extensions: "bazaar" },
);
```

## Configure a resource server

[`facilitator-client.ts`](../examples/resource-server/typescript/facilitator-client.ts)
constructs the official HTTP client with an API key scoped to `/verify` and
`/settle`. Load the value on the server from a secret manager or a mode-0600
credential file. Never put it in browser, payer, issue, log, or source code.

```ts
import { createAuthenticatedFacilitatorClient } from "./facilitator-client.js";

const facilitator = createAuthenticatedFacilitatorClient({
  facilitatorUrl: "https://base.x402.mikedotexe.com",
  apiKey: serverSideSecret,
});
```

Base mainnet resource requirements must use `eip155:8453`, canonical Circle
USDC `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`, and the token's real EIP-712
domain `USD Coin` / `2`. Give every deployed resource-server instance and
environment a separate key. The complete Express example retains the required
bounded retries and independent delivery-idempotency journal.

Run the type checks and offline integration tests with:

```sh
npm --prefix examples/resource-server ci
npm --prefix examples/resource-server run check
```
