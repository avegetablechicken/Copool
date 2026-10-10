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
  "PublicAccessSectionPresentationTests",
  "testStartLocalProxyCalloutOnlyAppearsWhenProxyIsStopped",
  () => {
    assert.notEqual(P.publicCallout(false), null);
    assert.equal(P.publicCallout(true), null);
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
  // The Coport panel has no remote detail cards; the metrics still apply.
  () => {
    const m = P.remoteMetrics(null);
    assert.equal(m.at(-1).value, "--");
    assert.equal(m[0].value, "Unknown");
  },
);
