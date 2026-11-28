const test = require("node:test");
const assert = require("node:assert/strict");
const { PendingSave } = require("../ui/controllers.js");

/// A PendingSave on timers that run only when the test elapses them.
function pending() {
  const timers = new Map();
  let next = 1;
  const save = new PendingSave(350, {
    setTimeout: (run, delay) => {
      assert.equal(delay, 350);
      timers.set(next, run);
      return next++;
    },
    clearTimeout: (id) => timers.delete(id),
  });
  const elapse = async () => {
    const runs = [...timers.values()];
    timers.clear();
    await Promise.all(runs.map((run) => run()));
  };
  return { save, elapse };
}

test("a port typed is saved as its pool's, even when the pool changes before it is saved", async () => {
  // As app.js's port field: each valid port typed replaces the one waiting.
  const { save, elapse } = pending();
  const saved = [];
  const type = (pool, port) => {
    save.cancel();
    if (!Number.isInteger(port) || port < 1 || port > 65535) return;
    save.schedule(pool, () => saved.push({ [pool]: port }));
  };
  type("codex", 9001);
  type("codex", 9002);
  await elapse();
  assert.deepEqual(saved, [{ codex: 9002 }]);
  // Switching pools saves the port typed for the pool left at once.
  type("codex", 9003);
  await save.flush();
  await elapse();
  assert.deepEqual(saved.at(-1), { codex: 9003 });
  assert.equal(saved.length, 2);
  // An invalid port is not saved, nor an earlier one still waiting.
  type("claude", 9004);
  type("claude", 70000);
  await elapse();
  await save.flush();
  assert.equal(saved.length, 2);
});

test("a hostname typed is saved as its pool's, even when another pool is shown by then", async () => {
  // As features.js's tunnel: each pool's controller owns its save.
  const { save, elapse } = pending();
  const saved = [];
  const schedule = (pool, hostname) => save.schedule(pool, async () => saved.push([pool, hostname]));
  schedule("codex", "codex.example.com");
  await elapse();
  assert.deepEqual(saved, [["codex", "codex.example.com"]]);
  // Typed in one pool, then another, before either is saved: both are.
  schedule("claude", "claude.example.com");
  schedule("codex", "codex2.example.com");
  await elapse();
  assert.deepEqual(saved.slice(1), [
    ["claude", "claude.example.com"],
    ["codex", "codex2.example.com"],
  ]);
  // Saved at once, a waiting save of the same pool is not made again; one of
  // another pool is made first.
  schedule("codex", "codex3.example.com");
  await save.now("codex", async () => saved.push(["codex", "now"]));
  await elapse();
  assert.deepEqual(saved.slice(3), [["codex", "now"]]);
  schedule("claude", "claude2.example.com");
  await save.now("codex", async () => saved.push(["codex", "now again"]));
  assert.deepEqual(saved.slice(4), [["claude", "claude2.example.com"], ["codex", "now again"]]);
});
