const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { Accounts, poolInvoke } = require("../ui/controllers.js");
const CardBack = require("../ui/card-back.js");
const dom = require("./fake-dom.cjs");

// The application's pool routing with the real accounts controller.
/// A clock that moves only as the controller waits.
function fakeClock() {
  const clock = { at: Date.now() };
  clock.now = () => clock.at;
  clock.sleep = (ms) => {
    clock.at += ms;
    return Promise.resolve();
  };
  return clock;
}
// A 5h window already running, as after a hi.
const RUNNING = { fiveHour: { usedPercent: 1, resetAt: 4102444800 } };
function fixture(target, usage = RUNNING, clock = fakeClock()) {
  const calls = [];
  let renewal = "always";
  const invoke = async (command, args) => {
    calls.push([command, structuredClone(args)]);
    if (command === "set_renewal") renewal = args.renewal;
    if (command === "accounts" && args.pool === target)
      return [{ id: "a", label: "Alice", updatedAt: 1, renewal, usage }];
    return [];
  };
  const model = new Accounts(poolInvoke(invoke, target), null, () => {}, () => {}, { pool: target, clock });
  return { calls, model };
}

// Every command of a pool's cards names that pool, and only that pool.
const onlyTo = (calls, target) => calls.every(([, args]) => args.pool === target);
for (const target of ["claude", "codex"]) {
  test(`a ${target} renewal choice goes to its own pool and the cards reload`, async () => {
    const { model, calls } = fixture(target);
    let mutations = 0;
    model.onMutation = () => mutations++;
    for (const choice of ["never", "always"]) {
      const before = calls.length;
      await model.setRenewal("a", choice);
      const sent = calls.slice(before);
      assert.deepEqual(sent[0], ["set_renewal", { id: "a", renewal: choice, pool: target }]);
      assert(sent.slice(1).some(([name]) => name === "accounts"), "the cards reload");
      assert.equal(model.accounts[0].renewal, choice);
    }
    assert.equal(mutations, 2);
    assert(onlyTo(calls, target));
  });
}

/// A card's renewal section, on a back whose renewals and actions are
/// recorded, its card's token expiring at `expiresAt` (Unix seconds).
function renewalSection(renewal, expiresAt = Date.now() / 1000 + 3 * 3600) {
  const calls = [];
  const controller = {
    cardState: () => ({ refreshing: false, hi: null, renewing: false }),
    setRenewal: (id, choice) => calls.push(["setRenewal", id, choice]),
  };
  const back = CardBack.create({
    el: dom.el,
    button: dom.button,
    tr: (key, ...args) => [key, ...args].join(" "),
    locale: "en",
    controller,
    renewNow: (account) => calls.push(["renewNow", account.id]),
    action: (task) => task(),
  });
  const account = { id: "a", renewal, tokenExpiresAt: expiresAt };
  return { section: back.renewalSection(account), calls };
}

