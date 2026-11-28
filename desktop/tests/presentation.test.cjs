const { test } = require("node:test");
const assert = require("node:assert/strict");
const P = require("../ui/presentation.js");
const legacy = (suite, name, fn) => test(`${suite}.${name}`, fn);
legacy(
  "AccountSummaryPresentationTests",
  "testWorkspaceDisplayNameNormalizationTrimsWhitespaceAndDropsEmptyValues",
  () => {
    assert.equal(P.clean(" workspace-a "), "workspace-a");
    assert.equal(P.clean(" \n "), null);
  },
);
// A card's own presentation is card-view.js's (card-view.test.cjs).
test("A reset date shows in the given locale and time zone", () => {
  assert.equal(
    P.formatReset(1763216000, "en_US", "Asia/Shanghai"),
    "11/15/25, 22:13:20",
  );
});
legacy(
  "AccountCardInteractionPresentationTests",
  "testMacOSHoverControlsCollapsedOverlayVisibility",
  () =>
    assert.deepEqual(
      P.collapsedInteraction({
        collapsed: true,
        current: false,
        switching: false,
        hovered: true,
      }),
      { canHover: true, canReveal: true, visible: true },
    ),
);
legacy(
  "AccountCardInteractionPresentationTests",
  "testSwitchingKeepsOverlayVisibleButPreventsReveal",
  () => {
    const p = P.collapsedInteraction({
      collapsed: true,
      current: false,
      switching: true,
      hovered: false,
    });
    assert.equal(p.canReveal, false);
    assert.equal(p.visible, true);
  },
);
legacy("AppLocaleTests", "testResolveNormalizesLegacyIdentifiers", () => {
  for (const [raw, want] of [
    ["en-US", "en"],
    ["zh-CN", "zh-Hans"],
    ["ja-JP", "ja"],
    ["ko-KR", "ko"],
  ])
    assert.equal(P.resolveLocale(raw), want);
});
legacy("AppLocaleTests", "testResolveFallsBackToEnglish", () => {
  assert.equal(P.resolveLocale("pt-BR"), "en");
  assert.equal(P.resolveLocale(""), "en");
});
legacy(
  "AppLocaleTests",
  "testPreferredUsesFirstSupportedSystemLanguage",
  () => {
    assert.equal(P.preferredLocale(["fr-FR", "ja-JP", "en-US"]), "fr");
    assert.equal(P.preferredLocale(["de-DE", "en-GB"]), "de");
    assert.equal(P.preferredLocale(["zh-CN", "en-US"]), "zh-Hans");
  },
);
legacy(
  "AccountsActionPresentationTests",
  "testImportMenuIncludesSub2APIWhenServiceIsAvailable",
  () =>
    assert.deepEqual(P.actions({ sub2api: true })[0].menu, [
      "importCurrentAuth",
      "importAuthFile",
      "importSub2APIAccounts",
    ]),
);
legacy(
  "AccountsActionPresentationTests",
  "testDesktopButtonsReflectBusyState",
  () => {
    const p = P.actions({ importing: true });
    assert.deepEqual(
      p.map((a) => a.intent),
      ["importCurrentAuth", "addAccount", "smartSwitch", "refreshUsage"],
    );
    assert.equal(p[0].title, "accounts.action.importing");
    assert.equal(p[0].label, "accounts.action.importing");
    assert.deepEqual(
      p.slice(0, 3).map((a) => a.enabled),
      [false, false, false],
    );
    assert.deepEqual(p[0].menu, ["importCurrentAuth", "importAuthFile"]);
  },
);
test(
  "desktop import and add buttons use the Swift symbols with short titles",
  () => {
    const [importAccount, addAccount] = P.actions();
    assert.equal(importAccount.symbol, "square.and.arrow.down");
    assert.equal(importAccount.label, "accounts.action.import");
    assert.equal(importAccount.title, "accounts.action.import_account");
    assert.equal(addAccount.symbol, "plus");
    assert.equal(addAccount.label, "accounts.action.add");
    assert.equal(addAccount.title, "accounts.action.add_account");
  },
);
legacy(
  "AccountsActionPresentationTests",
  "testDesktopButtonsSwapAddActionForCancelWhileWaitingForLogin",
  () => {
    const p = P.actions({ adding: true });
    assert.equal(p[1].intent, "cancelAddAccount");
    assert.equal(p[1].title, "common.cancel");
    assert.equal(p[1].symbol, "xmark");
    assert.equal(p[1].label, "common.cancel");
    assert.equal(p[1].enabled, true);
  },
);
legacy(
  "LiquidProgressMetricsTests",
  "testRenderModelClampsProgressIntoUnitRange",
  () => {
    assert.equal(P.progressMetrics(-0.4, 250).fillScale, 0);
    assert.equal(P.progressMetrics(0.45, 250).fillScale, 0.45);
    assert.equal(P.progressMetrics(1.8, 250).fillScale, 1);
  },
);
legacy(
  "LiquidProgressMetricsTests",
  "testRenderModelOnlyShowsFillForPositiveProgress",
  () => {
    assert.equal(P.progressMetrics(0, 250).showsFill, false);
    assert.equal(P.progressMetrics(0.01, 250).showsFill, true);
  },
);
legacy(
  "LiquidProgressMetricsTests",
  "testLowProgressUsesFullLeadingCapWidth",
  () => {
    const m = P.progressMetrics(0.01, 250);
    assert(m.rawFillWidth > 0 && m.rawFillWidth < m.grooveHeight);
    assert.equal(m.visibleFillWidth, m.grooveHeight);
  },
);
legacy(
  "LiquidProgressMetricsTests",
  "testHigherProgressKeepsMeasuredFillWidth",
  () => {
    const m = P.progressMetrics(0.3, 250);
    assert.equal(m.visibleFillWidth, m.rawFillWidth);
  },
);
test("token expiry shows the largest whole unit left, or expired", () => {
  const now = 1_800_000_000;
  assert.equal(P.tokenExpiry(null, now), null);
  assert.equal(P.tokenExpiry(undefined, now), null);
  assert.deepEqual(P.tokenExpiry(now, now), { expired: true, remaining: null });
  // Values the core never sends still neither render nor throw.
  for (const epoch of [0, -5, NaN, Infinity, "1900000000", 9e15, 253402300800])
    assert.equal(P.tokenExpiry(epoch, now), null, String(epoch));
  assert.equal(P.tokenExpiry(253402300799, now).expired, false);
  assert.doesNotThrow(() => P.formatReset(253402300799));
  const left = (seconds, locale) =>
    P.tokenExpiry(now + seconds, now, locale).remaining;
  assert.equal(left(6 * 86400 + 80000), "6d");
  assert.equal(left(86400), "1d");
  assert.equal(left(5 * 3600 + 3599), "5h");
  assert.equal(left(61), "2m");
  assert.equal(left(1), "1m");
  assert.equal(left(6 * 86400, "zh-Hans"), "6天");
  assert.equal(left(5 * 3600, "zh_CN"), "5小时");
});
test("The next automatic refresh counts down to the second, due once reached", () => {
  const now = 1_800_000_000;
  assert.equal(P.refreshCountdown(null, now), null);
  assert.equal(P.refreshCountdown(undefined, now), null);
  const left = (seconds, locale) => P.refreshCountdown(now + seconds, now, locale);
  for (const seconds of [-60, 0, -0.5])
    assert.deepEqual(left(seconds), { due: true, remaining: null });
  assert.equal(left(0.2).remaining, "1s");
  assert.equal(left(45).remaining, "45s");
  assert.equal(left(60).remaining, "1m");
  assert.equal(left(61).remaining, "2m");
  assert.equal(left(30 * 60).remaining, "30m");
  assert.equal(left(3600).remaining, "1h");
  assert.equal(left(3600 + 19 * 60 + 1).remaining, "1h 20m");
  assert.equal(left(4 * 60 + 30, "zh-Hans").remaining, "5分钟");
});
test("A reset counts down as OpenQuota shows it, soon within five minutes", () => {
  const now = 1_800_000_000;
  assert.equal(P.resetCountdown(null, now), null);
  assert.equal(P.resetCountdown(undefined, now), null);
  const left = (seconds, locale) => P.resetCountdown(now + seconds, now, locale);
  for (const seconds of [-60, 0, 300])
    assert.deepEqual(left(seconds), { soon: true, remaining: null });
  assert.equal(left(301).remaining, "6m");
  assert.equal(left(3600).remaining, "1h");
  assert.equal(left(3 * 3600 + 19 * 60 + 1).remaining, "3h 20m");
  assert.equal(left(2 * 86400 + 3 * 3600).remaining, "2d 3h");
  assert.equal(left(86400).remaining, "1d 0h");
  assert.equal(left(3 * 3600 + 20 * 60, "zh-Hans").remaining, "3小时20分钟");
});
test("A provider's cards gather where its first card stands", () => {
  // A provider with a single card (q) keeps an ordinary card.
  const cards = ["a", "p1", "b", "q1", "p2"].map((id) => ({ id }));
  const groups = P.providerGroups(cards, (c) =>
    c.id.length === 2 ? c.id[0] : null,
  );
  assert.deepEqual(
    groups.map((g) => (g.account ? g.account.id : `${g.key}:${g.accounts.map((c) => c.id)}`)),
    ["a", "p:p1,p2", "b", "q1"],
  );
  assert.deepEqual(P.providerGroups([], () => null), []);
  // Inside a group, by Sub2api account id rather than the usage ranking.
  const ranked = ["sub2api-x-12", "sub2api-x-3", "sub2api-x-7"].map((id) => ({ id }));
  assert.deepEqual(
    P.providerGroups(ranked, () => "x")[0].accounts.map((c) => c.id),
    ["sub2api-x-3", "sub2api-x-7", "sub2api-x-12"],
  );
});
test("Dragged order first, then cards not yet ordered as ranked", () => {
  const entries = ["a", "b", "c", "d"].map((id) => ({ id }));
  const order = (saved) =>
    P.orderEntries(entries, saved, (e) => e.id).map((e) => e.id);
  assert.deepEqual(order([]), ["a", "b", "c", "d"]);
  assert.deepEqual(order(undefined), ["a", "b", "c", "d"]);
  // A removed card's key ("x") is ignored; new cards follow.
  assert.deepEqual(order(["c", "x", "a"]), ["c", "a", "b", "d"]);
  // The current card leads whatever the saved order.
  assert.deepEqual(
    P.orderEntries(entries, ["c", "a", "b", "d"], (e) => e.id, (e) => e.id === "b")
      .map((e) => e.id),
    ["b", "c", "a", "d"],
  );
});

