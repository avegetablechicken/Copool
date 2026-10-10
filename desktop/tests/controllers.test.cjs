const { test } = require("node:test");
const assert = require("node:assert/strict");
const { Settings } = require("../ui/controllers.js");
const legacy = (suite, name, fn) => test(`${suite}.${name}`, fn);
const settings = (providers = []) => ({ sub2APIProvider: { providers } });
const p = (id, extra = {}) => ({
  id,
  providerID: id,
  username: `${id}@example.test`,
  password: "",
  cachedAccounts: [],
  importedAccountIDs: [],
  accountProxyURLs: {},
  ...extra,
});
legacy(
  "SettingsPageModelTests",
  "testQuitAppInvokesInjectedAction",
  async () => {
    let didQuit = false;
    const model = new Settings(async (command) => {
      assert.equal(command, "quit_app");
      didQuit = true;
    });
    await model.quit();
    assert.equal(didQuit, true);
  },
);
legacy(
  "SettingsPageModelTests",
  "testBackgroundCacheUpdatePreservesEditedSub2APICredentials",
  () => {
    const model = new Settings();
    model.accept(
      settings([
        p("id", {
          providerID: "one",
          cachedAccounts: [{ id: 42 }],
          importedAccountIDs: [42],
        }),
      ]),
    );
    model.providers.get("id").username = "edited@example.test";
    model.providers.get("id").password = "unsaved-secret";
    model.accept(
      settings([
        p("different-config-id", {
          providerID: "one",
          cachedAccounts: [{ id: 42, usage: { fetchedAt: 10 } }],
          importedAccountIDs: [42],
          accountProxyURLs: { 42: "http://127.0.0.1:8080" },
        }),
      ]),
    );
    assert.equal(model.providers.size, 1);
    const draft = model.providers.get("id");
    assert.equal(draft.username, "edited@example.test");
    assert.equal(draft.password, "unsaved-secret");
    assert.equal(draft.cachedAccounts[0].usage.fetchedAt, 10);
    assert.equal(draft.accountProxyURLs[42], "http://127.0.0.1:8080");
  },
);
legacy(
  "SettingsPageModelTests",
  "testSettingsPageAcceptsProviderChangedByAccountCard",
  () => {
    const model = new Settings();
    model.accept(settings(), "openai");
    model.accept(settings(), "ShareCoder");
    assert.equal(model.currentProvider, "ShareCoder");
  },
);
legacy(
  "SettingsPageModelTests",
  "testAccountsPageModelToggleUsageProgressDisplayPersistsAndShowsNotice",
  async () => {
    let saved;
    const model = new Settings(async (command, { value }) => {
      assert.equal(command, "save_preferences");
      saved = value;
    });
    const updated = await model.savePreferences(
      { usageDisplay: "used", locale: "en" },
      { usageDisplay: "remaining" },
    );
    assert.equal(updated.usageDisplay, "remaining");
    assert.deepEqual(saved, updated);
    assert.equal(model.notice, "settings.notice.updated");
  },
);
legacy(
  "SettingsPageModelTests",
  "testAccountsPageModelToggleUsageProgressDisplayInvokesSettingsUpdateCallback",
  async () => {
    let callback;
    const model = new Settings(
      async () => {},
      (value) => (callback = value),
    );
    const updated = await model.savePreferences(
      { usageDisplay: "used" },
      { usageDisplay: "remaining" },
    );
    assert.equal(callback, updated);
    assert.equal(callback.usageDisplay, "remaining");
  },
);
legacy(
  "SettingsPageModelTests",
  "testProviderSaveIsScopedAndDeletionPersistsIndependently",
  async () => {
    const persisted = [p("first"), p("second")];
    const removed = [];
    const model = new Settings(async (command, args) => {
      if (command === "save_provider") {
        const saved = structuredClone(args.value);
        if (
          persisted.some(
            (p) =>
              p.id !== saved.id &&
              p.providerID.toLowerCase() === saved.providerID.toLowerCase(),
          )
        )
          throw "error.sub2api.duplicate_provider";
        persisted[persisted.findIndex((p) => p.id === saved.id)] = saved;
        return saved;
      }
      if (command === "remove_provider") {
        removed.push(args.id);
        persisted.splice(
          persisted.findIndex((p) => p.id === args.id),
          1,
        );
        return;
      }
      throw Error(command);
    });
    model.accept(settings(structuredClone(persisted)));
    model.providers.get("first").adminBaseURL = "https://admin.example.test";
    model.providers.get("second").username = "";
    model.providers.get("second").password = "unsaved-password";
    model.providers.set("third", p("third", { providerID: "", username: "" }));
    await model.save("first");
    assert.equal(persisted.length, 2);
    assert.equal(persisted[0].adminBaseURL, "https://admin.example.test");
    assert.equal(persisted[1].username, "second@example.test");
    assert.equal(model.providers.get("second").password, "unsaved-password");
    assert.equal(model.providers.size, 3);
    assert.equal(model.notice, "settings.notice.sub2api_saved");
    model.providers.get("second").providerID = "renamed-unsaved";
    model.providers.get("first").providerID = "SECOND";
    await assert.rejects(model.save("first"));
    assert.equal(model.notice, "error.sub2api.duplicate_provider");
    await model.remove("second");
    assert.deepEqual(removed, ["second"]);
    assert.deepEqual(
      persisted.map((p) => p.id),
      ["first"],
    );
    assert.equal(model.providers.get("first").providerID, "SECOND");
    assert.equal(model.providers.size, 2);
  },
);
