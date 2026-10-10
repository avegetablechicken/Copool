"use strict";
let configurationCache = null;
let installedEditors = [];
const providerController = new window.CopoolControllers.Settings(invoke);
const providerDrafts = providerController.providers;
const accountsController = new window.CopoolControllers.Accounts(
  invoke,
  null,
  () => {
    accountsCache = accountsController.accounts;
    renderAccounts(accountsCache);
    renderPendingWorkspaces();
    syncButtons();
  },
  () => invoke("sync_remote_accounts").catch((error) => notice(error, true)),
);
let lastProxyNotice;
const proxyController = new window.CopoolControllers.Proxy(invoke, () => {
  proxyCache = proxyController.proxyStatus;
  renderProxy();
  renderRemoteStates();
  renderTunnelState();
  if (proxyController.notice && proxyController.notice !== lastProxyNotice) {
    lastProxyNotice = proxyController.notice;
    const n = lastProxyNotice;
    notice(n.message || tr(n.key, ...(n.args || [])), n.style === "error");
  }
});
function providerField(form, provider, key, label, type = "text") {
  const row = el("label", "settings-row provider-field");
  row.append(el("span", "", tr(label)));
  const input = el("input");
  input.type = type;
  input.value = provider[key] || "";
  input.autocomplete = type === "password" ? "new-password" : "off";
  input.setAttribute("aria-label", tr(label));
  input.oninput = () => (provider[key] = input.value);
  if (type === "password")
    input.placeholder = tr("settings.sub2api.password_keep");
  row.append(input);
  form.append(row);
}
const expandedProviders = new Set();
function renderProviders() {
  const root = $("provider-forms");
  root.replaceChildren();
  $("provider-count").textContent = String(providerDrafts.size);
  for (const [id, provider] of providerDrafts) {
    const form = el("form", "settings-group provider-form");
    const header = el("div", "settings-row provider-header");
    const toggle = button("", null, "plain provider-toggle", () => {
      if (expandedProviders.has(id)) expandedProviders.delete(id);
      else expandedProviders.add(id);
      renderProviders();
    });
    toggle.type = "button";
    toggle.setAttribute("aria-expanded", String(expandedProviders.has(id)));
    const chevron = icon("chevron-down");
    chevron.classList.add("provider-chevron");
    if (!expandedProviders.has(id)) chevron.classList.add("collapsed");
    toggle.append(
      chevron,
      el("span", "provider-accent"),
      icon("server"),
      el("span", "", provider.providerID || "Sub2api"),
    );
    const remove = button("", "trash", "plain icon-only", () =>
      action(async () => {
        await providerController.remove(id);
        expandedProviders.delete(id);
        renderProviders();
      }),
    );
    remove.type = "button";
    remove.title = tr("settings.sub2api.remove_provider");
    remove.setAttribute("aria-label", remove.title);
    header.append(toggle, remove);
    form.append(header);
    if (expandedProviders.has(id)) {
      const body = el("div", "provider-body");
      for (const [key, label, type] of [
        ["providerID", "provider_id"],
        ["adminBaseURL", "admin_base_url"],
        ["username", "username"],
        ["password", "password", "password"],
      ]) {
        providerField(body, provider, key, `settings.sub2api.${label}`, type);
        if (key === "adminBaseURL")
          body.append(
            el(
              "p",
              "caption secondary",
              tr("settings.sub2api.admin_base_url_help"),
            ),
          );
      }
      const row = el("label", "settings-row");
      row.append(el("span", "", tr("settings.sub2api.allow_insecure_tls")));
      const check = el("input", "switch");
      check.type = "checkbox";
      check.checked = !!provider.allowInsecureTLS;
      check.onchange = () => (provider.allowInsecureTLS = check.checked);
      row.append(check);
      body.append(row);
      const actions = el("div", "settings-row provider-save");
      const save = button(tr("common.save"), "import", "capsule prominent");
      save.type = "submit";
      actions.append(save);
      body.append(actions);
      form.append(body);
    }
    form.onsubmit = (event) => {
      event.preventDefault();
      action(async () => {
        await providerController.save(id);
        configurationCache = await invoke("configuration");
        renderProviders();
      }, tr("settings.notice.sub2api_saved"));
    };
    root.append(form);
  }
}
async function openProviderImport() {
  await action(async () => {
    const settings = await invoke("configuration");
    const providers = settings.sub2APIProvider.providers;
    if (!providers.length) {
      throw new Error(
        extra(
          "Add a Sub2api provider in Settings first.",
          "请先在设置中添加 Sub2api 服务商。",
        ),
      );
    }
    const failures = [];
    let count = 0;
    for (const provider of providers) {
      try {
        count += await invoke("refresh_provider", {
          id: provider.providerID,
          sync: true,
        });
      } catch (error) {
        failures.push(`${provider.providerID}: ${error}`);
      }
    }
    if (failures.length) throw new Error(failures.join("\n"));
    notice(tr("accounts.notice.sub2api_imported_format", count));
  });
}
async function initializeFeatures() {
  [configurationCache, installedEditors] = await Promise.all([
    invoke("configuration"),
    invoke("installed_editors"),
  ]);
  providerController.accept(configurationCache);
  renderProviders();
  const none = el("option", "", tr("common.none"));
  none.value = "";
  $("restart-target").replaceChildren(
    none,
    ...installedEditors.map((editor) => {
      const option = el("option", "", editor.label);
      option.value = editor.id;
      return option;
    }),
  );
  $("restart-target").value = preferences.restartEditorTargets[0] || "";
  $("restart-target").onchange = () =>
    savePreference({
      restartEditorTargets: $("restart-target").value
        ? [$("restart-target").value]
        : [],
    });
}
$("provider-add").onclick = () => {
  const id = crypto.randomUUID();
  providerDrafts.set(id, {
    id,
    providerID: "",
    username: "",
    password: "",
    proxyURL: "",
    adminBaseURL: "",
    importedAccountIDs: [],
    cachedAccounts: [],
    accountProxyURLs: {},
    allowInsecureTLS: false,
  });
  renderProviders();
};
async function signIn(workspace = null) {
  if (busy) return;
  await action(async () => {
    try {
      const count = await accountsController.login(workspace, () =>
        $("login-dialog").showModal(),
      );
      if (count !== false)
        notice(extra(`Imported ${count} accounts`, `已导入 ${count} 个账号`));
    } finally {
      $("login-dialog").close();
    }
  });
}
$("add-account").onclick = () => signIn();
$("login-cancel").onclick = () => accountsController.cancelLogin();
$("login-dialog").addEventListener("cancel", () =>
  accountsController.cancelLogin(),
);
$("import-file-open").onclick = () => {
  closeMenus();
  action(
    () => accountsController.importFile(),
    (count) => extra(`Imported ${count} accounts`, `已导入 ${count} 个账号`),
  );
};
const remoteDrafts = new Map();
function remoteField(form, server, key, label, type = "text") {
  const row = el("label", "settings-row provider-field");
  row.append(el("span", "", tr(label)));
  const input = el("input");
  input.type = type;
  input.value = server[key] ?? "";
  input.setAttribute("aria-label", tr(label));
  if (type === "number") {
    input.min = "1";
    input.max = "65535";
  }
  input.oninput = () =>
    (server[key] = type === "number" ? Number(input.value) : input.value);
  row.append(input);
  form.append(row);
}
const expandedRemotes = new Set();
const remoteTitles = {
  save: "common.save",
  removeLocal: "proxy.remote.action.remove_local",
  discover: "proxy.remote.action.discover",
  deploy: "common.deploy",
  syncAccounts: "proxy.remote.action.sync_accounts",
  refresh: "common.refresh",
  start: "common.start",
  stop: "common.stop",
  logs: "common.logs",
  uninstall: "proxy.remote.action.uninstall_remote",
};
const remoteOperations = { syncAccounts: "sync", refresh: "status" };
function renderRemotes() {
  $("remote-list").replaceChildren();
  $("remote-empty").hidden = remoteDrafts.size > 0;
  for (const [id, server] of remoteDrafts) {
    const form = el("form", "section-card remote-form");
    form.dataset.serverId = id;
    const header = button("", null, "plain remote-card-header", () => {
      if (expandedRemotes.has(id)) expandedRemotes.delete(id);
      else expandedRemotes.add(id);
      renderRemotes();
    });
    header.type = "button";
    const text = el("div", "remote-heading");
    text.append(el("h3", "", server.label || "new-server"));
    if (!expandedRemotes.has(id))
      text.append(
        el(
          "span",
          "caption secondary",
          `${server.sshUser}@${server.host}:${server.sshPort}`,
        ),
      );
    header.append(
      icon("server"),
      text,
      icon(expandedRemotes.has(id) ? "chevron-up" : "chevron-down"),
    );
    form.append(header);
    if (expandedRemotes.has(id)) {
      const grid = el("div", "remote-fields");
      for (const [key, label, type] of [
        ["label", "name"],
        ["host", "host"],
        ["sshPort", "ssh_port", "number"],
        ["sshUser", "ssh_user"],
        ["remoteDir", "deploy_dir"],
        ["listenPort", "proxy_port", "number"],
      ])
        remoteField(grid, server, key, `proxy.remote.field.${label}`, type);
      form.append(grid);
      const auth = el("label", "settings-row remote-auth");
      auth.append(
        el("span", "caption secondary", tr("proxy.remote.auth.title")),
      );
      const select = el("select");
      for (const [value, label] of [
        ["keyPath", "path"],
        ["keyContent", "private_key"],
        ["password", "password"],
      ]) {
        const option = el("option", "", tr(`proxy.remote.auth.${label}`));
        option.value = value;
        select.append(option);
      }
      select.value = server.authMode;
      select.onchange = () => {
        server.authMode = select.value;
        renderRemotes();
      };
      auth.append(select);
      form.append(auth);
      if (server.authMode === "keyContent") {
        const area = el("textarea", "remote-private-key");
        area.value = server.privateKey || "";
        area.setAttribute("aria-label", tr("proxy.remote.auth.private_key"));
        area.oninput = () => (server.privateKey = area.value);
        form.append(area);
      } else {
        const row = el("div", "remote-identity");
        remoteField(
          row,
          server,
          server.authMode === "password" ? "password" : "identityFile",
          `proxy.remote.auth.${server.authMode === "password" ? "password" : "path"}`,
          server.authMode === "password" ? "password" : "text",
        );
        if (server.authMode !== "password") {
          const choose = button("", "folder", "capsule icon-only", async () => {
            try {
              const path = await proxyController.chooseIdentityFile();
              if (path) {
                server.identityFile = path;
                renderRemotes();
              }
            } catch (error) {
              notice(error, true);
            }
          });
          choose.type = "button";
          choose.title = tr("proxy.remote.auth.choose_key_file");
          choose.setAttribute("aria-label", choose.title);
          row.append(choose);
        }
        form.append(row);
      }
      form.append(
        el("div", "remote-feedback"),
        el("div", "remote-actions"),
        el("div", "remote-output"),
      );
      const guide = el("details", "remote-action-guide");
      guide.append(
        el("summary", "caption secondary", tr("proxy.remote.help.title")),
      );
      for (const help of P.remoteHelp(proxyController.remoteButtons(id)))
        guide.append(
          el(
            "p",
            "caption",
            `${tr(remoteTitles[help.action])}: ${tr(help.key)}`,
          ),
        );
      form.append(guide);
    }
    form.onsubmit = (event) => {
      event.preventDefault();
      saveRemoteDraft(id);
    };
    $("remote-list").append(form);
  }
  renderRemoteStates();
}
async function saveRemoteDraft(id) {
  await action(async () => {
    const saved = await proxyController.saveRemote(remoteDrafts.get(id));
    remoteDrafts.set(id, saved);
    renderRemotes();
  }, tr("settings.notice.remote_servers_saved"));
}
function renderRemoteStates() {
  for (const form of document.querySelectorAll(
    ".remote-form[data-server-id]",
  )) {
    const id = form.dataset.serverId,
      server = remoteDrafts.get(id),
      actions = form.querySelector(".remote-actions");
    if (!actions) continue;
    const descriptors = proxyController.remoteButtons(id);
    const key = JSON.stringify(descriptors);
    if (actions.dataset.renderKey !== key) {
      actions.dataset.renderKey = key;
      actions.replaceChildren();
      for (const descriptor of descriptors) {
        const control = button(
          tr(remoteTitles[descriptor.intent]),
          descriptor.showsProgress ? "refresh" : null,
          `capsule ${descriptor.role === "destructive" ? "destructive" : descriptor.surfaceStyle === "prominent" ? "prominent" : ""}`,
          async () => {
            if (descriptor.intent === "save") {
              await saveRemoteDraft(id);
              return;
            }
            if (descriptor.intent === "removeLocal") {
              await action(async () => {
                await invoke("remove_remote", { id });
                remoteDrafts.delete(id);
                expandedRemotes.delete(id);
                renderRemotes();
              });
              return;
            }
            await proxyController.remoteAction(
              id,
              remoteOperations[descriptor.intent] || descriptor.intent,
              server.label,
            );
          },
        );
        control.type = "button";
        control.disabled = !descriptor.enabled;
        control.dataset.available = String(descriptor.enabled);
        if (descriptor.showsProgress) control.classList.add("spinning");
        control.title = tr(P.remoteHelp([descriptor])[0].key);
        actions.append(control);
      }
    }
    const feedback = form.querySelector(".remote-feedback");
    feedback.replaceChildren();
    const f = proxyController.feedbacks[id];
    if (f) {
      feedback.className = `remote-feedback value-card ${f.state === "failure" ? "error" : ""}`;
      feedback.append(
        el("span", "caption", f.message || tr(f.key, ...(f.args || []))),
      );
      const close = button("", "close", "plain icon-only", () =>
        proxyController.dismissFeedback(id),
      );
      close.type = "button";
      close.setAttribute("aria-label", tr("common.close"));
      feedback.append(close);
    } else feedback.className = "remote-feedback";
    const output = form.querySelector(".remote-output");
    const outputKey = JSON.stringify([
      proxyController.remoteStatuses[id],
      proxyController.remoteLogs[id],
      proxyController.remoteDiscoveries[id],
    ]);
    if (output.dataset.renderKey === outputKey) continue;
    output.dataset.renderKey = outputKey;
    output.replaceChildren();
    const status = proxyController.remoteStatuses[id];
    if (status) {
      const metrics = el("div", "remote-metrics");
      for (const metric of P.remoteMetrics(status)) {
        const card = el("div", "value-card");
        card.append(
          el(
            "span",
            "caption secondary",
            {
              installed: "Installed",
              serviceInstalled: "Systemd",
              enabled: "Enabled",
              running: "Running",
              pid: "PID",
            }[metric.id],
          ),
          el("span", "", metric.value),
        );
        metrics.append(card);
      }
      output.append(metrics);
      for (const [label, value, secret] of [
        ["proxy.detail.base_url", status.baseURL, false],
        ["proxy.detail.api_key", status.apiKey, true],
      ]) {
        if (!value) continue;
        const row = el("div", "copy-row");
        const field = el("input");
        field.readOnly = true;
        field.type = secret ? "password" : "text";
        field.value = value;
        field.setAttribute("aria-label", tr(label));
        const copy = button("", "copy", "plain icon-only", () =>
          navigator.clipboard.writeText(value),
        );
        copy.type = "button";
        copy.setAttribute("aria-label", tr("common.copy"));
        row.append(field, copy);
        output.append(row);
      }
      if (status.lastError)
        output.append(el("p", "caption error", status.lastError));
    }
    for (const found of proxyController.remoteDiscoveries[id] || []) {
      const card = el("div", "value-card");
      card.append(
        el(
          "p",
          "caption",
          `${found.label || found.service_name} · ${found.remote_dir}:${found.listen_port}`,
        ),
      );
      const adopt = button(
        tr("proxy.remote.discovery.use_instance"),
        null,
        "capsule",
        () =>
          action(async () => {
            const next = {
              ...server,
              id: found.server_id || id,
              remoteDir: found.remote_dir,
              listenPort: Number(found.listen_port),
              label: found.label || server.label,
            };
            const saved = await proxyController.adopt(next, id);
            remoteDrafts.delete(id);
            remoteDrafts.set(saved.id, saved);
            expandedRemotes.add(saved.id);
            renderRemotes();
          }),
      );
      adopt.type = "button";
      card.append(adopt);
      output.append(card);
    }
    if (proxyController.remoteLogs[id])
      output.append(el("pre", "remote-log", proxyController.remoteLogs[id]));
  }
}
$("remote-add").onclick = () =>
  action(async () => {
    const id = crypto.randomUUID();
    const saved = await proxyController.saveRemote({
      id,
      label: "new-server",
      host: "",
      sshPort: 22,
      sshUser: "root",
      authMode: "keyPath",
      remoteDir: "/opt/codex-tools",
      listenPort: 8787,
    });
    remoteDrafts.set(id, saved);
    expandedRemotes.add(id);
    renderRemotes();
  });
