const { test } = require("node:test");
const assert = require("node:assert/strict");
const { Accounts, poolInvoke } = require("../ui/controllers.js");

// The application's pool routing with the real accounts controller.
function fixture(target, cancelled = false) {
  const calls = [];
  let rows = [];
  const invoke = async (command, args) => {
    calls.push([command, args.pool]);
    if (command === "import_account_file") {
      if (cancelled) return null;
      rows = [{ id: "imported", label: "Imported account", updatedAt: 1 }];
      return 1;
    }
    if (command === "accounts") return rows;
    return [];
  };
  return { calls, model: new Accounts(poolInvoke(invoke, target)) };
}
for (const target of ["claude", "codex"]) {
  test(`${target} file import uses its own pool and reloads imported accounts`, async () => {
    const { model, calls } = fixture(target);
    assert.deepEqual(await model.importFile(), ["Imported account"]);
    assert.deepEqual(calls[0], ["import_account_file", target]);
    // Every command, the reload included, goes to that pool alone.
    assert(calls.every(([, sent]) => sent === target));
  });
}
test("cancelling a Claude file import does not reload or mutate accounts", async () => {
  const { model, calls } = fixture("claude", true);
  assert.equal(await model.importFile(), null);
  assert.deepEqual(calls, [["import_account_file", "claude"]]);
  assert.equal(model.importing, false);
});
