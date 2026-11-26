"use strict";
const $ = (id) => document.getElementById(id);
const invoke = (command, args = {}) =>
  window.__TAURI__.core.invoke(command, args);
const P = window.CopoolPresentation;
let collapsed = false;
let selectedPage = "accounts";
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
// The account pools, left to right as their buttons, for ⌘1–⌘4 and the way
// the cards slide.
const ACCOUNT_POOLS = ["codex", "claude", "antigravity", "grok"];
// The pools with an API proxy: all but Grok.
const PROXY_POOLS = ["codex", "claude", "antigravity"];
// The pools with Sub2api providers: every account pool.
const PROVIDER_POOLS = ACCOUNT_POOLS;
// The account pool on the accounts page: Codex, Claude Code, Antigravity or
// Grok.
let pool = ACCOUNT_POOLS.includes(localStorage.getItem("copool.pool"))
  ? localStorage.getItem("copool.pool")
  : "codex";
// The pool whose API proxy the proxy page shows: the accounts page's, or
// the last one with a proxy while that is Grok.
let proxyPool = PROXY_POOLS.includes(pool) ? pool : "codex";
/// The preferences of each pool's proxy on the proxy page.
const PROXY_PREFERENCES = {
  codex: { port: "proxyPort", autoStart: "autoStartProxy" },
  antigravity: { port: "antigravityProxyPort", autoStart: "antigravityAutoStartProxy" },
  claude: { port: "claudeProxyPort", autoStart: "claudeAutoStartProxy" },
};
// Reset times as the date and time or as the time left, as OpenQuota toggles
// them by clicking either.
let resetDisplay =
  localStorage.getItem("copool.resetDisplay") === "countdown"
    ? "countdown"
    : "exact";
