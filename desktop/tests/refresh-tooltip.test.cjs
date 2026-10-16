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
  return source.slice(start, source.indexOf("\n}\n", start) + 3);
};
function load() {
  const context = {
    P,
    locale: "en",
    tr: (key, ...args) => (args.length ? `${key}:${args.join(",")}` : key),
  };
  vm.createContext(context);
  vm.runInContext(body("nextRefreshAt") + body("refreshTooltip"), context);
  return context;
}

test("A refresh button tells how long until its next automatic refresh", () => {
  const { refreshTooltip } = load();
  const button = {};
  let at = null;
  refreshTooltip(button, "Refresh", () => at);
  assert.equal(button.title, "Refresh");
  // Worked out again as the pointer enters, not when the card rendered.
  at = Date.now() / 1000 + 90;
  button.onpointerenter();
  assert.equal(button.title, "Refresh\naccounts.refresh.next_format:2m");
  at = Date.now() / 1000 - 1;
  button.onpointerenter();
  assert.equal(button.title, "Refresh\naccounts.refresh.next_due");
});

test("The toolbar follows the full refresh; a provider its first card due", () => {
  const { nextRefreshAt } = load();
  assert.equal(nextRefreshAt([]), null);
  assert.equal(nextRefreshAt([{}, { nextRefreshAt: null }]), null);
  assert.equal(
    nextRefreshAt([{ nextRefreshAt: 300 }, {}, { nextRefreshAt: 120 }]),
    120,
  );
  // The toolbar shows the background's own schedule, not the cards'.
  assert.match(body("renderToolbar"), /refreshTooltip\([\s\S]*\$\("refresh-all"\)[\s\S]*\(\) => fullRefreshAt/);
  assert.match(body("refreshView"), /invoke\("next_full_refresh_at", \{ claude: pool === "claude" \}\)/);
  assert.match(source, /refreshTooltip\(refresh, tr\("common\.refresh_usage"\), \(\) => account\.nextRefreshAt\)/);
  assert.match(body("providerControls"), /nextRefreshAt\(group\.accounts\)/);
});
