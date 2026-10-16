use crate::models::AccountSummary;
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

pub(crate) const CODEX_REFRESH_INTERVAL: Duration = Duration::from_secs(10);
pub(crate) const CLAUDE_REFRESH_INTERVAL: Duration = Duration::from_secs(300);
pub(crate) const FAST_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const FAST_REFRESH_UNCHANGED_ROUNDS: u8 = 2;
/// Background rounds from one that refreshes the whole pool to the next.
const FULL_ROUND: u64 = 6;
/// The next background round: when it starts (Unix seconds; past while a
/// round runs), its tick, and the interval that follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Schedule {
    pub(crate) at: i64,
    pub(crate) tick: u64,
    pub(crate) interval: i64,
}
impl Schedule {
    /// When a round next asks for a card, should the interval stay as it
    /// is: the next round, or for `every_round` false the next that refreshes
    /// the whole pool; none starting before `not_before`.
    pub(crate) fn next_refresh_at(&self, every_round: bool, not_before: i64) -> i64 {
        let interval = self.interval.max(1);
        let skipped = ((not_before - self.at).max(0) + interval - 1) / interval;
        let (mut at, mut tick) = (
            self.at + skipped * interval,
            self.tick.wrapping_add(skipped as u64),
        );
        while !every_round && tick % FULL_ROUND != 0 {
            at += interval;
            tick = tick.wrapping_add(1);
        }
        at
    }
}
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
        CLAUDE_REFRESH_INTERVAL
    } else {
        *unchanged_refreshes = unchanged_refreshes.saturating_add(1);
        if *unchanged_refreshes >= FAST_REFRESH_UNCHANGED_ROUNDS {
            *unchanged_refreshes = 0;
            CLAUDE_REFRESH_INTERVAL
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
    if tick % FULL_ROUND == 0 {
        None
    } else {
        Some(
            round_card(accounts)
                .map(|id| vec![id.into()])
                .unwrap_or_default(),
        )
    }
}
/// The card every round asks for, the others waiting for a full round.
pub(crate) fn round_card(accounts: &[AccountSummary]) -> Option<&str> {
    accounts
        .iter()
        .find(|a| a.is_current)
        .map(|a| a.id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_intervals_schedule_current_and_idle_cards() {
        for (interval, seconds, full_seconds) in [
            (CODEX_REFRESH_INTERVAL, 10, 60),
            (CLAUDE_REFRESH_INTERVAL, 300, 1800),
            (FAST_REFRESH_INTERVAL, 60, 360),
        ] {
            assert_eq!(interval.as_secs(), seconds);
            let schedule = Schedule {
                at: 1_000 + seconds as i64,
                tick: 1,
                interval: seconds as i64,
            };
            assert_eq!(schedule.next_refresh_at(true, 0), 1_000 + seconds as i64);
            assert_eq!(schedule.next_refresh_at(false, 0), 1_000 + full_seconds);
        }
    }

    #[test]
    fn the_next_refresh_is_the_next_round_or_the_next_full_one() {
        let schedule = Schedule {
            at: 1_000,
            tick: 2,
            interval: 300,
        };
        assert_eq!(schedule.next_refresh_at(true, 0), 1_000);
        // Ticks 2 to 5 refresh only the current card; tick 6 the whole pool.
        assert_eq!(schedule.next_refresh_at(false, 0), 2_200);
        // A card not due yet waits for the first round from then on.
        assert_eq!(schedule.next_refresh_at(true, 1_000), 1_000);
        assert_eq!(schedule.next_refresh_at(true, 1_001), 1_300);
        assert_eq!(schedule.next_refresh_at(false, 2_201), 4_000);
        let full = Schedule {
            tick: 6,
            ..schedule
        };
        assert_eq!(full.next_refresh_at(false, 0), 1_000);
    }
}
