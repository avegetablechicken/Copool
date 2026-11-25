const { test } = require("node:test");
const assert = require("node:assert/strict");
const P = require("../ui/presentation.js");
const legacy = (suite, name, fn) => test(`${suite}.${name}`, fn);
legacy(
  "SettingsControlPresentationTests",
  "testSwitchBehaviorSectionDisablesRestartTargetPickerWhenSwitchingIsOff",
  () => {
    const p = P.settingsControls({}, [{ id: "cursor", label: "Cursor" }]);
    assert.deepEqual(p.switchToggles, [
      "autoSmartSwitch",
      "syncOpencodeOpenaiAuth",
      "syncPiAuth",
      "syncOmpAuth",
      "restartEditorsOnSwitch",
    ]);
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
  const start = source.indexOf("function switchNotice(execution) {");
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
    pool: "codex",
    tr: (key, ...args) => P.formatLocalized(en[key] ?? key, args),
  };
  vm.runInNewContext(snippet, scope);
  assert.equal(typeof scope.switchNotice, "function");

  const ok = scope.switchNotice({
    opencodeSynced: true,
    piSynced: true,
    ompSynced: true,
    restartedEditors: [],
  });
  assert.equal(ok.error, false);
  assert.match(ok.text, /Opencode auth synced/);
  assert.match(ok.text, /pi auth synced/);
  assert.match(ok.text, /omp auth synced/);

  const err = scope.switchNotice({
    piSyncError: "disk full",
    ompSyncError: "database locked",
    restartedEditors: [],
  });
  assert.equal(err.error, true);
  assert.match(err.text, /pi sync failed: disk full/);
  assert.match(err.text, /omp sync failed: database locked/);
});
