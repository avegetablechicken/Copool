"use strict";
if (navigator.userAgent.includes("Mac"))
  document.documentElement.classList.add("macos");
const $ = (id) => document.getElementById(id);
const invoke = (command, args = {}) =>
  window.__TAURI__.core.invoke(command, args);
const P = window.CopoolPresentation;
let busy = false;
let collapsed = false;
let selectedPage = "accounts";
let editingAccount = null;
let accountsCache = [];
let proxyCache = null;
let preferences = null;
let locale = "en";
let importingToken = false;
let noticeTimer;
let nativeIcons = {};
let iconFrame = 0;
const ICONS = {
  folder: '<path d="M2 5h7l2 3h9v12H2Z"/>',
  accounts:
    '<path d="M15 11c2.3 0 4 1.3 4.6 3.3.1.5-.2.7-.6.7h-8c-.4 0-.7-.2-.6-.7.6-2 2.3-3.3 4.6-3.3Z"/><circle cx="15" cy="5.8" r="2.8"/><path d="M6.5 11.5c-2.2.2-4.2 1.4-4.7 3.4-.1.5.1.8.6.8h5.1M7.4 3.3a2.6 2.6 0 1 0 0 5.1"/>',
  network:
    '<circle cx="11" cy="11" r="8.7"/><ellipse cx="11" cy="11" rx="4" ry="8.7" transform="rotate(-23 11 11)"/><path d="M3.4 6.8c3.5 3.7 8.8 6.5 16.1 7.2M4 16c4.9-.4 10-3 13.9-8.3"/>',
  settings:
    '<path d="m9 2-.7 2.4-1.6.9-2.4-.6-2 3.5 1.7 1.9v1.8l-1.7 1.9 2 3.5 2.4-.6 1.6.9L9 20h4l.7-2.4 1.6-.9 2.4.6 2-3.5-1.7-1.9v-1.8l1.7-1.9-2-3.5-2.4.6-1.6-.9L13 2Z"/><circle cx="11" cy="11" r="3.4"/>',
  import:
    '<path d="M11 1v12m-3-3 3 3 3-3M6 6H4a1.5 1.5 0 0 0-1.5 1.5v11A1.5 1.5 0 0 0 4 20h14a1.5 1.5 0 0 0 1.5-1.5v-11A1.5 1.5 0 0 0 18 6h-2"/>',
  plus: '<path d="M11 3v16M3 11h16"/>',
  wand: '<path d="m8 8 11 11-2 2L6 10ZM5 2v4M3 4h4M16 2v3M14.5 3.5h3M3 13v4M1 15h4M9 5l2-2M18 9l2-2"/>',
  refresh: '<path d="M18 8a8 8 0 1 0 .4 6M18 3v5h-5"/>',
  "chevron-up": '<path d="m5 14 6-6 6 6"/>',
  "chevron-down": '<path d="m5 8 6 6 6-6"/>',
  trash: '<path d="M3 5h16M8 5V3h6v2M5 5l1 15h10l1-15M9 8v9M13 8v9"/>',
  switch:
    '<circle cx="11" cy="11" r="9" fill="currentColor" stroke="none"/><path d="M7 8h8M7 8l3-3M7 8l3 3M15 14H7M15 14l-3-3M15 14l-3 3" stroke="var(--window)" stroke-width="1.5"/>',
  copy: '<rect x="7" y="6" width="12" height="14" rx="2"/><path d="M14 6V3H3v13h4"/>',
  eye: '<path d="M1 11s4-7 10-7 10 7 10 7-4 7-10 7S1 11 1 11Z"/><circle cx="11" cy="11" r="3"/>',
  close: '<path d="m5 5 12 12M5 17 17 5"/>',
  document: '<path d="M13 1H4v20h14V6ZM13 1v5h5M7 12h8M7 16h8"/>',
  server:
    '<rect x="2" y="3" width="18" height="6" rx="2"/><rect x="2" y="13" width="18" height="6" rx="2"/><path d="M5 6h1M5 16h1M10 6h7M10 16h7"/>',
  warning: '<path d="m11 2 10 18H1Z"/><path d="M11 8v5M11 16v1"/>',
};
function icon(name) {
  const node = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  node.setAttribute("viewBox", "0 0 22 22");
  node.setAttribute("class", "icon");
  node.setAttribute("aria-hidden", "true");
  node.dataset.glyph = name;
  if (!iconFrame)
    iconFrame = requestAnimationFrame(() => {
      iconFrame = 0;
      applyNativeIcons();
    });
  node.innerHTML = ICONS[name] || ICONS.network;
  return node;
}
function applyNativeIcons() {
  for (const node of document.querySelectorAll("svg[data-glyph]")) {
    const bounds = node.getBoundingClientRect();
    if (!bounds.width || !bounds.height) continue;
    const size = Math.round(Math.max(bounds.width, bounds.height));
    const variants = nativeIcons[`${node.dataset.glyph}:${size}`];
    if (!variants) continue;
    const symbol = document.createElement("span");
    symbol.className = `${node.getAttribute("class") || "icon"} native-symbol`;
    symbol.setAttribute("aria-hidden", "true");
    symbol.dataset.glyph = node.dataset.glyph;
    symbol.style.width = `${size}px`;
    symbol.style.height = `${size}px`;
    const imageSet = `image-set(${variants.map((url, index) => `url("${url}") ${index + 1}x`).join(",")})`;
    symbol.style.webkitMaskImage = imageSet;
    symbol.style.maskImage = imageSet;
    node.replaceWith(symbol);
    // Align the mask to physical pixels without a compositor transform.
    const placed = symbol.getBoundingClientRect();
    const scale = window.devicePixelRatio || 1;
    symbol.style.left = `${Math.round(placed.left * scale) / scale - placed.left}px`;
    symbol.style.top = `${Math.round(placed.top * scale) / scale - placed.top}px`;
  }
}
function mountIcons(root = document) {
  root
    .querySelectorAll("[data-icon]")
    .forEach((node) => node.replaceWith(icon(node.dataset.icon)));
}
function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}
function resolveLocale(value) {
  return P.resolveLocale(value);
}
function tr(key, ...args) {
  let value =
    window.COPOOL_LOCALES[locale]?.[key] ??
    window.COPOOL_LOCALES.en[key] ??
    key;
  return P.formatLocalized(value, args);
}
const extra = (en, zh) => (locale.startsWith("zh") ? zh : en);
function translate() {
  document.documentElement.lang = locale;
  document
    .querySelectorAll("[data-l10n]")
    .forEach((node) => (node.textContent = tr(node.dataset.l10n)));
  document.querySelectorAll("[data-title]").forEach((node) => {
    const label = tr(node.dataset.title);
    node.title = label;
    node.setAttribute("aria-label", label);
  });
  const languages = {
    en: "language.english",
    "zh-Hans": "language.simplified_chinese",
    "zh-Hant": "language.traditional_chinese",
    ja: "language.japanese",
    ko: "language.korean",
    fr: "language.french",
    de: "language.german",
    it: "language.italian",
    es: "language.spanish",
    ru: "language.russian",
    nl: "language.dutch",
  };
  $("language").replaceChildren(
    ...Object.entries(languages).map(([value, key]) => {
      const option = el("option", "", tr(key));
      option.value = value;
      return option;
    }),
  );
  $("language").value = locale;
  $("collapse-all").title = tr(
    collapsed ? "accounts.action.expand_all" : "accounts.action.collapse_all",
  );
  $("collapse-all").setAttribute("aria-label", $("collapse-all").title);
}
function notice(message, error = false) {
  clearTimeout(noticeTimer);
  $("notice-text").textContent = tr(
    message instanceof Error ? message.message : String(message),
  );
  $("notice").classList.toggle("error", error);
  $("notice").hidden = false;
  if (!error)
    noticeTimer = setTimeout(() => {
      $("notice").hidden = true;
    }, 4500);
}
function syncButtons() {
  const toolbar = P.actions({
    importing: busy,
    switching: busy,
    canRefresh: !busy && accountsCache.length > 0,
  });
  document.querySelectorAll("button").forEach((button) => {
    button.disabled =
      busy && !["login-cancel", "notice-close"].includes(button.id);
  });
  ["import-open", "add-account", "smart-switch", "refresh-all"].forEach(
    (id, index) => {
      $(id).disabled = !toolbar[index].enabled;
    },
  );
  $("proxy-start").disabled = busy || Boolean(proxyCache?.running);
  $("proxy-stop").disabled = busy || !proxyCache?.running;
  $("proxy-start").hidden = !!proxyCache?.running;
  $("proxy-stop").hidden = !proxyCache?.running;
  $("refresh-all").disabled = !accountsCache.length;
  $("refresh-all").classList.toggle("spinning", accountsController.spinner);
  $("smart-switch").disabled = busy || !accountsCache.length;
  document.querySelectorAll(".card-switch").forEach((button) => {
    button.disabled = busy || button.dataset.available === "false";
  });
  document
    .querySelectorAll(".remote-actions button")
    .forEach(
      (button) => (button.disabled = button.dataset.available === "false"),
    );
  renderTunnelState();
}
async function action(task, message, reload = true) {
  if (busy) return false;
  busy = true;
  syncButtons();
  try {
    const result = await task();
    if (message)
      notice(typeof message === "function" ? message(result) : message);
    return true;
  } catch (error) {
    notice(error, true);
    return false;
  } finally {
    busy = false;
    if (reload) await refreshView().catch((error) => notice(error, true));
    syncButtons();
  }
}
function button(label, glyph, className, handler) {
  const node = el("button", className);
  if (glyph) node.append(icon(glyph));
  if (label) node.append(el("span", "", label));
  if (handler) node.addEventListener("click", handler);
  return node;
}
function confirmAction(title, description) {
  $("confirm-title").textContent = title;
  $("confirm-description").textContent = description;
  const dialog = $("confirm-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  return new Promise((resolve) =>
    dialog.addEventListener(
      "close",
      () => resolve(dialog.returnValue === "confirm"),
      { once: true },
    ),
  );
}
function plan(account) {
  return P.normalizedPlan(account);
}
function percentage(window) {
  return P.displayPercent(window, preferences?.usageDisplay || "used");
}
function ring(window, label, tone) {
  const container = el("div", `ring ${tone}`);
  const value = percentage(window);
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 54 54");
  svg.setAttribute("aria-hidden", "true");
  for (const [name, radius] of [
    ["ring-track", 23],
    ["ring-fill", 23],
  ]) {
    const circle = document.createElementNS(svg.namespaceURI, "circle");
    circle.setAttribute("cx", "27");
    circle.setAttribute("cy", "27");
    circle.setAttribute("r", String(radius));
    circle.setAttribute("class", name);
    if (name === "ring-fill") {
      circle.setAttribute("pathLength", "100");
      circle.setAttribute("stroke-dasharray", `${value ?? 0} 100`);
      if (!value) circle.setAttribute("visibility", "hidden");
    }
    svg.append(circle);
  }
  const center = el("div", "ring-center");
  center.append(
    el("span", "ring-value", value === null ? "--" : `${Math.round(value)}%`),
    el("span", "ring-subtitle", label),
  );
  container.append(svg, center);
  container.setAttribute(
    "aria-label",
    `${label}: ${value === null ? "--" : Math.round(value) + "%"}`,
  );
  return container;
}
function usageWindow(window, title, tone) {
  const section = el("div", `usage-window ${tone}`);
  const used = window
    ? Math.round(Math.max(0, Math.min(100, window.usedPercent)))
    : 100;
  const remaining = 100 - used;
  const remainder = preferences?.usageDisplay === "remaining";
  const heading = el("div", "usage-heading");
  heading.append(
    el("span", "window-label", title),
    el(
      "span",
      "usage-primary",
      tr(
        remainder
          ? "accounts.window.remaining_format"
          : "accounts.window.used_format",
        `${remainder ? remaining : used}%`,
      ),
    ),
    el(
      "span",
      "usage-secondary",
      tr(
        remainder
          ? "accounts.window.used_format"
          : "accounts.window.remaining_format",
        `${remainder ? used : remaining}%`,
      ),
    ),
  );
  const track = el("div", "liquid-progress");
  track.setAttribute("role", "progressbar");
  track.setAttribute("aria-label", title);
  track.setAttribute("aria-valuenow", String(percentage(window) ?? 0));
  track.setAttribute("aria-valuemin", "0");
  track.setAttribute("aria-valuemax", "100");
  const fill = el("div", "liquid-fill");
  const displayed = P.displayPercent(
    window,
    preferences?.usageDisplay || "used",
    preferences?.usageDisplay === "remaining" ? 0 : 100,
  );
  fill.style.width = `${displayed}%`;
  fill.style.minWidth = displayed > 0 ? "8px" : "0";
  track.append(fill);
  const reset = P.formatReset(window?.resetAt, locale);
  section.append(
    heading,
    track,
    el("div", "reset-label", tr("accounts.window.reset_at_format", reset)),
  );
  return section;
}
function accountMenu(event, account) {
  event.preventDefault();
  invoke("account_menu", {
    id: account.id,
    title: tr("accounts.proxy.title"),
    x: event.clientX,
    y: event.clientY,
  }).catch((error) => notice(error, true));
}
window.__TAURI__.event
  .listen("account-proxy-menu", ({ payload }) => {
    const account = accountsCache.find((a) => a.id === payload);
    if (account) openAccountProxy(account);
  })
  .catch((error) => notice(error, true));