test("Server UTC corrects either direction of local clock skew", () => {
  const now = Date.parse("2026-10-30T18:52:00Z") / 1000;
  const reset = Date.parse("2026-10-31T07:40:00+08:00") / 1000;
  for (const offset of [6 * 3600, -6 * 3600, 0]) {
    assert.equal(P.resetCountdown(reset, now - offset, "en", offset).remaining, "4h 48m");
    assert.equal(P.resetCountdown(reset, now - offset + 60, "en", offset).remaining, "4h 47m");
  }
  assert.equal(P.resetCountdown(reset, now, "en", null).remaining, "4h 48m");
});


test("Usage colors follow OpenQuota pacing and fallback boundaries", () => {
  const now = 1_800_000_000;
  const quota = (usedPercent, elapsed, extra = {}) => ({ usedPercent,
    windowSeconds: 10000, resetAt: now + (1 - elapsed) * 10000, ...extra });
  for (const [used, elapsed, tone] of [
    [30, .5, ""], [46, .5, "high"], [60, .5, "full"],
    [45, .5, ""], [49.5, .5, "high"], [49.8, .5, "full"], [50, .5, "full"],
    [1, .009, ""], [4, .02, ""], [5, .02, "full"], [85, .95, ""],
  ]) assert.equal(P.usageTone(quota(used, elapsed), now), tone, `${used}/${elapsed}`);
  for (const [used, tone] of [[79.99, ""], [80, "high"], [89.99, "high"], [90, "full"], [100, "full"]]) {
    for (const extra of [{resetAt: null}, {resetAt: now}, {windowSeconds: 0}])
      assert.equal(P.usageTone(quota(used, .5, extra), now), tone);
  }
  assert.equal(P.usageTone(null, now), "");
  assert.equal(P.usageTone(quota(20, .01, {windowSeconds: 3600, resetAt: now+3564}), now), "");
  assert.equal(P.usageTone(quota(46, .5, {clockOffsetSeconds: 3600}), now-3600), "high");
  const window = quota(46, .5);
  assert.equal(P.usageTone(window, now), "high");
  assert.equal(P.usageTone(window, now+1000), "");
});

