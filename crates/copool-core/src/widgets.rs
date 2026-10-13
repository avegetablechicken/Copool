//! Portable widget snapshot format. WidgetKit presentation/reload is a host adapter.
use crate::{
    models::{AccountSummary, UsageWindow},
    persistence::write_json,
    ranking::sort_for_display,
};
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const KIND: &str = "CopoolAccountsWidget";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowSnapshot {
    pub title: String,
    pub progress_fraction: f64,
    pub used_text: String,
    pub remaining_text: String,
    pub reset_text: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardSnapshot {
    pub id: String,
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
fn window(title: &str, window: Option<&UsageWindow>, offset: Option<i32>) -> WindowSnapshot {
    let used = window
        .map(|w| w.used_percent)
        .unwrap_or(100.)
        .clamp(0., 100.);
    let reset = window
        .and_then(|w| w.reset_at)
        .and_then(|stamp| DateTime::from_timestamp(stamp, 0))
        .map(|date| {
            date.with_timezone(&zone_offset(offset, date))
                .format("%y/%-m/%-d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "--".into());
    WindowSnapshot {
        title: title.into(),
        progress_fraction: used / 100.,
        used_text: format!("{}%", used.round()),
        remaining_text: format!("{}%", 100. - used.round()),
        reset_text: reset,
    }
}
fn card(account: &AccountSummary, offset: Option<i32>) -> CardSnapshot {
    let raw = account
        .usage
        .as_ref()
        .and_then(|u| u.plan_type.as_ref())
        .filter(|s| !s.trim().is_empty())
        .or(account.plan_type.as_ref())
        .map(|s| s.trim().to_uppercase())
        .unwrap_or_else(|| "TEAM".into());
    let plan = if ["PRO", "PLUS", "FREE", "TEAM", "BUSINESS", "ENTERPRISE"].contains(&raw.as_str())
    {
        raw
    } else {
        "TEAM".into()
    };
    let label = account
        .email
        .as_deref()
        .unwrap_or(&account.account_id)
        .trim()
        .split('@')
        .next()
        .unwrap_or_default()
        .to_owned();
    CardSnapshot {
        id: account.id.clone(),
        plan_label: plan,
        workspace_label: account.team_name.clone(),
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
pub fn build(
    accounts: &[AccountSummary],
    mode: &str,
    offset_seconds: Option<i32>,
    now: i64,
) -> Snapshot {
    let mut accounts = accounts.to_vec();
    sort_for_display(&mut accounts);
    if accounts.is_empty() {
        return Snapshot::default();
    }
    Snapshot {
        generated_at: now,
        usage_progress_display_mode: normalized_mode(mode).into(),
        current_card: Some(card(&accounts[0], offset_seconds)),
        secondary_card: accounts.get(1).map(|a| card(a, offset_seconds)),
        rows: accounts
            .iter()
            .skip(1)
            .take(4)
            .map(|a| card(a, offset_seconds))
            .collect(),
    }
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
    path: &Path,
    accounts: &[AccountSummary],
    mode: &str,
    offset_seconds: Option<i32>,
    now: i64,
    reload: impl FnOnce(&str),
) -> Result<(), String> {
    let snapshot = build(accounts, mode, offset_seconds, now);
    save(path, &snapshot)?;
    reload(KIND);
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
        ("BUSINESS" | "ENTERPRISE", true) => ([0.22, 0.27, 0.46, 1.], [0.70, 0.77, 1., 1.]),
        ("BUSINESS" | "ENTERPRISE", false) => ([0.81, 0.85, 0.98, 1.], [0.24, 0.35, 0.69, 1.]),
        (_, true) => ([0.16, 0.35, 0.42, 1.], [0.19, 0.86, 0.93, 1.]),
        (_, false) => ([0.71, 0.90, 0.94, 1.], [0., 0.55, 0.65, 1.]),
    };
    Palette { fill, text }
}
