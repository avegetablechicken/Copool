const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const V = require("../ui/card-view.js");

// What the details say is card-view.js's (card-view.test.cjs); here, how
// the edit button shows them.
// Windows checkouts may end lines with CRLF; the slice below looks for LF.
const source = fs.readFileSync(path.join(__dirname, "../ui/app.js"), "utf8")
  .replace(/\r\n/g, "\n");
function load(extra = {}) {
  const scope = vm.createContext({
    V,
    pool: "claude",
    cardContext: () => ({
      pool: scope.pool,
      locale: "en",
      tr: (key, ...args) => [key, ...args].join(" "),
      accountProxy: "",
    }),
    tr: (key, ...args) => [key, ...args].join(" "),
    ...extra,
  });
  const start = source.indexOf("function editButton(");
  const end = source.indexOf("\n}\n", start);
  assert(start >= 0 && end > start, "editButton");
  vm.runInContext(source.slice(start, end + 3), scope);
  return scope;
}

test("hover and focus update details locally and ignore another pool", () => {
  // Its usual interval, as the core gives the card.
  const refresh = { key: "a", seconds: 180 };
  let target = { account: { id: "a", renewal: "always", tokenExpiresAt: Date.now() / 1000 + 3600, refresh } };
  const c = load({
    flipTarget: () => target,
    button: () => ({ setAttribute() {} }),
    invoke: () => assert.fail("Tooltips must not fetch schedules"),
  });
  const control = c.editButton("a", "Account");
  assert.match(control.title, /accounts.renewal.expires_format/);
  target.account.tokenExpiresAt = 1;
  control.onpointerenter();
  assert.match(control.title, /accounts.renewal.expired/);
  target = { account: { id: "a", proxyUrl: "direct://", refresh } };
  control.onfocus();
  assert.equal(control.title, "direct\naccounts.refresh.interval_format 3 minutes");
  c.pool = "codex";
  target = null;
  control.onpointerenter();
  assert.equal(control.title, "direct\naccounts.refresh.interval_format 3 minutes");
});