let renderedAccountsState;
function renderAccounts(accounts) {
  const renderState = [accounts, collapsed, locale, preferences?.usageDisplay];
  if (P.cardsEqual(renderState, renderedAccountsState)) return;
  renderedAccountsState = structuredClone(renderState);
  accounts = P.visibleAccounts(accounts);
  const grid = $("accounts");
  grid.classList.toggle("compact", collapsed);
  grid.style.gridTemplateColumns = `repeat(${P.grid({ collapsed }).count}, minmax(0, 1fr))`;
  grid.replaceChildren();
  if (!accounts.length) {
    const empty = el("div", "empty-state");
    empty.append(
      icon("accounts"),
      el("h3", "", tr("accounts.empty.title")),
      el("p", "", tr("accounts.empty.message.no_accounts")),
    );
    grid.append(empty);
    return;
  }
  for (const account of accounts) {
    const card = el(
      "article",
      `account-card${account.isCurrent ? " current" : ""}`,
    );
    card.dataset.plan = plan(account);
    card.addEventListener("contextmenu", (event) =>
      accountMenu(event, account),
    );
    const heading = el("div", "card-header");
    const tags = el("div", "account-tags");
    tags.append(el("span", "tag", plan(account)));
    if (P.workspaceTag(account))
      tags.append(el("span", "tag", P.workspaceTag(account)));
    if (account.isCurrent)
      tags.append(el("span", "tag", tr("accounts.card.current")));
    if (!account.available)
      tags.append(el("span", "tag", tr("accounts.card.status.deactivated")));
    heading.append(tags);
    if (!collapsed) {
      const remove = button(
        "",
        "trash",
        "capsule destructive icon-only",
        async () => {
          if (
            await confirmAction(
              tr("common.remove"),
              extra(
                `Remove ${account.label} from the account pool?`,
                `从账号池移除 ${account.label}？`,
              ),
            )
          )
            await action(() => accountsController.removeAccount(account.id));
        },
      );
      remove.title = tr("common.remove");
      remove.setAttribute(
        "aria-label",
        `${tr("common.remove")} ${account.label}`,
      );
      const proxyButton = button(
        "",
        "network",
        "plain icon-only account-proxy-button",
        () => openAccountProxy(account),
      );
      proxyButton.title = tr("accounts.proxy.title");
      proxyButton.setAttribute(
        "aria-label",
        `${tr("accounts.proxy.title")} ${account.email || account.label}`,
      );
      const headerActions = el("div", "card-header-actions");
      headerActions.append(proxyButton, remove);
      heading.append(headerActions);
    }
    const full = account.email || account.accountId;
    const name = collapsed ? full.split("@")[0] : full;
    const title = el("h3", "card-name", name);
    title.title = full;
    card.append(heading, title);
    const switchAccount = () =>
      action(
        () => invoke("switch_account", { id: account.id }),
        extra(`Switched to ${full}`, `已切换到 ${full}`),
      );
    if (collapsed) {
      const rings = el("div", "rings");
      rings.append(
        ring(account.usage?.fiveHour, "5h", "five-hour"),
        ring(account.usage?.oneWeek, "1w", "one-week"),
      );
      card.append(rings);
      if (!account.isCurrent) {
        card.tabIndex = 0;
        const overlay = el("div", "compact-switch-overlay");
        const control = button(
          tr("accounts.card.switch_to_this"),
          "switch",
          "capsule mint",
          switchAccount,
        );
        control.disabled = !account.available;
        overlay.append(control);
        card.append(overlay);
      }
    } else {
      card.append(
        usageWindow(
          account.usage?.fiveHour,
          tr("accounts.window.five_hour"),
          "five-hour",
        ),
        usageWindow(
          account.usage?.oneWeek,
          tr("accounts.window.one_week"),
          "one-week",
        ),
      );
      const credits = account.usage?.credits;
      let balance = "--";
      if (credits?.unlimited) balance = tr("accounts.card.unlimited");
      else if (credits?.balance !== null && credits?.balance !== undefined)
        balance = Number.isFinite(Number(credits.balance))
          ? String(Math.round(Number(credits.balance)))
          : credits.balance;
      const footer = el(
        "div",
        "card-footer",
        `${tr("accounts.card.credits_format", balance)} · ${tr(account.usage?.remainingResetCount != null ? "accounts.card.remaining_resets_format" : "accounts.card.resets_unavailable_format", account.usage?.remainingResetCount ?? "--")}`,
      );
      footer.title = footer.textContent;
      card.append(footer);
      const control = button(
        "",
        account.isCurrent ? "refresh" : "switch",
        `capsule mint card-switch`,
        P.needsReauthentication(account, tr("error.accounts.sign_in_expired"))
          ? () => signIn(account.accountId)
          : account.isCurrent
            ? () => action(() => accountsController.refresh(account.id))
            : switchAccount,
      );
      if (
        P.needsReauthentication(account, tr("error.accounts.sign_in_expired"))
      ) {
        control.replaceChildren(icon("plus"));
        control.dataset.available = "true";
      } else control.dataset.available = String(account.available);
      control.title = tr(
        account.isCurrent
          ? "common.refresh_usage"
          : "accounts.card.switch_to_this",
      );
      control.setAttribute("aria-label", `${control.title} ${full}`);
      card.append(control);
      if (account.usageError) {
        const warning = button("", "warning", "plain error-indicator", () =>
          notice(account.usageError, true),
        );
        warning.title = account.usageError;
        warning.setAttribute("aria-label", account.usageError);
        card.append(warning);
      }
    }
    grid.append(card);
  }
}
function renderProxy() {
  const proxy = proxyCache;
  if (!proxy) return;
  const callout = $("public-details").querySelector(".value-card");
  if (callout) callout.hidden = P.publicCallout(proxy.running) === null;
  $("proxy-state").textContent = tr(
    proxy.running ? "proxy.status.running" : "proxy.status.stopped",
  );
  $("proxy-status").classList.toggle("running", proxy.running);
  $("proxy-port-status").textContent = tr(
    "proxy.port_line_format",
    proxy.port ?? "--",
  );
  $("proxy-count").textContent = tr(
    "proxy.available_accounts_format",
    accountsCache.filter((a) => a.available).length,
  );
  $("proxy-url").value =
    proxy.baseUrl || tr("proxy.value.generated_after_start");
  $("proxy-key").value = proxy.apiKey || "";
  $("proxy-active").textContent =
    proxy.activeAccountLabel || tr("proxy.info.no_request_matched");
  $("proxy-active-id").textContent =
    proxy.activeAccountId || tr("proxy.info.active_account_hint");
  $("proxy-error").textContent = proxy.lastError || tr("common.none");
}
async function refreshView() {
  const [accounts, proxy] = await Promise.all([
    accountsController.load(),
    invoke("proxy_status"),
    refreshFeatureState(),
  ]);
  accountsCache = accounts;
  proxyCache = proxy;
  renderAccounts(accounts);
  await renderPendingWorkspaces();
  proxyController.applyLocal(
    proxy,
    configurationCache.proxyConfiguration,
    proxyController.tunnelStatus,
  );
  syncButtons();
}
function renderPreferences() {
  document
    .querySelectorAll("[data-preference]")
    .forEach(
      (input) =>
        (input.checked = Boolean(preferences[input.dataset.preference])),
    );
  const controls = P.settingsControls(preferences, installedEditors);
  $("usage-display").value = controls.usagePicker.selectedValue;
  $("restart-target").disabled = !controls.restartPicker.isEnabled;
  $("restart-target").value = controls.restartPicker.selectedValue || "";
  $("proxy-port").value = preferences.proxyPort;
}
async function savePreference(patch) {
  if (
    patch.restartEditorsOnSwitch &&
    !preferences.restartEditorTargets.length &&
    installedEditors.length
  )
    patch.restartEditorTargets = [installedEditors[0].id];
  const result = await action(async () => {
    preferences = await providerController.savePreferences(preferences, patch);
    locale = resolveLocale(preferences.locale || navigator.language);
    translate();
    renderPreferences();
    renderProviders();
    renderRemotes();
    renderTunnelControls();
  });
  if (!result) renderPreferences();
}
function closeMenus() {
  $("import-menu").hidden = true;
  $("import-open").setAttribute("aria-expanded", "false");
  $("account-menu").hidden = true;
}
mountIcons();
locale = resolveLocale(navigator.language);
translate();
$("notice-close").onclick = () => {
  $("notice").hidden = true;
};
document.querySelectorAll("[data-page]").forEach((button) =>
  button.addEventListener("click", () => {
    selectedPage = button.dataset.page;
    if (selectedPage === "proxy")
      proxyController.refresh().catch((error) => notice(error, true));
    requestAnimationFrame(applyNativeIcons);
    closeMenus();
    document
      .querySelectorAll("main>section")
      .forEach(
        (section) => (section.hidden = section.id !== `${selectedPage}-page`),
      );
    document
      .querySelectorAll("[data-page]")
      .forEach((tab) =>
        tab.setAttribute("aria-selected", String(tab === button)),
      );
  }),
);
document.querySelectorAll(".disclosure").forEach((button) =>
  button.addEventListener("click", () => {
    const opened = button.getAttribute("aria-expanded") !== "true";
    button.setAttribute("aria-expanded", String(opened));
    button.replaceChildren(icon(opened ? "chevron-up" : "chevron-down"));
    $(button.dataset.target).hidden = !opened;
    requestAnimationFrame(applyNativeIcons);
  }),
);
$("collapse-all").onclick = () => {
  collapsed = !collapsed;
  $("collapse-all").setAttribute("aria-expanded", String(!collapsed));
  $("collapse-all").replaceChildren(
    icon(collapsed ? "chevron-down" : "chevron-up"),
  );
  translate();
  renderAccounts(accountsCache);
  syncButtons();
};
$("import-open").onclick = async () => {
  const bounds = $("import-open").getBoundingClientRect();
  closeMenus();
  try {
    await invoke("import_menu", {
      labels: [
        tr("accounts.action.import_current_auth"),
        tr("accounts.action.import_auth_file"),
        tr("accounts.action.import_sub2api"),
      ],
      x: bounds.left,
      y: bounds.bottom + 4,
    });
  } catch (error) {
    notice(error, true);
  }
};
window.__TAURI__.event
  .listen("import-action", ({ payload }) => {
    if (payload === "import-providers") openProviderImport();
    else $(payload)?.click();
  })
  .catch((error) => notice(error, true));
