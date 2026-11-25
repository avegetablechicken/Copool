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
  const themes = ["system", "light", "dark"];
  function resolveTheme(value) {
    return themes.includes(value) ? value : "system";
  }
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
  /// When Claude renamed Max 5x to Max and Max 20x to Ultra.
  const CLAUDE_PLAN_RENAME = Date.UTC(2026, 9, 30);
  /// When OpenAI added its $100 Pro (prolite) beside the $200 one.
  const CODEX_PRO_LITE = Date.UTC(2026, 3, 9);
  /// When Google added its $100 Ultra beside the top one.
  const GOOGLE_ULTRA_LITE = Date.UTC(2026, 4, 19);
  /// A Claude Max plan by its name at `now`: "MAX 5X" and "MAX 20X" before
  /// the rename, "MAX" and "ULTRA" after; a Max without multiplier is "MAX".
  function claudeMaxName(times, now) {
    if (now >= CLAUDE_PLAN_RENAME) return times >= 20 ? "ULTRA" : "MAX";
    return times ? `MAX ${times}X` : "MAX";
  }
  function normalizedPlan(account, pool = "codex", now = Date.now()) {
    const raw =
      typeof account === "object" && account !== null
        ? clean(account.usage?.planType) || clean(account.planType)
        : clean(account);
    let p = (raw || "team").toUpperCase().replace(/\s+/g, " ");
    if (pool === "antigravity") {
      // Both Ultra plans are "Google AI Ultra": known by their tier id as
      // "ultra 5x" and "ultra 20x", else as "ultra" alone.
      if (p === "ULTRA 5X" || p === "ULTRA LITE") return "ULTRA 5X";
      if (p === "ULTRA 20X") return now >= GOOGLE_ULTRA_LITE ? "ULTRA 20X" : "ULTRA";
    }
    // Grok's plans are shown as named ("SUPERGROK HEAVY"), as its widget.
    if (pool === "grok" || (pool === "antigravity" && account?.sourceTag === "SUB2API")) return p;
    if (pool === "claude") {
      // Claude Code reports Max 5x and Max 20x as "max" with the multiplier,
      // also since their renaming; "ultra" is taken should it report one.
      p = p.replace(/^CLAUDE[ _]/, "");
      const max = /^MAX(?: (\d+)X)?$/.exec(p);
      if (max) return claudeMaxName(max[1] ? Number(max[1]) : 0, now);
      if (p === "ULTRA") return claudeMaxName(20, now);
    }
    if (pool === "codex") {
      // OpenAI's Pro plans: $100 (prolite), $200 (pro) and $500 (promax),
      // named as OpenAI does once the $100 one came.
      // ChatGPT Go, the $8 plan below Plus.
      if (p === "GO") return p;
      if (p === "PROLITE") return "PRO 5X";
      if (p === "PROMAX") return "PRO 50X";
      if (p === "PRO") return now >= CODEX_PRO_LITE ? "PRO 20X" : "PRO";
    }
    // Claude's Max plans keep their usage multiplier ("MAX 20X").
    if (/^MAX( \d+X)?$/.test(p)) return p;
    return ["PRO", "PLUS", "FREE", "BUSINESS", "ENTERPRISE", "ULTRA"].includes(p)
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
  function roundDisplayPercent(value, mode = "used") {
    if (value === null || !Number.isFinite(value)) return null;
    const clamped = Math.max(0, Math.min(100, value));
    return mode === "remaining" ? Math.ceil(clamped) : Math.floor(clamped);
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
  function resetCountdown(epoch, now, locale = "en", clockOffsetSeconds = 0) {
    if (!Number.isFinite(epoch)) return null;
    const seconds = epoch - (now + (clockOffsetSeconds ?? 0));
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
  /// Time until the next automatic refresh: hours and minutes, minutes
  /// (rounded up), or seconds under a minute, localized; due once reached;
  /// null without one.
  function refreshCountdown(epoch, now, locale = "en") {
    if (!Number.isFinite(epoch)) return null;
    const seconds = Math.ceil(epoch - now);
    if (seconds <= 0) return { due: true, remaining: null };
    const minutes = Math.ceil(seconds / 60),
      language = resolveLocale(locale);
    const parts = (
      seconds < 60
        ? [["second", seconds]]
        : minutes >= 60
          ? [
              ["hour", Math.floor(minutes / 60)],
              ["minute", minutes % 60],
            ].filter(([, n], i) => !i || n)
          : [["minute", minutes]]
    ).map(([unit, amount]) =>
      new Intl.NumberFormat(language, {
        style: "unit",
        unit,
        unitDisplay: "narrow",
      }).format(amount),
    );
    return {
      due: false,
      remaining: new Intl.ListFormat(language, {
        type: "unit",
        style: "narrow",
      }).format(parts),
    };
  }
  /// A refresh interval, localized: in hours, minutes or seconds, whichever
  /// it is a whole number of.
  function formatInterval(seconds, locale = "en") {
    const [unit, amount] =
      seconds % 3600 === 0
        ? ["hour", seconds / 3600]
        : seconds % 60 === 0
          ? ["minute", seconds / 60]
          : ["second", seconds];
    return new Intl.NumberFormat(resolveLocale(locale), {
      style: "unit",
      unit,
      unitDisplay: "long",
    }).format(amount);
  }
  // The intervals a pool's card in use may refresh at, and the rounds to
  // one that refreshes the whole pool; the core takes 10 s to an hour.
  const REFRESH_SECONDS = {
    codex: [10, 30, 60, 300, 600],
    claude: [60, 120, 300, 600, 900, 1800, 3600],
    antigravity: [60, 120, 300, 600, 900, 1800, 3600],
    grok: [60, 120, 300, 600, 900, 1800, 3600],
  };
  const FULL_EVERY = [1, 2, 3, 6, 12];
  /// A pool's refresh choices, the interval set among them.
  function refreshChoices(pool, seconds) {
    const choices = new Set(REFRESH_SECONDS[pool] || REFRESH_SECONDS.claude);
    if (Number.isFinite(seconds)) choices.add(seconds);
    return { seconds: [...choices].sort((a, b) => a - b), fullEvery: FULL_EVERY };
  }
  /// The interval a changing usage brings, as the core's fast_interval: a
  /// fifth, no sooner than every 30 seconds, never slower than set.
  function fastRefreshSeconds(seconds) {
    return Math.min(seconds, Math.max(seconds / 5, 30));
  }
  // OpenQuota's projectPace + QuotaMetric color rules, using Unix seconds.
  // Always judge consumption, independently of the displayed used/remaining mode.
  function usagePace(window, now = Date.now() / 1000) {
    const used = displayPercent(window);
    const result = (severity, projectedUsedPercent = null) => ({ severity, projectedUsedPercent });
    if (used === null) return result("level");
    if (100 - Math.floor(used) <= 0) return result("spent", 100);
    if (used <= 0) return result("level");
    now += window.clockOffsetSeconds ?? 0;
    const reset = window.resetAt;
    const period = window.windowSeconds;
    if (!Number.isFinite(reset) || reset <= now || !Number.isFinite(period) || period <= 0)
      return result("level");
    const elapsed = Math.max(0, now - (reset - period));
    if (elapsed < Math.max(60, period * 0.01)) return result("level");
    const projected = used / Math.min(1, elapsed / period);
    if (projected <= 90) return result("healthy", projected);
    if (used < 5) return result("level");
    return result(projected <= 100 && Math.round(100 - projected) >= 1 ? "close" : "runningOut", projected);
  }
  function usageTone(window, now = Date.now() / 1000) {
    const pace = usagePace(window, now);
    if (pace.severity === "level") {
      const used = displayPercent(window);
      return used === null ? "" : Math.floor(used) >= 90 ? "full" : Math.floor(used) >= 80 ? "high" : "";
    }
    return pace.severity === "healthy" ? "" : pace.severity === "close" ? "high" : "full";
  }
  function usagePaceMessage(window, now = Date.now() / 1000, locale = "en") {
    const { severity, projectedUsedPercent: projected } = usagePace(window, now);
    if (severity === "level" || severity === "healthy") return null;
    if (severity === "spent") return { key: "accounts.window.pace_spent", args: [] };
    if (severity === "close") return { key: "accounts.window.pace_spare", args: [Math.max(1, Math.round(100 - projected))] };
    // OpenQuota has no pace label when there is no pre-reset exhaustion time.
    if (projected <= 100) return null;
    const serverNow = now + (window.clockOffsetSeconds ?? 0);
    const start = window.resetAt - window.windowSeconds;
    const runOutAt = start + (serverNow - start) * 100 / displayPercent(window);
    const countdown = resetCountdown(runOutAt, serverNow, locale);
    return countdown.soon
      ? { key: "accounts.window.pace_soon", args: [] }
      : { key: "accounts.window.pace_in", args: [countdown.remaining] };
  }
  function windowPresentation(
    window,
    mode = "used",
    formatDate = (value) => formatReset(value),
    startsOnUse = false,
  ) {
    const used = Math.floor(displayPercent(window, "used", 100));
    return {
      // A missing window (e.g. no five-hour limit) has no data to show.
      available: displayPercent(window) !== null,
      // A five-hour window starts on first use. Unused, it has a real 0% but no
      // reset; a countdown would read as an open window, as in OpenQuota.
      notStarted: startsOnUse && displayPercent(window) === 0 && window?.resetAt == null,
      tone: usageTone(window),
      paceWindow: window,
      progressPercent: mode === "remaining" ? 100 - used : used,
      primaryKind: mode,
      primaryPercent: mode === "remaining" ? 100 - used : used,
      secondaryPercent: mode === "remaining" ? used : 100 - used,
      reset: window?.resetAt != null ? formatDate(window.resetAt) : "--",
      resetAt: window?.resetAt ?? null,
      clockOffsetSeconds: window?.clockOffsetSeconds ?? null,
    };
  }
  /// A plan's color, by tier so that providers' comparable plans match: the
  /// entry subscription (Codex Go and Plus, Claude Pro, Google AI Pro,
  /// SuperGrok Lite and SuperGrok) is pink; those of $100 to $300, which
  /// their labels tell apart by multiplier (Codex Pro 5x and 20x, Claude Max
  /// and Ultra, Google AI Ultra 5x and 20x, SuperGrok Plus and Heavy),
  /// orange; and Codex's $500 Pro 50x purple.
  function planAccent(plan, pool = "codex") {
    if (pool === "grok") {
      // Sub2api may space the name ("SUPER GROK HEAVY"). Also The X
      // Subscription's names, which may replace SuperGrok's.
      const name = plan.replace(/ /g, "");
      if (["SUPERGROKLITE", "SUPERGROK", "PLUS", "PREMIUM"].includes(name)) return "pink";
      if (["SUPERGROKPLUS", "SUPERGROKHEAVY", "SUPER", "ULTRA"].includes(name)) return "orange";
      if (!["FREE", "BUSINESS", "ENTERPRISE"].includes(plan)) return "teal";
    }
    const pro = /^PRO(?: (\d+)X)?$/.exec(plan);
    if (pool === "codex" && pro) return Number(pro[1] || 0) >= 50 ? "purple" : "orange";
    if (/^(?:MAX|ULTRA)(?: \d+X)?$/.test(plan)) return "orange";
    return (
      {
        GO: "pink",
        PRO: "pink",
        PLUS: "pink",
        FREE: "gray",
        BUSINESS: "indigo",
        ENTERPRISE: "indigo",
      }[plan] || "teal"
    );
  }
  function card(
    account,
    collapsed = false,
    mode = "used",
    formatDate,
    pool = "codex",
    now = Date.now(),
  ) {
    const plan = normalizedPlan(account, pool, now),
      name =
        clean(account.email) || clean(account.accountId) || clean(account.label) || "",
      credits = account.usage?.credits;
    return {
      plan,
      accent: planAccent(plan, pool),
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
      fiveHour: windowPresentation(account.usage?.fiveHour, mode, formatDate, true),
      oneWeek: windowPresentation(account.usage?.oneWeek, mode, formatDate),
      // Codex Free's 30-day monthly window, shown only when reported.
      oneMonth: windowPresentation(account.usage?.oneMonth, mode, formatDate),
      // Claude's weekly Fable window, shown only on accounts that report it.
      fableOneWeek: windowPresentation(account.usage?.fableOneWeek, mode, formatDate),
      // Antigravity's third-party (Claude) pool.
      thirdPartyFiveHour: windowPresentation(
        account.usage?.thirdPartyFiveHour,
        mode,
        formatDate,
        true,
      ),
      thirdPartyOneWeek: windowPresentation(account.usage?.thirdPartyOneWeek, mode, formatDate),
      // The Sub2api version too old to report the card's weekly usage.
      outdatedSub2api: clean(account.outdatedSub2apiVersion) || null,
      compact: [
        displayPercent(account.usage?.fiveHour, mode),
        displayPercent(account.usage?.oneWeek ?? account.usage?.oneMonth, mode),
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
      themePicker: {
        selectedValue: resolveTheme(settings.theme),
        options: themes,
      },
      switchToggles: [
        "autoSmartSwitch",
        "syncOpencodeOpenaiAuth",
        "syncPiAuth",
        "syncOmpAuth",
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
      [
        "error.accounts.sign_in_expired",
        "error.antigravity.sign_in_expired",
        "error.grok.sign_in_expired",
        translated,
      ].includes(account.usageError)
    );
  }
  function resetInventory(usage, now = Date.now() / 1000) {
    const grants = (usage?.resetGrants ?? []).filter(
      (g) => g.count > 0 && (g.expiresAt == null || g.expiresAt > now),
    );
    const hasInventory = (usage?.resetGrants ?? []).length > 0;
    let count = hasInventory
      ? grants.reduce((sum, g) => sum + g.count, 0)
      : usage?.remainingResetCount;
    if (!hasInventory && count >= 0 && count != null && usage?.resetsLastConfirmedAt != null) {
      const expired = (usage.resetExpiries ?? []).filter((at) => at <= now).length;
      count = Math.max(0, count - expired);
    }
    return {
      count: count != null && count >= 0 ? count : null,
      grants,
      warning: grants.some((g) => g.kind !== "full"),
      fable: grants.some((g) => g.kind === "fable"),
    };
  }
  function balanceTooltips(usage, now = Date.now() / 1000) {
    const inventory = resetInventory(usage, now);
    const types = new Map();
    const resetDates = new Map();
    const normalizeDate = (at) => (Number.isFinite(at) && at > 0 ? at : null);
    const addDate = (at, count = 1) => {
      const date = normalizeDate(at);
      resetDates.set(date, (resetDates.get(date) ?? 0) + count);
    };
    for (const grant of inventory.grants) {
      const kind = ["full", "fable", "partial"].includes(grant.kind) ? grant.kind : "unknown";
      const at = normalizeDate(grant.expiresAt);
      const key = `${kind}:${at ?? ""}`;
      const existing = types.get(key);
      if (existing) existing.count += grant.count;
      else types.set(key, { kind, count: grant.count, at });
      addDate(at, grant.count);
    }
    if (!inventory.grants.length && inventory.count) {
      const dates = (usage?.resetExpiries ?? []).filter((at) => Number.isFinite(at) && at > now);
      for (const at of dates) addDate(at);
      if (dates.length < inventory.count) addDate(null, inventory.count - dates.length);
    }
    return {
      types: [...types.values()].sort((a, b) => (a.at ?? Infinity) - (b.at ?? Infinity)),
      resetDates: [...resetDates].map(([at, count]) => ({ at, count }))
        .sort((a, b) => (a.at ?? Infinity) - (b.at ?? Infinity)),
      creditDates: [...new Set((usage?.credits?.expiries ?? [])
        .filter((at) => Number.isFinite(at) && at > now))].sort((a, b) => a - b),
    };
  }
  function cardsEqual(a, b) {
    return JSON.stringify(a) === JSON.stringify(b);
  }

  // While keyboard navigation of the menu bar holds the panel open (macOS 27),
  // the focused panel receives ←/→; those it does not use move the navigation
  // on to the neighbouring item, as from a menu. Returns "left", "right" or
  // null for `key`.
  function menuBarStep(key, { navigating, modified, inText, handled }) {
    if (!navigating || modified || inText || handled) return null;
    return key === "ArrowLeft" ? "left" : key === "ArrowRight" ? "right" : null;
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
    resetInventory,
    balanceTooltips,
    settingsControls,
    publicCallout,
    remoteHeader,
    remoteMetrics,
    clean,
    resolveLocale,
    resolveTheme,
    preferredLocale,
    normalizedPlan,
    planAccent,
    workspaceTag,
    displayPercent,
    roundDisplayPercent,
    windowPresentation,
    usageTone,
    usagePaceMessage,
    formatReset,
    tokenExpiry,
    refreshCountdown,
    formatInterval,
    refreshChoices,
    fastRefreshSeconds,
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
    menuBarStep,
  };
});
