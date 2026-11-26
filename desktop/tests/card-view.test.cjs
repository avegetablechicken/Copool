const { test } = require("node:test");
const assert = require("node:assert/strict");
const P = require("../ui/presentation.js");
const V = require("../ui/card-view.js");

// What a card shows, decided by card-view.js from an account and its
// context: the tests pass data and look at the view, never at app.js.
const NOW = 1_800_000_000;
const tr = (key, ...args) => [key, ...args].join(" ");
/// A card's context in `pool`, nothing under way unless `ui` says so.
const context = (extra = {}) => ({
  pool: "codex",
  mode: "used",
  resetDisplay: "exact",
  collapsed: false,
  locale: "en",
  tr,
  now: NOW,
  refresh: { seconds: 30, fullEvery: 6 },
  accountProxy: "",
  switchingId: null,
  adding: false,
  authorizing: null,
  ...extra,
});
/// `ui` for these cards' states under way, by id.
const states = (byId) => (id) => byId[id] ?? {};
const row = (view, tone) => view.rows.find((r) => r.tone === tone);
const rows = (view) => view.rows.map((r) => [r.tone, r.window.available]);
const action = (view, kind) => view.actions.find((a) => a.kind === kind);
/// The footer's buttons, in order, by kind.
const buttons = (view, place = "footer") =>
  view.actions.filter((a) => a.button?.place === place).map((a) => a.kind);
/// The menu's labels in order, a separator as "—".
const menu = (view) =>
  view.actions.filter((a) => a.menu).map((a) => (a.menu.separator ? "—" : a.menu.label));
const window = (usedPercent, resetAt = null, extra = {}) => ({ usedPercent, resetAt, ...extra });
const a = {
  id: "account-1",
  accountId: "account-1",
  email: "dev@example.com",
  planType: "business",
  teamName: "workspace-a",
  isCurrent: true,
  usage: {
    planType: "business",
    fiveHour: { usedPercent: 27.2, resetAt: 1763216000 },
    oneWeek: { usedPercent: 52.6 },
    credits: { balance: "128", unlimited: false },
  },
};

test("A collapsed card shows the email's name, its credits and compact usage", () => {
  const view = V.cardView(a, context({ collapsed: true }));
  assert.equal(view.name, "dev");
  assert.equal(view.full, "dev@example.com");
  assert.equal(view.footer.credits.text, "accounts.card.credits_format 128");
  assert.deepEqual(view.compact, [{ label: "5h", text: "27%" }, { label: "1w", text: "52%" }]);
  assert.equal(V.cardView(a, context()).name, "dev@example.com");
});

test("A card without usage windows shows no data, unlimited credits as such", () => {
  const view = V.cardView({ id: "b", accountId: "account-2", usage: { credits: { unlimited: true } } }, context());
  assert.equal(view.name, "account-2");
  assert.equal(view.footer.credits.text, "accounts.card.credits_format accounts.card.unlimited");
  const five = row(view, "five-hour");
  assert.equal(five.window.available, false);
  assert.equal(five.window.progressPercent, 100);
  assert.equal(five.window.reset, "--");
  assert.equal(five.value, "accounts.window.no_data");
  assert.equal(five.reset, "accounts.window.reset_unavailable");
  assert.equal(five.resettable, false);
});

test("A deactivated workspace or unavailable account shows the deactivated tag", () => {
  const bad = ["tag bad", "accounts.card.status.deactivated"];
  const tags = (account) => V.cardView(account, context()).tags.map((t) => [t.className, t.text]);
  assert.deepEqual(tags({ ...a, workspaceStatus: "deactivated" }).at(-1), bad);
  assert.deepEqual(tags({ ...a, available: false }).at(-1), bad);
  assert(!tags(a).some(([className]) => className === "tag bad"));
});

test("Remaining mode shows what is left in the rows and the compact row", () => {
  const account = { ...a, usage: { fiveHour: { usedPercent: 72 }, oneWeek: { usedPercent: 30 } } };
  const view = V.cardView(account, context({ mode: "remaining" }));
  const five = row(view, "five-hour");
  assert.equal(five.window.progressPercent, 28);
  assert.equal(five.window.primaryPercent, 28);
  assert.equal(five.window.secondaryPercent, 72);
  assert.equal(five.value, "accounts.window.remaining_format 28%");
  assert.deepEqual(V.cardView(account, context({ mode: "remaining", collapsed: true })).compact.map((c) => c.text), ["28%", "70%"]);
  assert.equal(row(V.cardView(account, context()), "five-hour").value, "accounts.window.used_format 72%");
});

test("A missing five-hour window has no data while the weekly one shows", () => {
  const view = V.cardView({ id: "w", accountId: "weekly-only", usage: { oneWeek: { usedPercent: 38, resetAt: 1763216000 } } }, context({ collapsed: true }));
  assert.equal(row(view, "five-hour").window.available, false);
  assert.equal(row(view, "five-hour").window.reset, "--");
  assert.equal(view.compact[0].text, "--");
  assert.equal(row(view, "one-week").window.available, true);
});

