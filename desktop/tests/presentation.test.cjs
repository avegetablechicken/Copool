const { test } = require("node:test");
const assert = require("node:assert/strict");
const P = require("../ui/presentation.js");
const legacy = (suite, name, fn) => test(`${suite}.${name}`, fn);
const a = {
  accountId: "account-1",
  email: "dev@example.com",
  planType: "business",
  teamName: "workspace-a",
  teamAlias: "Alias A",
  isCurrent: true,
  usage: {
    planType: "business",
    fiveHour: { usedPercent: 27.2, resetAt: 1763216000 },
    oneWeek: { usedPercent: 52.6 },
    credits: { balance: "128", unlimited: false },
  },
};
legacy(
  "AccountSummaryPresentationTests",
  "testNormalizedPlanLabelFromRawPlanTypeFallsBackToTeam",
  () => {
    assert.equal(P.normalizedPlan(" business "), "BUSINESS");
    assert.equal(P.normalizedPlan(null), "TEAM");
  },
);
legacy(
  "AccountSummaryPresentationTests",
  "testWorkspaceDisplayNameNormalizationTrimsWhitespaceAndDropsEmptyValues",
  () => {
    assert.equal(P.clean(" workspace-a "), "workspace-a");
    assert.equal(P.clean(" \n "), null);
  },
);
legacy(
  "AccountSummaryPresentationTests",
  "testNormalizedPlanLabelPrefersLatestUsagePlanType",
  () =>
    assert.equal(
      P.normalizedPlan({ planType: "free", usage: { planType: "plus" } }),
      "PLUS",
    ),
);
legacy(
  "AccountSummaryPresentationTests",
  "testWorkspaceTagIsShownForBusinessPlan",
  () =>
    assert.equal(
      P.workspaceTag({ planType: "business", teamName: "workspace-a" }),
      "workspace-a",
    ),
);
legacy(
  "AccountSummaryPresentationTests",
  "testWorkspaceTagStaysHiddenForProPlan",
  () =>
    assert.equal(
      P.workspaceTag({ planType: "pro", teamName: "workspace-a" }),
      null,
    ),
);
legacy(
  "AccountCardPresentationTests",
  "testCurrentAccountPaletteUsesOwnAccentForSelectionHighlight",
  () => {
    const c = P.card({ ...a, planType: "pro", usage: null });
    assert.equal(c.accent, "orange");
    assert.equal(c.selectionAccent, "PRO");
  },
);
legacy(
  "AccountCardPresentationTests",
  "testCollapsedPresentationUsesAliasAccentAndShortEmail",
  () => {
    const c = P.card(a, true);
    assert.equal(c.plan, "BUSINESS");
    assert.equal(c.accent, "indigo");
    assert.equal(c.workspaceTag, "Alias A");
    assert.equal(c.name, "dev");
    assert.equal(c.credits, "128");
    assert.deepEqual(c.compact, [27.2, 52.6]);
  },
);
legacy(
  "AccountCardPresentationTests",
  "testExpandedPresentationFallsBackToTeamAccentAndMissingWindowDefaults",
  () => {
    const c = P.card({
      accountId: "account-2",
      usage: { credits: { unlimited: true } },
    });
    assert.equal(c.plan, "TEAM");
    assert.equal(c.accent, "teal");
    assert.equal(c.name, "account-2");
    assert.equal(c.workspaceTag, null);
    assert.equal(c.credits, "unlimited");
    assert.equal(c.fiveHour.progressPercent, 100);
    assert.equal(c.fiveHour.reset, "--");
  },
);
legacy(
  "AccountCardPresentationTests",
  "testPresentationShowsDeactivatedStatusTag",
  () =>
    assert.equal(
      P.card({ ...a, workspaceStatus: "deactivated" }).status,
      "deactivated",
    ),
);
legacy(
  "AccountCardPresentationTests",
  "testRemainingModeUsesRemainingForProgressAndCompactPercent",
  () => {
    const c = P.card(
      {
        ...a,
        usage: { fiveHour: { usedPercent: 72 }, oneWeek: { usedPercent: 30 } },
      },
      false,
      "remaining",
    );
    assert.equal(c.fiveHour.progressPercent, 28);
    assert.equal(c.fiveHour.primaryPercent, 28);
    assert.equal(c.fiveHour.secondaryPercent, 72);
    assert.deepEqual(c.compact, [28, 70]);
    assert.equal(
      P.formatReset(1763216000, "en_US", "Asia/Shanghai"),
      "11/15/25, 22:13:20",
    );
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
    assert.deepEqual(
      p.slice(0, 3).map((a) => a.enabled),
      [false, false, false],
    );
    assert.deepEqual(p[0].menu, ["importCurrentAuth", "importAuthFile"]);
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
    assert.equal(p[1].enabled, true);
  },
);
