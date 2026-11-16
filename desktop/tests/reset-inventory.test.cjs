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

test("Credits, resets and the notice own separate concise tooltips", () => {
  const fs = require("node:fs"), vm = require("node:vm");
  const source = fs.readFileSync(require("node:path").join(__dirname, "../ui/app.js"), "utf8");
  const start = source.indexOf("function accountBalanceText(");
  const fn = source.slice(start, source.indexOf("\n}\n", start) + 3);
  const el = (tag, className, text = "") => ({tag, className, textContent: text, children: [],
    append(...nodes) {this.children.push(...nodes);}, setAttribute(name, value) {this[name] = value;}});
  const context = {P, el, icon: (name) => ({icon: name}), locale: "en",
    tr: (key, ...args) => `${key}:${args.join(",")}`};
  vm.createContext(context);vm.runInContext(fn, context);
  const future = Math.floor(Date.now()/1000) + 86400;
  const footer = context.accountBalanceText({credits: {expiries: [future]},
    resetGrants: [grant("full", 1, future), grant("fable", 1, future), grant("fable", 1, future+86400)]}, "5");
  const [credits, separator, resets, notice] = footer.children;
  assert.match(credits.title, /reset_expires_format/);
  assert.match(resets.title, /reset_expires_format/);
  assert.equal(resets.children.length, 0);
  assert.equal(notice.title, "accounts.card.reset_full: × 1\naccounts.card.reset_fable: × 2");
  assert.equal(notice["aria-label"], notice.title);
  assert.equal(footer.title, undefined);
  const unknown = context.accountBalanceText({}, "--");
  assert.match(unknown.children[0].title, /expiry_unknown/);
  assert.match(unknown.children[2].title, /expiry_unknown/);
});