test("a card's back offers renew now, then auto renew as a switch", () => {
  const { section, calls } = renewalSection("always");
  const now = section.find((n) => n.tag === "button" && n.textContent === "accounts.renewal.now");
  const auto = section.field("renewal-auto");
  assert(now && auto, "renew now and auto renew shown");
  const order = [...section.nodes()];
  assert(order.indexOf(now) < order.indexOf(auto), "renew now comes first, then auto renew");
  assert.equal(auto.parent.children[0].textContent, "accounts.renewal.auto");
  assert.equal(auto.checked, true);
  assert.equal(renewalSection("never").section.field("renewal-auto").checked, false);
  now.click();
  auto.checked = false;
  auto.onchange();
  auto.checked = true;
  auto.onchange();
  assert.deepEqual(calls, [["renewNow", "a"], ["setRenewal", "a", "never"], ["setRenewal", "a", "always"]]);
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
  test(`renewing a ${target} account now shows as under way, then done for a moment, and reloads its card`, async () => {
    // The moment it shows as done ends when the test says.
    let shownFor;
    let ended;
    const clock = fakeClock();
    clock.sleep = (ms) => new Promise((resolve) => ((shownFor = ms), (ended = resolve)));
    const { model, calls } = fixture(target, RUNNING, clock);
    let during;
    model.onUpdate = () => (during ??= model.cardState("a").renewing);
    assert.equal(await model.renewNow("a"), true);
    assert.equal(during, true);
    assert.equal(model.cardState("a").renewing, false);
    assert.equal(model.cardState("a").renewed, true);
    assert.equal(shownFor, 2200);
    let updated = false;
    model.onUpdate = () => (updated = true);
    ended();
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(model.cardState("a").renewed, false);
    assert(updated, "the card renders without the mark");
    const at = calls.findIndex(([name]) => name === "renew_now");
    assert.deepEqual(calls[at], ["renew_now", { id: "a", pool: target }]);
    assert(calls.slice(at + 1).some(([name]) => name === "accounts"), "the card reloads");
    assert(onlyTo(calls, target));
    assert.equal(model.refreshing, false);
  });
}

test("a renewal's done moment starts once its card reloads, and an earlier one's end leaves a later one's", async () => {
  // Each moment shown ends when the test says; reloads finish when it says.
  const ends = [];
  const clock = fakeClock();
  clock.sleep = () => new Promise((resolve) => ends.push(resolve));
  let reload = null;
  const invoke = async (command) => {
    if (command === "accounts") {
      await new Promise((resolve) => (reload = resolve));
      return [{ id: "a", usage: RUNNING }];
    }
    return [];
  };
  const model = new Accounts(invoke, [{ id: "a", usage: RUNNING }], () => {}, () => {}, { clock });
  const settle = () => new Promise((resolve) => setImmediate(resolve));
  const renew = async () => {
    const started = ends.length;
    const renewal = model.renewNow("a");
    await settle();
    // Still reloading: under way, and its moment has not started.
    assert.equal(model.cardState("a").renewing, true);
    assert.equal(ends.length, started);
    reload();
    assert.equal(await renewal, true);
    assert.equal(ends.length, started + 1, "the moment starts after the reload");
    assert.deepEqual(
      [model.cardState("a").renewing, model.cardState("a").renewed],
      [false, true],
    );
  };
  await renew();
  await renew();
  ends[0]();
  await settle();
  assert.equal(model.cardState("a").renewed, true, "the first moment's end keeps the second's");
  ends[1]();
  await settle();
  assert.equal(model.cardState("a").renewed, false);
});

test("a failed renewal shows no done moment", async () => {
  const clock = fakeClock();
  let slept = 0;
  clock.sleep = () => (slept++, new Promise(() => {}));
  const invoke = async (command) => {
    if (command === "renew_now") throw new Error("expired");
    return command === "accounts" ? [{ id: "a", usage: RUNNING }] : [];
  };
  const model = new Accounts(invoke, [{ id: "a", usage: RUNNING }], () => {}, () => {}, { clock });
  await assert.rejects(model.renewNow("a"), /expired/);
  assert.deepEqual(
    [model.cardState("a").renewing, model.cardState("a").renewed, slept],
    [false, false, 0],
  );
});

// The card's front no longer shows the token expiry (card-view.test.cjs).
test("the card's back shows the token expiry", () => {
  const detail = (expiresAt) =>
    renewalSection("always", expiresAt).section.find((n) => n.className === "back-detail").textContent;
  assert.match(detail(Date.now() / 1000 + 3 * 3600), /^accounts\.renewal\.expires_format /);
  assert.equal(detail(Date.now() / 1000 - 60), "accounts.renewal.expired");
});

test("saying hi goes to the Claude pool, locks the card's refresh without spinning it and reloads it", async () => {
  const { model, calls } = fixture("claude");
  let locked;
  model.onUpdate = () =>
    (locked ??= [model.cardState("a").hi, model.busy("a"), model.cardState("a").refreshing]);
  assert.equal(await model.sayHi("a"), "started");
  assert.deepEqual(locked, ["sent", true, false]);
  assert.equal(model.cardState("a").hi, null);
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
    "accounts.card.antigravity_say_hi.help",
    "accounts.card.antigravity_say_hi.sent",
    "accounts.card.antigravity_say_hi.received",
    "error.antigravity.say_hi_window_not_started",
  ])
    assert.equal(locales.split(`"${key}"`).length - 1, 11, `${key} in every locale`);
});

/// A pool's controller saying hi with `card`, whose windows the commands it
/// sends and its waits may start (`show`), on a clock that moves only as it
/// waits. Counts the refreshes it asks for.
function greeter(pool, card, show = () => {}) {
  const clock = fakeClock();
  const sleep = clock.sleep;
  clock.sleep = (ms) => sleep(ms).then(() => show(counts, clock));
  const counts = { refreshes: 0, sent: [] };
  const invoke = async (command, args) => {
    if (command === "accounts") return [card];
    if (command === "say_hi") {
      counts.sent.push(args);
      show(counts, clock);
    }
    if (command === "refresh_usage") {
      assert.equal(model.cardState(card.id).hi, "received", "refresh follows model reply");
      counts.refreshes++;
      show(counts, clock);
    }
    return [];
  };
  const model = new Accounts(invoke, [card], () => {}, () => {}, { pool, clock });
  return { model, clock, counts };
}

