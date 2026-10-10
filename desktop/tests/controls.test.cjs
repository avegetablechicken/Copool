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
  "AccountsActionPresentationTests",
  "testLeadingToolbarButtonsIncludeUsageToggleIcon",
  () => {
    const p = P.leadingActions();
    assert.deepEqual(
      p.map((a) => a.intent),
      ["toggleUsageProgressDisplay", "importCurrentAuth", "addAccount"],
    );
    assert.equal(p[0].symbol, "switch.2");
    assert.equal(p[0].label, "accounts.action.toggle_usage_progress_display");
    assert.equal(p[1].label, "accounts.action.import_account");
    assert.deepEqual(p[1].menu, ["importCurrentAuth", "importAuthFile"]);
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
legacy(
  "LiquidProgressMetricsTests",
  "testDarkGroovePaletteKeepsRingTrackVisibleOnBlackBackground",
  () => {
    const p = P.darkGroovePalette;
    assert.equal(p.glassTintOpacity, 0.14);
    assert.equal(p.topEdgeOpacity, 0.24);
    assert.equal(p.centerGlowOpacity, 0.11);
    assert.equal(p.ringOuterHighlightOpacity, 0.22);
  },
);