test("A reset shows its date and time, or counts down as OpenQuota does", () => {
  const usage = { fiveHour: window(1, NOW + 3 * 3600 + 20 * 60), oneWeek: window(1, NOW + 120) };
  const exact = V.cardView({ id: "r", usage }, context());
  assert.equal(row(exact, "five-hour").window.resetAt, NOW + 3 * 3600 + 20 * 60);
  assert.equal(row(exact, "five-hour").reset, `accounts.window.reset_at_format ${P.formatReset(NOW + 12000, "en")}`);
  assert.equal(row(exact, "five-hour").resettable, true);
  const countdown = V.cardView({ id: "r", usage }, context({ resetDisplay: "countdown" }));
  assert.equal(row(countdown, "five-hour").reset, "accounts.window.resets_in_format 3h 20m");
  assert.equal(row(countdown, "one-week").reset, "accounts.window.resets_soon");
  // Hovered, the other form is worked out at that time.
  const shown = row(exact, "five-hour").window;
  assert.equal(V.resetLabel(shown, "countdown", context({ now: NOW + 3600 })), "accounts.window.resets_in_format 2h 20m");
  assert.equal(row(V.cardView({ id: "r", usage: { fiveHour: window(1) } }, context()), "one-week").window.resetAt, null);
});

test("Server UTC corrects either direction of local clock skew in a card's countdown", () => {
  const now = Date.parse("2026-10-30T18:52:00Z") / 1000;
  const reset = Date.parse("2026-10-31T07:40:00+08:00") / 1000;
  for (const offset of [6 * 3600, -6 * 3600, 0]) {
    const account = { id: "s", usage: { fiveHour: window(2, reset, { clockOffsetSeconds: offset }) } };
    const five = row(V.cardView(account, context({ now: now - offset, resetDisplay: "countdown" })), "five-hour");
    assert.equal(five.window.resetAt, reset);
    assert.equal(five.window.clockOffsetSeconds, offset);
    assert.equal(five.reset, "accounts.window.resets_in_format 4h 48m");
    assert.equal(V.resetLabel(five.window, "countdown", context({ now: now - offset + 60 })), "accounts.window.resets_in_format 4h 47m");
  }
});

test("A card's meter tone, as of its context's time, is the same in either mode", () => {
  const live = { usedPercent: 46, windowSeconds: 10000, resetAt: NOW + 5000 };
  for (const mode of ["used", "remaining"])
    assert.equal(row(V.cardView({ id: "t", usage: { fiveHour: live } }, context({ mode })), "five-hour").window.tone, "high", mode);
  // Its tooltip tells when the usage runs out at its pace.
  const pace = P.usagePaceMessage(live, NOW, "en");
  assert.equal(V.paceTitle(live, context()), tr(pace.key, ...pace.args));
  assert.equal(V.paceTitle(undefined, context()), "");
});

test("Claude cards add a Fable row when reported; Codex Free's monthly quota stands alone", () => {
  const account = {
    id: "f",
    email: "alice@example.com",
    usage: {
      fiveHour: { usedPercent: 10, windowSeconds: 18000, resetAt: 1_800_010_000 },
      oneWeek: { usedPercent: 25, windowSeconds: 604800, resetAt: 1_800_500_000 },
      fableOneWeek: { usedPercent: 42, windowSeconds: 604800, resetAt: 1_800_600_000, clockOffsetSeconds: 12 },
    },
  };
  const fable = row(V.cardView(account, context({ pool: "claude" })), "fable");
  assert.equal(fable.title, "accounts.window.fable");
  assert.equal(fable.window.primaryPercent, 42);
  assert.equal(fable.window.reset, P.formatReset(1_800_600_000, "en"));
  assert.equal(fable.window.clockOffsetSeconds, 12);
  assert.equal(row(V.cardView(account, context({ pool: "claude", mode: "remaining" })), "fable").window.primaryPercent, 58);
  const absent = { ...account, usage: { fiveHour: account.usage.fiveHour } };
  assert.equal(row(V.cardView(absent, context({ pool: "claude" })), "fable"), undefined);

  const free = {
    id: "m",
    email: "free@example.com",
    usage: { oneMonth: { usedPercent: 40, windowSeconds: 2592000, resetAt: 1_802_000_000 } },
  };
  const monthly = V.cardView(free, context({ mode: "remaining" }));
  assert.deepEqual(rows(monthly), [["one-month", true]]);
  assert.equal(row(monthly, "one-month").window.primaryPercent, 60);
  assert.equal(row(monthly, "one-month").window.reset, P.formatReset(1_802_000_000, "en"));
  assert.deepEqual(V.cardView(free, context({ mode: "remaining", collapsed: true })).compact, [
    { label: "5h", text: "--" },
    { label: "1m", text: "60%" },
  ]);
});

test("Used percent floors and remaining percent ceils in a card's rows", () => {
  for (const [usedPercent, wantUsed, wantRemaining] of [
    [0, 0, 100], [0.1, 0, 100], [25.5, 25, 75], [27.9, 27, 73], [29, 29, 71], [99.9, 99, 1], [100, 100, 0],
  ]) {
    const account = { id: "p", usage: { fiveHour: { usedPercent }, oneWeek: { usedPercent } } };
    const used = V.cardView(account, context());
    const remaining = V.cardView(account, context({ mode: "remaining" }));
    for (const [view, primary, secondary] of [[used, wantUsed, wantRemaining], [remaining, wantRemaining, wantUsed]]) {
      const five = row(view, "five-hour").window;
      assert.equal(five.primaryPercent, primary);
      assert.equal(five.secondaryPercent, secondary);
      assert.equal(five.progressPercent, primary);
    }
    const compact = (mode) => V.cardView(account, context({ mode, collapsed: true })).compact[0].text;
    assert.equal(compact("used"), `${wantUsed}%`);
    assert.equal(compact("remaining"), `${wantRemaining}%`);
  }
});

