const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const source = fs.readFileSync(require("node:path").join(__dirname, "../ui/features.js"), "utf8");
const start = source.indexOf("function providerSyncChoices(");
const body = source.slice(start, source.indexOf("\nasync function openProviderImport", start));
test("Provider menu syncs only the chosen saved provider in its pool", () => {
  const calls = [];
  const scope = {
    claudeProvidersSaved: [{ id: "c1", name: "Claude One" }, { id: "c2", name: "Claude Two" }],
    providerController: { persisted: [{ providerID: "Codex One" }, { providerID: "Codex Two" }] },
    syncClaudeSub2api: (id) => calls.push(["claude", id]),
    openProviderImport: (id) => calls.push(["codex", id]),
    extra: (en) => en,
  };
  vm.createContext(scope);
  vm.runInContext(body, scope);
  const claude = scope.providerSyncChoices("claude");
  assert.equal(claude[1].label, "Claude Two");
  claude[1].run();
  scope.providerSyncChoices("codex")[0].run();
  assert.deepEqual(calls, [["claude", "c2"], ["codex", "Codex One"]]);
  scope.claudeProvidersSaved = [];
  assert.equal(scope.providerSyncChoices("claude")[0].disabled, true);
});

test("Import hover suppresses stale focus without moving focus; keyboard restores it", () => {
  const app = fs.readFileSync(require("node:path").join(__dirname, "../ui/app.js"), "utf8");
  const start = app.indexOf("function configureImportMenu(");
  const end = app.indexOf("\nfunction showMenu", start);
  const handlers = {};
  const classes = new Set();
  const menu = {
    classList: { add: (name) => classes.add(name), remove: (name) => classes.delete(name) },
    addEventListener: (name, handler) => { handlers[name] = handler; },
  };
  const scope = {};
  vm.createContext(scope);
  vm.runInContext(app.slice(start, end), scope);
  scope.configureImportMenu(menu);
  assert.ok(classes.has("import-menu"));
  handlers.pointerover();
  assert.ok(classes.has("pointer-navigation"));
  handlers.keydown();
  assert.ok(!classes.has("pointer-navigation"));
  handlers.pointermove();
  assert.ok(classes.has("pointer-navigation"));
});

const appSource = fs.readFileSync(require("node:path").join(__dirname, "../ui/app.js"), "utf8");
const layoutScope = {};
vm.createContext(layoutScope);
vm.runInContext(appSource.slice(appSource.indexOf("function menuPairLayout("), appSource.indexOf("function showMenu(")), layoutScope);
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
      addEventListener() {}
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
    const scope = {
      el: (tag, classes, text) => new Node(tag, classes, text),
      document: { body: new Node("body"), querySelector: () => ({ getBoundingClientRect: () => panel }) },
    };
    vm.createContext(scope);
    vm.runInContext(appSource.slice(appSource.indexOf("let openMenu = null;"), appSource.indexOf('document.addEventListener("pointerdown", (event) => {', appSource.indexOf("let openMenu = null;"))), scope);
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
