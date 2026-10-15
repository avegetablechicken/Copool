use crate::widgets::*;
#[test]
fn testDisplayModeStorePersistsSharedMode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mode.json");
    assert_eq!(load_display_mode(&path), "used");
    save_display_mode(&path, "remaining").unwrap();
    assert_eq!(load_display_mode(&path), "remaining");
}
#[test]
fn testLoadReturnsEmptyForInvalidSnapshotData() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("accounts-widget.json");
    std::fs::write(&path, "not json").unwrap();
    assert_eq!(load(&path), Snapshot::default());
}
#[test]
fn testSaveWritesSnapshotToExplicitURL() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/accounts-widget.json");
    let s = build(
        Pool::Codex,
        &[super::account("current", 30., 20., true)],
        "remaining",
        Some(0),
        1,
    );
    save(&path, &s).unwrap();
    let decoded: Snapshot = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(decoded, s);
}
#[test]
fn testWriterUsesExplicitUsageProgressDisplayModeOverride() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("accounts-widget.json");
    let s = Widget::Codex.build(
        &[super::account("current", 40., 25., true)],
        &[],
        "remaining",
        Some(0),
        1,
    );
    write(Widget::Codex, &path, &s, |_| {}).unwrap();
    assert_eq!(load(&path).usage_progress_display_mode, "remaining");
}
#[test]
fn testWriterReloadsCurrentWidgetKindAfterSave() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("accounts-widget.json");
    let mut reloaded = None;
    let s = build(
        Pool::Claude,
        &[super::account("current", 40., 25., true)],
        "used",
        Some(0),
        1,
    );
    write(Widget::Claude, &path, &s, |kind| {
        assert!(path.is_file());
        reloaded = Some(kind.to_owned());
    })
    .unwrap();
    assert_eq!(reloaded.as_deref(), Some(Widget::Claude.kind()));
}
#[test]
fn testSnapshotCarriesOwnDisplayModeForWidgetRendering() {
    let s = Snapshot {
        generated_at: 1,
        usage_progress_display_mode: "remaining".into(),
        ..Default::default()
    };
    assert_eq!(s.usage_progress_display_mode, "remaining");
}
#[test]
fn testPlanTagPaletteUsesMonochromeContrastInAccentedMode() {
    let p = plan_palette("PRO", false, true);
    assert_eq!(p.fill, [1., 1., 1., 0.18]);
    assert_eq!(p.text, [1., 1., 1., 0.98]);
}
#[test]
fn testPlanTagPaletteKeepsFullColorPaletteOutsideAccentedMode() {
    let p = plan_palette("PRO", false, false);
    assert_eq!(p.fill, [0.98, 0.82, 0.63, 1.]);
    assert_eq!(p.text, [0.78, 0.42, 0.03, 1.]);
}
#[test]
fn testPlanTagPaletteGivesClaudeMaxPlansTheirOwnColor() {
    assert_eq!(
        plan_palette("MAX 5X", false, false),
        plan_palette("MAX", false, false)
    );
    assert_ne!(
        plan_palette("MAX 5X", true, false),
        plan_palette("TEAM", true, false)
    );
}
