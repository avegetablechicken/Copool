use crate::models::AccountSummary;

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
