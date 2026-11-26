const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const V = require("../ui/card-view.js");

// Windows checkouts may end lines with CRLF; the markers below are one line.
const source = fs
  .readFileSync(path.join(__dirname, "../ui/app.js"), "utf8")
  .replace(/\r\n/g, "\n");
/// The app's source from `from` up to `to`, both found.
function between(from, to) {
  const start = source.indexOf(from);
  assert(start >= 0, `${from} found`);
  const end = source.indexOf(to, start);
  assert(end > start, `${to} found after ${from}`);
  return source.slice(start, end);
}
const editing = between("/// The card turned over to edit it", "/// A card's settings side");

/// The editing functions with a pool's cards and a recording controller.
function editor(accounts, extra = {}) {
  const saved = [];
  const rendered = [];
  const scope = {
    P: { visibleAccounts: (cards) => cards },
    accountsCache: accounts,
    pool: "codex",
    collapsed: false,
    preferences: { accountProxyUrl: "" },
    accountsController: {
      saveProxy: async (...args) => {
        if (scope.failSave) throw new Error(scope.failSave);
        saved.push(args);
      },
    },
    providerKey: (a) => /^sub2api-(.+)-\d+$/.exec(a.id)?.[1] ?? null,
    // Any non-Sub2api card greets here.
    V: {
      ...V,
      supportsSayHi: (a) => a.sourceTag !== "SUB2API",
      hasBack: (t) => !!t && (!!t.group || !t.grouped || t.account.sourceTag !== "SUB2API"),
    },
    tr: (key, ...args) => [key, ...args].join(" "),
    renderAccounts: () => rendered.push(scope.editing),
    ...extra,
  };
  vm.createContext(scope);
  vm.runInContext(
    `${editing}\nObject.defineProperty(this, "editing", { get: () => editing, set: (v) => (editing = v) });`,
    scope,
  );
  return { scope, saved, rendered };
}

test("a card's proxy saves none as direct, restores it and inherits again when blank", async () => {
  const card = { id: "card", proxyUrl: "http://127.0.0.1:7890" };
  const { scope, saved } = editor([card]);
  const save = async (draft) => {
    scope.editing = { key: "card", proxy: draft };
    return scope.saveProxyDraft(scope.editing);
  };
  // Turned off, the proxy saves as direct.
  const draft = scope.proxyDraft(card.proxyUrl);
  assert.deepEqual({ ...draft }, { enabled: true, url: "http://127.0.0.1:7890" });
  draft.enabled = false;
  assert.equal(await save(draft), true);
  assert.deepEqual(saved.pop(), ["card", "direct://"]);
  // Direct shows as off and blank; turned on and left blank, it inherits.
  card.proxyUrl = "direct://";
  const direct = scope.proxyDraft(card.proxyUrl);
  assert.deepEqual({ ...direct }, { enabled: false, url: "" });
  direct.enabled = true;
  await save(direct);
  assert.deepEqual(saved.pop(), ["card", ""]);
  // Unchanged, nothing is saved.
  card.proxyUrl = "";
  await save(scope.proxyDraft(""));
  assert.equal(saved.length, 0);
});

test("an invalid proxy keeps the card open with its error", async () => {
  const card = { id: "card", proxyUrl: "" };
  const savingWhenRendered = [];
  const { scope, saved } = editor([card], {
    renderAccounts: () => savingWhenRendered.push(scope.editing.saving),
  });
  scope.failSave = "Invalid account proxy URL";
  scope.editing = { key: "card", proxy: { enabled: true, url: "ftp://x" }, proxyError: null };
  assert.equal(await scope.saveProxyDraft(scope.editing), false);
  assert.equal(scope.editing.proxyError, "Invalid account proxy URL");
  assert.equal(scope.editing.saving, false);
  assert.equal(saved.length, 0);
  // The error shows with Done enabled, to try again.
  assert.deepEqual(savingWhenRendered, [false]);
});