test("Usage bar tooltips report estimated time to exhaustion, not excess percent", () => {
  const now = 1_800_000_000;
  const quota = (usedPercent, extra = {}) => ({usedPercent, windowSeconds: 18000, resetAt: now + 9000, ...extra});
  const message = (used, extra = {}, at = now) => P.usagePaceMessage(quota(used, extra), at);
  assert.deepEqual(message(75), {key: "accounts.window.pace_in", args: ["50m"]});
  assert.deepEqual(message(75, {clockOffsetSeconds: 3600}, now-3600), message(75));
  assert.deepEqual(message(98), {key: "accounts.window.pace_soon", args: []});
  assert.deepEqual(message(46), {key: "accounts.window.pace_spare", args: [8]});
  assert.deepEqual(message(49.5), {key: "accounts.window.pace_spare", args: [1]});
  for (const used of [30, 49.8, 50]) assert.equal(message(used), null);
  assert.deepEqual(message(100), {key: "accounts.window.pace_spent", args: []});
  assert.equal(message(90, {resetAt: null}), null);
  assert.equal(message(4, {resetAt: now+17640}), null);
  assert.equal(P.usagePaceMessage(null, now), null);
  assert.equal(message(0), null);
});
test("menu bar keyboard navigation moves on with plain left and right", () => {
  const navigating = { navigating: true, modified: false, inText: false, handled: false };
  assert.equal(P.menuBarStep("ArrowLeft", navigating), "left");
  assert.equal(P.menuBarStep("ArrowRight", navigating), "right");
  // Up and down keep their meaning in the panel, as after a click.
  assert.equal(P.menuBarStep("ArrowUp", navigating), null);
  assert.equal(P.menuBarStep("ArrowDown", navigating), null);
  // Clicked open, modified, in text or used already, arrows stay the panel's.
  for (const change of [
    { navigating: false },
    { modified: true },
    { inText: true },
    { handled: true },
  ]) {
    assert.equal(P.menuBarStep("ArrowRight", { ...navigating, ...change }), null);
  }
});

