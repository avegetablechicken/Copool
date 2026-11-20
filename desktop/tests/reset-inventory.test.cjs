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


test("Notice groups equal reset types across different expiries", () => {
  const usage = {resetGrants: [grant("full", 1, 200), grant("fable", 1, 300), grant("fable", 1, 400)]};
  const details = P.balanceTooltips(usage, 100);
  assert.deepEqual(details.types, [{kind: "full", count: 1}, {kind: "fable", count: 2}]);
  assert.deepEqual(details.resetDates, [{at: 200, count: 1}, {at: 300, count: 1}, {at: 400, count: 1}]);
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
  const fs = require("node:fs"), vm = require("node:vm");
  // Windows checkout uses CRLF; function boundaries below use LF.
  const source = fs.readFileSync(require("node:path").join(__dirname, "../ui/app.js"), "utf8").replace(/\r\n/g, "\n");
  const start = source.indexOf("function accountBalanceText(");
  const end = source.indexOf("\n}\n", start);
  assert.ok(start >= 0 && end > start);
  const fn = source.slice(start, end + 3);
  const el = (tag, className, text = "") => ({tag, className, textContent: text, children: [],
    append(...nodes) {this.children.push(...nodes);}, setAttribute(name, value) {this[name] = value;}});
  let opened;
  const context = {P, el, icon: (name) => ({icon: name}), locale: "en",
    button: (label, glyph, className, onclick) => Object.assign(el("button", className), {onclick}),
    showResetPopover: (trigger, lines) => {opened = {trigger, lines};},
    tr: (key, ...args) => `${key}:${args.join(",")}`};
  vm.createContext(context);vm.runInContext(fn, context);
  const future = Math.floor(Date.now()/1000) + 86400;
  const footer = context.accountBalanceText({credits: {expiries: [future]},
    resetGrants: [grant("full", 1, future), grant("fable", 1, future), grant("fable", 1, future+86400)]}, "5");
  const [credits, separator, resets, notice] = footer.children;
  assert.match(credits.title, /reset_expires_format/);
  assert.match(resets.title, /reset_expires_format/);
  assert.equal(resets.children.length, 0);
  assert.equal(notice.tag, "button");
  assert.equal(notice.type, "button");
  assert.equal(notice.title, undefined);
  assert.equal(notice["aria-haspopup"], "dialog");
  assert.equal(notice["aria-expanded"], "false");
  notice.onclick();
  assert.equal(opened.trigger, notice);
  assert.equal(opened.lines.join("\n"), "accounts.card.reset_full: × 1\naccounts.card.reset_fable: × 2");
  assert.equal(footer.title, undefined);
  assert.doesNotMatch(resets.title, /last_confirmed/);
  const cached = context.accountBalanceText({remainingResetCount: 2, resetExpiries: [1, future],
    resetsLastConfirmedAt: 100}, "--").children[2];
  assert.match(cached.textContent, /remaining_resets_format:1$/);
  assert.match(cached.title, /reset_expires_format/);
  assert.match(cached.title, /resets_last_confirmed_format/);
  const unknown = context.accountBalanceText({}, "--");
  assert.match(unknown.children[0].title, /expiry_unknown/);
  assert.match(unknown.children[2].title, /expiry_unknown/);
});


test("Reset popover fits the panel and toggles closed with focus restored", () => {
  const fs = require("node:fs"), vm = require("node:vm");
  // Windows checkout uses CRLF; function boundaries below use LF.
  const source = fs.readFileSync(require("node:path").join(__dirname, "../ui/app.js"), "utf8").replace(/\r\n/g, "\n");
  const body = (name) => {
    const start = source.indexOf(`function ${name}(`);
    return source.slice(start, source.indexOf("\n}\n", start) + 3);
  };
  const panel = {left: 0, right: 300, top: 0, bottom: 200};
  let appended;
  const context = {
    openMenu: null, closeSubmenu() {},
    document: {body: {append(node) {appended = node;}}, querySelector() {return {getBoundingClientRect: () => panel};}},
    el: (tag, className, text) => ({tag, className, text, style: {}, children: [], offsetWidth: 150, offsetHeight: 60,
      append(node) {this.children.push(node);}, setAttribute(key, value) {this[key] = value;},
      focus() {this.focused = true;}, remove() {this.removed = true;}}),
  };
  const trigger = {getAttribute: () => "Full reset × 1, Fable reset × 2",
    getBoundingClientRect: () => ({left: 270, top: 180, bottom: 192}),
    setAttribute(key, value) {this[key] = value;}, focus() {this.focused = true;}};
  vm.createContext(context);
  vm.runInContext(body("closeMenu") + body("menuLeft") + body("showResetPopover"), context);
  context.showResetPopover(trigger, ["Full reset × 1", "Fable reset × 2"]);
  assert.equal(appended.role, "dialog");
  assert.equal(appended.style.left, "142px");
  assert.equal(appended.style.top, "116px");
  assert.equal(appended.children.length, 2);
  assert.equal(appended.focused, true);
  assert.equal(trigger["aria-expanded"], "true");
  assert.equal(context.openMenu.menu, appended);
  context.showResetPopover(trigger, []);
  assert.equal(appended.removed, true);
  assert.equal(context.openMenu, null);
  assert.equal(trigger["aria-expanded"], "false");
  assert.equal(trigger.focused, true);
});
