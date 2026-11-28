/* A card's back, where it is edited: its proxy, token renewal, greetings,
   the pool's refresh and its removal, and turning cards over to it and back.
   What a card has under way and its draft are its pool's Accounts
   controller's (controllers.js); this paints the back from them and runs
   the turns through the app's animation. Used by the UI and node:test.

   `create(host)` takes what the back needs from the app, read when used:
     el, button             build nodes, as app.js's
     tr, locale, pool       the language and the pool shown
     preferences            the app's preferences (the shared account proxy,
                            the refresh intervals)
     controller             the shown pool's Accounts controller
     collapsed              whether the list is collapsed into rows
     cardContext()          card-view.js's context for the cards
     providerTitle(account) the title of a Sub2api card's provider
     turnCard(key, direction, change)   the flip animation, around change()
     renderEditing()        renders the card being edited again
     focusBackField(field)  focuses a field of the card being edited
     notice, action, queuePreferenceSave   app.js's
     renewNow, greet, removeCards          a card's actions
     timers                 { setTimeout } for tests */
(function (root, factory) {
  const node = typeof module === "object" && module.exports;
  const api = node
    ? factory(require("./presentation.js"), require("./card-view.js"))
    : factory(root.CopoolPresentation, root.CopoolCardView);
  if (node) module.exports = api;
  else root.CopoolCardBack = api;
})(globalThis, function (P, V) {
  function create(host) {
    const { el, button } = host;
    const tr = (...args) => host.tr(...args);
    const timers = host.timers ?? globalThis;
    /// The shared account proxy a blank proxy uses, when one is set; a
    /// Sub2api provider's never does.
    function sharedProxy(owner) {
      return owner.sourceTag === "SUB2API" ? "" : (host.preferences?.accountProxyUrl || "").trim();
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
    /// Turns a card over to its settings (the controller's edit), turning back
    /// any other card being edited.
    async function editCard(key) {
      if (host.collapsed) return;
      const controller = host.controller;
      const previous = controller.editing;
      const open = controller.edit(key, (error) => host.notice(error, true));
      if (!open) return;
      if (previous) host.turnCard(previous.key, -1, () => controller.close(previous));
      await host.turnCard(key, 1, open);
    }
    /// Saves the card's proxy, then turns it back; an invalid proxy keeps it
    /// open with the error.
    async function finishEditing() {
      const controller = host.controller;
      const state = controller.editing;
      if (!state || state.saving) return;
      if (state.proxy) {
        const saved = await controller.saveProxyDraft(state);
        // Turned back, superseded or another pool shown meanwhile: nothing to turn.
        if (controller !== host.controller || controller.editing !== state) return;
        if (!saved) {
          // Done shows enabled again, to save the corrected proxy.
          host.renderEditing();
          return;
        }
      }
      await host.turnCard(state.key, -1, () => controller.close(state));
    }
    /// Turns the card back without saving its proxy; a card still turning over
    /// turns back instead of showing its back.
    function cancelEditing() {
      const controller = host.controller;
      const state = controller.cancelTurn();
      if (state) host.turnCard(state.key, -1, () => controller.close(state));
    }
    /// A card's settings side: its proxy, token renewal, greetings, the pool's
    /// refresh and its removal; a provider's group has the provider's proxy;
    /// a provider's account its greetings alone.
    function cardBack(target) {
      const state = host.controller.editing;
      const { account, group, grouped } = target;
      const cards = group ? group.accounts : [account];
      const current = cards.some((a) => a.isCurrent);
      const name = group
        ? host.providerTitle(group.accounts[0])
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
      if (state.proxy) node.append(proxySection(state, P.proxyOwner(target)));
      if (account && !grouped && account.renewal) node.append(renewalSection(account));
      if (account && V.supportsSayHi(account, host.pool) && state.schedule)
        node.append(greetingSection(account, state));
      if (!grouped) node.append(refreshSection(current, V.refreshKey(target)));
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
      const help = backHelp(state.proxyError ? tr(state.proxyError) : proxyHelp(owner), !!state.proxyError);
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
      const ui = host.controller.cardState(account.id);
      const busy = ui.refreshing || ui.hi === "sent" || ui.renewing;
      // Under its "Token renewal" title, the time left alone.
      const expiry = P.tokenExpiry(account.tokenExpiresAt, Date.now() / 1000, host.locale);
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
        backButton(tr("accounts.renewal.now"), () => host.renewNow(account), busy),
      );
      section.group.append(
        backRow(
          tr("accounts.renewal.auto"),
          backSwitch("renewal-auto", account.renewal === "always", tr("accounts.renewal.auto"), (on) =>
            host.action(() => host.controller.setRenewal(account.id, on ? "always" : "never")),
          ),
        ),
      );
      return section;
    }
    /// Saying hi now, automatically, and at which times. Each edit is made
    /// to the draft as it is then (`state.schedule`), which an edit made
    /// before it has changed already.
    function greetingSection(account, state) {
      const schedule = state.schedule;
      const ui = host.controller.cardState(account.id);
      // The controller turns a hi away while the card refreshes or renews.
      const busy = !!ui.hi || ui.refreshing || ui.renewing;
      const section = backSection(
        tr("accounts.say_hi.title"),
        backButton(
          tr("accounts.say_hi.now"),
          () => host.greet(account),
          busy || !V.canSayHi(account, host.cardContext()),
        ),
      );
      // Greeting when the 5h window is idle and at set times are separate:
      // each greets on its own.
      section.group.append(
        backRow(
          tr("accounts.say_hi.auto"),
          backSwitch("greeting-auto", schedule.auto, tr("accounts.say_hi.auto"), (on) =>
            saveGreeting(account.id, state, on, state.schedule.times),
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
          saveGreeting(
            account.id,
            state,
            state.schedule.auto,
            P.withoutGreetingTime(state.schedule.times, time),
          ),
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
          const times = P.withGreetingTime(state.schedule.times, input.value);
          if (times) saveGreeting(account.id, state, state.schedule.auto, times);
          else host.renderEditing();
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
            host.renderEditing();
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
          host.renderEditing();
          host.focusBackField("greeting-time");
        };
        times.append(add);
      }
      daily.append(dailyLabel, times);
      section.group.append(daily);
      return section;
    }
    /// Shows a greeting schedule on the card being edited at once, and again
    /// once it is saved (or put back, when it could not be).
    async function saveGreeting(id, state, auto, times) {
      const controller = host.controller;
      const saved = controller.saveGreeting(state, id, auto, times);
      if (controller.editing === state) host.renderEditing();
      try {
        await saved;
      } catch (error) {
        host.notice(error, true);
      }
      if (controller.editing === state) host.renderEditing();
    }
    /// A card's background refresh: the pool's intervals, which all its cards
    /// share (the card in use's only on its own card), and the card's own
    /// speed-up and slow-down, which follow the pool's until set.
    function refreshSection(current, key) {
      const refresh = host.preferences.refresh[host.pool];
      const own = host.preferences.refresh.cards?.[host.pool]?.[key] ?? {};
      const adaptive = own.adaptive ?? refresh.adaptive;
      const backoff = own.backoff ?? refresh.backoff;
      const cardPool = host.pool;
      // On the refresh as saved by then: another change made before this
      // one is saved stays.
      const saved = (change) =>
        host.queuePreferenceSave((latest) => ({ refresh: change(latest.refresh) }))
          .catch((error) => host.notice(error, true))
          .finally(() => host.renderEditing());
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
      const choices = P.refreshChoices(host.pool, refresh.seconds);
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
      const interval = (seconds) => P.formatInterval(seconds, host.locale);
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
      if (host.pool === "claude" && !key.startsWith("sub2api:"))
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
          host.renderEditing();
          timers.setTimeout(() => {
            if (host.controller.editing !== state || !state.confirmRemove) return;
            state.confirmRemove = false;
            host.renderEditing();
          }, 3000);
          return;
        }
        host.controller.close(state);
        host.removeCards(cards, provider || cards[0].sourceTag === "SUB2API");
      };
      row.append(remove);
      return row;
    }
    return {
      cardBack,
      editCard,
      finishEditing,
      cancelEditing,
      proxyHelp,
      proxyPlaceholder,
      proxySection,
      renewalSection,
      greetingSection,
      refreshSection,
      removalSection,
      saveGreeting,
    };
  }
  return { create };
});
