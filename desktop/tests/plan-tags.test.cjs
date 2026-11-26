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

/// The app's tag and control functions, drawing plain nodes and recording
/// its sign-ins.
function app() {
  const node = (className, text) => ({
    className, text, children: [], dataset: {}, attributes: {}, listeners: {},
    append(...nodes) { this.children.push(...nodes); },
    setAttribute(name, value) { this.attributes[name] = value; },
    addEventListener(name, handler) { this.listeners[name] = handler; },
  });
  const scope = {
    P,
    pool: "codex",
    collapsed: false,
    signIns: [],
    accountsController: {
      refreshingIds: new Set(),
      remoteRefreshingIds: new Set(),
      adding: false,
      authorizing: null,
    },
    el: (tag, className, text) => node(className, text),
    button: (label, glyph, className, handler) => {
      const control = node(className, label);
      control.glyph = glyph;
      control.addEventListener("click", handler);
      return control;
    },
    notice: () => {},
    tr: (key) => key,
  };
  vm.createContext(scope);
  vm.runInContext(
    "function signIn(...args) { signIns.push(args); }\n" +
      body("planTag") +
      body("setPlanAccent") +
      body("accountTags") +
      body("accountControls"),
    scope,
  );
  scope.node = node;
  return scope;
}
/// A card's tags as [class, text], the empty plan tag left out.
const tags = (scope, account, grouped = false) =>
  scope
    .accountTags(account, P.card(account), grouped)
    .children.filter((tag) => tag !== "")
    .map((tag) => [tag.className, tag.text]);

test("A card's plan tag shows the label and color the core gives it", () => {
  const scope = app();
  const account = { id: "a", planType: "pro", planTag: { label: "PRO 20X", accent: "orange" } };
  const tag = scope.planTag(account);
  assert.equal(tag.className, "tag plan");
  assert.equal(tag.text, "PRO 20X");
  const card = scope.node("block account");
  scope.setPlanAccent(card, account);
  assert.equal(card.dataset.accent, "orange");
  // Whatever the raw plan, its label is the core's.
  const claude = { id: "c", planType: "max 20x", planTag: { label: "ULTRA", accent: "orange" } };
  assert.deepEqual(tags(scope, claude), [["tag plan", "ULTRA"]]);
});

test("A card without a plan tag shows none and no color, in every pool", () => {
  const scope = app();
  for (const pool of ["codex", "claude", "antigravity", "grok"]) {
    scope.pool = pool;
    // The app no longer names a plan of its own, a raw one included.
    for (const account of [{ id: "a" }, { id: "b", planType: "pro", planTag: null }]) {
      assert.equal(scope.planTag(account), "", pool);
      const card = scope.node("block account");
      scope.setPlanAccent(card, account);
      assert.equal(card.dataset.accent, undefined, pool);
      assert.deepEqual(tags(scope, account), [], pool);
    }
  }
});

test("A card's workspace tag is the core's, hidden in a provider's group", () => {
  const scope = app();
  const account = {
    id: "a",
    teamName: "workspace-a",
    planTag: { label: "BUSINESS", accent: "indigo" },
    workspaceTag: "workspace-a",
  };
  assert.deepEqual(tags(scope, account), [
    ["tag plan", "BUSINESS"],
    ["tag muted", "workspace-a"],
  ]);
  // A team the core leaves unnamed is not shown.
  const personal = { ...account, planTag: { label: "PRO", accent: "pink" }, workspaceTag: undefined };
  assert.deepEqual(tags(scope, personal), [["tag plan", "PRO"]]);
  // A provider's group carries its SUB2API tag once.
  const member = { ...account, sourceTag: "SUB2API", workspaceTag: "SUB2API" };
  assert.deepEqual(tags(scope, member), [
    ["tag plan", "BUSINESS"],
    ["tag muted", "SUB2API"],
  ]);
  assert.deepEqual(tags(scope, member, true), [["tag plan", "BUSINESS"]]);
});

test("Sign in again pins the workspace only for a card with a workspace tag", () => {
  const scope = app();
  const signInAgain = (account) => {
    const view = P.card(account);
    const controls = scope.accountControls(account, account.email, view);
    const control = controls.children.find((c) => c.glyph === "reauthenticate");
    assert(control, "Sign in again shown");
    control.listeners.click();
    return [...scope.signIns.pop()];
  };
  const expired = {
    id: "a",
    accountId: "workspace-1",
    email: "dev@example.com",
    planType: "team",
    teamName: "Acme",
    usageError: "error.accounts.sign_in_expired",
  };
  assert.deepEqual(signInAgain({ ...expired, workspaceTag: "Acme" }), [
    "workspace-1",
    "reauthenticate",
  ]);
  // Without one, whatever the raw plan and team, any workspace may sign in.
  assert.deepEqual(signInAgain(expired), [null, "reauthenticate"]);
});
