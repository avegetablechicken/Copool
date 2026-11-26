use crate::Pool;
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
fn cardsCarryTheAppsPlanAndWorkspaceTags() {
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
    let accounts = [max, team, unknown, provider];
    let now = 1_795_392_000; // 2026-11-23
    let s = build(Pool::Claude, &accounts, "used", Some(0), now);
    let cards: std::collections::HashMap<_, _> = s
        .current_card
        .iter()
        .chain(&s.rows)
        .map(|card| (card.id.as_str(), card))
        .collect();
    assert_eq!(cards.len(), accounts.len());
    for account in &accounts {
        let tags = crate::plan_tags::of(Pool::Claude, account, now);
        let card = cards[account.id.as_str()];
        // A card without a plan tag has an empty label, which the widget
        // draws no tag for.
        assert_eq!(
            card.plan_label,
            tags.tag.as_ref().map_or("", |t| t.label.as_str()),
            "{}",
            account.id
        );
        assert_eq!(
            card.plan_accent,
            tags.tag.as_ref().map_or("teal", |t| t.accent)
        );
        assert_eq!(card.workspace_label, tags.workspace);
    }
    assert_eq!(cards["max"].plan_label, "ULTRA");
    assert_eq!(cards["unknown"].plan_label, "");
    assert_eq!(cards["unknown"].account_label, "label");
    assert_eq!(
        cards["provider"].workspace_label.as_deref(),
        Some("SUB2API")
    );
}
#[test]
fn bothPoolsWidgetLeadsWithEachPoolsCurrentAccountThenRanksTheRest() {
    let codex = [
        account("codex-current", true, "codex@example.com", 90., 90.),
        account("codex-low", false, "low@example.com", 80., 80.),
    ];
    let claude = [
        account("claude-current", true, "claude@example.com", 95., 95.),
        account("claude-high", false, "high@example.com", 10., 10.),
    ];
    let s = Widget::All.build(
        &[(Pool::Codex, &codex), (Pool::Claude, &claude)],
        "used",
        Some(0),
        100,
    );
    let current = s.current_card.unwrap();
    assert_eq!(current.id, "codex-current");
    assert_eq!(current.pool_label.as_deref(), Some("CODEX"));
    let secondary = s.secondary_card.unwrap();
    assert_eq!(secondary.id, "claude-current");
    assert_eq!(secondary.pool_label.as_deref(), Some("CLAUDE"));
    let ids: Vec<_> = s.rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["claude-current", "claude-high", "codex-low"]);
}
#[test]
fn everyPoolsWidgetLeadsWithAntigravitysCurrentAccountAfterTheOthers() {
    let codex = [account(
        "codex-current",
        true,
        "codex@example.com",
        90.,
        90.,
    )];
    let claude = [account(
        "claude-current",
        true,
        "claude@example.com",
        95.,
        95.,
    )];
    let antigravity = [
        account("agy-current", true, "agy@gmail.com", 20., 0.),
        account("agy-spare", false, "spare@gmail.com", 0., 0.),
    ];
    let s = Widget::All.build(
        &[
            (Pool::Codex, &codex),
            (Pool::Claude, &claude),
            (Pool::Antigravity, &antigravity),
        ],
        "used",
        Some(0),
        100,
    );
    let ids: Vec<_> = s.rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["claude-current", "agy-current", "agy-spare"]);
    let agy = &s.rows[1];
    assert_eq!(agy.pool_label.as_deref(), Some("ANTIGRAVITY"));
    assert_eq!(agy.account_label, "agy");
    // Gemini's windows, as the app's collapsed rows.
    assert_eq!(agy.five_hour.used_text, "20%");
    // Its own widget names the pool too.
    let own = Widget::Antigravity.build(&[(Pool::Antigravity, &antigravity)], "used", Some(0), 100);
    assert_eq!(
        own.current_card.unwrap().pool_label.as_deref(),
        Some("ANTIGRAVITY")
    );
    assert_eq!(
        Widget::Antigravity.kind(),
        "CopoolAntigravityAccountsWidget"
    );
}
#[test]
fn grokHasItsOwnWidgetAndJoinsEveryPoolsWidget() {
    // xAI reports a weekly allowance only.
    let mut grok = [
        account("grok-current", true, "grok@example.com", 0., 7.),
        account("grok-spare", false, "spare@example.com", 0., 40.),
    ];
    for account in &mut grok {
        account.usage.as_mut().unwrap().five_hour = None;
    }
    let own = Widget::Grok.build(&[(Pool::Grok, &grok)], "used", Some(0), 100);
    let current = own.current_card.unwrap();
    assert_eq!(current.pool_label.as_deref(), Some("GROK"));
    assert_eq!(current.account_label, "grok");
    assert!(!current.five_hour.available);
    assert_eq!(current.one_week.used_text, "7%");
    assert_eq!(own.secondary_card.unwrap().id, "grok-spare");
    assert_eq!(Widget::Grok.kind(), "CopoolGrokAccountsWidget");
    assert_eq!(
        Widget::Grok.filename(),
        "grok-accounts-widget-snapshot.json"
    );
    assert_eq!(Widget::Grok.pools(), &[Pool::Grok]);
    assert!(Widget::ALL.contains(&Widget::Grok));

    let codex = [account(
        "codex-current",
        true,
        "codex@example.com",
        10.,
        10.,
    )];
    let all = Widget::All.build(
        &[(Pool::Codex, &codex), (Pool::Grok, &grok)],
        "used",
        Some(0),
        100,
    );
    assert_eq!(all.current_card.unwrap().id, "codex-current");
    let grok_current = all.secondary_card.unwrap();
    assert_eq!(grok_current.id, "grok-current");
    assert_eq!(grok_current.pool_label.as_deref(), Some("GROK"));

    // An allowance lasting a month shows as Codex Free's does.
    let usage = grok[0].usage.as_mut().unwrap();
    usage.one_month = usage.one_week.take();
    let own = Widget::Grok.build(&[(Pool::Grok, &grok)], "used", Some(0), 100);
    let month = own.current_card.unwrap().one_week;
    assert_eq!(
        (month.title.as_str(), month.used_text.as_str()),
        ("1m", "7%")
    );
}
#[test]
fn bothPoolsWidgetShowsTheOtherPoolWhenOneIsEmpty() {
    let claude = [account("claude", true, "claude@example.com", 10., 20.)];
    let s = Widget::All.build(&[(Pool::Claude, &claude)], "used", Some(0), 100);
    assert_eq!(s.current_card.unwrap().id, "claude");
    assert_eq!(s.secondary_card, None);
    assert_eq!(
        Widget::All.build(&[], "used", Some(0), 100),
        Snapshot::default()
    );
}
#[test]
fn singlePoolWidgetsNameTheirPoolOnEveryCard() {
    let s = Widget::Codex.build(
        &[(
            Pool::Codex,
            &[
                account("current", true, "current@example.com", 10., 20.),
                account("spare", false, "spare@example.com", 30., 40.),
            ],
        )],
        "used",
        Some(0),
        100,
    );
    assert_eq!(s.current_card.unwrap().pool_label.as_deref(), Some("CODEX"));
    assert!(s
        .rows
        .iter()
        .all(|row| row.pool_label.as_deref() == Some("CODEX")));
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
#[test]
fn widgetPercentagesFloorUsedAndCeilRemaining() {
    let s = build(
        Pool::Codex,
        &[account("current", true, "current@example.com", 25.5, 99.9)],
        "remaining",
        Some(0),
        100,
    );
    let card = s.current_card.unwrap();
    assert_eq!(card.five_hour.used_text, "25%");
    assert_eq!(card.five_hour.remaining_text, "75%");
    assert_eq!(card.one_week.used_text, "99%");
    assert_eq!(card.one_week.remaining_text, "1%");
}
