const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const app = fs
  .readFileSync(path.join(__dirname, "../ui/app.js"), "utf8")
  .replace(/\r\n/g, "\n");

/// The port field's saving, with the pool it is typed for settable.
function portField() {
  const start = app.indexOf("let portSaveTimer;");
  const end = app.indexOf("$(\"toggle-key\").onclick", start);
  assert.ok(start >= 0 && end > start, "the port field's code is found");
  const input = { value: "" };
  const saved = [];
  const timers = new Map();
  let nextTimer = 1;
  const scope = {
    proxyPool: "codex",
    PROXY_PREFERENCES: {
      codex: { port: "proxyPort" },
      claude: { port: "claudeProxyPort" },
    },
    $: (id) => {
      assert.equal(id, "proxy-port");
      return input;
    },
    queuePreferenceSave: async (patch) => saved.push({ ...patch }),
    notice: () => {},
    setTimeout: (run) => {
      timers.set(nextTimer, run);
      return nextTimer++;
    },
    clearTimeout: (id) => timers.delete(id),
  };
  vm.createContext(scope);
  vm.runInContext(app.slice(start, end), scope);
  const type = (value) => {
    input.value = value;
    input.oninput();
  };
  const elapse = () => {
    const runs = [...timers.values()];
    timers.clear();
    runs.forEach((run) => run());
  };
  return { scope, saved, type, elapse };
}

test("a port typed is saved as its pool's, even when the pool changes before it is saved", () => {
  const { scope, saved, type, elapse } = portField();
  type("9001");
  type("9002");
  elapse();
  assert.deepEqual(saved, [{ proxyPort: 9002 }]);
  // Switching pools saves the port typed for the pool left at once.
  type("9003");
  scope.savePortNow();
  scope.proxyPool = "claude";
  elapse();
  assert.deepEqual(saved.at(-1), { proxyPort: 9003 });
  assert.equal(saved.length, 2);
  // An invalid port is not saved, nor an earlier one still waiting.
  type("9004");
  type("70000");
  elapse();
  scope.savePortNow();
  assert.equal(saved.length, 2);
});