test("An unused five-hour window shows its 0% as not started", () => {
  const view = (usage, extra = {}) => V.cardView({ ...a, usage: { ...a.usage, ...usage } }, context(extra));
  const idle = row(view({ fiveHour: { usedPercent: 0, resetAt: null, windowSeconds: 18000 } }), "five-hour");
  assert.equal(idle.window.notStarted, true);
  assert.equal(idle.window.available, true);
  assert.equal(idle.window.primaryPercent, 0);
  assert.equal(idle.reset, "accounts.window.not_started");
  // A started window, any usage, a missing window or a weekly window is not.
  for (const fiveHour of [{ usedPercent: 0, resetAt: 1763216000 }, { usedPercent: 1, resetAt: null }, undefined])
    assert.equal(row(view({ fiveHour }), "five-hour").window.notStarted, false);
  assert.equal(row(view({ oneWeek: { usedPercent: 0, resetAt: null } }), "one-week").window.notStarted, false);
  // The third-party pool's own five-hour window, once shown.
  const third = view({ thirdPartyFiveHour: { usedPercent: 0, resetAt: null } }, {
    pool: "antigravity",
    ui: states({ "account-1": { thirdParty: true } }),
  });
  assert.equal(row(third, "five-hour").window.notStarted, true);
});

test("Cards show their own windows, or their third-party pool's when switched to", () => {
  const w = (usedPercent, windowSeconds) => ({ usedPercent, windowSeconds, resetAt: null });
  const view = (pool, usage, account = {}, shown = []) =>
    V.cardView({ id: "a", usage, ...account }, context({
      pool,
      ui: (id) => ({ thirdParty: shown.includes(id) }),
    }));
  const gemini = { fiveHour: w(20, 18000), oneWeek: w(5, 604800) };
  const own = [["five-hour", true], ["one-week", true]];
  assert.deepEqual(rows(view("codex", gemini)), own);
  assert.deepEqual(view("codex", gemini).rows.map((r) => r.title), ["accounts.window.five_hour", "accounts.window.one_week"]);
  assert.deepEqual(rows(view("antigravity", gemini)), own);
  // Any pool's card reporting a third-party pool switches between its own
  // model and the third party's, its own by default.
  for (const [pool, name] of [["codex", "Codex"], ["claude", "Claude"], ["grok", "Grok"]]) {
    const usage = { ...gemini, thirdPartyFiveHour: w(50, 18000) };
    assert.deepEqual(rows(view(pool, usage)), own);
    assert.deepEqual(view(pool, usage).models, { shown: name, other: "Claude", thirdParty: false });
    assert.equal(view(pool, gemini).models, null);
  }
  // Claude shows a Fable row only when the account reports Fable usage.
  assert.deepEqual(rows(view("claude", { ...gemini, fableOneWeek: w(35, 604800) })), [...own, ["fable", true]]);
  assert.deepEqual(rows(view("claude", { ...gemini, fableOneWeek: null })), own);
  assert.deepEqual(rows(view("codex", { ...gemini, fableOneWeek: w(35, 604800) })), own);
  assert.deepEqual(rows(view("antigravity", { ...gemini, fableOneWeek: w(35, 604800) })), own);
  // Codex Free monthly window: solo Monthly, Session + Monthly, or appended after Weekly.
  assert.deepEqual(rows(view("codex", { oneMonth: w(40, 2592000) })), [["one-month", true]]);
  assert.deepEqual(rows(view("codex", { fiveHour: w(10, 18000), oneMonth: w(40, 2592000) })), [
    ["five-hour", true],
    ["one-month", true],
  ]);
  assert.deepEqual(rows(view("codex", { ...gemini, oneMonth: w(40, 2592000) })), [...own, ["one-month", true]]);
  assert.deepEqual(rows(view("claude", { ...gemini, oneMonth: w(40, 2592000) })), own);
  // A Grok allowance lasting a month is its only window.
  assert.deepEqual(rows(view("grok", { oneMonth: w(40, 2592000) })), [["one-month", true]]);
  assert.deepEqual(rows(view("grok", { oneWeek: w(7, 604800) })), [["five-hour", false], ["one-week", true]]);
  // A card without usage keeps both rows, without data, and no switch.
  assert.deepEqual(rows(view("antigravity", null)), [["five-hour", false], ["one-week", false]]);
  assert.equal(view("antigravity", null).models, null);
  // A Google AI Pro account reporting a third-party pool shows Gemini's by default.
  const pro = { fiveHour: w(2, 18000), thirdPartyFiveHour: w(12, 18000), thirdPartyOneWeek: w(3, 604800) };
  const geminiRows = [["five-hour", true], ["one-week", false]];
  assert.deepEqual(view("antigravity", pro).models, { shown: "Gemini", other: "Claude", thirdParty: false });
  assert.deepEqual(rows(view("antigravity", pro)), geminiRows);
  // Switched to Claude, that card shows its third-party pool alone.
  assert.deepEqual(view("antigravity", pro, {}, ["a"]).models, { shown: "Claude", other: "Gemini", thirdParty: true });
  assert.deepEqual(rows(view("antigravity", pro, {}, ["a"])), [["five-hour", true], ["one-week", true]]);
  assert.equal(row(view("antigravity", pro, {}, ["a"]), "five-hour").window.primaryPercent, 12);
  assert.deepEqual(rows(view("antigravity", pro, {}, ["b"])), geminiRows);
  // A card no longer reporting its third-party pool falls back to its own.
  assert.deepEqual(rows(view("antigravity", { fiveHour: w(2, 18000) }, {}, ["a"])), geminiRows);
  assert.equal(view("antigravity", { fiveHour: w(2, 18000) }, {}, ["a"]).models, null);
});

