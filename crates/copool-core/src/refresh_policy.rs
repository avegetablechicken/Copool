use crate::models::AccountSummary;
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

/// OpenQuota's adaptive update frequency: five minutes between background
/// refreshes, one minute after a refresh that changed usage, and five again
/// after two one-minute rounds without a change.
pub(crate) const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub(crate) const FAST_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const FAST_REFRESH_UNCHANGED_ROUNDS: u8 = 2;
pub(crate) fn refresh_interval_for(
    current_interval: Duration,
    data_changed: bool,
    unchanged_refreshes: &mut u8,
) -> Duration {
    if data_changed {
        *unchanged_refreshes = 0;
        FAST_REFRESH_INTERVAL
    } else if current_interval != FAST_REFRESH_INTERVAL {
        *unchanged_refreshes = 0;
        REFRESH_INTERVAL
    } else {
        *unchanged_refreshes = unchanged_refreshes.saturating_add(1);
        if *unchanged_refreshes >= FAST_REFRESH_UNCHANGED_ROUNDS {
            *unchanged_refreshes = 0;
            REFRESH_INTERVAL
        } else {
            FAST_REFRESH_INTERVAL
        }
    }
}
/// Each card's usage, without when it was fetched.
pub(crate) fn usage_values(accounts: &[AccountSummary]) -> BTreeMap<String, Value> {
    accounts
        .iter()
        .filter_map(|a| {
            let mut usage = serde_json::to_value(a.usage.as_ref()?).ok()?;
            usage.as_object_mut()?.remove("fetchedAt");
            Some((a.id.clone(), usage))
        })
        .collect()
}
/// Usage changed: a card's values differ, or a card has usage for the first
/// time. A card losing its usage is no change.
pub(crate) fn usage_changed(
    previous: &BTreeMap<String, Value>,
    current: &BTreeMap<String, Value>,
) -> bool {
    current.iter().any(|(id, usage)| match previous.get(id) {
        Some(previous) => previous != usage,
        None => ["fiveHour", "oneWeek", "credits"]
            .iter()
            .any(|key| !usage[*key].is_null()),
    })
}

pub(crate) fn snapshot_is_fresh(synced_at: Option<i64>, now: i64, window: i64) -> bool {
    synced_at.is_some_and(|stamp| now.saturating_sub(stamp) <= window)
}
pub(crate) fn should_refresh(force: bool, synced_at: Option<i64>, now: i64, window: i64) -> bool {
    force || !snapshot_is_fresh(synced_at, now, window)
}
pub(crate) fn target_ids(accounts: &[AccountSummary], now: i64, lead: i64) -> Vec<String> {
    let mut ids = Vec::new();
    for account in accounts
        .iter()
        .filter(|a| a.is_current)
        .take(1)
        .chain(accounts.iter().filter(|a| !a.is_current))
    {
        let near_reset = account.usage.as_ref().is_some_and(|u| {
            [&u.five_hour, &u.one_week]
                .into_iter()
                .flatten()
                .filter_map(|w| w.reset_at)
                .any(|reset| reset.abs_diff(now) <= lead.max(0) as u64)
        });
        let failed = account
            .usage_error
            .as_ref()
            .is_some_and(|e| !e.trim().is_empty());
        if (account.is_current || near_reset || failed) && !ids.contains(&account.id) {
            ids.push(account.id.clone());
        }
    }
    ids
}
/// None means the complete pool; intermediate ticks only refresh the active card.
pub(crate) fn recurring_ids(tick: u64, accounts: &[AccountSummary]) -> Option<Vec<String>> {
    if tick % 6 == 0 {
        None
    } else {
        Some(
            accounts
                .iter()
                .find(|a| a.is_current)
                .map(|a| vec![a.id.clone()])
                .unwrap_or_default(),
        )
    }
}
