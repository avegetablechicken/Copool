const { test } = require("node:test");
const assert = require("node:assert/strict");
const { Providers, CliProxyApiProviders } = require("../ui/controllers.js");
const P = require("../ui/presentation.js");
const Menus = require("../ui/menus.js");
test("Import shows provider sync only for pools with saved providers", () => {
  const calls = [];
  const providerControllers = {
    claude: new Providers(null, "claude"),
    codex: new Providers(null, "codex"),
  };
  providerControllers.claude.persisted = [
    { id: "c1", name: "Claude One" },
    { id: "c2", name: "Claude Two" },
  ];
  providerControllers.codex.persisted = [
    { id: "x1", name: "Codex One" },
    { id: "x2", name: "Codex Two" },
  ];
  const menu = (target) =>
    P.importMenu((key) => key, {
      importCurrent() {},
      importFile() {},
      providers: providerControllers[target].syncChoices((key) => calls.push([target, key])),
    });
  let items = menu("claude");
  assert.equal(items[2].label, "accounts.action.import_sub2api");
  assert.equal(items[2].children[1].label, "Claude Two");
  items[2].children[1].run();
  providerControllers.claude.persisted = [];
  assert.deepEqual(Array.from(menu("claude"), (item) => item.label), [
    "accounts.action.import_current_auth", "accounts.action.import_auth_file",
  ]);
  menu("codex")[2].children[0].run();
  // Every pool's cards name the provider by its id.
  assert.deepEqual(calls, [["claude", "c2"], ["codex", "x1"]]);
});

test("Add unofficial provider offers the pools of Sub2api, and CLIProxyAPI for every pool", () => {
  const added = [];
  const items = P.providerAddMenu((key) => key, (pool) => pool, (pool, kind) => added.push([kind, pool]));
  assert.deepEqual(Array.from(items, (item) => item.label), ["Sub2API", "CLIProxyAPI"]);
  const [sub2api, cpa] = items;
  assert.deepEqual(Array.from(sub2api.children, (child) => child.label), P.ACCOUNT_POOLS.map((pool) => `pool.${pool}`));
  assert.deepEqual(Array.from(sub2api.children, (child) => child.icon), P.ACCOUNT_POOLS);
  for (const child of sub2api.children) child.run();
  assert.equal(cpa.children, undefined);
  cpa.run();
  assert.deepEqual(added, [...P.ACCOUNT_POOLS.map((pool) => ["sub2api", pool]), ["cliproxyapi", null]]);
});

test("A pool's forms are its Sub2api providers', signed in by an administrator", () => {
  globalThis.crypto ??= require("node:crypto").webcrypto;
  for (const pool of P.ACCOUNT_POOLS) {
    const providers = new Providers(null, pool);
    const id = providers.add();
    const sub2api = providers.providers.get(id);
    // A Sub2api form is as it was before kinds existed.
    assert.equal("kind" in sub2api, false, pool);
    const mode = pool === "claude" ? "address" : null;
    const fields = providers.fields(sub2api, mode);
    assert.ok(fields.some(([key]) => key === "username"), pool);
    assert.ok(fields.every(([, label]) => label.startsWith("settings.sub2api.")), pool);
    assert.ok(fields.every(([, , , keep]) => keep === "settings.sub2api.password_keep"), pool);
    assert.equal(providers.savedNotice(), "settings.notice.sub2api_saved");
    // A CLIProxyAPI provider's cards show here, but its form is every pool's.
    providers.accept([{ id: "cpa", kind: "cliproxyapi", name: "Relay" }]);
    assert.deepEqual(providers.forms().map(([key]) => key), [id], pool);
    assert.deepEqual(providers.syncChoices(() => {}).map((c) => c.label), ["Relay"]);
  }
});