test("A weekly row of an outdated Sub2api names its version and the pool's upgrade", () => {
  const notices = (pool, account) => V.cardView(account, context({ pool })).rows.map((r) => r.notice);
  const usage = { fiveHour: window(0, null, { windowSeconds: 18000 }), thirdPartyFiveHour: window(0, null, { windowSeconds: 18000 }) };
  const outdated = { id: "o", usage, outdatedSub2apiVersion: " 0.1.177 " };
  const lines = ["accounts.window.sub2api_weekly_unavailable_format 0.1.177", "accounts.window.sub2api_weekly_upgrade"];
  assert.deepEqual(notices("antigravity", outdated), [null, lines]);
  // The third-party pool's weekly row says so too.
  const shown = V.cardView(outdated, context({ pool: "antigravity", ui: () => ({ thirdParty: true }) }));
  assert.deepEqual(shown.rows.map((r) => r.notice), [null, lines]);
  // A current Sub2api's rows carry none.
  assert.deepEqual(notices("antigravity", { id: "o", usage }), [null, null]);
  // Grok's weekly usage came with an earlier Sub2api release.
  assert.deepEqual(notices("grok", { id: "g", usage: {}, outdatedSub2apiVersion: "0.1.150" })[1], [
    "accounts.window.sub2api_weekly_unavailable_format 0.1.150",
    "accounts.window.sub2api_grok_weekly_upgrade",
  ]);
});

test("A card's plan tag is the core's label and color, its workspace tag hidden in a provider's group", () => {
  const tags = (account, grouped = false, extra = {}) =>
    V.cardView({ id: "a", ...account }, context(extra), grouped).tags.map((t) => [t.className, t.text]);
  const pro = V.cardView({ id: "a", planType: "pro", planTag: { label: "PRO 20X", accent: "orange" } }, context());
  assert.equal(pro.plan, "PRO 20X");
  assert.equal(pro.accent, "orange");
  // Whatever the raw plan, its label is the core's.
  assert.deepEqual(tags({ planType: "max 20x", planTag: { label: "ULTRA", accent: "orange" } }, false, { pool: "claude" }), [
    ["tag plan", "ULTRA"],
  ]);
  // The app names no plan of its own, a raw one included, in any pool.
  for (const pool of ["codex", "claude", "antigravity", "grok"])
    for (const account of [{}, { planType: "pro", planTag: null }]) {
      const view = V.cardView({ id: "a", ...account }, context({ pool }));
      assert.equal(view.plan, null, pool);
      assert.equal(view.accent, null, pool);
      assert.deepEqual(view.tags, [], pool);
    }
  const team = { teamName: "workspace-a", planTag: { label: "BUSINESS", accent: "indigo" }, workspaceTag: "workspace-a" };
  assert.deepEqual(tags(team), [["tag plan", "BUSINESS"], ["tag muted", "workspace-a"]]);
  // A team the core leaves unnamed is not shown.
  assert.deepEqual(tags({ ...team, planTag: { label: "PRO", accent: "pink" }, workspaceTag: undefined }), [["tag plan", "PRO"]]);
  // A provider's group carries its SUB2API and CURRENT tags once.
  const member = { ...team, sourceTag: "SUB2API", workspaceTag: "SUB2API", isCurrent: true };
  assert.deepEqual(tags(member), [["tag plan", "BUSINESS"], ["tag muted", "SUB2API"], ["tag good", "accounts.card.current"]]);
  assert.deepEqual(tags(member, true), [["tag plan", "BUSINESS"]]);
  // Awaiting its first request, the card explains why.
  const waiting = V.cardView({ id: "a", awaitingFirstRequest: true }, context()).tags[0];
  assert.deepEqual(waiting, {
    className: "tag muted",
    text: "accounts.card.awaiting_first_request",
    title: "accounts.card.awaiting_first_request.help",
  });
  // A provider's account without tags or switch has no tag row.
  assert.equal(V.cardView({ id: "a", sourceTag: "SUB2API" }, context(), true).headed, false);
  assert.equal(V.cardView({ id: "a", sourceTag: "SUB2API" }, context()).headed, true);
});

test("Sign in again replaces refresh and switch, pinning the workspace only for a workspace tag", () => {
  const expired = {
    id: "a",
    accountId: "workspace-1",
    email: "dev@example.com",
    planType: "team",
    teamName: "Acme",
    usageError: "error.accounts.sign_in_expired",
  };
  const view = V.cardView({ ...expired, workspaceTag: "Acme" }, context());
  assert.deepEqual(buttons(view), ["warning", "reauthenticate"]);
  assert.equal(action(view, "reauthenticate").workspace, "workspace-1");
  assert.deepEqual(action(view, "reauthenticate").button, {
    place: "footer",
    label: "accounts.card.sign_in_again",
    title: "accounts.card.sign_in_again",
    compact: false,
    disabled: false,
  });
  // Without one, whatever the raw plan and team, any workspace may sign in.
  assert.equal(action(V.cardView(expired, context()), "reauthenticate").workspace, null);
  // Collapsed, an icon; it waits while a sign-in runs.
  const compact = action(V.cardView(expired, context({ collapsed: true, adding: true })), "reauthenticate").button;
  assert.deepEqual([compact.label, compact.compact, compact.disabled], ["", true, true]);
  // A Sub2api card's expired key is not a sign-in.
  assert.equal(action(V.cardView({ ...expired, sourceTag: "SUB2API" }, context()), "reauthenticate"), undefined);
});

