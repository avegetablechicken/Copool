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


test("saying hi goes to the Claude pool, locks the card's refresh without spinning it and reloads it", async () => {
  const { model, calls } = fixture("claude");
  let locked;
  model.onUpdate = () => (locked ??= [model.sayingHiIds.has("a"), model.refreshingIds.has("a")]);
  assert.equal(await model.sayHi("a"), true);
  assert.deepEqual(locked, [true, false]);
  assert.equal(model.sayingHiIds.has("a"), false);
  const at = calls.findIndex(([name]) => name === "claude_say_hi");
  assert.deepEqual(calls[at], ["claude_say_hi", { id: "a" }]);
  assert(calls.slice(at + 1).some(([name]) => name === "claude_accounts"), "the cards reload");
  const locales = fs.readFileSync(path.join(__dirname, "../ui/locales.js"), "utf8");
  for (const key of [
    "accounts.card.say_hi",
    "accounts.card.say_hi.help",
    "accounts.card.say_hi.sent",
    "accounts.card.say_hi.received",
    "accounts.notice.said_hi",
  ])
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

test("after a hi the card is refreshed again only until its 5h window shows", async () => {
  const start = source.indexOf("const SAY_HI_LOOKS =");
  const end = source.indexOf("/// Says hi with the account; shown");
  // [refreshes before the window shows, or the second another refresh shows it]
  for (const [shownAfter, shownAt, expected] of [
    [0, Infinity, { refreshes: 0, slept: 0, notices: [["accounts.notice.said_hi", false]] }],
    [2, Infinity, { refreshes: 2, slept: 30000, notices: [["accounts.notice.said_hi", false]] }],
    [Infinity, 15, { refreshes: 1, slept: 15000, notices: [["accounts.notice.said_hi", false]] }],
    [Infinity, Infinity, { refreshes: 4, slept: 180000, notices: [["error.claude.say_hi_window_not_started", true]] }],
  ]) {
    const notices = [];
    const phases = [];
    let refreshes = 0;
    let clock = Date.now();
    const replied = clock;
    const card = { id: "a", usage: { fiveHour: { resetAt: null } } };
    const show = () => {
      if (refreshes >= shownAfter || clock - replied >= shownAt * 1000)
        card.usage.fiveHour.resetAt = clock / 1000 + 600;
    };
    const accountsController = {
      accounts: [card],
      sayHi: async () => (phases.push(scope.pending.get("a")), show(), true),
      refresh: async () => (refreshes++, show(), true),
    };
    const scope = {
      Date: { now: () => clock },
      Math,
      Set,
      Map,
      Promise,
      accountsController,
      refreshView: async () => phases.push(scope.pending.get("a")),
      setTimeout: (resolve, ms) => ((clock += ms), show(), resolve()),
      tr: (key) => key,
      notice: (message, error = false) => notices.push([message, error]),
    };
    vm.createContext(scope);
    vm.runInContext(`${source.slice(start, end)}\nthis.run = sayHi; this.pending = sayingHi;`, scope);
    await scope.run({ id: "a" });
    assert.equal(refreshes, expected.refreshes);
    assert.equal(clock - replied, expected.slept);
    assert.deepEqual(notices, expected.notices);
    assert.equal(scope.pending.size, 0);
    assert.equal(phases[0], "sent");
    assert(phases.slice(1).every((phase) => phase === "received"));
  }
});
