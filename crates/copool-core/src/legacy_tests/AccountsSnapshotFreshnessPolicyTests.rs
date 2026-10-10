use crate::refresh_policy::*;
#[test]
fn testShouldSkipRemoteUsageRefreshWhenRemoteSnapshotIsFresh() {
    assert!(!should_refresh(false, Some(1000), 1029, 30));
}
#[test]
fn testShouldRefreshUsageWhenRemoteSnapshotIsStale() {
    assert!(should_refresh(false, Some(1000), 1031, 30));
}
#[test]
fn testForceRefreshAlwaysWins() {
    assert!(should_refresh(true, Some(1029), 1030, 30));
}
#[test]
fn testRemoteSnapshotFreshnessWindowIsInclusive() {
    assert!(snapshot_is_fresh(Some(1000), 1030, 30));
}
