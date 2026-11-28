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
  /// Each third-party agent's notices after a switch, by its id in the
  /// core, whose `SyncAgent` table alone decides what a switch syncs.
  const agentSyncNotices = {
    opencode: {
      done: "accounts.notice.sync_done",
      failed: "accounts.notice.sync_failed_format",
    },
    pi: {
      done: "accounts.notice.pi_sync_done",
      failed: "accounts.notice.pi_sync_failed_format",
    },
    omp: {
      done: "accounts.notice.omp_sync_done",
      failed: "accounts.notice.omp_sync_failed_format",
    },
  };
  /// An agent's saved sync: off with every pool chosen, unless saved not.
  function agentSync(settings = {}, id) {
    const saved = settings.agentSync?.[id] || {};
    return { enabled: Boolean(saved.enabled), pools: saved.pools || {} };
  }
  /// The third-party agents' rows: each one's switch, target and the pools
  /// it can receive, every pool chosen unless saved as not.
  function agentSyncRows(settings = {}, agents = []) {
    return agents
      .filter((agent) => agent.id in agentSyncNotices)
      .map((agent) => {
        const sync = agentSync(settings, agent.id);
        return {
          id: agent.id,
          enabled: sync.enabled,
          target: agent.target || null,
          found: Boolean(agent.found),
          pools: (agent.pools || []).map((pool) => ({
            pool,
            chosen: sync.pools[pool] !== false,
          })),
        };
      });
  }
  /// The preferences after one agent's saved sync is changed.
  function changeAgentSync(settings = {}, agent, change) {
    const all = settings.agentSync || {};
    return { agentSync: { ...all, [agent]: change(all[agent] || {}) } };
  }
  /// The preferences after one agent's sync is turned on or off.
  function setAgentSync(settings, agent, enabled) {
    return changeAgentSync(settings, agent, (saved) => ({ ...saved, enabled }));
  }
  /// The preferences after one agent's pool is chosen or not.
  function toggleAgentSyncPool(settings, agent, pool) {
    return changeAgentSync(settings, agent, (saved) => {
      const pools = saved.pools || {};
      return { ...saved, pools: { ...pools, [pool]: pools[pool] === false } };
    });
  }
  /// What a switch's notice says of one agent it synced: done, or why not.
  function agentSyncNotice({ agent, error } = {}) {
    const keys = agentSyncNotices[agent];
    if (!keys) return null;
    return error
      ? { key: keys.failed, args: [error], error: true }
      : { key: keys.done, args: [], error: false };
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
      switchToggles: ["autoSmartSwitch", "restartEditorsOnSwitch"],
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
  /// The provider of a Sub2api card, which its cards refresh with: the core
  /// names the card's refresh key `sub2api:<provider>` (refresh_plan.rs), so
  /// cards synced since the provider list was last read still group with
  /// their provider.
  function providerKey(account) {
    const key = account.refresh?.key;
    return key?.startsWith("sub2api:") ? key.slice("sub2api:".length) : null;
  }
  /// The card whose proxy a card's back edits: the account's own, or its
  /// provider's first card for a provider's group.
  function proxyOwner(target) {
    return target.group ? target.group.accounts[0] : target.account;
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
  /// How a list scrolled to `scrollTop`, which can scroll to `end`, keeps a
  /// card that moved by `shift` where it stood: the offset to scroll to, and
  /// the space to add at its end to scroll that far, which only a card being
  /// edited keeps (`editing`); a card it cannot hold that way moves.
  function heldScroll(shift, scrollTop, end, editing) {
    const wanted = scrollTop + shift;
    const space = editing && wanted > end ? Math.ceil(wanted - end) : 0;
    return { scrollTop: Math.max(0, Math.min(wanted, end + space)), space };
  }
  /// Whether a card shows an unofficial provider's account (Sub2api or
  /// CLIProxyAPI), as the core marks it: its provider owns its sign-in and
  /// proxy.
  function isProviderCard(account) {
    return !!account?.providerKind;
  }
  function needsReauthentication(
    account,
    translated = "Sign-in expired. Please sign in again.",
  ) {
    return (
      !isProviderCard(account) &&
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
  /// The account pools, left to right as their buttons, for ⌘1–⌘4 and the
  /// way the cards slide; every one has Sub2api providers.
  const ACCOUNT_POOLS = ["codex", "claude", "antigravity", "grok"];
  /// The pools with an API proxy: all but Grok.
  const PROXY_POOLS = ["codex", "claude", "antigravity"];
  /// The preferences of each pool's proxy on the proxy page.
  const PROXY_PREFERENCES = {
    codex: { port: "proxyPort", autoStart: "autoStartProxy" },
    antigravity: { port: "antigravityProxyPort", autoStart: "antigravityAutoStartProxy" },
    claude: { port: "claudeProxyPort", autoStart: "claudeAutoStartProxy" },
  };
  /// The preference a port typed on `pool`'s proxy page saves, named for
  /// that pool when typed; null for no port, or a pool without a proxy.
  function portPatch(pool, value) {
    const preferences = PROXY_PREFERENCES[pool];
    const port = Number(value);
    if (!preferences || !Number.isInteger(port) || port < 1 || port > 65535) return null;
    return { [preferences.port]: port };
  }
  /// The pools a pool chosen on either page shows: the accounts and proxy
  /// pages share one, but for Grok, which has no proxy. Null for no pool.
  function poolSelection(next) {
    if (!ACCOUNT_POOLS.includes(next)) return null;
    return { accounts: next, proxy: PROXY_POOLS.includes(next) ? next : null };
  }
  /// What ⌘1–⌘4 (`key`) shows from `page`: its pool, on the accounts page
  /// unless the proxy page shows that pool's proxy too.
  function poolShortcut(page, key) {
    const pool = ACCOUNT_POOLS[Number(key) - 1];
    if (!pool) return null;
    const stays = page === "accounts" || (page === "proxy" && PROXY_POOLS.includes(pool));
    return { pool, page: stays ? null : "accounts" };
  }
  /// How a theme applies to the page: its `data-theme` (null for the
  /// system's), the color-scheme meta and the value saved for the next start.
  function themeAttributes(value) {
    const theme = resolveTheme(value);
    return {
      dataTheme: theme === "system" ? null : theme,
      colorScheme: theme === "system" ? "light dark" : theme,
      saved: theme,
    };
  }
  /// The greeting times with a new one, in order; null for no time or one
  /// already set.
  function withGreetingTime(times, time) {
    if (!/^\d{2}:\d{2}$/.test(time) || times.includes(time)) return null;
    return [...times, time].sort();
  }
  function withoutGreetingTime(times, time) {
    return times.filter((t) => t !== time);
  }
  /// Swift's buildSwitchNotice: the switch in pool `from`, then what followed
  /// it, in `tr`'s words. `from` is the pool asked, which another may have
  /// replaced on screen.
  function switchNotice(execution, from, tr) {
    let error = false;
    const segments = [
      execution.provider
        ? tr(
            {
              claude: "accounts.notice.claude_provider_switched_format",
              antigravity: "accounts.notice.antigravity_provider_switched_format",
              grok: "accounts.notice.grok_provider_switched_format",
            }[from] || "accounts.notice.provider_switched_format",
            execution.provider,
          )
        : tr(
            {
              claude: "accounts.notice.claude_switch_done",
              antigravity: "accounts.notice.antigravity_switch_done",
              grok: "accounts.notice.grok_switch_done",
            }[from] || "accounts.notice.switch_done",
          ),
    ];
    // settings.json keys that keep Claude Code off the account login.
    if (execution.loginOverrides?.length) {
      error = true;
      segments.push(
        tr("accounts.notice.claude_settings_override_format", execution.loginOverrides.join(", ")),
      );
    }
    for (const outcome of execution.agentSync || []) {
      const message = agentSyncNotice(outcome);
      if (!message) continue;
      if (message.error) error = true;
      segments.push(tr(message.key, ...message.args));
    }
    if (execution.editorRestartError) {
      error = true;
      segments.push(tr("accounts.notice.editor_restart_failed_format", execution.editorRestartError));
    } else if (execution.restartedEditors?.length)
      segments.push(
        tr("accounts.notice.editor_restarted_format", execution.restartedEditors.join(" / ")),
      );
    if (execution.launchError) {
      error = true;
      segments.push(execution.launchError);
    }
    return { text: segments.join(" · "), error };
  }
  /// The import menu: the pool's current login, a file, and its Sub2api
  /// providers' cards (`providers`, the providers' choices) when it has any.
  function importMenu(tr, { importCurrent, importFile, providers }) {
    const items = [
      { label: tr("accounts.action.import_current_auth"), run: importCurrent },
      { label: tr("accounts.action.import_auth_file"), run: importFile },
    ];
    if (providers.length)
      items.push({ label: tr("accounts.action.import_sub2api"), children: providers });
    return items;
  }
  /// The kinds of unofficial provider, as the add menu names them.
  const PROVIDER_KINDS = [
    ["sub2api", "Sub2API"],
    ["cliproxyapi", "CLIProxyAPI"],
  ];
  /// Adding an unofficial provider: its kind, then for Sub2api the pool
  /// whose form opens.
  function providerAddMenu(tr, icon, add) {
    return PROVIDER_KINDS.map(([kind, label]) =>
      // A CLIProxyAPI server lists every pool's credentials: its provider is
      // every pool's.
      kind === "cliproxyapi"
        ? { label, run: () => add(null, kind) }
        : {
            label,
            children: ACCOUNT_POOLS.map((pool) => ({
              label: tr(`pool.${pool}`),
              icon: icon(pool),
              run: () => add(pool, kind),
            })),
          },
    );
  }
  /// The title of a provider without a name, by its kind: Sub2api's for
  /// one saved before kinds existed.
  function providerTitle(kind) {
    return kind === "cliproxyapi" ? "CLIProxyAPI" : "Sub2api";
  }
  /// What a provider's sync tells, as `[key, argument, tone]`: how many of
  /// its accounts it synced, or, a CLIProxyAPI one synced none without
  /// failing, that it has none for the pool (whose name is the argument),
  /// as a warning (app.js's `notice`).
  function providerSyncNotice(kind, count, pool) {
    if (kind !== "cliproxyapi") return ["accounts.notice.sub2api_imported_format", count, false];
    if (Number(count) > 0) return ["accounts.notice.cliproxyapi_imported_format", count, false];
    return ["accounts.notice.cliproxyapi_none_format", pool, "warning"];
  }
  return {
    ACCOUNT_POOLS,
    PROXY_POOLS,
    PROXY_PREFERENCES,
    portPatch,
    poolSelection,
    poolShortcut,
    themeAttributes,
    withGreetingTime,
    withoutGreetingTime,
    switchNotice,
    importMenu,
    providerAddMenu,
    heldScroll,
    isProviderCard,
    providerTitle,
    providerSyncNotice,
    providerKey,
    proxyOwner,
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
    agentSync,
    agentSyncRows,
    setAgentSync,
    toggleAgentSyncPool,
    agentSyncNotice,
    publicCallout,
    remoteHeader,
    remoteMetrics,
    clean,
    resolveLocale,
    resolveTheme,
    preferredLocale,
    displayPercent,
    roundDisplayPercent,
    usageTone,
    usagePaceMessage,
    formatReset,
    tokenExpiry,
    refreshCountdown,
    formatInterval,
    refreshChoices,
    fastRefreshSeconds,
    resetCountdown,
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
