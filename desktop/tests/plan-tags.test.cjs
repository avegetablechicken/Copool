const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const V = require("../ui/card-view.js");

// The core names and colors plan tags (plan_tags.rs) and card-view.js puts
// them in the card's view (card-view.test.cjs); here, how app.js paints them.
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
const tr = (key) => key;
const context = (pool = "codex") => ({ pool, locale: "en", tr, collapsed: false });

/// The app's tag and control painting, drawing plain nodes and recording
/// its sign-ins.
function app() {
  const node = (className, text) => ({
    className, text, children: [], dataset: {}, attributes: {}, listeners: {},
    classList: { toggle() {} },
    append(...nodes) { this.children.push(...nodes); },
    setAttribute(name, value) { this.attributes[name] = value; },
    addEventListener(name, handler) { this.listeners[name] = handler; },
  });
  const scope = {
    V,
    signIns: [],
    el: (tag, className, text) => node(className, text),
    button: (label, glyph, className, handler) => {
      const control = node(className, label);
      control.glyph = glyph;
      control.addEventListener("click", handler);
      return control;
    },
    notice: () => {},
    tr,
  };
  vm.createContext(scope);
  vm.runInContext(
    "function signIn(...args) { signIns.push(args); }\n" +
      body("setPlanAccent") +
      body("cardTags") +
      body("actionHandler") +
      body("cardControls"),
    scope,
  );
  scope.node = node;
  return scope;
}

test("A card's plan tag shows the label and color the core gives it", () => {
  const scope = app();
  const view = V.cardView({ id: "a", planType: "pro", planTag: { label: "PRO 20X", accent: "orange" } }, context());
  const [tag] = scope.cardTags(view.tags).children;
  assert.equal(tag.className, "tag plan");
  assert.equal(tag.text, "PRO 20X");
  const card = scope.node("block account");
  scope.setPlanAccent(card, view);
  assert.equal(card.dataset.accent, "orange");
  // A tag that explains itself has its tooltip.
  const waiting = V.cardView({ id: "b", awaitingFirstRequest: true }, context());
  assert.equal(scope.cardTags(waiting.tags).children[0].title, "accounts.card.awaiting_first_request.help");
});

test("A card without a plan tag shows none and no color, in every pool", () => {
  const scope = app();
  for (const pool of ["codex", "claude", "antigravity", "grok"]) {
    const view = V.cardView({ id: "a", planType: "pro", planTag: null }, context(pool));
    const card = scope.node("block account");
    scope.setPlanAccent(card, view);
    assert.equal(card.dataset.accent, undefined, pool);
    assert.deepEqual(scope.cardTags(view.tags).children, [], pool);
  }
});

test("Sign in again pins the workspace only for a card with a workspace tag", () => {
  const scope = app();
  const signInAgain = (account) => {
    const view = V.cardView(account, context());
    const controls = scope.cardControls(view.actions, view.full, { account });
    const control = controls.children.find((c) => c.glyph === "reauthenticate");
    assert(control, "Sign in again shown");
    assert.equal(control.attributes["aria-label"], "accounts.card.sign_in_again dev@example.com");
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
