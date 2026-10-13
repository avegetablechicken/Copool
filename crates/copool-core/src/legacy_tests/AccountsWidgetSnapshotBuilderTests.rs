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
    assert_eq!(build(&accounts, "used", Some(0), 100).rows.len(), 4);
}
#[test]
fn snapshotCarriesUsageProgressDisplayMode() {
    let s = build(
        &[account("current", true, "current@example.com", 12., 34.)],
        "remaining",
        Some(0),
        100,
    );
    assert_eq!(s.usage_progress_display_mode, "remaining");
    assert_eq!(s.current_card.as_ref().unwrap().five_hour.used_text, "12%");
    assert_eq!(s.current_card.unwrap().five_hour.remaining_text, "88%");
}
