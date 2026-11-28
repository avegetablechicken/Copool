/* A card's front, painted from its view (card-view.js): its tags, usage
   rows and meters, footer, buttons and menu, its model switch, a Sub2api
   provider's group, and the meters sliding to their new levels as the cards
   render again. What a card shows is card-view.js's; what it has under way
   is its pool's Accounts controller's. Used by the UI and node:test.

   `create(host)` takes what the cards need from the app, read when used:
     el, button, icon        build nodes, as app.js's
     tr, locale, pool        the language and the pool shown
     collapsed, resetDisplay how the list shows
     preferences             the app's preferences
     controller              the shown pool's Accounts controller
     switchingId             the account being switched to, or null
     cardContext()           card-view.js's context for the cards
     providerTitle(account)  the title of a Sub2api card's provider
     accountProxy(account)   the proxy an account's requests go through
     cardBack(target)        a card's back (card-back.js), while it is edited
     modelBrand(model)       a model's mark for the model switch
     showResetPopover(trigger, lines, wrapping)   menus.js's
     toggleResetDisplay()    switches reset times between their two forms
     getComputedStyle, reducedMotion()            the page's styles
     notice, signIn, editCard, refreshUsage, refreshProvider, greet,
     renewNow, switchAccount, confirmRemoval      a card's actions */
