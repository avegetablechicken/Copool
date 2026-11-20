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

test("Cmd+1 to Cmd+3 switch the proxy page's pool there, and the accounts page's elsewhere", () => {
  const app = fs
    .readFileSync(path.join(__dirname, "../ui/app.js"), "utf8")
    .replace(/\r\n/g, "\n");
  const start = app.indexOf('  } else if (mod && !event.altKey && !event.shiftKey && ["1", "2", "3"]');
  const end = app.indexOf('  } else if (mod && event.key === ",") {', start);
  assert(start >= 0 && end > start, "pool shortcut branch found");
  for (const [page, key, expected] of [
    ["proxy", "2", [["proxy", "claude"]]],
    ["proxy", "1", [["proxy", "codex"]]],
    ["accounts", "2", [["accounts", "claude"]]],
    ["settings", "1", [["page", "accounts"], ["accounts", "codex"]]],
    // Antigravity has no proxy: its shortcut shows its accounts.
    ["proxy", "3", [["page", "accounts"], ["accounts", "antigravity"]]],
    ["accounts", "3", [["accounts", "antigravity"]]],
  ]) {
    const calls = [];
    const scope = {
      ACCOUNT_POOLS: ["codex", "claude", "antigravity"],
      POOLS: ["codex", "claude"],
      selectedPage: page,
      showProxyPool: (target) => calls.push(["proxy", target]),
      showPool: (target) => calls.push(["accounts", target]),
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

test("Antigravity cards name their pools and show the windows the account reports", () => {
  const rows = (pool, usage) => {
    const context = vm.createContext({
      pool,
      tr: (key) => ({ "accounts.window.five_hour": "5h", "accounts.window.one_week": "1 week" })[key],
      usageWindow: (view, title, tone) => [title, tone, view.available],
    });
    vm.runInContext(body("usageWindows"), context);
    // Arrays from the context's realm compare as plain values.
    return JSON.parse(JSON.stringify(context.usageWindows(P.card({ usage }, false, "used", String))));
  };
  const window = (usedPercent, windowSeconds) => ({ usedPercent, windowSeconds, resetAt: null });
  const gemini = { fiveHour: window(20, 18000), oneWeek: window(5, 604800) };
  assert.deepEqual(rows("codex", gemini), [
    ["5h", "five-hour", true],
    ["1 week", "one-week", true],
  ]);
  assert.deepEqual(rows("antigravity", gemini), [
    ["Gemini 5h", "five-hour", true],
    ["Gemini 1 week", "one-week", true],
  ]);
  assert.deepEqual(
    rows("antigravity", { ...gemini, thirdPartyFiveHour: window(50, 18000) }).map(([title]) => title),
    ["Gemini 5h", "Gemini 1 week", "Claude 5h"],
  );
  // A Google AI Pro account: Gemini's five hours, and Claude's pool.
  assert.deepEqual(
    rows("antigravity", {
      fiveHour: window(2, 18000),
      thirdPartyFiveHour: window(0, 18000),
      thirdPartyOneWeek: window(0, 604800),
    }).map(([title]) => title),
    ["Gemini 5h", "Claude 5h", "Claude 1 week"],
  );
  // Gemini's five hours stay, without data, until a refresh reports them.
  assert.deepEqual(
    rows("antigravity", {
      thirdPartyFiveHour: window(0, 18000),
      thirdPartyOneWeek: window(0, 604800),
    }),
    [
      ["Gemini 5h", "five-hour", false],
      ["Claude 5h", "five-hour", true],
      ["Claude 1 week", "one-week", true],
    ],
  );
  assert.deepEqual(rows("antigravity", { fiveHour: window(2, 18000) }), [
    ["Gemini 5h", "five-hour", true],
  ]);
  // A card without usage keeps both Gemini rows, without data.
  assert.deepEqual(rows("antigravity", null), [
    ["Gemini 5h", "five-hour", false],
    ["Gemini 1 week", "one-week", false],
  ]);
});