document.addEventListener("click", (event) => {
  if (
    !event.target.closest(".menu-anchor") &&
    !event.target.closest(".account-menu")
  )
    closeMenus();
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") closeMenus();
});
$("import-current").onclick = () => {
  closeMenus();
  action(() => accountsController.importCurrent());
};
$("import-file-open").onclick = () => {
  closeMenus();
  $("import-file").click();
};
$("import-file").onchange = async (event) => {
  const file = event.target.files[0];
  if (!file) return;
  await action(
    async () => {
      if (file.size > 16 * 1024 * 1024)
        throw new Error(extra("File exceeds 16 MiB", "文件不能超过 16 MiB"));
      return accountsController.importDocument(await file.text());
    },
    (count) => extra(`Imported ${count} accounts`, `已导入 ${count} 个账号`),
  );
  event.target.value = "";
};
$("add-account").onclick = () => {
  $("import-error").hidden = true;
  $("import-dialog").showModal();
};
$("import-cancel").onclick = () => $("import-dialog").close();
$("import-dialog").addEventListener("close", () => {
  $("import-content").value = "";
  $("refresh-token").value = "";
});
for (const [id, token] of [
  ["auth-json-tab", false],
  ["refresh-token-tab", true],
])
  $(id).onclick = () => {
    importingToken = token;
    $("auth-json-tab").setAttribute("aria-pressed", String(!token));
    $("refresh-token-tab").setAttribute("aria-pressed", String(token));
    $("refresh-token").hidden = !token;
    $("import-content").hidden = token;
  };
