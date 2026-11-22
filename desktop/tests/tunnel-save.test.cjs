const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const features = fs
  .readFileSync(path.join(__dirname, "../ui/features.js"), "utf8")
  .replace(/\r\n/g, "\n");

/// The tunnel's saving, with the pool shown settable.
function tunnelSaving() {
  const start = features.indexOf("async function saveTunnelConfiguration()");
  const end = features.indexOf("async function refreshFeatureState()", start);
  assert.ok(start >= 0 && end > start, "the tunnel's saving is found");
  const saved = [];
  const controller = (pool) => ({
    tunnelConfiguration: { namedHostname: "" },
    updateTunnel: async (config) => saved.push([pool, config.namedHostname]),
  });
  const controllers = { codex: controller("codex"), claude: controller("claude") };
  const timers = new Map();
  let nextTimer = 1;
  const scope = {
    action: async (task) => (await task(), true),
    setTimeout: (run) => {
      timers.set(nextTimer, run);
      return nextTimer++;
    },
    clearTimeout: (id) => timers.delete(id),
  };
  const show = (pool) => {
    scope.proxyController = controllers[pool];
    scope.tunnelConfiguration = controllers[pool].tunnelConfiguration;
  };
  show("codex");
  vm.createContext(scope);
  vm.runInContext(features.slice(start, end), scope);
  const elapse = async () => {
    const runs = [...timers.values()];
    timers.clear();
    await Promise.all(runs.map((run) => run()));
  };
  return { scope, saved, show, elapse };
}

test("a hostname typed is saved as its pool's, even when another pool is shown by then", async () => {
  const { scope, saved, show, elapse } = tunnelSaving();
  scope.tunnelConfiguration.namedHostname = "codex.example.com";
  scope.scheduleTunnelSave();
  show("claude");
  await elapse();
  assert.deepEqual(saved, [["codex", "codex.example.com"]]);
  // Typed in one pool, then another, before either is saved: both are.
  scope.tunnelConfiguration.namedHostname = "claude.example.com";
  scope.scheduleTunnelSave();
  show("codex");
  scope.tunnelConfiguration.namedHostname = "codex2.example.com";
  scope.scheduleTunnelSave();
  await elapse();
  assert.deepEqual(saved.slice(1), [
    ["claude", "claude.example.com"],
    ["codex", "codex2.example.com"],
  ]);
  // Saved at once, a waiting save of the same pool is not made again.
  scope.scheduleTunnelSave();
  await scope.saveTunnelConfiguration();
  await elapse();
  assert.equal(saved.length, 4);
});
