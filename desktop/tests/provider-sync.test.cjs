const { test } = require("node:test");
const assert = require("node:assert/strict");
const { Providers } = require("../ui/controllers.js");
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

test("Add unofficial provider offers the pools as children of Sub2API", () => {
  const added = [];
  const items = P.providerAddMenu((key) => key, (pool) => pool, (pool) => added.push(pool));
  assert.equal(items.length, 1);
  assert.equal(items[0].label, "Sub2API");
  assert.deepEqual(Array.from(items[0].children, (child) => child.label), P.ACCOUNT_POOLS.map((pool) => `pool.${pool}`));
  assert.deepEqual(Array.from(items[0].children, (child) => child.icon), P.ACCOUNT_POOLS);
  for (const child of items[0].children) child.run();
  assert.deepEqual(added, P.ACCOUNT_POOLS);
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
    const all = [];
    class Node {
      constructor(tag, classes = "", text = "") {
        this.tag = tag;
        this.classes = classes;
        this.textContent = text;
        this.style = {};
        this.children = [];
        this.attrs = {};
        this.classList = { add() {}, remove() {}, toggle() {} };
        all.push(this);
      }
      append(child) { this.children.push(child); child.parent = this; }
      remove() {
        if (this.parent) this.parent.children = this.parent.children.filter(x => x !== this);
      }
      setAttribute(key, value) { this.attrs[key] = value; }
      addEventListener(name, handler) { (this.listeners ??= {})[name] = handler; }
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
    append(child) { this.children.push(child); child.parent = this; },
    prepend() {},
    remove() { this.removed = true; this.parent.children = this.parent.children.filter((x) => x !== this); },
    setAttribute() {},
    addEventListener(name, handler) { this.listeners[name] = handler; },
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
  menu.listeners.pointerover({ target: other });
  assert.equal(child.removed, undefined, "crossing another row must not immediately close the child");
  child.listeners.pointerenter();
  assert.equal(cancelled, 1, "entering the child or returning to its parent cancels dismissal");
  assert.equal(callback, null);
  assert.equal(child.removed, undefined);
  menu.listeners.pointerover({ target: other });
  callback();
  assert.equal(child.removed, true, "remaining on a different row eventually closes the child");
  child = open();
  menu.listeners.pointerover({ target: other });
  menus.close();
  assert.equal(callback, null, "closing the menu cancels its pending timer");
});
