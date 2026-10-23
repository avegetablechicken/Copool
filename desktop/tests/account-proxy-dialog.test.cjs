const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const source = fs.readFileSync(path.join(__dirname, "../ui/app.js"), "utf8");
test("account proxy dialog saves direct, restores it and allows inheritance again", async () => {
  const nodes = new Map();
  const $ = (id) => {
    if (!nodes.has(id)) nodes.set(id, {
      value: "", disabled: false, focus() {}, showModal() {}, close() {},
    });
    return nodes.get(id);
  };
  const saved = [];
  let pending;
  const scope = { $, tr: (key) => key, action: (fn) => { pending = fn(); },
    accountsController: { saveProxy: async (...args) => saved.push(args) } };
  vm.createContext(scope);
  vm.runInContext(source.slice(source.indexOf("function openAccountProxy("),
    source.indexOf("/// Refreshes one account")), scope);
  const open = (proxyUrl) => scope.openAccountProxy({ id: "card", proxyUrl, sourceTag: "SUB2API" });
  const submit = async () => {
    $("edit-form").onsubmit({ preventDefault() {}, submitter: { disabled: false } });
    await pending;
  };
  open("http://127.0.0.1:7890");
  assert.equal($("edit-proxy-enabled").checked, true);
  $("edit-proxy-enabled").checked = false;
  $("edit-proxy-enabled").onchange();
  assert.equal($("edit-proxy").disabled, true);
  await submit();
  assert.deepEqual(saved.pop(), ["card", "direct://"]);
  open("direct://");
  assert.equal($("edit-proxy-enabled").checked, false);
  assert.equal($("edit-proxy").value, "");
  $("edit-proxy-enabled").checked = true;
  $("edit-proxy-enabled").onchange();
  assert.equal($("edit-proxy").disabled, false);
  await submit();
  assert.deepEqual(saved.pop(), ["card", ""]);
  open("");
  assert.equal($("edit-proxy").disabled, false);
  await submit();
  assert.deepEqual(saved.pop(), ["card", ""]);
  assert.equal($("edit-proxy-enabled").checked, true);
});
test("a provider's proxy saves once, through its first card", async () => {
  const nodes = new Map();
  const $ = (id) => {
    if (!nodes.has(id)) nodes.set(id, {
      value: "", disabled: false, focus() {}, showModal() {}, close() {},
    });
    return nodes.get(id);
  };
  const saved = [];
  let pending;
  const scope = { $, tr: (key) => key, action: (fn) => { pending = fn(); },
    accountsController: { saveProxy: async (...args) => saved.push(args) } };
  vm.createContext(scope);
  vm.runInContext(source.slice(source.indexOf("function openAccountProxy("),
    source.indexOf("/// Refreshes one account")), scope);
  const submit = async () => {
    $("edit-form").onsubmit({ preventDefault() {}, submitter: { disabled: false } });
    await pending;
  };
  const group = (...proxyUrls) => ({
    accounts: proxyUrls.map((proxyUrl, i) => ({ id: `card-${i}`, proxyUrl })),
  });
  scope.openProviderProxy(group("socks5://h:1", "socks5://h:1"), "team");
  assert.equal($("edit-name").textContent, "team");
  assert.equal($("edit-proxy").value, "socks5://h:1");
  assert.equal($("edit-proxy-help").textContent, "settings.sub2api.proxy_help");
  $("edit-proxy-enabled").checked = false;
  await submit();
  assert.deepEqual(saved.splice(0), [["card-0", "direct://"]]);
});
test("a blank card proxy is described as the shared account proxy when one is set", () => {
  const nodes = new Map();
  const $ = (id) => {
    if (!nodes.has(id)) nodes.set(id, {
      value: "", disabled: false, focus() {}, showModal() {}, close() {},
    });
    return nodes.get(id);
  };
  const scope = {
    $,
    preferences: { accountProxyUrl: " socks5://127.0.0.1:7891 " },
    tr: (key, ...args) => [key, ...args].join(" "),
  };
  vm.createContext(scope);
  vm.runInContext(source.slice(source.indexOf("function proxyLabel("),
    source.indexOf("/// Proxy and removal")), scope);
  vm.runInContext(source.slice(source.indexOf("function openAccountProxy("),
    source.indexOf("/// The provider's proxy")), scope);
  const help = (account) => {
    scope.openAccountProxy({ id: "card", proxyUrl: "", ...account });
    return $("edit-proxy-help").textContent;
  };
  assert.equal(help({}), "accounts.proxy.shared_help_format socks5://127.0.0.1:7891");
  // A Sub2api provider's cards never take the shared proxy.
  assert.equal(help({ sourceTag: "SUB2API" }), "settings.sub2api.proxy_help");
  scope.preferences.accountProxyUrl = "direct://";
  assert.equal(help({}), "accounts.proxy.shared_help_format accounts.proxy.none");
  scope.preferences.accountProxyUrl = "";
  assert.equal(help({}), "settings.sub2api.proxy_help");
});
test("Sub2api cards group by the provider in their id, even before the provider list knows them", () => {
  const scope = {
    pool: "claude",
    // Read before card 2 was synced.
    claudeProvidersSaved: [{ id: "srv", name: "myServer", cachedAccounts: [{ id: 1 }] }],
    providerController: { persisted: [{ providerID: "Team-A", cachedAccounts: [] }] },
  };
  vm.createContext(scope);
  vm.runInContext(source.slice(source.indexOf("function providerKey("),
    source.indexOf("function accountUsesExplicitProxy(")), scope);
  const card = (id) => ({ id, sourceTag: "SUB2API" });
  assert.equal(scope.providerKey(card("claude-sub2api-srv-2")), "srv");
  assert.equal(scope.providerTitle(card("claude-sub2api-srv-2")), "myServer");
  assert.equal(scope.providerKey({ id: "claude-sub2api-srv-2" }), null);
  scope.pool = "codex";
  assert.equal(scope.providerKey(card("sub2api-team-a-17")), "team-a");
  assert.equal(scope.providerTitle(card("sub2api-team-a-17")), "Team-A");
  // An unknown provider still groups, under its id.
  assert.equal(scope.providerTitle(card("sub2api-gone-3")), "gone");
});
