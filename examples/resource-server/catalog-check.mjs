import { readFile } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  validateDiscoveryExtension,
  validateDiscoveryExtensionSpec,
} from "@x402/extensions/bazaar";

export function validateCatalogBazaarExtensions(manifest) {
  if (manifest?.schemaVersion !== 1 || !Array.isArray(manifest.resources)) {
    throw new Error("catalog manifest must use schemaVersion 1 and a resources array");
  }
  for (const [index, resource] of manifest.resources.entries()) {
    const extension = resource?.extensions?.bazaar;
    if (!extension || typeof extension !== "object" || Array.isArray(extension)) {
      throw new Error(`catalog resource ${index} is missing extensions.bazaar`);
    }
    const specResult = validateDiscoveryExtensionSpec(extension);
    if (!specResult.valid) {
      throw new Error(
        `catalog resource ${index} has invalid Bazaar shape: ${specResult.errors?.join(", ")}`,
      );
    }
    const result = validateDiscoveryExtension(extension);
    if (!result.valid) {
      throw new Error(
        `catalog resource ${index} has invalid Bazaar metadata: ${result.errors?.join(", ")}`,
      );
    }
  }
}

async function main() {
  const path = fileURLToPath(new URL("../../docs/catalog/resources.json", import.meta.url));
  const manifest = JSON.parse(await readFile(path, "utf8"));
  validateCatalogBazaarExtensions(manifest);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