test("A CLIProxyAPI form is every pool's: what they share, each pool's switch, and its key kept", async () => {
  globalThis.crypto ??= require("node:crypto").webcrypto;
  const calls = [];
  let saved = null;
  const invoke = async (name, args) => {
    calls.push([name, structuredClone(args)]);
    if (name === "cliproxyapi_providers") return saved ? [saved] : [];
    if (name === "save_cliproxyapi_provider") {
      saved = { ...args.value, id: args.value.id || "cpa-1" };
      delete saved.password;
      return saved;
    }
  };
  const providers = new CliProxyApiProviders(invoke);
  await providers.load();
  assert.deepEqual(providers.fields().map(([key]) => key), ["name", "adminBaseURL", "password"]);
  assert.equal(providers.fields()[2][1], "settings.cliproxyapi.management_key");
  // Codex switches to the provider of its name in config.toml.
  assert.deepEqual(providers.poolFields().map(([pool]) => pool), ["claude", "antigravity", "grok"]);
  const id = providers.add();
  const form = providers.providers.get(id);
  assert.equal(form.kind, "cliproxyapi");
  Object.assign(form, { name: "Relay", adminBaseURL: "http://a:8317", password: "key" });
  form.pools.grok.apiKeyVariable = "CPA_KEY";
  await providers.save(id);
  const [, { value }] = calls.find(([name]) => name === "save_cliproxyapi_provider");
  // A new one is named by the core.
  assert.equal(value.id, "");
  assert.equal(value.password, "key");
  assert.deepEqual(value.pools.grok, { baseURL: "", apiKeyVariable: "CPA_KEY" });
  assert.deepEqual([...providers.providers.keys()], ["cpa-1"]);
  // Saved, its key is blank in the form, which keeps it.
  assert.equal(providers.providers.get("cpa-1").password, "");
  assert.equal(providers.providers.get("cpa-1").pools.grok.apiKeyVariable, "CPA_KEY");
  assert.equal(providers.isSaved("cpa-1"), true);
  assert.equal(providers.notice, "settings.notice.cliproxyapi_saved");
  await providers.remove("cpa-1");
  assert.deepEqual(calls.at(-1), ["remove_cliproxyapi_provider", { id: "cpa-1" }]);
  assert.equal(providers.providers.size, 0);
});

test("A failed CLIProxyAPI save shows what it left saved", async () => {
  globalThis.crypto ??= require("node:crypto").webcrypto;
  let listed = [];
  const providers = new CliProxyApiProviders(async (name) => {
    if (name === "cliproxyapi_providers") return listed;
    listed = [{ id: "half", kind: "cliproxyapi", name: "Relay", pools: {} }];
    throw new Error("disk full");
  });
  const id = providers.add();
  await assert.rejects(providers.save(id), /disk full/);
  assert.equal(providers.isSaved("half"), true);
  assert.ok(providers.providers.has("half"));
  assert.equal(providers.notice, "disk full");
});

test("A provider card without its provider's name is titled by its kind", () => {
  const providers = new Providers(null, "claude");
  const card = (kind) => ({ id: "claude-sub2api-p-1", providerKind: kind, refresh: { key: "sub2api:p" } });
  providers.persisted = [];
  assert.equal(providers.cardTitle(card("sub2api")), "p");
  const unnamed = { id: "x", providerKind: "cliproxyapi" };
  assert.equal(providers.cardTitle(unnamed), "CLIProxyAPI");
  assert.equal(providers.cardTitle({ id: "x", providerKind: "sub2api" }), "Sub2api");
  assert.equal(P.isProviderCard(unnamed), true);
  assert.equal(P.isProviderCard({ id: "x" }), false);
});

test("Menu hover suppresses stale focus without moving focus; keyboard restores it", () => {
  const handlers = {};
  const classes = new Set();
  const menu = {
    classList: { add: (name) => classes.add(name), remove: (name) => classes.delete(name) },
    addEventListener: (name, handler) => { handlers[name] = handler; },
  };
  const scope = Menus;
  scope.configureMenuNavigation(menu);
  handlers.pointerover();
  assert.ok(classes.has("pointer-navigation"));
  handlers.keydown();
  assert.ok(!classes.has("pointer-navigation"));
  handlers.pointermove();
  assert.ok(classes.has("pointer-navigation"));
  const childClasses = new Set();
  const childHandlers = {};
  const child = {
    classList: { add: (name) => childClasses.add(name), remove: (name) => childClasses.delete(name) },
    addEventListener: (name, handler) => { childHandlers[name] = handler; },
  };
  const group = [menu, child];
  scope.configureMenuNavigation(menu, group);
  scope.configureMenuNavigation(child, group);
  handlers.pointerover();
  childHandlers.pointerover();
  assert.ok(classes.has("inactive-menu"), "hovering Add time clears the parent highlight");
  assert.ok(classes.has("pointer-navigation"));
  assert.ok(!childClasses.has("inactive-menu"));
  childHandlers.keydown();
  assert.ok(!classes.has("pointer-navigation"));
  assert.ok(!childClasses.has("pointer-navigation"));
  childHandlers.focusin();
  assert.ok(classes.has("inactive-menu"), "keyboard focus in the child suppresses parent hover");
  handlers.focusin();
  assert.ok(!classes.has("inactive-menu"));
  assert.ok(childClasses.has("inactive-menu"), "returning focus restores only the parent");
});