test("Used percent floors and remaining percent ceils", () => {
  for (const [usedPercent, wantUsed, wantRemaining] of [
    [0, 0, 100],
    [0.1, 0, 100],
    [25.5, 25, 75],
    [27.9, 27, 73],
    [29, 29, 71],
    [99.9, 99, 1],
    [100, 100, 0],
  ]) {
    const window = { usedPercent };
    assert.equal(P.roundDisplayPercent(P.displayPercent(window, "used"), "used"), wantUsed);
    assert.equal(
      P.roundDisplayPercent(P.displayPercent(window, "remaining"), "remaining"),
      wantRemaining,
    );
  }
  assert.equal(P.roundDisplayPercent(null, "used"), null);
  assert.equal(P.usagePaceMessage({ usedPercent: 99.9, resetAt: null }, 1_800_000_000), null);
  assert.deepEqual(P.usagePaceMessage({ usedPercent: 100, resetAt: null }, 1_800_000_000), {
    key: "accounts.window.pace_spent",
    args: [],
  });
});

test("a turned card stays where it stood, the list keeping space at its end while it is edited", () => {
  // A back 66px shorter, the list scrolled to its end: scrolling back needs
  // 66px more, which an edited card keeps.
  assert.deepEqual(P.heldScroll(66, 376, 376, true), { scrollTop: 442, space: 66 });
  // Turned back, the front is taller again: no space is needed.
  assert.deepEqual(P.heldScroll(-10, 100, 500, false), { scrollTop: 90, space: 0 });
  assert.deepEqual(P.heldScroll(30, 100, 500, false), { scrollTop: 130, space: 0 });
  // Without an edited card nothing is added, and the list stops at its ends.
  assert.deepEqual(P.heldScroll(66, 376, 376, false), { scrollTop: 376, space: 0 });
  assert.deepEqual(P.heldScroll(-50, 20, 500, true), { scrollTop: 0, space: 0 });
});
