//! One refresh plan for every pool: which cards a background round, or the
//! asks between rounds for cards speeding up, refresh, and when each card is
//! next refreshed. Both answers come from the same facts about the cards, so
//! the time a card shows is the round that asks for it.
//!
//! An account is asked for when it is not left to a manual refresh (its
//! first request waits), not asked within the interval (by a manual refresh,
//! say) and not paused by a rate limit: by every round while in use, else by
//! the rounds that refresh the whole pool. A Sub2api provider's cards refresh
//! together with their provider, every round while it is in use, else on
//! full rounds, whenever they were last refreshed.

use crate::{
    models::{AccountSummary, CardRefreshSummary, Pool},
    refresh_policy::{self, RefreshPreferences, Schedule},
    sub2api,
};
use std::collections::BTreeMap;

/// What the plan knows of a card.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Card<'a> {
    pub(crate) id: &'a str,
    /// The Sub2api provider whose cards this is one of.
    pub(crate) provider: Option<&'a str>,
    pub(crate) current: bool,
    /// When its usage was last asked for (Unix seconds).
    pub(crate) refreshed_at: i64,
    /// Imported and not yet asked for upstream: left to a manual refresh.
    pub(crate) waiting: bool,
    /// Deleted while deactivated, kept hidden: never asked for again, for a
    /// refresh would spend its login.
    pub(crate) deleted: bool,
    /// Until when a rate limit pauses its requests (Unix seconds).
    pub(crate) paused_until: Option<i64>,
}
impl<'a> Card<'a> {
    /// The facts of a listed card; `pauses` are the pool's rate limit
    /// pauses, by account id.
    pub(crate) fn of(
        pool: Pool,
        account: &'a AccountSummary,
        pauses: &BTreeMap<String, i64>,
    ) -> Self {
        Self {
            id: &account.id,
            provider: sub2api::card_provider(pool, &account.id),
            current: account.is_current,
            refreshed_at: account.updated_at,
            waiting: account.awaiting_first_request,
            deleted: account.display_status == "deleted",
            paused_until: pauses.get(&account.id).copied(),
        }
    }
    /// Whether a round, or ask, starting `at` asks for this account, which
    /// was refreshed within `interval` seconds before it if not.
    fn due(&self, at: i64, interval: i64) -> bool {
        !self.waiting
            && !self.deleted
            && at - self.refreshed_at >= interval
            && self.paused_until.is_none_or(|until| until <= at)
    }
    /// When an account can next be asked for: an interval after it was last
    /// asked, and once its pause is over.
    fn not_before(&self, interval: i64) -> i64 {
        (self.refreshed_at + interval).max(self.paused_until.unwrap_or(i64::MIN))
    }
}

/// What a round asks for: these accounts, and these providers' cards, or
/// every provider's for `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Ask {
    pub(crate) ids: Vec<String>,
    pub(crate) providers: Option<Vec<String>>,
}