const layoutScope = Menus;
for (const [name, panel, parentWidth, childWidth, preferred] of [
  ["Codex", { left: 0, right: 640 }, 260, 150, 30],
  ["long Claude label", { left: 0, right: 460 }, 420, 300, 35],
  ["narrow panel and long provider", { left: 12, right: 332 }, 400, 500, 80],
]) {
  test(`Submenu stays beside its parent inside the panel: ${name}`, () => {
    const layout = layoutScope.menuPairLayout(panel, parentWidth, childWidth, preferred);
    const childLeft = layout.left + layout.parentWidth - 1;
    assert.equal(layout.left, preferred);
    assert.ok(layout.left >= panel.left + 8);
    assert.ok(childLeft + layout.submenuWidth <= panel.right - 8);
    assert.ok(layout.parentWidth > 0 && layout.submenuWidth > 0);
    if (name === "Codex") {
      assert.equal(layout.left, preferred);
      assert.equal(layout.parentWidth, parentWidth);
      assert.equal(layout.submenuWidth, childWidth);
    }
  });
}

test("Different pool labels and provider counts produce the same menu origins", () => {
  function render(label, providers, right = 360) {
    const panel = { left: 0, right, top: 0, bottom: 500 };
    class Node {
      constructor(tag, classes = "", text = "") {
        this.tag = tag;
        this.classes = classes;
        this.textContent = text;
        this.style = {};
        this.children = [];
        this.attrs = {};
        this.classList = { add() {}, remove() {}, toggle() {} };
      }
      append(child) { this.children.push(child); child.parent = this; }
      remove() {
        if (this.parent) this.parent.children = this.parent.children.filter(x => x !== this);
      }
      setAttribute(key, value) { this.attrs[key] = value; }
      addEventListener(name, handler) { ((this.listeners ??= {})[name] ??= []).push(handler); }
      querySelector(selector) {
        return selector.includes("aria-checked") ? null : this.children.find(x => x.tag === "button");
      }
      focus() { this.onfocus?.(); }
      get offsetWidth() {
        const natural = Math.max(this.classes.includes("provider-submenu") ? 0 : 150,
          ...this.children.map(x => x.textContent.length * 6 + 28));
        return parseFloat(this.style.width) || Math.min(natural, parseFloat(this.style.maxWidth) || Infinity);
      }
      get offsetHeight() { return this.children.length * 23 + 8; }
      getBoundingClientRect() {
        if (this.tag === "button") {
          const parent = this.parent.getBoundingClientRect();
          return { ...parent, top: parent.top + 4 + this.parent.children.indexOf(this) * 23 };
        }
        const left = parseFloat(this.style.left) || 0;
        const top = parseFloat(this.style.top) || 0;
        return { left, right: left + this.offsetWidth, top, bottom: top + this.offsetHeight };
      }
    }
    const document = { body: new Node("body"), querySelector: () => ({ getBoundingClientRect: () => panel }) };
    const menus = Menus.create({ document, el: (tag, classes, text) => new Node(tag, classes, text) });
    const scope = { document, showMenu: menus.show };
    const trigger = new Node("button");
    trigger.id = "import-open";
    trigger.getBoundingClientRect = () => ({ left: 24, top: 40, bottom: 62 });
    scope.showMenu(trigger, [
      { label, run() {} }, { label: "Import JSON file", run() {} },
      { label: "Sync unofficial providers", children: providers.map(label => ({ label, run() {} })) },
    ]);
    const parent = scope.document.body.children[0];
    assert.equal(parent.style.left, "24px", "menu left must align with the button");
    assert.equal(parent.style.top, "66px", "menu must open directly below the button");
    parent.children[2].onpointerenter();
    const child = scope.document.body.children[1];
    assert.equal(child.style.width, undefined, "provider submenu must not have a forced width");
    if (providers[0] === "team")
      assert.equal(child.offsetWidth, 52, "short provider labels keep a compact submenu");
    assert.ok(child.getBoundingClientRect().right <= panel.right - 8);
    assert.equal(child.getBoundingClientRect().left, parent.getBoundingClientRect().right - 1);
    return [parent.style.left, parent.style.top, parent.style.width, child.style.left, child.style.top];
  }
  for (const width of [320, 360, 640]) {
    assert.deepEqual(
      render("Import current auth", ["team"], width),
      render("Import current Claude Code login", Array(30).fill("A very long provider name"), width),
    );
  }
});

test("Card menus retain room for their labels at the right edge", () => {
  for (const panel of [{ left: 0, right: 320 }, { left: 24, right: 484 }]) {
    for (const width of [150, 210, 280]) {
      const left = layoutScope.menuLeft(panel, width, panel.right - 24);
      assert.ok(left >= panel.left + 8);
      assert.ok(left + width <= panel.right - 8);
      assert.equal(left + width, panel.right - 8, "move the full-width menu inward");
      const childWidth = 150;
      const preferred = left - childWidth + 1;
      const childLeft = layoutScope.menuLeft(panel, childWidth, preferred);
      assert.ok(childLeft >= panel.left + 8);
      assert.ok(childLeft + childWidth <= panel.right - 8);
    }
  }
});

