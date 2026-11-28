const test = require("node:test");
const assert = require("node:assert/strict");
const { PendingSave } = require("../ui/controllers.js");
const P = require("../ui/presentation.js");

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
  return { save, elapse, timers };
}

test("a port typed is saved as its pool's, even when the pool changes before it is saved", async () => {
  // As app.js's port field: each port typed replaces the one waiting, its
  // preference named for the pool shown as it was typed.
  const { save, elapse } = pending();
  const saved = [];
  let shown = "codex";
  const type = (value) => {
    save.cancel();
    const patch = P.portPatch(shown, value);
    if (patch) save.schedule(shown, () => saved.push(patch));
  };
  type("9001");
  type("9002");
  await elapse();
  assert.deepEqual(saved, [{ proxyPort: 9002 }]);
  // Switching pools saves the port typed for the pool left at once.
  type("9003");
  shown = "claude";
  await save.flush();
  await elapse();
  assert.deepEqual(saved, [{ proxyPort: 9002 }, { proxyPort: 9003 }]);
  // An invalid port is not saved, nor an earlier one still waiting.
  type("9004");
  type("70000");
  await elapse();
  await save.flush();
  assert.equal(saved.length, 2);
});

test("a port is a preference of its own pool's proxy, and only a port", () => {
  assert.deepEqual(P.portPatch("codex", "8787"), { proxyPort: 8787 });
  assert.deepEqual(P.portPatch("claude", "8788"), { claudeProxyPort: 8788 });
  assert.deepEqual(P.portPatch("antigravity", " 8789 "), { antigravityProxyPort: 8789 });
  for (const value of ["", "0", "65536", "70000", "8.5", "port", "-1"]) {
    assert.equal(P.portPatch("codex", value), null, value);
  }
  assert.deepEqual(P.portPatch("codex", "65535"), { proxyPort: 65535 });
  // Grok has no proxy, so no port.
  assert.equal(P.portPatch("grok", "8790"), null);
});

test("a hostname typed is saved as its pool's, even when another pool is shown by then", async () => {
  // As features.js's tunnel: each pool's controller owns its save.
  const { save, elapse, timers } = pending();
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
  // The other pool's save finishes before this one is made.
  let finish;
  const slow = new Promise((resolve) => (finish = resolve));
  save.schedule("claude", async () => {
    await slow;
    saved.push(["claude", "claude2.example.com"]);
  });
  const now = save.now("codex", async () => saved.push(["codex", "now again"]));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(saved.length, 4);
  finish();
  await now;
  assert.deepEqual(saved.slice(4), [["claude", "claude2.example.com"], ["codex", "now again"]]);
  // Nothing is left waiting to be saved again.
  assert.equal(timers.size, 0);
  await elapse();
  assert.equal(saved.length, 6);
  // A hostname typed while another pool's is being saved is saved in turn.
  let finishAgain;
  const slowAgain = new Promise((resolve) => (finishAgain = resolve));
  save.schedule("claude", async () => {
    await slowAgain;
    saved.push(["claude", "claude3.example.com"]);
  });
  const nowAgain = save.now("codex", async () => saved.push(["codex", "now once more"]));
  schedule("antigravity", "antigravity.example.com");
  finishAgain();
  await nowAgain;
  await elapse();
  assert.deepEqual(saved.slice(6), [
    ["claude", "claude3.example.com"],
    ["codex", "now once more"],
    ["antigravity", "antigravity.example.com"],
  ]);
});
