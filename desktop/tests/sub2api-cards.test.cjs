const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

// Windows checkouts may end lines with CRLF.
const source = fs
  .readFileSync(path.join(__dirname, "../ui/app.js"), "utf8")
  .replace(/\r\n/g, "\n");
test("Sub2api cards group by the provider the core names, even before the provider list knows them", () => {
  const { Providers } = require("../ui/controllers.js");
  const providerControllers = {
    claude: new Providers(null, "claude"),
    codex: new Providers(null, "codex"),
    antigravity: new Providers(null, "antigravity"),
  };
  // Read before card 2 was synced.
  providerControllers.claude.persisted = [
    { id: "srv", name: "myServer", cachedAccounts: [{ id: 1 }] },
  ];
  providerControllers.codex.persisted = [{ id: "team-a", name: "Team-A", cachedAccounts: [] }];
  providerControllers.antigravity.persisted = [
    { id: "0b9e-4f1a", name: "Relay", cachedAccounts: [] },
  ];
  const scope = { pool: "claude", providerControllers, P: require("../ui/presentation.js") };
  vm.createContext(scope);
  const start = source.indexOf("/// The provider of a Sub2api card (presentation.js).");
  const end = source.indexOf("/// Removes an account, or all of a Sub2api provider's cards", start);
  assert(start >= 0 && end > start, "providerKey found");
  vm.runInContext(`${source.slice(start, end)}\nthis.providerKey = providerKey;`, scope);
  // A card's refresh key names its provider (refresh_plan.rs); the app
  // never reads the card's id for it.
  const card = (provider) => ({ id: "any", sourceTag: "SUB2API", refresh: { key: `sub2api:${provider}`, seconds: 300 } });
  assert.equal(scope.providerKey(card("srv")), "srv");
  assert.equal(scope.providerTitle(card("srv")), "myServer");
  assert.equal(scope.providerKey({ id: "claude-sub2api-srv-2", refresh: { key: "claude-sub2api-srv-2" } }), null);
  assert.equal(scope.providerKey({ id: "claude-sub2api-srv-2" }), null);
  // Codex's cards name their provider by its id too, in any case.
  scope.pool = "codex";
  assert.equal(scope.providerKey(card("team-a")), "team-a");
  assert.equal(scope.providerTitle(card("team-a")), "Team-A");
  assert.equal(scope.providerTitle(card("TEAM-A")), "Team-A");
  // An unknown provider still groups, under its id.
  assert.equal(scope.providerTitle(card("gone")), "gone");
  // Dashes and all.
  scope.pool = "antigravity";
  assert.equal(scope.providerKey(card("0b9e-4f1a")), "0b9e-4f1a");
  assert.equal(scope.providerTitle(card("0b9e-4f1a")), "Relay");
});
test("Antigravity's Sub2api form keeps agy's key out and its password secret", () => {
  const { Providers } = require("../ui/controllers.js");
  const providers = new Providers(null, "antigravity");
  assert.deepEqual(
    providers.form.fields.map(([key]) => key),
    ["name", "baseURL", "apiKeyVariable", "adminBaseURL", "username", "password"],
  );
  // The key's variable is a name, shown as saved; the key is never asked.
  assert.deepEqual(providers.form.secrets, ["password"]);
  const id = providers.add();
  assert.equal(providers.providers.get(id).baseURL, "");
  assert.equal(providers.providers.get(id).apiKeyVariable, "");
  const saved = { id: "p", name: "Relay", apiKeyVariable: "RELAY_KEY", password: "" };
  assert.equal(providers.draft(saved).apiKeyVariable, "RELAY_KEY");
  assert.equal(providers.key({ id: "p", name: "Relay" }), "p");
  assert.equal(providers.name({ id: "p", name: "Relay" }), "Relay");
});

test("Codex's Sub2api form names the Codex configuration's provider and keeps its password secret", () => {
  const { Providers } = require("../ui/controllers.js");
  const providers = new Providers(null, "codex");
  assert.deepEqual(
    providers.form.fields.map(([key, label]) => [key, label]),
    [["name", "provider_id"], ["adminBaseURL", "admin_base_url"],
      ["username", "username"], ["password", "password"]],
  );
  assert.deepEqual(providers.form.secrets, ["password"]);
  const id = providers.add();
  assert.equal(providers.providers.get(id).name, "");
  assert.equal("providerID" in providers.providers.get(id), false);
  assert.equal(providers.key({ id: "p", name: "my" }), "p");
  assert.equal(providers.name({ id: "p", name: "my" }), "my");
});

test("Claude providers keep the selected source across saving and reopening the form", async () => {
  const { Providers } = require("../ui/controllers.js");
  const providers = new Providers(async (command, { pool, value }) => {
    assert.equal(command, "save_provider");
    assert.equal(pool, "claude");
    assert.equal(value.mode, "address");
    assert.equal(value.baseURL, "https://relay.example");
    assert.equal(value.apiKeyVariable, "RELAY_KEY");
    return value;
  }, "claude");
  const id = providers.add();
  const draft = providers.providers.get(id);
  assert.equal(draft.mode, "address");
  assert.deepEqual(providers.form.sourceFields.address.map(([key]) => key),
    ["name", "baseURL", "apiKeyVariable"]);
  Object.assign(draft, { name: "Relay", baseURL: "https://relay.example", apiKeyVariable: "RELAY_KEY" });
  await providers.save(id);
  assert.equal(providers.providers.get(id).mode, "address");

  const legacy = { id: "old", name: "Team", baseURL: "https://team.example", settings: "Team" };
  providers.accept([legacy]);
  assert.equal(providers.providers.get("old").mode, undefined);
  assert.equal(providers.providers.get("old").settings, "Team");
  assert.deepEqual(providers.form.sourceFields.settings.map(([key]) => key), ["settings"]);
});
