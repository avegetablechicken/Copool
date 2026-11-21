const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const P = require("../ui/presentation.js");

// Windows checkouts may end lines with CRLF; the slices below look for LF.
const source = fs
  .readFileSync(path.join(__dirname, "../ui/app.js"), "utf8")
  .replace(/\r\n/g, "\n");
const body = (name) => {
  const start = source.indexOf(`function ${name}(`);
  const end = source.indexOf("\n}\n", start);
  assert(start >= 0 && end > start, name);
  return source.slice(start, end + 3);
};
function load() {
  const context = {
    P,
    locale: "en",
    tr: (key, ...args) => (args.length ? `${key}:${args.join(",")}` : key),
  };
  vm.createContext(context);
  vm.runInContext(body("nextRefreshAt") + body("refreshTooltip"), context);
  return context;
}

test("A refresh button tells how long until its next automatic refresh", () => {
  const { refreshTooltip } = load();
  const button = {};
  let at = null;
  refreshTooltip(button, "Refresh", () => at);
  assert.equal(button.title, "Refresh");
  // Worked out again as the pointer enters, not when the card rendered.
  at = Date.now() / 1000 + 90;
  button.onpointerenter();
  assert.equal(button.title, "Refresh\naccounts.refresh.next_format:2m");
  at = Date.now() / 1000 - 1;
  button.onpointerenter();
  assert.equal(button.title, "Refresh\naccounts.refresh.next_due");
});

