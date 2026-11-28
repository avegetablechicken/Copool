const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const P = require("../ui/presentation.js");
const V = require("../ui/card-view.js");
const CardBack = require("../ui/card-back.js");
const Keys = require("../ui/keys.js");
const { Accounts } = require("../ui/controllers.js");
const dom = require("./fake-dom.cjs");

/// One of the Sub2api provider "team"'s cards.
const member = (n, extra = {}) => ({
  id: `sub2api-team-${n}`,
  sourceTag: "SUB2API",
  proxyUrl: "",
  refresh: { key: "sub2api:team" },
  ...extra,
});

/// A Codex pool's controller over `accounts`, whose proxy saves are recorded
/// (or fail with `scope.failSave`), and card-back.js over it (`back`), whose
/// host is `scope` and whose card turns are run at once.
function editor(accounts, extra = {}) {
  const saved = [];
  const invoke = async (command, args) => {
    if (command === "accounts") return accounts;
    if (command === "save_account_proxy") {
      if (scope.failSave) throw new Error(scope.failSave);
      saved.push([args.id, args.value]);
    }
    return [];
  };
  const controller = new Accounts(invoke, accounts, () => {}, () => {}, { pool: "codex" });
  const scope = host({ controller, ...extra });
  return { scope, back: CardBack.create(scope), controller, saved };
}
/// A card back's host, as app.js gives it: fake nodes, a Codex card in
/// English, and recorded or idle actions, which `extra` replaces.
function host(extra = {}) {
  return {
    el: dom.el,
    button: dom.button,
    tr: (key, ...args) => [key, ...args].join(" "),
    locale: "en",
    pool: "codex",
    collapsed: false,
    preferences: { accountProxyUrl: "" },
    cardContext: () => ({ pool: "codex", locale: "en", tr: (key) => key }),
    providerTitle: () => "Team",
    turnCard: (_key, _direction, change) => change(),
    renderEditing: () => {},
    focusBackField: () => {},
    notice: () => {},
    action: (task) => task(),
    queuePreferenceSave: async () => {},
    renewNow: () => {},
    greet: () => {},
    removeCards: () => {},
    ...extra,
  };
}
/// The card of `key` turned over to edit it.
async function edited(controller, key) {
  await controller.edit(key)();
  return controller.editing;
}

test("a card's proxy saves none as direct, restores it and inherits again when blank", async () => {
  const card = { id: "card", proxyUrl: "http://127.0.0.1:7890" };
  const { controller, saved } = editor([card]);
  const draft = async () => {
    controller.stopEditing();
    return edited(controller, "card");
  };
  // Turned off, the proxy saves as direct.
  let state = await draft();
  assert.deepEqual({ ...state.proxy }, { enabled: true, url: "http://127.0.0.1:7890" });
  state.proxy.enabled = false;
  assert.equal(await controller.saveProxyDraft(state), true);
  assert.deepEqual(saved.pop(), ["card", "direct://"]);
  // Direct shows as off and blank; turned on and left blank, it inherits.
  card.proxyUrl = "direct://";
  state = await draft();
  assert.deepEqual({ ...state.proxy }, { enabled: false, url: "" });
  state.proxy.enabled = true;
  await controller.saveProxyDraft(state);
  assert.deepEqual(saved.pop(), ["card", ""]);
  // Unchanged, nothing is saved.
  card.proxyUrl = "";
  await controller.saveProxyDraft(await draft());
  assert.equal(saved.length, 0);
});

test("an invalid proxy keeps the card open with its error", async () => {
  const { scope, back, controller, saved } = editor([{ id: "card", proxyUrl: "" }]);
  scope.failSave = "Invalid account proxy URL";
  let rendered = 0;
  scope.renderEditing = () => rendered++;
  const state = await edited(controller, "card");
  state.proxy.url = "ftp://x";
  await back.finishEditing();
  assert.equal(controller.editing, state);
  assert.equal(state.proxyError, "Invalid account proxy URL");
  // The error shows with Done enabled, to try again.
  assert.equal(state.saving, false);
  assert.equal(rendered, 1);
  assert.equal(saved.length, 0);
  // Corrected, it saves and the card turns back.
  scope.failSave = null;
  state.proxy.url = "http://127.0.0.1:1";
  await back.finishEditing();
  assert.deepEqual(saved, [["card", "http://127.0.0.1:1"]]);
  assert.equal(controller.editing, null);
});