test("both pools refresh immediately after a reply, then only until the window shows", async () => {
  // [refreshes before the window shows, or the second another refresh shows it]
  for (const target of ["claude", "codex", "antigravity"]) {
    for (const [shownAfter, shownAt, expected] of [
      [0, Infinity, { refreshes: 1, slept: 0, outcome: "started" }],
      [2, Infinity, { refreshes: 2, slept: 10000, outcome: "started" }],
      [Infinity, 15, { refreshes: 2, slept: 15000, outcome: "started" }],
      [Infinity, Infinity, { refreshes: 5, slept: 180000, outcome: "not_started" }],
    ]) {
      // Antigravity's card has both pools' windows; the hi shows once both run.
      const card = { id: "a", usage: { fiveHour: { resetAt: null } } };
      if (target === "antigravity") card.usage.thirdPartyFiveHour = { resetAt: null };
      let replied;
      const { model, clock, counts } = greeter(target, card, ({ refreshes }, { at }) => {
        if (refreshes >= shownAfter || (replied !== undefined && at - replied >= shownAt * 1000))
          for (const window of Object.values(card.usage)) window.resetAt = at / 1000 + 600;
      });
      const phases = [];
      model.onUpdate = () => {
        const hi = model.cardState("a").hi;
        if (hi === "received") replied ??= clock.at;
        if (phases.at(-1) !== hi) phases.push(hi);
      };
      assert.equal(await model.sayHi("a"), expected.outcome);
      assert.equal(counts.refreshes, expected.refreshes);
      assert.equal(clock.at - replied, expected.slept);
      assert.deepEqual(phases, ["sent", "received", null]);
    }
  }
});

test("an Antigravity hi waits for both windows while another pool is shown", async () => {
  // Gemini's window starts at once; the third-party one at the third refresh.
  const card = { id: "a", usage: { fiveHour: { resetAt: null }, thirdPartyFiveHour: { resetAt: null } } };
  const { model, counts } = greeter("antigravity", card, ({ refreshes, sent }, { at }) => {
    if (sent.length) card.usage.fiveHour.resetAt = at / 1000 + 600;
    if (refreshes >= 3) card.usage.thirdPartyFiveHour.resetAt ??= at / 1000 + 600;
  });
  assert.equal(await model.sayHi("a"), "started");
  assert.equal(counts.refreshes, 3);
});

test("A card's model switch sends its hi to the chosen pool alone, and waits for its window", async () => {
  for (const target of [true, false]) {
    // Both pools report; only the chosen pool's window starts, at the first refresh.
    const window = () => ({ usedPercent: 0, resetAt: null });
    const card = { id: "a", usage: { fiveHour: window(), thirdPartyFiveHour: window() } };
    const { model, counts } = greeter("antigravity", card, ({ refreshes }, { at }) => {
      if (refreshes) card.usage[target ? "thirdPartyFiveHour" : "fiveHour"].resetAt = at / 1000 + 600;
    });
    model.showThirdParty("a", target);
    assert.equal(await model.sayHi("a"), "started");
    assert.deepEqual(counts.sent, [{ id: "a", thirdParty: target }]);
    assert.equal(counts.refreshes, 1, "the other pool's idle window is not awaited");
  }
});

test("A chosen pool's hi goes to say_hi with it", async () => {
  const window = { usedPercent: 0, resetAt: 4102444800 };
  const { model, calls } = fixture("antigravity", { fiveHour: window, thirdPartyFiveHour: window });
  await model.load();
  model.showThirdParty("a", true);
  assert.equal(await model.sayHi("a"), "started");
  assert.deepEqual(calls.find(([name]) => name === "say_hi"), ["say_hi", { id: "a", thirdParty: true, pool: "antigravity" }]);
  // Its window alone decides idleness (card-view.test.cjs).
});

test("A hi under way is not sent again, and a busy card's is not sent", async () => {
  const { model, calls } = fixture("claude");
  await model.load();
  const first = model.sayHi("a");
  assert.equal(await model.sayHi("a"), false);
  assert.equal(await first, "started");
  model.renewingIds.add("a");
  assert.equal(await model.sayHi("a"), false);
  assert.equal(calls.filter(([name]) => name === "say_hi").length, 1);
});

test("Codex Plus greetings route to their own account command", async () => {
  const { model, calls } = fixture("codex");
  assert.equal(await model.sayHi("a"), "started");
  assert(calls.some(([name, args]) => name === "say_hi" && args.id === "a"));
  assert(onlyTo(calls, "codex"));
});
