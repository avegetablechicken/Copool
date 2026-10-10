/* UI state shared by the actual desktop and migrated controller tests. */
(function (root, factory) {
  const api = factory(
    typeof module === "object" && module.exports
      ? require("./presentation.js")
      : root.CopoolPresentation,
  );
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.CopoolControllers = api;
})(globalThis, function (P) {
  const messageOf = (error) => error?.message || error?.code || String(error);
  class Settings {
    constructor(invoke, onUpdate = () => {}) {
      this.invoke = invoke;
      this.onUpdate = onUpdate;
      this.providers = new Map();
      this.persisted = [];
      this.currentProvider = "openai";
      this.notice = null;
      this.saving = false;
    }
    accept(settings, currentProvider = this.currentProvider) {
      this.currentProvider = currentProvider;
      this.persisted = settings.sub2APIProvider.providers;
      for (const saved of this.persisted) {
        const draft = [...this.providers.values()].find(
          (p) =>
            p.id === saved.id ||
            p.providerID.trim().toLowerCase() ===
              saved.providerID.trim().toLowerCase(),
        );
        if (draft) {
          for (const key of [
            "cachedAccounts",
            "importedAccountIDs",
            "accountProxyURLs",
          ])
            draft[key] = structuredClone(
              saved[key] || (key === "accountProxyURLs" ? {} : []),
            );
        } else
          this.providers.set(saved.id, {
            ...structuredClone(saved),
            password: "",
          });
      }
    }
    async save(id) {
      if (this.saving) return;
      this.saving = true;
      try {
        const saved = await this.invoke("save_provider", {
          value: structuredClone(this.providers.get(id)),
        });
        this.providers.set(id, { ...saved, password: "" });
        const index = this.persisted.findIndex((p) => p.id === id);
        if (index >= 0) this.persisted[index] = saved;
        else this.persisted.push(saved);
        this.notice = "settings.notice.sub2api_saved";
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
        if (this.persisted.some((p) => p.id === id))
          await this.invoke("remove_provider", { id });
        this.providers.delete(id);
        this.persisted = this.persisted.filter((p) => p.id !== id);
        this.notice = "settings.notice.sub2api_saved";
        this.onUpdate();
      } catch (error) {
        this.notice = messageOf(error);
        throw error;
      } finally {
        this.saving = false;
      }
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
  class Accounts {
    constructor(
      invoke,
      initial = null,
      onUpdate = () => {},
      onMutation = () => {},
    ) {
      this.invoke = invoke;
      this.onMutation = onMutation;
      this.accounts = initial || [];
      this.directory = [];
      this.onUpdate = onUpdate;
      this.loaded = initial !== null;
      this.manualRefreshing = false;
      this.refreshingIds = new Set();
      this.remoteRefreshingIds = new Set();
      this.pendingGeneration = 0;
      this.loadGeneration = 0;
      this.loginGeneration = 0;
      this.loginId = null;
      this.adding = false;
      this.notice = null;
      this.error = null;
      this.pendingError = null;
      this.discovery = null;
    }
    get refreshing() {
      return (
        this.manualRefreshing ||
        this.refreshingIds.size > 0 ||
        this.remoteRefreshingIds.size > 0
      );
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
    async refresh(id = null) {
      // Refreshing all waits for every refresh; one account only for itself.
      const busy =
        id === null
          ? this.refreshing
          : this.manualRefreshing ||
            this.refreshingIds.has(id) ||
            this.remoteRefreshingIds.has(id);
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
    async importCurrent() {
      const count = await this.invoke("import_current");
      await this.load();
      this.onMutation();
      this.discovery = this.refreshPending();
      return count;
    }
    async removeAccount(id) {
      await this.invoke("delete_account", { id });
      await this.load();
      this.onMutation();
    }
    async saveProxy(id, value) {
      await this.invoke("save_account_proxy", { id, value });
      await this.load();
      this.onMutation();
    }
    async importDocument(content) {
      const count = await this.invoke("import_json", { content });
      await this.load();
      this.onMutation();
      this.discovery = this.refreshPending();
      return count;
    }
    async importFile() {
      const count = await this.invoke("import_account_file");
      await this.load();
      this.onMutation();
      this.discovery = this.refreshPending();
      return count;
    }
    async login(workspace = null, started = () => {}) {
      if (this.adding) return false;
      this.adding = true;
      const generation = ++this.loginGeneration;
      try {
        const id = await this.invoke("begin_login", { workspace });
        if (generation !== this.loginGeneration) {
          await this.invoke("cancel_login");
          return false;
        }
        this.loginId = id;
        started();
        const count = await this.invoke("finish_login", { id });
        if (generation !== this.loginGeneration) return false;
        await this.load();
        this.onMutation();
        this.discovery = this.refreshPending();
        return count;
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
          this.adding = false;
        }
        this.onUpdate();
      }
    }
    cancelLogin() {
      this.loginGeneration++;
      this.loginId = null;
      this.adding = false;
      return this.invoke("cancel_login");
    }
  }
  class Proxy {
    constructor(invoke, onUpdate = () => {}, options = {}) {
      this.invoke = invoke;
      this.onUpdate = onUpdate;
      this.loading = false;
      this.proxyStatus = { running: false };
      this.tunnelStatus = { installed: false, running: false };
      this.preferredPortText = "8787";
      this.tunnelConfiguration = {
        enabled: false,
        tunnelMode: "quick",
        useHTTP2: false,
        namedHostname: "",
        accountId: "",
        zoneId: "",
        apiToken: "",
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
      this.schedule = options.schedule || setTimeout;
      this.clearSchedule = options.clearSchedule || clearTimeout;
      this.successDelay = options.successDelay ?? 4000;
      this.identityPicker =
        options.identityPicker || (() => this.invoke("pick_identity_file"));
    }
    applyLocal(proxy, configuration, tunnel) {
      const cf = configuration.cloudflared || {};
      return this.applySnapshot({
        syncedAt: Date.now(),
        proxyStatus: proxy,
        preferredProxyPortText: configuration.preferredPortText || "8787",
        cloudflaredStatus: tunnel || this.tunnelStatus,
        publicAccessEnabled: cf.enabled ?? this.tunnelConfiguration.enabled,
        cloudflaredTunnelMode:
          cf.tunnelMode || this.tunnelConfiguration.tunnelMode,
        cloudflaredUseHTTP2: cf.useHTTP2 ?? this.tunnelConfiguration.useHTTP2,
        cloudflaredNamedInput: {
          hostname: cf.namedHostname ?? this.tunnelConfiguration.namedHostname,
          apiToken: "",
          accountID: this.tunnelConfiguration.accountId
            ? ""
            : cf.accountId || "",
          zoneID: this.tunnelConfiguration.zoneId ? "" : cf.zoneId || "",
        },
        remoteServers: this.remoteServers,
        remoteStatuses: this.remoteStatuses,
        remoteLogs: this.remoteLogs,
        remoteDiscoveries: this.remoteDiscoveries,
      });
    }
    applySnapshot(snapshot) {
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
        String(snapshot.preferredProxyPort || 8787);
      const named = snapshot.cloudflaredNamedInput || {};
      Object.assign(this.tunnelConfiguration, {
        enabled: !!snapshot.publicAccessEnabled,
        tunnelMode: snapshot.cloudflaredTunnelMode || "quick",
        useHTTP2: !!snapshot.cloudflaredUseHTTP2,
        namedHostname: named.hostname || "",
      });
      for (const [local, remote] of [
        ["apiToken", "apiToken"],
        ["accountId", "accountID"],
        ["zoneId", "zoneID"],
      ])
        if (named[remote]) this.tunnelConfiguration[local] = named[remote];
      this.remoteStatuses = snapshot.remoteStatuses || this.remoteStatuses;
      this.remoteServers = snapshot.remoteServers || this.remoteServers;
      this.remoteDiscoveries =
        snapshot.remoteDiscoveries || this.remoteDiscoveries;
      this.remoteLogs = normalized.remoteLogs;
      this.lastSyncedConfiguration = {
        preferredPortText: this.preferredPortText,
        cloudflared: {
          enabled: this.tunnelConfiguration.enabled,
          tunnelMode: this.tunnelConfiguration.tunnelMode,
          useHTTP2: this.tunnelConfiguration.useHTTP2,
          namedHostname: this.tunnelConfiguration.namedHostname,
        },
      };
      this.lastAppliedSnapshot = normalized;
      this.onUpdate();
      return changed;
    }
    get canStartTunnel() {
      return (
        this.tunnelConfiguration.enabled &&
        this.proxyStatus.running &&
        this.tunnelStatus.installed &&
        !this.tunnelStatus.running &&
        !this.loading
      );
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
    async refresh() {
      const [proxy, configuration, tunnel] = await Promise.all([
        this.invoke("proxy_status"),
        this.invoke("configuration"),
        this.invoke("tunnel_action", { action: "status", value: null }),
      ]);
      this.remoteServers = configuration.remoteServers;
      this.applyLocal(proxy, configuration.proxyConfiguration, tunnel);
      if (this.remoteServers.length)
        await this.remoteStatusesFor(this.remoteServers);
      return proxy;
    }
    async updateTunnel(patch) {
      Object.assign(this.tunnelConfiguration, patch);
      await this.invoke("save_tunnel_configuration", {
        value: structuredClone(this.tunnelConfiguration),
      });
      this.onUpdate();
    }
    setPublicAccessEnabled(value) {
      return this.updateTunnel({ enabled: value });
    }
    async startProxy(port = Number(this.preferredPortText)) {
      if (!Number.isInteger(port) || port < 1 || port > 65535)
        throw new Error(`Invalid proxy port: ${port}`);
      this.loading = true;
      this.onUpdate();
      try {
        this.proxyStatus = await this.invoke("start_proxy", { port });
        this.onUpdate();
        return this.proxyStatus;
      } finally {
        this.loading = false;
        this.onUpdate();
      }
    }
    async stopProxy() {
      this.loading = true;
      this.onUpdate();
      try {
        this.proxyStatus = await this.invoke("stop_proxy");
        this.tunnelStatus = {
          ...this.tunnelStatus,
          running: false,
          publicUrl: null,
        };
        return this.proxyStatus;
      } finally {
        this.loading = false;
        this.onUpdate();
      }
    }
    async tunnelAction(action) {
      if (action === "start" && !this.canStartTunnel)
        throw new Error("error.cloudflared.start_api_proxy_first");
      this.loading = true;
      this.onUpdate();
      try {
        this.tunnelStatus = await this.invoke("tunnel_action", {
          action,
          value:
            action === "start"
              ? structuredClone(this.tunnelConfiguration)
              : null,
        });
        return this.tunnelStatus;
      } finally {
        this.loading = false;
        this.onUpdate();
      }
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
        value: structuredClone(server),
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
    async remoteAction(id, action, label = id) {
      if (this.activeActions[id]) return null;
      this.activeActions[id] = action;
      if (action === "deploy") {
        if (this.timers[id]) this.clearSchedule(this.timers[id]);
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
  return { Settings, Accounts, Proxy };
});
