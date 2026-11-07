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
