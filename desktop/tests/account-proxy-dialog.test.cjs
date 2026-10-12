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