$("import-form").onsubmit = (event) => {
  event.preventDefault();
  action(
    async () => {
      try {
        const count = importingToken
          ? await accountsController.importRefreshToken(
              $("refresh-token").value.trim(),
            )
          : await accountsController.importDocument($("import-content").value);
        $("import-dialog").close();
        return count;
      } catch (error) {
        $("import-error").textContent = String(error);
        $("import-error").hidden = false;
        throw error;
      }
    },
    (count) => extra(`Imported ${count} accounts`, `已导入 ${count} 个账号`),
  );
};
function openAccountProxy(account) {
  closeMenus();
  editingAccount = account;
  $("edit-name").textContent = account.email || account.accountId;
  $("edit-proxy").value = account.proxyUrl || "";
  $("edit-proxy-help").textContent = tr(
    account.sourceTag === "SUB2API"
      ? "accounts.proxy.provider_help"
      : "settings.sub2api.proxy_help",
  );
  $("edit-error").hidden = true;
  $("edit-dialog").showModal();
  $("edit-proxy").focus();
}
$("account-proxy-edit").onclick = () => {
  if (editingAccount) openAccountProxy(editingAccount);
};
$("edit-cancel").onclick = () => $("edit-dialog").close();
$("edit-form").onsubmit = (event) => {
  event.preventDefault();
  action(async () => {
    try {
      await accountsController.saveProxy(
        editingAccount.id,
        $("edit-proxy").value,
      );
      $("edit-dialog").close();
    } catch (error) {
      $("edit-error").textContent = String(error);
      $("edit-error").hidden = false;
      throw error;
    }
  });
};
$("refresh-all").onclick = () => {
  if (!busy) action(() => accountsController.refresh());
};
$("smart-switch").onclick = () =>
  action(
    () => invoke("smart_switch"),
    (label) => extra(`Switched to ${label}`, `已切换到 ${label}`),
  );