test("The usage warning shows unless the card refreshes, is collapsed or is a provider's", () => {
  const account = { id: "a", usageError: "error.usage.failed", isCurrent: true };
  const warning = action(V.cardView(account, context()), "warning");
  assert.deepEqual(warning, {
    kind: "warning",
    message: "error.usage.failed",
    button: { place: "footer", title: "error.usage.failed" },
    menu: null,
  });
  assert.equal(action(V.cardView(account, context({ ui: () => ({ refreshing: true }) })), "warning"), undefined);
  assert.equal(action(V.cardView(account, context({ collapsed: true })), "warning"), undefined);
  assert.equal(action(V.cardView(account, context(), true), "warning"), undefined);
});

test("A hi or renewal takes the refresh button's place: under way, then done", () => {
  const card = { id: "a", isCurrent: true };
  const task = (ui, pool = "codex") => action(V.cardView(card, context({ pool, ui: () => ui })), "task")?.button;
  assert.equal(task({}), undefined);
  assert.deepEqual(buttons(V.cardView(card, context())), ["refresh"]);
  assert.deepEqual(task({ hi: "sent" }), {
    place: "footer", glyph: "say-hi-sent", title: "accounts.card.codex_say_hi.sent", done: false,
  });
  assert.equal(task({ hi: "received" }).done, true);
  assert.equal(task({ hi: "sent" }, "antigravity").title, "accounts.card.antigravity_say_hi.sent");
  assert.equal(task({ hi: "sent" }, "claude").title, "accounts.card.say_hi.sent");
  assert.deepEqual(task({ renewing: true }), {
    place: "footer", glyph: "renewal", title: "accounts.renewal.renewing", done: false,
  });
  assert.deepEqual(task({ renewed: true }), {
    place: "footer", glyph: "renewal-done", title: "accounts.notice.token_renewed", done: true,
  });
  assert.deepEqual(buttons(V.cardView(card, context({ ui: () => ({ renewed: true }) }))), ["task"]);
  // A provider's account, refreshed with its provider, shows its task alone.
  assert.deepEqual(buttons(V.cardView(card, context({ ui: () => ({ hi: "sent" }) }), true)), ["task"]);
  assert.deepEqual(buttons(V.cardView(card, context(), true)), []);
});

test("A refresh button spins while its card refreshes and waits for a hi or renewal", () => {
  const card = { id: "a", isCurrent: true, nextRefreshAt: NOW + 90 };
  const refresh = (ui = {}) => action(V.cardView(card, context({ ui: () => ui })), "refresh").button;
  assert.deepEqual(refresh(), {
    place: "footer", label: "common.refresh_usage", nextAt: NOW + 90, spinning: false, disabled: false,
  });
  assert.equal(refresh({ refreshing: true }).spinning, true);
  assert.equal(refresh({ refreshing: true }).disabled, false);
  assert.equal(refresh({ greeting: true }).disabled, true);
  // A renewal under way shows in its place.
  assert.equal(refresh({ renewing: true }), null);
  // Its tooltip tells how long until the next automatic refresh, as the
  // pointer enters.
  assert.equal(V.refreshTitle("Refresh", null, context()), "Refresh");
  assert.equal(V.refreshTitle("Refresh", NOW + 90, context()), "Refresh\naccounts.refresh.next_format 2m");
  assert.equal(V.refreshTitle("Refresh", NOW - 1, context()), "Refresh\naccounts.refresh.next_due");
});

test("A card not in use switches; the one in use does not", () => {
  const card = { id: "a", isCurrent: false, available: false };
  const view = V.cardView(card, context());
  assert.deepEqual(buttons(view), ["refresh", "switch"]);
  assert.deepEqual(action(view, "switch"), {
    kind: "switch",
    id: "a",
    button: { place: "footer", title: "accounts.card.switch_to_this", available: false },
    menu: { label: "accounts.card.switch_to_this", disabled: true },
  });
  assert.equal(action(V.cardView({ ...card, available: true }, context()), "switch").menu.disabled, false);
  assert.equal(action(V.cardView({ ...card, available: true }, context({ switchingId: "b" })), "switch").menu.disabled, true);
  assert.equal(action(V.cardView({ ...card, isCurrent: true }, context()), "switch").button, null);
});

test("A card's menu edits it, then refreshes, greets, renews, switches and removes it", () => {
  const account = { id: "a", email: "a@example.com", planType: "plus", renewal: "always", isCurrent: false, usage: {} };
  const view = (target, extra = {}) =>
    target.group
      ? V.providerView(target.group, "Team", context(extra))
      : V.cardView(target.account, context(extra), !!target.grouped);
  assert.deepEqual(menu(view({ account })), [
    "accounts.card.edit", "—", "common.refresh_usage", "accounts.say_hi.now",
    "accounts.renewal.now", "accounts.card.switch_to_this", "—", "common.remove",
  ]);
  const remove = action(view({ account }), "remove");
  assert.deepEqual(remove, {
    kind: "remove", name: "a@example.com", sub2api: false, button: null,
    menu: { label: "common.remove", destructive: true },
  });
  // The card in use has no switch; collapsed cards no editing.
  assert.deepEqual(menu(view({ account: { ...account, isCurrent: true, renewal: null } }, { collapsed: true })), [
    "common.refresh_usage", "accounts.say_hi.now", "—", "common.remove",
  ]);
  // A provider's group: its provider's refresh, switch and removal.
  const member = { id: "sub2api-team-1", sourceTag: "SUB2API", isCurrent: false };
  const group = { key: "team", accounts: [member] };
  assert.deepEqual(menu(view({ group })), [
    "accounts.card.edit", "—", "common.refresh_usage", "accounts.card.switch_to_this", "—", "common.remove",
  ]);
  assert.deepEqual(action(view({ group }), "remove").name, "Team");
  assert.equal(action(view({ group }), "remove").sub2api, true);
  // A provider's account that cannot greet has no menu of its own.
  assert.deepEqual(menu(view({ account: member, grouped: true })), []);
  // A Claude provider's account greets alone.
  assert.deepEqual(menu(view({ account: member, grouped: true }, { pool: "claude" })), [
    "accounts.card.edit", "accounts.say_hi.now",
  ]);
  const currentGroup = { group: { key: "team", accounts: [{ ...member, isCurrent: true }] } };
  assert(!menu(view(currentGroup)).includes("accounts.card.switch_to_this"));
  // A Grok provider can reapply its config after an app or gateway update.
  assert(menu(view(currentGroup, { pool: "grok" })).includes("accounts.card.switch_to_this"));
  // Busy, its refresh, greeting and renewal wait.
  for (const ui of [{ refreshing: true }, { greeting: true }, { renewing: true }]) {
    const busy = view({ account }, { ui: () => ui });
    for (const kind of ["refresh", "say-hi", "renew"]) assert.equal(action(busy, kind).menu.disabled, true, kind);
    assert.equal(action(busy, "switch").menu.disabled, false);
  }
  // A hi under way waits for its own end.
  assert.equal(action(view({ account }, { ui: () => ({ hi: "sent" }) }), "say-hi").menu.disabled, true);
});