/// What the background round starting `at` asks for, refreshing the whole
/// pool when `full`; its accounts not asked within `interval` seconds.
/// `in_use` is the provider the pool's tool uses, which a round asks for
/// even before it has cards.
pub(crate) fn round(
    cards: &[Card],
    in_use: Option<&str>,
    full: bool,
    at: i64,
    interval: i64,
) -> Ask {
    let ids = cards
        .iter()
        .filter(|c| c.provider.is_none() && (full || c.current) && c.due(at, interval))
        .map(|c| c.id.to_owned())
        .collect();
    // Between full rounds, only a provider in use.
    let providers = (!full).then(|| {
        let current = cards
            .iter()
            .filter(|c| c.current)
            .filter_map(|c| c.provider);
        let mut providers: Vec<String> = current.chain(in_use).map(str::to_owned).collect();
        providers.sort();
        providers.dedup();
        providers
    });
    Ask { ids, providers }
}
/// The accounts a refresh of the whole pool asks for: all but those deleted,
/// whose logins are never used again.
pub(crate) fn whole_pool(cards: &[Card]) -> Vec<String> {
    cards
        .iter()
        .filter(|c| c.provider.is_none() && !c.deleted)
        .map(|c| c.id.to_owned())
        .collect()
}
/// What the ask at `at` for the cards speeding up (`keys`, card keys) asks
/// for: their accounts not asked within the sooner interval, `soon`
/// seconds, and their providers.
pub(crate) fn fast(pool: Pool, cards: &[Card], keys: &[String], at: i64, soon: i64) -> Ask {
    let ids = cards
        .iter()
        .filter(|c| {
            c.provider.is_none()
                && keys.contains(&refresh_policy::card_key(pool, c.id))
                && c.due(at, soon)
        })
        .map(|c| c.id.to_owned())
        .collect();
    let mut providers: Vec<String> = keys
        .iter()
        .filter_map(|key| key.strip_prefix("sub2api:"))
        .map(str::to_owned)
        .collect();
    providers.sort();
    providers.dedup();
    Ask {
        ids,
        providers: Some(providers),
    }
}

/// The asks between rounds for the cards speeding up.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Speedup {
    /// When the next ask starts (Unix seconds).
    pub(crate) at: i64,
    /// The sooner interval between asks.
    pub(crate) soon: i64,
}

/// When a card is next asked for, should the interval stay as it is: by the
/// next round that asks for it, or sooner by an ask between rounds while it
/// speeds up (`speedup`, for a card speeding up); none for an account left
/// to a manual refresh.
pub(crate) fn next_refresh_at(
    card: &Card,
    schedule: &Schedule,
    speedup: Option<Speedup>,
) -> Option<i64> {
    if card.provider.is_some() {
        let next = schedule.next_refresh_at(card.current, 0);
        return Some(speedup.map_or(next, |s| next.min(s.at)));
    }
    if card.waiting || card.deleted {
        return None;
    }
    let next = schedule.next_refresh_at(card.current, card.not_before(schedule.interval));
    // The first ask between rounds that finds the card due.
    let fast = speedup.map(|s| {
        let soon = s.soon.max(1);
        let wait = (card.not_before(s.soon) - s.at).max(0);
        s.at + (wait + soon - 1) / soon * soon
    });
    Some(fast.map_or(next, |fast| next.min(fast)))
}