test("a card turned back or superseded while its proxy saves is not turned again", async () => {
  const turned = [];
  const { scope, back, controller } = editor([{ id: "card", proxyUrl: "" }, { id: "other", proxyUrl: "" }], {
    turnCard: (key, direction, change) => (turned.push([key, direction]), change()),
  });
  scope.renderEditing = () => turned.push(["render"]);
  for (const meanwhile of [
    () => controller.stopEditing(),
    () => controller.edit("other")(),
    () => (scope.controller = new Accounts(async () => [], [], () => {}, () => {}, { pool: "claude" })),
  ]) {
    scope.controller = controller;
    controller.stopEditing();
    const state = await edited(controller, "card");
    state.proxy.url = "http://127.0.0.1:1";
    let save;
    controller.saveProxyDraft = () => new Promise((resolve) => (save = resolve));
    const finishing = back.finishEditing();
    await meanwhile();
    save(true);
    await finishing;
    assert.deepEqual(turned, []);
  }
});

test("which card shows its back is part of what the cards render from", async () => {
  // A failed removal closes the back without changing the cards.
  const cards = [{ id: "card", proxyUrl: "" }];
  const { controller } = editor(cards);
  const face = dom.cardFace({ controller });
  const front = face.renderState(cards);
  await edited(controller, "card");
  const back = face.renderState(cards);
  assert(!P.cardsEqual(back, front));
  controller.stopEditing();
  assert(P.cardsEqual(face.renderState(cards), front));
});

test("a provider's group edits its provider's proxy, saved once through its first card", async () => {
  const cards = [member(1, { proxyUrl: "socks5://h:1" }), member(2, { proxyUrl: "socks5://h:1" }), { id: "solo", proxyUrl: "" }];
  const { controller, saved } = editor(cards);
  const group = controller.flipTarget("sub2api:team");
  assert.deepEqual(group.group.accounts.map((a) => a.id), ["sub2api-team-1", "sub2api-team-2"]);
  // Its accounts have no proxy of their own; with no greetings, no back.
  const one = controller.flipTarget("sub2api-team-2");
  assert.equal(one.grouped, true);
  assert.equal(controller.edit("sub2api-team-2"), null);
  assert.equal(controller.flipTarget("gone"), null);
  assert.equal(controller.edit("gone"), null);
  const state = await edited(controller, "sub2api:team");
  assert.deepEqual({ ...state.proxy }, { enabled: true, url: "socks5://h:1" });
  state.proxy.enabled = false;
  await controller.saveProxyDraft(state);
  assert.deepEqual(saved, [["sub2api-team-1", "direct://"]]);
});

test("a provider's only card turns over as an ordinary card, to its proxy", async () => {
  const { controller, saved } = editor([member(1), { id: "solo", proxyUrl: "" }]);
  assert.equal(controller.flipTarget("sub2api-team-1").grouped, false);
  const state = await edited(controller, "sub2api-team-1");
  state.proxy.enabled = false;
  await controller.saveProxyDraft(state);
  assert.deepEqual(saved, [["sub2api-team-1", "direct://"]]);
});