$("proxy-start").onclick = () =>
  action(() => proxyController.startProxy(Number($("proxy-port").value)));
$("proxy-stop").onclick = () => action(() => proxyController.stopProxy());
$("proxy-refresh").onclick = () => action(() => proxyController.refresh());
$("toggle-key").onclick = () => {
  $("proxy-key").type =
    $("proxy-key").type === "password" ? "text" : "password";
};
document.querySelectorAll(".copy").forEach(
  (button) =>
    (button.onclick = () => {
      const value = $(button.dataset.copy).value;
      if (value)
        navigator.clipboard
          .writeText(value)
          .then(() => notice(extra("Copied", "已复制")))
          .catch((error) => notice(error, true));
    }),
);
document
  .querySelectorAll("[data-preference]")
  .forEach(
    (input) =>
      (input.onchange = () =>
        savePreference({ [input.dataset.preference]: input.checked })),
  );
$("usage-display").onchange = () =>
  savePreference({ usageDisplay: $("usage-display").value });
$("language").onchange = () => savePreference({ locale: $("language").value });
$("quit").onclick = () =>
  providerController.quit().catch((error) => notice(error, true));
document.addEventListener(
  "DOMContentLoaded",
  async () => {
    try {
      [preferences, nativeIcons] = await Promise.all([
        invoke("preferences"),
        invoke("native_symbols"),
      ]);
      applyNativeIcons();
      locale = resolveLocale(preferences.locale || navigator.language);
      translate();
      await initializeFeatures();
      renderPreferences();
      await refreshView();
    } catch (error) {
      notice(error, true);
    }
  },
  { once: true },
);
setInterval(() => {
  if (!busy && !document.hidden && !document.querySelector("dialog[open]"))
    refreshView().catch((error) => notice(error, true));
}, 15000);

let changeRefreshTimer;
window.__TAURI__.event
  .listen("copool-changed", () => {
    clearTimeout(changeRefreshTimer);
    changeRefreshTimer = setTimeout(() => {
      if (!busy) refreshView().catch((error) => notice(error, true));
    }, 50);
  })
  .catch((error) => notice(error, true));
