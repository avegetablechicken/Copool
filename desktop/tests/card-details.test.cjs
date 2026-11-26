const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const P = require("../ui/presentation.js");

const source = fs.readFileSync(path.join(__dirname, "../ui/app.js"), "utf8")
  .replace(/\r\n/g, "\n");
function load(extra = {}) {
  const scope = vm.createContext({
    P, locale: "en", pool: "claude",
    preferences: {
      accountProxyUrl: "socks5://127.0.0.1:7890",
      refresh: { claude: { seconds: 30, fullEvery: 6 } },
    },
    tr: (key, ...args) => [key, ...args].join(" "),
    ...extra,
  });
  for (const name of ["editButton", "cardDetails", "proxyOwner", "sharedProxy", "tokenLabel"]) {
    const start = source.indexOf(`function ${name}(`);
    const end = source.indexOf("\n}\n", start);
    assert(start >= 0 && end > start, name);
    vm.runInContext(source.slice(start, end + 3), scope);
  }
  return scope;
}

test("details contain the proxy, relative token expiry and this card's refresh interval", () => {
  const c = load();
  const account = { id: "a", isCurrent: true, renewal: "always", tokenExpiresAt: Date.now() / 1000 + 3600, nextRefreshAt: Date.now() / 1000 + 60 };
  const details = () => c.cardDetails({ account });
  const lines = details().split("\n");
  assert.equal(lines.length, 3);
  assert.equal(lines[0], "accounts.proxy.title: socks5://127.0.0.1:7890");
  assert.match(lines[1], /^accounts.renewal.expires_format \d+[mh]$/);
  assert.equal(lines[2], "accounts.refresh.interval_format 30 seconds");
  account.proxyUrl = "direct://";
  account.tokenExpiresAt = 1;
  account.renewal = "never";
  assert.equal(details(), "direct\naccounts.renewal.expired\naccounts.refresh.interval_format 30 seconds");
  account.tokenExpiresAt = null;
  assert.equal(details(), "direct\naccounts.refresh.interval_format 30 seconds");
  account.proxyUrl = "https://proxy.example:443";
  assert.equal(details(), "accounts.proxy.title: https://proxy.example:443\naccounts.refresh.interval_format 30 seconds");
  account.proxyUrl = "";
  c.preferences.accountProxyUrl = "direct://";
  assert.equal(details(), "direct\naccounts.refresh.interval_format 30 seconds");
  c.preferences.accountProxyUrl = "";
  assert.equal(details(), "direct\naccounts.refresh.interval_format 30 seconds");
  account.isCurrent = false;
  assert.equal(details(), "direct\naccounts.refresh.interval_format 3 minutes");
});

test("providers and their members show the interval for whether the provider is in use", () => {
  const c = load();
  const account = { id: "member", sourceTag: "SUB2API" };
  const group = { key: "provider", accounts: [account] };
  assert.equal(c.cardDetails({ group }), "direct\naccounts.refresh.interval_format 3 minutes");
  assert.equal(c.cardDetails({ account, grouped: true }), "accounts.refresh.interval_format 3 minutes");
  assert.equal(c.cardDetails({ account }), "direct\naccounts.refresh.interval_format 3 minutes");
  group.accounts.push({ id: "current", isCurrent: true });
  assert.equal(c.cardDetails({ group }), "direct\naccounts.refresh.interval_format 30 seconds");
});

test("hover and focus update details locally and ignore another pool", () => {
  let target = { account: { id: "a", renewal: "always", tokenExpiresAt: Date.now() / 1000 + 3600 } };
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
  target = { account: { id: "a", proxyUrl: "direct://" } };
  control.onfocus();
  assert.equal(control.title, "direct\naccounts.refresh.interval_format 3 minutes");
  c.pool = "codex";
  target = null;
  control.onpointerenter();
  assert.equal(control.title, "direct\naccounts.refresh.interval_format 3 minutes");
});