test("The toolbar follows the full refresh; a provider its first card due", () => {
  const { nextRefreshAt } = load();
  assert.equal(nextRefreshAt([]), null);
  assert.equal(nextRefreshAt([{}, { nextRefreshAt: null }]), null);
  assert.equal(
    nextRefreshAt([{ nextRefreshAt: 300 }, {}, { nextRefreshAt: 120 }]),
    120,
  );
  // The toolbar shows the background's own schedule, not the cards'.
  assert.match(body("renderToolbar"), /refreshTooltip\([\s\S]*\$\("refresh-all"\)[\s\S]*\(\) => fullRefreshAt/);
  assert.match(body("refreshView"), /invoke\("next_full_refresh_at", \{ pool \}\)/);
  assert.match(source, /refreshTooltip\(refresh, tr\("common\.refresh_usage"\), \(\) => account\.nextRefreshAt\)/);
  assert.match(body("providerControls"), /nextRefreshAt\(group\.accounts\)/);
});

test("Refreshing a provider's only account refreshes the entire provider", () => {
  const calls = [];
  const context = {
    accountsController: { refresh: (id) => { calls.push(["account", id]); return Promise.resolve(false); } },
    refreshProvider: (group) => calls.push(["provider", group.key, group.accounts[0].id]),
    refreshView: () => Promise.resolve(),
    notice: () => {},
  };
  vm.createContext(context);
  vm.runInContext(body("providerKey") + body("refreshUsage"), context);
  context.refreshUsage("claude-sub2api-team-7");
  context.refreshUsage("sub2api-work-42");
  context.refreshUsage("official");
  context.refreshUsage();
  assert.deepEqual(calls, [
    ["provider", "team", "claude-sub2api-team-7"],
    ["provider", "work", "sub2api-work-42"],
    ["account", "official"],
    ["account", null],
  ]);
});

test("Cmd+1 to Cmd+3 select the pool both pages share, in place on the proxy page", () => {
  const app = fs
    .readFileSync(path.join(__dirname, "../ui/app.js"), "utf8")
    .replace(/\r\n/g, "\n");
  const start = app.indexOf('  } else if (mod && !event.altKey && !event.shiftKey && ["1", "2", "3"]');
  const end = app.indexOf('  } else if (mod && event.key === ",") {', start);
  assert(start >= 0 && end > start, "pool shortcut branch found");
  for (const [page, key, expected] of [
    ["proxy", "2", [["pool", "claude"]]],
    ["proxy", "1", [["pool", "codex"]]],
    ["accounts", "2", [["pool", "claude"]]],
    ["settings", "1", [["page", "accounts"], ["pool", "codex"]]],
    ["proxy", "3", [["pool", "antigravity"]]],
    ["accounts", "3", [["pool", "antigravity"]]],
  ]) {
    const calls = [];
    const scope = {
      ACCOUNT_POOLS: ["codex", "claude", "antigravity"],
      selectedPage: page,
      showProxyPool: (target) => calls.push(["proxy only", target]),
      showAccountsPool: (target) => calls.push(["accounts only", target]),
      showPool: (target) => calls.push(["pool", target]),
      showPage: (target) => calls.push(["page", target]),
    };
    vm.createContext(scope);
    vm.runInContext(
      `function route(event, mod) { if (false) {\n${app.slice(start, end)}  } }`,
      scope,
    );
    scope.route({ key, preventDefault() {} }, true);
    assert.deepEqual(calls, expected, `${page} Cmd+${key}`);
  }
});

test("Selecting a pool on either page selects it on both", () => {
  const app = fs
    .readFileSync(path.join(__dirname, "../ui/app.js"), "utf8")
    .replace(/\r\n/g, "\n");
  const start = app.indexOf("function showPool(next) {");
  const end = app.indexOf("\n}\n", start);
  assert(start >= 0 && end > start, "showPool found");
  const calls = [];
  const scope = {
    ACCOUNT_POOLS: ["codex", "claude", "antigravity"],
    showAccountsPool: (target) => calls.push(["accounts", target]),
    showProxyPool: (target) => calls.push(["proxy", target]),
  };
  vm.createContext(scope);
  vm.runInContext(app.slice(start, end + 2), scope);
  scope.showPool("claude");
  scope.showPool("unknown");
  assert.deepEqual(calls, [["accounts", "claude"], ["proxy", "claude"]]);
  assert.match(
    app,
    /copy\.onclick = \(\) => showPool\(target\);/,
    "the proxy page's pool buttons select the shared pool",
  );
});

test("A visible meter changes from red to amber without a data refresh", () => {
  const classes = new Set();
  const track = { title: "", classList: {
    toggle(name, enabled) { if (enabled) classes.add(name); else classes.delete(name); }
  } };
  const meterWindows = new WeakMap([[track, {
    usedPercent: 16, windowSeconds: 18000, resetAt: 1795026520
  }]]);
  let now = 1795011414;
  const context = vm.createContext({
    P, meterWindows, document: { hidden: false }, Date: { now: () => now * 1000 },
    $: id => id === "accounts" ? { querySelectorAll: () => [track] } : { hidden: false },
    locale: "en", tr: (key, ...args) => [key, ...args].join(" ")
  });
  vm.runInContext(body("usagePaceTooltip") + body("refreshMeterPace") + body("refreshVisibleMeterPace"), context);
  assert.match(source, /setInterval\(refreshVisibleMeterPace, 1000\)/);
  context.refreshVisibleMeterPace();
  assert.deepEqual([...classes], ["full"]);
  now++;
  context.refreshVisibleMeterPace();
  assert.deepEqual([...classes], ["high"]);
  assert.equal(track.title, "accounts.window.pace_spare 1");
});

test("Antigravity cards show Gemini's windows as the others, and Claude's in an inset", () => {
  const rows = (pool, usage, account = {}) => {
    const el = (tag, className, text) => ({
      className, text, children: [],
      append(...nodes) { this.children.push(...nodes); },
    });
    const context = vm.createContext({
      pool, el,
      tr: (key) => ({ "accounts.window.five_hour": "5h", "accounts.window.one_week": "1 week" })[key],
      usageWindow: (view, title, tone) => [title, tone, view.available],
      // The weekly rows carry the outdated Sub2api version they warn about.
      weeklyNotice: (row, version) => (version ? [...row, version] : row),
    });
    vm.runInContext(body("usageWindows"), context);
    // The inset as its title and its rows; arrays from the context's realm
    // compare as plain values.
    const plain = (node) => Array.isArray(node)
      ? node
      : { [node.className]: node.children.map((child) =>
          Array.isArray(child) ? child : child.children.map((title) => title.text)[0]) };
    return JSON.parse(JSON.stringify(
      context.usageWindows(P.card({ usage, ...account }, false, "used", String)).map(plain),
    ));
  };
  const window = (usedPercent, windowSeconds) => ({ usedPercent, windowSeconds, resetAt: null });
  const gemini = { fiveHour: window(20, 18000), oneWeek: window(5, 604800) };
  const own = [
    ["5h", "five-hour", true],
    ["1 week", "one-week", true],
  ];
  assert.deepEqual(rows("codex", gemini), own);
  assert.deepEqual(rows("antigravity", gemini), own);
  // Codex and Claude have no third-party pool.
  assert.deepEqual(rows("claude", { ...gemini, thirdPartyFiveHour: window(50, 18000) }), own);
  assert.deepEqual(rows("antigravity", { ...gemini, thirdPartyFiveHour: window(50, 18000) }), [
    ...own,
    { "account-member": ["Claude", ["5h", "five-hour", true], ["1 week", "one-week", false]] },
  ]);
  // A Google AI Pro account: Gemini's five hours, and the third-party pool.
  assert.deepEqual(
    rows("antigravity", {
      fiveHour: window(2, 18000),
      thirdPartyFiveHour: window(0, 18000),
      thirdPartyOneWeek: window(0, 604800),
    }),
    [
      ["5h", "five-hour", true],
      ["1 week", "one-week", false],
      { "account-member": ["Claude", ["5h", "five-hour", true], ["1 week", "one-week", true]] },
    ],
  );
  // A card without usage keeps both rows, without data, and no inset.
  assert.deepEqual(rows("antigravity", null), [
    ["5h", "five-hour", false],
    ["1 week", "one-week", false],
  ]);
  // A Sub2api too old for weekly usage: every weekly row says so.
  assert.deepEqual(
    rows(
      "antigravity",
      { fiveHour: window(0, 18000), thirdPartyFiveHour: window(0, 18000) },
      { outdatedSub2apiVersion: " 0.1.177 " },
    ),
    [
      ["5h", "five-hour", true],
      ["1 week", "one-week", false, "0.1.177"],
      { "account-member": ["Claude", ["5h", "five-hour", true], ["1 week", "one-week", false, "0.1.177"]] },
    ],
  );
});

test("A weekly row of an outdated Sub2api opens a notice naming its version", () => {
  const opened = [];
  const node = () => ({
    attributes: {}, children: [], listeners: {},
    append(...nodes) { this.children.push(...nodes); },
    setAttribute(name, value) { this.attributes[name] = value; },
    addEventListener(name, handler) { this.listeners[name] = handler; },
  });
  const context = vm.createContext({
    tr: (key, ...args) => (args.length ? `${key}:${args.join(",")}` : key),
    el: (tag, className) => Object.assign(node(), { tag, className }),
    icon: (name) => ({ glyph: name }),
    showResetPopover: (...args) => opened.push(args),
  });
  vm.runInContext(body("button") + body("weeklyNotice"), context);
  const value = node();
  const row = { querySelector: (selector) => (selector === ".usage-value" ? value : null) };
  // A current Sub2api's row is left as it is.
  assert.equal(context.weeklyNotice(row, null), row);
  assert.equal(value.children.length, 0);
  assert.equal(context.weeklyNotice(row, "0.1.177"), row);
  const [notice] = value.children;
  assert.equal(notice.className, "reset-warning");
  assert.equal(notice.type, "button");
  assert.equal(notice.children[0].glyph, "reset-notice");
  const lines = [
    "accounts.window.sub2api_weekly_unavailable_format:0.1.177",
    "accounts.window.sub2api_weekly_upgrade",
  ];
  assert.equal(notice.attributes["aria-label"], lines.join("\n"));
  assert.equal(notice.attributes["aria-haspopup"], "dialog");
  notice.listeners.click();
  assert.equal(opened.length, 1);
  assert.equal(opened[0][0], notice);
  assert.deepEqual([...opened[0][1]], lines);
  assert.equal(opened[0][2], true);
});
