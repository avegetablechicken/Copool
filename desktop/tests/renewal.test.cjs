const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { Accounts } = require("../ui/controllers.js");

// The application's pool routing with the real accounts controller.
// Windows checkouts may end lines with CRLF; the slices below look for LF.
const source = fs
  .readFileSync(path.join(__dirname, "../ui/app.js"), "utf8")
  .replace(/\r\n/g, "\n");
function fixture(target) {
  const calls = [];
  let renewal = "always";
  const list = target === "claude" ? "claude_accounts" : "accounts";
  const scope = {
    navigator: { userAgent: "Linux" },
    localStorage: { getItem: () => target },
    window: {
      __TAURI__: {
        core: {
          invoke: async (command, args) => {
            calls.push([command, args]);
            if (command.endsWith("set_renewal")) renewal = args.renewal;
            if (command === list)
              return [{ id: "a", label: "Alice", updatedAt: 1, renewal }];
            return [];
          },
        },
      },
    },
  };
  vm.createContext(scope);
  vm.runInContext(source.slice(0, source.indexOf("const ICONS =")), scope);
  return { calls, model: new Accounts(vm.runInContext(`poolInvoke("${target}")`, scope)) };
}

for (const [target, command] of [
  ["claude", "claude_set_renewal"],
  ["codex", "set_renewal"],
]) {
  test(`a ${target} renewal choice goes to its own pool and the cards reload`, async () => {
    const { model, calls } = fixture(target);
    let mutations = 0;
    model.onMutation = () => mutations++;
    for (const choice of ["never", "once", "always"]) {
      const before = calls.length;
      await model.setRenewal("a", choice);
      const sent = calls.slice(before);
      assert.deepEqual(sent[0], [command, { id: "a", renewal: choice }]);
      assert(sent.slice(1).some(([name]) => name.endsWith("accounts")), "the cards reload");
      assert.equal(model.accounts[0].renewal, choice);
    }
    assert.equal(mutations, 3);
    const other = command === "set_renewal" ? "claude_set_renewal" : "set_renewal";
    assert(!calls.some(([sent]) => sent === other));
  });
}

test("the renewal menu offers the three choices with their labels", () => {
  assert.match(source, /const RENEWALS = \["never", "once", "always"\];/);
  const locales = fs.readFileSync(path.join(__dirname, "../ui/locales.js"), "utf8");
  for (const key of ["title", "never", "once", "always"])
    assert.equal(
      locales.split(`"accounts.renewal.${key}"`).length - 1,
      11,
      `accounts.renewal.${key} in every locale`,
    );
});

test("the card footer no longer shows the token expiry; the renewal button does", () => {
  const card = source.slice(source.indexOf("function accountCard("));
  assert(!card.slice(0, card.indexOf("\nfunction ")).includes("tokenLabel("));
  const button = source.slice(source.indexOf("function renewalButton("));
  assert.match(button.slice(0, button.indexOf("\n}\n")), /trigger\.title = tokenLabel\(account\) \|\| tr\("accounts\.renewal\.title"\);/);
});


test("saying hi goes to the Claude pool, shows the card refreshing and reloads it", async () => {
  const { model, calls } = fixture("claude");
  let refreshing;
  model.onUpdate = () => (refreshing ??= model.refreshingIds.has("a"));
  assert.equal(await model.sayHi("a"), true);
  assert.equal(refreshing, true);
  assert.equal(model.refreshingIds.has("a"), false);
  const at = calls.findIndex(([name]) => name === "claude_say_hi");
  assert.deepEqual(calls[at], ["claude_say_hi", { id: "a" }]);
  assert(calls.slice(at + 1).some(([name]) => name === "claude_accounts"), "the cards reload");
  const locales = fs.readFileSync(path.join(__dirname, "../ui/locales.js"), "utf8");
  for (const key of ["accounts.card.say_hi", "accounts.card.say_hi.help", "accounts.notice.said_hi"])
    assert.equal(locales.split(`"${key}"`).length - 1, 11, `${key} in every locale`);
});

test("say hi shows only while a known 5h window has yet to start", () => {
  const start = source.indexOf("function fiveHourIdle(");
  const idle = vm.runInNewContext(
    `(${source.slice(start, source.indexOf("\n}\n", start) + 2)})`,
    { Date },
  );
  const now = Date.now() / 1000;
  assert.equal(idle({ usage: null }), false, "usage not yet known");
  assert.equal(idle({ usage: {} }), true, "no window");
  assert.equal(idle({ usage: { fiveHour: { usedPercent: 0, resetAt: null } } }), true);
  assert.equal(idle({ usage: { fiveHour: { usedPercent: 3, resetAt: now - 60 } } }), true, "ended");
  assert.equal(idle({ usage: { fiveHour: { usedPercent: 3, resetAt: now + 60 } } }), false);
  assert.match(source, /if \(pool === "claude" && !collapsed && fiveHourIdle\(account\)\)\n\s+actions\.append\(sayHiButton/);
});
