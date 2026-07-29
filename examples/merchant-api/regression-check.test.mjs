import assert from "node:assert/strict";
import test from "node:test";

import {
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
