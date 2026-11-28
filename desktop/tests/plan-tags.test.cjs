const { test } = require("node:test");
const assert = require("node:assert/strict");
const V = require("../ui/card-view.js");
const dom = require("./fake-dom.cjs");

// The core names and colors plan tags (plan_tags.rs) and card-view.js puts
// them in the card's view (card-view.test.cjs); here, how card-face.js
// paints them.
const tr = (key) => key;
const context = (pool = "codex") => ({ pool, locale: "en", tr, collapsed: false });

/// card-face.js painting plain nodes and recording its sign-ins.
function app() {
  const signIns = [];
  const face = dom.cardFace({ tr, signIn: (...args) => signIns.push(args) });
  face.signIns = signIns;
  face.node = (className) => dom.el("section", className);
  return face;
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

test("A card's refresh and switch buttons are painted locked as card-view.js says, without syncButtons", () => {
  const scope = app();
  const paint = (account, extra = {}) => {
    const view = V.cardView(account, { ...context(), ...extra });
    const controls = scope.cardControls(view.actions, view.full, { account }).children;
    const disabled = (glyph) => controls.find((c) => c.glyph === glyph)?.disabled;
    return [disabled("refresh"), disabled("switch")];
  };
  const card = { id: "a", email: "a@example.com", isCurrent: false, available: true };
  assert.deepEqual(paint(card), [false, false]);
  assert.deepEqual(paint({ ...card, available: false }), [false, true]);
  assert.deepEqual(paint(card, { switchingId: "a" }), [false, true]);
  assert.deepEqual(paint(card, { ui: () => ({ refreshing: true }) }), [true, false]);
});