/// How the background refresh treats a card, for the card to show: the
/// key its own switches are saved under, and its usual interval, every
/// round while in use, else a full round's.
pub(crate) fn summary(
    pool: Pool,
    account: &AccountSummary,
    refresh: &RefreshPreferences,
) -> CardRefreshSummary {
    let settings = refresh.pool(pool);
    CardRefreshSummary {
        key: refresh_policy::card_key(pool, &account.id),
        seconds: if account.is_current {
            settings.seconds
        } else {
            settings.seconds * settings.full_every
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: i64 = 10_000;
    /// Rounds every 100 seconds, every third refreshing the whole pool; the
    /// next, at AT, is not one of them.
    const SCHEDULE: Schedule = Schedule {
        at: AT,
        tick: 1,
        interval: 100,
        full_every: 3,
    };
    fn card(id: &str) -> Card<'_> {
        Card {
            id,
            provider: None,
            current: false,
            refreshed_at: AT - 500,
            waiting: false,
            deleted: false,
            paused_until: None,
        }
    }
    /// The rounds of SCHEDULE from AT on, as (at, full).
    fn rounds() -> impl Iterator<Item = (i64, bool)> {
        (0..12).map(|n| (AT + n * 100, (1 + n as u64) % 3 == 0))
    }

    #[test]
    fn a_round_asks_for_the_card_in_use_and_on_full_rounds_for_all() {
        let current = Card {
            current: true,
            ..card("current")
        };
        let other = card("other");
        let cards = [current, other];
        assert_eq!(
            round(&cards, None, false, AT, 100),
            Ask {
                ids: vec!["current".into()],
                providers: Some(vec![])
            }
        );
        assert_eq!(
            round(&cards, None, true, AT, 100),
            Ask {
                ids: vec!["current".into(), "other".into()],
                providers: None
            }
        );
    }

    #[test]
    fn a_round_leaves_out_accounts_asked_lately_paused_waiting_or_deleted() {
        let lately = Card {
            refreshed_at: AT - 99,
            ..card("lately")
        };
        let paused = Card {
            paused_until: Some(AT + 1),
            ..card("paused")
        };
        let waiting = Card {
            waiting: true,
            ..card("waiting")
        };
        let deleted = Card {
            deleted: true,
            ..card("deleted")
        };
        let due = Card {
            refreshed_at: AT - 100,
            paused_until: Some(AT),
            ..card("due")
        };
        let ask = round(
            &[lately, paused, waiting, deleted, due],
            None,
            true,
            AT,
            100,
        );
        assert_eq!(ask.ids, ["due"]);
        assert_eq!(next_refresh_at(&deleted, &SCHEDULE, None), None);
        assert_eq!(whole_pool(&[lately, deleted]), ["lately"]);
    }

    #[test]
    fn a_providers_cards_refresh_with_it_while_in_use_or_on_full_rounds() {
        let used = Card {
            provider: Some("relay"),
            current: true,
            refreshed_at: AT,
            ..card("sub2api-relay-1")
        };
        let idle = Card {
            provider: Some("spare"),
            ..card("sub2api-spare-1")
        };
        let ask = round(&[used, idle], None, false, AT, 100);
        assert_eq!(
            ask,
            Ask {
                ids: vec![],
                providers: Some(vec!["relay".into()])
            }
        );
        assert_eq!(round(&[used, idle], None, true, AT, 100).providers, None);
        // A provider in use before it has cards.
        assert_eq!(
            round(&[], Some("new"), false, AT, 100).providers,
            Some(vec!["new".into()])
        );
        assert_eq!(
            round(&[used], Some("relay"), false, AT, 100).providers,
            Some(vec!["relay".into()])
        );
        // Whenever they were refreshed, their time is the provider's.
        assert_eq!(next_refresh_at(&used, &SCHEDULE, None), Some(AT));
        assert_eq!(next_refresh_at(&idle, &SCHEDULE, None), Some(AT + 200));
    }

    #[test]
    fn asks_between_rounds_take_the_cards_speeding_up_not_asked_lately() {
        let pool = Pool::Claude;
        let soon = Card {
            refreshed_at: AT - 30,
            ..card("soon")
        };
        let lately = Card {
            refreshed_at: AT - 29,
            ..card("lately")
        };
        let paused = Card {
            paused_until: Some(AT + 1),
            ..card("paused")
        };
        let slow = card("slow");
        let keys = ["soon", "lately", "paused", "sub2api:relay"].map(String::from);
        let ask = fast(pool, &[soon, lately, paused, slow], &keys, AT, 30);
        assert_eq!(
            ask,
            Ask {
                ids: vec!["soon".into()],
                providers: Some(vec!["relay".into()])
            }
        );
    }

    #[test]
    fn a_card_shows_the_next_round_that_asks_for_it() {
        let current = Card {
            current: true,
            ..card("current")
        };
        assert_eq!(next_refresh_at(&current, &SCHEDULE, None), Some(AT));
        // Others wait for the next full round.
        assert_eq!(
            next_refresh_at(&card("other"), &SCHEDULE, None),
            Some(AT + 200)
        );
        // One asked lately skips the rounds before an interval has passed.
        let lately = Card {
            current: true,
            refreshed_at: AT - 50,
            ..card("lately")
        };
        assert_eq!(next_refresh_at(&lately, &SCHEDULE, None), Some(AT + 100));
        // A pause holds it until a round after the pause.
        let paused = Card {
            paused_until: Some(AT + 250),
            ..card("paused")
        };
        assert_eq!(next_refresh_at(&paused, &SCHEDULE, None), Some(AT + 500));
        // One waiting for its first request has none.
        let waiting = Card {
            waiting: true,
            ..card("waiting")
        };
        assert_eq!(next_refresh_at(&waiting, &SCHEDULE, None), None);
    }

    #[test]
    fn a_card_speeding_up_shows_the_first_ask_that_finds_it_due() {
        let speedup = Some(Speedup {
            at: AT + 10,
            soon: 30,
        });
        let other = card("other");
        assert_eq!(next_refresh_at(&other, &SCHEDULE, speedup), Some(AT + 10));
        // Asked 10 seconds ago, the ask at AT + 10 passes it by; the next takes it.
        let lately = Card {
            refreshed_at: AT - 10,
            ..card("lately")
        };
        assert_eq!(next_refresh_at(&lately, &SCHEDULE, speedup), Some(AT + 40));
        // Paused, the first ask after its pause.
        let paused = Card {
            paused_until: Some(AT + 75),
            ..card("paused")
        };
        assert_eq!(next_refresh_at(&paused, &SCHEDULE, speedup), Some(AT + 100));
        // A round sooner than the ask wins.
        let current = Card {
            current: true,
            ..card("current")
        };
        let late = Some(Speedup {
            at: AT + 60,
            soon: 30,
        });
        assert_eq!(next_refresh_at(&current, &SCHEDULE, late), Some(AT));
    }

    /// A card shown due at a round's start is asked for by that round, and
    /// by none before it: across the rounds of SCHEDULE, for every kind of
    /// card.
    #[test]
    fn a_card_is_refreshed_when_it_shows() {
        let cards = [
            Card {
                current: true,
                ..card("current")
            },
            Card {
                current: true,
                refreshed_at: AT - 50,
                ..card("current-lately")
            },
            card("other"),
            Card {
                refreshed_at: AT + 150,
                ..card("other-lately")
            },
            Card {
                paused_until: Some(AT + 250),
                ..card("paused")
            },
            Card {
                current: true,
                paused_until: Some(AT + 420),
                ..card("current-paused")
            },
            Card {
                provider: Some("relay"),
                current: true,
                ..card("sub2api-relay-1")
            },
            Card {
                provider: Some("spare"),
                ..card("sub2api-spare-1")
            },
        ];
        for card in &cards {
            let shown = next_refresh_at(card, &SCHEDULE, None).unwrap();
            let asked = rounds()
                .find(|&(at, full)| {
                    let ask = round(
                        std::slice::from_ref(card),
                        None,
                        full,
                        at,
                        SCHEDULE.interval,
                    );
                    match card.provider {
                        Some(provider) => ask
                            .providers
                            .is_none_or(|p| p.iter().any(|p| p == provider)),
                        None => ask.ids.iter().any(|id| id == card.id),
                    }
                })
                .map(|(at, _)| at);
            assert_eq!(asked, Some(shown), "{}", card.id);
        }
    }

    fn listed(id: &str) -> AccountSummary {
        let stored: crate::models::StoredAccount = serde_json::from_value(serde_json::json!({
            "id": id, "label": id, "accountId": id, "authJson": {}, "addedAt": 0, "updatedAt": 0,
        }))
        .unwrap();
        stored.to_summary(None)
    }

    #[test]
    fn a_cards_summary_names_its_key_and_usual_interval() {
        let refresh = RefreshPreferences::default();
        let mut account = listed("acct");
        assert_eq!(
            summary(Pool::Codex, &account, &refresh),
            CardRefreshSummary {
                key: "acct".into(),
                seconds: 60
            }
        );
        account.is_current = true;
        assert_eq!(summary(Pool::Codex, &account, &refresh).seconds, 10);
        let provider = listed("claude-sub2api-relay-3");
        assert_eq!(
            summary(Pool::Claude, &provider, &refresh).key,
            "sub2api:relay"
        );
        assert_eq!(summary(Pool::Claude, &provider, &refresh).seconds, 300);
    }
}
