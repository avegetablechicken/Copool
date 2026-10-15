//! Portable widget snapshot format. WidgetKit presentation/reload is a host adapter.
use crate::{
    models::{AccountSummary, UsageWindow},
    persistence::write_json,
    ranking::{remaining_score, sort_for_display},
};
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// An account pool; its cards follow the app's tags for that pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pool {
    Codex,
    Claude,
}
impl Pool {
    /// The tag naming the pool on a card of the widget showing both.
    fn label(self) -> &'static str {
        match self {
            Self::Codex => "CODEX",
            Self::Claude => "CLAUDE",
        }
    }
}
/// A widget the app publishes; each has its own WidgetKit kind and snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Widget {
    Codex,
    Claude,
    /// Both pools.
    All,
}
impl Widget {
    pub const ALL: [Self; 3] = [Self::Codex, Self::Claude, Self::All];
    pub fn kind(self) -> &'static str {
        match self {
            Self::Codex => "CopoolCodexAccountsWidget",
            Self::Claude => "CopoolClaudeAccountsWidget",
            Self::All => "CopoolAccountsWidget",
        }
    }
    /// The snapshot's name in the data directory and the app group container.
    pub fn filename(self) -> &'static str {
        match self {
            Self::Codex => "codex-accounts-widget-snapshot.json",
            Self::Claude => "claude-accounts-widget-snapshot.json",
            Self::All => "accounts-widget-snapshot.json",
        }
    }
    pub fn build(
        self,
        codex: &[AccountSummary],
        claude: &[AccountSummary],
        mode: &str,
        offset_seconds: Option<i32>,
        now: i64,
    ) -> Snapshot {
        match self {
            Self::Codex => build(Pool::Codex, codex, mode, offset_seconds, now),
            Self::Claude => build(Pool::Claude, claude, mode, offset_seconds, now),
            Self::All => build_all(codex, claude, mode, offset_seconds, now),
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
    /// The card's pool, on the widget showing both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_label: Option<String>,
    pub plan_label: String,
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
    WindowSnapshot {
        title: title.into(),
        available: true,
        progress_fraction: used / 100.,
        used_text: format!("{}%", used.round()),
        remaining_text: format!("{}%", 100. - used.round()),
        reset_text: reset,
    }
}
/// Codex treats an unknown plan as Team.
fn codex_plan(raw: Option<String>) -> String {
    let raw = raw.unwrap_or_else(|| "TEAM".into());
    if ["PRO", "PLUS", "FREE", "TEAM", "BUSINESS", "ENTERPRISE"].contains(&raw.as_str()) {
        raw
    } else {
        "TEAM".into()
    }
}
/// As the app's plan tag: Claude shows none until its plan is known, and Max
/// plans keep their usage multiplier ("MAX 20X").
fn claude_plan(raw: Option<String>) -> String {
    let Some(raw) = raw else {
        return String::new();
    };
    let max = raw.strip_prefix("MAX").is_some_and(|rest| {
        rest.is_empty()
            || rest
                .strip_prefix(' ')
                .and_then(|r| r.strip_suffix('X'))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    });
    if max || ["PRO", "PLUS", "FREE", "BUSINESS", "ENTERPRISE"].contains(&raw.as_str()) {
        raw
    } else {
        "TEAM".into()
    }
}
fn card(pool: Pool, account: &AccountSummary, offset: Option<i32>) -> CardSnapshot {
    let raw = account
        .usage
        .as_ref()
        .and_then(|u| u.plan_type.as_ref())
        .filter(|s| !s.trim().is_empty())
        .or(account.plan_type.as_ref().filter(|s| !s.trim().is_empty()))
        .map(|s| s.trim().to_uppercase());
    let (plan, workspace, name) = match pool {
        Pool::Codex => (
            codex_plan(raw),
            account.team_name.clone(),
            account.email.as_deref().unwrap_or(&account.account_id),
        ),
        Pool::Claude => {
            let plan = claude_plan(raw);
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
    };
    let label = name.trim().split('@').next().unwrap_or_default().to_owned();
    CardSnapshot {
        id: account.id.clone(),
        pool_label: None,
        plan_label: plan,
        workspace_label: workspace,
        account_label: label,
        five_hour: window(
            "5h",
            account.usage.as_ref().and_then(|u| u.five_hour.as_ref()),
            offset,
        ),
        one_week: window(
            "1w",
            account.usage.as_ref().and_then(|u| u.one_week.as_ref()),
            offset,
        ),
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
        .map(|a| card(pool, a, offset_seconds))
        .collect();
    snapshot(cards, mode, now)
}
/// Both pools: each pool's first card (its current account) leads, Codex's
/// before Claude's, then the other accounts by remaining quota.
pub fn build_all(
    codex: &[AccountSummary],
    claude: &[AccountSummary],
    mode: &str,
    offset_seconds: Option<i32>,
    now: i64,
) -> Snapshot {
    let mut leading = vec![];
    let mut others = vec![];
    for (pool, accounts) in [(Pool::Codex, codex), (Pool::Claude, claude)] {
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
        .map(|(pool, a)| CardSnapshot {
            pool_label: Some(pool.label().into()),
            ..card(pool, &a, offset_seconds)
        })
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
pub fn plan_palette(plan: &str, dark: bool, monochrome: bool) -> Palette {
    if monochrome {
        return Palette {
            fill: [1., 1., 1., 0.18],
            text: [1., 1., 1., 0.98],
        };
    }
    let (fill, text) = match (plan, dark) {
        ("PRO", true) => ([0.46, 0.30, 0.15, 1.], [1., 0.73, 0.28, 1.]),
        ("PRO", false) => ([0.98, 0.82, 0.63, 1.], [0.78, 0.42, 0.03, 1.]),
        ("PLUS", true) => ([0.39, 0.19, 0.31, 1.], [0.98, 0.58, 0.82, 1.]),
        ("PLUS", false) => ([0.97, 0.78, 0.88, 1.], [0.70, 0.19, 0.44, 1.]),
        ("FREE", true) => ([0.24, 0.28, 0.37, 1.], [0.84, 0.88, 0.95, 1.]),
        ("FREE", false) => ([0.87, 0.89, 0.94, 1.], [0.34, 0.39, 0.50, 1.]),
        (max, true) if max.starts_with("MAX") => ([0.42, 0.24, 0.19, 1.], [1., 0.64, 0.50, 1.]),
        (max, false) if max.starts_with("MAX") => ([0.97, 0.84, 0.78, 1.], [0.76, 0.36, 0.22, 1.]),
        ("BUSINESS" | "ENTERPRISE", true) => ([0.22, 0.27, 0.46, 1.], [0.70, 0.77, 1., 1.]),
        ("BUSINESS" | "ENTERPRISE", false) => ([0.81, 0.85, 0.98, 1.], [0.24, 0.35, 0.69, 1.]),
        (_, true) => ([0.16, 0.35, 0.42, 1.], [0.19, 0.86, 0.93, 1.]),
        (_, false) => ([0.71, 0.90, 0.94, 1.], [0., 0.55, 0.65, 1.]),
    };
    Palette { fill, text }
}
