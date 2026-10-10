use crate::usage::{pick_nearest_window, UsageWindowRaw};
#[test]
fn testPickNearestWindowPrefersFiveHour() {
    let windows = [
        UsageWindowRaw {
            used_percent: 40.,
            limit_window_seconds: 18000,
            reset_at: 123,
        },
        UsageWindowRaw {
            used_percent: 20.,
            limit_window_seconds: 604800,
            reset_at: 456,
        },
    ];
    assert_eq!(
        pick_nearest_window(&windows, 18000)
            .unwrap()
            .limit_window_seconds,
        18000
    );
}
#[test]
fn testPickNearestWindowReturnsNilForEmptyInput() {
    assert!(pick_nearest_window(&[], 100).is_none());
}
