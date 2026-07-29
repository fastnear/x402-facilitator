import assert from "node:assert/strict";
import test from "node:test";

import {
  assertPublicReleaseProvenance,
  deployments,
  selectRegressionTargets,
} from "./regression-check.mjs";

test("regression target selection defaults to both production origins", () => {
  assert.deepEqual(
    selectRegressionTargets([]),
    deployments,
  );
});

test("regression target selection isolates one rolling-promotion origin", () => {
  assert.deepEqual(selectRegressionTargets(["--target", "near"]), [deployments[0]]);
  assert.deepEqual(selectRegressionTargets(["--target", "base"]), [deployments[1]]);
});

test("regression target selection rejects ambiguous or unknown arguments", () => {
  for (const argumentsList of [
    ["near"],
    ["--target"],
    ["--target", "testnet"],
    ["--target", "near", "base"],
  ]) {
    assert.throws(
      () => selectRegressionTargets(argumentsList),
      /usage: npm run regression \[-- --target near\|base\]/,
    );
  }
});

test("release provenance requires matching public metadata when installed", () => {
  const releaseId = "git-0123456789abcdef0123456789abcdef01234567";
  assertPublicReleaseProvenance({
    expectedReleaseId: releaseId,
    health: { release: { id: releaseId } },
    openApi: { info: { "x-x402-merchant-release-id": releaseId } },
  });
  assert.doesNotThrow(() => assertPublicReleaseProvenance({
    expectedReleaseId: undefined,
    health: { release: { id: "git-ffffffffffffffffffffffffffffffffffffffff" } },
    openApi: { info: { "x-x402-merchant-release-id": "git-ffffffffffffffffffffffffffffffffffffffff" } },
  }));

  assert.throws(
    () => assertPublicReleaseProvenance({
      expectedReleaseId: releaseId,
      health: { release: { id: "git-ffffffffffffffffffffffffffffffffffffffff" } },
      openApi: { info: { "x-x402-merchant-release-id": releaseId } },
    }),
    /Expected values to be strictly deep-equal/,
  );
  assert.throws(
    () => assertPublicReleaseProvenance({
      expectedReleaseId: releaseId,
      health: { release: { id: releaseId } },
      openApi: { info: {} },
    }),
    /Expected values to be strictly equal/,
  );
});