(function (root, factory) {
  const node = typeof module === "object" && module.exports;
  const api = node
    ? factory(require("./presentation.js"), require("./card-view.js"))
    : factory(root.CopoolPresentation, root.CopoolCardView);
  if (node) module.exports = api;
  else root.CopoolCardFace = api;
})(globalThis, function (P, V) {
  const METER_HEIGHT = 4; // .meter in style.css
  function create(host) {
    const { el, button, icon } = host;
    const tr = (...args) => host.tr(...args);
    const meterWindows = new WeakMap();
    function refreshMeterPace(track, now) {
      const window = meterWindows.get(track);
      const tone = P.usageTone(window, now);
      track.classList.toggle("high", tone === "high");
      track.classList.toggle("full", tone === "full");
      const title = V.paceTitle(window, { locale: host.locale, tr, now });
      if (track.title !== title) track.title = title;
    }
    /// A card's usage row (card-view.js): its label, meter, value and reset.
    function usageWindow(usage) {
      const { window: view, title } = usage;
      const track = el(
        "div",
        `meter${view.available && view.tone ? ` ${view.tone}` : ""}`,
      );
      meterWindows.set(track, view.paceWindow);
      track.title = V.paceTitle(view.paceWindow, { locale: host.locale, tr });
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
      // A known reset time switches between both forms and shows the other one
      // on hover, as in OpenQuota.
      const reset = usage.resettable
        ? button("", null, "usage-reset", () => host.toggleResetDisplay())
        : el("span", "usage-reset");
      reset.textContent = usage.reset;
      if (usage.resettable)
        reset.addEventListener("mouseenter", () => {
          reset.title = V.resetLabel(
            view,
            host.resetDisplay === "countdown" ? "exact" : "countdown",
            { locale: host.locale, tr, now: Date.now() / 1000 },
          );
        });
      const row = el("div", `usage ${usage.tone}${view.available ? "" : " no-data"}`);
      row.append(
        el("span", "usage-label", title),
        track,
        el("span", "usage-value", usage.value),
        reset,
      );
      return weeklyNotice(row, usage.notice);
    }
    function compactUsage({ label, text }) {
      const node = el("span", "usage-value");
      node.append(`${label} `, el("b", "", text));
      node.setAttribute("aria-label", `${label}: ${text}`);
      return node;
    }
    /// A hi or renewal under way, or just done (card-view.js), in the place of
    /// the card's refresh button.
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
      const control = button("", "info", "icon-btn card-edit", () => host.editCard(key));
      const cardPool = host.pool;
      const update = () => {
        if (host.pool !== cardPool) return;
        control.title = V.details(host.controller.flipTarget(key), host.cardContext());
      };
      update();
      control.onpointerenter = update;
      control.onfocus = update;
      control.setAttribute("aria-label", `${tr("accounts.card.edit")} ${name}`);
      return control;
    }
    /// A card's menu, at the pointer: editing it, then what its buttons do and
    /// what it has no button for, and its removal (card-view.js lists them).
    function cardMenu(target) {
      return cardView(target)
        .actions.filter((action) => action.menu)
        .map((action) =>
          action.kind === "separator"
            ? { separator: true }
            : { ...action.menu, run: actionHandler(action, target) },
        );
    }
    /// The view of the card a flip key names (flipTarget): an account's, maybe
    /// one of a provider's, or a provider's group.
    function cardView(target, context = host.cardContext()) {
      return target.group
        ? V.providerView(target.group, host.providerTitle(target.group.accounts[0]), context)
        : V.cardView(target.account, context, !!target.grouped);
    }
    /// What a card's action does, for its account or its provider's group.
    function actionHandler(action, { account, group }) {
      const cards = group ? group.accounts : [account];
      switch (action.kind) {
        case "warning":
          return () => host.notice(action.message, true);
        case "reauthenticate":
          return () => host.signIn(action.workspace, "reauthenticate");
        case "edit":
          return () => host.editCard(group ? `sub2api:${group.key}` : account.id);
        case "refresh":
          return () => (group ? host.refreshProvider(group) : host.refreshUsage(account.id));
        case "say-hi":
          return () => host.greet(account);
        case "renew":
          return () => host.renewNow(account);
        case "switch":
          return () => host.switchAccount(cards.find((a) => a.id === action.id));
        case "remove":
          return () => host.confirmRemoval(cards, action.name, action.sub2api);
        default:
          return () => {};
      }
    }
    /// The plan tag's tier color on the card or row holding it.
    function setPlanAccent(node, view) {
      if (view.accent) node.dataset.accent = view.accent;
    }
    /// A card's tags: its plan (named by the core, plan_tags.rs), workspace,
    /// CURRENT and status.
    function cardTags(tags) {
      const node = el("span", "account-tags");
      for (const tag of tags) {
        const span = el("span", tag.className, tag.text);
        if (tag.title) span.title = tag.title;
        node.append(span);
      }
      return node;
    }
    /// A refresh button's tooltip, followed by how long until the automatic
    /// refresh at `nextAt()`; worked out again as the pointer enters.
    function refreshTooltip(target, base, nextAt) {
      const update = () => {
        target.title = V.refreshTitle(base, nextAt(), { locale: host.locale, tr });
      };
      update();
      target.onpointerenter = update;
    }
    /// A card's buttons at its bottom right, clear of long account names, as
    /// in the SwiftUI card: its usage warning, then Sign in again, or its hi or
    /// renewal under way or refresh, and its switch. `owner` is the account or
    /// provider's group they act for, `full` its name.
    function cardControls(actions, full, owner) {
      const node = el("span", "block-aside");
      for (const action of actions) {
        const shown = action.button;
        if (shown?.place !== "footer") continue;
        const run = actionHandler(action, owner);
        if (action.kind === "warning") {
          const warning = button("", "warning", "icon-btn error-indicator", run);
          warning.title = shown.title;
          warning.setAttribute("aria-label", shown.title);
          node.append(warning);
        } else if (action.kind === "reauthenticate") {
          const signInAgain = button(
            shown.label,
            "reauthenticate",
            shown.compact ? "icon-btn card-reauth" : "btn quiet card-reauth",
            run,
          );
          signInAgain.title = shown.title;
          signInAgain.setAttribute("aria-label", `${shown.title} ${full}`);
          signInAgain.disabled = shown.disabled;
          node.append(signInAgain);
        } else if (action.kind === "task") {
          node.append(taskIndicator(shown, full));
        } else if (action.kind === "refresh") {
          const refresh = button("", "refresh", "icon-btn card-refresh", run);
          refresh.setAttribute("aria-label", `${shown.label} ${full}`);
          refreshTooltip(refresh, shown.label, () => shown.nextAt);
          refresh.classList.toggle("spinning", shown.spinning);
          refresh.disabled = shown.disabled;
          node.append(refresh);
        } else if (action.kind === "switch") {
          const control = button("", "switch", "icon-btn card-switch", run);
          control.dataset.id = action.id;
          control.dataset.available = String(shown.available);
          control.title = shown.title;
          control.setAttribute("aria-label", `${shown.title} ${full}`);
          control.disabled = shown.disabled;
          node.append(control);
        }
      }
      return node;
    }
    /// The footer's credits and resets with their expiry tooltips, and the
    /// notice of resets other than full ones, which opens a popover.
    function accountBalanceText(footer) {
      const footerText = el("span", "account-footer-text");
      const credits = el("span", "account-credits", footer.credits.text);
      if (footer.credits.title) credits.title = footer.credits.title;
      footerText.append(credits);
      if (!footer.resets) return footerText;
      const resets = el("span", "account-resets", footer.resets.text);
      if (footer.resets.title) resets.title = footer.resets.title;
      footerText.append(" · ", resets);
      if (footer.warning) {
        const lines = footer.warning;
        const warning = button("", "reset-notice", "reset-warning", () => host.showResetPopover(warning, lines, true));
        warning.type = "button";
        warning.setAttribute("aria-label", lines.join("\n"));
        warning.setAttribute("aria-haspopup", "dialog");
        warning.setAttribute("aria-expanded", "false");
        // Sibling of the reset text: the button has no expiry tooltip.
        footerText.append(warning);
      }
      return footerText;
    }

    /// An account's card, or its row when the list is collapsed, painted from
    /// its view (card-view.js). A collapsed provider's first row carries the
    /// provider's controls (`owner` its group).
    function accountCard(
      account,
      grouped = false,
      view = V.cardView(account, host.cardContext(), grouped),
      owner = { account },
    ) {
      if (!host.collapsed && host.controller.editing?.key === account.id) {
        const target = host.controller.flipTarget(account.id);
        if (target) return host.cardBack(target);
      }
      const controlsName = owner.group ? host.providerTitle(owner.group.accounts[0]) : view.full;
      if (host.collapsed) {
        const row = el("div", `account-row${view.row.current ? " current running" : ""}`);
        row.classList.toggle("can-reveal", view.row.canReveal);
        row.classList.toggle("revealed", view.row.revealed);
        setPlanAccent(row, view);
        const name = el("span", "account-row-name", view.name);
        name.title = view.full;
        row.append(
          el("span", view.row.dot ? "dot" : ""),
          name,
          view.plan ? el("span", "tag plan", view.plan) : el("span", "tag plan placeholder"),
          compactUsage(view.compact[0]),
          compactUsage(view.compact[1]),
          cardControls(view.actions, controlsName, owner),
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
          : `block account${view.current ? " current" : ""}`,
      );
      setPlanAccent(card, view);
      card.dataset.cardId = account.id;
      card.dataset.flipKey = account.id;
      const head = el("div", "block-head");
      const name = el("div", "account-name");
      const title = el("span", "block-title", view.full);
      title.title = view.full;
      name.append(title);
      head.append(cardTags(view.tags));
      const models = modelSwitch(view, account.id);
      // The edit button takes the free end of the first line, before the switch.
      if (view.actions.some((action) => action.button?.place === "head"))
        (view.headed ? head : name).append(editButton(account.id, view.full));
      if (models) head.append(models);
      // A card is dragged by its tags and name, as OpenQuota by its header.
      if (!grouped) head.dataset.reorderHandle = name.dataset.reorderHandle = "";
      const footer = el("div", "account-footer");
      footer.append(accountBalanceText(view.footer), cardControls(view.actions, controlsName, owner));
      card.append(
        ...(view.headed ? [head] : []),
        name,
        ...view.rows.map(usageWindow),
        footer,
      );
      return card;
    }
    /// The account whose model switch was just clicked, while its card renders.
    let modelsSwapped = null;
    /// A card reporting a third-party pool chooses whose windows its usage rows
    /// show, its own or the third party's, with one icon button at the end of its
    /// tag row: both models' marks stacked, the shown model's in front and the
    /// other's faded behind it, which a click brings to the front.
    function modelSwitch(view, accountId) {
      if (!view.models) return null;
      const { shown: label, other, thirdParty: third } = view.models;
      const swapped = modelsSwapped === accountId;
      const control = button("", null, `model-switch${swapped ? " swapped" : ""}`, () => {
        // The switch rendered anew trades its marks' places as it appears.
        modelsSwapped = accountId;
        try {
          host.controller.showThirdParty(accountId, !third);
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
      behind.append(host.modelBrand(other));
      const front = el("span", "model-mark front");
      front.append(host.modelBrand(label));
      control.append(behind, front);
      return control;
    }
    /// The fills of a card's usage rows, in order, as they look now (partway
    /// through a slide too): their levels in percent and their colors.
    function meterStates(card) {
      return [...card.querySelectorAll(":scope > .usage .meter-fill")].map((fill) => {
        const style = host.getComputedStyle(fill);
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
      if (!before.size || host.reducedMotion()) return;
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
    /// A weekly row's notice that its Sub2api reports no weekly usage: `lines`
    /// (card-view.js) open in a popover.
    function weeklyNotice(row, lines) {
      if (!lines) return row;
      const notice = button("", "reset-notice", "reset-warning", () =>
        host.showResetPopover(notice, lines, true),
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
      const flipKey = `sub2api:${group.key}`;
      if (!host.collapsed && host.controller.editing?.key === flipKey) return host.cardBack({ group });
      const view = V.providerView(group, host.providerTitle(group.accounts[0]), host.cardContext());
      // Collapsed, the first account's row carries the provider's controls.
      const members = group.accounts.map((account, index) =>
        accountCard(
          account,
          true,
          view.members[index],
          view.members[index].controls === "provider" ? { group } : { account },
        ),
      );
      if (host.collapsed) {
        const wrapper = el("div", "account-row-group");
        wrapper.dataset.reorderHandle = "";
        wrapper.dataset.flipKey = flipKey;
        wrapper.append(...members);
        return wrapper;
      }
      // A provider's cards are current together.
      const block = el(
        "section",
        `block account account-group${view.current ? " current" : ""}`,
      );
      const head = el("div", "block-head");
      const name = el("span", "block-title", view.title);
      name.title = view.title;
      head.append(name, ...view.tags.map((tag) => el("span", tag.className, tag.text)));
      head.dataset.reorderHandle = "";
      block.dataset.flipKey = flipKey;
      const list = el("div", "account-group-members");
      list.append(...members);
      const footer = el("div", "account-footer");
      footer.append(
        el("span", "account-footer-text"),
        cardControls(view.actions, view.title, { group }),
      );
      head.append(editButton(flipKey, view.title));
      block.append(head, list, footer);
      return block;
    }
    /// Every meter's pace color and tooltip in `list` at `now`, which change
    /// with time alone.
    function refreshMeterPaces(list, now) {
      for (const track of list.querySelectorAll(".meter")) refreshMeterPace(track, now);
    }
    /// What the cards render from: they render again only when it changes.
    function renderState(accounts) {
      const context = host.cardContext();
      const controller = host.controller;
      return [
        accounts,
        // Relative times change without new data.
        accounts.map((account) => V.tokenLabel(account, context)),
        host.resetDisplay,
        host.resetDisplay === "countdown" &&
          accounts.map((account) => V.cardView(account, context).rows.map((row) => row.reset)),
        host.collapsed,
        host.locale,
        host.preferences?.usageDisplay,
        accounts.map((account) => host.accountProxy(account)),
        // Cards group under their provider, which may load after them.
        accounts.map((account) => host.providerTitle(account)),
        host.preferences?.accountOrder?.[host.pool],
        // What each card has under way, and the pool its model switch shows.
        accounts.map((account) => controller.cardState(account.id)),
        host.switchingId,
        // Which card shows its back: a removal closes it before the cards change,
        // and they may not change at all when it fails.
        controller.editing?.key ?? null,
        controller.adding,
        controller.authorizing,
        host.pool,
      ];
    }
    return {
      accountCard,
      providerGroup,
      cardMenu,
      cardView,
      actionHandler,
      cardTags,
      setPlanAccent,
      cardControls,
      accountBalanceText,
      editButton,
      refreshTooltip,
      refreshMeterPace,
      refreshMeterPaces,
      usageWindow,
      modelSwitch,
      weeklyNotice,
      meterStates,
      cardMeters,
      slideMeters,
      renderState,
    };
  }
  return { create };
});
