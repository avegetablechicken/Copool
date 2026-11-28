/* What an account card shows, decided once: its tags, usage rows, footer
   and tooltips, details and actions. app.js only paints the view. Used by
   the UI and node:test.

   Every function takes the card's context, which app.js gathers once per
   render (`cardContext`):
     pool          the pool shown: "codex", "claude", "antigravity" or "grok"
     mode          usage shown as "used" or "remaining"
     resetDisplay  reset times as "countdown" or "exact"
     collapsed     the list is collapsed into rows
     locale, tr    the language and its translation, tr(key, ...args)
     now           Unix seconds
     accountProxy  the shared account proxy, trimmed; blank for none
     switchingId   the account being switched to, or null
     adding, authorizing   a sign-in under way
     ui(id)        the card's state under way: { refreshing, hi, renewing,
                   renewed, thirdParty } (see IDLE), as the Accounts
                   controller's cardState has it
   The view is plain data. What changes with time alone while it is shown
   (resetLabel, paceTitle, refreshTitle, details) is worked out again by the
   functions below as the pointer enters. */
(function (root, factory) {
  const node = typeof module === "object" && module.exports;
  const api = factory(node ? require("./presentation.js") : root.CopoolPresentation);
  if (node) module.exports = api;
  else root.CopoolCardView = api;
})(globalThis, function (P) {
  /// A card with nothing under way.
  const IDLE = {
    // Its usage is being read, here or by a remote server.
    refreshing: false,
    // How far its hi got: "sent" until the model replies, which locks its
    // refresh, then "received" while its 5h window has yet to show; null
    // for none.
    hi: null,
    renewing: false,
    // Its token was just renewed.
    renewed: false,
    // Its usage rows show its third-party pool (the model switch).
    thirdParty: false,
  };
  const uiOf = (ctx, id) => ({ ...IDLE, ...(ctx.ui?.(id) ?? {}) });
  const nowOf = (ctx) => ctx.now ?? Date.now() / 1000;
  /// The model of a pool's own windows.
  const OWN_MODEL = { codex: "Codex", claude: "Claude", antigravity: "Gemini", grok: "Grok" };
  /// The third-party pool's model: Antigravity's other than Gemini, Claude.
  const THIRD_PARTY_MODEL = "Claude";
  /// The pools whose accounts may have a monthly window: Codex Free, and a
  /// Grok plan whose allowance lasts a month.
  const MONTHLY_POOLS = ["codex", "grok"];
  /// Only Codex and Claude report banked resets; Antigravity and Grok show
  /// credits alone.
  const RESET_POOLS = ["codex", "claude"];

  function windowView(window, mode, formatDate, now, startsOnUse = false) {
    const used = Math.floor(P.displayPercent(window, "used", 100));
    return {
      // A missing window (e.g. no five-hour limit) has no data to show.
      available: P.displayPercent(window) !== null,
      // A five-hour window starts on first use. Unused, it has a real 0% but no
      // reset; a countdown would read as an open window, as in OpenQuota.
      notStarted: startsOnUse && P.displayPercent(window) === 0 && window?.resetAt == null,
      tone: P.usageTone(window, now),
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
  /// The account's windows and balance as the card reads them.
  function summary(account, ctx) {
    const mode = ctx.mode || "used";
    const now = nowOf(ctx);
    const formatDate = (epoch) => P.formatReset(epoch, ctx.locale);
    const usage = account.usage;
    const credits = usage?.credits;
    const window = (value, startsOnUse) => windowView(value, mode, formatDate, now, startsOnUse);
    return {
      credits: credits?.unlimited
        ? "unlimited"
        : credits?.balance == null
          ? "--"
          : Number.isFinite(Number(credits.balance))
            ? String(Math.round(Number(credits.balance)))
            : credits.balance,
      fiveHour: window(usage?.fiveHour, true),
      oneWeek: window(usage?.oneWeek),
      // Codex Free's 30-day monthly window, shown only when reported.
      oneMonth: window(usage?.oneMonth),
      // Claude's weekly Fable window, shown only on accounts that report it.
      fableOneWeek: window(usage?.fableOneWeek),
      // Antigravity's third-party (Claude) pool.
      thirdPartyFiveHour: window(usage?.thirdPartyFiveHour, true),
      thirdPartyOneWeek: window(usage?.thirdPartyOneWeek),
      // The Sub2api version too old to report the card's weekly usage.
      outdatedSub2api: P.clean(account.outdatedSub2apiVersion) || null,
    };
  }
  /// Whether a card reports a third-party pool beside its own.
  const hasThirdParty = (windows) =>
    windows.thirdPartyFiveHour.available || windows.thirdPartyOneWeek.available;

  /// A window's reset time in `mode`: the date and time, or the time left.
  function resetLabel(window, mode, ctx) {
    const { tr } = ctx;
    if (window.notStarted) return tr("accounts.window.not_started");
    if (!window.available || window.resetAt == null)
      return tr("accounts.window.reset_unavailable");
    if (mode === "exact") return tr("accounts.window.reset_at_format", window.reset);
    const countdown = P.resetCountdown(window.resetAt, nowOf(ctx), ctx.locale, window.clockOffsetSeconds);
    return countdown.soon
      ? tr("accounts.window.resets_soon")
      : tr("accounts.window.resets_in_format", countdown.remaining);
  }
  /// A meter's tooltip: when its usage runs out at its pace, if it does.
  function paceTitle(window, ctx) {
    const message = P.usagePaceMessage(window, nowOf(ctx), ctx.locale);
    return message ? ctx.tr(message.key, ...message.args) : "";
  }
  /// A refresh button's tooltip, `base` followed by how long until the
  /// automatic refresh at `nextAt` (Unix seconds), if one is due.
  function refreshTitle(base, nextAt, ctx) {
    const { tr } = ctx;
    const next = P.refreshCountdown(nextAt, nowOf(ctx), ctx.locale);
    return !next
      ? base
      : `${base}\n${
          next.due ? tr("accounts.refresh.next_due") : tr("accounts.refresh.next_format", next.remaining)
        }`;
  }
  /// The access token's remaining time, or null when unknown.
  function tokenLabel(account, ctx) {
    const { tr } = ctx;
    const expiry = P.tokenExpiry(account.tokenExpiresAt, nowOf(ctx), ctx.locale);
    if (!expiry) return null;
    return expiry.expired
      ? tr("accounts.renewal.expired")
      : tr("accounts.renewal.expires_format", expiry.remaining);
  }
  /// The edit button's tooltip, kept to the proxy, token lifetime and
  /// configured refresh interval. `target` is { account, grouped } or
  /// { group }, as the card's flip key names it.
  function details(target, ctx) {
    const { tr } = ctx;
    if (!target) return tr("accounts.card.edit");
    const { account, grouped } = target;
    const lines = [];
    if (!grouped) {
      // The proxy edited: the account's own, or its provider's for a group.
      const owner = target.group ? target.group.accounts[0] : account;
      // A blank proxy uses the shared account proxy; a Sub2api provider's never does.
      const shared = P.isProviderCard(owner) ? "" : ctx.accountProxy || "";
      const proxy = owner.proxyUrl?.trim() || shared;
      lines.push(!proxy || proxy === "direct://" ? "direct" : `${tr("accounts.proxy.title")}: ${proxy}`);
    }
    if (account && !grouped && account.renewal) {
      const token = tokenLabel(account, ctx);
      if (token) lines.push(token);
    }
    // Its usual interval, as the core gives each card (refresh_plan.rs): a
    // provider's cards share theirs.
    const cards = target.group ? target.group.accounts : [account];
    const seconds = cards.map((card) => card.refresh?.seconds).filter(Number.isFinite);
    if (seconds.length)
      lines.push(tr("accounts.refresh.interval_format", P.formatInterval(Math.min(...seconds), ctx.locale)));
    return lines.join("\n");
  }

  /// Claude subscriptions, official Codex Plus cards and Antigravity
  /// accounts can say hi.
  function supportsSayHi(account, pool) {
    if (pool === "claude") return true;
    // Grok has no five-hour window to start.
    if (pool === "grok") return false;
    // An Antigravity provider's cards say no hi, as Codex's.
    if (pool === "antigravity") return !P.isProviderCard(account);
    const plan = (account.usage?.planType?.trim() || account.planType || "").trim().toLowerCase();
    return !P.isProviderCard(account) && plan === "plus";
  }
  /// Known usage with no future reset indicates an idle window; Antigravity's
  /// third-party (Claude) pool has its own, idle as well when reported so.
  /// A hi to one pool alone (`target`, as helloTarget) looks at its window.
  function fiveHourIdle(account, pool, target = null, now = Date.now() / 1000) {
    if (!account.usage) return false;
    const idle = (window) => !(window?.resetAt > now);
    const third = account.usage.thirdPartyFiveHour;
    if (target === true) return !!third && idle(third);
    if (target === false) return idle(account.usage.fiveHour);
    return idle(account.usage.fiveHour) || (pool === "antigravity" && !!third && idle(third));
  }
  /// The one pool a hi greets on a card with a model switch: the third
  /// party's when chosen (true), else its own (false); null on a card without
  /// one, whose hi greets each idle pool.
  function helloTarget(account, ctx) {
    const usage = account.usage;
    if (!usage?.thirdPartyFiveHour && !usage?.thirdPartyOneWeek) return null;
    return hasThirdParty(summary(account, ctx)) ? uiOf(ctx, account.id).thirdParty : null;
  }
  /// Whether the card's hi can start a window now.
  function canSayHi(account, ctx) {
    return (
      fiveHourIdle(account, ctx.pool, helloTarget(account, ctx), nowOf(ctx)) &&
      supportsSayHi(account, ctx.pool)
    );
  }
  /// Whether a card turns over to a back: a provider's account has only its
  /// greetings to edit.
  function hasBack(target, pool) {
    return !!target && (!!target.group || !target.grouped || supportsSayHi(target.account, pool));
  }
  /// The key a card's own refresh switches are saved under, as the core
  /// names it (refresh_plan.rs): an account's id, or `sub2api:<provider>`
  /// for a Sub2api provider's cards, which refresh together.
  function refreshKey(target) {
    return (target.group ? target.group.accounts[0] : target.account).refresh.key;
  }
  /// The card a provider's switch goes through: any active one will do.
  function switchTarget(group) {
    return group.accounts.find((a) => a.available !== false) || group.accounts[0];
  }
  /// A hi or renewal under way, or just done, which takes the place of the
  /// card's refresh button: its glyph, tooltip and whether it is done.
  function cardTask(account, ctx) {
    const { tr } = ctx;
    const ui = uiOf(ctx, account.id);
    if (ui.hi) {
      const prefix = {
        codex: "accounts.card.codex_say_hi",
        antigravity: "accounts.card.antigravity_say_hi",
      }[ctx.pool] || "accounts.card.say_hi";
      return { glyph: `say-hi-${ui.hi}`, title: tr(`${prefix}.${ui.hi}`), done: ui.hi === "received" };
    }
    if (ui.renewing) return { glyph: "renewal", title: tr("accounts.renewal.renewing"), done: false };
    if (ui.renewed) return { glyph: "renewal-done", title: tr("accounts.notice.token_renewed"), done: true };
    return null;
  }

  /// A card's usage rows: five hours and the week. Codex Free cards that
  /// report a 30-day monthly quota show Monthly (omitting empty Session/Weekly
  /// rows). Claude cards that report Fable quota add its weekly row beneath
  /// them. An Antigravity card's are Gemini's. A card's third-party pool, once
  /// reported and chosen (the model switch), shows instead. The weekly rows of
  /// a Sub2api too old to report them carry a notice saying so.
  function usageRows(windows, thirdParty, ctx) {
    const { tr, pool } = ctx;
    const remaining = ctx.mode === "remaining";
    const valueFormat = remaining ? "accounts.window.remaining_format" : "accounts.window.used_format";
    // Reserve both possible value formats even when a model's windows are
    // hidden, so switching between 9%, 100% and no data cannot resize meters.
    const valueSizing = { full: tr(valueFormat, "100%"), empty: tr("accounts.window.no_data") };
    const row = (window, title, tone, notice = null) => ({
      tone,
      title,
      window,
      value: window.available
        ? tr(valueFormat, `${window.primaryPercent}%`)
        : valueSizing.empty,
      valueSizing,
      reset: resetLabel(window, ctx.resetDisplay, ctx),
      // A known reset time switches between both forms and shows the other
      // one on hover, as in OpenQuota.
      resettable: window.available && window.resetAt != null,
      notice,
    });
    // Each pool's weekly usage came with its own Sub2api release.
    const version = windows.outdatedSub2api;
    const weekly = version
      ? [
          tr("accounts.window.sub2api_weekly_unavailable_format", version),
          pool === "grok"
            ? tr("accounts.window.sub2api_grok_weekly_upgrade")
            : tr("accounts.window.sub2api_weekly_upgrade"),
        ]
      : null;
    const five = tr("accounts.window.five_hour"), week = tr("accounts.window.one_week");
    const pair = (fiveHour, oneWeek) => [
      row(fiveHour, five, "five-hour"),
      row(oneWeek, week, "one-week", weekly),
    ];
    if (thirdParty) return pair(windows.thirdPartyFiveHour, windows.thirdPartyOneWeek);
    const shown = pair(windows.fiveHour, windows.oneWeek);
    if (MONTHLY_POOLS.includes(pool) && windows.oneMonth.available) {
      const month = row(windows.oneMonth, tr("accounts.window.one_month"), "one-month");
      if (!windows.oneWeek.available)
        return windows.fiveHour.available ? [row(windows.fiveHour, five, "five-hour"), month] : [month];
      shown.push(month);
    }
    if (pool === "claude" && windows.fableOneWeek.available)
      shown.push(row(windows.fableOneWeek, tr("accounts.window.fable"), "fable"));
    return shown;
  }
  /// A collapsed row's usage: five hours, then the week (or the month).
  function compactUsage(account, windows, ctx) {
    const mode = ctx.mode || "used";
    const item = (value, label) => {
      const rounded = P.roundDisplayPercent(value, mode);
      return { label, text: rounded === null ? "--" : `${rounded}%` };
    };
    // The month only where the full card's rows show it in the week's place.
    const monthly =
      MONTHLY_POOLS.includes(ctx.pool) && !windows.oneWeek.available && windows.oneMonth.available;
    return [
      item(P.displayPercent(account.usage?.fiveHour, mode), "5h"),
      monthly
        ? item(P.displayPercent(account.usage?.oneMonth, mode), "1m")
        : item(P.displayPercent(account.usage?.oneWeek, mode), "1w"),
    ];
  }
  /// The footer's credits and, where the pool banks them, resets, each with
  /// the expiry dates as its tooltip, and the notice of resets other than
  /// full ones. A negative reset count is unavailable, as in
  /// AccountCardPresentation.
  function balance(usage, credits, ctx) {
    const { tr } = ctx;
    const now = nowOf(ctx);
    const inventory = P.resetInventory(usage, now);
    const dates = P.balanceTooltips(usage, now);
    const expiry = (at) => at == null
      ? tr("accounts.card.expiry_unknown")
      : tr("accounts.card.reset_expires_format", P.formatReset(at, ctx.locale));
    // A balance of nothing (none, or zero) has no expiry to tell.
    const value = String(credits ?? "").trim();
    const hasCredits = value !== "" && value !== "--" && (!/\d/.test(value) || /[1-9]/.test(value));
    const footer = {
      credits: {
        text: tr("accounts.card.credits_format", credits === "unlimited" ? tr("accounts.card.unlimited") : credits),
        title: hasCredits
          ? (dates.creditDates.length ? dates.creditDates.map(expiry).join("\n") : expiry(null))
          : null,
      },
      resets: null,
      warning: null,
    };
    if (!RESET_POOLS.includes(ctx.pool)) return footer;
    let title = null;
    if (inventory.count !== null) {
      if (inventory.count > 0) {
        title = dates.resetDates.length
          ? dates.resetDates.map(({ at, count }) => `${expiry(at)}${count > 1 ? ` × ${count}` : ""}`).join("\n")
          : expiry(null);
      }
      if (usage?.resetsLastConfirmedAt != null) {
        const confirmed = tr("accounts.card.resets_last_confirmed_format",
          P.formatReset(usage.resetsLastConfirmedAt, ctx.locale));
        title = title ? `${title}\n${confirmed}` : confirmed;
      }
    }
    footer.resets = {
      text: inventory.count !== null
        ? tr("accounts.card.remaining_resets_format", inventory.count)
        : tr("accounts.card.resets_unavailable_format", "--"),
      title,
    };
    if (inventory.warning) {
      const kinds = {
        full: tr("accounts.card.reset_full"), fable: tr("accounts.card.reset_fable"),
        partial: tr("accounts.card.reset_partial"), unknown: tr("accounts.card.reset_unknown"),
      };
      footer.warning = dates.types.map(({ kind, count, at }) => `${kinds[kind]} × ${count} · ${expiry(at)}`);
    }
    return footer;
  }

  /// A card's actions, in its menu's order. Each says where it shows: as a
  /// button (`button`: at the card's head or footer, with what it shows) and
  /// in the card's menu (`menu`: its label and whether it waits), either or
  /// both. A separator shows in the menu alone.
  const separator = () => ({ kind: "separator", button: null, menu: { separator: true } });
  /// An account's: its usage warning, Sign in again, editing, a hi or
  /// renewal under way, refresh, say hi, renew, switch and removal. A
  /// provider's account has only its editing, greeting and task.
  function accountActions(account, ctx, grouped) {
    const { tr, pool, collapsed } = ctx;
    const ui = uiOf(ctx, account.id);
    const actions = [];
    // The usage error is hidden while the card refreshes, as in AccountCardView.
    if (!grouped && !collapsed && account.usageError && !ui.refreshing)
      actions.push({
        kind: "warning",
        message: account.usageError,
        button: { place: "footer", title: tr(account.usageError) },
        menu: null,
      });
    // An expired sign-in replaces refresh and switch with "Sign in again",
    // which pins the workspace only for a named team, as in the SwiftUI card.
    const reauthenticate = P.needsReauthentication(account, tr("error.accounts.sign_in_expired"));
    if (reauthenticate)
      actions.push({
        kind: "reauthenticate",
        workspace: account.workspaceTag ? account.accountId : null,
        button: {
          place: "footer",
          label: collapsed ? "" : tr("accounts.card.sign_in_again"),
          title: tr("accounts.card.sign_in_again"),
          compact: !!collapsed,
          disabled: !!ctx.adding || !!ctx.authorizing,
        },
        menu: null,
      });
    // The edit button takes the free end of the card's first line.
    if (!collapsed && hasBack({ account, grouped }, pool))
      actions.push({ kind: "edit", button: { place: "head" }, menu: { label: tr("accounts.card.edit") } });
    const sayHi = supportsSayHi(account, pool);
    const ready = sayHi && canSayHi(account, ctx);
    // A hi or renewal shows in the refresh button's place while it lasts; a
    // provider's account, refreshed with its provider, shows it alone.
    const task = reauthenticate ? null : cardTask(account, ctx);
    const taskAction = () => ({ kind: "task", button: { place: "footer", ...task }, menu: null });
    if (grouped) {
      if (task) actions.push(taskAction());
      if (sayHi)
        actions.push({
          kind: "say-hi",
          button: null,
          menu: { label: tr("accounts.say_hi.now"), disabled: !ready || !!ui.hi },
        });
      return actions;
    }
    if (actions.some((action) => action.menu)) actions.push(separator());
    if (task) actions.push(taskAction());
    // The menu's refresh waits while any request is under way; the button
    // spins while the card refreshes, and gives way to a hi or renewal.
    const busy = ui.refreshing || ui.hi === "sent" || ui.renewing;
    actions.push({
      kind: "refresh",
      button: reauthenticate || task
        ? null
        : {
            place: "footer",
            label: tr("common.refresh_usage"),
            nextAt: account.nextRefreshAt ?? null,
            spinning: ui.refreshing,
            // A card refreshing locks its own refresh button.
            disabled: !!ui.refreshing,
          },
      menu: { label: tr("common.refresh_usage"), disabled: busy },
    });
    if (sayHi)
      actions.push({
        kind: "say-hi",
        button: null,
        menu: { label: tr("accounts.say_hi.now"), disabled: busy || !ready || !!ui.hi },
      });
    if (account.renewal)
      actions.push({ kind: "renew", button: null, menu: { label: tr("accounts.renewal.now"), disabled: busy } });
    actions.push(switchAction(account, account.isCurrent, account.available, reauthenticate, ctx, [account]));
    actions.push(separator(), removeAction(account.email || account.label, P.isProviderCard(account), ctx));
    return actions;
  }
  /// The switch to `target`: a button on a card not in use, and in the menu
  /// as well for a Grok provider, which can reapply its config after an app
  /// or gateway update.
  function switchAction(target, current, available, hidden, ctx, cards) {
    const { tr } = ctx;
    const reapply = ctx.pool === "grok" && P.isProviderCard(cards[0]);
    return {
      kind: "switch",
      id: target.id,
      button: hidden || current
        ? null
        : {
            place: "footer",
            title: tr("accounts.card.switch_to_this"),
            available,
            // A switch locks only its own card's switch button.
            disabled: available === false || ctx.switchingId === target.id,
          },
      menu: reapply || !current
        ? {
            label: tr("accounts.card.switch_to_this"),
            disabled: target.available === false || ctx.switchingId != null,
          }
        : null,
    };
  }
  function removeAction(name, sub2api, ctx) {
    return {
      kind: "remove",
      name,
      sub2api,
      button: null,
      menu: { label: ctx.tr("common.remove"), destructive: true },
    };
  }
  /// A whole Sub2api provider's: its cards' usage warnings, editing, its
  /// refresh and switch, which its cards share, and its removal.
  function providerActions(group, title, ctx) {
    const { tr, collapsed } = ctx;
    const cards = group.accounts;
    const uis = cards.map((account) => uiOf(ctx, account.id));
    const refreshing = uis.some((ui) => ui.refreshing);
    const current = cards.some((account) => account.isCurrent);
    const actions = [];
    if (!collapsed && !refreshing) {
      const errors = [...new Set(cards.map((a) => a.usageError).filter(Boolean).map((e) => tr(e)))];
      if (errors.length) {
        const message = errors.join("\n");
        actions.push({ kind: "warning", message, button: { place: "footer", title: message }, menu: null });
      }
    }
    if (!collapsed) {
      actions.push({ kind: "edit", button: { place: "head" }, menu: { label: tr("accounts.card.edit") } });
      actions.push(separator());
    }
    const nextAt = cards.map((account) => account.nextRefreshAt).filter(Number.isFinite);
    actions.push({
      kind: "refresh",
      button: {
        place: "footer",
        label: tr("common.refresh_usage"),
        // Its first card due.
        nextAt: nextAt.length ? Math.min(...nextAt) : null,
        spinning: refreshing,
        disabled: refreshing || uis.some((ui) => ui.hi === "sent"),
      },
      menu: {
        label: tr("common.refresh_usage"),
        disabled: uis.some((ui) => ui.refreshing || ui.hi === "sent" || ui.renewing),
      },
    });
    const target = switchTarget(group);
    actions.push(switchAction(target, current, cards.some((a) => a.available !== false), false, ctx, cards));
    actions.push(separator(), removeAction(title, true, ctx));
    return actions;
  }

  /// An account's card, or its row when the list is collapsed; `grouped`
  /// for one of a Sub2api provider's cards, shown in its provider's.
  function cardView(account, ctx, grouped = false) {
    const { tr } = ctx;
    const windows = summary(account, ctx);
    const full = account.email || account.accountId || account.label;
    const name = P.clean(account.email) || P.clean(account.accountId) || P.clean(account.label) || "";
    const ui = uiOf(ctx, account.id);
    // The CURRENT tag alone marks the active account, as in the SwiftUI card;
    // a provider's group carries its SUB2API and CURRENT tags once.
    const tags = [];
    if (account.planTag) tags.push({ className: "tag plan", text: account.planTag.label });
    if (account.workspaceTag && !grouped) tags.push({ className: "tag muted", text: account.workspaceTag });
    if (account.isCurrent && !grouped) tags.push({ className: "tag good", text: tr("accounts.card.current") });
    // Imported but not yet requested upstream: refreshing the card sends the
    // first request, after its proxy is set.
    if (account.awaitingFirstRequest)
      tags.push({
        className: "tag muted",
        text: tr("accounts.card.awaiting_first_request"),
        title: tr("accounts.card.awaiting_first_request.help"),
      });
    if (account.workspaceStatus === "deactivated" || account.available === false)
      tags.push({ className: "tag bad", text: tr("accounts.card.status.deactivated") });
    // A card reporting a third-party pool chooses whose windows its usage
    // rows show, its own or the third party's.
    const thirdParty = hasThirdParty(windows) && ui.thirdParty;
    const models = hasThirdParty(windows)
      ? thirdParty
        ? { shown: THIRD_PARTY_MODEL, other: OWN_MODEL[ctx.pool], thirdParty }
        : { shown: OWN_MODEL[ctx.pool], other: THIRD_PARTY_MODEL, thirdParty }
      : null;
    const interaction = P.collapsedInteraction({
      collapsed: !!ctx.collapsed,
      current: account.isCurrent,
      switching: ctx.switchingId === account.id,
      hovered: false,
    });
    return {
      id: account.id,
      grouped,
      full,
      name: ctx.collapsed && name.includes("@") ? name.slice(0, name.indexOf("@")) : name,
      current: !!account.isCurrent,
      // The plan tag's tier color on the card or row holding it.
      accent: account.planTag?.accent ?? null,
      plan: account.planTag?.label ?? null,
      tags,
      models,
      // A provider's account without a plan tag or switch has no tag row;
      // its name's line is then its first.
      headed: !(grouped && !tags.length && !models),
      rows: usageRows(windows, thirdParty, ctx),
      compact: compactUsage(account, windows, ctx),
      footer: balance(account.usage, windows.credits, ctx),
      // A collapsed row: its dot, whether it shows as the one in use, and
      // its switch on hover.
      row: {
        dot: !grouped,
        current: !grouped && !!account.isCurrent,
        canReveal: !grouped && interaction.canReveal,
        revealed: !grouped && interaction.visible,
      },
      // Its controls: its own, or its provider's on the first row of a
      // collapsed provider (`controls: "provider"`).
      controls: "account",
      actions: accountActions(account, ctx, grouped),
    };
  }
  /// A Sub2api provider's cards close together in one card, which shows once
  /// whether the provider is in use, with its controls where a single
  /// account's card has them. Collapsed, it has no head row: its first
  /// account's row carries the provider's dot and controls, as a single
  /// account's row does. `title` is the provider's name.
  function providerView(group, title, ctx) {
    const { tr } = ctx;
    const current = group.accounts.some((a) => a.isCurrent);
    const actions = providerActions(group, title, ctx);
    const members = group.accounts.map((account) => cardView(account, ctx, true));
    if (ctx.collapsed && members.length) {
      const interaction = P.collapsedInteraction({
        collapsed: true,
        current,
        switching: ctx.switchingId === switchTarget(group).id,
        hovered: false,
      });
      members[0] = {
        ...members[0],
        row: { dot: true, current, canReveal: interaction.canReveal, revealed: interaction.visible },
        controls: "provider",
        actions,
      };
    }
    const tags = [{ className: "tag muted", text: P.clean(group.accounts[0]?.sourceTag) || "SUB2API" }];
    if (current) tags.push({ className: "tag good", text: tr("accounts.card.current") });
    return { key: group.key, title, current, tags, members, actions };
  }

  return {
    cardView,
    providerView,
    resetLabel,
    paceTitle,
    refreshTitle,
    tokenLabel,
    details,
    supportsSayHi,
    fiveHourIdle,
    helloTarget,
    canSayHi,
    hasBack,
    refreshKey,
  };
});