test("Crossing parent rows toward Add time can cancel pending dismissal", () => {
  let callback;
  let cancelled = 0;
  const timers = {
    setTimeout(fn, delay) { assert.equal(delay, 350); callback = fn; return 1; },
    clearTimeout() { cancelled++; callback = null; },
  };
  // Menus with rows that know their own button and a body that keeps them.
  const node = (tag) => ({
    tag, style: {}, children: [], listeners: {}, offsetWidth: 100, offsetHeight: 40,
    classList: { add() {}, remove() {}, toggle() {} },
    append(child) { this.children.push(child); child.parent = this; child.removed = undefined; },
    prepend() {},
    remove() { this.removed = true; this.parent.children = this.parent.children.filter((x) => x !== this); },
    setAttribute() {},
    // Every handler is kept and run, as the DOM does.
    addEventListener(name, handler) { (this.listeners[name] ??= []).push(handler); },
    fire(name, event = {}) { for (const handler of this.listeners[name] ?? []) handler(event); },
    querySelector() { return null; },
    contains: () => false,
    closest() { return this.tag === "button" ? this : null; },
    focus() {},
    getBoundingClientRect: () => ({ left: 0, right: 100, top: 0, bottom: 40 }),
  });
  const panel = { left: 0, right: 400, top: 0, bottom: 400 };
  const document = { body: node("body"), querySelector: () => ({ getBoundingClientRect: () => panel }) };
  const menus = Menus.create({ document, el: (tag) => node(tag), timers });
  menus.show(node("button"), [
    { label: "Every day", run() {} },
    { label: "Times", children: [{ label: "Add time", run() {} }] },
  ]);
  const [menu] = document.body.children;
  const [other, parent] = menu.children;
  const open = () => {
    cancelled = 0;
    parent.onpointerenter();
    return document.body.children[1];
  };
  let child = open();
  menu.fire("pointerover", { target: other });
  assert.equal(child.removed, undefined, "crossing another row must not immediately close the child");
  child.fire("pointerenter");
  assert.equal(cancelled, 1, "entering the child or returning to its parent cancels dismissal");
  assert.equal(callback, null);
  assert.equal(child.removed, undefined);
  menu.fire("pointerover", { target: other });
  callback();
  assert.equal(child.removed, true, "remaining on a different row eventually closes the child");
  child = open();
  menu.fire("pointerover", { target: other });
  menus.close();
  assert.equal(callback, null, "closing the menu cancels its pending timer");
});

test("The arrows reach a menu's first row down and its last up when none is focused", () => {
  const document = { activeElement: null };
  const node = (tag) => ({
    tag, style: {}, children: [], listeners: {}, offsetWidth: 100, offsetHeight: 40, disabled: false,
    classList: { add() {}, remove() {}, toggle() {} },
    append(...children) { for (const child of children) { this.children.push(child); child.parent = this; } },
    prepend() {},
    remove() { this.parent.children = this.parent.children.filter((x) => x !== this); },
    setAttribute() {},
    getAttribute: () => null,
    addEventListener(name, handler) { (this.listeners[name] ??= []).push(handler); },
    buttons() { return this.children.flatMap((c) => (c.tag === "button" ? [c] : c.buttons?.() ?? [])); },
    querySelector(selector) { return selector.startsWith("button") ? this.buttons()[0] ?? null : null; },
    querySelectorAll(selector) { return selector === "button" ? this.buttons() : []; },
    contains(other) { return other === this || this.children.some((c) => c.contains?.(other)); },
    closest() { return null; },
    focus() { document.activeElement = this; },
    getBoundingClientRect: () => ({ left: 0, right: 100, top: 0, bottom: 40 }),
  });
  const panel = { left: 0, right: 400, top: 0, bottom: 400 };
  Object.assign(document, { body: node("body"), querySelector: () => ({ getBoundingClientRect: () => panel }) });
  const menus = Menus.create({ document, el: (tag) => node(tag), timers: { setTimeout() {}, clearTimeout() {} } });
  menus.show(node("button"), [
    { label: "First", run() {} },
    { label: "Second", run() {} },
    { label: "Last", run() {} },
  ]);
  const rows = document.body.children[0].buttons();
  assert.equal(rows.length, 3);
  const key = (name) => menus.handleKey({ key: name, preventDefault() {} });
  document.activeElement = null;
  key("ArrowUp");
  assert.equal(document.activeElement, rows[2]);
  document.activeElement = null;
  key("ArrowDown");
  assert.equal(document.activeElement, rows[0]);
  key("ArrowUp");
  assert.equal(document.activeElement, rows[2]);
});