test("A provider's group shares its cards' warnings, refresh and switch", () => {
  const card = (id, extra = {}) => ({ id, sourceTag: "SUB2API", ...extra });
  const group = {
    key: "team",
    accounts: [
      card("sub2api-team-1", { available: false, usageError: "error.a", nextRefreshAt: NOW + 300 }),
      card("sub2api-team-2", { usageError: "error.a", nextRefreshAt: NOW + 120 }),
      card("sub2api-team-3", { usageError: "error.b" }),
    ],
  };
  const view = V.providerView(group, "Team", context());
  assert.equal(view.title, "Team");
  assert.equal(view.current, false);
  assert.deepEqual(view.tags, [{ className: "tag muted", text: "SUB2API" }]);
  assert.deepEqual(buttons(view), ["warning", "refresh", "switch"]);
  assert.deepEqual(buttons(view, "head"), ["edit"]);
  assert.equal(action(view, "warning").message, "error.a\nerror.b");
  // Its refresh is due with its first card; its switch goes through an
  // active card.
  assert.equal(action(view, "refresh").button.nextAt, NOW + 120);
  assert.equal(action(view, "switch").id, "sub2api-team-2");
  assert.equal(action(view, "switch").button.available, true);
  assert.equal(view.members.length, 3);
  assert(view.members.every((member) => member.grouped && member.controls === "account"));
  // Any card refreshing spins it and hides the warnings; a hi waits it.
  const busy = V.providerView(group, "Team", context({ ui: (id) => ({ refreshing: id === "sub2api-team-3" }) }));
  assert.deepEqual(buttons(busy), ["refresh", "switch"]);
  assert.equal(action(busy, "refresh").button.spinning, true);
  const greeting = V.providerView(group, "Team", context({ ui: (id) => ({ greeting: id === "sub2api-team-1" }) }));
  assert.equal(action(greeting, "refresh").button.disabled, true);
  // In use, it says so once and has no switch button.
  const current = { key: "team", accounts: [card("sub2api-team-1", { isCurrent: true })] };
  const inUse = V.providerView(current, "Team", context());
  assert.deepEqual(inUse.tags.map((t) => t.text), ["SUB2API", "accounts.card.current"]);
  assert.deepEqual(buttons(inUse), ["refresh"]);
});

test("A collapsed provider's first row carries the provider's dot and controls", () => {
  const card = (id, extra = {}) => ({ id, sourceTag: "SUB2API", ...extra });
  const group = { key: "team", accounts: [card("sub2api-team-1"), card("sub2api-team-2", { isCurrent: true })] };
  const view = V.providerView(group, "Team", context({ collapsed: true }));
  const [first, second] = view.members;
  assert.deepEqual(first.row, { dot: true, current: true, canReveal: false, revealed: false });
  assert.equal(first.controls, "provider");
  assert.deepEqual(buttons(first), ["refresh"]);
  assert.deepEqual(second.row, { dot: false, current: false, canReveal: false, revealed: false });
  assert.equal(second.controls, "account");
  // A provider not in use reveals its switch, kept shown while switching.
  const idle = { key: "team", accounts: [card("sub2api-team-1"), card("sub2api-team-2")] };
  const switching = V.providerView(idle, "Team", context({ collapsed: true, switchingId: "sub2api-team-1" }));
  assert.deepEqual(switching.members[0].row, { dot: true, current: false, canReveal: false, revealed: true });
  assert.deepEqual(buttons(switching.members[0]), ["refresh", "switch"]);
  // A single account's row is its own.
  assert.deepEqual(V.cardView(card("x"), context({ collapsed: true })).row, {
    dot: true, current: false, canReveal: true, revealed: false,
  });
});

