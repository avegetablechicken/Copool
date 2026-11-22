use crate::models::{AccountSummary, Pool};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

/// The quickest a changing usage brings a pool's rounds to.
const FASTEST_ADAPTIVE_SECONDS: u64 = 30;
const FAST_REFRESH_UNCHANGED_ROUNDS: u8 = 2;
/// The longest round interval, and the most rounds to a full one.
pub(crate) const MAX_ROUND_SECONDS: u64 = 3600;
pub(crate) const MAX_FULL_EVERY: u64 = 12;
pub(crate) const MIN_ROUND_SECONDS: u64 = 10;

/// One pool's background refresh, shared by all its cards: the card in use
/// every `seconds`, the whole pool every `full_every` rounds, and, when
/// `adaptive`, sooner rounds while its usage changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PoolRefresh {
    pub seconds: u64,
    pub full_every: u64,
    pub adaptive: bool,
}
impl PoolRefresh {
    pub(crate) fn interval(self) -> Duration {
        Duration::from_secs(self.seconds)
    }
    pub(crate) fn validate(self) -> Result<(), String> {
        if !(MIN_ROUND_SECONDS..=MAX_ROUND_SECONDS).contains(&self.seconds)
            || !(1..=MAX_FULL_EVERY).contains(&self.full_every)
        {
            return Err("Invalid refresh interval".into());
        }
        Ok(())
    }
}
/// Each pool's background refresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RefreshPreferences {
    pub codex: PoolRefresh,
    pub claude: PoolRefresh,
    pub antigravity: PoolRefresh,
}
impl Default for RefreshPreferences {
    /// Codex: its card in use every 10 seconds, the whole pool every minute.
    /// Claude and Antigravity: the whole pool every five minutes, Claude's
    /// every minute while its usage changes.
    fn default() -> Self {
        Self {
            codex: PoolRefresh {
                seconds: 10,
                full_every: 6,
                adaptive: false,
            },
            claude: PoolRefresh {
                seconds: 300,
                full_every: 1,
                adaptive: true,
            },
            antigravity: PoolRefresh {
                seconds: 300,
                full_every: 1,
                adaptive: false,
            },
        }
    }
}
impl RefreshPreferences {
    pub fn pool(&self, pool: Pool) -> PoolRefresh {
        match pool {
            Pool::Codex => self.codex,
            Pool::Claude => self.claude,
            Pool::Antigravity => self.antigravity,
        }
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        [self.codex, self.claude, self.antigravity]
            .into_iter()
            .try_for_each(PoolRefresh::validate)
    }
}

/// The next background round: when it starts (Unix seconds; past while a
/// round runs), its tick, the interval that follows it, and how many rounds
/// make one that refreshes the whole pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Schedule {
    pub(crate) at: i64,
    pub(crate) tick: u64,
    pub(crate) interval: i64,
    pub(crate) full_every: u64,
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
        while !every_round && !is_full_round(tick, self.full_every) {
            at += interval;
            tick = tick.wrapping_add(1);
        }
        at
    }
}
/// Whether the round of this tick refreshes the whole pool.
pub(crate) fn is_full_round(tick: u64, full_every: u64) -> bool {
    tick % full_every.max(1) == 0
}
/// The interval a changing usage brings: a fifth of the pool's, no sooner
/// than every 30 seconds.
pub(crate) fn fast_interval(base: Duration) -> Duration {
    (base / 5)
        .max(Duration::from_secs(FASTEST_ADAPTIVE_SECONDS))
        .min(base)
}
/// The interval after a round at `current_interval`: the fast one once
/// usage changed, back to `base` after two rounds in a row without a change.
pub(crate) fn refresh_interval_for(
    base: Duration,
    current_interval: Duration,
    data_changed: bool,
    unchanged_refreshes: &mut u8,
) -> Duration {
    let fast = fast_interval(base);
    if data_changed {
        *unchanged_refreshes = 0;
        fast
    } else if current_interval != fast {
        *unchanged_refreshes = 0;
        base
    } else {
        *unchanged_refreshes = unchanged_refreshes.saturating_add(1);
        if *unchanged_refreshes >= FAST_REFRESH_UNCHANGED_ROUNDS {
            *unchanged_refreshes = 0;
            base
        } else {
            fast
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
        None => ["fiveHour", "oneWeek", "oneMonth", "fableOneWeek", "credits"]
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
            [&u.five_hour, &u.one_week, &u.one_month, &u.fable_one_week]
                .into_iter()
                .flatten()
                .filter_map(|w| {
                    w.reset_at
                        .map(|reset| reset.saturating_sub(w.clock_offset_seconds.unwrap_or(0)))
                })
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
pub(crate) fn recurring_ids(
    tick: u64,
    full_every: u64,
    accounts: &[AccountSummary],
) -> Option<Vec<String>> {
    if is_full_round(tick, full_every) {
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
    fn rounds_schedule_the_current_card_and_the_whole_pool() {
        let defaults = RefreshPreferences::default();
        for (pool, seconds, full_seconds) in [
            (Pool::Codex, 10, 60),
            (Pool::Claude, 300, 300),
            (Pool::Antigravity, 300, 300),
        ] {
            let refresh = defaults.pool(pool);
            assert_eq!(refresh.seconds, seconds);
            let schedule = Schedule {
                at: 1_000 + seconds as i64,
                tick: 1,
                interval: seconds as i64,
                full_every: refresh.full_every,
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
            full_every: 6,
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
        // Every third round refreshes the whole pool.
        let thirds = Schedule {
            full_every: 3,
            ..schedule
        };
        assert_eq!(thirds.next_refresh_at(false, 0), 1_300);
    }

    #[test]
    fn a_changing_usage_speeds_rounds_up_to_a_fifth_no_sooner_than_half_a_minute() {
        let secs = Duration::from_secs;
        assert_eq!(fast_interval(secs(300)), secs(60));
        assert_eq!(fast_interval(secs(600)), secs(120));
        assert_eq!(fast_interval(secs(60)), secs(30));
        // Never slower than the interval itself.
        assert_eq!(fast_interval(secs(10)), secs(10));
        let mut unchanged = 0;
        let ten = secs(600);
        assert_eq!(
            refresh_interval_for(ten, ten, true, &mut unchanged),
            secs(120)
        );
        assert_eq!(
            refresh_interval_for(ten, secs(120), false, &mut unchanged),
            secs(120)
        );
        assert_eq!(
            refresh_interval_for(ten, secs(120), false, &mut unchanged),
            ten
        );
    }

    #[test]
    fn refresh_preferences_are_limited() {
        let mut refresh = RefreshPreferences::default();
        assert!(refresh.validate().is_ok());
        refresh.claude.seconds = 5;
        assert!(refresh.validate().is_err());
        refresh.claude.seconds = 3600;
        refresh.codex.full_every = 0;
        assert!(refresh.validate().is_err());
        refresh.codex.full_every = 12;
        assert!(refresh.validate().is_ok());
        refresh.codex.full_every = 13;
        assert!(refresh.validate().is_err());
    }
}
