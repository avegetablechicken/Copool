use crate::models::{AccountSummary, Pool};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

/// The quickest a changing usage brings a card's refreshes to.
const FASTEST_ADAPTIVE_SECONDS: u64 = 30;
/// Refreshes in a row without a change that end a card's speed-up.
pub(crate) const FAST_REFRESH_UNCHANGED_ROUNDS: u8 = 2;
/// Small reset timestamp shifts (sub-second rounding or server clock jitter)
/// that do not count as a usage change when all other window fields match.
const RESET_AT_JITTER_SECONDS: u64 = 5;
const WINDOW_KEYS: [&str; 6] = [
    "fiveHour",
    "oneWeek",
    "oneMonth",
    "fableOneWeek",
    "thirdPartyFiveHour",
    "thirdPartyOneWeek",
];
/// The longest round interval, and the most rounds to a full one.
pub(crate) const MAX_ROUND_SECONDS: u64 = 3600;
pub(crate) const MAX_FULL_EVERY: u64 = 12;
pub(crate) const MIN_ROUND_SECONDS: u64 = 10;

/// One pool's background refresh: the card in use every `seconds`, the whole
/// pool every `full_every` rounds, shared by all its cards. `adaptive` and
/// `backoff` are those of cards without their own (`CardRefresh`): with
/// `adaptive`, a card refreshes sooner while its usage changes; with
/// `backoff`, an account answered with HTTP 429 pauses its requests for a
/// while, which only Claude's accounts do for now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PoolRefresh {
    pub seconds: u64,
    pub full_every: u64,
    pub adaptive: bool,
    pub backoff: bool,
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
/// A card's own speed-up and slow-down; one it has not set follows its
/// pool's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CardRefresh {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backoff: Option<bool>,
}
/// Each pool's cards' own refresh switches, by card key: an account's id, or
/// `sub2api:<provider>` for all of a Sub2api provider's cards.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CardRefreshes {
    pub codex: BTreeMap<String, CardRefresh>,
    pub claude: BTreeMap<String, CardRefresh>,
    pub antigravity: BTreeMap<String, CardRefresh>,
    pub grok: BTreeMap<String, CardRefresh>,
}
impl CardRefreshes {
    pub fn pool(&self, pool: Pool) -> &BTreeMap<String, CardRefresh> {
        match pool {
            Pool::Codex => &self.codex,
            Pool::Claude => &self.claude,
            Pool::Antigravity => &self.antigravity,
            Pool::Grok => &self.grok,
        }
    }
    fn pool_mut(&mut self, pool: Pool) -> &mut BTreeMap<String, CardRefresh> {
        match pool {
            Pool::Codex => &mut self.codex,
            Pool::Claude => &mut self.claude,
            Pool::Antigravity => &mut self.antigravity,
            Pool::Grok => &mut self.grok,
        }
    }
}
/// Each pool's background refresh, and its cards' own switches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RefreshPreferences {
    pub codex: PoolRefresh,
    pub claude: PoolRefresh,
    pub antigravity: PoolRefresh,
    pub grok: PoolRefresh,
    pub cards: CardRefreshes,
}
impl Default for RefreshPreferences {
    /// Codex: its card in use every 10 seconds, the whole pool every minute.
    /// Claude, Antigravity and Grok: the whole pool every five minutes, Claude's
    /// every minute while its usage changes. Each slows down on HTTP 429.
    fn default() -> Self {
        Self {
            codex: PoolRefresh {
                seconds: 10,
                full_every: 6,
                adaptive: false,
                backoff: true,
            },
            claude: PoolRefresh {
                seconds: 300,
                full_every: 1,
                adaptive: true,
                backoff: true,
            },
            antigravity: PoolRefresh {
                seconds: 300,
                full_every: 1,
                adaptive: false,
                backoff: true,
            },
            grok: PoolRefresh {
                seconds: 300,
                full_every: 1,
                adaptive: false,
                backoff: true,
            },
            cards: CardRefreshes::default(),
        }
    }
}
impl RefreshPreferences {
    pub fn pool(&self, pool: Pool) -> PoolRefresh {
        match pool {
            Pool::Codex => self.codex,
            Pool::Claude => self.claude,
            Pool::Antigravity => self.antigravity,
            Pool::Grok => self.grok,
        }
    }
    /// A card's refresh: its pool's, with the card's own switches.
    pub fn card(&self, pool: Pool, key: &str) -> PoolRefresh {
        let mut refresh = self.pool(pool);
        if let Some(card) = self.cards.pool(pool).get(key) {
            refresh.adaptive = card.adaptive.unwrap_or(refresh.adaptive);
            refresh.backoff = card.backoff.unwrap_or(refresh.backoff);
        }
        refresh
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        [self.codex, self.claude, self.antigravity, self.grok]
            .into_iter()
            .try_for_each(PoolRefresh::validate)
    }
    /// The saved preferences, never failing: a pool's missing settings take
    /// its defaults and ones this build does not know are ignored; a pool
    /// whose settings cannot be read falls back to its defaults.
    pub(crate) fn from_saved(saved: &Value) -> Self {
        let defaults = Self::default();
        let read = |name: &str, default: PoolRefresh| {
            let Some(saved) = saved.get(name) else {
                return default;
            };
            let mut value = serde_json::to_value(default).unwrap_or_default();
            if let (Some(fields), Some(saved)) = (value.as_object_mut(), saved.as_object()) {
                for (key, field) in fields.iter_mut() {
                    if let Some(saved) = saved.get(key) {
                        *field = saved.clone();
                    }
                }
            }
            serde_json::from_value::<PoolRefresh>(value)
                .ok()
                .filter(|refresh| refresh.validate().is_ok())
                .unwrap_or_else(|| {
                    log::warn!("Unreadable {name} refresh preference; using its defaults");
                    default
                })
        };
        // A card's switches that cannot be read follow its pool's.
        let mut cards = CardRefreshes::default();
        for pool in Pool::ALL {
            let Some(saved) = saved["cards"][pool_name(pool)].as_object() else {
                continue;
            };
            cards
                .pool_mut(pool)
                .extend(saved.iter().filter_map(|(key, card)| {
                    let card = serde_json::from_value::<CardRefresh>(card.clone()).ok()?;
                    (card != CardRefresh::default()).then(|| (key.clone(), card))
                }));
        }
        Self {
            codex: read("codex", defaults.codex),
            claude: read("claude", defaults.claude),
            antigravity: read("antigravity", defaults.antigravity),
            grok: read("grok", defaults.grok),
            cards,
        }
    }
}
fn pool_name(pool: Pool) -> &'static str {
    match pool {
        Pool::Codex => "codex",
        Pool::Claude => "claude",
        Pool::Antigravity => "antigravity",
        Pool::Grok => "grok",
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
/// A card's speed-up after a refresh: a change starts it over; a refresh
/// of a card speeding up without one (`counted`) adds to its unchanged
/// refreshes, and enough of them in a row end it. A card without `adaptive`
/// never speeds up.
pub(crate) fn track_speed_up(
    fast: &mut BTreeMap<String, u8>,
    key: &str,
    adaptive: bool,
    changed: bool,
    counted: bool,
) {
    if !adaptive {
        fast.remove(key);
    } else if changed {
        fast.insert(key.to_owned(), 0);
    } else if counted {
        if let Some(unchanged) = fast.get_mut(key) {
            *unchanged = unchanged.saturating_add(1);
            if *unchanged >= FAST_REFRESH_UNCHANGED_ROUNDS {
                fast.remove(key);
            }
        }
    }
}
/// The key of the card an account shows on: its id, or `sub2api:<provider>`
/// for a Sub2api provider's, which refresh together.
pub(crate) fn card_key(pool: Pool, id: &str) -> String {
    match crate::sub2api::card_provider(pool, id) {
        Some(provider) => format!("sub2api:{provider}"),
        None => id.to_owned(),
    }
}
/// Each card's usage, without fetch/clock metadata that varies across polls
/// even when usage has not changed.
pub(crate) fn usage_values(accounts: &[AccountSummary]) -> BTreeMap<String, Value> {
    accounts
        .iter()
        .filter_map(|a| {
            let mut usage = serde_json::to_value(a.usage.as_ref()?).ok()?;
            strip_usage_metadata(&mut usage);
            Some((a.id.clone(), usage))
        })
        .collect()
}
fn strip_usage_metadata(usage: &mut Value) {
    let Some(object) = usage.as_object_mut() else {
        return;
    };
    object.remove("fetchedAt");
    object.remove("resetsLastConfirmedAt");
    for key in WINDOW_KEYS {
        if let Some(window) = object.get_mut(key).and_then(Value::as_object_mut) {
            window.remove("clockOffsetSeconds");
        }
    }
}
/// The cards whose usage changed: their values differ, or they have usage
/// for the first time. A card losing its usage is no change.
pub(crate) fn changed_cards<'a>(
    previous: &'a BTreeMap<String, Value>,
    current: &'a BTreeMap<String, Value>,
) -> impl Iterator<Item = &'a str> {
    current
        .iter()
        .filter(|(id, usage)| usage_changed(previous.get(*id), usage))
        .map(|(id, _)| id.as_str())
}
fn usage_changed(previous: Option<&Value>, usage: &Value) -> bool {
    match previous {
        Some(previous) => !same_usage(previous, usage),
        None => WINDOW_KEYS
            .iter()
            .chain(std::iter::once(&"credits"))
            .any(|key| !usage[*key].is_null()),
    }
}
fn same_usage(previous: &Value, current: &Value) -> bool {
    let (Some(a), Some(b)) = (previous.as_object(), current.as_object()) else {
        return previous == current;
    };
    let ignored = |key: &str| matches!(key, "fetchedAt" | "resetsLastConfirmedAt");
    let keys: std::collections::BTreeSet<&str> = a
        .keys()
        .chain(b.keys())
        .map(String::as_str)
        .filter(|key| !ignored(key))
        .collect();
    keys.into_iter().all(|key| {
        let av = a.get(key).unwrap_or(&Value::Null);
        let bv = b.get(key).unwrap_or(&Value::Null);
        if WINDOW_KEYS.contains(&key) {
            same_window(av, bv)
        } else {
            av == bv
        }
    })
}
fn same_window(previous: &Value, current: &Value) -> bool {
    let (Some(a), Some(b)) = (previous.as_object(), current.as_object()) else {
        return previous == current;
    };
    let keys: std::collections::BTreeSet<&str> = a
        .keys()
        .chain(b.keys())
        .map(String::as_str)
        .filter(|key| *key != "clockOffsetSeconds")
        .collect();
    keys.into_iter().all(|key| {
        let av = a.get(key).unwrap_or(&Value::Null);
        let bv = b.get(key).unwrap_or(&Value::Null);
        if key == "resetAt" {
            match (av.as_i64(), bv.as_i64()) {
                (Some(x), Some(y)) => x.abs_diff(y) <= RESET_AT_JITTER_SECONDS,
                _ => av == bv,
            }
        } else {
            av == bv
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cards_first_usage_of_any_window_is_a_change() {
        let window = serde_json::json!({"usedPercent": 1, "windowSeconds": 18000});
        for key in ["fiveHour", "thirdPartyFiveHour", "thirdPartyOneWeek"] {
            let current = BTreeMap::from([("a".to_owned(), serde_json::json!({ key: window }))]);
            assert!(changed_cards(&BTreeMap::new(), &current).eq(["a"]), "{key}");
        }
        let empty = BTreeMap::from([("a".to_owned(), serde_json::json!({}))]);
        assert_eq!(changed_cards(&BTreeMap::new(), &empty).count(), 0);
        // Only the card whose usage differs; one losing its usage is none.
        let before = BTreeMap::from([
            ("a".to_owned(), serde_json::json!({"fiveHour": window})),
            ("b".to_owned(), serde_json::json!({"fiveHour": window})),
            ("c".to_owned(), serde_json::json!({"fiveHour": window})),
        ]);
        let mut after = before.clone();
        after.insert(
            "b".into(),
            serde_json::json!({"fiveHour": {"usedPercent": 2}}),
        );
        after.remove("c");
        assert!(changed_cards(&before, &after).eq(["b"]));
    }

    #[test]
    fn clock_offset_reset_jitter_and_confirmation_time_are_not_usage_changes() {
        let base = serde_json::json!({
            "fetchedAt": 1000,
            "remainingResetCount": 1,
            "fiveHour": {
                "clockOffsetSeconds": 0,
                "usedPercent": 0.0,
                "windowSeconds": 18000,
                "resetAt": null
            },
            "oneWeek": {
                "clockOffsetSeconds": 0,
                "usedPercent": 100.0,
                "windowSeconds": 604800,
                "resetAt": 1_795_899_600
            }
        });
        let noisy = serde_json::json!({
            "fetchedAt": 1030,
            "resetsLastConfirmedAt": 1000,
            "remainingResetCount": 1,
            "fiveHour": {
                "clockOffsetSeconds": -1,
                "usedPercent": 0.0,
                "windowSeconds": 18000,
                "resetAt": null
            },
            "oneWeek": {
                "clockOffsetSeconds": -1,
                "usedPercent": 100.0,
                "windowSeconds": 604800,
                "resetAt": 1_795_899_599
            }
        });
        let before = BTreeMap::from([("a".to_owned(), base.clone())]);
        let after = BTreeMap::from([("a".to_owned(), noisy)]);
        assert_eq!(changed_cards(&before, &after).count(), 0);

        // Starting an idle 5h window (resetAt None -> Some) is a real change.
        let mut started = base.clone();
        started["fiveHour"]["resetAt"] = serde_json::json!(1_795_900_000);
        let after = BTreeMap::from([("a".to_owned(), started)]);
        assert!(changed_cards(&before, &after).eq(["a"]));

        // A reset jump beyond the jitter tolerance is a real change even with the same percent.
        let mut rolled = base.clone();
        rolled["oneWeek"]["resetAt"] = serde_json::json!(1_795_899_600 + 6);
        let after = BTreeMap::from([("a".to_owned(), rolled)]);
        assert!(changed_cards(&before, &after).eq(["a"]));

        // Spending a banked reset is a real change.
        let mut spent = base;
        spent["remainingResetCount"] = serde_json::json!(0);
        let after = BTreeMap::from([("a".to_owned(), spent)]);
        assert!(changed_cards(&before, &after).eq(["a"]));
    }

    #[test]
    fn rounds_schedule_the_current_card_and_the_whole_pool() {
        let defaults = RefreshPreferences::default();
        for (pool, seconds, full_seconds) in [
            (Pool::Codex, 10, 60),
            (Pool::Claude, 300, 300),
            (Pool::Antigravity, 300, 300),
            (Pool::Grok, 300, 300),
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
    }

    #[test]
    fn a_card_speeds_up_on_a_change_until_two_refreshes_pass_unchanged() {
        let mut fast = BTreeMap::new();
        // Unchanged, a card not speeding up stays so.
        track_speed_up(&mut fast, "a", true, false, true);
        assert!(fast.is_empty());
        // Without its own speed-up, a change is ignored.
        track_speed_up(&mut fast, "b", false, true, true);
        assert!(fast.is_empty());
        track_speed_up(&mut fast, "a", true, true, false);
        assert_eq!(fast.get("a"), Some(&0));
        // A round that did not ask for it counts nothing...
        track_speed_up(&mut fast, "a", true, false, false);
        assert_eq!(fast.get("a"), Some(&0));
        // ...a change starts it over...
        track_speed_up(&mut fast, "a", true, false, true);
        track_speed_up(&mut fast, "a", true, true, true);
        assert_eq!(fast.get("a"), Some(&0));
        // ...and the second unchanged refresh in a row ends it.
        track_speed_up(&mut fast, "a", true, false, true);
        assert_eq!(fast.get("a"), Some(&1));
        track_speed_up(&mut fast, "a", true, false, true);
        assert!(fast.is_empty());
        // Turning the card's speed-up off ends it at once.
        track_speed_up(&mut fast, "a", true, true, true);
        track_speed_up(&mut fast, "a", false, false, false);
        assert!(fast.is_empty());
    }

    #[test]
    fn a_card_follows_its_pools_switches_but_those_it_set() {
        let mut refresh = RefreshPreferences::default();
        refresh.cards.claude.insert(
            "a".into(),
            CardRefresh {
                adaptive: Some(false),
                backoff: None,
            },
        );
        refresh.cards.claude.insert(
            "sub2api:p".into(),
            CardRefresh {
                adaptive: None,
                backoff: Some(false),
            },
        );
        let card = refresh.card(Pool::Claude, "a");
        assert_eq!((card.adaptive, card.backoff), (false, true));
        assert_eq!(card.seconds, refresh.claude.seconds);
        let group = refresh.card(Pool::Claude, "sub2api:p");
        assert_eq!((group.adaptive, group.backoff), (true, false));
        assert_eq!(refresh.card(Pool::Claude, "other"), refresh.claude);
        // Another pool's card of the same key keeps its own pool's.
        assert_eq!(refresh.card(Pool::Codex, "a"), refresh.codex);
        assert_eq!(card_key(Pool::Claude, "claude-sub2api-p-3"), "sub2api:p");
        assert_eq!(card_key(Pool::Codex, "sub2api-p-3"), "sub2api:p");
        assert_eq!(card_key(Pool::Claude, "a"), "a");
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

    #[test]
    fn saved_refresh_preferences_take_defaults_for_what_they_lack_or_cannot_read() {
        let defaults = RefreshPreferences::default();
        // Saved before the slow-down: kept, with the slow-down on.
        let saved = serde_json::json!({
            "codex": {"seconds": 30, "fullEvery": 2, "adaptive": true},
            "claude": {"seconds": 600, "fullEvery": 1, "adaptive": false},
        });
        let read = RefreshPreferences::from_saved(&saved);
        assert_eq!(
            read.codex,
            PoolRefresh {
                seconds: 30,
                full_every: 2,
                adaptive: true,
                backoff: true
            }
        );
        assert_eq!((read.claude.seconds, read.claude.adaptive), (600, false));
        assert!(read.claude.backoff);
        assert_eq!(read.antigravity, defaults.antigravity);
        // A setting this build does not know is ignored; a pool it cannot
        // read, or out of bounds, takes its defaults alone.
        let saved = serde_json::json!({
            "codex": {"seconds": 30, "later": 1},
            "claude": {"seconds": "soon"},
            "antigravity": {"seconds": 5},
        });
        let read = RefreshPreferences::from_saved(&saved);
        assert_eq!(read.codex.seconds, 30);
        assert_eq!(read.claude, defaults.claude);
        assert_eq!(read.antigravity, defaults.antigravity);
        assert_eq!(RefreshPreferences::from_saved(&"none".into()), defaults);
        // A card's switches are kept; those that cannot be read, or set
        // nothing, follow the pool's.
        let saved = serde_json::json!({
            "claude": {"adaptive": true},
            "cards": {
                "claude": {
                    "a": {"adaptive": false},
                    "b": {"adaptive": "no"},
                    "c": {},
                    "d": {"backoff": false, "later": 1},
                },
                "codex": "none",
                "later": {"a": {"adaptive": true}},
            },
        });
        let read = RefreshPreferences::from_saved(&saved);
        assert_eq!(
            read.cards.claude.keys().collect::<Vec<_>>(),
            [&"a".to_owned()]
        );
        assert!(!read.card(Pool::Claude, "a").adaptive);
        assert!(read.card(Pool::Claude, "b").adaptive);
        assert!(read.cards.codex.is_empty());
        // As saved, a card's unset switch is left out.
        assert_eq!(
            serde_json::to_value(&read.cards).unwrap()["claude"],
            serde_json::json!({"a": {"adaptive": false}})
        );
    }
}