const tunnelConfiguration = proxyController.tunnelConfiguration;
function renderTunnelControls() {
  const root = $("tunnel-controls");
  root.replaceChildren();
  const row = el("label", "settings-row");
  row.append(el("span", "", tr("proxy.section.public_access")));
  const mode = el("select");
  for (const value of ["quick", "named"]) {
    const option = el("option", "", tr(`proxy.public.mode.${value}_kicker`));
    option.value = value;
    mode.append(option);
  }
  mode.value = tunnelConfiguration.tunnelMode;
  mode.onchange = () => {
    tunnelConfiguration.tunnelMode = mode.value;
    saveTunnelConfiguration();
    renderTunnelControls();
  };
  row.append(mode);
  root.append(row);
  const http = el("label", "settings-row");
  http.append(el("span", "", "HTTP/2"));
  const toggle = el("input", "switch");
  toggle.type = "checkbox";
  toggle.checked = tunnelConfiguration.useHTTP2;
  toggle.onchange = () => {
    tunnelConfiguration.useHTTP2 = toggle.checked;
    saveTunnelConfiguration();
  };
  http.append(toggle);
  root.append(http);
  if (tunnelConfiguration.tunnelMode === "named")
    for (const [key, label, type] of [
      ["apiToken", "api_token", "password"],
      ["accountId", "account_id"],
      ["zoneId", "zone_id"],
      ["namedHostname", "hostname"],
    ])
      remoteField(
        root,
        tunnelConfiguration,
        key,
        `proxy.public.field.${label}`,
        type,
      );
  else
    root.append(
      el("p", "caption secondary", tr("proxy.public.quick_note_message")),
    );
  const actions = el("div", "settings-row provider-actions");
  for (const [op, label] of [
    ["start", "start_action"],
    ["stop", "stop_action"],
    ["status", "refresh_status"],
  ]) {
    const control = button(tr(`proxy.public.${label}`), null, "capsule", () =>
      runTunnel(op),
    );
    control.dataset.tunnelAction = op;
    actions.append(control);
  }
  root.append(actions, el("p", "caption", ""));
  root.lastChild.id = "tunnel-status";
  renderTunnelState();
}
function renderTunnelState() {
  const status = proxyController.tunnelStatus;
  $("public-enabled").checked = tunnelConfiguration.enabled;
  const text = $("tunnel-status");
  if (text)
    text.textContent =
      status.publicUrl ||
      status.publicURL ||
      status.lastError ||
      tr(status.running ? "proxy.status.running" : "proxy.status.stopped");
  $("tunnel-install").closest(".value-card").hidden = status.installed;
  for (const node of document.querySelectorAll("[data-tunnel-action]")) {
    const intent = node.dataset.tunnelAction;
    const descriptor = proxyController.publicButtons.find(
      (b) => (b.intent === "refreshStatus" ? "status" : b.intent) === intent,
    );
    node.hidden = !descriptor;
    node.disabled = !descriptor?.enabled;
  }
}
async function runTunnel(operation) {
  await action(() => proxyController.tunnelAction(operation));
}
$("public-enabled").onchange = () => {
  tunnelConfiguration.enabled = $("public-enabled").checked;
  saveTunnelConfiguration();
};
$("tunnel-install").onclick = () => runTunnel("install");
const initializeBaseFeatures = initializeFeatures;
initializeFeatures = async () => {
  await initializeBaseFeatures();
  for (const server of configurationCache.remoteServers) {
    remoteDrafts.set(server.id, server);
    if (!server.host) expandedRemotes.add(server.id);
  }
  renderRemotes();
  Object.assign(
    tunnelConfiguration,
    configurationCache.proxyConfiguration.cloudflared,
  );
  renderTunnelControls();
};
function renderPendingWorkspaces() {
  const entries = accountsController.directory;
  const root = $("pending-workspaces");
  root.replaceChildren();
  const presentation = accountsController.presentation(collapsed);
  const visible = presentation.pending;
  if (!presentation.showPending) return;
  if (presentation.pendingError)
    root.append(el("p", "caption error", presentation.pendingError));
  root.append(el("p", "caption error", accountsController.pendingError));
  root.append(
    el("h3", "", tr("accounts.pending.title")),
    el("p", "caption secondary", tr("accounts.pending.subtitle")),
  );
  for (const workspace of visible) {
    const card = el("div", "value-card pending-workspace");
    card.append(el("h3", "", workspace.workspaceName || workspace.workspaceId));
    const actions = el("div", "provider-actions");
    actions.append(
      button(
        tr("accounts.action.add_account"),
        "plus",
        "capsule prominent",
        () => signIn(workspace.workspaceId),
      ),
    );
    actions.append(
      button(tr("common.remove"), "trash", "capsule destructive", () =>
        action(
          () => accountsController.dismissPending(workspace.id),
          null,
          false,
        ),
      ),
    );
    card.append(actions);
    root.append(card);
  }
}
async function saveTunnelConfiguration() {
  await action(() => proxyController.updateTunnel(tunnelConfiguration));
}

async function refreshFeatureState() {
  const [settings, provider, activity, tunnel] = await Promise.all([
    invoke("configuration"),
    invoke("current_provider"),
    invoke("refresh_activity"),
    invoke("tunnel_action", { action: "status", value: null }),
  ]);
  accountsController.syncActivity(activity);
  proxyController.tunnelStatus = tunnel;
  proxyController.remoteServers = settings.remoteServers;
  proxyController.applyLocal(
    proxyController.proxyStatus,
    settings.proxyConfiguration,
    tunnel,
  );
  configurationCache = settings;
  providerController.accept(settings, provider.id);
}
window.__TAURI__.event
  .listen("remote-sync-error", ({ payload }) => notice(payload, true))
  .catch((error) => notice(error, true));
