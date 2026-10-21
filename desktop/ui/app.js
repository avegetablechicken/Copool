"use strict";
const $ = (id) => document.getElementById(id);
const invoke = (command, args = {}) =>
  window.__TAURI__.core.invoke(command, args);
const P = window.CopoolPresentation;
let collapsed = false;
let selectedPage = "accounts";
let editingAccount = null;
let switchingId = null;
// A card is being dragged: the list waits to render until it is dropped.
let reorderingAccounts = false;
let accountsCache = [];
/// When the background refresh of the whole pool next starts (Unix seconds).
let fullRefreshAt = null;
let proxyCache = null;
let preferences = null;
let locale = "en";
let noticeTimer;
let nativeIcons = {};
let iconFrame = 0;
// The account pool on the accounts page: Codex or Claude Code.
let pool = localStorage.getItem("copool.pool") === "claude" ? "claude" : "codex";
// Reset times as the date and time or as the time left, as OpenQuota toggles
// them by clicking either.
let resetDisplay =
  localStorage.getItem("copool.resetDisplay") === "countdown"
    ? "countdown"
    : "exact";
const CLAUDE_COMMANDS = {
  accounts: "claude_accounts",
  import_current: "claude_import_current",
  import_account_file: "claude_import_account_file",
  delete_account: "claude_delete_account",
  save_account_proxy: "claude_save_account_proxy",
  set_renewal: "claude_set_renewal",
  refresh_usage: "claude_refresh_usage",
  say_hi: "claude_say_hi",
  refresh_provider: "claude_refresh_provider",
  switch_account: "claude_switch_account",
  smart_switch: "claude_smart_switch",
  begin_login: "begin_claude_login",
};
function poolCommand(command, target = pool) {
  return target === "claude" ? CLAUDE_COMMANDS[command] || command : command;
}
/// The commands of one pool. Claude has no workspaces or remote servers.
function poolInvoke(target) {
  return (command, args = {}) => {
    if (target === "claude" && command === "workspace_directory")
      return Promise.resolve([]);
    if (target === "claude" && command === "cancel_workspace_refresh")
      return Promise.resolve(null);
    return invoke(poolCommand(command, target), args);
  };
}
const ICONS = {
  folder: '<path d="M2 5h7l2 3h9v12H2Z"/>',
  network:
    '<circle cx="11" cy="11" r="8.7"/><ellipse cx="11" cy="11" rx="4" ry="8.7" transform="rotate(-23 11 11)"/><path d="M3.4 6.8c3.5 3.7 8.8 6.5 16.1 7.2M4 16c4.9-.4 10-3 13.9-8.3"/>',
  plus: '<path d="M11 3v16M3 11h16"/>',
  refresh: '<path d="M18 8a8 8 0 1 0 .4 6M18 3v5h-5"/>',
  "say-hi": '<path d="M3 4h16v11H9l-4 4v-4H3Z"/>',
  renewal:
    '<path d="M18.6 12.5A7.7 7.7 0 1 1 16.3 5.4M17 2.4v3.8h-3.8"/><path d="M11 7.2V11l2.6 1.8"/>',
  "chevron-up": '<path d="m5 14 6-6 6 6"/>',
  "chevron-down": '<path d="m5 8 6 6 6-6"/>',
  trash: '<path d="M3 5h16M8 5V3h6v2M5 5l1 15h10l1-15M9 8v9M13 8v9"/>',
  switch:
    '<circle cx="11" cy="11" r="9" fill="currentColor" stroke="none"/><path d="M7 8h8M7 8l3-3M7 8l3 3M15 14H7M15 14l-3-3M15 14l-3 3" stroke="var(--window)" stroke-width="1.5"/>',
  copy: '<rect x="7" y="6" width="12" height="14" rx="2"/><path d="M14 6V3H3v13h4"/>',
  eye: '<path d="M1 11s4-7 10-7 10 7 10 7-4 7-10 7S1 11 1 11Z"/><circle cx="11" cy="11" r="3"/>',
  close: '<path d="m5 5 12 12M5 17 17 5"/>',
  "chevron-left": '<path d="m14 5-6 6 6 6"/>',
  import: '<path d="M7 8H4v12h14V8h-3M11 2v11M7.5 9.5 11 13l3.5-3.5"/>',
  reauthenticate:
    '<circle cx="10" cy="11" r="8"/><circle cx="10" cy="9" r="2.6"/><path d="M5 16.5c1.4-2 3-2.8 5-2.8s3.6.8 5 2.8M18 2v4M18 8.5v.5"/>',
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
  "square.and.arrow.down": "import",
  plus: "plus",
  xmark: "close",
};
function setIcon(button, glyph) {
  if (button.querySelector(`[data-glyph="${glyph}"]`)) return;
  button.replaceChildren(icon(glyph));
}
/// An icon and a short title, as the SwiftUI toolbar's labeled buttons.
function setLabeledIcon(button, glyph, text) {
  button.classList.add("labeled");
  if (!button.querySelector(`[data-glyph="${glyph}"]`))
    button.replaceChildren(icon(glyph), el("span"));
  button.lastChild.textContent = text;
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
  syncGeneralSelectLabels();
  $("page-title").textContent = tr(`tab.${selectedPage}`);
  document.querySelectorAll(".pool-btn").forEach((button) => {
    button.title = withShortcut(
      tr(`pool.${button.dataset.pool}`),
      button.dataset.pool === "codex" ? "1" : "2",
    );
    button.setAttribute("aria-label", tr(`pool.${button.dataset.pool}`));
  });
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
  // As Swift's AccountsActionPresentation: each action waits only for its own.
  const options = {
    importing: accountsController.importing,
    adding: accountsController.adding,
    switching: switchingId !== null,
    canRefresh: !accountsController.adding,
    spinning: accountsController.spinner,
    collapsed,
  };
  const [importAccount, addAccount, smartSwitch] = P.actions(options);
  const [refresh, collapse] = P.trailingActions(options);
  const label = (button, key) => {
    button.title = tr(key);
    button.setAttribute("aria-label", button.title);
  };
  setLabeledIcon(
    $("import-open"),
    SYMBOL_ICONS[importAccount.symbol],
    tr(importAccount.label),
  );
  $("import-open").title = withShortcut(tr(importAccount.title), "i");
  $("import-open").setAttribute("aria-label", tr(importAccount.title));
  $("import-open").disabled = !importAccount.enabled;
  setLabeledIcon(
    $("add-account"),
    SYMBOL_ICONS[addAccount.symbol],
    tr(addAccount.label),
  );
  $("add-account").title = withShortcut(tr(addAccount.title), "n");
  $("add-account").setAttribute("aria-label", tr(addAccount.title));
  $("add-account").disabled = !addAccount.enabled;
  $("smart-switch").title = $("smart-switch").textContent;
  $("smart-switch").disabled = !smartSwitch.enabled || !accountsCache.length;
  label($("refresh-all"), "common.refresh_usage");
  // The background refresh of the whole pool keeps its own schedule.
  refreshTooltip(
    $("refresh-all"),
    withShortcut($("refresh-all").title, "r"),
    () => fullRefreshAt,
  );
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
  // As NoticeMessage.autoDismissDelay: 3 s, or 5 s for errors.
  noticeTimer = setTimeout(
    () => {
      $("notice").hidden = true;
    },
    error ? 5000 : 3000,
  );
}
function syncButtons() {
  renderToolbar();
  const proxyLoading = proxyController.loading;
  $("proxy-start").disabled = proxyLoading || Boolean(proxyCache?.running);
  $("proxy-stop").disabled = proxyLoading || !proxyCache?.running;
  $("proxy-refresh").disabled = proxyLoading;
  $("proxy-key-refresh").disabled = proxyLoading;
  $("proxy-start").hidden = !!proxyCache?.running;
  $("proxy-stop").hidden = !proxyCache?.running;
  // A card refreshing its usage only locks its own refresh button.
  document
    .querySelectorAll(".card-refresh.spinning")
    .forEach((button) => (button.disabled = true));
  // A switch locks only its own card's switch button.
  document.querySelectorAll(".card-switch").forEach((button) => {
    button.disabled =
      button.dataset.available === "false" || button.dataset.id === switchingId;
  });
  document
    .querySelectorAll(".remote-actions button")
    .forEach(
      (button) => (button.disabled = button.dataset.available === "false"),
    );
  renderTunnelState();
}
/// Runs a task and reports it. No panel-wide lock, as in the Swift app:
/// each operation disables only its own controls through its own state.
async function action(task, message, reload = true) {
  try {
    const result = await task();
    if (message)
      notice(typeof message === "function" ? message(result) : message);
    return true;
  } catch (error) {
    notice(error, true);
    return false;
  } finally {
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
/// A window's reset time in `mode`: the date and time, or the time left.
function resetLabel(view, mode) {
  if (!view.available || view.resetAt == null)
    return tr("accounts.window.reset_unavailable");
  if (mode === "exact")
    return tr("accounts.window.reset_at_format", view.reset);
  const countdown = P.resetCountdown(view.resetAt, Date.now() / 1000, locale);
  return countdown.soon
    ? tr("accounts.window.resets_soon")
    : tr("accounts.window.resets_in_format", countdown.remaining);
}
function toggleResetDisplay() {
  resetDisplay = resetDisplay === "countdown" ? "exact" : "countdown";
  localStorage.setItem("copool.resetDisplay", resetDisplay);
  renderAccounts(accountsCache);
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
  const primary = remaining
    ? "accounts.window.remaining_format"
    : "accounts.window.used_format";
  const known = view.available && view.resetAt != null;
  // A known reset time switches between both forms and shows the other one
  // on hover, as in OpenQuota.
  const reset = known
    ? button("", null, "usage-reset", toggleResetDisplay)
    : el("span", "usage-reset");
  reset.textContent = resetLabel(view, resetDisplay);
  if (known)
    reset.addEventListener("mouseenter", () => {
      reset.title = resetLabel(
        view,
        resetDisplay === "countdown" ? "exact" : "countdown",
      );
    });
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
    reset,
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
// The pool, mode and current card last rendered, to animate what changed.
let renderedLayout = null;
// The button an import or sign-in was started from, while it runs: its new
// cards drop from there into place.
let dropOrigin = null;
/// Cards new while `task` runs drop from `origin` rather than fade in.
async function droppingNewCards(origin, task) {
  dropOrigin = origin;
  try {
    return await task();
  } finally {
    if (dropOrigin === origin) dropOrigin = null;
  }
}
/// The provider of a Sub2api card, from its id (`sub2api-<provider>-<n>`,
/// `claude-sub2api-<provider>-<n>`): cards synced since the provider list
/// was last read still group with their provider.
function providerKey(account) {
  if (account.sourceTag !== "SUB2API") return null;
  return /^(?:claude-)?sub2api-(.+)-\d+$/.exec(account.id)?.[1] ?? null;
}
/// The saved Sub2api provider a card belongs to, if any.
function accountProvider(account) {
  const key = providerKey(account);
  if (key === null) return null;
  return pool === "claude"
    ? claudeProvidersSaved.find((p) => p.id === key) || null
    : providerController.persisted.find(
        (p) => p.providerID.trim().toLowerCase() === key.toLowerCase(),
      ) || null;
}
function providerTitle(account) {
  const provider = accountProvider(account);
  return (
    (pool === "claude" ? provider?.name : provider?.providerID) ||
    providerKey(account) ||
    "Sub2api"
  );
}
/// The proxy an account's requests go through: its own, which for a Sub2api
/// card is its provider's, else the default account proxy; blank for the
/// system settings, "direct://" for none.
function accountProxy(account) {
  let proxy = (account.proxyUrl || "").trim();
  if (account.sourceTag !== "SUB2API")
    proxy ||= (preferences?.accountProxyUrl || "").trim();
  return proxy;
}
function accountUsesExplicitProxy(account) {
  const proxy = accountProxy(account);
  return !!proxy && proxy !== "direct://";
}
function proxyLabel(proxy) {
  if (!proxy) return tr("accounts.proxy.system");
  return proxy === "direct://" ? tr("accounts.proxy.none") : proxy;
}
/// Proxy and removal, in the card's top row as in the SwiftUI card. A
/// Sub2api provider's group has its provider's proxy and removes all its
/// cards at once; its accounts have neither of their own.
function accountManagement(account, group = null, title = null) {
  const actions = el("span", "block-aside");
  const cards = group ? group.accounts : [account];
  const name = group ? title : account.label;
  const proxyButton = button("", "network", "icon-btn card-proxy", () =>
    group ? openProviderProxy(group, title) : openAccountProxy(account),
  );
  proxyButton.classList.toggle("proxy-muted", !accountUsesExplicitProxy(account));
  proxyButton.title = proxyLabel(accountProxy(account));
  proxyButton.setAttribute(
    "aria-label",
    `${tr("accounts.proxy.title")} ${group ? title : account.email || account.label}`,
  );
  const remove = button("", "trash", "icon-btn destructive", async () => {
    if (
      await confirmAction(
        tr("common.remove"),
        extra(
          `Remove ${name} from the account pool?`,
          `从账号池移除 ${name}？`,
        ),
        tr("common.remove"),
        true,
      )
    )
      await action(
        () => accountsController.removeAccounts(cards.map((a) => a.id)),
        tr(
          account.sourceTag === "SUB2API"
            ? "accounts.notice.sub2api_removed"
            : "accounts.notice.account_deleted",
        ),
      );
  });
  remove.title = tr("common.remove");
  remove.setAttribute("aria-label", `${tr("common.remove")} ${name}`);
  // An account's token renewal; a provider renews its own accounts.
  if (account.renewal && !group) actions.append(renewalButton(account));
  actions.append(proxyButton, remove);
  return actions;
}
const RENEWALS = ["never", "once", "always"];
/// A Claude or Codex account's token renewal; shown, as the proxy button,
/// only on the focused card.
function renewalButton(account) {
  const trigger = button("", "renewal", "icon-btn card-renewal", () => {
    if (openMenu?.trigger === trigger) {
      closeMenu(true);
      return;
    }
    showMenu(
      trigger,
      RENEWALS.map((choice) => ({
        label: tr(`accounts.renewal.${choice}`),
        selected: account.renewal === choice,
        run: () => {
          if (account.renewal !== choice)
            action(() => accountsController.setRenewal(account.id, choice));
        },
      })),
    );
  });
  // Its tooltip tells how long the access token has left; a token problem
  // shows as the card's usage error, as in the SwiftUI card.
  trigger.title = tokenLabel(account) || tr("accounts.renewal.title");
  trigger.setAttribute(
    "aria-label",
    `${tr("accounts.renewal.title")} ${account.email || account.label}: ${trigger.title}`,
  );
  trigger.setAttribute("aria-haspopup", "menu");
  trigger.setAttribute("aria-expanded", "false");
  return trigger;
}
/// Usage warning, refresh and switch, at the card's bottom right as in the
/// SwiftUI card, clear of long account names. A provider's group carries
/// its usage warnings, refresh and switch together.
function accountControls(account, full, view, grouped = false) {
  const actions = el("span", "block-aside");
  const reauthenticate = P.needsReauthentication(
    account,
    tr("error.accounts.sign_in_expired"),
  );
  const refreshing =
    accountsController.refreshingIds.has(account.id) ||
    accountsController.remoteRefreshingIds.has(account.id);
  // The usage error is hidden while the card refreshes, as in AccountCardView.
  if (!grouped && !collapsed && account.usageError && !refreshing) {
    const warning = button("", "warning", "icon-btn error-indicator", () =>
      notice(account.usageError, true),
    );
    warning.title = account.usageError;
    warning.setAttribute("aria-label", account.usageError);
    actions.append(warning);
  }
  // An expired sign-in replaces refresh and switch with "Sign in again",
  // which pins the workspace only for a named team, as in the SwiftUI card.
  if (reauthenticate) {
    const signInAgain = button(
      collapsed ? "" : tr("accounts.card.sign_in_again"),
      "reauthenticate",
      collapsed ? "icon-btn card-reauth" : "btn quiet card-reauth",
      () => signIn(view.workspaceTag ? account.accountId : null, "reauthenticate"),
    );
    signInAgain.title = tr("accounts.card.sign_in_again");
    signInAgain.setAttribute("aria-label", `${signInAgain.title} ${full}`);
    signInAgain.disabled = accountsController.adding || !!accountsController.authorizing;
    actions.append(signInAgain);
    return actions;
  }
  // A Claude card, a Sub2api provider's too, says hi to start its 5h window
  // while that window has yet to start.
  if (pool === "claude" && !collapsed && fiveHourIdle(account))
    actions.append(sayHiButton(account, full));
  if (grouped) return actions;
  const refresh = button("", "refresh", "icon-btn card-refresh", () =>
    refreshUsage(account.id),
  );
  refresh.setAttribute("aria-label", `${tr("common.refresh_usage")} ${full}`);
  refreshTooltip(refresh, tr("common.refresh_usage"), () => account.nextRefreshAt);
  refresh.classList.toggle("spinning", refreshing);
  actions.append(refresh);
  if (!account.isCurrent) {
    const control = button("", "switch", "icon-btn card-switch", () =>
      switchAccount(account),
    );
    control.dataset.id = account.id;
    control.dataset.available = String(account.available);
    control.title = tr("accounts.card.switch_to_this");
    control.setAttribute("aria-label", `${control.title} ${full}`);
    actions.append(control);
  }
  return actions;
}
/// Whether the account's 5h window has yet to start: its usage, once known,
/// shows no window, or one without a reset ahead.
function fiveHourIdle(account) {
  if (!account.usage) return false;
  return !(account.usage.fiveHour?.resetAt > Date.now() / 1000);
}
/// Says hi with the account, which starts its 5h window; shown, as the
/// proxy button, only on the focused card.
function sayHiButton(account, full) {
  const control = button("", "say-hi", "icon-btn card-say-hi", () =>
    accountsController
      .sayHi(account.id)
      .then((done) => done && notice(tr("accounts.notice.said_hi")))
      .catch((error) => notice(error, true))
      .finally(() => refreshView().catch((error) => notice(error, true))),
  );
  control.title = tr("accounts.card.say_hi.help");
  control.setAttribute("aria-label", `${tr("accounts.card.say_hi")} ${full}`);
  control.disabled =
    accountsController.refreshingIds.has(account.id) ||
    accountsController.remoteRefreshingIds.has(account.id);
  return control;
}
/// The plan tag. Codex treats an unknown plan as Team; Claude shows none
/// until its plan is known.
function planTag(account, view) {
  const known = P.clean(account.usage?.planType) || P.clean(account.planType);
  return pool === "claude" && !known ? "" : el("span", "tag plan", view.plan);
}
function accountTags(account, view, grouped = false) {
  const tags = el("span", "account-tags");
  tags.append(planTag(account, view));
  // A provider's group carries its SUB2API and CURRENT tags once.
  if (view.workspaceTag && !grouped)
    tags.append(el("span", "tag muted", view.workspaceTag));
  if (account.isCurrent && !grouped)
    tags.append(el("span", "tag good", tr("accounts.card.current")));
  // Imported but not yet requested upstream: refreshing the card sends the
  // first request, after its proxy is set.
  if (account.awaitingFirstRequest) {
    const waiting = el("span", "tag muted", tr("accounts.card.awaiting_first_request"));
    waiting.title = tr("accounts.card.awaiting_first_request.help");
    tags.append(waiting);
  }
  if (view.status === "deactivated")
    tags.append(el("span", "tag bad", tr("accounts.card.status.deactivated")));
  return tags;
}
/// The access token's remaining time, shown by the renewal button.
function tokenLabel(account) {
  const expiry = P.tokenExpiry(
    account.tokenExpiresAt,
    Date.now() / 1000,
    locale,
  );
  if (!expiry) return null;
  return expiry.expired
    ? tr("accounts.card.token_expired")
    : tr("accounts.card.token_expires_format", expiry.remaining);
}
/// The earliest automatic refresh of these cards (Unix seconds), or null.
function nextRefreshAt(accounts) {
  const times = accounts
    .map((account) => account.nextRefreshAt)
    .filter(Number.isFinite);
  return times.length ? Math.min(...times) : null;
}
/// A refresh button's tooltip, followed by how long until the automatic
/// refresh at `nextAt()`; worked out again as the pointer enters.
function refreshTooltip(target, base, nextAt) {
  const update = () => {
    const next = P.refreshCountdown(nextAt(), Date.now() / 1000, locale);
    target.title = !next
      ? base
      : `${base}\n${
          next.due
            ? tr("accounts.refresh.next_due")
            : tr("accounts.refresh.next_format", next.remaining)
        }`;
  };
  update();
  target.onpointerenter = update;
}
/// Refresh and switch for a whole Sub2api provider, which its cards share:
/// switching to any of them selects the provider.
function providerControls(group, title, current) {
  const actions = el("span", "block-aside");
  const ids = group.accounts.map((a) => a.id);
  const refreshing = ids.some(
    (id) =>
      accountsController.refreshingIds.has(id) ||
      accountsController.remoteRefreshingIds.has(id),
  );
  if (!collapsed && !refreshing) {
    const errors = [...new Set(group.accounts.map((a) => a.usageError).filter(Boolean))];
    if (errors.length) {
      const message = errors.join("\n");
      const warning = button("", "warning", "icon-btn error-indicator", () =>
        notice(message, true),
      );
      warning.title = message;
      warning.setAttribute("aria-label", message);
      actions.append(warning);
    }
  }
  const refresh = button("", "refresh", "icon-btn card-refresh", () =>
    refreshProvider(group),
  );
  refresh.setAttribute("aria-label", `${tr("common.refresh_usage")} ${title}`);
  refreshTooltip(refresh, tr("common.refresh_usage"), () =>
    nextRefreshAt(group.accounts),
  );
  refresh.classList.toggle("spinning", refreshing);
  actions.append(refresh);
  if (!current) {
    const target = switchTarget(group);
    const control = button("", "switch", "icon-btn card-switch", () =>
      switchAccount(target),
    );
    control.dataset.id = target.id;
    control.dataset.available = String(
      group.accounts.some((a) => a.available !== false),
    );
    control.title = tr("accounts.card.switch_to_this");
    control.setAttribute("aria-label", `${control.title} ${title}`);
    actions.append(control);
  }
  return actions;
}
/// The card a provider's switch goes through: any active one will do.
function switchTarget(group) {
  return group.accounts.find((a) => a.available !== false) || group.accounts[0];
}
function accountCard(account, grouped = false) {
  const full = account.email || account.accountId || account.label;
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
      `account-row${!grouped && account.isCurrent ? " current running" : ""}`,
    );
    row.classList.toggle("can-reveal", !grouped && interaction.canReveal);
    row.classList.toggle("revealed", !grouped && interaction.visible);
    row.dataset.plan = view.plan;
    const name = el("span", "account-row-name", view.name);
    name.title = full;
    row.append(
      el("span", grouped ? "" : "dot"),
      name,
      planTag(account, view) || el("span", "tag plan placeholder"),
      compactUsage(view.compact[0], "5h"),
      compactUsage(view.compact[1], "1w"),
      accountControls(account, full, view, grouped),
    );
    if (!grouped) row.dataset.reorderHandle = "";
    row.dataset.cardId = account.id;
    return row;
  }
  const card = el(
    grouped ? "div" : "section",
    grouped
      ? "account-member"
      : `block account${account.isCurrent ? " current" : ""}`,
  );
  card.dataset.plan = view.plan;
  card.dataset.cardId = account.id;
  const head = el("div", "block-head");
  // The CURRENT tag alone marks the active account, as in the SwiftUI card.
  const name = el("div", "account-name");
  const title = el("span", "block-title", full);
  title.title = full;
  name.append(title);
  const tags = accountTags(account, view, grouped);
  head.append(tags, ...(grouped ? [] : [accountManagement(account)]));
  // A card is dragged by its tags and name, as OpenQuota by its header.
  if (!grouped) head.dataset.reorderHandle = name.dataset.reorderHandle = "";
  // A negative reset count is unavailable, as in AccountCardPresentation.
  // Sub2api cards keep their credits, which their usage reports.
  const credits =
    view.credits === "unlimited" ? tr("accounts.card.unlimited") : view.credits;
  const count = account.usage?.remainingResetCount;
  const resets = count != null && count >= 0 ? count : null;
  const footer = el("div", "account-footer");
  const footerText = el(
    "span",
    "account-footer-text",
    `${tr("accounts.card.credits_format", credits)} · `,
  );
  const resetCount = el(
    "span",
    "",
    tr(
      resets !== null
        ? "accounts.card.remaining_resets_format"
        : "accounts.card.resets_unavailable_format",
      resets ?? "--",
    ),
  );
  // Its tooltip lists when each remaining reset expires, as in OpenQuota.
  const now = Date.now() / 1000;
  const expiries = (account.usage?.resetExpiries ?? []).filter((at) => at > now);
  if (resets && expiries.length)
    resetCount.title = expiries
      .map((at) =>
        tr("accounts.card.reset_expires_format", P.formatReset(at, locale)),
      )
      .join("\n");
  footerText.append(resetCount);
  footer.append(footerText, accountControls(account, full, view, grouped));
  card.append(
    // A provider's account without a plan tag has no tag row to show.
    ...(grouped && !tags.childElementCount ? [] : [head]),
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
  return card;
}
/// A Sub2api provider's cards close together in one card, which shows once
/// whether the provider is in use, with its controls where a single
/// account's card has them: proxy and removal at the top right, refresh and
/// switch at the bottom right.
function providerGroup(group) {
  const title = providerTitle(group.accounts[0]);
  const source = el("span", "tag muted", P.clean(group.accounts[0].sourceTag) || "SUB2API");
  // A provider's cards are current together.
  const current = group.accounts.some((a) => a.isCurrent);
  const members = group.accounts.map((account) => accountCard(account, true));
  if (collapsed) {
    // No head row: the first account's row carries the provider's dot and
    // its refresh and switch, as a single account's row does.
    const first = members[0];
    const target = switchTarget(group);
    const interaction = P.collapsedInteraction({
      collapsed,
      current,
      switching: switchingId === target.id,
      hovered: false,
    });
    first.classList.toggle("current", current);
    first.classList.toggle("running", current);
    first.classList.toggle("can-reveal", interaction.canReveal);
    first.classList.toggle("revealed", interaction.visible);
    first.firstElementChild.replaceWith(el("span", "dot"));
    first.lastElementChild.replaceWith(providerControls(group, title, current));
    const wrapper = el("div", "account-row-group");
    wrapper.dataset.reorderHandle = "";
    wrapper.append(...members);
    return wrapper;
  }
  const block = el(
    "section",
    `block account account-group${current ? " current" : ""}`,
  );
  const head = el("div", "block-head");
  const name = el("span", "block-title", title);
  name.title = title;
  head.append(name, source);
  if (current) head.append(el("span", "tag good", tr("accounts.card.current")));
  head.append(accountManagement(group.accounts[0], group, title));
  head.dataset.reorderHandle = "";
  const list = el("div", "account-group-members");
  list.append(...members);
  const footer = el("div", "account-footer");
  footer.append(
    el("span", "account-footer-text"),
    providerControls(group, title, current),
  );
  block.append(head, list, footer);
  return block;
}
/// A card's key in the saved order: a Sub2api provider's cards share one.
function entryKey(entry) {
  const key = entry.account ? providerKey(entry.account) : entry.key;
  return key === null ? entry.account.id : `sub2api:${key}`;
}
function entryIsCurrent(entry) {
  return entry.account
    ? entry.account.isCurrent
    : entry.accounts.some((a) => a.isCurrent);
}
function renderAccounts(accounts) {
  if (reorderingAccounts) return;
  const renderState = [
    accounts,
    // Relative times change without new data.
    accounts.map(tokenLabel),
    resetDisplay,
    resetDisplay === "countdown" &&
      accounts.map((account) => {
        const view = P.card(account, false, "used", String);
        return [view.fiveHour, view.oneWeek].map((w) =>
          resetLabel(w, "countdown"),
        );
      }),
    collapsed,
    locale,
    preferences?.usageDisplay,
    accounts.map(accountProxy),
    // Cards group under their provider, which may load after them.
    accounts.map(providerTitle),
    preferences?.accountOrder?.[pool],
    [...accountsController.refreshingIds],
    [...accountsController.remoteRefreshingIds],
    switchingId,
    accountsController.adding,
    accountsController.authorizing,
    pool,
  ];
  if (P.cardsEqual(renderState, renderedAccountsState)) return;
  renderedAccountsState = structuredClone(renderState);
  accounts = P.visibleAccounts(accounts);
  const list = $("accounts");
  list.className = collapsed ? "account-list compact block flush" : "account-list";
  // Switching between full and collapsed, the cards' parts are measured too.
  const before = captureLayout(
    list,
    renderedLayout?.pool === pool && renderedLayout.collapsed !== collapsed,
  );
  list.replaceChildren();
  if (!accounts.length) {
    list.className = "account-list block";
    const empty = el("div", "empty");
    empty.append(
      el("span", "row-name", tr("accounts.empty.title")),
      el(
        "span",
        "setting-description",
        tr(
          pool === "claude"
            ? "accounts.empty.message.no_claude_accounts"
            : "accounts.empty.message.no_accounts",
        ),
      ),
    );
    list.append(empty);
    animateLayout(list, before, null);
    return;
  }
  const entries = P.orderEntries(
    P.providerGroups(accounts, providerKey),
    preferences?.accountOrder?.[pool],
    entryKey,
    entryIsCurrent,
  );
  for (const entry of entries) {
    const node = entry.account ? accountCard(entry.account) : providerGroup(entry);
    node.dataset.reorderId = entryKey(entry);
    // The current card stays first: it is neither dragged nor passed.
    if (entryIsCurrent(entry)) {
      node.dataset.reorderFixed = "";
      for (const handle of [node, ...node.querySelectorAll("[data-reorder-handle]")])
        delete handle.dataset.reorderHandle;
    }
    list.append(node);
  }
  alignMeters(list);
  const current = entries.find(entryIsCurrent);
  animateLayout(list, before, current ? entryKey(current) : null);
}
/// Where each card was before a render: entries by key, every card id and,
/// for a change of mode, each card's shared parts and the list's rect.
function captureLayout(list, withParts = false) {
  const entries = new Map();
  for (const node of list.children)
    if (node.dataset.reorderId)
      entries.set(node.dataset.reorderId, {
        node,
        rect: node.getBoundingClientRect(),
        measure: withParts ? window.CopoolMorph.measure(node) : null,
      });
  const cards = new Set(
    [...list.querySelectorAll("[data-card-id]")].map((n) => n.dataset.cardId),
  );
  return { entries, cards, listRect: list.getBoundingClientRect() };
}
/// Animates what a render changed, with the drag spring: cards slide to
/// their new places (a newly activated one rising over the others), new
/// cards drop in after an import or sign-in and fade in otherwise, and
/// removed ones fade where they were while the cards below close up. A new
/// mode fades in; a new pool slides in on its own.
function animateLayout(list, before, current) {
  const previous = renderedLayout;
  renderedLayout = { pool, collapsed, current };
  if (!previous || previous.pool !== pool) return;
  const R = window.CopoolReorder;
  if (previous.collapsed !== collapsed) {
    morphMode(list, before);
    return;
  }
  const activated = current !== null && previous.current !== current;
  const appear = (node) => {
    const from = dropOrigin?.isConnected && dropOrigin.getBoundingClientRect();
    if (from?.width) {
      // Out of the button, as wide as it, growing as it falls into place
      // over the cards it passes.
      const to = node.getBoundingClientRect();
      const width = Math.min(to.width, Math.max(to.width * 0.1, from.width));
      R.slide(
        node,
        { left: from.left + (from.width - width) / 2, top: from.top, width },
        true,
      );
    } else
      R.springAnimate(node, [
        { opacity: 0, transform: "scale(0.98)" },
        { opacity: 1, transform: "none" },
      ]);
  };
  const kept = new Set();
  for (const node of list.children) {
    const key = node.dataset.reorderId;
    if (!key) continue;
    const old = before.entries.get(key);
    if (!old) {
      appear(node);
      continue;
    }
    kept.add(key);
    // A card still rising or dropping keeps passing over the others.
    R.slide(
      node,
      old.rect,
      (activated && key === current) || old.node.classList.contains("reorder-raised"),
    );
    // An account joining a provider's group arrives inside it.
    for (const member of node.querySelectorAll("[data-card-id]"))
      if (!before.cards.has(member.dataset.cardId)) appear(member);
  }
  const gone = [...before.entries]
    .filter(([key]) => !kept.has(key))
    .map(([, value]) => value);
  if (gone.length) fadeOut(list, gone);
}
/// A layer over the accounts page, clipped to it, for copies of cards that
/// leave it.
function pageLayer(className) {
  const frame = $("accounts-page").getBoundingClientRect();
  const layer = el("div", `page-layer ${className}`);
  // To the panel's bottom, so the layer grows with a panel fitted taller.
  Object.assign(layer.style, {
    left: `${frame.left}px`,
    top: `${frame.top}px`,
    width: `${frame.width}px`,
    bottom: "var(--panel-below)",
  });
  const place = (node, rect) => {
    Object.assign(node.style, {
      left: `${rect.left - frame.left}px`,
      top: `${rect.top - frame.top}px`,
      width: `${rect.width}px`,
      height: `${rect.height}px`,
    });
    node.inert = true;
    layer.append(node);
  };
  return { layer, frame, place };
}
// The cards changing between full and collapsed, shown in place of the list.
let modeMorph = null;
// While cards morph, the panel follows them instead of their final layout.
let holdFit = false;
// Each card starts changing this long after the one above it, the delays
// together never exceeding MORPH_STAGGER_SPAN; collapsing runs quicker than
// expanding.
const MORPH_STAGGER = 8;
const MORPH_STAGGER_SPAN = 30;
const MORPH_DURATION = { collapse: 100, expand: 120 };
/// Between full cards and collapsed rows, with continuity (morph.js): the
/// cards stay one contiguous stack as each changes in turn, their names,
/// plans, usage and controls travel to their places in the other form, and
/// the cards' backgrounds merge into the collapsed list's, or split from it.
function morphMode(list, before) {
  modeMorph?.finish();
  const M = window.CopoolMorph;
  if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
  const { layer, frame } = pageLayer("mode-morph");
  // The list's shared column widths, inherited by every copy.
  for (const name of list.style)
    if (name.startsWith("--"))
      layer.style.setProperty(name, list.style.getPropertyValue(name));
  const items = [];
  for (const node of list.children) {
    const old = before.entries.get(node.dataset.reorderId);
    if (!old?.measure) continue;
    for (const running of old.node.getAnimations()) running.cancel();
    old.node.classList.remove("reorder-raised");
    items.push({
      source: { node: old.node, measure: old.measure },
      target: { node: node.cloneNode(true), measure: M.measure(node) },
    });
  }
  if (!items.length) return;
  // The collapsed list's background, which the cards merge into.
  const sheet = el("div", "morph-sheet");
  const rect = collapsed ? list.getBoundingClientRect() : before.listRect;
  Object.assign(sheet.style, {
    left: `${rect.left - frame.left}px`,
    top: `${rect.top - frame.top}px`,
    width: `${rect.width}px`,
    height: `${rect.height}px`,
  });
  layer.append(sheet);
  document.body.append(layer);
  // The panel's height follows the cards' as they change: the page above
  // the list, the stack's bottom, and the collapsed list's padding below its
  // last row growing in or out with that row.
  const section = $("accounts-page");
  const style = getComputedStyle(section);
  const chrome =
    document.querySelector(".top").offsetHeight +
    parseFloat(style.paddingTop) +
    parseFloat(style.paddingBottom) -
    section.firstElementChild.getBoundingClientRect().top;
  const lastItem = items[items.length - 1];
  const pad = (listRect, itemRect) =>
    Math.max(0, listRect.bottom - (itemRect.top + itemRect.height));
  const padFrom = pad(before.listRect, lastItem.source.measure.rect);
  const padTo = pad(list.getBoundingClientRect(), lastItem.target.measure.rect);
  const follow = (frames) => {
    const { e, rect } = frames[frames.length - 1];
    const bottom = rect.top + rect.height + padFrom + (padTo - padFrom) * e;
    const height = Math.ceil(chrome + bottom);
    if (Math.abs(height - fittedHeight) > 1) {
      fittedHeight = height;
      invoke("fit_panel", { height }).catch(() => {});
    }
  };
  holdFit = true;
  const run = M.run({
    layer,
    frame,
    items,
    sheet,
    collapsing: collapsed,
    duration: MORPH_DURATION[collapsed ? "collapse" : "expand"],
    stagger: Math.min(MORPH_STAGGER, MORPH_STAGGER_SPAN / Math.max(1, items.length - 1)),
    radius: collapsed ? { source: 8, target: 0 } : { source: 0, target: 8 },
    onFrame: follow,
  });
  list.style.opacity = "0";
  let done = false;
  const finish = () => {
    if (done) return;
    done = true;
    run.stop();
    if (modeMorph === morph) modeMorph = null;
    list.style.removeProperty("opacity");
    layer.remove();
    if (holdFit) {
      holdFit = false;
      queueFit();
    }
  };
  const morph = { finish };
  modeMorph = morph;
  run.done.then(finish);
}
/// Fades removed cards out where they were.
function fadeOut(list, gone) {
  const R = window.CopoolReorder;
  const { layer, place } = pageLayer("card-exit");
  for (const { node, rect } of gone) {
    for (const running of node.getAnimations()) running.cancel();
    node.classList.remove("reorder-raised");
    // The list's shared column widths, which the card leaves behind.
    for (const name of list.style)
      if (name.startsWith("--"))
        node.style.setProperty(name, list.style.getPropertyValue(name));
    place(node, rect);
  }
  document.body.append(layer);
  const animations = [...layer.children].map((node) =>
    R.springAnimate(node, [
      { opacity: 1, transform: "none" },
      { opacity: 0, transform: "scale(0.98)" },
    ]),
  );
  if (!animations[0]) return layer.remove();
  Promise.allSettled(animations.map((a) => a.finished)).then(() => layer.remove());
}
/// Dragging the cards, as in OpenQuota; the order is kept per pool.
window.CopoolReorder.attach($("accounts"), {
  onStart: () => (reorderingAccounts = true),
  onEnd: ({ order, moved, cancelled }) => {
    reorderingAccounts = false;
    if (moved && !cancelled) saveAccountOrder(order);
    renderedAccountsState = undefined;
    renderAccounts(accountsCache);
  },
});
function saveAccountOrder(order) {
  const accountOrder = { ...preferences.accountOrder, [pool]: order };
  preferences = { ...preferences, accountOrder };
  queuePreferenceSave({ accountOrder }).catch(async (error) => {
    notice(error, true);
    preferences = await invoke("preferences").catch(() => preferences);
    renderedAccountsState = undefined;
    renderAccounts(accountsCache);
  });
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
  // The proxy's own routable candidates, as Swift's availableAccounts: not
  // every listed card (a free or cooling-down account is left out).
  $("proxy-count").textContent = tr(
    "proxy.available_accounts_format",
    String(proxy.availableAccounts ?? 0),
  );
  $("proxy-url").value =
    proxy.baseUrl || tr("proxy.value.generated_after_start");
  $("proxy-key").value = proxy.apiKey || "";
  $("proxy-key").placeholder = tr("proxy.value.generated_after_first_start");
  // Only real values copy, as ProxyCopyableValueCard's canCopy.
  document.querySelector('[data-copy="proxy-url"]').disabled = !proxy.baseUrl;
  document.querySelector('[data-copy="proxy-key"]').disabled = !proxy.apiKey;
  $("proxy-active").textContent =
    proxy.activeAccountLabel || tr("proxy.info.no_request_matched");
  $("proxy-active-id").textContent =
    proxy.activeAccountId || tr("proxy.info.active_account_hint");
  $("proxy-error").textContent = proxy.lastError || tr("common.none");
}
async function refreshView() {
  const controller = accountsController;
  const [accounts, proxy, fullRefresh] = await Promise.all([
    controller.load(),
    invoke("proxy_status"),
    invoke("next_full_refresh_at", { claude: pool === "claude" }),
    refreshFeatureState(),
  ]);
  proxyCache = proxy;
  // A pool switched during the load renders through its own refresh.
  if (controller === accountsController) {
    fullRefreshAt = fullRefresh;
    accountsCache = accounts;
    renderAccounts(accounts);
    await renderPendingWorkspaces();
  }
  proxyController.applyLocal(
    proxy,
    configurationCache.proxyConfiguration,
    proxyController.tunnelStatus,
  );
  syncButtons();
}
function syncGeneralSelectLabels() {
  for (const id of ["usage-display", "language"]) {
    $(`${id}-label`).textContent = $(id).selectedOptions[0]?.textContent || "";
  }
}
function renderPreferences() {
  document
    .querySelectorAll("[data-preference]")
    .forEach(
      (input) =>
        (input.checked = Boolean(preferences[input.dataset.preference])),
    );
  if (document.activeElement !== $("account-proxy"))
    $("account-proxy").value = preferences.accountProxyUrl || "";
  const controls = P.settingsControls(preferences, installedEditors);
  $("usage-display").value = controls.usagePicker.selectedValue;
  syncGeneralSelectLabels();
  $("restart-target").disabled = !controls.restartPicker.isEnabled;
  $("restart-target").value = controls.restartPicker.selectedValue || "";
  // An edit still waiting to be saved is not overwritten.
  if (document.activeElement !== $("proxy-port"))
    $("proxy-port").value = preferences.proxyPort;
}
let preferenceSave = Promise.resolve();
/// Every save of the preferences runs in turn, each over the preferences the
/// previous one stored: each writes all of them.
function queuePreferenceSave(patch) {
  const save = preferenceSave.then(async () => {
    preferences = await providerController.savePreferences(preferences, patch);
  });
  preferenceSave = save.catch(() => {});
  return save;
}
async function savePreference(patch) {
  if (
    patch.restartEditorsOnSwitch &&
    !preferences.restartEditorTargets.length &&
    installedEditors.length
  )
    patch.restartEditorTargets = [installedEditors[0].id];
  const updatedKey =
    "restartEditorTargets" in patch && !("restartEditorsOnSwitch" in patch)
      ? "settings.notice.restart_target_updated"
      : "settings.notice.updated";
  const result = await action(async () => {
    await queuePreferenceSave(patch);
    locale = appLocale();
    translate();
    renderPreferences();
    renderProviders();
    renderClaudeProviders();
    renderRemotes();
    renderTunnelControls();
  }, () => tr(updatedKey));
  if (!result) renderPreferences();
}
mountIcons();
locale = appLocale();
translate();
$("notice-close").onclick = () => {
  $("notice").hidden = true;
};
// The pages' order, left to right, for the way they slide.
const PAGES = ["accounts", "proxy", "settings"];
// The latest page turn in each container, which tidies up when it ends.
const pageTurns = new Map();
/// Shows `to` in place of `from` like a navigation stack: `to` slides in
/// from the side `direction` points to (1 right, −1 left) while `from`, held
/// over their container until it is gone, slides out the other way. A turn
/// caught midway continues from where its pages are.
function turnPage(from, to, direction, animated = true) {
  if (from === to) return;
  const container = to.parentElement;
  const turn = {};
  pageTurns.set(container, turn);
  const place = from.classList.contains("page-leaving")
    ? null
    : { top: from.offsetTop, left: from.offsetLeft, width: from.offsetWidth };
  if (place)
    Object.assign(from.style, {
      top: `${place.top}px`,
      left: `${place.left}px`,
      width: `${place.width}px`,
    });
  from.classList.add("page-leaving");
  to.classList.remove("page-leaving");
  to.style.removeProperty("top");
  to.style.removeProperty("left");
  to.style.removeProperty("width");
  to.hidden = false;
  const end = () => {
    if (pageTurns.get(container) !== turn) return;
    pageTurns.delete(container);
    container.classList.remove("paging");
    for (const node of container.querySelectorAll(":scope>.page-leaving")) {
      for (const running of node.getAnimations())
        if (running.id === "page") running.cancel();
      node.classList.remove("page-leaving");
      node.style.removeProperty("top");
      node.style.removeProperty("left");
      node.style.removeProperty("width");
      node.hidden = true;
    }
  };
  if (!animated) return end();
  const width = container.clientWidth;
  const shown = { transform: "none", opacity: 1 };
  const aside = (side) => ({
    transform: `translateX(${side * width}px)`,
    opacity: 0,
  });
  const slide = (node, start, finish) => {
    const running = node.getAnimations().filter((a) => a.id === "page");
    if (running.length) {
      const now = getComputedStyle(node);
      start = { transform: now.transform, opacity: Number(now.opacity) };
      for (const animation of running) animation.cancel();
    }
    const animation = window.CopoolReorder.springAnimate(node, [start, finish]);
    if (animation) animation.id = "page";
    return animation;
  };
  container.classList.add("paging");
  const animations = [
    slide(from, shown, aside(-direction)),
    slide(to, aside(direction), shown),
  ];
  if (!animations[0]) return end();
  Promise.allSettled(animations.map((a) => a?.finished)).then(end);
}
function showPage(page, animated = true) {
  const previous = selectedPage;
  selectedPage = page;
  // Entering the tab refreshes the local status; remote servers load once.
  if (selectedPage === "proxy")
    proxyController
      .refresh()
      .then(() => proxyController.loadRemoteStatuses())
      .catch((error) => notice(error, true));
  closeMenu();
  // Into a subpage the new page comes from the right; back, from the left.
  const direction = PAGES.indexOf(page) > PAGES.indexOf(previous) ? 1 : -1;
  const row = (name) => $(name === "accounts" ? "top-main" : "top-sub");
  turnPage($(`${previous}-page`), $(`${page}-page`), direction, animated);
  turnPage(row(previous), row(page), direction, animated);
  document
    .querySelectorAll("[data-page]")
    .forEach((link) =>
      link.setAttribute(
        "aria-selected",
        String(link.dataset.page === selectedPage),
      ),
    );
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
function setDisclosure(button, opened) {
  button.setAttribute("aria-expanded", String(opened));
  button.replaceChildren(icon(opened ? "chevron-up" : "chevron-down"));
  $(button.dataset.target).hidden = !opened;
  requestAnimationFrame(applyNativeIcons);
}
function openDisclosure(target) {
  const button = document.querySelector(`.disclosure[data-target="${target}"]`);
  if (button && button.getAttribute("aria-expanded") !== "true")
    setDisclosure(button, true);
}
document.querySelectorAll(".disclosure").forEach((button) =>
  button.addEventListener("click", () =>
    setDisclosure(button, button.getAttribute("aria-expanded") !== "true"),
  ),
);
/// Shows the Codex or the Claude pool; each keeps its own controller state.
// The pool buttons' order, left to right, for the way the cards slide.
const POOLS = ["codex", "claude"];
let poolExit = null;
/// The pool's cards: the pending workspaces and the account list.
const poolBlocks = () => [$("pending-workspaces"), $("accounts")];
/// Slides copies of the shown pool's cards out toward `direction` (−1 left,
/// 1 right), clipped to the page, while the real ones are replaced.
function slidePoolOut(direction) {
  poolExit?.remove();
  poolExit = null;
  const { layer, frame, place } = pageLayer("pool-exit");
  for (const block of poolBlocks()) {
    if (!block.childElementCount) continue;
    const rect = block.getBoundingClientRect();
    const copy = block.cloneNode(true);
    for (const node of [copy, ...copy.querySelectorAll("[id]")])
      node.removeAttribute("id");
    place(copy, rect);
  }
  if (!layer.childElementCount) return;
  document.body.append(layer);
  const animations = [...layer.children].map((copy) =>
    window.CopoolReorder.springAnimate(copy, [
      { transform: "none", opacity: 1 },
      { transform: `translateX(${direction * frame.width}px)`, opacity: 0 },
    ]),
  );
  if (!animations[0]) return layer.remove();
  poolExit = layer;
  Promise.allSettled(animations.map((a) => a.finished)).then(() => {
    layer.remove();
    if (poolExit === layer) poolExit = null;
  });
}
/// Slides the new pool's cards in from the other side.
function slidePoolIn(direction) {
  const width = $("accounts-page").clientWidth;
  for (const block of poolBlocks()) {
    for (const running of block.getAnimations())
      if (running.id === "pool") running.cancel();
    const animation = window.CopoolReorder.springAnimate(block, [
      { transform: `translateX(${-direction * width}px)`, opacity: 0 },
      { transform: "none", opacity: 1 },
    ]);
    if (animation) animation.id = "pool";
  }
}
function showPool(next) {
  if (next !== "codex" && next !== "claude") return;
  document
    .querySelectorAll(".pool-btn")
    .forEach((button) =>
      button.setAttribute("aria-selected", String(button.dataset.pool === next)),
    );
  if (next === pool) return;
  // To the pool on the right, the cards leave to the left; and back.
  const direction = POOLS.indexOf(next) > POOLS.indexOf(pool) ? -1 : 1;
  slidePoolOut(direction);
  pool = next;
  fullRefreshAt = null;
  localStorage.setItem("copool.pool", pool);
  closeMenu();
  accountsController = poolControllers[pool];
  accountsCache = accountsController.accounts;
  renderAccounts(accountsCache);
  renderPendingWorkspaces();
  slidePoolIn(direction);
  syncButtons();
  refreshView().catch((error) => notice(error, true));
}
document
  .querySelectorAll(".pool-btn")
  .forEach((button) => (button.onclick = () => showPool(button.dataset.pool)));
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
  // Choices show a check mark on the current one, as a native menu.
  const choices = items.some((item) => "selected" in item);
  menu.classList.toggle("choices", choices);
  for (const item of items) {
    const option = el("button", "", item.label);
    if (item.icon) option.prepend(item.icon);
    option.type = "button";
    option.setAttribute("role", choices ? "menuitemradio" : "menuitem");
    if (choices) option.setAttribute("aria-checked", item.selected ? "true" : "false");
    option.onclick = () => {
      closeMenu(true);
      item.run();
    };
    menu.append(option);
  }
  document.body.append(menu);
  // Below the trigger, or above it when the panel is too short; kept inside.
  const bounds = trigger.getBoundingClientRect();
  const panel = document.querySelector(".panel").getBoundingClientRect();
  const left = Math.min(bounds.left, innerWidth - menu.offsetWidth - 8);
  const below = bounds.bottom + 4;
  const top =
    below + menu.offsetHeight <= panel.bottom - 8
      ? below
      : Math.max(panel.top + 8, bounds.top - menu.offsetHeight - 4);
  menu.style.left = `${Math.max(8, left)}px`;
  menu.style.top = `${top}px`;
  menu.style.maxHeight = `${panel.bottom - top - 8}px`;
  trigger.setAttribute("aria-expanded", "true");
  openMenu = { menu, trigger };
  (
    menu.querySelector("[aria-checked=true]") || menu.querySelector("button")
  )?.focus();
}
document.addEventListener("pointerdown", (event) => {
  if (
    openMenu &&
    !openMenu.menu.contains(event.target) &&
    !openMenu.trigger.contains(event.target)
  )
    closeMenu();
});
// On Linux a <select>'s popup is a GTK menu outside the panel's frosted
// material; the in-panel menu replaces it there.
if (document.documentElement.classList.contains("linux")) {
  const showSelectMenu = (select) => {
    if (openMenu?.trigger === select) {
      closeMenu(true);
      return;
    }
    showMenu(
      select,
      [...select.options].map((option, index) => ({
        label: option.textContent,
        selected: option.selected,
        run: () => {
          if (select.selectedIndex === index) return;
          select.selectedIndex = index;
          select.dispatchEvent(new Event("change"));
        },
      })),
    );
  };
  document.addEventListener("mousedown", (event) => {
    const select = event.target.closest?.("select");
    if (!select || select.disabled || event.button !== 0) return;
    event.preventDefault();
    showSelectMenu(select);
  });
  document.addEventListener(
    "keydown",
    (event) => {
      const select = event.target.closest?.("select");
      if (!select || select.disabled || openMenu) return;
      const opens =
        event.key === " " ||
        event.key === "Enter" ||
        (event.altKey && ["ArrowDown", "ArrowUp"].includes(event.key));
      if (!opens) return;
      event.preventDefault();
      showSelectMenu(select);
    },
    true,
  );
}
// WebKitGTK and WebView2 draw a `title` as the desktop's own tooltip, in its
// theme rather than the panel's: an in-panel one in the menus' look replaces
// it there. While the pointer is on an element, its title and every
// ancestor's move to `data-tooltip`, so that no native one shows, not even an
// ancestor's in its place; they come back as it leaves.
if (!document.documentElement.classList.contains("macos")) {
  const tip = el("div", "tooltip");
  tip.setAttribute("aria-hidden", "true");
  document.body.append(tip);
  let owner = null;
  let held = [];
  let timer;
  const hold = (node) => {
    for (; node instanceof Element; node = node.parentElement) {
      const text = node.getAttribute("title");
      if (text === null) continue;
      node.dataset.tooltip = text;
      node.removeAttribute("title");
      if (!held.includes(node)) held.push(node);
    }
  };
  // A title set while the pointer is on its owner, as a refresh button's is.
  new MutationObserver(() => {
    if (owner) hold(owner);
  }).observe(document.body, {
    subtree: true,
    attributes: true,
    attributeFilter: ["title"],
  });
  const hide = () => {
    clearTimeout(timer);
    tip.classList.remove("shown");
    for (const node of held) {
      if (!node.hasAttribute("title")) node.title = node.dataset.tooltip;
      delete node.dataset.tooltip;
    }
    owner = null;
    held = [];
  };
  // Below its owner, or above it near the panel's bottom; kept inside.
  const show = () => {
    if (!owner?.isConnected || !owner.dataset.tooltip) return hide();
    tip.textContent = owner.dataset.tooltip;
    tip.classList.add("shown");
    const bounds = owner.getBoundingClientRect();
    const panel = document.querySelector(".panel").getBoundingClientRect();
    const center = bounds.left + bounds.width / 2 - tip.offsetWidth / 2;
    const left = Math.min(center, panel.right - tip.offsetWidth - 6);
    const below = bounds.bottom + 4;
    const top =
      below + tip.offsetHeight <= panel.bottom - 6
        ? below
        : bounds.top - tip.offsetHeight - 4;
    tip.style.left = `${Math.max(panel.left + 6, left)}px`;
    tip.style.top = `${Math.max(panel.top + 6, top)}px`;
  };
  document.addEventListener("pointerover", (event) => {
    const node = event.target.closest?.("[title], [data-tooltip]");
    if (node === owner) return;
    hide();
    if (!node || !node.getAttribute("title")) return;
    owner = node;
    hold(node);
    timer = setTimeout(show, 600);
  });
  document.addEventListener("pointerout", (event) => {
    if (!event.relatedTarget) hide();
  });
  for (const type of ["pointerdown", "keydown", "wheel", "blur"])
    window.addEventListener(type, hide, true);
}
$("import-open").onclick = () => {
  if (openMenu?.trigger === $("import-open")) {
    closeMenu(true);
    return;
  }
  if (pool === "claude") {
    showMenu($("import-open"), [
      {
        label: tr("accounts.action.import_current_claude"),
        run: () =>
          importAccounts(
            () => accountsController.importCurrent(),
            "accounts.notice.imported_format",
          ),
      },
      {
        label: tr("accounts.action.import_auth_file"),
        run: () => importAccountFile(),
      },
      {
        label: tr("accounts.action.import_sub2api"),
        run: () => syncClaudeSub2api(),
      },
    ]);
    return;
  }
  showMenu($("import-open"), [
    {
      label: tr("accounts.action.import_current_auth"),
      run: () =>
        importAccounts(
          () => accountsController.importCurrent(),
          "accounts.notice.imported_format",
        ),
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
  } else if (mod && !event.altKey && !event.shiftKey && ["1", "2"].includes(event.key)) {
    event.preventDefault();
    if (selectedPage !== "accounts") showPage("accounts");
    showPool(event.key === "1" ? "codex" : "claude");
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
  $("edit-name").textContent =
    account.email || account.accountId || account.label;
  const direct = account.proxyUrl === "direct://";
  $("edit-proxy-enabled").checked = !direct;
  $("edit-proxy").disabled = direct;
  $("edit-proxy").value = direct ? "" : account.proxyUrl || "";
  $("edit-proxy-help").textContent = tr("settings.sub2api.proxy_help");
  $("edit-error").hidden = true;
  $("edit-dialog").showModal();
  $(direct ? "edit-proxy-enabled" : "edit-proxy").focus();
}
/// The provider's proxy, saved through any of its cards.
function openProviderProxy(group, title) {
  const { id, proxyUrl } = group.accounts[0];
  openAccountProxy({ id, label: title, sourceTag: "SUB2API", proxyUrl });
}
$("edit-proxy-enabled").onchange = () => {
  $("edit-proxy").disabled = !$("edit-proxy-enabled").checked;
};
$("edit-cancel").onclick = () => $("edit-dialog").close();
$("edit-form").onsubmit = (event) => {
  event.preventDefault();
  // Only this dialog waits for its save.
  const save = event.submitter || $("edit-form").querySelector("[type=submit]");
  if (save?.disabled) return;
  if (save) save.disabled = true;
  action(async () => {
    try {
      const value = $("edit-proxy-enabled").checked
        ? $("edit-proxy").value
        : "direct://";
      await accountsController.saveProxy(editingAccount.id, value);
      $("edit-dialog").close();
    } catch (error) {
      $("edit-error").textContent = String(error);
      $("edit-error").hidden = false;
      throw error;
    } finally {
      if (save) save.disabled = false;
    }
  });
};
/// Refreshes one account, or all without an id. As in the Swift app this
/// takes no panel-wide lock: each card updates and becomes operable again as
/// its own refresh lands ("copool-changed"), while the others stay usable.
function refreshUsage(id = null) {
  accountsController
    .refresh(id)
    .then((done) => {
      // As Swift: only a manual refresh of every account reports success.
      if (done && id === null) notice(tr("accounts.notice.usage_refreshed"));
    })
    .catch((error) => notice(error, true))
    .finally(() => refreshView().catch((error) => notice(error, true)));
}
/// Refreshes every card of one Sub2api provider.
function refreshProvider(group) {
  accountsController
    .refreshProvider(
      group.key,
      group.accounts.map((a) => a.id),
    )
    .catch((error) => notice(error, true))
    .finally(() => refreshView().catch((error) => notice(error, true)));
}
$("refresh-all").onclick = () => refreshUsage();
/// Swift's buildSwitchNotice: the switch, then what followed it.
function switchNotice(execution) {
  let error = false;
  const segments = [
    execution.provider
      ? tr(
          pool === "claude"
            ? "accounts.notice.claude_provider_switched_format"
            : "accounts.notice.provider_switched_format",
          execution.provider,
        )
      : tr(
          pool === "claude"
            ? "accounts.notice.claude_switch_done"
            : "accounts.notice.switch_done",
        ),
  ];
  // settings.json keys that keep Claude Code off the account login.
  if (execution.loginOverrides?.length) {
    error = true;
    segments.push(
      tr(
        "accounts.notice.claude_settings_override_format",
        execution.loginOverrides.join(", "),
      ),
    );
  }
  if (execution.opencodeSyncError) {
    error = true;
    segments.push(
      tr("accounts.notice.sync_failed_format", execution.opencodeSyncError),
    );
  } else if (execution.opencodeSynced)
    segments.push(tr("accounts.notice.sync_done"));
  if (execution.editorRestartError) {
    error = true;
    segments.push(
      tr(
        "accounts.notice.editor_restart_failed_format",
        execution.editorRestartError,
      ),
    );
  } else if (execution.restartedEditors.length)
    segments.push(
      tr(
        "accounts.notice.editor_restarted_format",
        execution.restartedEditors.join(" / "),
      ),
    );
  if (execution.launchError) {
    error = true;
    segments.push(execution.launchError);
  }
  return { text: segments.join(" · "), error };
}
/// A switch locks only its own card's switch button and Smart Switch.
async function switchAccount(account) {
  if (switchingId !== null) return;
  switchingId = account.id;
  renderAccounts(accountsCache);
  syncButtons();
  try {
    const result = switchNotice(
      await invoke(poolCommand("switch_account"), { id: account.id }),
    );
    notice(result.text, result.error);
  } catch (error) {
    notice(error, true);
  } finally {
    switchingId = null;
    await refreshView().catch((error) => notice(error, true));
  }
}
$("smart-switch").onclick = async () => {
  if (switchingId !== null) return;
  switchingId = "";
  syncButtons();
  try {
    const outcome = await invoke(poolCommand("smart_switch"));
    if (outcome.kind === "noTarget")
      notice(tr("accounts.notice.no_switch_target"));
    else if (outcome.kind === "alreadyBest")
      notice(tr("accounts.notice.already_best"));
    else {
      const result = switchNotice(outcome.execution);
      notice(
        result.error
          ? result.text
          : tr(
              "accounts.notice.smart_switched_prefix_format",
              outcome.label,
              result.text,
            ),
        result.error,
      );
    }
  } catch (error) {
    notice(error, true);
  } finally {
    switchingId = null;
    await refreshView().catch((error) => notice(error, true));
  }
};
// The API proxy's controls wait on its loading state only, as in Swift.
$("proxy-start").onclick = () =>
  action(() => proxyController.startProxy(Number($("proxy-port").value)));
$("proxy-stop").onclick = () => action(() => proxyController.stopProxy());
$("proxy-refresh").onclick = () =>
  action(() => proxyController.refreshStatus(), null, false);
$("proxy-key-refresh").onclick = () =>
  action(() => proxyController.rotateKey(), null, false);
// The preferred port is saved as it is edited, as updatePreferredPortText.
let portSaveTimer;
$("proxy-port").oninput = () => {
  clearTimeout(portSaveTimer);
  const port = Number($("proxy-port").value);
  if (!Number.isInteger(port) || port < 1 || port > 65535) return;
  portSaveTimer = setTimeout(() => {
    queuePreferenceSave({ proxyPort: port }).catch((error) =>
      notice(error, true),
    );
  }, 350);
};
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
// Support horizontal trackpad gestures and Shift+wheel without trapping page scroll.
$("account-proxy").addEventListener("wheel", (event) => {
  const input = event.currentTarget;
  const delta = event.deltaX || (event.shiftKey ? event.deltaY : 0);
  if (!delta) return;
  const scale = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? input.clientWidth : 1;
  const previous = input.scrollLeft;
  input.scrollLeft += delta * scale;
  if (input.scrollLeft !== previous) event.preventDefault();
}, { passive: false });
$("account-proxy").onchange = () =>
  savePreference({ accountProxyUrl: $("account-proxy").value.trim() });
$("usage-display").onchange = () => {
  syncGeneralSelectLabels();
  savePreference({ usageDisplay: $("usage-display").value });
};
$("language").onchange = () => {
  syncGeneralSelectLabels();
  savePreference({ locale: $("language").value });
};
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
      showPool(pool);
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
  if (!document.hidden && !document.querySelector("dialog[open]"))
    refreshView().catch((error) => notice(error, true));
}, 15000);

// The panel follows its content height, like a menu; the native side clamps
// it to the screen and keeps it under the status bar icon.
let fitFrame = 0;
let fittedHeight = 0;
function queueFit() {
  if (fitFrame || holdFit) return;
  fitFrame = requestAnimationFrame(() => {
    fitFrame = 0;
    const dialog = document.querySelector("dialog[open]");
    document.documentElement.classList.toggle("modal", Boolean(dialog));
    if (holdFit) return;
    const section = document.querySelector(
      "main>section:not([hidden], .page-leaving)",
    );
    const page = section?.firstElementChild;
    if (!page) return;
    const style = getComputedStyle(section);
    let height =
      document.querySelector(".top").offsetHeight +
      page.offsetHeight +
      parseFloat(style.paddingTop) +
      parseFloat(style.paddingBottom);
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
      showPage("accounts", false);
    panelHiddenAt = null;
    queueFit();
    if (!document.querySelector("dialog[open]"))
      refreshView().catch((error) => notice(error, true));
  })
  .catch((error) => notice(error, true));

// Where the panel lies in its window on Linux, which keeps one height.
window.__TAURI__.event
  .listen("panel-frame", ({ payload }) => {
    const root = document.documentElement.style;
    root.setProperty("--panel-top", `${payload.top}px`);
    root.setProperty("--panel-height", `${payload.height}px`);
  })
  .catch((error) => notice(error, true));

// Linux has no system material behind the panel: the backend sends the screen
// under it, blurred, and where the window lies on it. `image` is null when
// unchanged and empty without a backdrop.
window.__TAURI__.event
  .listen("panel-backdrop", ({ payload }) => {
    const root = document.documentElement;
    root.classList.toggle("composited", payload.composited);
    root.style.setProperty("--backdrop-size", `${payload.width}px ${payload.height}px`);
    root.style.setProperty("--backdrop-position", `${-payload.x}px ${-payload.y}px`);
    if (payload.image === null) return;
    root.style.setProperty("--backdrop-image", payload.image ? `url("${payload.image}")` : "none");
    root.classList.toggle("frosted", Boolean(payload.image));
  })
  .catch((error) => notice(error, true));

let changeRefreshTimer;
window.__TAURI__.event
  .listen("copool-changed", () => {
    clearTimeout(changeRefreshTimer);
    changeRefreshTimer = setTimeout(() => {
      refreshView().catch((error) => notice(error, true));
    }, 50);
  })
  .catch((error) => notice(error, true));