test("Details contain the proxy, relative token expiry and this card's refresh interval", () => {
  const account = { id: "a", isCurrent: true, renewal: "always", tokenExpiresAt: NOW + 3600 };
  const c = context({ pool: "claude", accountProxy: "socks5://127.0.0.1:7890" });
  const details = (extra = {}) => V.details({ account }, { ...c, ...extra });
  assert.equal(details(), "accounts.proxy.title: socks5://127.0.0.1:7890\naccounts.renewal.expires_format 1h\naccounts.refresh.interval_format 30 seconds");
  account.proxyUrl = "direct://";
  account.tokenExpiresAt = 1;
  account.renewal = "never";
  assert.equal(details(), "direct\naccounts.renewal.expired\naccounts.refresh.interval_format 30 seconds");
  // A card without renewal has no token to tell.
  account.renewal = null;
  assert.equal(details(), "direct\naccounts.refresh.interval_format 30 seconds");
  account.renewal = "always";
  account.tokenExpiresAt = null;
  assert.equal(details(), "direct\naccounts.refresh.interval_format 30 seconds");
  account.proxyUrl = "https://proxy.example:443";
  assert.equal(details(), "accounts.proxy.title: https://proxy.example:443\naccounts.refresh.interval_format 30 seconds");
  account.proxyUrl = "";
  assert.equal(details({ accountProxy: "direct://" }), "direct\naccounts.refresh.interval_format 30 seconds");
  assert.equal(details({ accountProxy: "" }), "direct\naccounts.refresh.interval_format 30 seconds");
  account.isCurrent = false;
  assert.equal(details({ accountProxy: "" }), "direct\naccounts.refresh.interval_format 3 minutes");
  assert.equal(V.details(null, c), "accounts.card.edit");
});

test("Providers and their members show the interval for whether the provider is in use", () => {
  const c = context({ pool: "claude", accountProxy: "socks5://127.0.0.1:7890" });
  const account = { id: "member", sourceTag: "SUB2API" };
  const group = { key: "provider", accounts: [account] };
  // A provider's proxy never falls back to the shared account proxy.
  assert.equal(V.details({ group }, c), "direct\naccounts.refresh.interval_format 3 minutes");
  assert.equal(V.details({ account, grouped: true }, c), "accounts.refresh.interval_format 3 minutes");
  assert.equal(V.details({ account }, c), "direct\naccounts.refresh.interval_format 3 minutes");
  group.accounts.push({ id: "current", isCurrent: true });
  assert.equal(V.details({ group }, c), "direct\naccounts.refresh.interval_format 30 seconds");
});

test("The card's front shows no token expiry; its details and back do", () => {
  const account = { id: "a", renewal: "always", tokenExpiresAt: NOW + 3600, usage: {} };
  const view = V.cardView(account, context());
  assert(!JSON.stringify(view).includes("accounts.renewal.expires_format"));
  assert.equal(V.tokenLabel(account, context()), "accounts.renewal.expires_format 1h");
  assert.equal(V.tokenLabel({ tokenExpiresAt: 1 }, context()), "accounts.renewal.expired");
  assert.equal(V.tokenLabel({}, context()), null);
});

test("Credits and resets carry their expiry dates; resets other than full ones a notice", () => {
  const future = NOW + 86400;
  const later = future + 86400;
  const grant = (kind, count, expiresAt) => ({ kind, count, expiresAt });
  const footer = (usage, pool = "codex") => V.cardView({ id: "a", usage }, context({ pool })).footer;
  const expires = (at) => `accounts.card.reset_expires_format ${P.formatReset(at, "en")}`;
  const banked = footer({
    credits: { balance: "5", expiries: [future] },
    resetGrants: [grant("full", 1, future), grant("fable", 1, future), grant("fable", 1, later), grant("fable", 1, later), grant("partial", 1, null)],
  });
  assert.deepEqual(banked.credits, { text: "accounts.card.credits_format 5", title: expires(future) });
  assert.deepEqual(banked.resets, {
    text: "accounts.card.remaining_resets_format 5",
    title: [`${expires(future)} × 2`, `${expires(later)} × 2`, "accounts.card.expiry_unknown"].join("\n"),
  });
  assert.deepEqual(banked.warning, [
    `accounts.card.reset_full × 1 · ${expires(future)}`,
    `accounts.card.reset_fable × 1 · ${expires(future)}`,
    `accounts.card.reset_fable × 2 · ${expires(later)}`,
    "accounts.card.reset_partial × 1 · accounts.card.expiry_unknown",
  ]);
  // A cached count drops resets since expired and tells when it was confirmed.
  const cached = footer({ remainingResetCount: 2, resetExpiries: [1, future], resetsLastConfirmedAt: 100 }).resets;
  assert.equal(cached.text, "accounts.card.remaining_resets_format 1");
  assert.equal(cached.title, `${expires(future)}\naccounts.card.resets_last_confirmed_format ${P.formatReset(100, "en")}`);
  // Nothing known: no tooltips, nor for a balance and resets of zero.
  const unknown = footer({});
  assert.deepEqual(unknown.credits, { text: "accounts.card.credits_format --", title: null });
  assert.deepEqual(unknown.resets, { text: "accounts.card.resets_unavailable_format --", title: null });
  assert.equal(unknown.warning, null);
  const undated = footer({ credits: { expiries: [future] }, resetsLastConfirmedAt: 100 });
  assert.equal(undated.credits.title, null);
  assert.equal(undated.resets.title, null);
  const zero = footer({ credits: { balance: "$0.00" }, remainingResetCount: 0 });
  assert.equal(zero.credits.title, null);
  assert.equal(zero.resets.title, null);
  const confirmed = footer({ credits: { balance: "0" }, remainingResetCount: 0, resetsLastConfirmedAt: 100 });
  assert.equal(confirmed.credits.title, null);
  assert.match(confirmed.resets.title, /^accounts\.card\.resets_last_confirmed_format /);
  const dateless = footer({ credits: { balance: "5" }, remainingResetCount: 2 });
  assert.equal(dateless.credits.title, "accounts.card.expiry_unknown");
  assert.equal(dateless.resets.title, "accounts.card.expiry_unknown × 2");
  assert.equal(footer({ credits: { unlimited: true } }).credits.title, "accounts.card.expiry_unknown");
  // Antigravity and Grok: their credits alone, even with resets in the usage.
  for (const pool of ["antigravity", "grok"]) {
    const credits = footer({ credits: { balance: "1000" }, resetGrants: [grant("partial", 1, future)] }, pool);
    assert.deepEqual(credits, {
      credits: { text: "accounts.card.credits_format 1000", title: "accounts.card.expiry_unknown" },
      resets: null,
      warning: null,
    });
    assert.deepEqual(footer({ credits: { balance: "$0.00" } }, pool).credits, { text: "accounts.card.credits_format $0.00", title: null });
    assert.equal(footer({}, pool).credits.title, null);
  }
  assert.notEqual(footer({}, "claude").resets, null);
});

