const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const P = require("../ui/presentation.js");
const V = require("../ui/card-view.js");
const { Accounts } = require("../ui/controllers.js");

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
    V,
    locale: "en",
    tr: (key, ...args) => (args.length ? `${key}:${args.join(",")}` : key),
  };
  vm.createContext(context);
  vm.runInContext(body("refreshTooltip"), context);
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

// A card's or provider's next refresh is card-view.js's (card-view.test.cjs).
test("The toolbar follows the full refresh", () => {
  // The toolbar shows the background's own schedule, not the cards'.
  assert.match(body("renderToolbar"), /refreshTooltip\([\s\S]*\$\("refresh-all"\)[\s\S]*\(\) => fullRefreshAt/);
  assert.match(body("refreshView"), /invoke\("next_full_refresh_at", \{ pool \}\)/);
});

test("Refreshing a provider's only account refreshes the entire provider", () => {
  const calls = [];
  const context = {
    accountsController: { refresh: (id) => { calls.push(["account", id]); return Promise.resolve(false); } },
    refreshProvider: (group) => calls.push(["provider", group.key, group.accounts[0].id]),
    refreshView: () => Promise.resolve(),
    notice: () => {},
    providerKey: P.providerKey,
  };
  // The cards shown, with the refresh keys the core gives them.
  context.accountsCache = [
    { id: "claude-sub2api-team-7", refresh: { key: "sub2api:team" } },
    { id: "sub2api-work-42", refresh: { key: "sub2api:work" } },
    { id: "official", refresh: { key: "official" } },
  ];
  vm.createContext(context);
  vm.runInContext(body("refreshUsage"), context);
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

test("Cmd+1 to Cmd+4 select the pool both pages share, in place on the proxy page but for Grok", () => {
  const app = fs
    .readFileSync(path.join(__dirname, "../ui/app.js"), "utf8")
    .replace(/\r\n/g, "\n");
  const start = app.indexOf('  } else if (mod && !event.altKey && !event.shiftKey && ["1", "2", "3", "4"]');
  const end = app.indexOf('  } else if (mod && event.key === ",") {', start);
  assert(start >= 0 && end > start, "pool shortcut branch found");
  for (const [page, key, expected] of [
    ["proxy", "2", [["pool", "claude"]]],
    ["proxy", "1", [["pool", "codex"]]],
    ["accounts", "2", [["pool", "claude"]]],
    ["settings", "1", [["page", "accounts"], ["pool", "codex"]]],
    ["proxy", "3", [["pool", "antigravity"]]],
    ["accounts", "3", [["pool", "antigravity"]]],
    // Grok has no proxy page: its accounts are shown.
    ["proxy", "4", [["page", "accounts"], ["pool", "grok"]]],
    ["accounts", "4", [["pool", "grok"]]],
  ]) {
    const calls = [];
    const scope = {
      ACCOUNT_POOLS: ["codex", "claude", "antigravity", "grok"],
      PROXY_POOLS: ["codex", "claude", "antigravity"],
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

test("Selecting a pool on either page selects it on both, but Grok only on the accounts page", () => {
  const app = fs
    .readFileSync(path.join(__dirname, "../ui/app.js"), "utf8")
    .replace(/\r\n/g, "\n");
  const start = app.indexOf("function showPool(next) {");
  const end = app.indexOf("\n}\n", start);
  assert(start >= 0 && end > start, "showPool found");
  const calls = [];
  const scope = {
    ACCOUNT_POOLS: ["codex", "claude", "antigravity", "grok"],
    PROXY_POOLS: ["codex", "claude", "antigravity"],
    showAccountsPool: (target) => calls.push(["accounts", target]),
    showProxyPool: (target) => calls.push(["proxy", target]),
  };
  vm.createContext(scope);
  vm.runInContext(app.slice(start, end + 2), scope);
  scope.showPool("claude");
  scope.showPool("unknown");
  scope.showPool("grok");
  assert.deepEqual(calls, [["accounts", "claude"], ["proxy", "claude"], ["accounts", "grok"]]);
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
    P, V, meterWindows, document: { hidden: false }, Date: { now: () => now * 1000 },
    $: id => id === "accounts" ? { querySelectorAll: () => [track] } : { hidden: false },
    locale: "en", tr: (key, ...args) => [key, ...args].join(" ")
  });
  vm.runInContext(body("refreshMeterPace") + body("refreshVisibleMeterPace"), context);
  assert.match(source, /setInterval\(refreshVisibleMeterPace, 1000\)/);
  context.refreshVisibleMeterPace();
  assert.deepEqual([...classes], ["full"]);
  now++;
  context.refreshVisibleMeterPace();
  assert.deepEqual([...classes], ["high"]);
  assert.equal(track.title, "accounts.window.pace_spare 1");
});

// Which rows a card shows, its own or its third-party pool's, is
// card-view.js's (card-view.test.cjs); here, its model switch.
test("A card's model switch stacks both marks and switches the rows it shows", () => {
  const swapped = "let modelsSwapped = null;\n";
  assert(source.includes(swapped), "modelsSwapped");
  const node = (className, text) => ({
    className, text, children: [], dataset: {}, attributes: {}, listeners: {},
    append(...nodes) { this.children.push(...nodes); },
    setAttribute(name, value) { this.attributes[name] = value; },
    addEventListener(name, handler) { this.listeners[name] = handler; },
  });
  // The pool's controller keeps the choice and renders the cards anew.
  const controller = new Accounts(async () => [], [], () => context.renderAccounts(), () => {}, { pool: "antigravity" });
  const context = vm.createContext({
    accountsController: controller,
    renders: 0,
    el: (tag, className, text) => node(className, text),
    button: (label, glyph, className, handler) => {
      const option = node(className, label);
      option.addEventListener("click", handler);
      return option;
    },
    modelBrand: (model) => `${model} mark`,
  });
  vm.runInContext(
    swapped + "function renderAccounts() { renders++; }\n" + body("modelSwitch"),
    context,
  );
  const window = (usedPercent, windowSeconds) => ({ usedPercent, windowSeconds, resetAt: null });
  const pro = { fiveHour: window(2, 18000), thirdPartyFiveHour: window(12, 18000), thirdPartyOneWeek: window(3, 604800) };
  const view = (id) => V.cardView({ id, usage: pro }, {
    pool: "antigravity",
    locale: "en",
    tr: (key) => key,
    ui: (card) => controller.cardState(card),
  });
  // The switch as the name of the model it shows, bracketed when that is the
  // third party's; the other model's mark sits behind the shown one's.
  const models = (id = "a") => {
    const control = context.modelSwitch(view(id), id);
    const [behind, front] = control.children;
    assert.equal(control.children.length, 2);
    assert.equal(behind.className, "model-mark behind");
    assert.equal(front.className, "model-mark front");
    assert.deepEqual(front.children, [`${control.title} mark`]);
    assert.deepEqual(behind.children, [`${control.title === "Claude" ? "Gemini" : "Claude"} mark`]);
    assert.equal(control.attributes["aria-label"], control.title);
    assert.equal(control.dataset.modelSwitch, id);
    return control.attributes["aria-pressed"] === "true" ? `[${control.title}]` : control.title;
  };
  assert.equal(models(), "Gemini");
  assert.equal(context.modelSwitch(V.cardView({ id: "c", usage: null }, { pool: "antigravity", tr: String }), "c"), null);
  // A click switches to Claude; the switch rendered by its own click trades
  // its marks' places.
  context.renderAccounts = () => {
    context.renders++;
    assert.equal(context.modelSwitch(view("a"), "a").className, "model-switch swapped");
    assert.equal(context.modelSwitch(view("b"), "b").className, "model-switch");
  };
  context.modelSwitch(view("a"), "a").listeners.click();
  assert.equal(context.renders, 1);
  assert.equal(models(), "[Claude]");
  assert.equal(models("b"), "Gemini");
  assert.equal(context.modelSwitch(view("a"), "a").className, "model-switch");
  // Another click switches back to Gemini.
  context.modelSwitch(view("a"), "a").listeners.click();
  assert.equal(context.renders, 2);
  assert.equal(models(), "Gemini");
  // A switched card renders again.
  assert.match(body("renderAccounts"), /accountsController\.cardState\(account\.id\)/);
});

test("A render slides each card's meters from the levels they showed, changing color as they go", () => {
  // A fill on a 200px track, drawn at its width unless an animation holds it.
  const fill = (width, hidden = false, background = "gray", opacity = "0.55", drawn = width) => ({
    hidden, background, opacity, style: { width }, animations: [],
    parentElement: { getBoundingClientRect: () => ({ width: 200 }) },
    getBoundingClientRect: () => ({ width: hidden ? 0 : parseFloat(drawn) * 2 }),
    animate(frames, options) {
      const animation = { frames, options };
      this.animations.push(animation);
      return animation;
    },
  });
  const card = (id, fills) => ({
    dataset: { cardId: id },
    querySelectorAll: (selector) => {
      assert.equal(selector, ":scope > .usage .meter-fill");
      return fills;
    },
  });
  const list = (...cards) => ({
    querySelectorAll: (selector) => {
      assert.equal(selector, "[data-card-id]");
      return cards;
    },
  });
  const context = vm.createContext({
    reduced: false,
    getComputedStyle: (node) => ({ backgroundColor: node.background, opacity: node.opacity }),
  });
  context.window = { matchMedia: () => ({ matches: context.reduced }) };
  vm.runInContext(body("meterStates") + body("cardMeters") + body("slideMeters"), context);
  const frames = (node) => JSON.parse(JSON.stringify(node.animations.map((a) => a.frames)));

  // Levels are what the meters show, partway through a slide too; a card
  // without usage rows, as one turned to its back, has none.
  const before = context.cardMeters(list(
    card("a", [fill("2%"), fill("18%", false, "gray", "0.55", "30%"), fill("40%")]),
    card("b", [fill("0%", true)]),
    card("c", []),
  ));
  assert.deepEqual([...before.keys()], ["a", "b"]);
  assert.deepEqual([...before.get("a")].map(({ level }) => level), [2, 30, 40]);
  assert.deepEqual([...before.get("b")].map(({ level }) => level), [0]);
  // Card a's Session fills up into the warning color, its Weekly empties
  // out, a row new to it rises from empty and a row left as it was stays
  // still; b's meter fills from empty; c, which showed no usage rows, and
  // d, new, appear as they are.
  const a = [fill("92%", false, "orange", "0.9"), fill("0%", true), fill("40%"), fill("5%")];
  const b = [fill("10%")];
  const c = [fill("50%")];
  const d = [fill("60%")];
  context.slideMeters(list(card("a", a), card("b", b), card("c", c), card("d", d)), before);
  assert.deepEqual(frames(a[0]), [[
    { width: "2%", backgroundColor: "gray", opacity: "0.55" },
    { width: "92%", backgroundColor: "orange", opacity: "0.9" },
  ]]);
  assert.deepEqual(frames(a[1]), [[
    { width: "30%", backgroundColor: "gray", opacity: "0.55" },
    { width: "0%", backgroundColor: "gray", opacity: "0.55" },
  ]]);
  // An emptied meter shows while it drains, and hides once it has.
  assert.equal(a[1].hidden, false);
  a[1].animations[0].onfinish();
  assert.equal(a[1].hidden, true);
  assert.equal(a[2].animations.length, 0);
  assert.deepEqual(frames(a[3]), [[
    { width: "0%", backgroundColor: "gray", opacity: "0.55" },
    { width: "5%", backgroundColor: "gray", opacity: "0.55" },
  ]]);
  assert.deepEqual(frames(b[0]), [[
    { width: "0%", backgroundColor: "gray", opacity: "0.55" },
    { width: "10%", backgroundColor: "gray", opacity: "0.55" },
  ]]);
  assert.equal(c[0].animations.length + d[0].animations.length, 0);

  // Reduced motion changes them at once.
  context.reduced = true;
  const still = [fill("60%")];
  context.slideMeters(list(card("a", still)), before);
  assert.equal(still[0].animations.length, 0);

  // Every render of the same pool's cards slides them, the model switch's
  // and a usage refresh's alike; another pool's cards appear as they are.
  const render = body("renderAccounts");
  const captured = render.indexOf("const meters = renderedLayout?.pool === pool ? cardMeters(list) : new Map();");
  const cleared = render.indexOf("list.replaceChildren();");
  const aligned = render.indexOf("alignMeters(list);");
  const slid = render.indexOf("slideMeters(list, meters);");
  assert(captured >= 0 && captured < cleared, "meters captured before the cards are replaced");
  assert(aligned >= 0 && aligned < slid, "meters slide once aligned");
  assert(!body("modelSwitch").includes("slideMeters"));
});

// Its lines, naming the version and the pool's upgrade, are card-view.js's.
test("A weekly row of an outdated Sub2api opens a notice of its lines", () => {
  const opened = [];
  const node = () => ({
    attributes: {}, children: [], listeners: {},
    append(...nodes) { this.children.push(...nodes); },
    setAttribute(name, value) { this.attributes[name] = value; },
    addEventListener(name, handler) { this.listeners[name] = handler; },
  });
  const context = vm.createContext({
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
  const lines = [
    "accounts.window.sub2api_weekly_unavailable_format:0.1.177",
    "accounts.window.sub2api_weekly_upgrade",
  ];
  assert.equal(context.weeklyNotice(row, lines), row);
  const [notice] = value.children;
  assert.equal(notice.className, "reset-warning");
  assert.equal(notice.type, "button");
  assert.equal(notice.children[0].glyph, "reset-notice");
  assert.equal(notice.attributes["aria-label"], lines.join("\n"));
  assert.equal(notice.attributes["aria-haspopup"], "dialog");
  notice.listeners.click();
  assert.equal(opened.length, 1);
  assert.equal(opened[0][0], notice);
  assert.deepEqual([...opened[0][1]], lines);
  assert.equal(opened[0][2], true);
});
