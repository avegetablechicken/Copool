const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

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
    supportsSayHi: (a) => a.sourceTag !== "SUB2API",
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
  const { scope, saved, rendered } = editor([card]);
  scope.failSave = "Invalid account proxy URL";
  scope.editing = { key: "card", proxy: { enabled: true, url: "ftp://x" }, proxyError: null };
  assert.equal(await scope.saveProxyDraft(scope.editing), false);
  assert.equal(scope.editing.proxyError, "Invalid account proxy URL");
  assert.equal(scope.editing.saving, false);
  assert.equal(saved.length, 0);
  assert.equal(rendered.length, 1, "the error shows");
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
  assert.equal(scope.hasBack(group), true);
  // Its accounts have no proxy of their own; with no greetings, no back.
  const member = scope.flipTarget("sub2api-team-2");
  assert.equal(member.grouped, true);
  assert.equal(scope.hasBack(member), false);
  assert.equal(scope.hasBack(scope.flipTarget("solo")), true);
  assert.equal(scope.flipTarget("gone"), null);
  scope.editing = { key: "sub2api:team", proxy: { enabled: false, url: "" } };
  await scope.saveProxyDraft(scope.editing);
  assert.deepEqual(saved, [["sub2api-team-1", "direct://"]]);
});

test("a blank card proxy is described as the shared account proxy when one is set", () => {
  const { scope } = editor([]);
  scope.preferences.accountProxyUrl = " socks5://127.0.0.1:7891 ";
  assert.equal(scope.proxyHelp({}), "accounts.proxy.shared_help");
  // A Sub2api provider's cards never take the shared proxy.
  assert.equal(scope.proxyHelp({ sourceTag: "SUB2API" }), "accounts.proxy.system_help");
  scope.preferences.accountProxyUrl = "direct://";
  assert.equal(scope.proxyHelp({}), "accounts.proxy.shared_help");
  scope.preferences.accountProxyUrl = "";
  assert.equal(scope.proxyHelp({}), "accounts.proxy.system_help");
});

test("Escape turns a card back at once without saving when the page changes under it", () => {
  const card = { id: "card", proxyUrl: "" };
  const { scope, saved, rendered } = editor([card]);
  scope.editing = { key: "card", proxy: { enabled: true, url: "http://new:1" } };
  scope.cancelEditing(false);
  assert.equal(scope.editing, null);
  assert.equal(saved.length, 0);
  assert.deepEqual(rendered, [null]);
});

const greeting = between("/// The times with a new one, in order;", "/// The pool's background refresh");
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

const task = between("/// A hi or renewal under way, or just done", "function taskIndicator(");
test("a hi or renewal takes the refresh button's place: under way, then done", () => {
  const scope = {
    pool: "codex",
    sayingHi: new Map(),
    renewedIds: new Set(),
    accountsController: { renewingIds: new Set() },
    tr: (key) => key,
  };
  vm.createContext(scope);
  vm.runInContext(task, scope);
  const card = { id: "a" };
  assert.equal(scope.cardTask(card), null);
  scope.sayingHi.set("a", "sent");
  assert.deepEqual({ ...scope.cardTask(card) }, {
    glyph: "say-hi-sent", title: "accounts.card.codex_say_hi.sent", done: false,
  });
  scope.sayingHi.set("a", "received");
  assert.equal(scope.cardTask(card).done, true);
  scope.sayingHi.clear();
  scope.accountsController.renewingIds.add("a");
  assert.deepEqual({ ...scope.cardTask(card) }, {
    glyph: "renewal", title: "accounts.renewal.renewing", done: false,
  });
  scope.accountsController.renewingIds.clear();
  scope.renewedIds.add("a");
  assert.deepEqual({ ...scope.cardTask(card) }, {
    glyph: "renewal-done", title: "accounts.notice.token_renewed", done: true,
  });
});

const menu = between("/// A card's menu, at the pointer", "/// The plan tag.");
test("a card's menu edits it, then refreshes, greets, renews, switches and removes it", () => {
  const scope = {
    collapsed: false,
    switchingId: null,
    sayingHi: new Map(),
    accountsController: {
      refreshingIds: new Set(),
      remoteRefreshingIds: new Set(),
      sayingHiIds: new Set(),
      renewingIds: new Set(),
    },
    hasBack: (target) => !!target.group || !target.grouped || target.account.sourceTag !== "SUB2API",
    supportsSayHi: (a) => a.sourceTag !== "SUB2API",
    canSayHi: () => true,
    providerTitle: () => "Team",
    switchTarget: (group) => group.accounts[0],
    tr: (key) => key,
  };
  vm.createContext(scope);
  vm.runInContext(menu, scope);
  const labels = (target) =>
    Array.from(scope.cardMenu(target), (item) => (item.separator ? "—" : item.label));
  const account = { id: "a", email: "a@example.com", renewal: "always", isCurrent: false };
  assert.deepEqual(labels({ account }), [
    "accounts.card.edit", "—", "common.refresh_usage", "accounts.say_hi.now",
    "accounts.renewal.now", "accounts.card.switch_to_this", "—", "common.remove",
  ]);
  assert.equal(scope.cardMenu({ account }).at(-1).destructive, true);
  // The card in use has no switch; collapsed cards no editing.
  scope.collapsed = true;
  assert.deepEqual(labels({ account: { ...account, isCurrent: true, renewal: null } }), [
    "common.refresh_usage", "accounts.say_hi.now", "—", "common.remove",
  ]);
  scope.collapsed = false;
  // A provider's group: its provider's refresh, switch and removal.
  const member = { id: "sub2api-team-1", sourceTag: "SUB2API", isCurrent: false };
  assert.deepEqual(labels({ group: { key: "team", accounts: [member] } }), [
    "accounts.card.edit", "—", "common.refresh_usage", "accounts.card.switch_to_this",
    "—", "common.remove",
  ]);
  // A provider's account that cannot greet has no menu of its own.
  assert.deepEqual(labels({ account: member, grouped: true }), []);
  // Busy, its refresh, greeting and renewal wait.
  scope.accountsController.renewingIds.add("a");
  const busy = scope.cardMenu({ account });
  assert(busy.filter((item) => item.label && item.label !== "accounts.card.edit" && item.label !== "common.remove" && item.label !== "accounts.card.switch_to_this").every((item) => item.disabled));
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