test("Antigravity accounts say hi, its Sub2api cards do not; Grok never does", () => {
  assert.equal(V.supportsSayHi({ id: "a" }, "antigravity"), true);
  assert.equal(V.supportsSayHi({ id: "antigravity-sub2api-p-1", sourceTag: "SUB2API" }, "antigravity"), false);
  assert.equal(V.supportsSayHi({ id: "claude-sub2api-p-1", sourceTag: "SUB2API" }, "claude"), true);
  assert.equal(V.supportsSayHi({ id: "g" }, "grok"), false);
});

test("Say hi now requires an idle window", () => {
  const now = NOW;
  const idle = (account, pool = "claude", target = null) => V.fiveHourIdle(account, pool, target, now);
  assert.equal(idle({ usage: null }), false, "usage not yet known");
  assert.equal(idle({ usage: {} }), true, "no window");
  assert.equal(idle({ usage: { fiveHour: { usedPercent: 0, resetAt: null } } }), true);
  assert.equal(idle({ usage: { fiveHour: { usedPercent: 3, resetAt: now - 60 } } }), true, "ended");
  assert.equal(idle({ usage: { fiveHour: { usedPercent: 3, resetAt: now + 60 } } }), false);
  // Antigravity: either pool's idle window will do; only a reported one counts.
  const started = { usedPercent: 3, resetAt: now + 60 };
  const quiet = { usedPercent: 0, resetAt: null };
  const both = (fiveHour, thirdPartyFiveHour) => ({ usage: { fiveHour, thirdPartyFiveHour } });
  assert.equal(idle(both(started, quiet)), false, "other pools ignore the third-party window");
  assert.equal(idle(both(started, quiet), "antigravity"), true, "Claude's window idle");
  assert.equal(idle(both(quiet, started), "antigravity"), true, "Gemini's window idle");
  assert.equal(idle(both(started, started), "antigravity"), false);
  assert.equal(idle(both(started, undefined), "antigravity"), false, "no Claude pool reported");
  // A hi to the chosen pool alone looks at its window alone.
  assert.equal(idle(both(started, quiet), "antigravity", true), true);
  assert.equal(idle(both(started, quiet), "antigravity", false), false);
  assert.equal(idle(both(quiet, started), "antigravity", true), false);
  assert.equal(idle(both(quiet, started), "antigravity", false), true);
  assert.equal(idle(both(quiet, undefined), "antigravity", true), false, "no third-party pool");
});

test("A card's model switch chooses the pool its hi greets", () => {
  const card = { id: "a", usage: { fiveHour: { usedPercent: 3 }, thirdPartyFiveHour: { usedPercent: 1 } } };
  const target = (thirdParty) => V.helloTarget(card, context({ pool: "antigravity", ui: () => ({ thirdParty }) }));
  assert.equal(target(false), false);
  assert.equal(target(true), true);
  assert.equal(V.helloTarget({ id: "b", usage: { fiveHour: { usedPercent: 3 } } }, context()), null);
});

test("Codex greetings are limited to idle official Plus cards", () => {
  const can = (account, pool = "codex") => V.canSayHi(account, context({ pool }));
  const card = { id: "a", planType: "plus", usage: {} };
  assert.equal(can(card), true);
  assert.equal(can({ ...card, usage: null }), false);
  assert.equal(can({ ...card, sourceTag: "SUB2API" }), false);
  for (const planType of ["pro", "free", "team", ""]) assert.equal(can({ ...card, planType }), false);
  assert.equal(can({ ...card, usage: { planType: "pro" } }), false);
  assert.equal(can({ ...card, usage: { fiveHour: { resetAt: NOW + 100 } } }), false);
  assert.equal(can({ ...card, planType: "max" }, "claude"), true);
});

test("A card turns over only to what it can edit", () => {
  const member = { id: "m", sourceTag: "SUB2API" };
  assert.equal(V.hasBack(null, "codex"), false);
  assert.equal(V.hasBack({ group: { key: "p", accounts: [member] } }, "codex"), true);
  assert.equal(V.hasBack({ account: member }, "codex"), true);
  // A provider's account has only its greetings to edit.
  assert.equal(V.hasBack({ account: member, grouped: true }, "codex"), false);
  assert.equal(V.hasBack({ account: member, grouped: true }, "claude"), true);
});

test("A card's view is plain data, with no functions to call", () => {
  // structuredClone refuses functions.
  const view = V.cardView(a, context({ ui: () => ({ hi: "sent" }) }));
  assert.deepEqual(structuredClone(view), view);
  const group = V.providerView({ key: "p", accounts: [{ id: "sub2api-p-1", sourceTag: "SUB2API" }] }, "P", context());
  assert.deepEqual(structuredClone(group), group);
});
