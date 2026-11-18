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
  const scope = {
    navigator: { userAgent: "Linux" },
    localStorage: { getItem: () => target },
    window: {
      __TAURI__: {
        core: {
          invoke: async (command, args) => {
            // Arguments built inside the app's context, as plain data.
            calls.push([command, JSON.parse(JSON.stringify(args))]);
            if (command === "set_renewal") renewal = args.renewal;
            if (command === "accounts" && args.pool === target)
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

// Every command of a pool's cards names that pool, and only that pool.
const onlyTo = (calls, target) => calls.every(([, args]) => args.pool === target);
for (const target of ["claude", "codex"]) {
  test(`a ${target} renewal choice goes to its own pool and the cards reload`, async () => {
    const { model, calls } = fixture(target);
    let mutations = 0;
    model.onMutation = () => mutations++;
    for (const choice of ["never", "once", "always"]) {
      const before = calls.length;
      await model.setRenewal("a", choice);
      const sent = calls.slice(before);
      assert.deepEqual(sent[0], ["set_renewal", { id: "a", renewal: choice, pool: target }]);
      assert(sent.slice(1).some(([name]) => name === "accounts"), "the cards reload");
      assert.equal(model.accounts[0].renewal, choice);
    }
    assert.equal(mutations, 3);
    assert(onlyTo(calls, target));
  });
}

test("the renewal menu offers renew now, then auto renew as a toggle", () => {
  const start = source.indexOf("function renewalButton(");
  assert(start >= 0, "renewalButton found");
  const end = source.indexOf("\n}\n", start);
  assert(end > start, "renewalButton end found");
  const body = source.slice(start, end);
  const now = body.indexOf('tr("accounts.renewal.now")');
  const auto = body.indexOf('tr("accounts.renewal.auto")');
  assert(now >= 0 && auto > now, "renew now comes first, then auto renew");
  assert.match(body, /run: \(\) => renewNow\(account\)/);
  assert.match(body, /checked: account\.renewal === "always",/);
  assert.match(body, /account\.renewal === "always" \? "never" : "always"/);
  assert(!source.includes("RENEWALS"));
  const locales = fs.readFileSync(path.join(__dirname, "../ui/locales.js"), "utf8");
  for (const key of [
    "accounts.renewal.title",
    "accounts.renewal.now",
    "accounts.renewal.renewing",
    "accounts.renewal.auto",
    "accounts.notice.token_renewed",
  ])
    assert.equal(locales.split(`"${key}"`).length - 1, 11, `${key} in every locale`);
  for (const key of ["once", "never", "always"])
    assert(!locales.includes(`"accounts.renewal.${key}"`), `accounts.renewal.${key} removed`);
});

for (const target of ["claude", "codex"]) {
  test(`renewing a ${target} account now shows as under way, then reloads its card`, async () => {
    const { model, calls } = fixture(target);
    let during;
    model.onUpdate = () => (during ??= model.renewingIds.has("a"));
    assert.equal(await model.renewNow("a"), true);
    assert.equal(during, true);
    assert.equal(model.renewingIds.has("a"), false);
    const at = calls.findIndex(([name]) => name === "renew_now");
    assert.deepEqual(calls[at], ["renew_now", { id: "a", pool: target }]);
    assert(calls.slice(at + 1).some(([name]) => name === "accounts"), "the card reloads");
    assert(onlyTo(calls, target));
    assert.equal(model.refreshing, false);
  });
}

test("the card footer no longer shows the token expiry; the renewal button does", () => {
  const card = source.slice(source.indexOf("function accountCard("));
  assert(!card.slice(0, card.indexOf("\nfunction ")).includes("tokenLabel("));
  const button = source.slice(source.indexOf("function renewalButton("));
  assert.match(button.slice(0, button.indexOf("\n}\n")), /trigger\.title = renewing\n\s+\? tr\("accounts\.renewal\.renewing"\)\n\s+: tokenLabel\(account\) \|\| tr\("accounts\.renewal\.title"\);/);
});


test("saying hi goes to the Claude pool, locks the card's refresh without spinning it and reloads it", async () => {
  const { model, calls } = fixture("claude");
  let locked;
  model.onUpdate = () => (locked ??= [model.sayingHiIds.has("a"), model.refreshingIds.has("a")]);
  assert.equal(await model.sayHi("a"), true);
  assert.deepEqual(locked, [true, false]);
  assert.equal(model.sayingHiIds.has("a"), false);
  const at = calls.findIndex(([name]) => name === "say_hi");
  assert.deepEqual(calls[at], ["say_hi", { id: "a", pool: "claude" }]);
  assert(calls.slice(at + 1).some(([name]) => name === "accounts"), "the cards reload");
  assert(onlyTo(calls, "claude"));
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

test("say hi now requires an idle window while scheduling stays available", () => {
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
  assert.match(source, /if \(!collapsed && \(sayingHi\.has\(account\.id\) \|\| supportsSayHi\(account\)\)\)\n\s+actions\.append\(sayHiButton/);
});

test("both pools refresh immediately after a reply, then only until the window shows", async () => {
  const start = source.indexOf("const SAY_HI_LOOKS =");
  const end = source.indexOf("/// Says hi with the account; shown");
  // [refreshes before the window shows, or the second another refresh shows it]
  for (const target of ["claude", "codex"]) {
    for (const [shownAfter, shownAt, expected] of [
      [0, Infinity, { refreshes: 1, slept: 0, notices: [["accounts.notice.said_hi", false]] }],
      [2, Infinity, { refreshes: 2, slept: 10000, notices: [["accounts.notice.said_hi", false]] }],
      [Infinity, 15, { refreshes: 2, slept: 15000, notices: [["accounts.notice.said_hi", false]] }],
      [Infinity, Infinity, { refreshes: 5, slept: 180000, notices: [["error.claude.say_hi_window_not_started", true]] }],
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
        onUpdate: () => phases.push(scope.pending.get("a")),
        sayHi: async () => (phases.push(scope.pending.get("a")), show(), true),
        refresh: async () => {
          assert.equal(scope.pending.get("a"), "received", "refresh follows model reply");
          if (refreshes === 0) assert.equal(clock, replied, "first refresh has no delay");
          refreshes++;
          show();
          return true;
        },
      };
      const scope = {
        Date: { now: () => clock },
        Math,
        Set,
        Map,
        Promise,
        accountsController,
        pool: target,
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
      assert.deepEqual(notices, expected.notices.map(([key, error]) => [key.replace("error.claude.", `error.${target}.`), error]));
      assert.equal(scope.pending.size, 0);
      assert.equal(phases[0], "sent");
      assert(phases.slice(1).every((phase) => phase === "received"));
    }
  }
});


test("Codex Plus greetings route to their own account command", async () => {
  const { model, calls } = fixture("codex");
  assert.equal(await model.sayHi("a"), true);
  assert(calls.some(([name, args]) => name === "say_hi" && args.id === "a"));
  assert(onlyTo(calls, "codex"));
});

test("Codex greetings are limited to idle official Plus cards", () => {
  const start = source.indexOf("function canSayHi(");
  const end = source.indexOf("/// Both pools", start);
  const scope = { pool: "codex", Date };
  vm.createContext(scope);
  vm.runInContext(source.slice(start, end), scope);
  const card = { planType: "plus", usage: {} };
  assert.equal(scope.canSayHi(card), true);
  assert.equal(scope.canSayHi({ ...card, usage: null }), false);
  assert.equal(scope.canSayHi({ ...card, sourceTag: "SUB2API" }), false);
  for (const planType of ["pro", "free", "team", ""]) {
    assert.equal(scope.canSayHi({ ...card, planType }), false);
  }
  assert.equal(scope.canSayHi({ ...card, usage: { planType: "pro" } }), false);
  assert.equal(scope.canSayHi({ ...card, usage: { fiveHour: { resetAt: Date.now() / 1000 + 100 } } }), false);
  scope.pool = "claude";
  assert.equal(scope.canSayHi({ ...card, planType: "max" }), true);
});
