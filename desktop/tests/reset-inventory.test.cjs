const { test } = require("node:test");
const assert = require("node:assert/strict");
const P = require("../ui/presentation.js");
const grant = (kind, count, expiresAt = 300) => ({ kind, count, expiresAt });

test("Full and Fable resets share one count and warn for non-full grants", () => {
  const result = P.resetInventory({ remainingResetCount: 3,
    resetGrants: [grant("full", 1), grant("fable", 2)] }, 100);
  assert.equal(result.count, 3);
  assert.equal(result.warning, true);
  assert.equal(result.fable, true);
  assert.equal(P.resetInventory({ resetGrants: [grant("full", 2)] }, 100).warning, false);
  for (const kind of ["partial", "unknown", "futureKind"])
    assert.equal(P.resetInventory({ resetGrants: [grant(kind, 1)] }, 100).warning, true);
});

test("Expired or spent partial resets do not warn or contribute to the sum", () => {
  const usage = { remainingResetCount: 5,
    resetGrants: [grant("full", 1), grant("fable", 3, 100), grant("partial", 0)] };
  assert.deepEqual(P.resetInventory(usage, 100), {
    count: 1, grants: [grant("full", 1)], warning: false, fable: false,
  });
  assert.equal(P.resetInventory(usage, 300).count, 0);
  assert.equal(P.resetInventory({resetGrants: [grant("fable", 1, null)]}, 100).warning, true);
});

test("Older caches and Codex keep their existing counts", () => {
  assert.equal(P.resetInventory({remainingResetCount: 4}).count, 4);
  assert.equal(P.resetInventory({remainingResetCount: 0}).count, 0);
  assert.equal(P.resetInventory({remainingResetCount: -1}).count, null);
  assert.equal(P.resetInventory().count, null);
  assert.equal(P.resetInventory({remainingResetCount: 4}).warning, false);
});
