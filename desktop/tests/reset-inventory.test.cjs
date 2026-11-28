const { test } = require("node:test");
const assert = require("node:assert/strict");
const P = require("../ui/presentation.js");
const grant = (kind, count, expiresAt = 300) => ({ kind, count, expiresAt });

test("Full and Fable resets share one count and warn for non-full grants", () => {
  const result = P.resetInventory({ remainingResetCount: 3,
    resetGrants: [grant("full", 1), grant("fable", 2)] }, 100);
  assert.equal(result.count, 3);
  assert.equal(result.warning, true);
  assert.equal(result.fable, true);
  assert.equal(P.resetInventory({ resetGrants: [grant("full", 2)] }, 100).warning, false);
  for (const kind of ["partial", "unknown", "futureKind"])
    assert.equal(P.resetInventory({ resetGrants: [grant(kind, 1)] }, 100).warning, true);
});

test("Expired or spent partial resets do not warn or contribute to the sum", () => {
  const usage = { remainingResetCount: 5,
    resetGrants: [grant("full", 1), grant("fable", 3, 100), grant("partial", 0)] };
  assert.deepEqual(P.resetInventory(usage, 100), {
    count: 1, grants: [grant("full", 1)], warning: false, fable: false,
  });
  assert.equal(P.resetInventory(usage, 300).count, 0);
  assert.equal(P.resetInventory({resetGrants: [grant("fable", 1, null)]}, 100).warning, true);
});

test("Older caches and Codex keep their existing counts", () => {
  assert.equal(P.resetInventory({remainingResetCount: 4}).count, 4);
  assert.equal(P.resetInventory({remainingResetCount: 0}).count, 0);
  assert.equal(P.resetInventory({remainingResetCount: -1}).count, null);
  assert.equal(P.resetInventory().count, null);
  assert.equal(P.resetInventory({remainingResetCount: 4}).warning, false);
});


test("Notice groups equal reset types by expiry and sorts earliest first", () => {
  const usage = {
    resetGrants: [
      grant("fable", 1, 400),
      grant("full", 1, 200),
      grant("fable", 1, 300),
      grant("fable", 2, 300),
      grant("partial", 1, null),
    ],
  };
  const details = P.balanceTooltips(usage, 100);
  assert.deepEqual(details.types, [
    { kind: "full", count: 1, at: 200 },
    { kind: "fable", count: 3, at: 300 },
    { kind: "fable", count: 1, at: 400 },
    { kind: "partial", count: 1, at: null },
  ]);
  assert.deepEqual(details.resetDates, [
    { at: 200, count: 1 },
    { at: 300, count: 3 },
    { at: 400, count: 1 },
    { at: null, count: 1 },
  ]);
});

test("Balance expiry tooltips handle Codex, unknown dates, and stale credits", () => {
  const details = P.balanceTooltips({remainingResetCount: 3, resetExpiries: [300, 300],
    credits: {expiries: [400, 200, 200, 99]}}, 100);
  assert.deepEqual(details.resetDates, [{at: 300, count: 2}, {at: null, count: 1}]);
  assert.deepEqual(details.creditDates, [200, 400]);
  assert.deepEqual(P.balanceTooltips(undefined, 100).creditDates, []);
  assert.deepEqual(P.balanceTooltips({resetGrants: [grant("fable", 1, 100)]}, 100).types, []);
});

test("Credits and resets keep expiry tooltips while the notice opens a popover", () => {
  // What the footer says is card-view.js's (card-view.test.cjs); here, how
  // card-face.js paints it.
  const V = require("../ui/card-view.js");
  const dom = require("./fake-dom.cjs");
  let opened;
  const context = dom.cardFace({
    showResetPopover: (trigger, lines, wrapping) => {opened = {trigger, lines, wrapping};},
  });
  const tr = (key, ...args) => `${key}:${args.join(",")}`;
  const paint = (usage, pool = "codex") =>
    context.accountBalanceText(V.cardView({id: "a", usage}, {pool, locale: "en", tr}).footer);
  const future = Math.floor(Date.now()/1000) + 86400;
  const footer = paint({credits: {balance: "5", expiries: [future]},
    resetGrants: [grant("full", 1, future), grant("fable", 1, future), grant("partial", 1, null)]});
  const [credits, separator, resets, notice] = footer.children;
  assert.equal(credits.className, "account-credits");
  assert.match(credits.textContent, /credits_format:5$/);
  assert.match(credits.title, /reset_expires_format/);
  assert.equal(separator, " · ");
  assert.equal(resets.className, "account-resets");
  assert.match(resets.title, /reset_expires_format/);
  assert.equal(resets.children.length, 0);
  assert.equal(notice.tag, "button");
  assert.equal(notice.type, "button");
  assert.equal(notice.title, "");
  assert.equal(notice.attributes["aria-haspopup"], "dialog");
  assert.equal(notice.attributes["aria-expanded"], "false");
  notice.click();
  assert.equal(opened.trigger, notice);
  assert.equal(opened.wrapping, true);
  assert.equal(opened.lines.length, 3);
  assert.equal(notice.attributes["aria-label"], opened.lines.join("\n"));
  assert.equal(footer.title, "");
  // Without tooltips, none is set.
  const unavailable = paint({});
  assert.equal(unavailable.children[0].title, "");
  assert.equal(unavailable.children[2].title, "");
  assert.equal(unavailable.children.length, 3);
  // Antigravity and Grok: their credits alone.
  const creditsOnly = paint({credits: {balance: "1000"}, resetGrants: [grant("partial", 1, future)]}, "grok");
  assert.equal(creditsOnly.children.length, 1);
  assert.equal(creditsOnly.children[0].className, "account-credits");
});


test("Reset popover fits the panel and toggles closed with focus restored", () => {
  const Menus = require("../ui/menus.js");
  const panel = {left: 0, right: 300, top: 0, bottom: 200};
  let appended;
  const document = {body: {append(node) {appended = node;}}, querySelector() {return {getBoundingClientRect: () => panel};}};
  const el = (tag, className, text) => ({tag, className, text, style: {}, children: [], offsetWidth: 150, offsetHeight: 60,
    append(node) {this.children.push(node);}, setAttribute(key, value) {this[key] = value;},
    focus() {this.focused = true;}, remove() {this.removed = true;}});
  const trigger = {getAttribute: () => "Full reset × 1, Fable reset × 2",
    getBoundingClientRect: () => ({left: 270, top: 180, bottom: 192}),
    setAttribute(key, value) {this[key] = value;}, focus() {this.focused = true;}};
  const menus = Menus.create({document, el});
  menus.showResetPopover(trigger, ["Full reset × 1", "Fable reset × 2"]);
  assert.equal(appended.role, "dialog");
  assert.equal(appended.style.left, "142px");
  assert.equal(appended.style.top, "116px");
  assert.equal(appended.children.length, 2);
  assert.equal(appended.focused, true);
  assert.equal(trigger["aria-expanded"], "true");
  assert.equal(menus.isOpenFor(trigger), true);
  menus.showResetPopover(trigger, []);
  assert.equal(appended.removed, true);
  assert.equal(menus.isOpen, false);
  assert.equal(trigger["aria-expanded"], "false");
  assert.equal(trigger.focused, true);
});
