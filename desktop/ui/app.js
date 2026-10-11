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
let switchingId = null;
let accountsCache = [];
let proxyCache = null;
let preferences = null;
let locale = "en";
let noticeTimer;
let nativeIcons = {};
let iconFrame = 0;
const ICONS = {
  folder: '<path d="M2 5h7l2 3h9v12H2Z"/>',
  network:
    '<circle cx="11" cy="11" r="8.7"/><ellipse cx="11" cy="11" rx="4" ry="8.7" transform="rotate(-23 11 11)"/><path d="M3.4 6.8c3.5 3.7 8.8 6.5 16.1 7.2M4 16c4.9-.4 10-3 13.9-8.3"/>',
  plus: '<path d="M11 3v16M3 11h16"/>',
  refresh: '<path d="M18 8a8 8 0 1 0 .4 6M18 3v5h-5"/>',
  "chevron-up": '<path d="m5 14 6-6 6 6"/>',
  "chevron-down": '<path d="m5 8 6 6 6-6"/>',
  trash: '<path d="M3 5h16M8 5V3h6v2M5 5l1 15h10l1-15M9 8v9M13 8v9"/>',
  switch:
    '<circle cx="11" cy="11" r="9" fill="currentColor" stroke="none"/><path d="M7 8h8M7 8l3-3M7 8l3 3M15 14H7M15 14l-3-3M15 14l-3 3" stroke="var(--window)" stroke-width="1.5"/>',
  copy: '<rect x="7" y="6" width="12" height="14" rx="2"/><path d="M14 6V3H3v13h4"/>',
  eye: '<path d="M1 11s4-7 10-7 10 7 10 7-4 7-10 7S1 11 1 11Z"/><circle cx="11" cy="11" r="3"/>',
  close: '<path d="m5 5 12 12M5 17 17 5"/>',
  "chevron-left": '<path d="m14 5-6 6 6 6"/>',
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
// SF Symbol names used by the presentation models, as panel icons.
const SYMBOL_ICONS = {
  "chevron.down": "chevron-down",
  "chevron.up": "chevron-up",
};
function setIcon(button, glyph) {
  if (button.querySelector(`[data-glyph="${glyph}"]`)) return;
  button.replaceChildren(icon(glyph));
}
function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}
/// The saved language, else the first supported system language.
function appLocale() {
  return preferences?.locale
    ? P.resolveLocale(preferences.locale)
    : P.preferredLocale(navigator.languages?.length ? navigator.languages : [navigator.language]);
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
  $("page-title").textContent = tr(`tab.${selectedPage}`);
  // features.js defines the controllers after this script's first translate().
  if (typeof accountsController !== "undefined") renderToolbar();
}
// Toolbar shortcuts: Cmd (Ctrl elsewhere) with these keys.
const TOOLBAR_SHORTCUTS = { i: "import-open", n: "add-account", r: "refresh-all" };
function withShortcut(text, key) {
  const mac = navigator.userAgent.includes("Mac");
  return `${text} (${mac ? "⌘" : "Ctrl+"}${key.toUpperCase()})`;
}
/// The account toolbar, as the SwiftUI leading and trailing action groups.
function renderToolbar() {
  const options = {
    importing: busy,
    switching: busy,
    canRefresh: !busy && accountsCache.length > 0,
    spinning: accountsController.spinner,
    collapsed,
  };
  const [importAccount, addAccount, smartSwitch] = P.actions(options);
  const [refresh, collapse] = P.trailingActions(options);
  const label = (button, key) => {
    button.title = tr(key);
    button.setAttribute("aria-label", button.title);
  };
  $("import-open").textContent = tr(importAccount.title);
  $("import-open").title = withShortcut(tr(importAccount.title), "i");
  $("import-open").disabled = !importAccount.enabled;
  $("add-account").textContent = tr(addAccount.title);
  $("add-account").title = withShortcut(tr(addAccount.title), "n");
  $("add-account").disabled = !addAccount.enabled;
  $("smart-switch").title = $("smart-switch").textContent;
  $("smart-switch").disabled = !smartSwitch.enabled || !accountsCache.length;
  label($("refresh-all"), "common.refresh_usage");
  $("refresh-all").title = withShortcut($("refresh-all").title, "r");
  $("refresh-all").disabled = !refresh.enabled;
  $("refresh-all").classList.toggle("spinning", Boolean(refresh.spinning));
  setIcon($("collapse-all"), SYMBOL_ICONS[collapse.symbol]);
  label($("collapse-all"), collapse.label);
  $("collapse-all").setAttribute("aria-expanded", String(!collapsed));
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
  document.querySelectorAll("button").forEach((button) => {
    button.disabled =
      busy && !["login-cancel", "notice-close"].includes(button.id);
  });
  renderToolbar();
  $("proxy-start").disabled = busy || Boolean(proxyCache?.running);
  $("proxy-stop").disabled = busy || !proxyCache?.running;
  $("proxy-start").hidden = !!proxyCache?.running;
  $("proxy-stop").hidden = !proxyCache?.running;
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
function confirmAction(title, description, accept, destructive = false) {
  $("confirm-title").textContent = title;
  $("confirm-description").textContent = description;
  $("confirm-accept").textContent = accept;
  $("confirm-accept").classList.toggle("danger", destructive);
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
const METER_HEIGHT = 4; // .meter in style.css
function usageMode() {
  return preferences?.usageDisplay || "used";
}
function usageWindow(view, title, tone) {
  const remaining = usageMode() === "remaining";
  const used = remaining ? view.secondaryPercent : view.primaryPercent;
  const track = el(
    "div",
    `meter${view.available && used >= 95 ? " full" : view.available && used >= 80 ? " high" : ""}`,
  );
  track.setAttribute("role", "progressbar");
  track.setAttribute("aria-label", title);
  if (view.available)
    track.setAttribute("aria-valuenow", String(view.progressPercent));
  track.setAttribute("aria-valuemin", "0");
  track.setAttribute("aria-valuemax", "100");
  const fill = el("div", "meter-fill");
  const metrics = P.progressMetrics(
    view.available ? view.progressPercent / 100 : 0,
    0,
    METER_HEIGHT,
    0,
  );
  fill.hidden = !metrics.showsFill;
  fill.style.width = `${metrics.fillScale * 100}%`;
  fill.style.minWidth = `${metrics.grooveHeight}px`;
  track.append(fill);
  const [primary, secondary] = remaining
    ? ["accounts.window.remaining_format", "accounts.window.used_format"]
    : ["accounts.window.used_format", "accounts.window.remaining_format"];
  const reset =
    view.available && view.reset !== "--"
      ? tr("accounts.window.reset_at_format", view.reset)
      : tr("accounts.window.reset_unavailable");
  const row = el("div", `usage ${tone}${view.available ? "" : " no-data"}`);
  row.append(
    el("span", "usage-label", title),
    track,
    el(
      "span",
      "usage-value",
      view.available
        ? tr(primary, `${view.primaryPercent}%`)
        : tr("accounts.window.no_data"),
    ),
    el(
      "span",
      "usage-reset",
      view.available
        ? `${tr(secondary, `${view.secondaryPercent}%`)} · ${reset}`
        : reset,
    ),
  );
  return row;
}
function compactUsage(value, label) {
  const text = value === null ? "--" : `${Math.round(value)}%`;
  const node = el("span", "usage-value");
  node.append(`${label} `, el("b", "", text));
  node.setAttribute("aria-label", `${label}: ${text}`);
  return node;
}
// As in Coport: no WebKit context menu (Reload, …) outside text.
document.addEventListener("contextmenu", (event) => {
  if (!event.target.closest("input, textarea, pre, dd, .message-body"))
    event.preventDefault();
});
let renderedAccountsState;
/// Proxy and removal, in the card's top row as in the SwiftUI card.
function accountManagement(account) {
  const actions = el("span", "block-aside");
  const proxyButton = button("", "network", "icon-btn", () =>
    openAccountProxy(account),
  );
  proxyButton.title = tr("accounts.proxy.title");
  proxyButton.setAttribute(
    "aria-label",
    `${tr("accounts.proxy.title")} ${account.email || account.label}`,
  );
  const remove = button("", "trash", "icon-btn destructive", async () => {
    if (
      await confirmAction(
        tr("common.remove"),
        extra(
          `Remove ${account.label} from the account pool?`,
          `从账号池移除 ${account.label}？`,
        ),
        tr("common.remove"),
        true,
      )
    )
      await action(() => accountsController.removeAccount(account.id));
  });
  remove.title = tr("common.remove");
  remove.setAttribute("aria-label", `${tr("common.remove")} ${account.label}`);
  actions.append(proxyButton, remove);
  return actions;
}
/// Usage warning, refresh and switch, at the card's bottom right as in the
/// SwiftUI card, clear of long account names.
function accountControls(account, full) {
  const actions = el("span", "block-aside");
  const reauthenticate = P.needsReauthentication(
    account,
    tr("error.accounts.sign_in_expired"),
  );
  if (!collapsed && account.usageError) {
    const warning = button("", "warning", "icon-btn error-indicator", () =>
      notice(account.usageError, true),
    );
    warning.title = account.usageError;
    warning.setAttribute("aria-label", account.usageError);
    actions.append(warning);
  }
  // Every account refreshes on its own; an expired sign-in offers sign-in.
  const refresh = button(
    "",
    reauthenticate ? "plus" : "refresh",
    "icon-btn card-refresh",
    reauthenticate
      ? () => signIn(account.accountId)
      : () => action(() => accountsController.refresh(account.id)),
  );
  refresh.title = tr(
    reauthenticate ? "accounts.card.sign_in_again" : "common.refresh_usage",
  );
  refresh.setAttribute("aria-label", `${refresh.title} ${full}`);
  refresh.classList.toggle(
    "spinning",
    accountsController.refreshingIds.has(account.id) ||
      accountsController.remoteRefreshingIds.has(account.id),
  );
  actions.append(refresh);
  if (!account.isCurrent) {
    const control = button("", "switch", "icon-btn card-switch", async () => {
      switchingId = account.id;
      renderAccounts(accountsCache);
      await action(
        () => invoke("switch_account", { id: account.id }),
        extra(`Switched to ${full}`, `已切换到 ${full}`),
      );
      switchingId = null;
      renderAccounts(accountsCache);
    });
    control.dataset.available = String(account.available);
    control.title = tr("accounts.card.switch_to_this");
    control.setAttribute("aria-label", `${control.title} ${full}`);
    actions.append(control);
  }
  return actions;
}
function accountTags(account, view) {
  const tags = el("span", "account-tags");
  tags.append(el("span", "tag plan", view.plan));
  if (view.workspaceTag)
    tags.append(el("span", "tag muted", view.workspaceTag));
  if (account.isCurrent)
    tags.append(el("span", "tag good", tr("accounts.card.current")));
  if (view.status === "deactivated")
    tags.append(el("span", "tag bad", tr("accounts.card.status.deactivated")));
  return tags;
}
function renderAccounts(accounts) {
  const renderState = [
    accounts,
    collapsed,
    locale,
    preferences?.usageDisplay,
    [...accountsController.refreshingIds],
    [...accountsController.remoteRefreshingIds],
    switchingId,
  ];
  if (P.cardsEqual(renderState, renderedAccountsState)) return;
  renderedAccountsState = structuredClone(renderState);
  accounts = P.visibleAccounts(accounts);
  const list = $("accounts");
  list.className = collapsed ? "account-list compact block flush" : "account-list";
  list.replaceChildren();
  if (!accounts.length) {
    list.className = "account-list block";
    const empty = el("div", "empty");
    empty.append(
      el("span", "row-name", tr("accounts.empty.title")),
      el("span", "setting-description", tr("accounts.empty.message.no_accounts")),
    );
    list.append(empty);
    return;
  }
  for (const account of accounts) {
    const full = account.email || account.accountId;
    const view = P.card(account, collapsed, usageMode(), (epoch) =>
      P.formatReset(epoch, locale),
    );
    if (collapsed) {
      const interaction = P.collapsedInteraction({
        collapsed,
        current: account.isCurrent,
        switching: switchingId === account.id,
        hovered: false,
      });
      const row = el(
        "div",
        `account-row${account.isCurrent ? " current running" : ""}`,
      );
      row.classList.toggle("can-reveal", interaction.canReveal);
      row.classList.toggle("revealed", interaction.visible);
      row.dataset.plan = view.plan;
      const name = el("span", "account-row-name", view.name);
      name.title = full;
      row.append(
        el("span", "dot"),
        name,
        el("span", "tag plan", view.plan),
        compactUsage(view.compact[0], "5h"),
        compactUsage(view.compact[1], "1w"),
        accountControls(account, full),
      );
      list.append(row);
      continue;
    }
    const card = el(
      "section",
      `block account${account.isCurrent ? " current" : ""}`,
    );
    card.dataset.plan = view.plan;
    const head = el("div", "block-head");
    head.append(accountTags(account, view), accountManagement(account));
    // The CURRENT tag alone marks the active account, as in the SwiftUI card.
    const name = el("div", "account-name");
    const title = el("span", "block-title", full);
    title.title = full;
    name.append(title);
    const credits =
      view.credits === "unlimited" ? tr("accounts.card.unlimited") : view.credits;
    const resets = account.usage?.remainingResetCount;
    const footer = el("div", "account-footer");
    footer.append(
      el(
        "span",
        "account-footer-text",
        `${tr("accounts.card.credits_format", credits)} · ${tr(resets != null ? "accounts.card.remaining_resets_format" : "accounts.card.resets_unavailable_format", resets ?? "--")}`,
      ),
      accountControls(account, full),
    );
    card.append(
      head,
      name,
      usageWindow(
        view.fiveHour,
        tr("accounts.window.five_hour"),
        "five-hour",
      ),
      usageWindow(
        view.oneWeek,
        tr("accounts.window.one_week"),
        "one-week",
      ),
      footer,
    );
    list.append(card);
  }
  alignMeters(list);
}
/// Gives every usage row the widest label and value, so all meters line up.
function alignMeters(list) {
  list.style.removeProperty("--usage-label-width");
  list.style.removeProperty("--usage-value-width");
  const widest = (selector) =>
    Math.ceil(
      Math.max(0, ...[...list.querySelectorAll(selector)].map((n) => n.scrollWidth)),
    );
  const label = widest(".usage .usage-label");
  const value = widest(".usage .usage-value");
  if (label) list.style.setProperty("--usage-label-width", `${label}px`);
  if (value) list.style.setProperty("--usage-value-width", `${value}px`);
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
    locale = appLocale();
    translate();
    renderPreferences();
    renderProviders();
    renderRemotes();
    renderTunnelControls();
  });
  if (!result) renderPreferences();
}
mountIcons();
locale = appLocale();
translate();
$("notice-close").onclick = () => {
  $("notice").hidden = true;
};
function showPage(page) {
  selectedPage = page;
  if (selectedPage === "proxy")
    proxyController.refresh().catch((error) => notice(error, true));
  closeMenu();
  document
    .querySelectorAll("main>section")
    .forEach(
      (section) => (section.hidden = section.id !== `${selectedPage}-page`),
    );
  document
    .querySelectorAll("[data-page]")
    .forEach((link) =>
      link.setAttribute(
        "aria-selected",
        String(link.dataset.page === selectedPage),
      ),
    );
  $("top-main").hidden = selectedPage !== "accounts";
  $("top-sub").hidden = selectedPage === "accounts";
  $("page-title").textContent = tr(`tab.${selectedPage}`);
  // Widths measure as zero while the page is hidden.
  if (selectedPage === "accounts") alignMeters($("accounts"));
  requestAnimationFrame(applyNativeIcons);
}
document
  .querySelectorAll("[data-page]")
  .forEach((link) =>
    link.addEventListener("click", () => showPage(link.dataset.page)),
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
  renderAccounts(accountsCache);
  syncButtons();
};
// In-panel menus, as Coport's: they keep focus in the panel window, which a
// native menu would take away.
let openMenu = null;
function closeMenu(restoreFocus = false) {
  if (!openMenu) return;
  const { menu, trigger } = openMenu;
  openMenu = null;
  menu.remove();
  trigger.setAttribute("aria-expanded", "false");
  if (restoreFocus) trigger.focus();
}
function showMenu(trigger, items) {
  closeMenu();
  const menu = el("div", "select-menu action-menu");
  menu.setAttribute("role", "menu");
  for (const item of items) {
    const option = el("button", "", item.label);
    option.type = "button";
    option.setAttribute("role", "menuitem");
    option.onclick = () => {
      closeMenu(true);
      item.run();
    };
    menu.append(option);
  }
  document.body.append(menu);
  // Below the trigger, or above it when the panel is too short; kept inside.
  const bounds = trigger.getBoundingClientRect();
  const left = Math.min(bounds.left, innerWidth - menu.offsetWidth - 8);
  const below = bounds.bottom + 4;
  const top =
    below + menu.offsetHeight <= innerHeight - 8
      ? below
      : Math.max(8, bounds.top - menu.offsetHeight - 4);
  menu.style.left = `${Math.max(8, left)}px`;
  menu.style.top = `${top}px`;
  menu.style.maxHeight = `${innerHeight - top - 8}px`;
  trigger.setAttribute("aria-expanded", "true");
  openMenu = { menu, trigger };
  menu.querySelector("button")?.focus();
}
document.addEventListener("pointerdown", (event) => {
  if (
    openMenu &&
    !openMenu.menu.contains(event.target) &&
    !openMenu.trigger.contains(event.target)
  )
    closeMenu();
});
$("import-open").onclick = () => {
  if (openMenu?.trigger === $("import-open")) {
    closeMenu(true);
    return;
  }
  showMenu($("import-open"), [
    {
      label: tr("accounts.action.import_current_auth"),
      run: () => action(() => accountsController.importCurrent()),
    },
    {
      label: tr("accounts.action.import_auth_file"),
      run: () => importAccountFile(),
    },
    {
      label: tr("accounts.action.import_sub2api"),
      run: () => openProviderImport(),
    },
  ]);
};
// Coport's panel shortcuts. An open dialog keeps Escape and Return for
// cancel and submit, as the SwiftUI dialogs did.
document.addEventListener("keydown", (event) => {
  if (document.querySelector("dialog[open]")) return;
  if (openMenu) {
    const items = [...openMenu.menu.querySelectorAll("button")];
    const index = items.indexOf(document.activeElement);
    if (event.key === "Escape" || event.key === "Tab") {
      event.preventDefault();
      closeMenu(true);
    } else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
      event.preventDefault();
      const next =
        event.key === "Home"
          ? 0
          : event.key === "End"
            ? items.length - 1
            : (index + (event.key === "ArrowDown" ? 1 : -1) + items.length) %
              items.length;
      items[next].focus();
    }
    return;
  }
  const mac = navigator.userAgent.includes("Mac");
  const mod = event.metaKey || event.ctrlKey;
  const back =
    !event.shiftKey &&
    (mac
      ? event.metaKey && !event.ctrlKey && !event.altKey && event.key === "["
      : event.altKey && !event.ctrlKey && !event.metaKey && event.key === "ArrowLeft");
  if (event.key === "Escape") {
    // Back from a subpage first; on the accounts page, dismiss like a menu.
    if (selectedPage !== "accounts") showPage("accounts");
    else invoke("hide_panel").catch(() => {});
  } else if (back) {
    event.preventDefault();
    if (selectedPage !== "accounts") showPage("accounts");
  } else if (mod && event.key === "q") {
    event.preventDefault();
    providerController.quit().catch((error) => notice(error, true));
  } else if (mod && event.key === ",") {
    event.preventDefault();
    showPage("settings");
  } else if (mod && !event.altKey && !event.shiftKey && TOOLBAR_SHORTCUTS[event.key]) {
    // Always handled, so Cmd+R never reloads the page; a disabled button
    // ignores its shortcut as it ignores clicks.
    event.preventDefault();
    if (selectedPage !== "accounts") showPage("accounts");
    $(TOOLBAR_SHORTCUTS[event.key]).click();
  }
});
function openAccountProxy(account) {
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
      locale = appLocale();
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

// The panel follows its content height, like a menu; the native side clamps
// it to the screen and keeps it under the status bar icon.
let fitFrame = 0;
let fittedHeight = 0;
function queueFit() {
  if (fitFrame) return;
  fitFrame = requestAnimationFrame(() => {
    fitFrame = 0;
    const section = document.querySelector("main>section:not([hidden])");
    const page = section?.firstElementChild;
    if (!page) return;
    const style = getComputedStyle(section);
    let height =
      document.querySelector(".top").offsetHeight +
      page.offsetHeight +
      parseFloat(style.paddingTop) +
      parseFloat(style.paddingBottom);
    const dialog = document.querySelector("dialog[open]");
    if (dialog) height = Math.max(height, dialog.offsetHeight + 40);
    height = Math.ceil(height);
    if (Math.abs(height - fittedHeight) > 1) {
      fittedHeight = height;
      invoke("fit_panel", { height }).catch(() => {});
    }
  });
}
const fitObserver = new ResizeObserver(queueFit);
document
  .querySelectorAll(".top, main>section>.page, dialog")
  .forEach((node) => fitObserver.observe(node));

// A panel reopened after a while starts on the accounts page, as in Coport.
const PANEL_PAGE_TIMEOUT_MS = 60 * 1000;
let panelHiddenAt = null;
window.__TAURI__.event
  .listen("panel-hidden", () => {
    panelHiddenAt = Date.now();
    closeMenu();
  })
  .catch((error) => notice(error, true));
window.__TAURI__.event
  .listen("panel-shown", () => {
    if (panelHiddenAt && Date.now() - panelHiddenAt > PANEL_PAGE_TIMEOUT_MS)
      showPage("accounts");
    panelHiddenAt = null;
    queueFit();
    if (!busy && !document.querySelector("dialog[open]"))
      refreshView().catch((error) => notice(error, true));
  })
  .catch((error) => notice(error, true));

let changeRefreshTimer;
window.__TAURI__.event
  .listen("copool-changed", () => {
    clearTimeout(changeRefreshTimer);
    changeRefreshTimer = setTimeout(() => {
      if (!busy) refreshView().catch((error) => notice(error, true));
    }, 50);
  })
  .catch((error) => notice(error, true));
