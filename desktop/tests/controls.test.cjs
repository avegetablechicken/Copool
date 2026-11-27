const { test } = require("node:test");
const assert = require("node:assert/strict");
const P = require("../ui/presentation.js");
const legacy = (suite, name, fn) => test(`${suite}.${name}`, fn);
legacy(
  "SettingsControlPresentationTests",
  "testSwitchBehaviorSectionDisablesRestartTargetPickerWhenSwitchingIsOff",
  () => {
    const p = P.settingsControls({}, [{ id: "cursor", label: "Cursor" }]);
    assert.deepEqual(p.switchToggles, ["autoSmartSwitch", "restartEditorsOnSwitch"]);
    assert.equal(p.restartPicker.isEnabled, false);
    assert.deepEqual(
      p.restartPicker.options.map((o) => o.title),
      ["common.none", "Cursor"],
    );
  },
);
legacy(
  "SettingsControlPresentationTests",
  "testGeneralSectionIncludesUsageProgressDisplayPicker",
  () => {
    const p = P.settingsControls({});
    assert.equal(p.usagePicker.selectedValue, "used");
    assert.deepEqual(p.usagePicker.options, ["used", "remaining"]);
  },
);
test("SettingsControlPresentationTests.testGeneralSectionIncludesThemePicker", () => {
  assert.deepEqual(P.settingsControls({}).themePicker, {
    selectedValue: "system",
    options: ["system", "light", "dark"],
  });
  assert.equal(P.settingsControls({ theme: "dark" }).themePicker.selectedValue, "dark");
  assert.equal(P.settingsControls({ theme: "light" }).themePicker.selectedValue, "light");
  assert.equal(P.settingsControls({ theme: "invalid" }).themePicker.selectedValue, "system");
});
legacy(
  "SettingsControlPresentationTests",
  "testSwitchBehaviorSectionEnablesRestartTargetPickerWhenConfigured",
  () => {
    const p = P.settingsControls(
      { restartEditorsOnSwitch: true, restartEditorTargets: ["vscode"] },
      [{ id: "vscode", label: "VS Code" }],
    );
    assert.equal(p.restartPicker.isEnabled, true);
    assert.equal(p.restartPicker.selectedValue, "vscode");
  },
);
legacy(
  "SettingsControlPresentationTests",
  "testLanguageSectionNormalizesSelectedLocale",
  () => {
    const p = P.settingsControls({ locale: "zh_CN" });
    assert.equal(p.languagePicker.selectedValue, "zh-Hans");
    assert.equal(p.languagePicker.options.length, 11);
  },
);
legacy(
  "AccountsActionPresentationTests",
  "testTrailingToolbarButtonsReflectCollapseStateAndSpinner",
  () => {
    const p = P.trailingActions({
      canRefresh: true,
      spinning: true,
      collapsed: true,
    });
    assert.deepEqual(
      p.map((a) => a.intent),
      ["refreshUsage", "toggleCollapse"],
    );
    assert.equal(p[0].spinning, true);
    assert.equal(p[1].symbol, "chevron.down");
    assert.equal(p[1].label, "accounts.action.expand_all");
  },
);
legacy(
  "PublicAccessSectionPresentationTests",
  "testModeCardsReflectSelectedModeAndEditability",
  () => {
    const p = P.publicModes("named", false);
    assert.deepEqual(
      p.map((a) => a.mode),
      ["quick", "named"],
    );
    assert.equal(p[0].selected, false);
    assert.equal(p[1].selected, true);
    assert.equal(p[0].isEnabled, false);
  },
);
legacy(
  "PublicAccessSectionPresentationTests",
  "testStartLocalProxyCalloutOnlyAppearsWhenProxyIsStopped",
  () => {
    assert.notEqual(P.publicCallout(false), null);
    assert.equal(P.publicCallout(true), null);
  },
);
legacy(
  "PublicAccessSectionPresentationTests",
  "testStatusCardsExposePublicURLCopyBehavior",
  () => {
    const p = P.publicStatus({
      running: true,
      publicURL: "https://example.trycloudflare.com",
      binaryPath: "/usr/local/bin/cloudflared",
    });
    assert.deepEqual(
      p.map((c) => c.id),
      ["status", "url", "install-path", "last-error"],
    );
    assert.equal(p[1].copyValue, "https://example.trycloudflare.com");
    assert.equal(p[1].truncation, "middle");
  },
);
legacy(
  "RemoteServerCardPresentationTests",
  "testHeaderFallsBackToDefaultLabelAndCollapsedSubtitle",
  () => {
    const p = P.remoteHeader({
      label: "",
      sshUser: "root",
      host: "1.2.3.4",
      sshPort: 2222,
      expanded: false,
      status: null,
    });
    assert.equal(p.title, "new-server");
    assert.equal(p.subtitle, "root@1.2.3.4:2222");
    assert.equal(p.isRunning, false);
  },
);
legacy(
  "RemoteServerCardPresentationTests",
  "testMetricsAndDetailsExposeFallbackValues",
  () => {
    const m = P.remoteMetrics(null),
      d = P.remoteDetails(null);
    assert.equal(m.at(-1).value, "--");
    assert.equal(d[0].value, "--");
    assert.equal(d[1].canCopy, false);
    assert.equal(d[1].value, "Generated after first start");
  },
);
legacy(
  "RemoteServerCardPresentationTests",
  "testLogsPresentationUsesPlaceholderWhenLogsMissing",
  () => {
    const p = P.remoteLogs(null);
    assert.equal(p.content, "Logs have not been loaded yet");
    assert.equal(p.canCopy, false);
  },
);