test("a card that greets reads its greeting schedule as it turns over, and saves it to its pool", async () => {
  const calls = [];
  const invoke = async (command, args) => {
    calls.push([command, args]);
    if (command === "greeting_schedule") return { auto: true, times: ["08:00"] };
    return [];
  };
  const card = { id: "a", usage: { planType: "plus" }, proxyUrl: "" };
  const controller = new Accounts(invoke, [card], () => {}, () => {}, { pool: "codex" });
  const state = await edited(controller, "a");
  assert.deepEqual(state.schedule, { auto: true, times: ["08:00"] });
  await controller.saveGreeting(state, "a", false, ["08:00", "09:00"]);
  assert.deepEqual(calls, [
    ["greeting_schedule", { id: "a" }],
    ["set_greeting_schedule", { id: "a", auto: false, times: ["08:00", "09:00"] }],
  ]);
  assert.deepEqual(state.schedule, { auto: false, times: ["08:00", "09:00"] });
  // One it could not read is told, and the card turns over without it.
  const failed = [];
  const broken = new Accounts(async () => { throw new Error("offline"); }, [card], () => {}, () => {}, { pool: "codex" });
  await broken.edit("a", (error) => failed.push(error.message))();
  assert.equal(broken.editing.schedule, null);
  assert.deepEqual(failed, ["offline"]);
});

test("a blank card proxy is described and shown as the shared account proxy when one is set", () => {
  const { scope, back } = editor([]);
  scope.preferences.accountProxyUrl = " socks5://127.0.0.1:7891 ";
  assert.equal(back.proxyHelp({}), "accounts.proxy.shared_help");
  assert.equal(back.proxyPlaceholder({}), "socks5://127.0.0.1:7891");
  // A Sub2api provider's cards never take the shared proxy.
  const provider = { sourceTag: "SUB2API" };
  assert.equal(back.proxyHelp(provider), "accounts.proxy.system_help");
  assert.equal(back.proxyPlaceholder(provider), "http://127.0.0.1:7890");
  scope.preferences.accountProxyUrl = "direct://";
  assert.equal(back.proxyHelp({}), "accounts.proxy.shared_help");
  assert.equal(back.proxyPlaceholder({}), "direct://");
  scope.preferences.accountProxyUrl = "";
  assert.equal(back.proxyHelp({}), "accounts.proxy.system_help");
  assert.equal(back.proxyPlaceholder({}), "http://127.0.0.1:7890");
});

test("other cards shown end editing at once without saving, and a turn under way never shows", async () => {
  const { controller, saved } = editor([{ id: "a", proxyUrl: "" }, { id: "b", proxyUrl: "" }]);
  const state = await edited(controller, "a");
  state.proxy.url = "http://new:1";
  const open = controller.edit("b");
  assert.equal(controller.isEditing("b"), true);
  assert.equal(controller.stopEditing(), state);
  await open();
  assert.equal(controller.editing, null);
  assert.equal(controller.isEditing("b"), false);
  assert.equal(saved.length, 0);
});

/// keys.js over a card back and its controller, handling `document`'s
/// keydown events; `globals` are its host's (`page`, `collapsed`, …).
function keyboard(back, controller, globals) {
  const host = {
    mac: true,
    menus: { handleKey: () => false },
    pool: controller.pool,
    collapsed: false,
    reordering: false,
    controller,
    menuBarNavigation: false,
    editCard: (key) => back.editCard(key),
    cancelEditing: () => back.cancelEditing(),
    ...globals,
  };
  host.document.addEventListener("keydown", Keys.create(host).onKeydown);
  return host;
}

test("Escape cancels card editing and only a fresh press can hide the panel", () => {
  const document = new EventTarget();
  document.querySelector = () => null;
  const commands = [];
  const { back, controller, saved } = editor([{ id: "card", proxyUrl: "" }]);
  keyboard(back, controller, {
    document,
    page: "accounts",
    invoke: async (command) => commands.push(command),
  });
  function escape(repeat = false) {
    const event = Object.assign(new Event("keydown", { cancelable: true }), { key: "Escape", repeat });
    document.dispatchEvent(event);
    assert.equal(event.defaultPrevented, true);
  }

  controller.editing = { key: "card", proxy: { enabled: true, url: "http://draft:1" } };
  escape();
  assert.equal(controller.editing, null);
  assert.deepEqual(saved, []);
  escape(true);
  assert.deepEqual(commands, []);
  escape();
  assert.deepEqual(commands, ["hide_panel"]);
});

