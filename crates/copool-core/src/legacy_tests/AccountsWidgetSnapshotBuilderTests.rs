use crate::{models::AccountSummary, widgets::*};
fn account(id: &str, current: bool, email: &str, hour: f64, week: f64) -> AccountSummary {
    let mut a = super::account(id, week, hour, current);
    a.email = Some(email.into());
    a.plan_type = Some("team".into());
    let u = a.usage.as_mut().unwrap();
    u.five_hour.as_mut().unwrap().reset_at = Some(1774020225);
    u.one_week.as_mut().unwrap().reset_at = Some(1774106625);
    a
}
#[test]
fn currentAccountBecomesPrimaryCardAndSecondaryUsesNextDisplayAccount() {
    let s = build(
        Pool::Codex,
        &[
            account("b", false, "second@example.com", 35., 45.),
            account("a", true, "current@example.com", 15., 25.),
        ],
        "used",
        Some(0),
        100,
    );
    let current = s.current_card.unwrap();
    let secondary = s.secondary_card.unwrap();
    assert_eq!(current.id, "a");
    assert_eq!(current.account_label, "current");
    assert_eq!(secondary.id, "b");
    assert_eq!(secondary.account_label, "second");
}
#[test]
fn rowsKeepWorkspaceAndAccountLabelsAndExposeWindowSnapshots() {
    let mut team = account("team", false, "member@example.com", 45., 55.);
    team.team_name = Some("workspace".into());
    let s = build(
        Pool::Codex,
        &[
            account("current", true, "current@example.com", 10., 20.),
            team,
        ],
        "used",
        Some(0),
        100,
    );
    assert_eq!(s.rows.len(), 1);
    assert_eq!(s.rows[0].workspace_label.as_deref(), Some("workspace"));
    assert_eq!(s.rows[0].account_label, "member");
    assert_eq!(s.rows[0].five_hour.remaining_text, "55%");
    assert_eq!(s.rows[0].one_week.remaining_text, "45%");
}
#[test]
fn resetTimeUsesStableWidgetFormat() {
    let s = build(
        Pool::Codex,
        &[account("current", true, "current@example.com", 12., 34.)],
        "used",
        Some(8 * 3600),
        100,
    );
    let card = s.current_card.unwrap();
    assert_eq!(card.five_hour.reset_text, "26/3/20 23:23:45");
    assert_eq!(card.one_week.reset_text, "26/3/21 23:23:45");
}
#[test]
fn defaultBuilderLimitsLargeRowsToFourAccounts() {
    let accounts = ["current", "a", "b", "c", "d"]
        .map(|id| account(id, id == "current", &format!("{id}@example.com"), 10., 20.));
    assert_eq!(
        build(Pool::Codex, &accounts, "used", Some(0), 100)
            .rows
            .len(),
        4
    );
}
#[test]
fn snapshotCarriesUsageProgressDisplayMode() {
    let s = build(
        Pool::Codex,
        &[account("current", true, "current@example.com", 12., 34.)],
        "remaining",
        Some(0),
        100,
    );
    assert_eq!(s.usage_progress_display_mode, "remaining");
    assert_eq!(s.current_card.as_ref().unwrap().five_hour.used_text, "12%");
    assert_eq!(s.current_card.unwrap().five_hour.remaining_text, "88%");
}
#[test]
fn claudeCardsFollowTheAppsPlanAndWorkspaceTags() {
    let mut max = account("max", true, "max@example.com", 10., 20.);
    max.plan_type = Some("max 20x".into());
    max.team_name = Some("max@example.com's Organization".into());
    let mut team = account("team", false, "member@example.com", 30., 40.);
    team.team_name = Some("Acme".into());
    let mut unknown = account("unknown", false, "new@example.com", 50., 60.);
    unknown.plan_type = None;
    unknown.email = None;
    unknown.label = "label@example.com".into();
    let mut provider = account("provider", false, "relay", 70., 80.);
    provider.plan_type = None;
    provider.source_tag = Some("SUB2API".into());
    let s = build(
        Pool::Claude,
        &[max, team, unknown, provider],
        "used",
        Some(0),
        100,
    );
    let current = s.current_card.unwrap();
    assert_eq!(current.plan_label, "MAX 20X");
    assert_eq!(current.workspace_label, None);
    let rows: std::collections::HashMap<_, _> = s.rows.iter().map(|r| (r.id.as_str(), r)).collect();
    assert_eq!(rows["team"].plan_label, "TEAM");
    assert_eq!(rows["team"].workspace_label.as_deref(), Some("Acme"));
    assert_eq!(rows["unknown"].plan_label, "");
    assert_eq!(rows["unknown"].account_label, "label");
    assert_eq!(rows["provider"].workspace_label.as_deref(), Some("SUB2API"));
}
#[test]
fn codexCardsKeepTreatingUnknownPlansAsTeam() {
    let mut a = account("current", true, "current@example.com", 10., 20.);
    a.plan_type = Some("max 20x".into());
    let s = build(Pool::Codex, &[a], "used", Some(0), 100);
    assert_eq!(s.current_card.unwrap().plan_label, "TEAM");
}
#[test]
fn bothPoolsWidgetLeadsWithEachPoolsCurrentAccountThenRanksTheRest() {
    let codex = [
        account("codex-current", true, "codex@example.com", 90., 90.),
        account("codex-low", false, "low@example.com", 80., 80.),
    ];
    let mut claude = [
        account("claude-current", true, "claude@example.com", 95., 95.),
        account("claude-high", false, "high@example.com", 10., 10.),
    ];
    claude[0].plan_type = Some("max 5x".into());
    claude[1].plan_type = None;
    let s = Widget::All.build(&codex, &claude, "used", Some(0), 100);
    let current = s.current_card.unwrap();
    assert_eq!(current.id, "codex-current");
    assert_eq!(current.pool_label.as_deref(), Some("CODEX"));
    let secondary = s.secondary_card.unwrap();
    assert_eq!(secondary.id, "claude-current");
    assert_eq!(secondary.pool_label.as_deref(), Some("CLAUDE"));
    assert_eq!(secondary.plan_label, "MAX 5X");
    let ids: Vec<_> = s.rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["claude-current", "claude-high", "codex-low"]);
    // Each card keeps its own pool's tags.
    assert_eq!(s.rows[1].plan_label, "");
    assert_eq!(s.rows[2].plan_label, "TEAM");
}
#[test]
fn bothPoolsWidgetShowsTheOtherPoolWhenOneIsEmpty() {
    let claude = [account("claude", true, "claude@example.com", 10., 20.)];
    let s = Widget::All.build(&[], &claude, "used", Some(0), 100);
    assert_eq!(s.current_card.unwrap().id, "claude");
    assert_eq!(s.secondary_card, None);
    assert_eq!(
        Widget::All.build(&[], &[], "used", Some(0), 100),
        Snapshot::default()
    );
}
#[test]
fn singlePoolWidgetsCarryNoPoolTag() {
    let s = Widget::Codex.build(
        &[account("current", true, "current@example.com", 10., 20.)],
        &[],
        "used",
        Some(0),
        100,
    );
    assert_eq!(s.current_card.unwrap().pool_label, None);
}
#[test]
fn aWindowWithoutUsageShowsNoDataRatherThanExhausted() {
    let mut a = account("current", true, "current@example.com", 10., 20.);
    a.usage.as_mut().unwrap().five_hour = None;
    let s = build(Pool::Codex, &[a], "remaining", Some(0), 100);
    let card = s.current_card.unwrap();
    assert!(!card.five_hour.available);
    assert_eq!(card.five_hour.progress_fraction, 0.);
    assert_eq!(card.five_hour.used_text, "--");
    assert_eq!(card.five_hour.remaining_text, "--");
    assert_eq!(card.five_hour.reset_text, "--");
    assert!(card.one_week.available);
    assert_eq!(card.one_week.used_text, "20%");
}
