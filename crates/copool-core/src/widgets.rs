//! Portable widget snapshot format. WidgetKit presentation/reload is a host adapter.
use crate::{
    models::{AccountSummary, UsageWindow},
    persistence::write_json,
    pool::Pool,
    ranking::{remaining_score, sort_for_display},
};
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The tag naming a card's pool.
fn pool_label(pool: Pool) -> &'static str {
    match pool {
        Pool::Codex => "CODEX",
        Pool::Claude => "CLAUDE",
        Pool::Antigravity => "ANTIGRAVITY",
        Pool::Grok => "GROK",
    }
}
/// A widget the app publishes; each has its own WidgetKit kind and snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Widget {
    Codex,
    Claude,
    Antigravity,
    Grok,
    /// Every pool.
    All,
}
impl Widget {
    pub const ALL: [Self; 5] = [
        Self::Codex,
        Self::Claude,
        Self::Antigravity,
        Self::Grok,
        Self::All,
    ];
    pub fn kind(self) -> &'static str {
        match self {
            Self::Codex => "CopoolCodexAccountsWidget",
            Self::Claude => "CopoolClaudeAccountsWidget",
            Self::Antigravity => "CopoolAntigravityAccountsWidget",
            Self::Grok => "CopoolGrokAccountsWidget",
            Self::All => "CopoolAccountsWidget",
        }
    }
    /// The snapshot's name in the data directory and the app group container.
    pub fn filename(self) -> &'static str {
        match self {
            Self::Codex => "codex-accounts-widget-snapshot.json",
            Self::Claude => "claude-accounts-widget-snapshot.json",
            Self::Antigravity => "antigravity-accounts-widget-snapshot.json",
            Self::Grok => "grok-accounts-widget-snapshot.json",
            Self::All => "accounts-widget-snapshot.json",
        }
    }
    /// The pools the widget shows, in their order.
    pub fn pools(self) -> &'static [Pool] {
        match self {
            Self::Codex => &[Pool::Codex],
            Self::Claude => &[Pool::Claude],
            Self::Antigravity => &[Pool::Antigravity],
            Self::Grok => &[Pool::Grok],
            Self::All => &Pool::ALL,
        }
    }
    /// The widget's snapshot from the accounts of its pools; a pool not
    /// given has none.
    pub fn build(
        self,
        pools: &[(Pool, &[AccountSummary])],
        mode: &str,
        offset_seconds: Option<i32>,
        now: i64,
    ) -> Snapshot {
        let accounts = |pool: Pool| {
            pools
                .iter()
                .find(|(p, _)| *p == pool)
                .map(|(_, accounts)| *accounts)
                .unwrap_or_default()
        };
        match self {
            Self::All => {
                let pools: Vec<_> = Pool::ALL.iter().map(|&p| (p, accounts(p))).collect();
                build_all(&pools, mode, offset_seconds, now)
            }
            single => {
                let pool = single.pools()[0];
                build(pool, accounts(pool), mode, offset_seconds, now)
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowSnapshot {
    pub title: String,
    /// Whether the window has usage; one without shows no data.
    pub available: bool,
    pub progress_fraction: f64,
    pub used_text: String,
    pub remaining_text: String,
    pub reset_text: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardSnapshot {
    pub id: String,
    /// The tag naming the card's pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_label: Option<String>,
    pub plan_label: String,
    /// The plan's tier color, as the app's plan tag (`plan_accent`).
    #[serde(default)]
    pub plan_accent: String,
    pub workspace_label: Option<String>,
    pub account_label: String,
    pub five_hour: WindowSnapshot,
    pub one_week: WindowSnapshot,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub generated_at: i64,
    pub usage_progress_display_mode: String,
    pub current_card: Option<CardSnapshot>,
    pub secondary_card: Option<CardSnapshot>,
    pub rows: Vec<CardSnapshot>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            generated_at: 0,
            usage_progress_display_mode: "used".into(),
            current_card: None,
            secondary_card: None,
            rows: vec![],
        }
    }
}
fn normalized_mode(mode: &str) -> &str {
    if mode == "remaining" {
        "remaining"
    } else {
        "used"
    }
}
/// The offset a reset is rendered in: the caller's when the host resolved one,
/// otherwise the local one in effect at the reset instant, so the zone of a
/// reset still ahead cannot be taken from the offset at build time.
fn zone_offset(offset: Option<i32>, date: DateTime<chrono::Utc>) -> FixedOffset {
    let seconds = offset.unwrap_or_else(|| {
        date.with_timezone(&chrono::Local)
            .offset()
            .local_minus_utc()
    });
    FixedOffset::east_opt(seconds).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap())
}
/// As the app's usage rows: a window without usage (e.g. no five-hour limit)
/// shows no data, neither used nor remaining, and no reset.
fn window(title: &str, window: Option<&UsageWindow>, offset: Option<i32>) -> WindowSnapshot {
    let Some(window) = window.filter(|w| w.used_percent.is_finite()) else {
        return WindowSnapshot {
            title: title.into(),
            available: false,
            progress_fraction: 0.,
            used_text: "--".into(),
            remaining_text: "--".into(),
            reset_text: "--".into(),
        };
    };
    let used = window.used_percent.clamp(0., 100.);
    let reset = window
        .reset_at
        .and_then(|stamp| DateTime::from_timestamp(stamp, 0))
        .map(|date| {
            date.with_timezone(&zone_offset(offset, date))
                .format("%y/%-m/%-d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "--".into());
    let used_display = used.floor();
    WindowSnapshot {
        title: title.into(),
        available: true,
        progress_fraction: used / 100.,
        used_text: format!("{}%", used_display),
        remaining_text: format!("{}%", 100. - used_display),
        reset_text: reset,
    }
}
/// When OpenAI added its $100 Pro (prolite) beside the $200 one
/// (2026-04-09 UTC).
const CODEX_PRO_LITE: i64 = 1_775_692_800;
/// When Google added its $100 Ultra beside the top one (2026-05-19 UTC).
const GOOGLE_ULTRA_LITE: i64 = 1_779_148_800;
/// Codex treats an unknown plan as Team. Its Pro plans, $100 (prolite),
/// $200 (pro) and $500 (promax), are named as OpenAI does once the $100
/// one came.
fn codex_plan(raw: Option<String>, now: i64) -> String {
    let raw = raw.unwrap_or_else(|| "TEAM".into());
    match raw.as_str() {
        "PROLITE" => "PRO 5X".into(),
        "PROMAX" => "PRO 50X".into(),
        "PRO" if now >= CODEX_PRO_LITE => "PRO 20X".into(),
        // ChatGPT Go, the $8 plan below Plus.
        "GO" | "PRO" | "PLUS" | "FREE" | "TEAM" | "BUSINESS" | "ENTERPRISE" => raw,
        _ => "TEAM".into(),
    }
}
/// Both Ultra plans are "Google AI Ultra": known by their tier id as
/// "ULTRA 5X" and "ULTRA 20X", else as "ULTRA" alone. The top one is plain
/// Ultra before the $100 one came.
fn antigravity_plan(plan: String, now: i64) -> String {
    match plan.as_str() {
        "ULTRA 5X" | "ULTRA LITE" => "ULTRA 5X".into(),
        "ULTRA 20X" if now < GOOGLE_ULTRA_LITE => "ULTRA".into(),
        _ => plan,
    }
}
/// When Claude renamed Max 5x to Max and Max 20x to Ultra (2026-10-30 UTC).
const CLAUDE_PLAN_RENAME: i64 = 1_793_318_400;
/// As the app's plan tag: Claude shows none until its plan is known, and its
/// Max plans are named as they were at `now`: "MAX 5X" and "MAX 20X" before
/// the rename, "MAX" and "ULTRA" after. Claude Code reports both as "max"
/// with the multiplier, also since the rename; "ultra" is taken should it
/// report one.
fn claude_plan(raw: Option<String>, now: i64) -> String {
    let Some(raw) = raw else {
        return String::new();
    };
    let raw = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = raw
        .strip_prefix("CLAUDE ")
        .or_else(|| raw.strip_prefix("CLAUDE_"))
        .unwrap_or(&raw);
    let times = match name.strip_prefix("MAX") {
        Some("") => Some(0),
        Some(rest) => rest
            .strip_prefix(' ')
            .and_then(|r| r.strip_suffix('X'))
            .and_then(|n| n.parse::<u32>().ok()),
        None => (name == "ULTRA").then_some(20),
    };
    if let Some(times) = times {
        return if now >= CLAUDE_PLAN_RENAME {
            if times >= 20 { "ULTRA" } else { "MAX" }.into()
        } else if times == 0 {
            "MAX".into()
        } else {
            format!("MAX {times}X")
        };
    }
    if ["PRO", "PLUS", "FREE", "BUSINESS", "ENTERPRISE"].contains(&name) {
        name.into()
    } else {
        "TEAM".into()
    }
}
/// The multiplier of a plan named `base` or "`base` <n>X", 0 for the plain one.
fn multiplier(plan: &str, base: &str) -> Option<u32> {
    match plan.strip_prefix(base)? {
        "" => Some(0),
        rest => rest.strip_prefix(' ')?.strip_suffix('X')?.parse().ok(),
    }
}
/// A plan's color, by tier so that providers' comparable plans match, as
/// the app's `planAccent`: the entry subscription (Codex Go and Plus, Claude
/// Pro, Google AI Pro, SuperGrok Lite and SuperGrok) is pink; those of $100 to $300, which their
/// labels tell apart by multiplier (Codex Pro 5x and 20x, Claude Max and
/// Ultra, Google AI Ultra 5x and 20x, SuperGrok Plus and Heavy), orange; and
/// Codex's $500 Pro 50x purple.
pub fn plan_accent(pool: Pool, plan: &str) -> &'static str {
    if pool == Pool::Grok {
        // Sub2api may space the name ("SUPER GROK HEAVY"). Also The X
        // Subscription's names, which may replace SuperGrok's.
        match plan.replace(' ', "").as_str() {
            "SUPERGROKLITE" | "SUPERGROK" | "PLUS" | "PREMIUM" => return "pink",
            "SUPERGROKPLUS" | "SUPERGROKHEAVY" | "SUPER" | "ULTRA" => return "orange",
            "FREE" | "BUSINESS" | "ENTERPRISE" => {}
            _ => return "teal",
        }
    }
    if pool == Pool::Codex {
        if let Some(times) = multiplier(plan, "PRO") {
            return if times >= 50 { "purple" } else { "orange" };
        }
    }
    if multiplier(plan, "MAX").is_some() || multiplier(plan, "ULTRA").is_some() {
        return "orange";
    }
    match plan {
        "GO" | "PRO" | "PLUS" => "pink",
        "FREE" => "gray",
        "BUSINESS" | "ENTERPRISE" => "indigo",
        _ => "teal",
    }
}
fn card(pool: Pool, account: &AccountSummary, offset: Option<i32>, now: i64) -> CardSnapshot {
    let raw = account
        .usage
        .as_ref()
        .and_then(|u| u.plan_type.as_ref())
        .filter(|s| !s.trim().is_empty())
        .or(account.plan_type.as_ref().filter(|s| !s.trim().is_empty()))
        .map(|s| s.trim().to_uppercase());
    let (plan, workspace, name) = match pool {
        Pool::Codex => (
            codex_plan(raw, now),
            account.team_name.clone(),
            account.email.as_deref().unwrap_or(&account.account_id),
        ),
        Pool::Claude => {
            let plan = claude_plan(raw, now);
            // Every Claude login names an organization; as the app, only a
            // shared plan's, or the provider's source, is shown.
            let workspace = account.source_tag.clone().or_else(|| {
                ["TEAM", "BUSINESS", "ENTERPRISE"]
                    .contains(&plan.as_str())
                    .then(|| account.team_name.clone())
                    .flatten()
            });
            (
                plan,
                workspace,
                account.email.as_deref().unwrap_or(&account.label),
            )
        }
        // As the app's plan tag: none until the plan is known. The windows
        // are Gemini's, as the app's collapsed rows show them.
        Pool::Antigravity | Pool::Grok => (
            raw.map(|plan| plan.split_whitespace().collect::<Vec<_>>().join(" "))
                .map(|plan| match pool {
                    Pool::Antigravity => antigravity_plan(plan, now),
                    _ => plan,
                })
                .unwrap_or_default(),
            None,
            account.email.as_deref().unwrap_or(&account.label),
        ),
    };
    let label = name.trim().split('@').next().unwrap_or_default().to_owned();
    let (long_title, long_window) = match (
        account.usage.as_ref().and_then(|u| u.one_week.as_ref()),
        account.usage.as_ref().and_then(|u| u.one_month.as_ref()),
    ) {
        // Codex Free, and a Grok allowance lasting a month.
        (None, Some(month)) if matches!(pool, Pool::Codex | Pool::Grok) => ("1m", Some(month)),
        (week, _) => ("1w", week),
    };
    CardSnapshot {
        id: account.id.clone(),
        pool_label: Some(pool_label(pool).into()),
        plan_accent: plan_accent(pool, &plan).into(),
        plan_label: plan,
        workspace_label: workspace,
        account_label: label,
        five_hour: window(
            "5h",
            account.usage.as_ref().and_then(|u| u.five_hour.as_ref()),
            offset,
        ),
        one_week: window(long_title, long_window, offset),
    }
}
/// The first card is the current one, the next the secondary; the large
/// widget lists up to four after the current.
fn snapshot(cards: Vec<CardSnapshot>, mode: &str, now: i64) -> Snapshot {
    let mut cards = cards.into_iter();
    let Some(current) = cards.next() else {
        return Snapshot::default();
    };
    let rows: Vec<_> = cards.take(4).collect();
    Snapshot {
        generated_at: now,
        usage_progress_display_mode: normalized_mode(mode).into(),
        current_card: Some(current),
        secondary_card: rows.first().cloned(),
        rows,
    }
}
pub fn build(
    pool: Pool,
    accounts: &[AccountSummary],
    mode: &str,
    offset_seconds: Option<i32>,
    now: i64,
) -> Snapshot {
    let mut accounts = accounts.to_vec();
    sort_for_display(&mut accounts);
    let cards = accounts
        .iter()
        .map(|a| card(pool, a, offset_seconds, now))
        .collect();
    snapshot(cards, mode, now)
}
/// Every pool: each pool's first card (its current account) leads, in the
/// pools' order, then the other accounts by remaining quota.
pub fn build_all(
    pools: &[(Pool, &[AccountSummary])],
    mode: &str,
    offset_seconds: Option<i32>,
    now: i64,
) -> Snapshot {
    let mut leading = vec![];
    let mut others = vec![];
    for &(pool, accounts) in pools {
        let mut accounts = accounts.to_vec();
        sort_for_display(&mut accounts);
        let mut accounts = accounts.into_iter().map(|a| (pool, a));
        leading.extend(accounts.next());
        others.extend(accounts);
    }
    others.sort_by(|(_, a), (_, b)| {
        remaining_score(b)
            .total_cmp(&remaining_score(a))
            .then(a.added_at.cmp(&b.added_at))
    });
    let cards = leading
        .into_iter()
        .chain(others)
        .map(|(pool, a)| card(pool, &a, offset_seconds, now))
        .collect();
    snapshot(cards, mode, now)
}
pub fn load(path: &Path) -> Snapshot {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}
pub fn save(path: &Path, snapshot: &Snapshot) -> Result<(), String> {
    write_json(path, snapshot)
}
pub fn load_display_mode(path: &Path) -> String {
    let value = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<String>(&bytes).ok())
        .unwrap_or_default();
    normalized_mode(&value).into()
}
pub fn save_display_mode(path: &Path, mode: &str) -> Result<(), String> {
    write_json(path, &normalized_mode(mode))
}
pub fn write(
    widget: Widget,
    path: &Path,
    snapshot: &Snapshot,
    reload: impl FnOnce(&str),
) -> Result<(), String> {
    save(path, snapshot)?;
    reload(widget.kind());
    Ok(())
}
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub fill: [f64; 4],
    pub text: [f64; 4],
}
/// A plan tag's colors by its `plan_accent`, as the widget draws them.
pub fn plan_palette(accent: &str, dark: bool, monochrome: bool) -> Palette {
    if monochrome {
        return Palette {
            fill: [1., 1., 1., 0.18],
            text: [1., 1., 1., 0.98],
        };
    }
    let (fill, text) = match (accent, dark) {
        ("orange", true) => ([0.46, 0.30, 0.15, 1.], [1., 0.73, 0.28, 1.]),
        ("orange", false) => ([0.98, 0.82, 0.63, 1.], [0.78, 0.42, 0.03, 1.]),
        ("purple", true) => ([0.33, 0.20, 0.44, 1.], [0.85, 0.64, 1., 1.]),
        ("purple", false) => ([0.90, 0.80, 0.97, 1.], [0.56, 0.25, 0.72, 1.]),
        ("pink", true) => ([0.39, 0.19, 0.31, 1.], [0.98, 0.58, 0.82, 1.]),
        ("pink", false) => ([0.97, 0.78, 0.88, 1.], [0.70, 0.19, 0.44, 1.]),
        ("gray", true) => ([0.24, 0.28, 0.37, 1.], [0.84, 0.88, 0.95, 1.]),
        ("gray", false) => ([0.87, 0.89, 0.94, 1.], [0.34, 0.39, 0.50, 1.]),
        ("indigo", true) => ([0.22, 0.27, 0.46, 1.], [0.70, 0.77, 1., 1.]),
        ("indigo", false) => ([0.81, 0.85, 0.98, 1.], [0.24, 0.35, 0.69, 1.]),
        (_, true) => ([0.16, 0.35, 0.42, 1.], [0.19, 0.86, 0.93, 1.]),
        (_, false) => ([0.71, 0.90, 0.94, 1.], [0., 0.55, 0.65, 1.]),
    };
    Palette { fill, text }
}