test("Return flips the hovered account card to edit, unless a focused UI element owns Return", async () => {
  const body = { closest: () => null };
  const button = { closest: (sel) => (sel.includes("button") ? button : null) };
  const cardNode = (key, classes = []) => ({
    dataset: { flipKey: key },
    classList: { contains: (name) => classes.includes(name) },
  });
  let hovered = [];
  const document = new EventTarget();
  document.body = body;
  document.documentElement = {};
  document.activeElement = body;
  document.querySelector = () => null;
  document.getElementById = (id) =>
    id === "accounts"
      ? { querySelectorAll: (sel) => (sel === "[data-flip-key]:is(:hover, .hover-hold)" ? hovered : []) }
      : null;

  const accounts = [{ id: "solo", proxyUrl: "" }, member(1), member(2)];
  const { back, controller } = editor(accounts);
  const scope = keyboard(back, controller, {
    document,
    page: "accounts",
    invoke: async () => null,
  });

  function pressEnter(extra = {}) {
    const event = Object.assign(new Event("keydown", { cancelable: true }), {
      key: "Enter",
      ...extra,
    });
    document.dispatchEvent(event);
    return event;
  }

  // Focused interactive control (e.g. Tab on a button) keeps Return even while hovering a card.
  hovered = [cardNode("solo")];
  document.activeElement = button;
  assert.equal(pressEnter().defaultPrevented, false);
  assert.equal(controller.editing, null);

  // Repeat, modifiers, IME composition, collapsed mode, reordering, and other pages do not flip.
  document.activeElement = body;
  assert.equal(pressEnter({ repeat: true }).defaultPrevented, false);
  assert.equal(pressEnter({ metaKey: true }).defaultPrevented, false);
  assert.equal(pressEnter({ isComposing: true }).defaultPrevented, false);
  assert.equal(pressEnter({ keyCode: 229 }).defaultPrevented, false);
  scope.collapsed = true;
  assert.equal(pressEnter().defaultPrevented, false);
  scope.collapsed = false;
  scope.reordering = true;
  assert.equal(pressEnter().defaultPrevented, false);
  scope.reordering = false;
  scope.page = "settings";
  assert.equal(pressEnter().defaultPrevented, false);
  scope.page = "accounts";
  assert.equal(controller.editing, null);

  // Plain Return on a hovered card flips it into edit mode.
  assert.equal(pressEnter().defaultPrevented, true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(controller.editing?.key, "solo");

  // Hovering an already-flipped card back does nothing.
  hovered = [cardNode("solo", ["account-back"])];
  assert.equal(pressEnter().defaultPrevented, false);

  // Hovering a grouped Sub2api member without its own back flips its provider group.
  hovered = [cardNode("sub2api:team"), cardNode("sub2api-team-1")];
  assert.equal(pressEnter().defaultPrevented, true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(controller.editing?.key, "sub2api:team");
});

test("a card turning over is turned back by Escape or by another card's edit", async () => {
  // Turns wait until run, as an animation's first half does.
  const turns = [];
  const { back, controller } = editor([{ id: "a", proxyUrl: "" }, { id: "b", proxyUrl: "" }], {
    turnCard: (key, direction, change) =>
      new Promise((resolve) => turns.push({ key, direction, run: () => Promise.resolve(change()).then(resolve) })),
  });
  // Escape while the card is edge-on: its back never shows.
  const first = back.editCard("a");
  back.cancelEditing();
  await turns.shift().run();
  await first;
  assert.equal(controller.editing, null);
  // A card being turned over is not turned again.
  const second = back.editCard("a");
  assert.equal(controller.edit("a"), null);
  // Another card edited meanwhile is the one that shows its back.
  const third = back.editCard("b");
  await turns.shift().run();
  await turns.shift().run();
  await Promise.all([second, third]);
  assert.equal(controller.editing.key, "b");
  // Editing another card turns this one back, without saving; should the
  // other's back show first, this one's turn does not close it.
  const fourth = back.editCard("a");
  assert.deepEqual(turns.map(({ key, direction }) => [key, direction]), [["b", -1], ["a", 1]]);
  await turns.pop().run();
  await fourth;
  await turns.shift().run();
  assert.equal(controller.editing.key, "a");
  assert.equal(turns.length, 0);
});

test("greeting times are added in order once and removed by value", async () => {
  const saves = [];
  const invoke = async (command, args) => saves.push([command, args]);
  // The schedule goes to the card's pool through its controller.
  const back = CardBack.create(host({
    controller: new Accounts(invoke, [], () => {}, () => {}, { pool: "claude" }),
    notice: (error) => {
      throw error;
    },
  }));
  const add = (times, time) => P.withGreetingTime(times, time);
  assert.deepEqual(add(["12:30"], "08:00"), ["08:00", "12:30"]);
  assert.equal(add(["08:00"], "08:00"), null, "a time already set");
  assert.equal(add(["08:00"], ""), null, "a cleared time input");
  assert.deepEqual(P.withoutGreetingTime(["08:00", "12:30"], "08:00"), ["12:30"]);
  // Saving keeps Auto and shows the times saved on the card.
  const state = { schedule: { auto: true, times: ["08:00"], lastError: null } };
  await back.saveGreeting("a", state, true, []);
  assert.deepEqual(saves, [["set_greeting_schedule", { id: "a", auto: true, times: [] }]]);
  assert.deepEqual(state.schedule.times, []);
  assert.equal(state.schedule.auto, true);
});

// Which items a card's menu has is card-view.js's (card-view.test.cjs);
// here, what they run.
test("a card's menu runs its card's actions, a provider's for its group", () => {
  const calls = [];
  const record = (name) => (...args) => (calls.push([name, ...args.map((a) => a?.id ?? a)]), Promise.resolve());
  const scope = dom.cardFace({
    tr: (key) => key,
    cardContext: () => ({ pool: "codex", locale: "en", tr: (key) => key, collapsed: false }),
    providerTitle: () => "Team",
    editCard: record("editCard"),
    refreshUsage: record("refreshUsage"),
    refreshProvider: (group) => calls.push(["refreshProvider", group.key]),
    greet: record("sayHi"),
    renewNow: record("renewNow"),
    switchAccount: record("switchAccount"),
    confirmRemoval: (cards, name, sub2api) => calls.push(["confirmRemoval", cards.map((a) => a.id), name, sub2api]),
    notice: record("notice"),
    signIn: record("signIn"),
  });
  const run = (target) =>
    Array.from(scope.cardMenu(target), (item) => {
      if (item.separator) return "—";
      const before = calls.length;
      item.run();
      return [item.label, ...calls.slice(before)];
    });
  const account = { id: "a", email: "a@example.com", planType: "plus", renewal: "always", isCurrent: false, usage: {} };
  assert.deepEqual(JSON.parse(JSON.stringify(run({ account }))), [
    ["accounts.card.edit", ["editCard", "a"]],
    "—",
    ["common.refresh_usage", ["refreshUsage", "a"]],
    ["accounts.say_hi.now", ["sayHi", "a"]],
    ["accounts.renewal.now", ["renewNow", "a"]],
    ["accounts.card.switch_to_this", ["switchAccount", "a"]],
    "—",
    ["common.remove", ["confirmRemoval", ["a"], "a@example.com", false]],
  ]);
  assert.equal(scope.cardMenu({ account }).at(-1).destructive, true);
  // A provider's group: its provider's refresh, switch through an active
  // card and removal.
  const group = { key: "team", accounts: [
    { id: "sub2api-team-1", sourceTag: "SUB2API", available: false },
    { id: "sub2api-team-2", sourceTag: "SUB2API" },
  ] };
  assert.deepEqual(JSON.parse(JSON.stringify(run({ group }))), [
    ["accounts.card.edit", ["editCard", "sub2api:team"]],
    "—",
    ["common.refresh_usage", ["refreshProvider", "team"]],
    ["accounts.card.switch_to_this", ["switchAccount", "sub2api-team-2"]],
    "—",
    ["common.remove", ["confirmRemoval", ["sub2api-team-1", "sub2api-team-2"], "Team", true]],
  ]);
});

test("refresh intervals offer the one set and speed up to a fifth, no sooner than 30 s", () => {
  const P = require("../ui/presentation.js");
  assert.deepEqual(P.refreshChoices("codex", 10).seconds, [10, 30, 60, 300, 600]);
  assert.deepEqual(P.refreshChoices("claude", 300).fullEvery, [1, 2, 3, 6, 12]);
  // A value set elsewhere stays selectable, in order.
  assert.deepEqual(P.refreshChoices("antigravity", 450).seconds.slice(2, 5), [300, 450, 600]);
  assert.equal(P.fastRefreshSeconds(300), 60);
  assert.equal(P.fastRefreshSeconds(60), 30);
  assert.equal(P.fastRefreshSeconds(10), 10);
  assert.equal(P.formatInterval(10, "en"), "10 seconds");
  assert.equal(P.formatInterval(300, "en"), "5 minutes");
  assert.equal(P.formatInterval(3600, "en"), "1 hour");
  assert.equal(P.formatInterval(300, "zh-Hans"), "5分钟");
});

/// A pool's refresh section on a card in use: each row's label, field,
/// line under it and tooltip, and the refresh each control saves.
function refreshRows(pool, refresh, key = "acct", cards) {
  const saved = [];
  const scope = host({
    pool,
    preferences: { refresh: { [pool]: refresh, ...(cards ? { cards } : {}) } },
    // As the app's queue: each patch on the preferences the last left.
    queuePreferenceSave: async (patch) => {
      const value = typeof patch === "function" ? patch(scope.preferences) : patch;
      scope.preferences = { ...scope.preferences, ...value };
      saved.push(value);
    },
  });
  const section = CardBack.create(scope).refreshSection(true, key);
  /// The row of a control: its label, its control, the line under the label
  /// and the label's tooltip.
  const field = (name) => {
    const control = section.field(name);
    if (!control) return undefined;
    const label = control.parent.children[0];
    const note = label.children.find((n) => n.className === "back-note");
    return { label: label.textContent, control, note: note?.textContent ?? null, tip: label.title || null };
  };
  return { saved, scope, field };
}
/// Turns a switch of a card's back on or off, as a click does.
function flip(control, on) {
  control.checked = on;
  control.onchange();
}

test("two refresh changes made before the card renders again both stay, on the card alone", async () => {
  const refresh = { seconds: 300, fullEvery: 1, adaptive: true, backoff: true };
  const other = { other: { adaptive: true } };
  const { field, scope } = refreshRows("claude", refresh, "acct", { claude: other });
  flip(field("refresh-backoff").control, false);
  flip(field("refresh-adaptive").control, false);
  const fullEvery = field("refresh-full-every").control;
  fullEvery.value = "2";
  fullEvery.onchange();
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(JSON.parse(JSON.stringify(scope.preferences.refresh)), {
    // The pool's interval saves on the pool; its switches, those of cards
    // without their own, stay.
    claude: { ...refresh, fullEvery: 2 },
    cards: { claude: { ...other, acct: { adaptive: false, backoff: false } } },
  });
});

test("a card's switches follow its pool's until it sets its own", () => {
  const refresh = { seconds: 300, fullEvery: 1, adaptive: true, backoff: true };
  const cards = { claude: { acct: { adaptive: false } }, codex: { other: { backoff: false } } };
  const own = refreshRows("claude", refresh, "acct", cards);
  assert.equal(own.field("refresh-adaptive").control.checked, false);
  assert.equal(own.field("refresh-backoff").control.checked, true);
  // Another card, or the same key in another pool, has the pool's.
  assert.equal(refreshRows("claude", refresh, "other", cards).field("refresh-adaptive").control.checked, true);
  assert.equal(refreshRows("codex", refresh, "acct", cards).field("refresh-adaptive").control.checked, true);
  assert.equal(refreshRows("claude", { ...refresh, adaptive: false }).field("refresh-adaptive").control.checked, false);
});

test("a Sub2api provider's cards share their speed-up and have no slow-down", async () => {
  const refresh = { seconds: 300, fullEvery: 1, adaptive: true, backoff: true };
  const { field, scope } = refreshRows("claude", refresh, "sub2api:p");
  assert.equal(field("refresh-backoff"), undefined);
  flip(field("refresh-adaptive").control, false);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(JSON.parse(JSON.stringify(scope.preferences.refresh.cards)), {
    claude: { "sub2api:p": { adaptive: false } },
  });
  // The key is the core's, the card's or its provider's cards' (refresh_plan.rs).
  const { refreshKey } = V;
  const member = { id: "claude-sub2api-p-3", sourceTag: "SUB2API", refresh: { key: "sub2api:p" } };
  assert.equal(refreshKey({ group: { key: "p", accounts: [member] } }), "sub2api:p");
  assert.equal(refreshKey({ account: member }), "sub2api:p");
  assert.equal(refreshKey({ account: { id: "acct", refresh: { key: "acct" } } }), "acct");
});

test("only Claude's own accounts offer the slow-down after HTTP 429, on by default", async () => {
  const refresh = { seconds: 300, fullEvery: 1, adaptive: true, backoff: true };
  for (const pool of ["codex", "antigravity"])
    assert.equal(refreshRows(pool, refresh).field("refresh-backoff"), undefined, pool);
  const { field, saved } = refreshRows("claude", refresh);
  const backoff = field("refresh-backoff");
  assert.equal(backoff.label, "accounts.refresh.backoff");
  assert.equal(backoff.control.checked, true);
  // What it does shows on its name, on or off, as does the speed-up's.
  assert.deepEqual([backoff.note, backoff.tip], [null, "accounts.refresh.backoff_help"]);
  assert.deepEqual(
    [field("refresh-adaptive").note, field("refresh-adaptive").tip],
    [null, "accounts.refresh.adaptive_help_format 1 minute"],
  );
  // Turned off, it saves on the card.
  flip(backoff.control, false);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(JSON.parse(JSON.stringify(saved)), [
    { refresh: { claude: refresh, cards: { claude: { acct: { backoff: false } } } } },
  ]);
  assert.equal(
    refreshRows("claude", { ...refresh, backoff: false }).field("refresh-backoff").tip,
    "accounts.refresh.backoff_help",
  );
});

/// A card back whose greetings and proxy are read on a Codex Plus card
/// with nothing under way, and the inputs it built.
function cardFields() {
  const inputs = [];
  const scope = host({
    controller: { cardState: () => ({ refreshing: false, hi: null }) },
    el: (...args) => {
      const field = dom.el(...args);
      if (field.tag === "input") inputs.push(field);
      return field;
    },
  });
  return { scope, back: CardBack.create(scope), inputs };
}
const greeter = { id: "a", usage: { planType: "plus" } };
function greetingTimesLabel(lastError) {
  const { back } = cardFields();
  const state = { adding: null, schedule: { auto: false, times: ["08:00"], lastError } };
  const section = back.greetingSection(greeter, state);
  const daily = section.find((n) => n.className === "back-row back-stack");
  return daily.children.find((n) => n.className === "back-label");
}

test("Escape leaves the proxy field with its draft intact; Enter still finishes editing", async () => {
  const { scope, back, inputs } = cardFields();
  const state = { proxy: { enabled: true, url: "" } };
  let finished = 0;
  let blurred = 0;
  // Enter finishes editing: the card's proxy saves through its controller.
  scope.controller = { editing: state, saveProxyDraft: async () => (finished++, false) };
  back.proxySection(state, {});
  const [url] = inputs;
  url.blur = () => blurred++;
  url.value = "http://draft:1";
  url.oninput();
  for (const composition of [{ isComposing: true }, { keyCode: 229 }]) {
    const ime = Object.assign(new Event("keydown", { cancelable: true }), { key: "Escape", ...composition });
    url.onkeydown(ime);
    assert.equal(ime.defaultPrevented, false);
    assert.equal(blurred, 0);
  }
  const esc = Object.assign(new Event("keydown", { cancelable: true }), { key: "Escape" });
  url.onkeydown(esc);
  assert.equal(esc.defaultPrevented, true);
  assert.equal(esc.cancelBubble, true);
  assert.equal(blurred, 1);
  assert.equal(finished, 0);
  assert.equal(state.proxy.url, "http://draft:1");
  url.onkeydown(Object.assign(new Event("keydown", { cancelable: true }), { key: "Enter" }));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(finished, 1);
});

test("Escape cancels only the new greeting time, including when the field blurs", () => {
  const { scope, back, inputs } = cardFields();
  const state = { adding: "12:30", schedule: { auto: false, times: [] } };
  back.greetingSection(greeter, state);
  const time = inputs.find((input) => input.dataset.backField === "greeting-time");
  time.isConnected = true;
  scope.renderEditing = () => time.onblur();
  const esc = Object.assign(new Event("keydown", { cancelable: true }), { key: "Escape" });
  time.onkeydown(esc);
  assert.equal(esc.defaultPrevented, true);
  assert.equal(esc.cancelBubble, true);
  assert.equal(state.adding, null);
  assert.deepEqual(state.schedule.times, []);
});

test("Say hi now waits while its card refreshes, greets or renews", () => {
  const { scope, back } = cardFields();
  const state = { adding: null, schedule: { auto: false, times: [] } };
  const offered = (ui) => {
    scope.controller.cardState = () => ({ refreshing: false, hi: null, renewing: false, ...ui });
    const section = back.greetingSection(greeter, state);
    return !section.find((n) => n.textContent === "accounts.say_hi.now").disabled;
  };
  assert.equal(offered({}), true);
  for (const ui of [{ refreshing: true }, { hi: "sent" }, { hi: "received" }, { renewing: true }])
    assert.equal(offered(ui), false, JSON.stringify(ui));
});

test("what the greeting times do shows on their name, a failed greeting under it", () => {
  const label = greetingTimesLabel(null);
  assert.equal(label.title, "accounts.say_hi.times_help");
  assert.deepEqual(label.children, []);
  const failed = greetingTimesLabel("Greeting failed");
  assert.equal(failed.title, "accounts.say_hi.times_help");
  assert.deepEqual(
    failed.children.map((n) => [n.className, n.text]),
    [["back-note error", "Greeting failed"]],
  );
});

test("daily greeting times and Add time flow inline after the label and wrap when full", () => {
  const css = fs
    .readFileSync(path.join(__dirname, "../ui/style.css"), "utf8")
    .replace(/\r\n/g, "\n");
  const rule = (selector) => {
    const start = css.indexOf(`${selector} {\n`);
    assert(start >= 0, `${selector} rule found`);
    const end = css.indexOf("\n}", start);
    assert(end > start, `${selector} rule closed`);
    return css.slice(start, end);
  };
  const stack = rule(".back-stack");
  assert.match(stack, /flex-wrap:\s*wrap;/);
  assert.doesNotMatch(stack, /flex-direction:\s*column;/);
  assert.match(rule(".back-stack > .back-label"), /flex:\s*0 1 auto;/);
  assert.match(rule(".back-stack > .back-label:has(.back-note)"), /flex-basis:\s*100%;/);
  assert.match(rule(".back-times"), /display:\s*contents;/);
  assert.match(rule(".back-time"), /white-space:\s*nowrap;/);
});
