/* UI state shared by the actual desktop and migrated controller tests. */
(function (root, factory) {
  const node = typeof module === "object" && module.exports;
  const api = node
    ? factory(require("./presentation.js"), require("./card-view.js"))
    : factory(root.CopoolPresentation, root.CopoolCardView);
  if (node) module.exports = api;
  else root.CopoolControllers = api;
})(globalThis, function (P, V) {
  const messageOf = (error) => error?.message || error?.code || String(error);
  /// The commands of one pool: every one names the pool it acts on. Only
  /// Codex has workspaces.
  function poolInvoke(invoke, target) {
    return (command, args = {}) => {
      if (target !== "codex" && command === "workspace_directory")
        return Promise.resolve([]);
      if (target !== "codex" && command === "cancel_workspace_refresh")
        return Promise.resolve(null);
      return invoke(command, { ...args, pool: target });
    };
  }
  /// A save typed for one owner (a pool, or its proxy page's controller),
  /// made once typing pauses for `delay` ms or at once by `flush`. One
  /// scheduled for another owner is made first, so that it stays the one
  /// it was typed for. `timers` ({ setTimeout, clearTimeout }) for tests.
  class PendingSave {
    constructor(delay = 350, timers = globalThis) {
      this.delay = delay;
      this.timers = timers;
      this.timer = null;
      this.pending = null;
    }
    /// Saves `run` once typing pauses; a save waiting for another owner is
    /// made now.
    schedule(owner, run) {
      if (this.pending && this.pending.owner !== owner) this.flush();
      this.cancel();
      this.pending = { owner, run };
      this.timer = this.timers.setTimeout(() => this.flush(), this.delay);
    }
    /// Saves `run` now, after any save waiting for another owner; one waiting
    /// for this owner is replaced. A save scheduled while the other owner's
    /// is made is newer, and stays.
    async now(owner, run) {
      if (this.pending && this.pending.owner !== owner) await this.flush();
      else this.cancel();
      return run();
    }
    /// Makes the save waiting, if any.
    flush() {
      const pending = this.pending;
      this.cancel();
      return pending ? pending.run() : Promise.resolve();
    }
    /// Drops the save waiting, if any.
    cancel() {
      if (this.timer !== null) this.timers.clearTimeout(this.timer);
      this.timer = null;
      this.pending = null;
    }
  }
  /// The Sub2api provider forms of each pool: what they show. Everything
  /// else about them is the same: cards name a provider by its id.
  const PROVIDER_FORMS = {
    codex: {
      // The name is the Codex configuration's provider ID, whose gateway
      // the provider serves.
      fields: [
        ["name", "provider_id"],
        ["adminBaseURL", "admin_base_url"],
        ["username", "username"],
        ["password", "password", "password"],
      ],
      secrets: ["password"],
      blank: () => ({
        name: "",
        username: "",
        password: "",
        proxyURL: "",
        adminBaseURL: "",
        importedAccountIDs: [],
        cachedAccounts: [],
        allowInsecureTLS: false,
      }),
    },
    claude: {
      sourceFields: {
        address: [
          ["name", "claude_name"],
          ["baseURL", "claude_base_url"],
          ["apiKeyVariable", "claude_api_key_variable"],
        ],
        settings: [["settings", "claude_settings"]],
      },
      fields: [
        ["adminBaseURL", "admin_base_url"],
        ["username", "username"],
        ["password", "password", "password"],
      ],
      secrets: ["password", "apiKey"],
      blank: () => ({
        mode: "address",
        name: "",
        baseURL: "",
        apiKeyVariable: "",
        settings: "",
        adminBaseURL: "",
        username: "",
        password: "",
        apiKey: "",
        allowInsecureTLS: false,
      }),
    },
    antigravity: {
      // agy's key stays in the environment: the form names its variable,
      // never the key. A switch writes the variable and the Sub2api URL,
      // agy's gateway, to ~/.gemini/.env.
      fields: [
        ["name", "antigravity_name"],
        ["baseURL", "antigravity_base_url"],
        ["apiKeyVariable", "antigravity_api_key_variable"],
        ["adminBaseURL", "admin_base_url"],
        ["username", "username"],
        ["password", "password", "password"],
      ],
      secrets: ["password"],
      blank: () => ({
        name: "",
        baseURL: "",
        apiKeyVariable: "",
        adminBaseURL: "",
        username: "",
        password: "",
        allowInsecureTLS: false,
      }),
    },
    grok: {
      // The variable authenticating Grok Build's Sub2API requests.
      fields: [
        ["name", "grok_name"],
        ["baseURL", "grok_base_url"],
        ["apiKeyVariable", "grok_api_key_variable"],
        ["adminBaseURL", "admin_base_url"],
        ["username", "username"],
        ["password", "password", "password"],
      ],
      secrets: ["password"],
      blank: () => ({
        name: "",
        baseURL: "",
        apiKeyVariable: "",
        adminBaseURL: "",
        username: "",
        password: "",
        allowInsecureTLS: false,
      }),
    },
  };
  /// One pool's unofficial providers (Sub2api, CLIProxyAPI): those saved,
  /// whose cards the pool shows, and the forms editing its Sub2api ones. A
  /// CLIProxyAPI provider is every pool's, edited in one form
  /// (`CliProxyApiProviders`).
  class Providers {
    constructor(invoke, pool = "codex", onUpdate = () => {}) {
      this.invoke = invoke;
      this.pool = pool;
      this.form = PROVIDER_FORMS[pool];
      this.onUpdate = onUpdate;
      this.providers = new Map();
      this.persisted = [];
      this.notice = null;
      this.saving = false;
    }
    /// A form's draft of a saved provider, its secrets blank: a blank one
    /// keeps the saved secret.
    draft(saved) {
      const draft = structuredClone(saved);
      for (const key of this.form.secrets) draft[key] = "";
      return draft;
    }
    /// The key naming a provider in its cards' ids: its id.
    key(provider) {
      return provider?.id ?? "";
    }
    name(provider) {
      return provider?.name || "";
    }
    /// The title of a Sub2api card's provider: its saved name, else its id,
    /// as one synced before the provider list was read names it.
    cardTitle(account) {
      const key = P.providerKey(account);
      const provider = key !== null ? this.find(key) : null;
      return this.name(provider) || key || P.providerTitle(account?.providerKind);
    }
    /// The import menu's Sub2api choices: the saved providers, each syncing
    /// its cards with `sync(key)`.
    syncChoices(sync) {
      return this.persisted.map((provider) => ({
        label: this.name(provider),
        run: () => sync(this.key(provider)),
      }));
    }
    /// The saved provider whose cards carry this key.
    find(key) {
      const wanted = String(key).trim().toLowerCase();
      return (
        this.persisted.find(
          (p) => this.key(p).trim().toLowerCase() === wanted,
        ) || null
      );
    }
    isSaved(id) {
      return this.persisted.some((p) => p.id === id);
    }
    /// Takes the saved providers. A form being edited keeps its edits,
    /// taking only its provider's cards.
    accept(persisted) {
      this.persisted = persisted;
      for (const saved of persisted) {
        const name = this.name(saved).trim().toLowerCase();
        const draft = [...this.providers.values()].find(
          (p) =>
            p.id === saved.id ||
            (name && this.name(p).trim().toLowerCase() === name),
        );
        if (draft) {
          for (const key of ["cachedAccounts", "importedAccountIDs"])
            draft[key] = structuredClone(saved[key] || []);
        } else this.providers.set(saved.id, this.draft(saved));
      }
    }
    async load() {
      this.accept(await this.invoke("providers", { pool: this.pool }));
      return this.persisted;
    }
    /// The forms the pool's settings show: its Sub2api providers'.
    forms() {
      return [...this.providers].filter(([, provider]) => provider.kind !== "cliproxyapi");
    }
    /// A new, unsaved form of a Sub2api provider, saved as before kinds
    /// existed, without one; returns its id.
    add() {
      const id = crypto.randomUUID();
      this.providers.set(id, { id, ...this.form.blank() });
      return id;
    }
    /// The fields of a Sub2api provider's form, as `[key, label, type,
    /// keep]`: its pool's (a Claude provider's as its `mode` configures it),
    /// and its administrator's sign-in. `keep` is the placeholder of a saved
    /// secret.
    fields(provider, mode) {
      const own = this.form.sourceFields
        ? [...this.form.sourceFields[mode], ...this.form.fields]
        : this.form.fields;
      return own.map(([key, label, type]) => [
        key,
        `settings.sub2api.${label}`,
        type,
        "settings.sub2api.password_keep",
      ]);
    }
    /// The notice of a saved or removed provider.
    savedNotice() {
      return "settings.notice.sub2api_saved";
    }
    /// Replaces a form, in its place, by its saved provider.
    replace(id, saved) {
      const entries = [...this.providers].map(([key, value]) =>
        key === id ? [saved.id, this.draft(saved)] : [key, value],
      );
      this.providers.clear();
      for (const [key, value] of entries) this.providers.set(key, value);
    }
    async save(id) {
      if (this.saving) return;
      this.saving = true;
      try {
        const saved = await this.invoke("save_provider", {
          pool: this.pool,
          value: structuredClone(this.providers.get(id)),
        });
        this.replace(id, saved);
        const index = this.persisted.findIndex((p) => p.id === saved.id);
        if (index >= 0) this.persisted[index] = saved;
        else this.persisted.push(saved);
        this.notice = this.savedNotice(saved);
        this.onUpdate();
        return saved;
      } catch (error) {
        this.notice = messageOf(error);
        throw error;
      } finally {
        this.saving = false;
      }
    }
    async remove(id) {
      if (this.saving) return;
      this.saving = true;
      try {
        const removed = this.providers.get(id);
        if (this.isSaved(id))
          await this.invoke("remove_provider", { pool: this.pool, id });
        this.providers.delete(id);
        this.persisted = this.persisted.filter((p) => p.id !== id);
        this.notice = this.savedNotice(removed);
        this.onUpdate();
      } catch (error) {
        this.notice = messageOf(error);
        throw error;
      } finally {
        this.saving = false;
      }
    }
  }
  /// The switch's fields each pool keeps of a CLIProxyAPI provider, as
  /// `[key, label]` (cliproxyapi/shared.rs): Codex's is the provider of its
  /// name in `config.toml`.
  const CLIPROXYAPI_POOL_FIELDS = {
    claude: [
      ["baseURL", "settings.sub2api.claude_base_url"],
      ["apiKeyVariable", "settings.sub2api.claude_api_key_variable"],
    ],
    antigravity: [
      ["baseURL", "settings.sub2api.antigravity_base_url"],
      ["apiKeyVariable", "settings.sub2api.antigravity_api_key_variable"],
    ],
    grok: [
      ["baseURL", "settings.sub2api.grok_base_url"],
      ["apiKeyVariable", "settings.sub2api.grok_api_key_variable"],
    ],
  };
  /// The CLIProxyAPI providers, which every pool shares: those saved and
  /// the forms editing them, each with what every pool shares of it and
  /// each pool's switch (`pools`), any of which may be left blank.
  class CliProxyApiProviders {
    constructor(invoke, onUpdate = () => {}) {
      this.invoke = invoke;
      this.onUpdate = onUpdate;
      this.providers = new Map();
      this.persisted = [];
      this.notice = null;
      this.saving = false;
    }
    /// The fields of a form, as `[key, label, type]`.
    fields() {
      return [
        ["name", "settings.cliproxyapi.name"],
        ["adminBaseURL", "settings.cliproxyapi.management_base_url"],
        ["password", "settings.cliproxyapi.management_key", "password"],
      ];
    }
    /// The pools whose switch a form edits, with their fields.
    poolFields() {
      return Object.entries(CLIPROXYAPI_POOL_FIELDS);
    }
    blank(id) {
      const pools = Object.fromEntries(
        Object.entries(CLIPROXYAPI_POOL_FIELDS).map(([pool, fields]) => [
          pool,
          Object.fromEntries(fields.map(([key]) => [key, ""])),
        ]),
      );
      return {
        id,
        kind: "cliproxyapi",
        name: "",
        adminBaseURL: "",
        password: "",
        allowInsecureTLS: false,
        pools,
      };
    }
    /// A form's draft of a saved provider, its key blank, which keeps it.
    draft(saved) {
      const draft = { ...this.blank(saved.id), ...structuredClone(saved), password: "" };
      for (const [pool, fields] of Object.entries(CLIPROXYAPI_POOL_FIELDS))
        for (const [key] of fields) draft.pools[pool] = { ...draft.pools[pool], [key]: saved.pools?.[pool]?.[key] || "" };
      return draft;
    }
    isSaved(id) {
      return this.persisted.some((p) => p.id === id);
    }
    /// Takes the saved providers; a form being edited keeps its edits.
    accept(persisted) {
      this.persisted = persisted;
      for (const saved of persisted)
        if (!this.providers.has(saved.id)) this.providers.set(saved.id, this.draft(saved));
    }
    async load() {
      this.accept(await this.invoke("cliproxyapi_providers"));
      return this.persisted;
    }
    /// A new, unsaved form; returns its id.
    add() {
      const id = crypto.randomUUID();
      this.providers.set(id, this.blank(id));
      return id;
    }
    async save(id) {
      if (this.saving) return;
      this.saving = true;
      try {
        const draft = structuredClone(this.providers.get(id));
        // A new one is named by the core.
        if (!this.isSaved(id)) draft.id = "";
        const saved = await this.invoke("save_cliproxyapi_provider", { value: draft });
        const entries = [...this.providers].map(([key, value]) =>
          key === id ? [saved.id, this.draft(saved)] : [key, value],
        );
        this.providers = new Map(entries);
        const index = this.persisted.findIndex((p) => p.id === saved.id);
        if (index >= 0) this.persisted[index] = saved;
        else this.persisted.push(saved);
        this.notice = "settings.notice.cliproxyapi_saved";
        this.onUpdate();
        return saved;
      } catch (error) {
        this.notice = messageOf(error);
        // What a failed save left saved shows, to be edited or removed.
        await this.load().catch(() => {});
        throw error;
      } finally {
        this.saving = false;
      }
    }
    async remove(id) {
      if (this.saving) return;
      this.saving = true;
      try {
        if (this.isSaved(id)) await this.invoke("remove_cliproxyapi_provider", { id });
        this.providers.delete(id);
        this.persisted = this.persisted.filter((p) => p.id !== id);
        this.notice = "settings.notice.cliproxyapi_saved";
        this.onUpdate();
      } catch (error) {
        this.notice = messageOf(error);
        throw error;
      } finally {
        this.saving = false;
      }
    }
  }
  class Settings {
    constructor(invoke, onUpdate = () => {}) {
      this.invoke = invoke;
      this.onUpdate = onUpdate;
      this.currentProvider = "openai";
      this.notice = null;
    }
    /// The Codex provider in use, as the account cards last changed it.
    accept(currentProvider = this.currentProvider) {
      this.currentProvider = currentProvider;
    }
    async savePreferences(current, patch) {
      const updated = { ...current, ...patch };
      await this.invoke("save_preferences", { value: updated });
      this.notice = "settings.notice.updated";
      this.onUpdate(updated);
      return updated;
    }
    quit() {
      return this.invoke("quit_app");
    }
  }
  /// The time, and waiting, as the Accounts controller has them.
  const REAL_CLOCK = {
    now: () => Date.now(),
    sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
  };
  /// Seconds after a hi's reply at which its card is refreshed again, until
  /// its 5h window shows.
  const SAY_HI_LOOKS = [10, 30, 90, 180];
  /// How long a card shows its token as just renewed.
  const RENEWED_SHOWN_MS = 2200;
  /// A proxy as its card's back edits it: on, with its address; off for
  /// "direct://".
  function proxyDraft(proxyUrl) {
    const direct = proxyUrl === "direct://";
    return { enabled: !direct, url: direct ? "" : proxyUrl || "" };
  }
  /// The proxy saved for a draft: none as "direct://"; blank inherits.
  function proxyValue(draft) {
    return draft.enabled ? draft.url : "direct://";
  }
  /// One pool's accounts and what each card has under way: its refresh, its
  /// hi and the refreshes that follow it, its renewal and the moment it
  /// shows as done, and which pool's windows its model switch shows. The
  /// cards read it through `cardState`. It also holds the card turned over
  /// to edit it and its draft. `options.pool` names the pool;
  /// `options.clock` ({ now, sleep }) is the time it waits by.
  /// The app's sign-ins: one at a time, whichever pool's, so a pool's
  /// sign-in waits for any pool's cancelled one to end (Accounts.login).
  /// `cancel` is the cancel being sent.
  const SIGN_INS = { cancel: null };
  class Accounts {
    constructor(
      invoke,
      initial = null,
      onUpdate = () => {},
      onMutation = () => {},
      options = {},
    ) {
      this.invoke = invoke;
      this.pool = options.pool ?? null;
      this.clock = options.clock ?? REAL_CLOCK;
      this.onMutation = onMutation;
      this.accounts = initial || [];
      this.directory = [];
      this.onUpdate = onUpdate;
      this.loaded = initial !== null;
      this.manualRefreshing = false;
      this.refreshingIds = new Set();
      this.remoteRefreshingIds = new Set();
      // How far each card's hi got: "sent" until the model replies, which
      // locks its refresh, then "received" until its 5h window shows.
      this.hi = new Map();
      this.renewingIds = new Set();
      // Cards showing their renewal done, each with the renewal that marked it,
      // so an earlier renewal's moment ending cannot clear a later one's.
      this.renewedIds = new Map();
      // Cards whose usage rows show their third-party pool (the model switch).
      this.thirdPartyIds = new Set();
      // The card turned over to edit it, as a widget's, one at a time: its
      // flip key (an account's id, or `sub2api:<provider>` for a provider's
      // group) and its draft, which stays over renders: the proxy, its error
      // and whether it is being saved, the greeting schedule once read, a
      // time being added and a removal awaiting confirmation.
      this.editing = null;
      // The card being turned over, until its back shows, and the turn's
      // number: a later turn, or a cancel, supersedes one under way.
      this.turning = null;
      this.turnGeneration = 0;
      this.pendingGeneration = 0;
      this.loadGeneration = 0;
      this.loginGeneration = 0;
      // The sign-in under way: its id once begun, the request beginning it
      // until then; and the app's sign-ins, every pool's (SIGN_INS).
      this.loginId = null;
      this.loginBeginning = null;
      this.signIns = options.signIns ?? SIGN_INS;
      this.adding = false;
      this.authorizing = null;
      this.importing = false;
      this.notice = null;
      this.error = null;
      this.pendingError = null;
      this.discovery = null;
    }
    get refreshing() {
      return (
        this.manualRefreshing ||
        this.refreshingIds.size > 0 ||
        this.remoteRefreshingIds.size > 0 ||
        [...this.hi.values()].includes("sent") ||
        this.renewingIds.size > 0
      );
    }
    /// Whether a card's refresh, hi or renewal waits for another one.
    busy(id) {
      return (
        this.manualRefreshing ||
        this.refreshingIds.has(id) ||
        this.remoteRefreshingIds.has(id) ||
        this.hi.get(id) === "sent" ||
        this.renewingIds.has(id)
      );
    }
    /// What a card has under way, as card-view.js's `ui(id)`.
    cardState(id) {
      return {
        refreshing: this.refreshingIds.has(id) || this.remoteRefreshingIds.has(id),
        hi: this.hi.get(id) ?? null,
        renewing: this.renewingIds.has(id),
        renewed: this.renewedIds.has(id),
        thirdParty: this.thirdPartyIds.has(id),
      };
    }
    /// Shows a card's third-party pool in its usage rows, or its own.
    showThirdParty(id, shown) {
      if (shown) this.thirdPartyIds.add(id);
      else this.thirdPartyIds.delete(id);
      this.onUpdate();
    }
    get spinner() {
      return this.manualRefreshing;
    }
    get pending() {
      return P.pendingCards(this.accounts, this.directory);
    }
    get visible() {
      return P.visibleAccounts(this.accounts);
    }
    presentation(collapsed = false) {
      const pending = collapsed ? [] : this.pending;
      return {
        ids: this.visible.map((a) => a.id),
        pending,
        pendingError: pending.length ? this.pendingError : null,
        showPending: pending.length > 0,
      };
    }
    get canRefresh() {
      return true;
    }

    syncActivity(ids) {
      this.remoteRefreshingIds = new Set(ids);
      this.onUpdate();
    }
    accept(accounts) {
      this.accounts = accounts;
      this.error = null;
      this.loaded = true;
      this.onUpdate();
    }
    async load() {
      const generation = ++this.loadGeneration;
      try {
        const [accounts, directory] = await Promise.all([
          this.invoke("accounts"),
          this.invoke("workspace_directory", { refresh: false }),
        ]);
        if (generation === this.loadGeneration) {
          this.directory = directory;
          this.accept(accounts);
        }
        return this.accounts;
      } catch (error) {
        if (generation === this.loadGeneration) {
          this.error = messageOf(error);
          this.loaded = true;
          this.onUpdate();
        }
        throw error;
      }
    }
    /// Loads the cards along with the page's `others` requests, then gives
    /// the cards as they are once all are in, with the others' answers: a
    /// later load may have finished while the others were pending.
    async loadAlong(others) {
      const [, ...answers] = await Promise.all([this.load(), ...others]);
      return { accounts: this.accounts, answers };
    }
    async refresh(id = null) {
      // Refreshing all waits for every refresh; one account only for itself.
      const busy = id === null ? this.refreshing : this.busy(id);
      if (busy) return false;
      if (id === null) this.manualRefreshing = true;
      else this.refreshingIds.add(id);
      this.onUpdate();
      try {
        await this.invoke("refresh_usage", { id });
        await this.load();
        await this.refreshPending();
        this.notice = "accounts.notice.usage_refreshed";
        return true;
      } catch (error) {
        this.notice = messageOf(error);
        await this.load();
        throw error;
      } finally {
        this.manualRefreshing = false;
        this.refreshingIds.delete(id);
        this.onUpdate();
      }
    }
    /// What a flip key names among the shown cards: an account, maybe one of
    /// a Sub2api provider's, or a provider's group; null for none.
    flipTarget(key) {
      const accounts = this.visible;
      if (key.startsWith("sub2api:")) {
        const provider = key.slice("sub2api:".length);
        const members = accounts.filter((a) => P.providerKey(a) === provider);
        return members.length ? { group: { key: provider, accounts: members } } : null;
      }
      const account = accounts.find((a) => a.id === key);
      if (!account) return null;
      // A provider's only card is an ordinary card, as P.providerGroups shows it.
      const provider = P.providerKey(account);
      const grouped =
        provider !== null && accounts.filter((a) => P.providerKey(a) === provider).length > 1;
      return { account, grouped };
    }
    /// Whether a card is being edited, or turned over to be.
    isEditing(key) {
      return this.editing?.key === key || this.turning === key;
    }
    /// Starts turning a card over to edit it, superseding any other turn,
    /// and reads its greeting schedule meanwhile (`failed` hears why it could
    /// not). Null for a card being edited or turned already, or without a
    /// back. Otherwise the function that, once the card is edge-on, shows its
    /// back with its draft, unless a later turn or a cancel came first. The
    /// card edited before is the caller's to turn back (`close`).
    edit(key, failed = () => {}) {
      if (this.isEditing(key)) return null;
      const target = this.flipTarget(key);
      if (!V.hasBack(target, this.pool)) return null;
      const state = {
        key,
        proxy: target.grouped ? null : proxyDraft(P.proxyOwner(target).proxyUrl),
        proxyError: null,
        saving: false,
        schedule: null,
        // The schedule last saved (as read, at first), the edits made to the
        // draft's and the save of it under way (saveGreeting).
        greetingSaved: null,
        greetingEdits: 0,
        greetingSaving: null,
        adding: null,
        confirmRemove: false,
      };
      const schedule =
        target.account && V.supportsSayHi(target.account, this.pool)
          ? this.invoke("greeting_schedule", { id: target.account.id }).catch((error) => {
              failed(error);
              return null;
            })
          : null;
      const generation = ++this.turnGeneration;
      this.turning = key;
      return async () => {
        state.schedule = await schedule;
        if (state.schedule)
          state.greetingSaved = { auto: state.schedule.auto, times: [...(state.schedule.times || [])] };
        if (generation !== this.turnGeneration) return;
        this.turning = null;
        this.editing = state;
      };
    }
    /// Cancels a card's turn under way, whose back then never shows, and
    /// gives the card being edited, for the caller to turn back and close.
    cancelTurn() {
      if (this.turning !== null) {
        this.turning = null;
        this.turnGeneration++;
      }
      return this.editing;
    }
    /// Ends editing `state` without saving, unless another card's editing
    /// has taken its place.
    close(state) {
      if (this.editing === state) this.editing = null;
    }
    /// Ends any editing at once, as when other cards are shown.
    stopEditing() {
      const state = this.cancelTurn();
      this.editing = null;
      return state;
    }
    /// Saves the draft's proxy, when changed; false with its error, which
    /// keeps the card open to correct it.
    async saveProxyDraft(state) {
      const target = this.flipTarget(state.key);
      if (!target) return true;
      const owner = P.proxyOwner(target);
      const value = proxyValue(state.proxy);
      if (value.trim() === (owner.proxyUrl || "").trim()) return true;
      state.saving = true;
      try {
        await this.saveProxy(owner.id, value);
        state.proxyError = null;
        return true;
      } catch (error) {
        state.proxyError = String(error?.message ?? error);
        return false;
      } finally {
        state.saving = false;
      }
    }
    /// Takes an account's greeting schedule into the draft at once, then
    /// saves it after any save of the draft still under way: each save sends
    /// the draft as it is then, so a later edit is never undone by an
    /// earlier one, and one already sent is not sent again. A save that
    /// fails, with no later edit to save, puts back the schedule saved.
    saveGreeting(state, id, auto, times) {
      state.schedule = { ...state.schedule, auto, times };
      const generation = (state.greetingEdits = (state.greetingEdits ?? 0) + 1);
      const save = async () => {
        const draft = { auto: state.schedule.auto, times: [...state.schedule.times] };
        if (JSON.stringify(draft) === JSON.stringify(state.greetingSaved)) return;
        try {
          await this.invoke("set_greeting_schedule", { id, ...draft });
          state.greetingSaved = draft;
        } catch (error) {
          if (generation === state.greetingEdits && state.greetingSaved)
            state.schedule = { ...state.schedule, ...state.greetingSaved };
          throw error;
        }
      };
      const result = (state.greetingSaving ?? Promise.resolve()).then(save, save);
      state.greetingSaving = result.catch(() => {});
      return result;
    }
    /// Says hi with an account, which starts its 5h window, then refreshes
    /// its card over the next minutes until the window shows: by these
    /// refreshes or any other, at once; still not shown at the last, then.
    /// A card with a model switch greets the pool it shows alone. Resolves
    /// to "started" or "not_started", or false when the card was busy.
    async sayHi(id) {
      if (this.hi.has(id) || this.busy(id)) return false;
      const card = () => this.accounts.find((a) => a.id === id);
      const target = card()
        ? V.helloTarget(card(), { pool: this.pool, ui: (card) => this.cardState(card) })
        : null;
      const shown = () => {
        const account = card();
        return (
          !account ||
          (!!account.usage &&
            !V.fiveHourIdle(account, this.pool, target, this.clock.now() / 1000))
        );
      };
      this.hi.set(id, "sent");
      this.onUpdate();
      try {
        try {
          await this.invoke("say_hi", target === null ? { id } : { id, thirdParty: target });
        } finally {
          await this.load().catch(() => {});
        }
        this.hi.set(id, "received");
        this.onUpdate();
        const replied = this.clock.now();
        // Always fetch usage after the reply; a cached reset cannot confirm this hi.
        await this.refresh(id);
        for (const at of SAY_HI_LOOKS) {
          if (shown()) break;
          await this.load();
          // Until the next refresh, the window is looked for each second.
          const due = replied + at * 1000;
          while (!shown() && this.clock.now() < due)
            await this.clock.sleep(Math.min(1000, due - this.clock.now()));
          if (shown()) break;
          await this.refresh(id);
        }
        return shown() ? "started" : "not_started";
      } finally {
        this.hi.delete(id);
        this.onUpdate();
      }
    }
    /// Renews an account's tokens at once; its usage refreshes with them,
    /// and once reloaded its card shows the renewal done for a moment.
    async renewNow(id) {
      if (this.busy(id)) return false;
      this.renewingIds.add(id);
      this.onUpdate();
      let renewed = false;
      try {
        await this.invoke("renew_now", { id });
        renewed = true;
        return true;
      } finally {
        await this.load().catch(() => {});
        this.renewingIds.delete(id);
        if (renewed) {
          const mark = {};
          this.renewedIds.set(id, mark);
          this.clock.sleep(RENEWED_SHOWN_MS).then(() => {
            if (this.renewedIds.get(id) !== mark) return;
            this.renewedIds.delete(id);
            this.onUpdate();
          });
        }
        this.onUpdate();
      }
    }
    /// Refreshes one card, or every card without an id: a Sub2api
    /// provider's card refreshes with all of its provider's.
    refreshCard(id = null) {
      const account = id === null ? null : this.accounts.find((a) => a.id === id);
      const provider = account ? P.providerKey(account) : null;
      if (provider === null) return this.refresh(id);
      const ids = this.accounts.filter((a) => P.providerKey(a) === provider).map((a) => a.id);
      return this.refreshProvider(provider, ids);
    }
    /// Every card of one Sub2api provider in a single pass; only its cards
    /// show as refreshing.
    async refreshProvider(provider, ids) {
      const busy =
        this.manualRefreshing ||
        ids.some(
          (id) =>
            this.refreshingIds.has(id) ||
            this.remoteRefreshingIds.has(id) ||
            this.hi.get(id) === "sent",
        );
      if (busy) return false;
      for (const id of ids) this.refreshingIds.add(id);
      this.onUpdate();
      try {
        await this.invoke("refresh_provider", { id: provider, sync: false });
        await this.load();
        this.notice = "accounts.notice.usage_refreshed";
        return true;
      } catch (error) {
        this.notice = messageOf(error);
        await this.load();
        throw error;
      } finally {
        for (const id of ids) this.refreshingIds.delete(id);
        this.onUpdate();
      }
    }
    async refreshPending() {
      const generation = ++this.pendingGeneration;
      try {
        const rows = await this.invoke("workspace_directory", {
          refresh: true,
        });
        if (generation === this.pendingGeneration) {
          this.directory = rows;
          this.pendingError = null;
          this.onUpdate();
        }
      } catch (error) {
        if (
          generation === this.pendingGeneration &&
          !/cancel/i.test(messageOf(error))
        ) {
          this.pendingError = messageOf(error);
          this.onUpdate();
        }
      }
    }
    cancelPending() {
      this.pendingGeneration++;
      return this.invoke("cancel_workspace_refresh");
    }
    async dismissPending(id) {
      const card = this.pending.find((p) => p.id === id);
      if (!card) return;
      const oldAccounts = this.accounts,
        oldDirectory = this.directory;
      const cancellation = this.cancelPending();
      this.accounts = this.accounts.map((a) =>
        a.id === card.cardId ? { ...a, displayStatus: "deleted" } : a,
      );
      this.directory = this.directory.map((e) =>
        e.workspaceId === card.workspaceId
          ? { ...e, visibility: "deleted" }
          : e,
      );
      this.onUpdate();
      try {
        await cancellation;
        await this.invoke(
          card.cardId ? "delete_account" : "dismiss_workspace",
          { id: card.cardId || card.workspaceId },
        );
        if (card.cardId) this.onMutation();
        this.notice = "accounts.pending.notice.dismissed";
      } catch (error) {
        this.accounts = oldAccounts;
        this.directory = oldDirectory;
        this.onUpdate();
        throw error;
      }
    }
    /// Labels of the accounts an import added or updated, for Swift's
    /// "Account imported: …" notices; the commands only report a count.
    importedLabels(before) {
      const previous = new Map(before.map((a) => [a.id, a.updatedAt]));
      return this.accounts
        .filter((a) => !previous.has(a.id) || previous.get(a.id) !== a.updatedAt)
        .map((a) => a.label);
    }
    /// As Swift's isImporting: only the import controls wait for an import.
    async runImport(task) {
      if (this.importing) return null;
      this.importing = true;
      this.onUpdate();
      try {
        const before = this.accounts;
        if ((await task()) === null) return null;
        await this.load();
        this.onMutation();
        this.discovery = this.refreshPending();
        return this.importedLabels(before);
      } finally {
        this.importing = false;
        this.onUpdate();
      }
    }
    importCurrent() {
      return this.runImport(() => this.invoke("import_current"));
    }
    async removeAccount(id) {
      await this.removeAccounts([id]);
    }
    /// A Sub2api provider's cards go together; the list reloads once, even
    /// when a removal fails partway.
    async removeAccounts(ids) {
      try {
        for (const id of ids) await this.invoke("delete_account", { id });
      } finally {
        await this.load();
        this.onMutation();
      }
    }
    async saveProxy(id, value) {
      await this.invoke("save_account_proxy", { id, value });
      await this.load();
      this.onMutation();
    }
    /// "never" or "always".
    async setRenewal(id, renewal) {
      await this.invoke("set_renewal", { id, renewal });
      await this.load();
      this.onMutation();
    }
    importDocument(content) {
      return this.runImport(() => this.invoke("import_json", { content }));
    }
    /// Null when the file picker was cancelled.
    importFile() {
      return this.runImport(() => this.invoke("import_account_file"));
    }
    /// Adds or reauthenticates an account (Swift's isAdding), or authorizes a
    /// pending workspace (its authorizingWorkspaceID); one sign-in at a time.
    /// Resolves to the imported account labels, or false when superseded.
    async login(workspace = null, started = () => {}, pendingId = null) {
      if (this.adding || this.authorizing) return false;
      if (pendingId) this.authorizing = pendingId;
      else this.adding = true;
      this.onUpdate();
      const before = this.accounts;
      const generation = ++this.loginGeneration;
      try {
        // A sign-in cancelled just before, in any pool, ends first, as the
        // app has one at a time.
        while (this.signIns.cancel) {
          await this.signIns.cancel;
          if (generation !== this.loginGeneration) return false;
        }
        const beginning = this.invoke("begin_login", { workspace });
        this.loginBeginning = beginning;
        const id = await beginning;
        // Cancelled meanwhile: cancelLogin ends it by its id.
        if (generation !== this.loginGeneration) return false;
        this.loginBeginning = null;
        this.loginId = id;
        started();
        await this.invoke("finish_login", { id });
        if (generation !== this.loginGeneration) return false;
        await this.load();
        this.onMutation();
        this.discovery = this.refreshPending();
        return this.importedLabels(before);
      } catch (error) {
        if (generation === this.loginGeneration) {
          this.notice = error?.message || error?.code || messageOf(error);
          if (
            workspace &&
            /workspace_deactivated|workspace (?:is|has been) deactivated/i.test(
              this.notice,
            )
          ) {
            this.directory = this.directory.map((e) =>
              e.workspaceId === workspace ? { ...e, status: "deactivated" } : e,
            );
            this.notice = "error.accounts.workspace_deactivated";
            this.onUpdate();
          }
          throw error;
        }
        return false;
      } finally {
        if (generation === this.loginGeneration) {
          this.loginId = null;
          this.loginBeginning = null;
          this.adding = false;
          this.authorizing = null;
        }
        this.onUpdate();
      }
    }
    /// Cancels the sign-in under way by its id, once it has one, so that a
    /// sign-in begun after it is never the one cancelled.
    cancelLogin() {
      this.loginGeneration++;
      const id = this.loginId;
      const beginning = this.loginBeginning;
      this.loginId = null;
      this.loginBeginning = null;
      this.adding = false;
      this.authorizing = null;
      this.onUpdate();
      // Nothing begun to cancel, as while waiting for another pool's cancel:
      // that one is still the app's to wait for.
      if (id === null && !beginning) return Promise.resolve();
      const cancel =
        id !== null
          ? this.invoke("cancel_login", { id })
          : beginning.then(
              (id) => this.invoke("cancel_login", { id }),
              () => {},
            );
      // Ended once every cancel sent before it has.
      const before = this.signIns.cancel;
      const ended = Promise.all([before, cancel.catch(() => {})]).then(() => {
        if (this.signIns.cancel === ended) this.signIns.cancel = null;
      });
      this.signIns.cancel = ended;
      return cancel;
    }
  }
  /// Each pool's API proxy: its own port and key, public tunnel and remote
  /// servers. `options.pool` names the pool; the invoke names it to the app.
  const DEFAULT_PROXY_PORTS = { codex: "8787", claude: "8788", antigravity: "8789" };
  class Proxy {
    constructor(invoke, onUpdate = () => {}, options = {}) {
      this.invoke = invoke;
      this.onUpdate = onUpdate;
      this.pool = options.pool || "codex";
      this.loading = false;
      this.proxyStatus = { running: false };
      this.tunnelStatus = { installed: false, running: false };
      this.preferredPortText = DEFAULT_PROXY_PORTS[this.pool];
      this.tunnelConfiguration = {
        enabled: false,
        tunnelMode: "quick",
        useHTTP2: false,
        namedHostname: "",
        accountId: "",
        zoneId: "",
        apiToken: "",
        hasAPIToken: false,
      };
      this.remoteServers = [];
      this.remoteStatuses = {};
      this.remoteLogs = {};
      this.remoteDiscoveries = {};
      this.activeActions = {};
      this.feedbacks = {};
      this.timers = {};
      this.notice = null;
      this.lastAppliedSnapshot = null;
      this.lastSyncedConfiguration = null;
      // The tunnel's saves begun, and those under way (configurationRead).
      this.tunnelSaves = 0;
      this.tunnelSaving = 0;
      // The save sent last, which the next waits for (updateTunnel).
      this.tunnelQueue = Promise.resolve();
      this.schedule = options.schedule || setTimeout;
      this.clearSchedule = options.clearSchedule || clearTimeout;
      this.successDelay = options.successDelay ?? 3000;
      this.identityPicker =
        options.identityPicker || (() => this.invoke("pick_identity_file"));
    }
    /// The pool's part of the app's settings: its port and tunnel.
    localConfiguration(settings) {
      const proxy = settings.proxyConfiguration || {};
      if (this.pool === "codex") return proxy;
      return {
        preferredPortText: String(settings[`${this.pool}ProxyPort`] ?? DEFAULT_PROXY_PORTS[this.pool]),
        cloudflared: proxy[`${this.pool}Cloudflared`] || {},
      };
    }
    /// The remote servers of the pool.
    ownServers(servers = []) {
      return servers.filter((server) => (server.pool || "codex") === this.pool);
    }
    /// Taken as the app's configuration is asked for, and given back with
    /// it to applyLocal: a configuration asked for before a save of the
    /// tunnel ended, or while one was under way, may hold the tunnel as it
    /// was, and leaves the tunnel as it is.
    configurationRead() {
      return this.tunnelSaving ? -1 : this.tunnelSaves;
    }
    applyLocal(proxy, configuration, tunnel, read = this.configurationRead()) {
      const stale = read !== this.tunnelSaves || this.tunnelSaving > 0;
      const cf = stale ? {} : configuration.cloudflared || {};
      // A field the configuration leaves out keeps what the app held, not
      // the draft, which may hold an edit not yet saved.
      const held =
        this.lastSyncedConfiguration?.cloudflared || this.tunnelConfiguration;
      return this.applySnapshot({
        syncedAt: Date.now(),
        proxyStatus: proxy,
        preferredProxyPortText:
          configuration.preferredPortText || DEFAULT_PROXY_PORTS[this.pool],
        cloudflaredStatus: tunnel || this.tunnelStatus,
        publicAccessEnabled: cf.enabled ?? held.enabled,
        cloudflaredTunnelMode: cf.tunnelMode || held.tunnelMode,
        cloudflaredUseHTTP2: cf.useHTTP2 ?? held.useHTTP2,
        cloudflaredNamedInput: {
          hostname: cf.namedHostname ?? held.namedHostname,
          apiToken: "",
          accountID: this.tunnelConfiguration.accountId
            ? ""
            : cf.accountId || "",
          zoneID: this.tunnelConfiguration.zoneId ? "" : cf.zoneId || "",
          hasAPIToken: !!cf.hasAPIToken,
        },
        remoteServers: this.remoteServers,
        remoteStatuses: this.remoteStatuses,
        remoteLogs: this.remoteLogs,
        remoteDiscoveries: this.remoteDiscoveries,
      }, { stale });
    }
    /// Takes a snapshot of the proxy; one `stale` leaves the tunnel's
    /// configuration as it is (configurationRead).
    applySnapshot(snapshot, { stale = false } = {}) {
      const normalized = structuredClone(snapshot);
      normalized.remoteLogs = Object.fromEntries(
        Object.entries(snapshot.remoteLogs || {}).map(([id, log]) => [
          id,
          P.normalizeLogs(log),
        ]),
      );
      const comparable = (value) => {
        if (!value) return null;
        const { syncedAt, lastHandledCommandID, lastCommandError, ...rest } =
          value;
        return rest;
      };
      const changed =
        JSON.stringify(comparable(normalized)) !==
        JSON.stringify(comparable(this.lastAppliedSnapshot));
      this.proxyStatus = snapshot.proxyStatus || this.proxyStatus;
      this.tunnelStatus = snapshot.cloudflaredStatus || this.tunnelStatus;
      this.preferredPortText =
        snapshot.preferredProxyPortText ||
        String(snapshot.preferredProxyPort || DEFAULT_PROXY_PORTS[this.pool]);
      const named = snapshot.cloudflaredNamedInput || {};
      const synced = {
        enabled: !!snapshot.publicAccessEnabled,
        tunnelMode: snapshot.cloudflaredTunnelMode || "quick",
        useHTTP2: !!snapshot.cloudflaredUseHTTP2,
        namedHostname: named.hostname || "",
      };
      // The draft keeps what was edited since the app last held it, until it
      // is saved: a snapshot read before then would undo the edit.
      const held = this.lastSyncedConfiguration?.cloudflared;
      // Stale, nothing of the tunnel is taken, the token's place either.
      if (!stale) {
        for (const [key, value] of Object.entries(synced))
          if (!held || this.tunnelConfiguration[key] === held[key])
            this.tunnelConfiguration[key] = value;
        this.tunnelConfiguration.hasAPIToken = !!named.hasAPIToken;
        for (const [local, remote] of [
          ["apiToken", "apiToken"],
          ["accountId", "accountID"],
          ["zoneId", "zoneID"],
        ])
          if (named[remote]) this.tunnelConfiguration[local] = named[remote];
      }
      this.remoteStatuses = snapshot.remoteStatuses || this.remoteStatuses;
      this.remoteServers = snapshot.remoteServers || this.remoteServers;
      this.remoteDiscoveries =
        snapshot.remoteDiscoveries || this.remoteDiscoveries;
      this.remoteLogs = normalized.remoteLogs;
      this.lastSyncedConfiguration = {
        preferredPortText: this.preferredPortText,
        cloudflared: stale && held ? held : synced,
      };
      this.lastAppliedSnapshot = normalized;
      this.onUpdate();
      return changed;
    }
    /// The token typed in, or one saved in the Keychain, which the
    /// configuration never returns.
    get namedTunnelReady() {
      const c = this.tunnelConfiguration;
      return (
        (c.hasAPIToken || !!c.apiToken?.trim()) &&
        [c.accountId, c.zoneId, c.namedHostname].every(
          (value) => !!value?.trim(),
        )
      );
    }
    /// ProxyPageModel.canStartCloudflared.
    get canStartTunnel() {
      return (
        this.tunnelConfiguration.enabled &&
        this.proxyStatus.running &&
        this.proxyStatus.port != null &&
        this.tunnelStatus.installed &&
        !this.tunnelStatus.running &&
        !this.loading &&
        (this.tunnelConfiguration.tunnelMode === "quick" ||
          this.namedTunnelReady)
      );
    }
    /// ProxyPageModel.canEditCloudflaredInput.
    get canEditTunnel() {
      return !this.loading && !this.tunnelStatus.running;
    }
    get apiButtons() {
      return P.proxyButtons(this.proxyStatus.running, this.loading);
    }
    get publicButtons() {
      return P.publicButtons(
        this.tunnelStatus.running,
        this.loading,
        this.canStartTunnel,
      );
    }
    /// The local proxy and tunnel only, as Swift's refreshStatus and tab entry;
    /// remote servers are asked over SSH once, by loadRemoteStatuses().
    async refresh() {
      const read = this.configurationRead();
      const [proxy, configuration, tunnel] = await Promise.all([
        this.invoke("proxy_status"),
        this.invoke("configuration"),
        this.invoke("tunnel_action", { action: "status", value: null }),
      ]);
      this.remoteServers = this.ownServers(configuration.remoteServers);
      this.applyLocal(proxy, this.localConfiguration(configuration), tunnel, read);
      return proxy;
    }
    /// The Refresh button: the local status, under the proxy's loading state.
    refreshStatus() {
      return this.whileLoading(() => this.refresh());
    }
    /// As Swift's first load(): every remote server's status, once.
    async loadRemoteStatuses() {
      if (this.remoteStatusesLoaded) return;
      this.remoteStatusesLoaded = true;
      if (this.remoteServers.length)
        await this.remoteStatusesFor(this.remoteServers);
    }
    async whileLoading(task, notice = null) {
      if (this.loading) return null;
      this.loading = true;
      this.onUpdate();
      try {
        const result = await task();
        if (notice) this.notice = notice;
        return result;
      } finally {
        this.loading = false;
        this.onUpdate();
      }
    }
    rotateKey() {
      return this.whileLoading(
        async () => (this.proxyStatus = await this.invoke("rotate_proxy_key")),
        { style: "success", key: "proxy.notice.api_key_refreshed" },
      );
    }
    /// Saves the tunnel's draft, with `patch` applied; what it saved is then
    /// what the app holds, which later snapshots are compared with. One save
    /// at a time, after those before it, each of the draft as it is when its
    /// turn comes: the app writes the last edit last.
    async updateTunnel(patch = {}) {
      Object.assign(this.tunnelConfiguration, patch);
      this.tunnelSaves++;
      this.tunnelSaving++;
      const save = async () => {
        const value = structuredClone(this.tunnelConfiguration);
        await this.invoke("save_tunnel_configuration", { value });
        this.holdTunnel(value);
      };
      const saved = this.tunnelQueue.then(save, save);
      this.tunnelQueue = saved.catch(() => {});
      try {
        await saved;
      } finally {
        this.tunnelSaving--;
      }
      this.onUpdate();
    }
    /// Takes the tunnel configuration the app holds, as at launch, into the
    /// draft.
    adoptTunnel(cloudflared = {}) {
      Object.assign(this.tunnelConfiguration, cloudflared);
      this.holdTunnel(this.tunnelConfiguration);
    }
    /// Records `value` as the tunnel configuration the app holds.
    holdTunnel(value) {
      this.lastSyncedConfiguration = {
        preferredPortText: this.preferredPortText,
        ...this.lastSyncedConfiguration,
        cloudflared: {
          enabled: !!value.enabled,
          tunnelMode: value.tunnelMode || "quick",
          useHTTP2: !!value.useHTTP2,
          namedHostname: value.namedHostname || "",
        },
      };
    }
    setPublicAccessEnabled(value) {
      return this.updateTunnel({ enabled: value });
    }
    async startProxy(port = Number(this.preferredPortText)) {
      if (!Number.isInteger(port) || port < 1 || port > 65535)
        throw new Error(`Invalid proxy port: ${port}`);
      return this.whileLoading(
        async () =>
          (this.proxyStatus = await this.invoke("start_proxy", { port })),
        { style: "success", key: "proxy.notice.api_proxy_started" },
      );
    }
    stopProxy() {
      return this.whileLoading(
        async () => {
          this.proxyStatus = await this.invoke("stop_proxy");
          this.tunnelStatus = {
            ...this.tunnelStatus,
            running: false,
            publicUrl: null,
          };
          return this.proxyStatus;
        },
        { style: "info", key: "proxy.notice.api_proxy_stopped" },
      );
    }
    async tunnelAction(action) {
      if (action === "start" && !this.canStartTunnel)
        throw new Error("error.cloudflared.start_api_proxy_first");
      const notices = {
        install: { style: "success", key: "proxy.notice.cloudflared_installed" },
        start: { style: "success", key: "proxy.notice.cloudflared_started" },
        stop: { style: "info", key: "proxy.notice.cloudflared_stopped" },
      };
      return this.whileLoading(
        async () =>
          (this.tunnelStatus = await this.invoke("tunnel_action", {
            action,
            value:
              action === "start"
                ? structuredClone(this.tunnelConfiguration)
                : null,
          })),
        notices[action],
      );
    }
    chooseIdentityFile() {
      return this.identityPicker();
    }
    remoteButtons(id) {
      return P.remoteButtons(
        !!this.remoteStatuses[id]?.running,
        this.activeActions[id],
      );
    }
    async remoteStatusesFor(servers) {
      const rows = await Promise.all(
        servers.map(async (server) => {
          try {
            return [
              server.id,
              await this.invoke("remote_action", {
                id: server.id,
                action: "status",
              }),
            ];
          } catch (error) {
            return [server.id, { lastError: messageOf(error), running: false }];
          }
        }),
      );
      Object.assign(this.remoteStatuses, Object.fromEntries(rows));
      this.onUpdate();
      return Object.fromEntries(rows);
    }
    async saveRemote(server, previousId = null) {
      const saved = await this.invoke("save_remote", {
        value: { ...structuredClone(server), pool: this.pool },
        previousId,
      });
      this.remoteServers = this.remoteServers.filter(
        (s) => s.id !== saved.id && s.id !== previousId,
      );
      this.remoteServers.push(saved);
      return saved;
    }
    async adopt(server, previousId) {
      const saved = await this.saveRemote(server, previousId);
      await this.remoteAction(saved.id, "status", saved.label);
      return saved;
    }
    dismissFeedback(id) {
      if (this.timers[id]) this.clearSchedule(this.timers[id]);
      delete this.feedbacks[id];
      delete this.timers[id];
      this.onUpdate();
    }
    /// Swift's withRemoteAction: one card's action locks only that card.
    async withRemote(id, intent, task) {
      if (this.activeActions[id]) return null;
      this.activeActions[id] = intent;
      this.onUpdate();
      try {
        return await task();
      } catch (error) {
        this.notice = { style: "error", message: messageOf(error) };
        return null;
      } finally {
        delete this.activeActions[id];
        this.onUpdate();
      }
    }
    saveRemoteCard(server, previousId = null) {
      return this.withRemote(previousId || server.id, "save", async () => {
        const saved = await this.saveRemote(server, previousId);
        this.notice = {
          style: "success",
          key: "settings.notice.remote_servers_saved",
        };
        return saved;
      });
    }
    removeRemote(id) {
      return this.withRemote(id, "removeLocal", async () => {
        await this.invoke("remove_remote", { id });
        this.remoteServers = this.remoteServers.filter((s) => s.id !== id);
        this.notice = {
          style: "success",
          key: "proxy.notice.remote_server_removed",
        };
        return true;
      });
    }
    async remoteAction(id, action, label = id, intent = action) {
      if (this.activeActions[id]) return null;
      this.activeActions[id] = intent;
      if (action === "deploy") {
        if (this.timers[id]) this.clearSchedule(this.timers[id]);
        this.notice = {
          style: "info",
          key: "proxy.notice.remote_deploying_format",
          args: [label],
        };
        this.feedbacks[id] = {
          state: "deploying",
          key: "proxy.notice.remote_deploying_format",
          args: [label],
        };
      }
      this.onUpdate();
      try {
        const result = await this.invoke("remote_action", { id, action });
        if (action === "logs")
          this.remoteLogs[id] = P.normalizeLogs(result.logs);
        else if (action === "discover") this.remoteDiscoveries[id] = result;
        else this.remoteStatuses[id] = result;
        // The notices of ProxyPageModel+RemoteServerActions.
        const notices = {
          sync: ["success", "proxy.notice.remote_accounts_synced_format"],
          start: ["success", "proxy.notice.remote_started_format"],
          stop: ["info", "proxy.notice.remote_stopped_format"],
          uninstall: ["success", "proxy.notice.remote_uninstalled_format"],
        };
        if (notices[action])
          this.notice = {
            style: notices[action][0],
            key: notices[action][1],
            args: [label],
          };
        else if (action === "discover")
          this.notice = result?.length
            ? {
                style: "success",
                key: "proxy.notice.remote_discovery_found_format",
                args: [String(result.length)],
              }
            : { style: "info", key: "proxy.notice.remote_discovery_empty" };
        if (action === "deploy") {
          this.notice = {
            style: "success",
            key: "proxy.notice.remote_deploy_done_format",
            args: [label],
          };
          this.feedbacks[id] = {
            state: "success",
            key: this.notice.key,
            args: [label],
          };
          this.timers[id] = this.schedule(
            () => this.dismissFeedback(id),
            this.successDelay,
          );

        }
        return result;
      } catch (error) {
        this.notice = { style: "error", message: messageOf(error) };
        if (action === "deploy")
          this.feedbacks[id] = { state: "failure", message: messageOf(error) };
        return null;
      } finally {
        delete this.activeActions[id];
        this.onUpdate();
      }
    }
  }
  return { Settings, Providers, CliProxyApiProviders, Accounts, Proxy, PendingSave, poolInvoke };
});
