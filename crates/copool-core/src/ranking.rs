//! Quota ranking shared by application commands and the migrated Swift tests.
use crate::models::{AccountSummary, UsageWindow};
pub(crate) fn remaining_score(a: &AccountSummary) -> f64 {
    let remaining = |w: Option<&UsageWindow>| {
        w.map(|w| (100.0 - w.used_percent).clamp(0.0, 100.0))
            .unwrap_or(0.0)
    };
    0.7 * remaining(a.usage.as_ref().and_then(|u| u.one_week.as_ref()))
        + 0.3
            * a.usage
                .as_ref()
                .and_then(|u| u.five_hour_remaining())
                .unwrap_or(0.0)
}
pub(crate) fn sort_for_display(accounts: &mut [AccountSummary]) {
    accounts.sort_by(|a, b| {
        b.is_current
            .cmp(&a.is_current)
            .then_with(|| remaining_score(b).total_cmp(&remaining_score(a)))
            .then(a.added_at.cmp(&b.added_at))
    });
}
pub(crate) fn best(accounts: &[AccountSummary]) -> Option<&AccountSummary> {
    accounts
        .iter()
        .max_by(|a, b| remaining_score(a).total_cmp(&remaining_score(b)))
}
pub(crate) fn exhausted(a: &AccountSummary) -> bool {
    a.usage.as_ref().is_some_and(|u| {
        [&u.five_hour, &u.one_week]
            .iter()
            .any(|w| w.as_ref().is_some_and(|w| w.used_percent >= 100.0))
    })
}
pub(crate) fn auto_switch_target(accounts: &[AccountSummary]) -> Option<&AccountSummary> {
    // A Sub2api provider's accounts, on its one card, are current together:
    // it is exhausted once each is exhausted or unavailable, as a single
    // account is.
    let current: Vec<_> = accounts.iter().filter(|a| a.is_current).collect();
    let spent: Vec<_> = current.iter().filter(|a| exhausted(a)).collect();
    if spent.is_empty() || current.iter().any(|a| a.available && !exhausted(a)) {
        return None;
    }
    let floor = spent
        .iter()
        .map(|a| remaining_score(a))
        .fold(f64::MIN, f64::max);
    accounts
        .iter()
        .filter(|a| !a.is_current && a.available && !exhausted(a) && remaining_score(a) > floor)
        .max_by(|a, b| remaining_score(a).total_cmp(&remaining_score(b)))
}
