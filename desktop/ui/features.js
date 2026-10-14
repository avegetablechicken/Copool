"use strict";
let configurationCache = null;
let installedEditors = [];
const providerController = new window.CopoolControllers.Settings(invoke);
const providerDrafts = providerController.providers;
/// One controller per account pool; only the shown pool renders.
function poolController(target, onMutation) {
  const controller = new window.CopoolControllers.Accounts(
    poolInvoke(target),
    null,
    () => {
      if (controller !== accountsController) return;
      accountsCache = accountsController.accounts;
      renderAccounts(accountsCache);
      renderPendingWorkspaces();
      syncButtons();
    },
    onMutation,
  );
  return controller;
}
const poolControllers = {
  codex: poolController("codex", () =>
    invoke("sync_remote_accounts").catch((error) => notice(error, true)),
  ),
  // Remote servers only serve the Codex pool.
  claude: poolController("claude", () => {}),
};
let accountsController = poolControllers[pool];
let lastProxyNotice;
const proxyController = new window.CopoolControllers.Proxy(invoke, () => {
  proxyCache = proxyController.proxyStatus;
  renderProxy();
  renderRemoteStates();
  syncButtons();
  if (proxyController.notice && proxyController.notice !== lastProxyNotice) {
    lastProxyNotice = proxyController.notice;
    const n = lastProxyNotice;
    notice(n.message || tr(n.key, ...(n.args || [])), n.style === "error");
  }
});
function providerField(
  form,
  provider,
  key,
  label,
  type = "text",
  saved = providerController.persisted.some((p) => p.id === provider.id),
  keepLabel = "settings.sub2api.password_keep",
) {
  const row = el("label", "row provider-field");
  row.append(el("span", "", tr(label)));
  const input = el("input");
  input.type = type;
  input.value = provider[key] || "";
  input.autocomplete = type === "password" ? "new-password" : "off";
  input.setAttribute("aria-label", tr(label));
  input.oninput = () => (provider[key] = input.value);
  // Only a saved provider has a password to keep, as in the SwiftUI form.
  input.placeholder = tr(
    type === "password" && saved ? keepLabel : `${label}_placeholder`,
  );
  row.append(input);
  form.append(row);
}
const expandedProviders = new Set();
function renderProviders() {
  const root = $("provider-forms");
  root.replaceChildren();
  renderProviderCount();
  for (const [id, provider] of providerDrafts) {
    const form = el("form", "provider-form");
    const header = el("div", "row provider-header");
    const toggle = button("", null, "provider-toggle", () => {
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
      brandIcon("codex"),
      el("span", "", provider.providerID || "Sub2api"),
    );
    const remove = button("", "trash", "icon-btn", () => {
      const saved = providerController.persisted.some((p) => p.id === id);
      action(
        async () => {
          await providerController.remove(id);
          expandedProviders.delete(id);
          renderProviders();
        },
        saved ? tr("settings.notice.sub2api_saved") : undefined,
      );
    });
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
      }
      const row = el("label", "row");
      row.append(
        el("span", "row-label", tr("settings.sub2api.allow_insecure_tls")),
      );
      const check = el("input", "switch");
      check.type = "checkbox";
      check.checked = !!provider.allowInsecureTLS;
      check.onchange = () => (provider.allowInsecureTLS = check.checked);
      row.append(check);
      body.append(row);
      const actions = el("div", "provider-save");
      const save = button(tr("common.save"), null, "btn primary");
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
  // The pool can change while the import runs; release the controller that started it.
  const controller = accountsController;
  if (controller.importing) return;
  controller.importing = true;
  syncButtons();
  await droppingNewCards($("import-open"), () => action(async () => {
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
  }));
  controller.importing = false;
  syncButtons();
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
/// The pool's colored icon from the header, for provider rows and menus.
/// Each copy keeps its own gradient: one inside the header, which is hidden
/// on subpages, would not paint.
let brandIcons = 0;
function brandIcon(target) {
  const icon = document
    .querySelector(`.pool-btn[data-pool="${target}"] svg`)
    .cloneNode(true);
  for (const gradient of icon.querySelectorAll("[id]")) {
    const id = `${gradient.id}-${++brandIcons}`;
    for (const node of icon.querySelectorAll(`[fill="url(#${gradient.id})"]`))
      node.setAttribute("fill", `url(#${id})`);
    gradient.id = id;
  }
  icon.classList.add("provider-brand");
  return icon;
}
/// Both pools' providers share one count, as they share one block.
function renderProviderCount() {
  $("provider-count").textContent = String(
    providerDrafts.size +
      (typeof claudeProviderDrafts === "undefined" ? 0 : claudeProviderDrafts.size),
  );
}
/// One button adds a provider to either pool.
$("provider-add").onclick = () => {
  if (openMenu?.trigger === $("provider-add")) {
    closeMenu(true);
    return;
  }
  showMenu($("provider-add"), [
    { label: tr("pool.codex"), icon: brandIcon("codex"), run: addCodexProvider },
    { label: tr("pool.claude"), icon: brandIcon("claude"), run: addClaudeProvider },
  ]);
};
function addCodexProvider() {
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
}
/// Adds an account, signs one in again, or authorizes a pending workspace,
/// waiting for the browser sign-in in a dialog that can cancel it.
function signIn(workspace = null, kind = "add", pending = null) {
  // A pending workspace's account comes out of the pending list.
  const origin = pending ? $("pending-workspaces") : $("add-account");
  return droppingNewCards(origin, () => signInOnce(workspace, kind, pending));
}
async function signInOnce(workspace, kind, pending) {
  try {
    const labels = await accountsController.login(
      workspace,
      () => $("login-dialog").showModal(),
      pending?.id,
    );
    if (labels === false) return;
    notice(
      pending
        ? tr("accounts.notice.workspace_authorized_format", pending.workspaceName)
        : tr(
            kind === "reauthenticate"
              ? "accounts.notice.imported_format"
              : "accounts.notice.imported_new_format",
            labels.join(", "),
          ),
    );
  } catch (error) {
    notice(error, true);
  } finally {
    $("login-dialog").close();
    await refreshView().catch((error) => notice(error, true));
  }
}
$("login-cancel").onclick = () => cancelSignIn();
$("login-dialog").addEventListener("cancel", () => cancelSignIn());
function cancelSignIn() {
  accountsController.cancelLogin().catch(() => {});
  notice(tr("error.oauth.request_cancelled"), true);
}
$("add-account").onclick = () =>
  accountsController.adding ? cancelSignIn() : signIn();
/// Imports through Swift's isImporting state; a cancelled picker is silent.
function importAccounts(task, key) {
  return droppingNewCards($("import-open"), () => importAccountsOnce(task, key));
}
async function importAccountsOnce(task, key) {
  try {
    const labels = await task();
    if (labels?.length) notice(tr(key, labels.join(", ")));
  } catch (error) {
    notice(error, true);
  } finally {
    await refreshView().catch((error) => notice(error, true));
  }
}
function importAccountFile() {
  importAccounts(
    () => accountsController.importFile(),
    "accounts.notice.imported_new_format",
  );
}
const remoteDrafts = new Map();
function remoteField(
  form,
  server,
  key,
  label,
  type = "text",
  onEdit = null,
  disabled = false,
) {
  const row = el("label", "row provider-field");
  row.append(el("span", "", tr(label)));
  const input = el("input");
  input.type = type;
  input.value = server[key] ?? "";
  input.setAttribute("aria-label", tr(label));
  if (type === "number") {
    input.min = "1";
    input.max = "65535";
  }
  input.disabled = disabled;
  input.oninput = () => {
    server[key] = type === "number" ? Number(input.value) : input.value;
    onEdit?.();
  };
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
    const form = el("form", "remote-form");
    form.dataset.serverId = id;
    const header = button("", null, "remote-card-header", () => {
      if (expandedRemotes.has(id)) expandedRemotes.delete(id);
      else expandedRemotes.add(id);
      renderRemotes();
    });
    header.type = "button";
    const heading = P.remoteHeader({
      ...server,
      expanded: expandedRemotes.has(id),
    });
    const text = el("div", "remote-heading");
    text.append(el("h3", "", heading.title));
    if (heading.subtitle) text.append(el("span", "", heading.subtitle));
    // The status pill, collapsed or not, as RemoteServerCardPrimitives.
    const pill = el("span", "state remote-state");
    pill.append(el("span", "dot"), el("span"));
    header.append(
      icon("server"),
      text,
      pill,
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
      const auth = el("label", "row remote-auth");
      auth.append(el("span", "row-label", tr("proxy.remote.auth.title")));
      const select = el("select", "select");
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
          const choose = button("", "folder", "icon-btn", async () => {
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
        el("summary", "", tr("proxy.remote.help.title")),
      );
      for (const help of P.remoteHelp(proxyController.remoteButtons(id)))
        guide.append(
          el("p", "", `${tr(remoteTitles[help.action])}: ${tr(help.key)}`),
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
  const saved = await proxyController.saveRemoteCard(remoteDrafts.get(id));
  if (!saved) return;
  remoteDrafts.set(id, saved);
  renderRemotes();
}
function renderRemoteStates() {
  for (const form of document.querySelectorAll(
    ".remote-form[data-server-id]",
  )) {
    const id = form.dataset.serverId,
      server = remoteDrafts.get(id),
      actions = form.querySelector(".remote-actions");
    const status = proxyController.remoteStatuses[id];
    const pill = form.querySelector(".remote-state");
    pill.classList.toggle("running", !!status?.running);
    // RemoteServerConfiguration.statusLabel, "Unknown" included.
    pill.lastChild.textContent = !status
      ? "Unknown"
      : tr(status.running ? "proxy.status.running" : "proxy.status.stopped");
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
          descriptor.intent === "save"
            ? "btn primary"
            : `btn quiet ${descriptor.role === "destructive" ? "destructive" : descriptor.surfaceStyle === "prominent" ? "prominent" : ""}`,
          async () => {
            if (descriptor.intent === "save") {
              await saveRemoteDraft(id);
              return;
            }
            if (descriptor.intent === "removeLocal") {
              if (await proxyController.removeRemote(id)) {
                remoteDrafts.delete(id);
                expandedRemotes.delete(id);
                renderRemotes();
              }
              return;
            }
            await proxyController.remoteAction(
              id,
              remoteOperations[descriptor.intent] || descriptor.intent,
              server.label,
              descriptor.intent,
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
      feedback.className = `remote-feedback message ${f.state === "failure" ? "bad" : "good"}`;
      feedback.append(
        el("span", "message-dot"),
        el("span", "message-body", f.message || tr(f.key, ...(f.args || []))),
      );
      const close = button("", "close", "icon-btn", () =>
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
    const details = el("dl", "kv remote-details");
    P.remoteDetails(status).forEach((detail, index) => {
      const label = [
        tr("proxy.detail.base_url"),
        tr("proxy.detail.api_key"),
        extra("Service", "服务"),
      ][index];
      const value = el("dd", "copy-row");
      const field = el("input", "field");
      field.readOnly = true;
      field.type = index === 1 && detail.canCopy ? "password" : "text";
      field.value = detail.value;
      field.setAttribute("aria-label", label);
      value.append(field);
      if (detail.canCopy) value.append(copyButton(detail.value));
      details.append(el("dt", "", label), value);
    });
    output.append(details);
    if (status) {
      const metrics = el("div", "remote-metrics");
      for (const metric of P.remoteMetrics(status)) {
        const card = el("div", "remote-metric");
        card.append(
          el(
            "span",
            "",
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
      output.prepend(metrics);
      if (status.lastError)
        output.append(el("p", "error", status.lastError));
    }
    // RemoteServerDiscoverySection and its cards.
    const discoveries = proxyController.remoteDiscoveries[id] || [];
    if (discoveries.length)
      output.append(
        el("div", "subhead", tr("proxy.remote.discovery.title")),
      );
    for (const found of discoveries) {
      const card = el("div", "remote-discovery");
      const flag = (key) => found[key] === "1";
      const label = found.label?.trim() || found.service_name;
      const summary = el("div", "remote-discovery-summary");
      summary.append(el("span", "row-name", label));
      if (label !== found.service_name)
        summary.append(el("span", "secondary mono", found.service_name));
      summary.append(
        el(
          "span",
          "secondary",
          [
            flag("running") ? "running" : "stopped",
            flag("enabled") ? "enabled" : "disabled",
            flag("installed") ? "binary_present" : "binary_missing",
          ]
            .map((state) => tr(`proxy.remote.discovery.state.${state}`))
            .join(" · "),
        ),
      );
      const values = el("dl", "kv");
      for (const [key, value] of [
        ["remote_dir", found.remote_dir],
        ["listen_port", found.listen_port],
        ["base_url", `http://${server.host}:${found.listen_port}/v1`],
        [
          "api_key",
          tr(
            flag("api_key_present")
              ? "proxy.remote.discovery.api_key_present"
              : "proxy.remote.discovery.api_key_missing",
          ),
        ],
      ])
        values.append(
          el("dt", "", tr(`proxy.remote.discovery.${key}`)),
          el("dd", "", value),
        );
      card.append(summary);
      const adopt = button(
        tr("proxy.remote.discovery.use_instance"),
        null,
        "btn quiet prominent",
        async () => {
          const next = {
            ...server,
            id: found.server_id || id,
            remoteDir: found.remote_dir,
            listenPort: Number(found.listen_port),
            label: found.label || server.label,
          };
          const saved = await proxyController.saveRemoteCard(next, id);
          if (!saved) return;
          remoteDrafts.delete(id);
          remoteDrafts.set(saved.id, saved);
          expandedRemotes.add(saved.id);
          renderRemotes();
          await proxyController.remoteAction(saved.id, "status", saved.label, "refresh");
        },
      );
      adopt.disabled = !!proxyController.activeActions[id];
      adopt.type = "button";
      card.append(adopt, values);
      output.append(card);
    }
    const logs = P.remoteLogs(proxyController.remoteLogs[id]);
    const logView = el("details", "remote-logs");
    logView.open = logs.canCopy;
    const summary = el("summary", "", tr("proxy.remote.logs"));
    if (logs.canCopy) summary.append(copyButton(logs.content));
    logView.append(summary, el("pre", "remote-log", logs.content));
    output.append(logView);
  }
}
function copyButton(value) {
  const copy = button("", "copy", "icon-btn", (event) => {
    event.preventDefault();
    navigator.clipboard
      .writeText(value)
      .then(() => notice(extra("Copied", "已复制")))
      .catch((error) => notice(error, true));
  });
  copy.type = "button";
  copy.title = tr("common.copy");
  copy.setAttribute("aria-label", copy.title);
  return copy;
}
// Adding a server takes no lock, as Swift's addRemoteServer.
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
  const row = el("label", "row");
  row.append(el("span", "row-label", tr("proxy.section.public_access")));
  const mode = el("select", "select");
  const modes = P.publicModes(
    tunnelConfiguration.tunnelMode,
    !proxyController.tunnelStatus.running,
  );
  for (const choice of modes) {
    const option = el(
      "option",
      "",
      tr(`proxy.public.mode.${choice.mode}_kicker`),
    );
    option.value = choice.mode;
    option.selected = choice.selected;
    mode.append(option);
  }
  mode.disabled = !modes[0].isEnabled || !proxyController.canEditTunnel;
  mode.onchange = () => {
    tunnelConfiguration.tunnelMode = mode.value;
    saveTunnelConfiguration();
    renderTunnelControls();
  };
  row.append(mode);
  root.append(row);
  const http = el("label", "row");
  http.append(el("span", "row-label", "HTTP/2"));
  const toggle = el("input", "switch");
  toggle.type = "checkbox";
  toggle.checked = tunnelConfiguration.useHTTP2;
  toggle.disabled = !proxyController.canEditTunnel;
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
        // Edits enable Start; the hostname is saved as it is typed.
        () => {
          if (key === "namedHostname") scheduleTunnelSave();
          renderTunnelState();
        },
        !proxyController.canEditTunnel,
      );
  else
    root.append(
      el("p", "setting-description", tr("proxy.public.quick_note_message")),
    );
  const actions = el("div", "provider-actions");
  for (const [op, label] of [
    ["start", "start_action"],
    ["stop", "stop_action"],
    ["status", "refresh_status"],
  ]) {
    const control = button(tr(`proxy.public.${label}`), null, "btn quiet", () =>
      runTunnel(op),
    );
    control.dataset.tunnelAction = op;
    actions.append(control);
  }
  const statusList = el("dl", "kv");
  statusList.id = "tunnel-status";
  root.append(actions, statusList);
  renderTunnelState();
}
function renderTunnelState() {
  const status = proxyController.tunnelStatus;
  $("public-enabled").checked = tunnelConfiguration.enabled;
  const list = $("tunnel-status");
  if (list) {
    const labels = {
      status: "proxy.public.status_title",
      url: "proxy.public.url_title",
      "install-path": "proxy.public.install_path_title",
      "last-error": "proxy.detail.last_error",
    };
    list.replaceChildren();
    for (const card of P.publicStatus({
      ...status,
      publicURL: status.publicURL ?? status.publicUrl ?? undefined,
    })) {
      const value = el("dd", `truncate-${card.truncation}`);
      const text = tr(card.headline);
      value.append(el("span", "", text));
      value.title = text;
      if (card.copyValue) value.append(copyButton(card.copyValue));
      list.append(el("dt", "", tr(labels[card.id])), value);
    }
  }
  $("tunnel-install").closest(".value-card").hidden = status.installed;
  // Without cloudflared only the install card shows, as PublicAccessSection.
  $("tunnel-controls").hidden = !status.installed;
  // Open while the tunnel runs, as applyCloudflaredStatus.
  if (status.running) openDisclosure("public-details");
  const editable = proxyController.canEditTunnel;
  for (const control of $("tunnel-controls").querySelectorAll(
    "select, input",
  ))
    control.disabled =
      !editable ||
      (control.tagName === "SELECT" &&
        !P.publicModes(tunnelConfiguration.tunnelMode, !status.running)[0]
          .isEnabled);
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
  // Enabling opens the section, as setPublicAccessEnabled.
  if (tunnelConfiguration.enabled) openDisclosure("public-details");
  saveTunnelConfiguration();
};
$("tunnel-install").onclick = () => runTunnel("install");
const initializeBaseFeatures = initializeFeatures;
initializeFeatures = async () => {
  await initializeBaseFeatures();
  for (const server of configurationCache.remoteServers) {
    remoteDrafts.set(server.id, server);
    // A placeholder draft opens, as RemoteServerCardView.isPlaceholderDraft.
    if (!server.label?.trim() || server.label.trim() === "new-server")
      expandedRemotes.add(server.id);
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
  const block = el("section", "block pending");
  const head = el("div", "block-head");
  head.append(el("span", "block-title", tr("accounts.pending.title")));
  block.append(head);
  if (presentation.pendingError)
    block.append(el("p", "error", presentation.pendingError));
  if (accountsController.pendingError)
    block.append(el("p", "error", accountsController.pendingError));
  block.append(
    el("p", "setting-description", tr("accounts.pending.subtitle")),
  );
  for (const workspace of visible) {
    const authorizing = accountsController.authorizing === workspace.id;
    const row = el("div", "row pending-workspace");
    row.append(
      el(
        "span",
        "row-label row-name",
        workspace.workspaceName || workspace.workspaceId,
      ),
      // As the SwiftUI pending card: Authorize, or Cancel while authorizing.
      button(
        tr(authorizing ? "common.cancel" : "accounts.pending.action.authorize"),
        null,
        `btn quiet${authorizing ? "" : " prominent"}`,
        () =>
          authorizing
            ? cancelSignIn()
            : signIn(workspace.workspaceId, "authorize", workspace),
      ),
    );
    row.lastChild.disabled =
      !authorizing &&
      (accountsController.adding ||
        accountsController.importing ||
        accountsController.refreshing ||
        !!accountsController.authorizing);
    const remove = button("", "trash", "icon-btn destructive", () =>
      action(
        () => accountsController.dismissPending(workspace.id),
        null,
        false,
      ),
    );
    remove.title = tr("common.remove");
    remove.setAttribute("aria-label", remove.title);
    remove.disabled = authorizing;
    row.append(remove);
    block.append(row);
  }
  root.append(block);
}
async function saveTunnelConfiguration() {
  clearTimeout(tunnelSaveTimer);
  await action(
    () => proxyController.updateTunnel(tunnelConfiguration),
    null,
    false,
  );
}
let tunnelSaveTimer;
/// Swift's scheduleProxyConfigurationSync: a 350 ms debounce.
function scheduleTunnelSave() {
  clearTimeout(tunnelSaveTimer);
  tunnelSaveTimer = setTimeout(saveTunnelConfiguration, 350);
}

async function refreshFeatureState() {
  const [settings, provider, activity, tunnel] = await Promise.all([
    invoke("configuration"),
    invoke("current_provider"),
    invoke("refresh_activity"),
    invoke("tunnel_action", { action: "status", value: null }),
  ]);
  // Activity covers both pools; each pool waits only on its own accounts.
  for (const controller of Object.values(poolControllers))
    controller.syncActivity(
      activity.filter((id) => controller.accounts.some((a) => a.id === id)),
    );
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

// Sub2api providers of the Claude pool: their Claude accounts join the pool,
// and switching to one points Claude Code at the provider's gateway.
const claudeProviderDrafts = new Map();
let claudeProvidersSaved = [];
const expandedClaudeProviders = new Set();
async function loadClaudeProviders() {
  claudeProvidersSaved = await invoke("claude_sub2api_providers");
  for (const saved of claudeProvidersSaved)
    if (!claudeProviderDrafts.has(saved.id))
      claudeProviderDrafts.set(saved.id, { ...saved, password: "", apiKey: "" });
  renderClaudeProviders();
}
function renderClaudeProviders() {
  const root = $("claude-provider-forms");
  root.replaceChildren();
  renderProviderCount();
  for (const [id, provider] of claudeProviderDrafts) {
    const saved = claudeProvidersSaved.find((p) => p.id === id);
    const form = el("form", "provider-form");
    const header = el("div", "row provider-header");
    const toggle = button("", null, "provider-toggle", () => {
      if (expandedClaudeProviders.has(id)) expandedClaudeProviders.delete(id);
      else expandedClaudeProviders.add(id);
      renderClaudeProviders();
    });
    toggle.type = "button";
    toggle.setAttribute("aria-expanded", String(expandedClaudeProviders.has(id)));
    const chevron = icon("chevron-down");
    chevron.classList.add("provider-chevron");
    if (!expandedClaudeProviders.has(id)) chevron.classList.add("collapsed");
    toggle.append(
      chevron,
      brandIcon("claude"),
      el("span", "", provider.name || "Sub2api"),
    );
    const remove = button("", "trash", "icon-btn", () =>
      action(
        async () => {
          if (saved) await invoke("claude_remove_sub2api_provider", { id });
          claudeProviderDrafts.delete(id);
          expandedClaudeProviders.delete(id);
          await loadClaudeProviders();
        },
        saved ? tr("settings.notice.sub2api_saved") : undefined,
      ),
    );
    remove.type = "button";
    remove.title = tr("settings.sub2api.remove_provider");
    remove.setAttribute("aria-label", remove.title);
    header.append(toggle, remove);
    form.append(header);
    if (expandedClaudeProviders.has(id)) {
      const body = el("div", "provider-body");
      for (const [key, label, type] of [
        ["settings", "claude_settings"],
        ["adminBaseURL", "admin_base_url"],
        ["username", "username"],
        ["password", "password", "password"],
      ]) {
        providerField(
          body,
          provider,
          key,
          `settings.sub2api.${label}`,
          type,
          !!saved,
        );
        // Settings name a file to search for, or its path, typed or chosen
        // with a button inside the field.
        if (key === "settings") {
          const field = body.lastElementChild;
          const choose = button("", "folder", "icon-btn", async () => {
            try {
              const path = await invoke("pick_settings_file");
              if (path) {
                provider.settings = path;
                renderClaudeProviders();
              }
            } catch (error) {
              notice(error, true);
            }
          });
          choose.type = "button";
          choose.title = tr("settings.sub2api.choose_settings_file");
          choose.setAttribute("aria-label", choose.title);
          field.classList.add("with-button");
          field.append(choose);
        }
      }
      const row = el("label", "row");
      row.append(
        el("span", "row-label", tr("settings.sub2api.allow_insecure_tls")),
      );
      const check = el("input", "switch");
      check.type = "checkbox";
      check.checked = !!provider.allowInsecureTLS;
      check.onchange = () => (provider.allowInsecureTLS = check.checked);
      row.append(check);
      body.append(row);
      const actions = el("div", "provider-save");
      const save = button(tr("common.save"), null, "btn primary");
      save.type = "submit";
      actions.append(save);
      body.append(actions);
      form.append(body);
    }
    form.onsubmit = (event) => {
      event.preventDefault();
      action(async () => {
        const value = await invoke("claude_save_sub2api_provider", {
          value: structuredClone(provider),
        });
        claudeProviderDrafts.delete(id);
        claudeProviderDrafts.set(value.id, { ...value, password: "", apiKey: "" });
        await loadClaudeProviders();
      }, tr("settings.notice.sub2api_saved"));
    };
    root.append(form);
  }
}
function addClaudeProvider() {
  const id = crypto.randomUUID();
  claudeProviderDrafts.set(id, {
    id,
    name: "",
    settings: "",
    adminBaseURL: "",
    username: "",
    password: "",
    apiKey: "",
    allowInsecureTLS: false,
  });
  expandedClaudeProviders.add(id);
  renderClaudeProviders();
}
/// Imports the Claude accounts of every Claude Sub2api provider.
async function syncClaudeSub2api() {
  // The pool can change while the import runs; release the controller that started it.
  const controller = accountsController;
  if (controller.importing) return;
  controller.importing = true;
  syncButtons();
  await droppingNewCards($("import-open"), () => action(async () => {
    const count = await invoke("claude_sync_sub2api");
    // The cards' providers, as the sync left them.
    await loadClaudeProviders();
    notice(tr("accounts.notice.sub2api_imported_format", count));
  }));
  controller.importing = false;
  syncButtons();
}
const initializeClaudeFeatures = initializeFeatures;
initializeFeatures = async () => {
  await initializeClaudeFeatures();
  // A Claude store error must not keep the rest of the app from rendering.
  await loadClaudeProviders().catch((error) => notice(error, true));
};
