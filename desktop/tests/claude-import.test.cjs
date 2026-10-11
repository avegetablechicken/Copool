const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { Accounts } = require("../ui/controllers.js");

// Run the application's pool routing with the real accounts controller.
const source = fs.readFileSync(path.join(__dirname, "../ui/app.js"), "utf8");
function fixture(target, cancelled = false) {
  const calls = [];
  let rows = [];
  const scope = {
    navigator: { userAgent: "Linux" },
    localStorage: { getItem: () => target },
    window: {
      __TAURI__: {
        core: {
          invoke: async (command) => {
            calls.push(command);
            if (command.endsWith("import_account_file")) {
              if (cancelled) return null;
              rows = [{ id: "imported", label: "Imported account", updatedAt: 1 }];
              return 1;
            }
            if (command === "accounts" || command === "claude_accounts") return rows;
            return [];
          },
        },
      },
    },
  };
  vm.createContext(scope);
  vm.runInContext(source.slice(0, source.indexOf("const ICONS =")), scope);
  const invoke = vm.runInContext(`poolInvoke("${target}")`, scope);
  return { calls, model: new Accounts(invoke) };
}
for (const target of ["claude", "codex"]) {
  test(`${target} file import uses its own pool and reloads imported accounts`, async () => {
    const { model, calls } = fixture(target);
    assert.deepEqual(await model.importFile(), ["Imported account"]);
    assert(calls.includes(target === "claude" ? "claude_import_account_file" : "import_account_file"));
    assert(!calls.includes(target === "claude" ? "import_account_file" : "claude_import_account_file"));
  });
}
test("cancelling a Claude file import does not reload or mutate accounts", async () => {
  const { model, calls } = fixture("claude", true);
  assert.equal(await model.importFile(), null);
  assert.deepEqual(calls, ["claude_import_account_file"]);
  assert.equal(model.importing, false);
});
