"use strict";
const $ = (id) => document.getElementById(id);
const invoke = (command, args = {}) =>
  window.__TAURI__.core.invoke(command, args);
const P = window.CopoolPresentation;
const V = window.CopoolCardView;
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
const { ACCOUNT_POOLS, PROXY_POOLS, PROXY_PREFERENCES } = P;
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
// Reset times as the date and time or as the time left, as OpenQuota toggles
// them by clicking either.
let resetDisplay =
  localStorage.getItem("copool.resetDisplay") === "countdown"
    ? "countdown"
    : "exact";
/// The commands of one pool (controllers.js).
const poolInvoke = (target) => window.CopoolControllers.poolInvoke(invoke, target);
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
// In-panel menus (menus.js).
const menus = window.CopoolMenus.create({ document, el });
const showMenu = menus.show;
const closeMenu = menus.close;
const showResetPopover = menus.showResetPopover;
// A card's back and the turns to it and back (card-back.js).
const cardBacks = window.CopoolCardBack.create({
  el,
  button,
  tr,
  get locale() {
    return locale;
  },
  get pool() {
    return pool;
  },
  get preferences() {
    return preferences;
  },
  get controller() {
    return accountsController;
  },
  get collapsed() {
    return collapsed;
  },
  cardContext: () => cardContext(),
  providerTitle: (account) => providerTitle(account),
  turnCard: (key, direction, change) => turnCard(key, direction, change),
  renderEditing: () => renderEditing(),
  focusBackField: (field) =>
    $("accounts").querySelector(`[data-back-field="${field}"]`)?.focus(),
  notice: (message, error) => notice(message, error),
  action: (task) => action(task),
  queuePreferenceSave: (patch) => queuePreferenceSave(patch),
  renewNow: (account) => renewNow(account),
  greet: (account) => sayHi(account).then(refreshView).catch((error) => notice(error, true)),
  removeCards: (cards, sub2api) => removeCards(cards, sub2api),
});
const { cardBack, editCard, cancelEditing } = cardBacks;
// A card's front (card-face.js).
const cardFaces = window.CopoolCardFace.create({
  el,
  button,
  icon,
  tr,
  get locale() {
    return locale;
  },
  get pool() {
    return pool;
  },
  get collapsed() {
    return collapsed;
  },
  get resetDisplay() {
    return resetDisplay;
  },
  get preferences() {
    return preferences;
  },
  get controller() {
    return accountsController;
  },
  get switchingId() {
    return switchingId;
  },
  cardContext: () => cardContext(),
  providerTitle: (account) => providerTitle(account),
  accountProxy: (account) => accountProxy(account),
  cardBack: (target) => cardBack(target),
  modelBrand: (model) => modelBrand(model),
  showResetPopover: (trigger, lines, wrapping) => showResetPopover(trigger, lines, wrapping),
  toggleResetDisplay: () => toggleResetDisplay(),
  getComputedStyle: (node) => getComputedStyle(node),
  reducedMotion: () => !!window.matchMedia?.("(prefers-reduced-motion: reduce)").matches,
  notice: (message, error) => notice(message, error),
  signIn: (workspace, kind) => signIn(workspace, kind),
  editCard: (key) => editCard(key),
  refreshUsage: (id) => refreshUsage(id),
  refreshProvider: (group) => refreshProvider(group),
  greet: (account) => sayHi(account).then(refreshView).catch((error) => notice(error, true)),
  renewNow: (account) => renewNow(account),
  switchAccount: (account) => switchAccount(account),
  confirmRemoval: (cards, name, sub2api) => confirmRemoval(cards, name, sub2api),
});
const { accountCard, providerGroup, cardMenu, refreshTooltip, alignMeters, cardMeters, slideMeters } = cardFaces;
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
/// Shows a notice: done (green), an error (`tone` true, red) or a warning
/// (`tone` "warning", yellow: nothing failed, but nothing came of it).
function notice(message, tone = false) {
  const error = tone === true;
  clearTimeout(noticeTimer);
  $("notice-text").textContent = tr(
    message instanceof Error ? message.message : String(message),
  );
  $("notice").classList.toggle("error", error);
  $("notice").classList.toggle("warning", tone === "warning");
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
function usageMode() {
  return preferences?.usageDisplay || "used";
}
/// The context card-view.js decides the cards in: the pool shown, the
/// display preferences and what each card has under way.
function cardContext(now = Date.now() / 1000) {
  const controller = accountsController;
  return {
    pool,
    mode: usageMode(),
    resetDisplay,
    collapsed,
    locale,
    tr,
    now,
    accountProxy: (preferences?.accountProxyUrl || "").trim(),
    switchingId,
    adding: controller.adding,
    authorizing: controller.authorizing,
    ui: (id) => controller.cardState(id),
  };
}
function toggleResetDisplay() {
  resetDisplay = resetDisplay === "countdown" ? "exact" : "countdown";
  localStorage.setItem("copool.resetDisplay", resetDisplay);
  renderAccounts(accountsCache);
}
function refreshVisibleMeterPace() {
  if (document.hidden || $("accounts-page").hidden) return;
  cardFaces.refreshMeterPaces($("accounts"), Date.now() / 1000);
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
/// The provider of a Sub2api card (presentation.js).
const providerKey = P.providerKey;
/// The title of a Sub2api card's provider (controllers.js).
function providerTitle(account) {
  return providerControllers[pool].cardTitle(account);
}
/// The proxy an account's requests go through: its own, which for a Sub2api
/// card is its provider's, else the default account proxy; blank for the
/// system settings, "direct://" for none.
function accountProxy(account) {
  let proxy = (account.proxyUrl || "").trim();
  if (!P.isProviderCard(account))
    proxy ||= (preferences?.accountProxyUrl || "").trim();
  return proxy;
}
/// Removes an account, or all of an unofficial provider's cards at once.
async function removeCards(cards, provider) {
  const providerRemoved = {
    cliproxyapi: "accounts.notice.cliproxyapi_removed",
    sub2api: "accounts.notice.sub2api_removed",
  };
  const removed = provider
    ? providerRemoved[cards[0]?.providerKind] || providerRemoved.sub2api
    : "accounts.notice.account_deleted";
  await action(() => accountsController.removeAccounts(cards.map((a) => a.id)), tr(removed));
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
/// Renews the account's tokens at once (the controller's renewNow); its
/// refresh button shows the renewal under way, then done, and a notice
/// tells how it went, as saying hi does.
async function renewNow(account) {
  try {
    if (await accountsController.renewNow(account.id))
      notice(tr("accounts.notice.token_renewed"));
  } catch (error) {
    notice(error, true);
  }
}
/// Says hi with the account (the controller's sayHi), then tells whether
/// its 5h window started.
async function sayHi(account) {
  const controller = accountsController;
  const outcome = await controller.sayHi(account.id);
  if (outcome === "started") notice(tr("accounts.notice.said_hi"));
  else if (outcome === "not_started")
    notice(tr(`error.${controller.pool}.say_hi_window_not_started`), true);
}
// Each half of a turn: edge-on at its middle, where the sides swap.
const FLIP_HALF_MS = 210;
// The list entry whose face is changing during this render.
let turningEntry = null;
/// Renders the card being edited again, as its settings change.
function renderEditing() {
  renderedAccountsState = undefined;
  renderAccounts(accountsCache);
}
/// The space kept at the end of the list while a card is edited (holdInPlace).
let editingSpace = 0;
function keepEditingSpace(px) {
  editingSpace = px;
  $("accounts").style.paddingBottom = px ? `${px}px` : "";
}
/// Where an entry stands in the list's view, by its layout alone: neither the
/// turn nor a hovered card's lift moves it.
function viewTop(entry) {
  return entry.offsetTop - $("accounts-page").scrollTop;
}
/// Scrolls the list so that `holder`, the entry of a card just turned, stands
/// where it stood before the turn (`top`, its viewTop then). A
/// back shorter than its front, as a provider's group has, near the end of a
/// scrolled list would otherwise pull the list up to its new end and the card
/// with it: while a card is edited, the list keeps the space it needs at its
/// end.
function holdInPlace(holder, top) {
  const scroller = $("accounts-page");
  const shift = viewTop(holder) - top;
  if (!shift) return;
  const end = scroller.scrollHeight - scroller.clientHeight;
  const held = P.heldScroll(shift, scroller.scrollTop, end, !!accountsController.editing);
  if (held.space) keepEditingSpace(editingSpace + held.space);
  scroller.scrollTop = held.scrollTop;
}
/// Turns the card of `key` edge-on, makes `change`, renders, and turns the
/// card's other side in, growing it as the cards below slide down. The card
/// stays where it stood (holdInPlace).
async function turnCard(key, direction, change) {
  const list = $("accounts");
  const find = () =>
    [...list.querySelectorAll("[data-flip-key]")].find((n) => n.dataset.flipKey === key);
  const still = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
  const node = find();
  const entry = node?.closest("[data-reorder-id]");
  const before = entry?.offsetHeight;
  const top = entry && viewTop(entry);
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
  const holder = next.closest("[data-reorder-id]");
  if (holder && top !== undefined) holdInPlace(holder, top);
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
  // Only an edited card keeps space at the list's end (holdInPlace).
  if (editingSpace && !accountsController.editing) keepEditingSpace(0);
  const renderState = cardFaces.renderState(accounts);
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
  const {
    accounts,
    answers: [proxy, fullRefresh],
  } = await controller.loadAlong([
    proxies.invoke("proxy_status"),
    invoke("next_full_refresh_at", { pool }),
    refreshFeatureState(),
  ]);
  // A pool switched during the load renders through its own refresh.
  if (controller === accountsController) {
    fullRefreshAt = fullRefresh;
    accountsCache = accounts;
    renderAccounts(accountsCache);
    await renderPendingWorkspaces();
  }
  // The proxy page switched pools meanwhile renders through its own refresh.
  // The cache, as it was asked for: a tunnel saved since leaves it out.
  proxies.applyLocal(
    proxy,
    proxies.localConfiguration(configurationCache),
    proxies.tunnelStatus,
    configurationReads[proxies.pool] ?? -1,
  );
  syncButtons();
}
/// Applies a theme as presentation.js's themeAttributes has it.
function applyTheme(value) {
  const theme = P.themeAttributes(value);
  if (theme.dataTheme === null) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme.dataTheme;
  const meta = document.querySelector('meta[name="color-scheme"]');
  if (meta) meta.content = theme.colorScheme;
  try {
    localStorage.setItem("copool.theme", theme.saved);
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
/// Selects the pool on both pages (presentation.js's poolSelection).
function showPool(next) {
  const shown = P.poolSelection(next);
  if (!shown) return;
  showAccountsPool(shown.accounts);
  if (shown.proxy) showProxyPool(shown.proxy);
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
  accountsController.stopEditing();
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
  portSave.flush();
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
  let target = node && accountsController.flipTarget(node.dataset.flipKey);
  if (target?.grouped && !cardMenu(target).length) {
    node = node.parentElement.closest("[data-flip-key]");
    target = node && accountsController.flipTarget(node.dataset.flipKey);
  }
  if (!target) return;
  event.preventDefault();
  showMenu(node, cardMenu(target), { x: event.clientX, y: event.clientY });
});
$("collapse-all").onclick = () => {
  // Collapsed cards have no settings side.
  accountsController.stopEditing();
  collapsed = !collapsed;
  renderAccounts(accountsCache);
  syncButtons();
};
document.addEventListener("pointerdown", (event) => menus.dismissOutside(event.target));
// On Linux a <select>'s popup is a GTK menu outside the panel's frosted
// material; the in-panel menu replaces it there.
if (document.documentElement.classList.contains("linux")) {
  const showSelectMenu = (select) => {
    if (menus.isOpenFor(select)) {
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
      if (!select || select.disabled || menus.isOpen) return;
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
  if (menus.isOpenFor($("import-open"))) {
    closeMenu(true);
    return;
  }
  const target = pool;
  showMenu(
    $("import-open"),
    P.importMenu(tr, {
      importCurrent: () =>
        importAccounts(
          () => accountsController.importCurrent(),
          "accounts.notice.imported_format",
        ),
      importFile: () => importAccountFile(),
      providers: providerControllers[target].syncChoices((key) => syncProvider(target, key)),
    }),
  );
};
// Opened by keyboard navigation of the menu bar (macOS 27); see menuBarStep.
let menuBarNavigation = false;
// Coport's panel shortcuts (keys.js).
const keys = window.CopoolKeys.create({
  document,
  mac: navigator.userAgent.includes("Mac"),
  menus,
  get page() {
    return selectedPage;
  },
  get collapsed() {
    return collapsed;
  },
  get reordering() {
    return reorderingAccounts;
  },
  get pool() {
    return pool;
  },
  get controller() {
    return accountsController;
  },
  get menuBarNavigation() {
    return menuBarNavigation;
  },
  invoke: (command, args) => invoke(command, args),
  showPage: (page) => showPage(page),
  showPool: (next) => showPool(next),
  editCard: (key) => editCard(key),
  cancelEditing: () => cancelEditing(),
  quit: () => settingsController.quit().catch((error) => notice(error, true)),
  click: (id) => $(id).click(),
});
document.addEventListener("keydown", keys.onKeydown);
/// Refreshes one account, or all without an id. As in the Swift app this
/// takes no panel-wide lock: each card updates and becomes operable again as
/// its own refresh lands ("copool-changed"), while the others stay usable.
function refreshUsage(id = null) {
  // A Sub2api provider's card refreshes with its provider's (controllers.js).
  accountsController
    .refreshCard(id)
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
/// A switch's notice, in the pool it was asked for (presentation.js).
const switchNotice = (execution, from) => P.switchNotice(execution, from, tr);
/// A switch locks only its own card's switch button and Smart Switch.
async function switchAccount(account) {
  if (switchingId !== null) return;
  switchingId = account.id;
  renderAccounts(accountsCache);
  syncButtons();
  const from = pool;
  try {
    const result = switchNotice(
      await invoke("switch_account", { pool: from, id: account.id }),
      from,
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
  const from = pool;
  try {
    const outcome = await invoke("smart_switch", { pool: from });
    if (outcome.kind === "noTarget")
      notice(tr("accounts.notice.no_switch_target"));
    else if (outcome.kind === "alreadyBest")
      notice(tr("accounts.notice.already_best"));
    else {
      const result = switchNotice(outcome.execution, from);
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
// as the port of the pool it was typed for (controllers.js's PendingSave).
const portSave = new window.CopoolControllers.PendingSave();
$("proxy-port").oninput = () => {
  portSave.cancel();
  const patch = P.portPatch(proxyPool, $("proxy-port").value);
  if (!patch) return;
  portSave.schedule(proxyPool, () =>
    queuePreferenceSave(patch).catch((error) => notice(error, true)),
  );
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
    if (menuBarNavigation && document.activeElement?.closest?.(window.CopoolKeys.TEXT_ENTRY))
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
