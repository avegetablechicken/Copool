/* Pure presentation rules ported from Swift. Used by the UI and node:test. */
(function (root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.CopoolPresentation = api;
})(globalThis, function () {
  const locales = [
    "en",
    "zh-Hans",
    "zh-Hant",
    "ja",
    "ko",
    "fr",
    "de",
    "it",
    "es",
    "ru",
    "nl",
  ];
  const clean = (value) =>
    typeof value === "string" && value.trim() ? value.trim() : null;
  function formatLocalized(template, args = []) {
    let cursor = 0;
    return template.replace(
      /%%|%(?:(\d+)\$)?(?:ll|l)?([@diuf])/g,
      (match, position) => {
        if (match === "%%") return "%";
        const index = position ? Number(position) - 1 : cursor++;
        return index < args.length ? String(args[index]) : match;
      },
    );
  }
  function resolveLocale(value) {
    const v = String(value ?? "")
      .trim()
      .toLowerCase()
      .replaceAll("_", "-");
    if (/^zh-(hant|tw|hk|mo)/.test(v)) return "zh-Hant";
    if (v.startsWith("zh")) return "zh-Hans";
    return locales.find((locale) => v.startsWith(locale)) || "en";
  }
  function preferredLocale(values) {
    for (const value of values) {
      const locale = resolveLocale(value);
      if (
        locale !== "en" ||
        String(value).trim().toLowerCase().startsWith("en")
      )
        return locale;
    }
    return "en";
  }
  function normalizedPlan(account) {
    const raw =
      typeof account === "object" && account !== null
        ? clean(account.usage?.planType) || clean(account.planType)
        : clean(account);
    const p = (raw || "team").toUpperCase();
    // Claude's Max plans keep their usage multiplier ("MAX 20X").
    if (/^MAX( \d+X)?$/.test(p)) return p;
    return ["PRO", "PLUS", "FREE", "BUSINESS", "ENTERPRISE"].includes(p)
      ? p
      : "TEAM";
  }
  function workspaceTag(account) {
    return (
      clean(account.sourceTag) ||
      (["TEAM", "BUSINESS", "ENTERPRISE"].includes(normalizedPlan(account))
        ? clean(account.teamAlias) || clean(account.teamName)
        : null)
    );
  }
  function displayPercent(window, mode = "used", missing = null) {
    if (!window || !Number.isFinite(window.usedPercent)) return missing;
    const used = Math.max(0, Math.min(100, window.usedPercent));
    return mode === "remaining" ? 100 - used : used;
  }
  function formatReset(epoch, locale = "en", timeZone) {
    if (epoch == null) return "--";
    const date = new Date(epoch * 1000),
      language = resolveLocale(locale);
    const options = timeZone ? { timeZone } : {};
    return (
      new Intl.DateTimeFormat(language, {
        dateStyle: "short",
        ...options,
      }).format(date) +
      ", " +
      new Intl.DateTimeFormat(language, {
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
        hourCycle: "h23",
        ...options,
      }).format(date)
    );
  }
  /// Time left on the access token in its largest whole unit (minutes round
  /// up), or expired; null without a known expiry.
  function tokenExpiry(epoch, now, locale = "en") {
    // As the core's displayed_expiry: seconds from 1970 to 9999-12-31, so a
    // date always formats.
    if (!Number.isFinite(epoch) || epoch <= 0 || epoch > 253402300799)
      return null;
    const seconds = epoch - now;
    if (seconds <= 0) return { expired: true, remaining: null };
    const [unit, amount] =
      seconds >= 86400
        ? ["day", Math.floor(seconds / 86400)]
        : seconds >= 3600
          ? ["hour", Math.floor(seconds / 3600)]
          : ["minute", Math.ceil(seconds / 60)];
    return {
      expired: false,
      remaining: new Intl.NumberFormat(resolveLocale(locale), {
        style: "unit",
        unit,
        unitDisplay: "narrow",
      }).format(amount),
    };
  }
  /// Time left until a reset as OpenQuota shows it: days and hours, hours
  /// and minutes, or minutes (rounded up), localized; soon within five
  /// minutes or once past; null without a reset time.
  function resetCountdown(epoch, now, locale = "en") {
    if (!Number.isFinite(epoch)) return null;
    const seconds = epoch - now;
    if (seconds <= 5 * 60) return { soon: true, remaining: null };
    const minutes = Math.ceil(seconds / 60),
      days = Math.floor(minutes / 1440),
      hours = Math.floor((minutes % 1440) / 60),
      rest = minutes % 60,
      language = resolveLocale(locale);
    const parts = (
      days > 0
        ? [["day", days], ["hour", hours]]
        : hours > 0
          ? [["hour", hours], ["minute", rest]].filter(([, n], i) => !i || n)
          : [["minute", rest]]
    ).map(([unit, amount]) =>
      new Intl.NumberFormat(language, {
        style: "unit",
        unit,
        unitDisplay: "narrow",
      }).format(amount),
    );
    return {
      soon: false,
      remaining: new Intl.ListFormat(language, {
        type: "unit",
        style: "narrow",
      }).format(parts),
    };
  }
  function windowPresentation(
    window,
    mode = "used",
    formatDate = (value) => formatReset(value),
  ) {
    const used = Math.round(displayPercent(window, "used", 100));
    return {
      // A missing window (e.g. no five-hour limit) has no data to show.
      available: displayPercent(window) !== null,
      progressPercent: mode === "remaining" ? 100 - used : used,
      primaryKind: mode,
      primaryPercent: mode === "remaining" ? 100 - used : used,
      secondaryPercent: mode === "remaining" ? used : 100 - used,
      reset: window?.resetAt != null ? formatDate(window.resetAt) : "--",
      resetAt: window?.resetAt ?? null,
    };
  }
  function card(account, collapsed = false, mode = "used", formatDate) {
    const plan = normalizedPlan(account),
      name =
        clean(account.email) || clean(account.accountId) || clean(account.label) || "",
      credits = account.usage?.credits;
    return {
      plan,
      accent:
        {
          PRO: "orange",
          PLUS: "pink",
          FREE: "gray",
          BUSINESS: "indigo",
          ENTERPRISE: "indigo",
        }[plan] || "teal",
      workspaceTag: workspaceTag(account),
      name:
        collapsed && name.includes("@")
          ? name.slice(0, name.indexOf("@"))
          : name,
      status:
        account.workspaceStatus === "deactivated" || account.available === false
          ? "deactivated"
          : null,
      credits: credits?.unlimited
        ? "unlimited"
        : credits?.balance == null
          ? "--"
          : Number.isFinite(Number(credits.balance))
            ? String(Math.round(Number(credits.balance)))
            : credits.balance,
      fiveHour: windowPresentation(account.usage?.fiveHour, mode, formatDate),
      oneWeek: windowPresentation(account.usage?.oneWeek, mode, formatDate),
      compact: [
        displayPercent(account.usage?.fiveHour, mode),
        displayPercent(account.usage?.oneWeek, mode),
      ],
      selectionAccent: account.isCurrent ? plan : null,
    };
  }
  function collapsedInteraction({ collapsed, current, switching, hovered }) {
    return {
      canHover: collapsed && !current,
      canReveal: collapsed && !current && !switching,
      visible: collapsed && !current && (hovered || switching),
    };
  }
  function progressMetrics(progress, width, height = 12, inset = 2) {
    const clamped = Math.max(0, Math.min(1, progress)),
      grooveHeight = Math.max(0, height - 2 * inset),
      grooveWidth = Math.max(0, width - 2 * inset),
      raw = grooveWidth * clamped;
    return {
      fillScale: clamped,
      showsFill: clamped > 0,
      grooveHeight,
      rawFillWidth: raw,
      visibleFillWidth: raw > 0 ? Math.max(raw, grooveHeight) : 0,
    };
  }
  function actions({
    importing = false,
    adding = false,
    sub2api = false,
    switching = false,
    canRefresh = true,
    spinning = false,
  } = {}) {
    return [
      {
        intent: "importCurrentAuth",
        title: importing
          ? "accounts.action.importing"
          : "accounts.action.import_account",
        label: importing ? "accounts.action.importing" : "accounts.action.import",
        enabled: !importing && !adding,
        symbol: "square.and.arrow.down",
        menu: [
          "importCurrentAuth",
          "importAuthFile",
          ...(sub2api ? ["importSub2APIAccounts"] : []),
        ],
      },
      {
        intent: adding ? "cancelAddAccount" : "addAccount",
        title: adding ? "common.cancel" : "accounts.action.add_account",
        label: adding ? "common.cancel" : "accounts.action.add",
        enabled: adding || !importing,
        symbol: adding ? "xmark" : "plus",
      },
      { intent: "smartSwitch", enabled: !importing && !adding && !switching },
      { intent: "refreshUsage", enabled: canRefresh, spinning },
    ];
  }
  function settingsControls(settings = {}, editors = []) {
    return {
      usagePicker: {
        selectedValue:
          settings.usageProgressDisplayMode ?? settings.usageDisplay ?? "used",
        options: ["used", "remaining"],
      },
      switchToggles: [
        "autoSmartSwitch",
        "syncOpencodeOpenaiAuth",
        "restartEditorsOnSwitch",
      ],
      restartPicker: {
        isEnabled:
          Boolean(settings.restartEditorsOnSwitch) && editors.length > 0,
        selectedValue: settings.restartEditorTargets?.[0] ?? null,
        options: [
          { value: null, title: "common.none" },
          ...editors.map((e) => ({ value: e.id, title: e.label })),
        ],
      },
      languagePicker: {
        selectedValue: resolveLocale(settings.locale),
        options: locales,
      },
    };
  }
  function trailingActions({
    canRefresh = true,
    spinning = false,
    collapsed = false,
  } = {}) {
    return [
      { intent: "refreshUsage", enabled: canRefresh, spinning },
      {
        intent: "toggleCollapse",
        symbol: collapsed ? "chevron.down" : "chevron.up",
        label: collapsed
          ? "accounts.action.expand_all"
          : "accounts.action.collapse_all",
      },
    ];
  }
  function publicCallout(running) {
    return running
      ? null
      : {
          title: "proxy.public.callout.start_local_first_title",
          message: "proxy.public.callout.start_local_first_message",
        };
  }
  function remoteHeader({
    label = "",
    sshUser = "root",
    host = "",
    sshPort = 22,
    expanded = false,
    status = null,
  } = {}) {
    return {
      title: label || "new-server",
      subtitle: expanded ? null : `${sshUser}@${host}:${sshPort}`,
      isRunning: status?.running === true,
    };
  }
  function publicModes(selectedMode, enabled) {
    return ["quick", "named"].map((mode) => ({
      mode,
      selected: mode === selectedMode,
      isEnabled: enabled,
    }));
  }
  function publicStatus(status = {}) {
    return [
      {
        id: "status",
        headline: status.running
          ? "proxy.status.running"
          : "proxy.status.stopped",
        copyValue: null,
        truncation: "tail",
      },
      {
        id: "url",
        headline: status.publicURL ?? "proxy.value.generated_after_start",
        copyValue: status.publicURL ?? null,
        truncation: "middle",
      },
      {
        id: "install-path",
        headline: status.binaryPath ?? "proxy.public.not_detected",
        copyValue: null,
        truncation: "middle",
      },
      {
        id: "last-error",
        headline: status.lastError ?? "common.none",
        copyValue: null,
        truncation: "tail",
      },
    ];
  }
  function remoteMetrics(status) {
    const boolean = (value) =>
      value == null ? "Unknown" : value ? "Yes" : "No";
    return ["installed", "serviceInstalled", "enabled", "running", "pid"].map(
      (key) => ({
        id: key,
        value:
          key === "pid" ? String(status?.pid ?? "--") : boolean(status?.[key]),
      }),
    );
  }
  function remoteDetails(status) {
    return [
      { value: status?.baseURL ?? "--", canCopy: status != null },
      {
        value: status?.apiKey ?? "Generated after first start",
        canCopy: status?.apiKey != null,
      },
      { value: status?.serviceName ?? "Unknown", canCopy: status != null },
    ];
  }
  function remoteLogs(logs) {
    return {
      content: logs || "Logs have not been loaded yet",
      canCopy: Boolean(logs),
    };
  }
  function proxyButtons(running, loading = false) {
    return [
      { intent: "refreshStatus", enabled: !loading },
      {
        intent: running ? "stop" : "start",
        enabled: !loading,
        surfaceStyle: "prominent",
        role: running ? "destructive" : "standard",
      },
    ];
  }
  function publicButtons(running, loading, canStart) {
    const buttons = proxyButtons(running, loading);
    buttons[1].enabled = running ? !loading : canStart;
    return buttons;
  }
  function remoteButtons(running, active = null) {
    return [
      "save",
      "removeLocal",
      "discover",
      "deploy",
      "syncAccounts",
      "refresh",
      running ? "stop" : "start",
      "logs",
      "uninstall",
    ].map((intent) => ({
      intent,
      enabled: !active,
      showsProgress: active === intent,
      surfaceStyle: ["save", "deploy", "syncAccounts"].includes(intent)
        ? "prominent"
        : intent === "uninstall"
          ? "dangerProminent"
          : "regular",
      role: ["removeLocal", "uninstall", "stop"].includes(intent)
        ? "destructive"
        : "standard",
    }));
  }
  function remoteHelp(buttons) {
    const names = {
      removeLocal: "remove_local",
      syncAccounts: "sync_accounts",
      uninstall: "uninstall_remote",
    };
    return buttons.map((b) => ({
      action: b.intent,
      key: `proxy.remote.help.${names[b.intent] || b.intent}`,
    }));
  }
  function normalizeLogs(log) {
    const chars = Array.from(log || "");
    return chars.length > 12000
      ? "...\n" + chars.slice(-11996).join("")
      : log || "";
  }
  function visibleAccounts(accounts) {
    return accounts.filter(
      (a) => !a.displayStatus || a.displayStatus === "list",
    );
  }
  function pendingCards(accounts, directory = []) {
    const pending = accounts
      .filter(
        (a) =>
          a.displayStatus !== "deleted" &&
          (["pending", "deactivated"].includes(a.displayStatus) ||
            a.workspaceStatus === "deactivated"),
      )
      .map((a) => ({
        id: a.id,
        cardId: a.id,
        workspaceId: a.accountId,
        workspaceName: a.teamName || a.email || a.accountId,
        email: a.email,
        planType: a.planType,
        status:
          a.workspaceStatus === "deactivated" ? "deactivated" : a.displayStatus,
        visibility: "visible",
      }));
    for (const row of directory) {
      if (
        row.visibility === "deleted" ||
        accounts.some(
          (a) =>
            a.accountId?.trim().toLowerCase() ===
            row.workspaceId?.trim().toLowerCase(),
        )
      )
        continue;
      pending.push({
        ...row,
        id: row.workspaceId,
        status: row.status === "deactivated" ? "deactivated" : "pending",
      });
    }
    return pending.sort(
      (a, b) =>
        (a.status === b.status ? 0 : a.status === "deactivated" ? -1 : 1) ||
        // A directory row keeps whatever the store holds, so a missing name
        // must sort as empty instead of failing the whole render.
        String(a.workspaceName ?? "").localeCompare(
          String(b.workspaceName ?? ""),
          undefined,
          { sensitivity: "base" },
        ),
    );
  }
  /// The cards in display order, each provider's cards gathered where its
  /// first card stands: `{ account }` alone, `{ key, accounts }` for a
  /// provider with several. `keyOf` gives a card's provider, or null.
  function providerGroups(accounts, keyOf) {
    const entries = [],
      groups = new Map();
    for (const account of accounts) {
      const key = keyOf(account);
      if (key == null) {
        entries.push({ account });
        continue;
      }
      let group = groups.get(key);
      if (!group) {
        group = { key, accounts: [] };
        groups.set(key, group);
        entries.push(group);
      }
      group.accounts.push(account);
    }
    // A provider's accounts are not comparable by usage: they keep their
    // Sub2api account id order, the number ending each card id.
    const sub2apiId = (account) => Number(/-(\d+)$/.exec(account.id)?.[1] ?? 0);
    for (const e of entries)
      e.accounts?.sort((a, b) => sub2apiId(a) - sub2apiId(b));
    // A provider's only card is an ordinary card.
    return entries.map((e) =>
      e.accounts?.length === 1 ? { account: e.accounts[0] } : e,
    );
  }
  /// The cards in the order they were dragged into: saved keys first, in
  /// that order, then any card not saved yet, as ranked. The current card
  /// always leads, whatever the saved order.
  function orderEntries(entries, order = [], keyOf, isCurrent = () => false) {
    const rank = new Map(order.map((key, i) => [key, i]));
    const saved = entries
      .filter((e) => rank.has(keyOf(e)))
      .sort((a, b) => rank.get(keyOf(a)) - rank.get(keyOf(b)));
    const ordered = [...saved, ...entries.filter((e) => !rank.has(keyOf(e)))];
    return [...ordered.filter(isCurrent), ...ordered.filter((e) => !isCurrent(e))];
  }
  function needsReauthentication(
    account,
    translated = "Sign-in expired. Please sign in again.",
  ) {
    return (
      account.sourceTag !== "SUB2API" &&
      ["error.accounts.sign_in_expired", translated].includes(
        account.usageError,
      )
    );
  }
  function cardsEqual(a, b) {
    return JSON.stringify(a) === JSON.stringify(b);
  }
  return {
    formatLocalized,
    proxyButtons,
    publicButtons,
    remoteButtons,
    remoteHelp,
    normalizeLogs,
    visibleAccounts,
    pendingCards,
    providerGroups,
    orderEntries,
    needsReauthentication,
    cardsEqual,
    settingsControls,
    publicCallout,
    remoteHeader,
    remoteMetrics,
    clean,
    resolveLocale,
    preferredLocale,
    normalizedPlan,
    workspaceTag,
    displayPercent,
    windowPresentation,
    formatReset,
    tokenExpiry,
    resetCountdown,
    card,
    actions,
    collapsedInteraction,
    progressMetrics,
    trailingActions,
    publicModes,
    publicStatus,
    remoteDetails,
    remoteLogs,
  };
});