test("a provider's group edits its provider's proxy, saved once through its first card", async () => {
  const cards = [
    { id: "sub2api-team-1", sourceTag: "SUB2API", proxyUrl: "socks5://h:1" },
    { id: "sub2api-team-2", sourceTag: "SUB2API", proxyUrl: "socks5://h:1" },
    { id: "solo", proxyUrl: "" },
  ];
  const { scope, saved } = editor(cards);
  const group = scope.flipTarget("sub2api:team");
  assert.deepEqual(group.group.accounts.map((a) => a.id), ["sub2api-team-1", "sub2api-team-2"]);
  assert.equal(scope.proxyOwner(group).id, "sub2api-team-1");
  assert.equal(V.hasBack(group, scope.pool), true);
  // Its accounts have no proxy of their own; with no greetings, no back.
  const member = scope.flipTarget("sub2api-team-2");
  assert.equal(member.grouped, true);
  assert.equal(V.hasBack(member, scope.pool), false);
  assert.equal(V.hasBack(scope.flipTarget("solo"), scope.pool), true);
  assert.equal(scope.flipTarget("gone"), null);
  scope.editing = { key: "sub2api:team", proxy: { enabled: false, url: "" } };
  await scope.saveProxyDraft(scope.editing);
  assert.deepEqual(saved, [["sub2api-team-1", "direct://"]]);
});

test("a provider's only card turns over as an ordinary card, to its proxy", async () => {
  const card = { id: "sub2api-team-1", sourceTag: "SUB2API", proxyUrl: "" };
  const { scope, saved } = editor([card, { id: "solo", proxyUrl: "" }]);
  const target = scope.flipTarget(card.id);
  assert.equal(target.grouped, false);
  assert.equal(V.hasBack(target, scope.pool), true);
  scope.editing = { key: card.id, proxy: { enabled: false, url: "" } };
  await scope.saveProxyDraft(scope.editing);
  assert.deepEqual(saved, [["sub2api-team-1", "direct://"]]);
});

test("a blank card proxy is described and shown as the shared account proxy when one is set", () => {
  const { scope } = editor([]);
  scope.preferences.accountProxyUrl = " socks5://127.0.0.1:7891 ";
  assert.equal(scope.proxyHelp({}), "accounts.proxy.shared_help");
  assert.equal(scope.proxyPlaceholder({}), "socks5://127.0.0.1:7891");
  // A Sub2api provider's cards never take the shared proxy.
  const provider = { sourceTag: "SUB2API" };
  assert.equal(scope.proxyHelp(provider), "accounts.proxy.system_help");
  assert.equal(scope.proxyPlaceholder(provider), "http://127.0.0.1:7890");
  scope.preferences.accountProxyUrl = "direct://";
  assert.equal(scope.proxyHelp({}), "accounts.proxy.shared_help");
  assert.equal(scope.proxyPlaceholder({}), "direct://");
  scope.preferences.accountProxyUrl = "";
  assert.equal(scope.proxyHelp({}), "accounts.proxy.system_help");
  assert.equal(scope.proxyPlaceholder({}), "http://127.0.0.1:7890");
});

test("a page change turns a card back at once without saving", () => {
  const card = { id: "card", proxyUrl: "" };
  const { scope, saved, rendered } = editor([card]);
  scope.editing = { key: "card", proxy: { enabled: true, url: "http://new:1" } };
  scope.cancelEditing(false);
  assert.equal(scope.editing, null);
  assert.equal(saved.length, 0);
  assert.deepEqual(rendered, [null]);
});