/// The commands of one pool: every one names the pool it acts on. Only
/// Codex has workspaces.
function poolInvoke(target) {
  return (command, args = {}) => {
    if (target !== "codex" && command === "workspace_directory")
      return Promise.resolve([]);
    if (target !== "codex" && command === "cancel_workspace_refresh")
      return Promise.resolve(null);
    return invoke(command, { ...args, pool: target });
  };
}
const ICONS = {
  folder: '<path d="M2 5h7l2 3h9v12H2Z"/>',
  network:
    '<circle cx="11" cy="11" r="8.7"/><ellipse cx="11" cy="11" rx="4" ry="8.7" transform="rotate(-23 11 11)"/><path d="M3.4 6.8c3.5 3.7 8.8 6.5 16.1 7.2M4 16c4.9-.4 10-3 13.9-8.3"/>',
  plus: '<path d="M11 3v16M3 11h16"/>',
  refresh: '<path d="M18 8a8 8 0 1 0 .4 6M18 3v5h-5"/>',
  "say-hi": '<path d="M3 4h16v11H9l-4 4v-4H3Z"/>',
  "say-hi-sent":
    '<path d="M3 4h16v11H9l-4 4v-4H3Z"/><path d="M7.5 9.5h.01M11 9.5h.01M14.5 9.5h.01" stroke-width="2.4" stroke-linecap="round"/>',
  "say-hi-received": '<path d="M3 4h16v11H9l-4 4v-4H3Z"/><path d="m7.5 9.5 2.5 2.5 4.5-4.5"/>',
  renewal:
    '<path d="M18.6 12.5A7.7 7.7 0 1 1 16.3 5.4M17 2.4v3.8h-3.8"/><path d="M11 7.2V11l2.6 1.8"/>',
  "renewal-done": '<circle cx="11" cy="11" r="8.5"/><path d="m7.3 11.2 2.5 2.5 4.9-5"/>',
  info: '<circle cx="11" cy="11" r="8.5"/><path d="M11 10v5.5M11 6.6v.4"/>',
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
  "reset-notice": '<circle cx="11" cy="11" r="8.5"/><path d="M11 6.5v6M11 15.5v.5"/>',
  play: '<path d="M6.5 3.8v14.4L18 11Z" fill="currentColor"/>',
  stop: '<rect x="4.5" y="4.5" width="13" height="13" rx="2.5" fill="currentColor"/>',
  search: '<circle cx="9.5" cy="9.5" r="6.5"/><path d="m14.3 14.3 5.2 5.2"/>',
  upload: '<path d="M7 8H4v12h14V8h-3M11 14V2M7.5 5.5 11 2l3.5 3.5"/>',
  sync: '<path d="M3.8 9.5a7.5 7.5 0 0 1 13.6-3.4M17.8 2.4v4h-4M18.2 12.5a7.5 7.5 0 0 1-13.6 3.4M4.2 19.6v-4h4"/>',
  logs: '<path d="M5 2h8l4 4v14H5Z"/><path d="M13 2v4h4M8 11h6M8 14.5h6"/>',
  uninstall: '<path d="M3 5h16M8 5V3h6v2M5 5l1 15h10l1-15"/><path d="m8.7 9.7 4.6 4.6M13.3 9.7l-4.6 4.6"/>',
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
/// Core errors arrive as their key followed by its arguments, each after a
/// U+001F; an argument that is itself a key is translated too.
function tr(message, ...args) {
  const [key, ...carried] = String(message).split("\u001f");
  const lookup = (key) =>
    window.COPOOL_LOCALES[locale]?.[key] ?? window.COPOOL_LOCALES.en[key];
  const value = lookup(key) ?? key;
  const values = carried.map((arg) => (lookup(arg) === undefined ? arg : tr(arg)));
  return P.formatLocalized(value, [...values, ...args]);
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
  // The back chevron has no text; name it for assistive technology.
  document
    .querySelector("#top-sub .back")
    .setAttribute("aria-label", tr("tab.accounts"));
  document.querySelectorAll("#pool-switch .pool-btn").forEach((button) => {
    button.title = withShortcut(
      tr(`pool.${button.dataset.pool}`),
      String(ACCOUNT_POOLS.indexOf(button.dataset.pool) + 1),
    );
    button.setAttribute("aria-label", tr(`pool.${button.dataset.pool}`));
  });
  document.querySelectorAll("#proxy-pool-switch .pool-btn").forEach((button) => {
    button.title = withShortcut(
      tr(`pool.${button.dataset.proxyPool}`),
      String(ACCOUNT_POOLS.indexOf(button.dataset.proxyPool) + 1),
    );
    button.setAttribute("aria-label", tr(`pool.${button.dataset.proxyPool}`));
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
  if (view.notStarted) return tr("accounts.window.not_started");
  if (!view.available || view.resetAt == null)
    return tr("accounts.window.reset_unavailable");
  if (mode === "exact")
    return tr("accounts.window.reset_at_format", view.reset);
  const countdown = P.resetCountdown(view.resetAt, Date.now() / 1000, locale, view.clockOffsetSeconds);
  return countdown.soon
    ? tr("accounts.window.resets_soon")
    : tr("accounts.window.resets_in_format", countdown.remaining);
}
function toggleResetDisplay() {
  resetDisplay = resetDisplay === "countdown" ? "exact" : "countdown";
  localStorage.setItem("copool.resetDisplay", resetDisplay);
  renderAccounts(accountsCache);
}
const meterWindows = new WeakMap();
function usagePaceTooltip(window, now = Date.now() / 1000) {
  const message = P.usagePaceMessage(window, now, locale);
  return message ? tr(message.key, ...message.args) : "";
}
function refreshMeterPace(track, now) {
  const window = meterWindows.get(track);
  const tone = P.usageTone(window, now);
  track.classList.toggle("high", tone === "high");
  track.classList.toggle("full", tone === "full");
  const title = usagePaceTooltip(window, now);
  if (track.title !== title) track.title = title;
}
function refreshVisibleMeterPace() {
  if (document.hidden || $("accounts-page").hidden) return;
  const now = Date.now() / 1000;
  for (const track of $("accounts").querySelectorAll(".meter"))
    refreshMeterPace(track, now);
}
function usageWindow(view, title, tone) {
  const remaining = usageMode() === "remaining";
  const track = el(
    "div",
    `meter${view.available && view.tone ? ` ${view.tone}` : ""}`,
  );
  meterWindows.set(track, view.paceWindow);
  track.title = usagePaceTooltip(view.paceWindow);
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
  const rounded = P.roundDisplayPercent(value, usageMode());
  const text = rounded === null ? "--" : `${rounded}%`;
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
/// `claude-sub2api-<provider>-<n>`, `antigravity-sub2api-<provider>-<n>`,
/// `grok-sub2api-<provider>-<n>`):
/// cards synced since the provider list was last read still group with
/// their provider.
function providerKey(account) {
  if (account.sourceTag !== "SUB2API") return null;
  return /^(?:claude-|antigravity-|grok-)?sub2api-(.+)-\d+$/.exec(account.id)?.[1] ?? null;
}
/// The saved Sub2api provider a card belongs to, if any.
function accountProvider(account) {
  const key = providerKey(account);
  return key === null ? null : (providerControllers[pool]?.find(key) ?? null);
}
function providerTitle(account) {
  return (
    providerControllers[pool]?.name(accountProvider(account)) ||
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
/// Removes an account, or all of a Sub2api provider's cards at once.
async function removeCards(cards, sub2api) {
  await action(
    () => accountsController.removeAccounts(cards.map((a) => a.id)),
    tr(sub2api ? "accounts.notice.sub2api_removed" : "accounts.notice.account_deleted"),
  );
}
/// Asks before removing, as from a card's menu.
async function confirmRemoval(cards, name, sub2api) {
  if (
    await confirmAction(
      tr("common.remove"),
      tr("accounts.card.remove_confirm_format", name),
      tr("common.remove"),
      true,
    )
  )
    await removeCards(cards, sub2api);
}
/// Cards whose token was just renewed, shown so on their refresh button
/// for a moment.
const renewedIds = new Set();
const RENEWED_SHOWN_MS = 2200;
/// Renews the account's tokens at once; its refresh button shows the
/// renewal under way, then done, and a notice tells how it went, as saying
/// hi does.
async function renewNow(account) {
  const controller = accountsController;
  try {
    if (await controller.renewNow(account.id)) {
      renewedIds.add(account.id);
      setTimeout(() => {
        renewedIds.delete(account.id);
        renderAccounts(accountsCache);
      }, RENEWED_SHOWN_MS);
      renderAccounts(accountsCache);
      notice(tr("accounts.notice.token_renewed"));
    }
  } catch (error) {
    notice(error, true);
  }
}
/// A hi or renewal under way, or just done, which takes the place of the
/// card's refresh button: its glyph, tooltip and whether it is done.
function cardTask(account) {
  const phase = sayingHi.get(account.id);
  if (phase) {
    const prefix = {
      codex: "accounts.card.codex_say_hi",
      antigravity: "accounts.card.antigravity_say_hi",
    }[pool] || "accounts.card.say_hi";
    return { glyph: `say-hi-${phase}`, title: tr(`${prefix}.${phase}`), done: phase === "received" };
  }
  if (accountsController.renewingIds.has(account.id))
    return { glyph: "renewal", title: tr("accounts.renewal.renewing"), done: false };
  if (renewedIds.has(account.id))
    return { glyph: "renewal-done", title: tr("accounts.notice.token_renewed"), done: true };
  return null;
}
function taskIndicator(task, full) {
  const node = el("span", `icon-btn card-task ${task.done ? "done" : "running"}`);
  node.append(icon(task.glyph));
  node.title = task.title;
  node.setAttribute("role", "status");
  node.setAttribute("aria-label", `${task.title} ${full}`);
  return node;
}
/// Turns the card over to edit it, at its top right, shown on the focused
/// card as a widget's.
function editButton(key, name) {
  const control = button("", "info", "icon-btn card-edit", () => editCard(key));
  const cardPool = pool;
  const update = () => {
    if (pool !== cardPool) return;
    control.title = cardDetails(flipTarget(key));
  };
  update();
  control.onpointerenter = update;
  control.onfocus = update;
  control.setAttribute("aria-label", `${tr("accounts.card.edit")} ${name}`);
  return control;
}
/// Keep the tooltip to the proxy, token lifetime and configured refresh interval.
function cardDetails(target) {
  if (!target) return tr("accounts.card.edit");
  const { account, grouped } = target;
  const lines = [];
  if (!grouped) {
    const owner = proxyOwner(target);
    const own = owner.proxyUrl?.trim();
    const proxy = own || sharedProxy(owner);
    lines.push(!proxy || proxy === "direct://" ? "direct" : `${tr("accounts.proxy.title")}: ${proxy}`);
  }
  if (account && !grouped && account.renewal) {
    const token = tokenLabel(account);
    if (token) lines.push(token);
  }
  const cards = target.group ? target.group.accounts : [account];
  const refresh = preferences.refresh[pool];
  const seconds = refresh.seconds * (cards.some((card) => card.isCurrent) ? 1 : refresh.fullEvery);
  lines.push(tr("accounts.refresh.interval_format", P.formatInterval(seconds, locale)));
  return lines.join("\n");
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
    warning.title = tr(account.usageError);
    warning.setAttribute("aria-label", warning.title);
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
  // A hi or renewal shows in the refresh button's place while it lasts; a
  // provider's account, refreshed with its provider, shows it alone.
  const task = cardTask(account);
  if (grouped) {
    if (task) actions.append(taskIndicator(task, full));
    return actions;
  }
  if (task) actions.append(taskIndicator(task, full));
  else {
    const refresh = button("", "refresh", "icon-btn card-refresh", () =>
      refreshUsage(account.id),
    );
    refresh.setAttribute("aria-label", `${tr("common.refresh_usage")} ${full}`);
    refreshTooltip(refresh, tr("common.refresh_usage"), () => account.nextRefreshAt);
    refresh.classList.toggle("spinning", refreshing);
    refresh.disabled =
      accountsController.sayingHiIds.has(account.id) ||
      accountsController.renewingIds.has(account.id);
    actions.append(refresh);
  }
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
/// Claude subscriptions and official Codex Plus cards can start an idle window.
function canSayHi(account) {
  return fiveHourIdle(account, pool, helloTarget(account)) && supportsSayHi(account);
}
/// The one pool a hi greets on a card with a model switch: the third party's
/// when chosen (true), else its own (false); null on a card without one,
/// whose hi greets each idle pool.
function helloTarget(account) {
  const usage = account.usage;
  if (!usage?.thirdPartyFiveHour && !usage?.thirdPartyOneWeek) return null;
  const view = P.card(account, false, "used", String);
  return hasThirdParty(view) ? showsThirdParty(view, account.id) : null;
}
function supportsSayHi(account) {
  if (pool === "claude") return true;
  // Grok has no five-hour window to start.
  if (pool === "grok") return false;
  // An Antigravity provider's cards say no hi, as Codex's.
  if (pool === "antigravity") return account.sourceTag !== "SUB2API";
  const plan = (account.usage?.planType?.trim() || account.planType || "").trim().toLowerCase();
  return account.sourceTag !== "SUB2API" && plan === "plus";
}
/// Known usage with no future reset indicates an idle window; Antigravity's
/// third-party (Claude) pool has its own, idle as well when reported so.
/// A hi to one pool alone (`target`, as helloTarget) looks at its window.
function fiveHourIdle(account, cardPool = pool, target = null) {
  if (!account.usage) return false;
  const now = Date.now() / 1000;
  const idle = (window) => !(window?.resetAt > now);
  const third = account.usage.thirdPartyFiveHour;
  if (target === true) return !!third && idle(third);
  if (target === false) return idle(account.usage.fiveHour);
  return idle(account.usage.fiveHour) ||
    (cardPool === "antigravity" && !!third && idle(third));
}
/// Both pools use the same delayed usage checks after the model replies.
const SAY_HI_LOOKS = [10, 30, 90, 180];
/// Cards saying hi, by how far: "sent" until the model replies, then
/// "received" while their 5h window has yet to show.
const sayingHi = new Map();
/// Says hi with the account, which starts its 5h window, then refreshes its
/// card over the next minutes until the window shows. Its say hi button
/// shows the hi sent, then the reply received; each refresh shows as one,
/// and nothing between them. A window shown, by these refreshes or any
/// other, is reported at once; one still not shown at the last, then.
async function sayHi(account) {
  const controller = accountsController;
  const targetPool = pool;
  // The model the card's switch shows, if it has one, alone.
  const target = helloTarget(account);
  const shown = () => {
    const card = controller.accounts.find((a) => a.id === account.id);
    // The card's pool's windows, whichever pool is shown meanwhile.
    return !card || (!!card.usage && !fiveHourIdle(card, targetPool, target));
  };
  if (sayingHi.has(account.id)) return;
  sayingHi.set(account.id, "sent");
  try {
    if (!(await controller.sayHi(account.id, target))) return;
    sayingHi.set(account.id, "received");
    controller.onUpdate();
    const replied = Date.now();
    // Always fetch usage after the reply; a cached reset cannot confirm this hi.
    await controller.refresh(account.id);
    for (const at of SAY_HI_LOOKS) {
      if (shown()) break;
      await refreshView();
      // Until the next refresh, the window is looked for each second.
      const due = replied + at * 1000;
      while (!shown() && Date.now() < due)
        await new Promise((resolve) =>
          setTimeout(resolve, Math.min(1000, due - Date.now())),
        );
      if (shown()) break;
      await controller.refresh(account.id);
    }
    if (shown()) notice(tr("accounts.notice.said_hi"));
    else notice(tr(`error.${targetPool}.say_hi_window_not_started`), true);
  } finally {
    sayingHi.delete(account.id);
  }
}
/// The card turned over to edit it, as a widget's: an account's id, or
/// `sub2api:<provider>` for a provider's group; one at a time. Its draft
/// proxy, greeting schedule once read, a time being added and a removal
/// awaiting confirmation stay over renders.
let editing = null;
/// The card being turned over, until its back shows, and the turn's
/// number: a later turn, or Escape, cancels one under way.
let pendingEdit = null;
let editGeneration = 0;
// Each half of a turn: edge-on at its middle, where the sides swap.
const FLIP_HALF_MS = 210;
// The list entry whose face is changing during this render.
let turningEntry = null;
/// What a flip key names among the shown cards: an account, maybe one of a
/// Sub2api provider's, or a provider's group.
function flipTarget(key) {
  const accounts = P.visibleAccounts(accountsCache);
  if (key.startsWith("sub2api:")) {
    const provider = key.slice("sub2api:".length);
    const members = accounts.filter((a) => providerKey(a) === provider);
    return members.length ? { group: { key: provider, accounts: members } } : null;
  }
  const account = accounts.find((a) => a.id === key);
  if (!account) return null;
  // A provider's only card is an ordinary card, as P.providerGroups shows it.
  const provider = providerKey(account);
  const grouped =
    provider !== null && accounts.filter((a) => providerKey(a) === provider).length > 1;
  return { account, grouped };
}
/// A provider's account has only its greetings to edit.
function hasBack(target) {
  return !!target && (!!target.group || !target.grouped || supportsSayHi(target.account));
}
/// The proxy edited: the account's own, or its provider's for a group.
function proxyOwner(target) {
  return target.group ? target.group.accounts[0] : target.account;
}
function proxyDraft(proxyUrl) {
  const direct = proxyUrl === "direct://";
  return { enabled: !direct, url: direct ? "" : proxyUrl || "" };
}
/// The proxy saved for a draft: none as "direct://"; blank inherits.
function proxyValue(draft) {
  return draft.enabled ? draft.url : "direct://";
}
/// The shared account proxy a blank proxy uses, when one is set; a
/// Sub2api provider's never does.
function sharedProxy(owner) {
  return owner.sourceTag === "SUB2API" ? "" : (preferences?.accountProxyUrl || "").trim();
}
/// The help under a proxy: what a blank one uses.
function proxyHelp(owner) {
  return tr(sharedProxy(owner) ? "accounts.proxy.shared_help" : "accounts.proxy.system_help");
}
/// A blank proxy's placeholder: the shared account proxy it uses, else an
/// example.
function proxyPlaceholder(owner) {
  return sharedProxy(owner) || "http://127.0.0.1:7890";
}
/// Turns a card over to its settings.
async function editCard(key) {
  if (collapsed || editing?.key === key || pendingEdit === key) return;
  const target = flipTarget(key);
  if (!hasBack(target)) return;
  const state = {
    key,
    pool,
    proxy: target.grouped ? null : proxyDraft(proxyOwner(target).proxyUrl),
    proxyError: null,
    schedule: null,
    adding: null,
    confirmRemove: false,
  };
  const schedule =
    target.account && supportsSayHi(target.account)
      ? invoke("greeting_schedule", { pool, id: target.account.id }).catch((error) => {
          notice(error, true);
          return null;
        })
      : null;
  if (editing || pendingEdit) cancelEditing();
  const generation = ++editGeneration;
  pendingEdit = key;
  await turnCard(key, 1, async () => {
    state.schedule = await schedule;
    if (generation !== editGeneration) return;
    pendingEdit = null;
    if (pool === state.pool && !collapsed) editing = state;
  });
}
/// Saves the card's proxy, then turns it back; an invalid proxy keeps it
/// open with the error.
async function finishEditing() {
  const state = editing;
  if (!state || state.saving) return;
  if (state.proxy && !(await saveProxyDraft(state))) return;
  await turnCard(state.key, -1, () => {
    if (editing === state) editing = null;
  });
}
/// Turns the card back without saving its proxy; at once, when the page
/// changes under it.
function cancelEditing(animated = true) {
  // A card still turning over turns back instead of showing its back.
  if (pendingEdit) {
    pendingEdit = null;
    editGeneration++;
  }
  const state = editing;
  if (!state) return;
  if (!animated) {
    editing = null;
    renderedAccountsState = undefined;
    renderAccounts(accountsCache);
    return;
  }
  turnCard(state.key, -1, () => {
    if (editing === state) editing = null;
  });
}
async function saveProxyDraft(state) {
  const target = flipTarget(state.key);
  if (!target) return true;
  const owner = proxyOwner(target);
  const value = proxyValue(state.proxy);
  if (value.trim() === (owner.proxyUrl || "").trim()) return true;
  state.saving = true;
  try {
    await accountsController.saveProxy(owner.id, value);
    return true;
  } catch (error) {
    // Done shows enabled again, to save the corrected proxy.
    state.saving = false;
    state.proxyError = tr(String(error?.message ?? error));
    renderEditing();
    return false;
  } finally {
    state.saving = false;
  }
}
/// Renders the card being edited again, as its settings change.
function renderEditing() {
  renderedAccountsState = undefined;
  renderAccounts(accountsCache);
}
/// Turns the card of `key` edge-on, makes `change`, renders, and turns the
/// card's other side in, growing it as the cards below slide down.
async function turnCard(key, direction, change) {
  const list = $("accounts");
  const find = () =>
    [...list.querySelectorAll("[data-flip-key]")].find((n) => n.dataset.flipKey === key);
  const still = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
  const node = find();
  const entry = node?.closest("[data-reorder-id]");
  const before = entry?.offsetHeight;
  const keyboardFocus = node?.contains(document.activeElement) &&
    document.activeElement.matches(":focus-visible");
  const raised = !still && node?.matches(":hover, :has(:focus-visible), .flip-raised, .hover-hold");
  if (node && !still) {
    if (raised) node.classList.add("flip-raised");
    await node
      .animate(
        [
          { transform: "perspective(900px) rotateY(0deg)" },
          { transform: `perspective(900px) rotateY(${90 * direction}deg) scale(0.96)` },
        ],
        { duration: FLIP_HALF_MS, easing: "cubic-bezier(0.55, 0, 0.9, 0.55)" },
      )
      .finished.catch(() => {});
    node.style.visibility = "hidden";
  }
  await change();
  turningEntry = entry?.dataset.reorderId;
  try {
    renderEditing();
  } finally {
    turningEntry = null;
  }
  const next = find();
  if (!next) return;
  if (keyboardFocus && document.activeElement === document.body)
    next.querySelector(":scope > :first-child > .card-edit, :scope > .back-head > .back-done")?.focus({ preventScroll: true });
  if (still) return;
  if (raised) next.classList.add("flip-raised");
  const turn = next.animate(
    [
      { transform: `perspective(900px) rotateY(${-90 * direction}deg) scale(0.96)` },
      { transform: "none" },
    ],
    { duration: FLIP_HALF_MS, easing: "cubic-bezier(0.1, 0.45, 0.45, 1)" },
  );
  const holder = next.closest("[data-reorder-id]");
  const after = holder?.offsetHeight;
  // Reveal the extra height without cropping the lifted card's outer shadow.
  if (before && after > before)
    window.CopoolReorder.springAnimate(holder, [
      { clipPath: `inset(-24px -24px ${after - before}px round 8px)` },
      { clipPath: "inset(-24px round 8px)" },
    ]);
  await turn.finished.catch(() => {});
  next.classList.remove("flip-raised");
}
/// A card's settings side: its proxy, token renewal, greetings, the pool's
/// refresh and its removal; a provider's group has the provider's proxy;
/// a provider's account its greetings alone.
function cardBack(target) {
  const state = editing;
  const { account, group, grouped } = target;
  const cards = group ? group.accounts : [account];
  const current = cards.some((a) => a.isCurrent);
  const name = group
    ? providerTitle(group.accounts[0])
    : account.email || account.accountId || account.label;
  const node = el(
    grouped ? "div" : "section",
    grouped ? "account-member account-back" : `block account account-back${current ? " current" : ""}`,
  );
  node.dataset.flipKey = state.key;
  if (account) node.dataset.cardId = account.id;
  const head = el("div", "back-head");
  const title = el("span", "block-title", name);
  title.title = name;
  const done = el("button", "btn primary back-done", tr("accounts.card.done"));
  done.type = "button";
  done.disabled = !!state.saving;
  done.onclick = () => finishEditing();
  head.append(title, done);
  node.append(head);
  if (state.proxy) node.append(proxySection(state, proxyOwner(target)));
  if (account && !grouped && account.renewal) node.append(renewalSection(account));
  if (account && supportsSayHi(account) && state.schedule)
    node.append(greetingSection(account, state));
  if (!grouped) node.append(refreshSection(current, refreshKey(target)));
  if (!grouped) node.append(removalSection(state, cards, !!group));
  return node;
}
/// A section of a card's back, as a grouped form's: its caption, with any
/// detail and action, over an inset group of rows, then any footnote.
function backSection(title, ...aside) {
  const section = el("div", "back-section");
  const caption = el("div", "back-caption");
  caption.append(el("span", "back-caption-title", title), ...aside);
  const group = el("div", "back-group");
  section.append(caption, group);
  section.group = group;
  return section;
}
/// A row of a section's group: a label, maybe a line under it or a tooltip
/// on it, and its control.
function backRow(label, control, note = null, tip = null) {
  const row = el("label", "back-row");
  const text = el("span", "back-label", label);
  if (tip) text.title = tip;
  if (note) text.append(el("span", "back-note", note));
  row.append(text, control);
  return row;
}
function backSwitch(field, checked, label, onchange) {
  const input = el("input", "switch");
  input.type = "checkbox";
  input.checked = checked;
  input.dataset.backField = field;
  input.setAttribute("aria-label", label);
  input.onchange = () => onchange(input.checked);
  return input;
}
function backButton(label, run, disabled = false) {
  const control = el("button", "btn quiet prominent", label);
  control.type = "button";
  control.disabled = disabled;
  control.onclick = run;
  return control;
}
function backHelp(text, error = false) {
  return el("p", `back-help${error ? " error" : ""}`, text);
}
/// The proxy, saved with Done or Return.
function proxySection(state, owner) {
  const draft = state.proxy;
  const help = backHelp(state.proxyError || proxyHelp(owner), !!state.proxyError);
  const url = el("input", "field");
  url.dataset.backField = "proxy-url";
  url.placeholder = proxyPlaceholder(owner);
  url.value = draft.url;
  url.disabled = !draft.enabled;
  url.spellcheck = false;
  url.setAttribute("aria-label", tr("accounts.proxy.title"));
  url.oninput = () => {
    draft.url = url.value;
  };
  url.onkeydown = (event) => {
    if (event.defaultPrevented || event.isComposing || event.keyCode === 229) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      if (!event.repeat) url.blur();
    } else if (event.key === "Enter") {
      event.preventDefault();
      finishEditing();
    }
  };
  const enabled = backSwitch("proxy-enabled", draft.enabled, tr("accounts.proxy.enabled"), (on) => {
    draft.enabled = on;
    url.disabled = !on;
  });
  const section = backSection(tr("accounts.proxy.title"));
  const field = el("div", "back-row back-field");
  field.append(url);
  section.group.append(backRow(tr("accounts.proxy.enabled"), enabled), field);
  section.append(help);
  return section;
}
/// The token's time left, renewing it now, and renewing it as needed.
function renewalSection(account) {
  const busy =
    accountsController.refreshingIds.has(account.id) ||
    accountsController.remoteRefreshingIds.has(account.id) ||
    accountsController.sayingHiIds.has(account.id) ||
    accountsController.renewingIds.has(account.id);
  // Under its "Token renewal" title, the time left alone.
  const expiry = P.tokenExpiry(account.tokenExpiresAt, Date.now() / 1000, locale);
  const left = el(
    "span",
    "back-detail",
    !expiry
      ? ""
      : expiry.expired
        ? tr("accounts.renewal.expired")
        : tr("accounts.renewal.expires_format", expiry.remaining),
  );
  const section = backSection(
    tr("accounts.renewal.title"),
    left,
    backButton(tr("accounts.renewal.now"), () => renewNow(account), busy),
  );
  section.group.append(
    backRow(
      tr("accounts.renewal.auto"),
      backSwitch("renewal-auto", account.renewal === "always", tr("accounts.renewal.auto"), (on) =>
        action(() => accountsController.setRenewal(account.id, on ? "always" : "never")),
      ),
    ),
  );
  return section;
}
/// Saying hi now, automatically, and at which times.
function greetingSection(account, state) {
  const schedule = state.schedule;
  const busy =
    sayingHi.has(account.id) ||
    accountsController.refreshingIds.has(account.id) ||
    accountsController.remoteRefreshingIds.has(account.id);
  const section = backSection(
    tr("accounts.say_hi.title"),
    backButton(
      tr("accounts.say_hi.now"),
      () => sayHi(account).then(refreshView).catch((error) => notice(error, true)),
      busy || !canSayHi(account),
    ),
  );
  // Greeting when the 5h window is idle and at set times are separate:
  // each greets on its own.
  section.group.append(
    backRow(
      tr("accounts.say_hi.auto"),
      backSwitch("greeting-auto", schedule.auto, tr("accounts.say_hi.auto"), (on) =>
        saveGreeting(account.id, state, on, schedule.times),
      ),
      tr("accounts.say_hi.auto_help"),
    ),
  );
  const daily = el("div", "back-row back-stack");
  // What the times do shows on their name; a failed greeting under it.
  const dailyLabel = el("span", "back-label", tr("accounts.say_hi.times"));
  dailyLabel.title = tr("accounts.say_hi.times_help");
  if (schedule.lastError) dailyLabel.append(el("span", "back-note error", schedule.lastError));
  const times = el("div", "back-times");
  for (const time of schedule.times) {
    const chip = el("span", "back-time", time);
    const remove = button("", "close", "back-time-remove", () =>
      saveGreeting(account.id, state, schedule.auto, withoutGreetingTime(schedule.times, time)),
    );
    remove.title = tr("accounts.say_hi.remove");
    remove.setAttribute("aria-label", `${tr("accounts.say_hi.remove")} ${time}`);
    chip.append(remove);
    times.append(chip);
  }
  if (state.adding !== null) {
    const chip = el("span", "back-time adding");
    const input = el("input");
    input.type = "time";
    input.required = true;
    input.value = state.adding;
    input.dataset.backField = "greeting-time";
    input.setAttribute("aria-label", tr("accounts.say_hi.add"));
    const commit = () => {
      if (!input.isConnected || state.adding === null) return;
      state.adding = null;
      const times = withGreetingTime(schedule.times, input.value);
      if (times) saveGreeting(account.id, state, schedule.auto, times);
      else renderEditing();
    };
    input.oninput = () => {
      state.adding = input.value;
    };
    input.onkeydown = (event) => {
      if (event.defaultPrevented || event.isComposing || event.keyCode === 229) return;
      if (event.key === "Enter") {
        event.preventDefault();
        commit();
      } else if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        if (event.repeat) return;
        state.adding = null;
        renderEditing();
      }
    };
    input.onblur = commit;
    chip.append(input);
    times.append(chip);
  } else {
    const add = el("button", "back-time add", `+ ${tr("accounts.say_hi.add")}`);
    add.type = "button";
    add.onclick = () => {
      state.adding = "12:30";
      renderEditing();
      $("accounts").querySelector('[data-back-field="greeting-time"]')?.focus();
    };
    times.append(add);
  }
  daily.append(dailyLabel, times);
  section.group.append(daily);
  return section;
}
/// The times with a new one, in order; null for no time or one already set.
function withGreetingTime(times, time) {
  if (!/^\d{2}:\d{2}$/.test(time) || times.includes(time)) return null;
  return [...times, time].sort();
}
function withoutGreetingTime(times, time) {
  return times.filter((t) => t !== time);
}
/// Saves a greeting schedule, then shows it on the card being edited.
async function saveGreeting(id, state, auto, times) {
  try {
    await invoke("set_greeting_schedule", { pool: state.pool, id, auto, times });
    state.schedule = { ...state.schedule, auto, times };
  } catch (error) {
    notice(error, true);
  }
  if (editing === state) renderEditing();
}
/// The key a card's own refresh switches are saved under: an account's id,
/// or `sub2api:<provider>` for a Sub2api provider's cards, which refresh
/// together.
function refreshKey(target) {
  if (target.group) return `sub2api:${target.group.key}`;
  const provider = providerKey(target.account);
  return provider === null ? target.account.id : `sub2api:${provider}`;
}
/// A card's background refresh: the pool's intervals, which all its cards
/// share (the card in use's only on its own card), and the card's own
/// speed-up and slow-down, which follow the pool's until set.
function refreshSection(current, key) {
  const refresh = preferences.refresh[pool];
  const own = preferences.refresh.cards?.[pool]?.[key] ?? {};
  const adaptive = own.adaptive ?? refresh.adaptive;
  const backoff = own.backoff ?? refresh.backoff;
  const cardPool = pool;
  // On the refresh as saved by then: another change made before this
  // one is saved stays.
  const saved = (change) =>
    queuePreferenceSave((latest) => ({ refresh: change(latest.refresh) }))
      .catch((error) => notice(error, true))
      .finally(renderEditing);
  const save = (patch) =>
    saved((latest) => ({ ...latest, [cardPool]: { ...latest[cardPool], ...patch } }));
  const saveCard = (patch) =>
    saved((latest) => {
      const cards = latest.cards ?? {};
      const pooled = cards[cardPool] ?? {};
      return {
        ...latest,
        cards: { ...cards, [cardPool]: { ...pooled, [key]: { ...pooled[key], ...patch } } },
      };
    });
  // Its rows name what each interval is for; all the pool's cards share them.
  const section = backSection(tr("accounts.refresh.title"));
  const choices = P.refreshChoices(pool, refresh.seconds);
  const select = (field, options, value, onchange) => {
    const node = el("select");
    node.dataset.backField = field;
    for (const [optionValue, label] of options) {
      const option = el("option", "", label);
      option.value = String(optionValue);
      option.selected = optionValue === value;
      node.append(option);
    }
    node.onchange = () => onchange(Number(node.value));
    return node;
  };
  const interval = (seconds) => P.formatInterval(seconds, locale);
  if (current)
    section.group.append(
      backRow(
        tr("accounts.refresh.current"),
        select(
          "refresh-seconds",
          choices.seconds.map((seconds) => [seconds, interval(seconds)]),
          refresh.seconds,
          (seconds) => save({ seconds }),
        ),
      ),
    );
  section.group.append(
    backRow(
      tr("accounts.refresh.all"),
      select(
        "refresh-full-every",
        choices.fullEvery.map((n) => [
          n,
          n === 1
            ? tr("accounts.refresh.every_round_format", interval(refresh.seconds))
            : tr("accounts.refresh.multiple_format", interval(refresh.seconds * n), n),
        ]),
        refresh.fullEvery,
        (fullEvery) => save({ fullEvery }),
      ),
    ),
    backRow(
      tr("accounts.refresh.adaptive"),
      backSwitch("refresh-adaptive", adaptive, tr("accounts.refresh.adaptive"), (adaptive) =>
        saveCard({ adaptive }),
      ),
      null,
      tr("accounts.refresh.adaptive_help_format", interval(P.fastRefreshSeconds(refresh.seconds))),
    ),
  );
  // Only Claude's own accounts pause after HTTP 429 for now.
  if (pool === "claude" && !key.startsWith("sub2api:"))
    section.group.append(
      backRow(
        tr("accounts.refresh.backoff"),
        backSwitch("refresh-backoff", backoff, tr("accounts.refresh.backoff"), (backoff) =>
          saveCard({ backoff }),
        ),
        null,
        tr("accounts.refresh.backoff_help"),
      ),
    );
  return section;
}
/// Removal, confirmed by a second click.
function removalSection(state, cards, provider) {
  const row = el("div", "back-section back-removal");
  const remove = el(
    "button",
    `back-group back-remove${state.confirmRemove ? " confirm" : ""}`,
    tr(state.confirmRemove ? "accounts.card.remove_confirm" : "common.remove"),
  );
  remove.type = "button";
  remove.dataset.backField = "remove";
  remove.onclick = () => {
    if (!state.confirmRemove) {
      state.confirmRemove = true;
      renderEditing();
      setTimeout(() => {
        if (editing !== state || !state.confirmRemove) return;
        state.confirmRemove = false;
        renderEditing();
      }, 3000);
      return;
    }
    if (editing === state) editing = null;
    removeCards(cards, provider || cards[0].sourceTag === "SUB2API");
  };
  row.append(remove);
  return row;
}
/// A card's menu, at the pointer: editing it, then what its buttons do and
/// what it has no button for, and its removal.
function cardMenu(target) {
  const { account, group, grouped } = target;
  const items = [];
  if (!collapsed && hasBack(target))
    items.push({ label: tr("accounts.card.edit"), run: () => editCard(group ? `sub2api:${group.key}` : account.id) });
  if (account && grouped) {
    if (supportsSayHi(account))
      items.push({
        label: tr("accounts.say_hi.now"),
        disabled: !canSayHi(account) || sayingHi.has(account.id),
        run: () => sayHi(account).then(refreshView).catch((error) => notice(error, true)),
      });
    return items;
  }
  if (items.length) items.push({ separator: true });
  const cards = group ? group.accounts : [account];
  const busy = cards.some(
    (a) =>
      accountsController.refreshingIds.has(a.id) ||
      accountsController.remoteRefreshingIds.has(a.id) ||
      accountsController.sayingHiIds.has(a.id) ||
      accountsController.renewingIds.has(a.id),
  );
  items.push({
    label: tr("common.refresh_usage"),
    disabled: busy,
    run: () => (group ? refreshProvider(group) : refreshUsage(account.id)),
  });
  if (account && supportsSayHi(account))
    items.push({
      label: tr("accounts.say_hi.now"),
      disabled: busy || !canSayHi(account) || sayingHi.has(account.id),
      run: () => sayHi(account).then(refreshView).catch((error) => notice(error, true)),
    });
  if (account?.renewal)
    items.push({ label: tr("accounts.renewal.now"), disabled: busy, run: () => renewNow(account) });
  // A Grok provider can reapply its config after an app or gateway update.
  const canReapply = pool === "grok" && cards[0]?.sourceTag === "SUB2API";
  if (canReapply || !cards.some((a) => a.isCurrent)) {
    const target = group ? switchTarget(group) : account;
    items.push({
      label: tr("accounts.card.switch_to_this"),
      disabled: target.available === false || switchingId !== null,
      run: () => switchAccount(target),
    });
  }
  const name = group ? providerTitle(group.accounts[0]) : account.email || account.label;
  items.push(
    { separator: true },
    {
      label: tr("common.remove"),
      destructive: true,
      run: () => confirmRemoval(cards, name, !!group || account.sourceTag === "SUB2API"),
    },
  );
  return items;
}
/// The plan tag. Codex treats an unknown plan as Team; Claude shows none
/// until its plan is known.
function planTag(account, view) {
  const known = P.clean(account.usage?.planType) || P.clean(account.planType);
  return pool !== "codex" && !known ? "" : el("span", "tag plan", view.plan);
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
/// The access token's remaining time in the card's details tooltip.
function tokenLabel(account) {
  const expiry = P.tokenExpiry(
    account.tokenExpiresAt,
    Date.now() / 1000,
    locale,
  );
  if (!expiry) return null;
  return expiry.expired
    ? tr("accounts.renewal.expired")
    : tr("accounts.renewal.expires_format", expiry.remaining);
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
    const errors = [
      ...new Set(group.accounts.map((a) => a.usageError).filter(Boolean).map((e) => tr(e))),
    ];
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
  refresh.disabled = ids.some((id) => accountsController.sayingHiIds.has(id));
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
/// Without resets (Antigravity and Grok, which have none), the credits alone.
function accountBalanceText(usage, credits, inventory = P.resetInventory(usage), resets = true) {
  const footerText = el("span", "account-footer-text");
  const details = P.balanceTooltips(usage);
  const expiry = (at) => at == null
    ? tr("accounts.card.expiry_unknown")
    : tr("accounts.card.reset_expires_format", P.formatReset(at, locale));
  const creditText = el("span", "account-credits", tr("accounts.card.credits_format", credits));
  const creditValue = String(credits ?? "").trim();
  const hasCredits = creditValue !== "" && creditValue !== "--"
    && (!/\d/.test(creditValue) || /[1-9]/.test(creditValue));
  if (hasCredits) {
    creditText.title = details.creditDates.length
      ? details.creditDates.map(expiry).join("\n") : expiry(null);
  }
  if (!resets) {
    footerText.append(creditText);
    return footerText;
  }
  const resetCount = el("span", "account-resets", tr(
    inventory.count !== null
      ? "accounts.card.remaining_resets_format" : "accounts.card.resets_unavailable_format",
    inventory.count ?? "--",
  ));
  if (inventory.count !== null) {
    if (inventory.count > 0) {
      resetCount.title = details.resetDates.length
        ? details.resetDates.map(({ at, count }) => `${expiry(at)}${count > 1 ? ` × ${count}` : ""}`).join("\n")
        : expiry(null);
    }
    if (usage?.resetsLastConfirmedAt != null) {
      const confirmed = tr("accounts.card.resets_last_confirmed_format",
        P.formatReset(usage.resetsLastConfirmedAt, locale));
      resetCount.title = resetCount.title ? `${resetCount.title}\n${confirmed}` : confirmed;
    }
  }
  footerText.append(creditText, " · ", resetCount);
  if (inventory.warning) {
    const kinds = {
      full: "accounts.card.reset_full", fable: "accounts.card.reset_fable",
      partial: "accounts.card.reset_partial", unknown: "accounts.card.reset_unknown",
    };
    const lines = details.types.map(
      ({ kind, count, at }) => `${tr(kinds[kind])} × ${count} · ${expiry(at)}`,
    );
    const warning = button("", "reset-notice", "reset-warning", () => showResetPopover(warning, lines, true));
    warning.type = "button";
    warning.setAttribute("aria-label", lines.join("\n"));
    warning.setAttribute("aria-haspopup", "dialog");
    warning.setAttribute("aria-expanded", "false");
    // Sibling of the reset text: the button has no expiry tooltip.
    footerText.append(warning);
  }
  return footerText;
}

function accountCard(account, grouped = false) {
  if (!collapsed && editing?.key === account.id) {
    const target = flipTarget(account.id);
    if (target) return cardBack(target);
  }
  const full = account.email || account.accountId || account.label;
  const view = P.card(
    account,
    collapsed,
    usageMode(),
    (epoch) => P.formatReset(epoch, locale),
    pool,
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
    row.dataset.accent = view.accent;
    const name = el("span", "account-row-name", view.name);
    name.title = full;
    const secondCompactLabel =
      hasMonthlyWindow() && !view.oneWeek.available && view.oneMonth.available ? "1m" : "1w";
    row.append(
      el("span", grouped ? "" : "dot"),
      name,
      planTag(account, view) || el("span", "tag plan placeholder"),
      compactUsage(view.compact[0], "5h"),
      compactUsage(view.compact[1], secondCompactLabel),
      accountControls(account, full, view, grouped),
    );
    if (!grouped) row.dataset.reorderHandle = "";
    row.dataset.cardId = account.id;
    row.dataset.flipKey = account.id;
    return row;
  }
  const card = el(
    grouped ? "div" : "section",
    grouped
      ? "account-member"
      : `block account${account.isCurrent ? " current" : ""}`,
  );
  card.dataset.accent = view.accent;
  card.dataset.cardId = account.id;
  card.dataset.flipKey = account.id;
  const head = el("div", "block-head");
  // The CURRENT tag alone marks the active account, as in the SwiftUI card.
  const name = el("div", "account-name");
  const title = el("span", "block-title", full);
  title.title = full;
  name.append(title);
  const tags = accountTags(account, view, grouped);
  head.append(tags);
  const models = modelSwitch(view, account.id);
  // A provider's account without a plan tag or switch has no tag row; its
  // name's line is then its first.
  const headed = !(grouped && !tags.childElementCount && !models);
  // The edit button takes the free end of the first line, before the switch.
  if (hasBack({ account, grouped }))
    (headed ? head : name).append(editButton(account.id, full));
  if (models) head.append(models);
  // A card is dragged by its tags and name, as OpenQuota by its header.
  if (!grouped) head.dataset.reorderHandle = name.dataset.reorderHandle = "";
  // A negative reset count is unavailable, as in AccountCardPresentation.
  // Sub2api cards keep their credits, which their usage reports.
  const credits =
    view.credits === "unlimited" ? tr("accounts.card.unlimited") : view.credits;
  const inventory = P.resetInventory(account.usage);
  const footer = el("div", "account-footer");
  // Only Codex and Claude report banked resets; Antigravity and Grok show credits alone.
  const footerText = accountBalanceText(account.usage, credits, inventory, pool === "codex" || pool === "claude");
  footer.append(footerText, accountControls(account, full, view, grouped));
  card.append(
    ...(headed ? [head] : []),
    name,
    ...usageWindows(view, account.id),
    footer,
  );
  return card;
}
/// Accounts whose usage rows show their third-party pool rather than their
/// own.
const thirdPartyShown = new Set();
/// The account whose model switch was just clicked, while its card renders.
let modelsSwapped = null;
/// The third-party pool's model: Antigravity's other than Gemini, Claude.
const THIRD_PARTY_MODEL = "Claude";
/// The model of a pool's own windows.
function ownModel() {
  return { codex: "Codex", claude: "Claude", antigravity: "Gemini", grok: "Grok" }[pool];
}
/// Whether a card reports a third-party pool beside its own.
function hasThirdParty(view) {
  return view.thirdPartyFiveHour.available || view.thirdPartyOneWeek.available;
}
function showsThirdParty(view, accountId) {
  return hasThirdParty(view) && thirdPartyShown.has(accountId);
}
/// A model's mark for the model switch: its pool's from the pool switch, and
/// Gemini's spark (Simple Icons, CC0) in its gradient.
function modelBrand(model) {
  if (model !== "Gemini") {
    const mark = document.querySelector(`.pool-btn[data-pool="${model.toLowerCase()}"] .brand`);
    if (mark) return mark.cloneNode(true);
  }
  const mark = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  mark.setAttribute("class", "brand");
  mark.setAttribute("viewBox", "0 0 24 24");
  mark.setAttribute("aria-hidden", "true");
  mark.innerHTML =
    '<defs><linearGradient id="gemini-tint" x1="0" y1="1" x2="1" y2="0">' +
    '<stop offset="0" stop-color="#4285f4"/><stop offset="0.7" stop-color="#9b72cb"/>' +
    '<stop offset="1" stop-color="#d96570"/></linearGradient></defs>' +
    '<path fill="url(#gemini-tint)" d="M11.04 19.32Q12 21.51 12 24q0-2.49.93-4.68.96-2.19 ' +
    "2.58-3.81t3.81-2.55Q21.51 12 24 12q-2.49 0-4.68-.93a12.3 12.3 0 0 1-3.81-2.58 12.3 12.3 " +
    "0 0 1-2.58-3.81Q12 2.49 12 0q0 2.49-.96 4.68-.93 2.19-2.55 3.81a12.3 12.3 0 0 1-3.81 " +
    '2.58Q2.49 12 0 12q2.49 0 4.68.96 2.19.93 3.81 2.55t2.55 3.81"/>';
  return mark;
}
/// A card reporting a third-party pool chooses whose windows its usage rows
/// show, its own or the third party's, with one icon button at the end of its
/// tag row: both models' marks stacked, the shown model's in front and the
/// other's faded behind it, which a click brings to the front.
function modelSwitch(view, accountId) {
  if (!hasThirdParty(view)) return null;
  const third = showsThirdParty(view, accountId);
  const [label, other] = third
    ? [THIRD_PARTY_MODEL, ownModel()]
    : [ownModel(), THIRD_PARTY_MODEL];
  const swapped = modelsSwapped === accountId;
  const control = button("", null, `model-switch${swapped ? " swapped" : ""}`, () => {
    if (third) thirdPartyShown.delete(accountId);
    else thirdPartyShown.add(accountId);
    // The switch rendered anew trades its marks' places as it appears.
    modelsSwapped = accountId;
    try {
      renderAccounts(accountsCache);
    } finally {
      modelsSwapped = null;
    }
  });
  control.type = "button";
  control.title = label;
  control.setAttribute("aria-label", label);
  control.setAttribute("aria-pressed", String(third));
  control.dataset.modelSwitch = accountId;
  const behind = el("span", "model-mark behind");
  behind.append(modelBrand(other));
  const front = el("span", "model-mark front");
  front.append(modelBrand(label));
  control.append(behind, front);
  return control;
}
/// The fills of a card's usage rows, in order, as they look now (partway
/// through a slide too): their levels in percent and their colors.
function meterStates(card) {
  return [...card.querySelectorAll(":scope > .usage .meter-fill")].map((fill) => {
    const style = getComputedStyle(fill);
    const track = fill.parentElement.getBoundingClientRect().width;
    return {
      fill,
      level: fill.hidden || !track ? 0 : (fill.getBoundingClientRect().width / track) * 100,
      background: style.backgroundColor,
      opacity: style.opacity,
    };
  });
}
/// The meters of the list's cards that show usage rows, by card.
function cardMeters(list) {
  const meters = new Map();
  for (const card of list.querySelectorAll("[data-card-id]")) {
    const states = meterStates(card);
    if (states.length) meters.set(card.dataset.cardId, states);
  }
  return meters;
}
/// After a render, the meters of cards that showed usage rows before (their
/// usage refreshed, or switched to another pool's) slide from the levels
/// they showed to their own, and change color as they go, as a progress bar
/// does. A row new to its card rises from empty.
function slideMeters(list, before) {
  if (!before.size || window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
  for (const card of list.querySelectorAll("[data-card-id]")) {
    const shown = before.get(card.dataset.cardId);
    if (!shown) continue;
    meterStates(card).forEach(({ fill, background, opacity }, index) => {
      const level = fill.hidden ? 0 : parseFloat(fill.style.width) || 0;
      const from = shown[index] ?? { level: 0, background, opacity };
      if (
        Math.abs(from.level - level) < 0.5 &&
        from.background === background &&
        from.opacity === opacity
      )
        return;
      const empty = fill.hidden;
      fill.hidden = false;
      fill.animate(
        [
          { width: `${from.level}%`, backgroundColor: from.background, opacity: from.opacity },
          { width: `${level}%`, backgroundColor: background, opacity },
        ],
        { duration: 420, easing: "cubic-bezier(0.25, 1, 0.5, 1)" },
      ).onfinish = () => { fill.hidden = empty; };
    });
  }
}
/// A card's usage rows: five hours and the week. Codex Free cards that report
/// a 30-day monthly quota show Monthly (omitting empty Session/Weekly rows).
/// Claude cards that report Fable quota add its weekly row beneath them. An
/// Antigravity card's are Gemini's. A card's third-party pool, once reported
/// and chosen (modelSwitch), shows instead.
/// The weekly rows of a Sub2api too old to report them say so when clicked.
/// Whether the pool's accounts may have a monthly window: Codex Free, and a
/// Grok plan whose allowance lasts a month.
function hasMonthlyWindow() {
  return pool === "codex" || pool === "grok";
}
function usageWindows(view, accountId) {
  const five = tr("accounts.window.five_hour"), week = tr("accounts.window.one_week");
  const rows = (fiveHour, oneWeek) => [
    usageWindow(fiveHour, five, "five-hour"),
    weeklyNotice(usageWindow(oneWeek, week, "one-week"), view.outdatedSub2api),
  ];
  if (showsThirdParty(view, accountId))
    return rows(view.thirdPartyFiveHour, view.thirdPartyOneWeek);
  const shown = rows(view.fiveHour, view.oneWeek);
  if (hasMonthlyWindow() && view.oneMonth.available) {
    const month = usageWindow(view.oneMonth, tr("accounts.window.one_month"), "one-month");
    if (!view.oneWeek.available) {
      return view.fiveHour.available
        ? [usageWindow(view.fiveHour, five, "five-hour"), month]
        : [month];
    }
    shown.push(month);
  }
  if (pool === "claude" && view.fableOneWeek.available) {
    shown.push(usageWindow(view.fableOneWeek, tr("accounts.window.fable"), "fable"));
  }
  return shown;
}
/// A weekly row's notice that its Sub2api `version` reports no weekly usage.
function weeklyNotice(row, version) {
  if (!version) return row;
  // Each pool's weekly usage came with its own Sub2api release.
  const lines = [
    tr("accounts.window.sub2api_weekly_unavailable_format", version),
    tr(pool === "grok"
      ? "accounts.window.sub2api_grok_weekly_upgrade"
      : "accounts.window.sub2api_weekly_upgrade"),
  ];
  const notice = button("", "reset-notice", "reset-warning", () =>
    showResetPopover(notice, lines, true),
  );
  notice.type = "button";
  notice.setAttribute("aria-label", lines.join("\n"));
  notice.setAttribute("aria-haspopup", "dialog");
  notice.setAttribute("aria-expanded", "false");
  row.querySelector(".usage-value").append(notice);
  return row;
}
/// A Sub2api provider's cards close together in one card, which shows once
/// whether the provider is in use, with its controls where a single
/// account's card has them: editing at the top right, refresh and switch at
/// the bottom right.
function providerGroup(group) {
  const title = providerTitle(group.accounts[0]);
  const source = el("span", "tag muted", P.clean(group.accounts[0].sourceTag) || "SUB2API");
  // A provider's cards are current together.
  const current = group.accounts.some((a) => a.isCurrent);
  const flipKey = `sub2api:${group.key}`;
  if (!collapsed && editing?.key === flipKey) return cardBack({ group });
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
    wrapper.dataset.flipKey = flipKey;
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
  head.dataset.reorderHandle = "";
  block.dataset.flipKey = flipKey;
  const list = el("div", "account-group-members");
  list.append(...members);
  const footer = el("div", "account-footer");
  footer.append(
    el("span", "account-footer-text"),
    providerControls(group, title, current),
  );
  head.append(editButton(flipKey, title));
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
let heldHoverNodes = [];
function clearHoverHold() {
  if (!heldHoverNodes.length) return;
  const nodes = heldHoverNodes;
  heldHoverNodes = [];
  for (const node of nodes) node.classList.remove("hover-hold");
}
for (const type of ["pointermove", "wheel", "blur"])
  window.addEventListener(type, clearHoverHold, { passive: true });
document.documentElement.addEventListener("pointerleave", clearHoverHold, { passive: true });
document.addEventListener(
  "keydown",
  (event) => {
    if (event.key === "Tab") clearHoverHold();
  },
  { capture: true, passive: true },
);
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
        return [
          view.fiveHour,
          view.oneWeek,
          view.oneMonth,
          view.fableOneWeek,
          view.thirdPartyFiveHour,
          view.thirdPartyOneWeek,
        ].map((w) =>
          resetLabel(w, "countdown"),
        );
      }),
    collapsed,
    [...thirdPartyShown],
    locale,
    preferences?.usageDisplay,
    accounts.map(accountProxy),
    // Cards group under their provider, which may load after them.
    accounts.map(providerTitle),
    preferences?.accountOrder?.[pool],
    [...accountsController.refreshingIds],
    [...accountsController.remoteRefreshingIds],
    [...accountsController.sayingHiIds],
    [...sayingHi],
    [...accountsController.renewingIds],
    [...renewedIds],
    switchingId,
    accountsController.adding,
    accountsController.authorizing,
    pool,
  ];
  if (P.cardsEqual(renderState, renderedAccountsState)) return;
  renderedAccountsState = structuredClone(renderState);
  accounts = P.visibleAccounts(accounts);
  const list = $("accounts");
  // The card being edited keeps the field being typed in.
  const focused = list.contains(document.activeElement)
    ? document.activeElement.dataset.backField
    : undefined;
  const selection = focused && document.activeElement.selectionStart != null
    ? [document.activeElement.selectionStart, document.activeElement.selectionEnd]
    : null;
  // A model switch used from the keyboard keeps its focus.
  const models = list.contains(document.activeElement)
    ? document.activeElement.dataset.modelSwitch
    : undefined;
  list.className = collapsed ? "account-list compact block flush" : "account-list";
  // Preserve hover elevation across DOM replacement until :hover re-settles.
  const hoveredEntries = new Set(
    [...list.querySelectorAll("[data-reorder-id]:is(:hover, .hover-hold)")].map(
      (n) => n.dataset.reorderId,
    ),
  );
  const hoveredCards = new Set(
    [...list.querySelectorAll("[data-card-id]:is(:hover, .hover-hold)")].map(
      (n) => n.dataset.cardId,
    ),
  );
  // Switching between full and collapsed, the cards' parts are measured too.
  const before = captureLayout(
    list,
    renderedLayout?.pool === pool && renderedLayout.collapsed !== collapsed,
  );
  // The meters slide from what they showed to their new levels.
  const meters = renderedLayout?.pool === pool ? cardMeters(list) : new Map();
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
          {
            claude: "accounts.empty.message.no_claude_accounts",
            antigravity: "accounts.empty.message.no_antigravity_accounts",
            grok: "accounts.empty.message.no_grok_accounts",
          }[pool] || "accounts.empty.message.no_accounts",
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
  const held = [];
  if (hoveredEntries.size || hoveredCards.size) {
    for (const node of list.querySelectorAll("[data-reorder-id]")) {
      const key = node.dataset.reorderId;
      if (key !== turningEntry && hoveredEntries.has(key)) {
        node.classList.add("hover-hold");
        held.push(node);
      }
    }
    for (const node of list.querySelectorAll("[data-card-id]")) {
      const id = node.dataset.cardId;
      if (
        node.closest("[data-reorder-id]")?.dataset.reorderId !== turningEntry &&
        hoveredCards.has(id)
      ) {
        node.classList.add("hover-hold");
        held.push(node);
      }
    }
  }
  heldHoverNodes = held;
  alignMeters(list);
  slideMeters(list, meters);
  if (focused) {
    const field = list.querySelector(`[data-back-field="${focused}"]`);
    field?.focus({ preventScroll: true });
    if (field && selection) field.setSelectionRange?.(...selection);
  } else if (models) {
    [...list.querySelectorAll("[data-model-switch]")]
      .find((node) => node.dataset.modelSwitch === models)
      ?.focus({ preventScroll: true });
  }
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
        rect: window.CopoolReorder?.unscaledRect?.(node) ?? node.getBoundingClientRect(),
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
    // Its height can change, but the turning card already owns its transform.
    if (key === turningEntry) continue;
    // A card still rising or dropping keeps passing over the others.
    R.slide(
      node,
      old.rect,
      (activated && key === current) || old.node.classList.contains("reorder-raised"),
    );
    // An account joining a provider's group arrives inside it; a group
    // turned over shows its accounts again without them arriving.
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
/// A secret's ends with the middle hidden, short enough for one line.
function maskSecret(value) {
  const text = String(value);
  if (text.length <= 8) return "•".repeat(text.length);
  return `${text.slice(0, 3)}••••••••${text.slice(-4)}`;
}
/// Shows a value as text, or its placeholder dimmed; `copyValue` keeps the
/// real value for the copy button whatever is displayed.
function showValue(node, value, placeholder, masked = false) {
  node.dataset.copyValue = value || "";
  node.classList.toggle("empty", !value);
  node.classList.toggle("revealed", !!value && !masked);
  node.textContent = !value ? placeholder : masked ? maskSecret(value) : value;
  node.title = value && !masked ? value : "";
}
let proxyKeyRevealed = false;
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
  showValue(
    $("proxy-url"),
    proxy.baseUrl,
    tr("proxy.value.generated_after_start"),
  );
  showValue(
    $("proxy-key"),
    proxy.apiKey,
    tr("proxy.value.generated_after_first_start"),
    !proxyKeyRevealed,
  );
  $("toggle-key").disabled = !proxy.apiKey;
  // Only real values copy, as ProxyCopyableValueCard's canCopy.
  document.querySelector('[data-copy="proxy-url"]').disabled = !proxy.baseUrl;
  document.querySelector('[data-copy="proxy-key"]').disabled = !proxy.apiKey;
  $("proxy-active").textContent =
    proxy.activeAccountLabel || tr("proxy.info.no_request_matched");
  $("proxy-active-id").textContent =
    proxy.activeAccountId || tr("proxy.info.active_account_hint");
  $("proxy-error").textContent = tr(proxy.lastError || "common.none");
}
async function refreshView() {
  const controller = accountsController;
  const proxies = proxyController;
  const [, proxy, fullRefresh] = await Promise.all([
    controller.load(),
    proxies.invoke("proxy_status"),
    invoke("next_full_refresh_at", { pool }),
    refreshFeatureState(),
  ]);
  // A pool switched during the load renders through its own refresh.
  if (controller === accountsController) {
    fullRefreshAt = fullRefresh;
    // A newer load may have finished while the other requests were pending.
    accountsCache = controller.accounts;
    renderAccounts(accountsCache);
    await renderPendingWorkspaces();
  }
  // The proxy page switched pools meanwhile renders through its own refresh.
  proxies.applyLocal(
    proxy,
    proxies.localConfiguration(configurationCache),
    proxies.tunnelStatus,
  );
  syncButtons();
}
function applyTheme(value) {
  const theme = P.resolveTheme(value);
  if (theme === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
  const meta = document.querySelector('meta[name="color-scheme"]');
  if (meta) meta.content = theme === "system" ? "light dark" : theme;
  try {
    localStorage.setItem("copool.theme", theme);
  } catch {}
}
function syncGeneralSelectLabels() {
  for (const id of ["usage-display", "theme", "language"]) {
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
  $("theme").value = controls.themePicker.selectedValue;
  applyTheme(controls.themePicker.selectedValue);
  syncGeneralSelectLabels();
  $("restart-target").disabled = !controls.restartPicker.isEnabled;
  $("restart-target").value = controls.restartPicker.selectedValue || "";
  renderAgentSync();
  // An edit still waiting to be saved is not overwritten.
  if (document.activeElement !== $("proxy-port"))
    $("proxy-port").value = preferences[PROXY_PREFERENCES[proxyPool].port];
}
let preferenceSave = Promise.resolve();
/// Every save of the preferences runs in turn, each over the preferences the
/// previous one stored: each writes all of them.
/// Saves a patch after those queued before it; a function gives the patch
/// from the preferences as those left them.
function queuePreferenceSave(patch) {
  const save = preferenceSave.then(async () => {
    const value = typeof patch === "function" ? patch(preferences) : patch;
    preferences = await settingsController.savePreferences(preferences, value);
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
  // The proxy page chooses its pool beside its title.
  $("proxy-pool-switch").hidden = selectedPage !== "proxy";
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
/// Shows an account pool; each keeps its own controller state.
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
/// Selects the pool on both pages: the accounts and proxy pages show the
/// same upstream, but for Grok, which has no proxy.
function showPool(next) {
  if (!ACCOUNT_POOLS.includes(next)) return;
  showAccountsPool(next);
  if (PROXY_POOLS.includes(next)) showProxyPool(next);
}
function showAccountsPool(next) {
  document
    .querySelectorAll("#pool-switch .pool-btn")
    .forEach((button) =>
      button.setAttribute("aria-selected", String(button.dataset.pool === next)),
    );
  if (next === pool) return;
  // To the pool on the right, the cards leave to the left; and back. Only
  // the shown page slides: the cards are copied where they are on screen.
  const direction = ACCOUNT_POOLS.indexOf(next) > ACCOUNT_POOLS.indexOf(pool) ? -1 : 1;
  const animate = selectedPage === "accounts";
  if (animate) slidePoolOut(direction);
  // Another pool's cards are shown; none of them is being edited.
  editing = null;
  pool = next;
  fullRefreshAt = null;
  localStorage.setItem("copool.pool", pool);
  closeMenu();
  accountsController = poolControllers[pool];
  accountsCache = accountsController.accounts;
  renderAccounts(accountsCache);
  renderPendingWorkspaces();
  if (animate) slidePoolIn(direction);
  syncButtons();
  refreshView().catch((error) => notice(error, true));
}
document
  .querySelectorAll("#pool-switch .pool-btn")
  .forEach((button) => (button.onclick = () => showPool(button.dataset.pool)));
/// The proxy page's pool switch: the header's pool icons, each copy with
/// its own gradient, as `brandIcon` copies them.
for (const original of document.querySelectorAll("#pool-switch .pool-btn")) {
  if (!PROXY_POOLS.includes(original.dataset.pool)) continue;
  const copy = original.cloneNode(true);
  const target = copy.dataset.pool;
  delete copy.dataset.pool;
  copy.dataset.proxyPool = target;
  for (const gradient of copy.querySelectorAll("[id]")) {
    const id = `${gradient.id}-proxy`;
    for (const node of copy.querySelectorAll(`[fill="url(#${gradient.id})"]`))
      node.setAttribute("fill", `url(#${id})`);
    gradient.id = id;
  }
  copy.onclick = () => showPool(target);
  $("proxy-pool-switch").append(copy);
}
/// Shows the selected pool's API proxy; only showPool calls it.
function showProxyPool(next) {
  document
    .querySelectorAll("#proxy-pool-switch .pool-btn")
    .forEach((button) =>
      button.setAttribute(
        "aria-selected",
        String(button.dataset.proxyPool === next),
      ),
    );
  $("proxy-auto-start").dataset.preference = PROXY_PREFERENCES[next].autoStart;
  if (next === proxyPool && proxyController === proxyControllers[next]) return;
  // A port being typed is the pool left's.
  savePortNow();
  proxyPool = next;
  closeMenu();
  proxyController = proxyControllers[next];
  tunnelConfiguration = proxyController.tunnelConfiguration;
  proxyCache = proxyController.proxyStatus;
  if (preferences) {
    renderPreferences();
    // Shown even while focused, as the pool's own.
    $("proxy-port").value = preferences[PROXY_PREFERENCES[next].port];
  }
  renderProxy();
  renderRemotes();
  renderTunnelControls();
  syncButtons();
  proxyController
    .refresh()
    .then(() => proxyController.loadRemoteStatuses())
    .catch((error) => notice(error, true));
}
// A card's menu opens where it is right-clicked.
$("accounts").addEventListener("contextmenu", (event) => {
  if (event.target.closest(".account-back, input, select, textarea")) return;
  // A provider's account without a menu of its own has its provider's.
  let node = event.target.closest("[data-flip-key]");
  let target = node && flipTarget(node.dataset.flipKey);
  if (target?.grouped && !cardMenu(target).length) {
    node = node.parentElement.closest("[data-flip-key]");
    target = node && flipTarget(node.dataset.flipKey);
  }
  if (!target) return;
  event.preventDefault();
  showMenu(node, cardMenu(target), { x: event.clientX, y: event.clientY });
});
$("collapse-all").onclick = () => {
  // Collapsed cards have no settings side.
  editing = null;
  collapsed = !collapsed;
  renderAccounts(accountsCache);
  syncButtons();
};
// In-panel menus, as Coport's: they keep focus in the panel window, which a
// native menu would take away.
let openMenu = null;
let openSubmenu = null;
let submenuDismissTimer = null;
function cancelSubmenuDismiss() {
  if (submenuDismissTimer !== null) clearTimeout(submenuDismissTimer);
  submenuDismissTimer = null;
}
function deferSubmenuDismiss() {
  if (submenuDismissTimer !== null || !openSubmenu) return;
  const pending = openSubmenu;
  submenuDismissTimer = setTimeout(() => {
    submenuDismissTimer = null;
    if (openSubmenu === pending) closeSubmenu();
  }, 350);
}
// Entering a menu or returning from its child focuses the row without
// opening the child until the user navigates to it.
function focusMenuRow(row) {
  if (!row) return;
  const onfocus = row.onfocus;
  row.onfocus = null;
  row.focus();
  row.onfocus = onfocus;
}
function closeSubmenu(restoreFocus = false) {
  cancelSubmenuDismiss();
  if (!openSubmenu) return;
  const { menu, trigger } = openSubmenu;
  openSubmenu = null;
  menu.remove();
  trigger.setAttribute("aria-expanded", "false");
  if (restoreFocus) focusMenuRow(trigger);
}
function closeMenu(restoreFocus = false) {
  closeSubmenu();
  if (!openMenu) return;
  const { menu, trigger, at } = openMenu;
  openMenu = null;
  menu.remove();
  if (at) return;
  trigger.setAttribute("aria-expanded", "false");
  if (restoreFocus) trigger.focus();
}
// Parent and child share one navigation mode and one highlighted menu.
// Hover must not leave a previously focused row painted in either menu.
function configureMenuNavigation(menu, group = [menu]) {
  const activate = () => {
    for (const member of group) {
      if (member === menu) member.classList.remove("inactive-menu");
      else member.classList.add("inactive-menu");
    }
  };
  const pointer = () => {
    for (const member of group) member.classList.add("pointer-navigation");
    activate();
  };
  menu.addEventListener("pointerover", pointer);
  menu.addEventListener("pointermove", pointer);
  menu.addEventListener("focusin", activate);
  menu.addEventListener("keydown", () => {
    for (const member of group) member.classList.remove("pointer-navigation");
    activate();
  });
}
// Keep the parent aligned with its button. Fit the menu pair into the
// space to its right instead of shifting the parent away from its anchor.
function menuPairLayout(panel, parentWidth, submenuWidth, preferredLeft) {
  const left = Math.max(panel.left + 8, Math.min(preferredLeft, panel.right - 8));
  const available = Math.max(0, panel.right - 8 - left);
  const child = Math.floor(Math.min(submenuWidth, available * 0.45));
  const parent = Math.floor(Math.min(parentWidth, available - child + 1));
  return { left, parentWidth: parent, submenuWidth: child };
}
function importMenuLayout(panel, preferredLeft) {
  // Both pools share the toolbar anchor and parent width. Reserve only
  // a maximum for the child; its actual width follows its provider names.
  return menuPairLayout(panel, 200, 150, preferredLeft);
}
// Preserve the menu's measured width; move it inward instead of squeezing it.
function menuLeft(panel, width, preferredLeft) {
  return Math.max(panel.left + 8, Math.min(preferredLeft, panel.right - width - 8));
}
function showResetPopover(trigger, lines, wrapping = false) {
  if (openMenu?.trigger === trigger) {
    closeMenu(true);
    return;
  }
  closeMenu();
  const menu = el("div", `select-menu reset-popover${wrapping ? " wrapping" : ""}`);
  menu.setAttribute("role", "dialog");
  menu.setAttribute("aria-label", trigger.getAttribute("aria-label"));
  menu.tabIndex = -1;
  for (const line of lines) menu.append(el("div", "", line));
  document.body.append(menu);
  const bounds = trigger.getBoundingClientRect();
  const panel = document.querySelector(".panel").getBoundingClientRect();
  menu.style.maxWidth = `${Math.max(0, panel.right - panel.left - 16)}px`;
  const below = bounds.bottom + 4;
  const top = below + menu.offsetHeight <= panel.bottom - 8
    ? below : Math.max(panel.top + 8, bounds.top - menu.offsetHeight - 4);
  menu.style.left = `${menuLeft(panel, menu.offsetWidth, bounds.left)}px`;
  menu.style.top = `${top}px`;
  menu.style.maxHeight = `${panel.bottom - top - 8}px`;
  trigger.setAttribute("aria-expanded", "true");
  // Share outside-click, Escape, page-change and panel-hide dismissal.
  openMenu = { menu, trigger };
  menu.focus();
}

/// Shows a menu below `trigger`, or at the point `at` for a context menu.
/// Items may be separators, and the destructive one is red.
function showMenu(trigger, items, at = null) {
  closeMenu();
  const menu = el("div", "select-menu action-menu");
  const submenus = [];
  const navigationGroup = [menu];
  configureMenuNavigation(menu, navigationGroup);
  if (trigger.id === "import-open") menu.classList.add("import-menu");
  menu.setAttribute("role", "menu");
  // Reserve the checkmark gutter only when an item is actually checked.
  const choices = items.some((item) => item.selected || item.checked);
  menu.classList.toggle("choices", choices);
  for (const item of items) {
    if (item.separator) {
      const line = el("div", "menu-separator");
      line.setAttribute("role", "separator");
      menu.append(line);
      continue;
    }
    const option = el("button", item.destructive ? "destructive" : "", item.label);
    if (item.icon) option.prepend(item.icon);
    option.type = "button";
    option.disabled = !!item.disabled;
    option.setAttribute("role", "selected" in item ? "menuitemradio" : "menuitem");
    if ("selected" in item) option.setAttribute("aria-checked", item.selected ? "true" : "false");
    if ("checked" in item) {
      option.setAttribute("role", "menuitemcheckbox");
      option.setAttribute("aria-checked", String(item.checked));
    }
    menu.append(option);
    if (item.children) {
      const submenu = el("div", "select-menu action-menu provider-submenu");
      navigationGroup.push(submenu);
      configureMenuNavigation(submenu, navigationGroup);
      if (trigger.id === "import-open") submenu.classList.add("import-menu");
      submenu.addEventListener("pointerenter", cancelSubmenuDismiss);
      submenu.addEventListener("focusin", cancelSubmenuDismiss);
      submenu.setAttribute("role", "menu");
      submenu.setAttribute("aria-label", item.label);
      option.setAttribute("aria-haspopup", "menu");
      option.setAttribute("aria-expanded", "false");
      const expand = () => {
        cancelSubmenuDismiss();
        if (openSubmenu?.trigger === option) return;
        closeSubmenu();
        document.body.append(submenu);
        const bounds = option.getBoundingClientRect();
        const parent = menu.getBoundingClientRect();
        const panel = document.querySelector(".panel").getBoundingClientRect();
        const right = parent.right - 1;
        const left = right + submenu.offsetWidth <= panel.right - 8
          ? right
          : parent.left - submenu.offsetWidth + 1;
        // Align to the parent row in both pools; long lists scroll below it.
        const top = Math.max(panel.top + 8,
          Math.min(bounds.top - 4, panel.bottom - 32));
        submenu.style.left = `${menuLeft(panel, submenu.offsetWidth, left)}px`;
        submenu.style.top = `${top}px`;
        submenu.style.maxHeight = `${panel.bottom - top - 8}px`;
        openSubmenu = { menu: submenu, trigger: option };
        option.setAttribute("aria-expanded", "true");
      };
      option.onpointerenter = expand;
      option.onfocus = expand;
      option.onclick = () => {
        expand();
        submenu.querySelector("button:not(:disabled)")?.focus();
      };
      option.onkeydown = (event) => {
        if (event.key !== "ArrowRight") return;
        event.preventDefault();
        option.onclick();
      };
      for (const child of item.children) {
        const button = el("button", "", child.label);
        if (child.icon) button.prepend(child.icon);
        button.type = "button";
        button.setAttribute("role", "menuitem");
        button.disabled = !!child.disabled;
        button.onclick = () => {
          closeMenu(true);
          child.run();
        };
        submenu.append(button);
      }
      submenus.push(submenu);
      submenu.onkeydown = (event) => {
        if (!["ArrowLeft", "Escape"].includes(event.key)) return;
        event.preventDefault();
        event.stopPropagation();
        closeSubmenu(true);
      };
    } else {
      option.onclick = () => {
        closeMenu(true);
        item.run();
      };
    }
  }
  // Padding is the bridge between the two menus, not another choice.
  // Give diagonal pointer travel across adjacent rows time to reach the child.
  menu.addEventListener("pointerover", (event) => {
    const row = event.target.closest("button");
    if (!openSubmenu) return;
    if (!row || row === openSubmenu.trigger) cancelSubmenuDismiss();
    else deferSubmenuDismiss();
  });
  menu.addEventListener("focusin", (event) => {
    const row = event.target.closest("button");
    if (openSubmenu && row && row !== openSubmenu.trigger) closeSubmenu();
  });
  document.body.append(menu);
  // Below the trigger, or above it when the panel is too short; kept inside.
  const bounds = at ? new DOMRect(at.x, at.y, 0, 0) : trigger.getBoundingClientRect();
  const panel = document.querySelector(".panel").getBoundingClientRect();
  // Freeze the intrinsic width before assigning left: positioning must not
  // change the measured width (especially for cards along the right edge).
  menu.style.maxWidth = `${Math.max(0, panel.right - panel.left - 16)}px`;
  menu.style.width = `${menu.offsetWidth}px`;
  let left = menuLeft(panel, menu.offsetWidth, bounds.left);
  if (submenus.length && trigger.id === "import-open") {
    const layout = importMenuLayout(panel, bounds.left);
    menu.style.minWidth = "0";
    menu.style.width = `${layout.parentWidth}px`;
    for (const submenu of submenus) {
      submenu.style.minWidth = "0";
      submenu.style.maxWidth = `${layout.submenuWidth}px`;
    }
    left = layout.left;
  }
  // Other card menus keep their normal width; expand() opens the child to
  // the left when the trigger is near the panel's right edge.

  const below = bounds.bottom + 4;
  const top =
    below + menu.offsetHeight <= panel.bottom - 8
      ? below
      : Math.max(panel.top + 8, bounds.top - menu.offsetHeight - 4);
  menu.style.left = `${Math.max(8, left)}px`;
  menu.style.top = `${top}px`;
  menu.style.maxHeight = `${panel.bottom - top - 8}px`;
  if (!at) trigger.setAttribute("aria-expanded", "true");
  openMenu = { menu, trigger, at };
  focusMenuRow(
    menu.querySelector("[aria-checked=true]") || menu.querySelector("button:not(:disabled)")
  );
}
document.addEventListener("pointerdown", (event) => {
  if (
    openMenu &&
    !openMenu.menu.contains(event.target) &&
    !openSubmenu?.menu.contains(event.target) &&
    (openMenu.at || !openMenu.trigger.contains(event.target))
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
  const items = [
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
  ];
  const providers = providerSyncChoices(pool);
  if (providers.length) {
    items.push({
      label: tr("accounts.action.import_sub2api"),
      children: providers,
    });
  }
  showMenu($("import-open"), items);
};
// Opened by keyboard navigation of the menu bar (macOS 27); see menuBarStep.
let menuBarNavigation = false;
// Where ←/→ move the caret.
const TEXT_ENTRY =
  'textarea, [contenteditable]:not([contenteditable="false"]), input:not([type="checkbox"]):not([type="radio"]):not([type="button"]):not([type="submit"]):not([type="reset"])';
// Interactive controls that own Return when focused (e.g. via Tab).
const INTERACTIVE_ELEMENT =
  'a[href], button, input, select, textarea, summary, [contenteditable]:not([contenteditable="false"]), [role="button"], [role="link"], [role="menuitem"], [role="tab"], [role="switch"], [role="checkbox"], [role="radio"], [role="combobox"], [role="option"], [role="textbox"], [tabindex]:not([tabindex="-1"])';
/// Whether Return should act on a focused UI element rather than a hovered card.
function focusOwnsEnter(event) {
  if (event.target?.closest?.(INTERACTIVE_ELEMENT)) return true;
  const active = document.activeElement;
  return Boolean(
    active &&
      active !== document.body &&
      active !== document.documentElement &&
      active.closest?.(INTERACTIVE_ELEMENT),
  );
}
/// The hovered account card (or provider group) that Return can turn over to
/// edit, or null when none is hovered, collapsed, already turned over, or has
/// no settings side.
function hoveredEditableCard() {
  if (selectedPage !== "accounts" || collapsed) return null;
  if (typeof reorderingAccounts !== "undefined" && reorderingAccounts) return null;
  const list = document.getElementById?.("accounts");
  if (!list) return null;
  const hovered = [...list.querySelectorAll("[data-flip-key]:is(:hover, .hover-hold)")];
  // Innermost first: a grouped account with its own back wins over its group,
  // while one without a back falls through to its group (as contextmenu does).
  for (let i = hovered.length - 1; i >= 0; i--) {
    const node = hovered[i];
    const key = node.dataset.flipKey;
    if (node.classList?.contains("account-back") || editing?.key === key || pendingEdit === key) {
      return null;
    }
    if (hasBack(flipTarget(key))) return key;
  }
  return null;
}
// Coport's panel shortcuts. An open dialog keeps Escape and Return for
// cancel and submit, as the SwiftUI dialogs did.
document.addEventListener("keydown", (event) => {
  if (document.querySelector("dialog[open]")) return;
  if (openMenu) {
    const activeMenu = openSubmenu?.menu.contains(document.activeElement)
      ? openSubmenu.menu : openMenu.menu;
    const items = [...activeMenu.querySelectorAll("button")].filter(
      (item) => !item.disabled && !item.closest("[hidden]"),
    );
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
      items[next]?.focus();
    }
    return;
  }
  const step = P.menuBarStep(event.key, {
    navigating: menuBarNavigation,
    modified: event.metaKey || event.ctrlKey || event.altKey || event.shiftKey,
    inText: Boolean(event.target.closest?.(TEXT_ENTRY)),
    handled: event.defaultPrevented || event.isComposing,
  });
  if (step) {
    event.preventDefault();
    invoke("navigate_menu_bar", { right: step === "right" }).catch(() => {});
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
    if (event.defaultPrevented || event.isComposing || event.keyCode === 229) return;
    event.preventDefault();
    // Holding Escape through a card flip must not dismiss the panel too.
    if (event.repeat) return;
    // Back from a subpage first; on the accounts page, turn an edited card
    // back without saving, else dismiss like a menu.
    if (selectedPage !== "accounts") showPage("accounts");
    else if (editing || pendingEdit) cancelEditing();
    else invoke("hide_panel").catch(() => {});
  } else if (
    event.key === "Enter" &&
    !event.defaultPrevented &&
    !event.isComposing &&
    event.keyCode !== 229 &&
    !event.repeat &&
    !event.metaKey &&
    !event.ctrlKey &&
    !event.altKey &&
    !event.shiftKey &&
    !focusOwnsEnter(event)
  ) {
    const key = hoveredEditableCard();
    if (key) {
      event.preventDefault();
      editCard(key);
    }
  } else if (back) {
    event.preventDefault();
    if (selectedPage !== "accounts") showPage("accounts");
  } else if (mod && event.key === "q") {
    event.preventDefault();
    settingsController.quit().catch((error) => notice(error, true));
  } else if (mod && !event.altKey && !event.shiftKey && ["1", "2", "3", "4"].includes(event.key)) {
    event.preventDefault();
    const target = ACCOUNT_POOLS[Number(event.key) - 1];
    // The pool is shared, so the proxy page switches in place; Grok's
    // accounts have no proxy page.
    if (
      (selectedPage !== "proxy" || !PROXY_POOLS.includes(target)) &&
      selectedPage !== "accounts"
    )
      showPage("accounts");
    showPool(target);
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
/// Refreshes one account, or all without an id. As in the Swift app this
/// takes no panel-wide lock: each card updates and becomes operable again as
/// its own refresh lands ("copool-changed"), while the others stay usable.
function refreshUsage(id = null) {
  const provider = id === null ? null : providerKey({ id, sourceTag: "SUB2API" });
  if (provider !== null) return refreshProvider({ key: provider, accounts: [{ id }] });
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
          {
            claude: "accounts.notice.claude_provider_switched_format",
            antigravity: "accounts.notice.antigravity_provider_switched_format",
            grok: "accounts.notice.grok_provider_switched_format",
          }[pool] || "accounts.notice.provider_switched_format",
          execution.provider,
        )
      : tr(
          {
            claude: "accounts.notice.claude_switch_done",
            antigravity: "accounts.notice.antigravity_switch_done",
            grok: "accounts.notice.grok_switch_done",
          }[pool] || "accounts.notice.switch_done",
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
  for (const [synced, syncError, doneKey, failedKey] of [
    [
      "opencodeSynced",
      "opencodeSyncError",
      "accounts.notice.sync_done",
      "accounts.notice.sync_failed_format",
    ],
    [
      "piSynced",
      "piSyncError",
      "accounts.notice.pi_sync_done",
      "accounts.notice.pi_sync_failed_format",
    ],
    [
      "ompSynced",
      "ompSyncError",
      "accounts.notice.omp_sync_done",
      "accounts.notice.omp_sync_failed_format",
    ],
  ]) {
    if (execution[syncError]) {
      error = true;
      segments.push(tr(failedKey, execution[syncError]));
    } else if (execution[synced]) segments.push(tr(doneKey));
  }
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
      await invoke("switch_account", { pool, id: account.id }),
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
    const outcome = await invoke("smart_switch", { pool });
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
// The preferred port is saved as it is edited, as updatePreferredPortText,
// as the port of the pool it was typed for.
let portSaveTimer;
let pendingPortSave = null;
function savePortNow() {
  clearTimeout(portSaveTimer);
  const save = pendingPortSave;
  pendingPortSave = null;
  if (save) save();
}
$("proxy-port").oninput = () => {
  clearTimeout(portSaveTimer);
  pendingPortSave = null;
  const port = Number($("proxy-port").value);
  if (!Number.isInteger(port) || port < 1 || port > 65535) return;
  const key = PROXY_PREFERENCES[proxyPool].port;
  pendingPortSave = () =>
    queuePreferenceSave({ [key]: port }).catch((error) => notice(error, true));
  portSaveTimer = setTimeout(savePortNow, 350);
};
$("toggle-key").onclick = () => {
  proxyKeyRevealed = !proxyKeyRevealed;
  $("toggle-key").setAttribute("aria-pressed", String(proxyKeyRevealed));
  renderProxy();
};
document.querySelectorAll(".copy").forEach(
  (button) =>
    (button.onclick = () => {
      const value = $(button.dataset.copy).dataset.copyValue;
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
$("theme").onchange = () => {
  syncGeneralSelectLabels();
  applyTheme($("theme").value);
  savePreference({ theme: $("theme").value });
};
$("language").onchange = () => {
  syncGeneralSelectLabels();
  savePreference({ locale: $("language").value });
};
$("quit").onclick = () =>
  settingsController.quit().catch((error) => notice(error, true));
document.addEventListener(
  "DOMContentLoaded",
  async () => {
    try {
      [preferences, nativeIcons] = await Promise.all([
        invoke("preferences"),
        invoke("native_symbols"),
      ]);
      applyNativeIcons();
      showAccountsPool(pool);
      showProxyPool(proxyPool);
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
setInterval(refreshVisibleMeterPace, 1000);
document.addEventListener("visibilitychange", refreshVisibleMeterPace);
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
    menuBarNavigation = false;
    panelHiddenAt = Date.now();
    clearHoverHold();
    closeMenu();
  })
  .catch((error) => notice(error, true));
window.__TAURI__.event
  .listen("panel-shown", ({ payload } = {}) => {
    menuBarNavigation = Boolean(payload?.keyboard);
    // Keys start on the panel, not in a text field left focused, where ←/→
    // would not move along the menu bar.
    if (menuBarNavigation && document.activeElement?.closest?.(TEXT_ENTRY))
      document.activeElement.blur();
    if (panelHiddenAt && Date.now() - panelHiddenAt > PANEL_PAGE_TIMEOUT_MS)
      showPage("accounts", false);
    panelHiddenAt = null;
    refreshVisibleMeterPace();
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