test("SwitchNoticeReportsPiAndOmpSyncSuccessAndErrors", () => {
  const fs = require("node:fs");
  const path = require("node:path");
  const vm = require("node:vm");
  const source = fs
    .readFileSync(path.join(__dirname, "../ui/app.js"), "utf8")
    .replace(/\r\n/g, "\n");
  const start = source.indexOf("function switchNotice(execution, from) {");
  assert.notEqual(start, -1, "switchNotice start marker not found");
  const end = source.indexOf("\n}\n", start);
  assert.notEqual(end, -1, "switchNotice end marker not found");
  const snippet = source.slice(start, end + 2);

  const localeScope = { window: {} };
  vm.runInNewContext(
    fs
      .readFileSync(path.join(__dirname, "../ui/locales.js"), "utf8")
      .replace(/\r\n/g, "\n"),
    localeScope,
  );
  const en = localeScope.window.COPOOL_LOCALES.en;
  const scope = {
    P,
    // The pool on screen now; the notice names the one switched in.
    pool: "grok",
    tr: (key, ...args) => P.formatLocalized(en[key] ?? key, args),
  };
  vm.runInNewContext(snippet, scope);
  assert.equal(typeof scope.switchNotice, "function");

  // The agents the core synced, in its order; an unknown one says nothing.
  const ok = scope.switchNotice({
    agentSync: [
      { agent: "opencode", error: null },
      { agent: "pi", error: null },
      { agent: "omp", error: null },
      { agent: "later", error: null },
    ],
    restartedEditors: [],
  }, "codex");
  assert.equal(ok.error, false);
  assert.match(ok.text, /OpenCode auth synced.*Pi auth synced.*omp auth synced/s);

  const err = scope.switchNotice({
    agentSync: [
      { agent: "pi", error: "disk full" },
      { agent: "omp", error: "database locked" },
    ],
    restartedEditors: [],
  }, "codex");
  assert.equal(err.error, true);
  assert.match(err.text, /Pi sync failed: disk full/);
  assert.match(err.text, /omp sync failed: database locked/);
  assert.doesNotMatch(err.text, /OpenCode/);
  // No agent synced: none is mentioned.
  assert.doesNotMatch(scope.switchNotice({ restartedEditors: [] }, "codex").text, /sync/);
  // Each pool's switch says so in its own words, whichever pool shows now.
  assert.equal(
    scope.switchNotice({ restartedEditors: [] }, "codex").text,
    scope.tr("accounts.notice.switch_done"),
  );
  assert.equal(
    scope.switchNotice({ restartedEditors: [] }, "claude").text,
    scope.tr("accounts.notice.claude_switch_done"),
  );
});

test("SettingsControlPresentationTests.testAgentSyncRowsChooseEveryPoolUnlessSavedOff", () => {
  const agents = [
    { id: "opencode", pools: ["codex", "grok"], target: "~/.config/opencode/auth.json", found: true },
    { id: "omp", pools: ["codex", "claude", "antigravity", "grok"], target: null, found: false },
    { id: "unknown", pools: ["codex"] },
  ];
  const rows = P.agentSyncRows(
    { agentSync: { opencode: { enabled: true, pools: { grok: false } } } },
    agents,
  );
  assert.deepEqual(rows, [
    {
      id: "opencode",
      enabled: true,
      target: "~/.config/opencode/auth.json",
      found: true,
      pools: [
        { pool: "codex", chosen: true },
        { pool: "grok", chosen: false },
      ],
    },
    {
      id: "omp",
      enabled: false,
      target: null,
      found: false,
      pools: ["codex", "claude", "antigravity", "grok"].map((pool) => ({ pool, chosen: true })),
    },
  ]);
});
test("SettingsControlPresentationTests.testToggleAgentSyncPoolKeepsOtherAgentsAndPools", () => {
  const all = { codex: true, claude: true, antigravity: true, grok: true };
  const settings = {
    agentSync: {
      opencode: { enabled: true, pools: all },
      pi: { enabled: false, pools: { ...all, codex: false } },
    },
  };
  assert.deepEqual(P.toggleAgentSyncPool(settings, "opencode", "grok").agentSync, {
    opencode: { enabled: true, pools: { ...all, grok: false } },
    pi: { enabled: false, pools: { ...all, codex: false } },
  });
  assert.equal(P.toggleAgentSyncPool(settings, "pi", "codex").agentSync.pi.pools.codex, true);
  // A selection not saved yet starts from every pool chosen.
  assert.deepEqual(P.toggleAgentSyncPool({}, "omp", "claude"), {
    agentSync: { omp: { pools: { claude: false } } },
  });
  // A switch keeps the agent's pools and the other agents.
  assert.deepEqual(P.setAgentSync(settings, "pi", true).agentSync, {
    opencode: { enabled: true, pools: all },
    pi: { enabled: true, pools: { ...all, codex: false } },
  });
  assert.deepEqual(P.agentSync({}, "omp"), { enabled: false, pools: {} });
  assert.equal(P.agentSync(settings, "opencode").enabled, true);
});