test("Escape cancels card editing and only a fresh press can hide the panel", () => {
  const document = new EventTarget();
  document.querySelector = () => null;
  const commands = [];
  const { scope, saved } = editor([{ id: "card", proxyUrl: "" }], {
    document,
    selectedPage: "accounts",
    openMenu: null,
    TOOLBAR_SHORTCUTS: {},
    navigator: { userAgent: "Mac" },
    invoke: async (command) => commands.push(command),
  });
  scope.P.menuBarStep = require("../ui/presentation.js").menuBarStep;
  scope.turnCard = (_key, _direction, change) => change();
  vm.runInContext(between("let menuBarNavigation", "/// Refreshes one account"), scope);
  function escape(repeat = false) {
    const event = Object.assign(new Event("keydown", { cancelable: true }), { key: "Escape", repeat });
    document.dispatchEvent(event);
    assert.equal(event.defaultPrevented, true);
  }

  scope.editing = { key: "card", proxy: { enabled: true, url: "http://draft:1" } };
  escape();
  assert.equal(scope.editing, null);
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

  const accounts = [
    { id: "solo", proxyUrl: "" },
    { id: "sub2api-team-1", sourceTag: "SUB2API", proxyUrl: "" },
    { id: "sub2api-team-2", sourceTag: "SUB2API", proxyUrl: "" },
  ];
  const { scope } = editor(accounts, {
    document,
    selectedPage: "accounts",
    reorderingAccounts: false,
    openMenu: null,
    TOOLBAR_SHORTCUTS: {},
    navigator: { userAgent: "Mac" },
    invoke: async () => null,
  });
  scope.P.menuBarStep = require("../ui/presentation.js").menuBarStep;
  scope.turnCard = (_key, _direction, change) => change();
  vm.runInContext(between("let menuBarNavigation", "/// Refreshes one account"), scope);

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
  assert.equal(scope.editing, null);

  // Repeat, modifiers, IME composition, collapsed mode, reordering, and other pages do not flip.
  document.activeElement = body;
  assert.equal(pressEnter({ repeat: true }).defaultPrevented, false);
  assert.equal(pressEnter({ metaKey: true }).defaultPrevented, false);
  assert.equal(pressEnter({ isComposing: true }).defaultPrevented, false);
  assert.equal(pressEnter({ keyCode: 229 }).defaultPrevented, false);
  scope.collapsed = true;
  assert.equal(pressEnter().defaultPrevented, false);
  scope.collapsed = false;
  scope.reorderingAccounts = true;
  assert.equal(pressEnter().defaultPrevented, false);
  scope.reorderingAccounts = false;
  scope.selectedPage = "settings";
  assert.equal(pressEnter().defaultPrevented, false);
  scope.selectedPage = "accounts";
  assert.equal(scope.editing, null);

  // Plain Return on a hovered card flips it into edit mode.
  assert.equal(pressEnter().defaultPrevented, true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(scope.editing?.key, "solo");

  // Hovering an already-flipped card back does nothing.
  hovered = [cardNode("solo", ["account-back"])];
  assert.equal(pressEnter().defaultPrevented, false);

  // Hovering a grouped Sub2api member without its own back flips its provider group.
  hovered = [cardNode("sub2api:team"), cardNode("sub2api-team-1")];
  assert.equal(pressEnter().defaultPrevented, true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(scope.editing?.key, "sub2api:team");
});

test("a card turning over is turned back by Escape or by another card's edit", async () => {
  const { scope } = editor([{ id: "a", proxyUrl: "" }, { id: "b", proxyUrl: "" }], {
    invoke: async () => null,
  });
  // Turns wait until run, as an animation's first half does.
  const turns = [];
  scope.turnCard = (key, direction, change) =>
    new Promise((resolve) => turns.push({ key, direction, run: () => change().then(resolve) }));
  // Escape while the card is edge-on: its back never shows.
  const first = scope.editCard("a");
  scope.cancelEditing();
  await turns.shift().run();
  await first;
  assert.equal(scope.editing, null);
  // Another card edited meanwhile is the one that shows its back.
  const second = scope.editCard("a");
  const third = scope.editCard("b");
  await turns.shift().run();
  await turns.shift().run();
  await Promise.all([second, third]);
  assert.equal(scope.editing.key, "b");
  assert.equal(turns.length, 0);
});

const greeting = between("/// The times with a new one, in order;", "/// The key a card's own refresh switches are saved under");
test("greeting times are added in order once and removed by value", async () => {
  const saves = [];
  const scope = {
    invoke: async (command, args) => saves.push([command, JSON.parse(JSON.stringify(args))]),
    notice: (error) => {
      throw error;
    },
    renderEditing: () => {},
    editing: null,
  };
  vm.createContext(scope);
  vm.runInContext(greeting, scope);
  const add = (times, time) => {
    const result = scope.withGreetingTime(times, time);
    return result && Array.from(result);
  };
  assert.deepEqual(add(["12:30"], "08:00"), ["08:00", "12:30"]);
  assert.equal(add(["08:00"], "08:00"), null, "a time already set");
  assert.equal(add(["08:00"], ""), null, "a cleared time input");
  assert.deepEqual(Array.from(scope.withoutGreetingTime(["08:00", "12:30"], "08:00")), ["12:30"]);
  // Saving keeps Auto and shows the times saved on the card.
  const state = { pool: "claude", schedule: { auto: true, times: ["08:00"], lastError: null } };
  await scope.saveGreeting("a", state, true, []);
  assert.deepEqual(saves, [["set_greeting_schedule", { pool: "claude", id: "a", auto: true, times: [] }]]);
  assert.deepEqual(Array.from(state.schedule.times), []);
  assert.equal(state.schedule.auto, true);
});

// Which items a card's menu has is card-view.js's (card-view.test.cjs);
// here, what they run.
const menu = between("/// A card's menu, at the pointer", "/// The plan tag's tier color");
test("a card's menu runs its card's actions, a provider's for its group", () => {
  const calls = [];
  const record = (name) => (...args) => (calls.push([name, ...args.map((a) => a?.id ?? a)]), Promise.resolve());
  const scope = {
    V,
    cardContext: () => ({ pool: "codex", locale: "en", tr: (key) => key, collapsed: false }),
    providerTitle: () => "Team",
    editCard: record("editCard"),
    refreshUsage: record("refreshUsage"),
    refreshProvider: (group) => calls.push(["refreshProvider", group.key]),
    sayHi: record("sayHi"),
    refreshView: record("refreshView"),
    renewNow: record("renewNow"),
    switchAccount: record("switchAccount"),
    confirmRemoval: (cards, name, sub2api) => calls.push(["confirmRemoval", cards.map((a) => a.id), name, sub2api]),
    notice: record("notice"),
    signIn: record("signIn"),
  };
  vm.createContext(scope);
  vm.runInContext(menu, scope);
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

const refreshSection = between("/// A card's background refresh: the pool's intervals", "/// Removal, confirmed by a second click.");
/// A pool's refresh section on a card in use: each row's label, field,
/// line under it and tooltip, and the refresh each control saves.
function refreshRows(pool, refresh, key = "acct", cards) {
  const saved = [];
  const scope = {
    pool,
    locale: "en",
    preferences: { refresh: { [pool]: refresh, ...(cards ? { cards } : {}) } },
    P: {
      refreshChoices: () => ({ seconds: [refresh.seconds], fullEvery: [1] }),
      formatInterval: (seconds) => `${seconds}s`,
      fastRefreshSeconds: (seconds) => seconds / 5,
    },
    tr: (key, ...args) => [key, ...args].join(" "),
    el: () => ({ dataset: {}, append() {} }),
    backSection: () => ({ group: { rows: [], append(...rows) { this.rows.push(...rows); } } }),
    backRow: (label, control, note, tip) => ({ label, control, note, tip }),
    backSwitch: (field, on, label, onchange) => ({ dataset: { backField: field }, on, onchange }),
    // As the app's queue: each patch on the preferences the last left.
    queuePreferenceSave: async (patch) => {
      const value = typeof patch === "function" ? patch(scope.preferences) : patch;
      scope.preferences = { ...scope.preferences, ...value };
      saved.push(value);
    },
    notice: () => {},
    renderEditing: () => {},
  };
  vm.createContext(scope);
  vm.runInContext(refreshSection, scope);
  const rows = scope.refreshSection(true, key).group.rows;
  return {
    rows,
    saved,
    scope,
    field: (name) => rows.find((r) => r.control.dataset.backField === name),
  };
}

test("two refresh changes made before the card renders again both stay, on the card alone", async () => {
  const refresh = { seconds: 300, fullEvery: 1, adaptive: true, backoff: true };
  const other = { other: { adaptive: true } };
  const { field, scope } = refreshRows("claude", refresh, "acct", { claude: other });
  field("refresh-backoff").control.onchange(false);
  field("refresh-adaptive").control.onchange(false);
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
  assert.equal(own.field("refresh-adaptive").control.on, false);
  assert.equal(own.field("refresh-backoff").control.on, true);
  // Another card, or the same key in another pool, has the pool's.
  assert.equal(refreshRows("claude", refresh, "other", cards).field("refresh-adaptive").control.on, true);
  assert.equal(refreshRows("codex", refresh, "acct", cards).field("refresh-adaptive").control.on, true);
  assert.equal(refreshRows("claude", { ...refresh, adaptive: false }).field("refresh-adaptive").control.on, false);
});

test("a Sub2api provider's cards share their speed-up and have no slow-down", async () => {
  const refresh = { seconds: 300, fullEvery: 1, adaptive: true, backoff: true };
  const { field, scope } = refreshRows("claude", refresh, "sub2api:p");
  assert.equal(field("refresh-backoff"), undefined);
  field("refresh-adaptive").control.onchange(false);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(JSON.parse(JSON.stringify(scope.preferences.refresh.cards)), {
    claude: { "sub2api:p": { adaptive: false } },
  });
  const refreshKey = vm.runInNewContext(
    `${between("/// The key a card's own refresh switches are saved under", "/// A card's background refresh: the pool's intervals")}; refreshKey`,
    {
      providerKey: (a) =>
        a.sourceTag === "SUB2API" ? /^(?:claude-)?sub2api-(.+)-\d+$/.exec(a.id)?.[1] ?? null : null,
    },
  );
  assert.equal(refreshKey({ group: { key: "p" } }), "sub2api:p");
  assert.equal(refreshKey({ account: { id: "claude-sub2api-p-3", sourceTag: "SUB2API" } }), "sub2api:p");
  assert.equal(refreshKey({ account: { id: "acct" } }), "acct");
});

test("only Claude's own accounts offer the slow-down after HTTP 429, on by default", async () => {
  const refresh = { seconds: 300, fullEvery: 1, adaptive: true, backoff: true };
  for (const pool of ["codex", "antigravity"])
    assert.equal(refreshRows(pool, refresh).field("refresh-backoff"), undefined, pool);
  const { field, saved } = refreshRows("claude", refresh);
  const backoff = field("refresh-backoff");
  assert.equal(backoff.label, "accounts.refresh.backoff");
  assert.equal(backoff.control.on, true);
  // What it does shows on its name, on or off, as does the speed-up's.
  assert.deepEqual([backoff.note, backoff.tip], [null, "accounts.refresh.backoff_help"]);
  assert.deepEqual(
    [field("refresh-adaptive").note, field("refresh-adaptive").tip],
    [null, "accounts.refresh.adaptive_help_format 60s"],
  );
  // Turned off, it saves on the card.
  backoff.control.onchange(false);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(JSON.parse(JSON.stringify(saved)), [
    { refresh: { claude: refresh, cards: { claude: { acct: { backoff: false } } } } },
  ]);
  assert.equal(
    refreshRows("claude", { ...refresh, backoff: false }).field("refresh-backoff").tip,
    "accounts.refresh.backoff_help",
  );
});

const greetingSectionSource = between("/// Saying hi now, automatically, and at which times.", "/// The times with a new one, in order;");
const proxySectionSource = between("/// The proxy, saved with Done or Return.", "/// The token's time left");
function cardFields() {
  const inputs = [];
  const node = (tag, className = "", text = "") => ({
    tag,
    className,
    text,
    title: "",
    dataset: {},
    children: [],
    append(...nodes) {
      this.children.push(...nodes);
    },
    setAttribute() {},
  });
  const scope = {
    sayingHi: new Map(),
    accountsController: { refreshingIds: new Set(), remoteRefreshingIds: new Set() },
    V: { canSayHi: () => true },
    cardContext: () => ({}),
    tr: (key) => key,
    el: (...args) => {
      const field = node(...args);
      if (field.tag === "input") inputs.push(field);
      return field;
    },
    button: () => node("button"),
    backSection: () => Object.assign(node("div"), { group: node("div", "back-group") }),
    backButton: () => node("button"),
    backRow: () => node("label", "back-row"),
    backSwitch: () => node("input", "switch"),
    backHelp: () => node("p"),
    proxyHelp: () => "",
    proxyPlaceholder: () => "",
  };
  vm.createContext(scope);
  vm.runInContext(greetingSectionSource + proxySectionSource, scope);
  return { scope, inputs };
}
function greetingTimesLabel(lastError) {
  const { scope } = cardFields();
  const state = { adding: null, schedule: { auto: false, times: ["08:00"], lastError } };
  const section = scope.greetingSection({ id: "a" }, state);
  const daily = section.group.children.find((n) => n.className === "back-row back-stack");
  return daily.children.find((n) => n.className === "back-label");
}

test("Escape leaves the proxy field with its draft intact; Enter still finishes editing", () => {
  const { scope, inputs } = cardFields();
  const state = { proxy: { enabled: true, url: "" } };
  let finished = 0;
  let blurred = 0;
  scope.finishEditing = () => finished++;
  scope.proxySection(state, {});
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
  assert.equal(finished, 1);
});

test("Escape cancels only the new greeting time, including when the field blurs", () => {
  const { scope, inputs } = cardFields();
  const state = { adding: "12:30", schedule: { auto: false, times: [] } };
  scope.greetingSection({ id: "a" }, state);
  const [time] = inputs;
  time.isConnected = true;
  scope.renderEditing = () => time.onblur();
  const esc = Object.assign(new Event("keydown", { cancelable: true }), { key: "Escape" });
  time.onkeydown(esc);
  assert.equal(esc.defaultPrevented, true);
  assert.equal(esc.cancelBubble, true);
  assert.equal(state.adding, null);
  assert.deepEqual(state.schedule.times, []);
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
