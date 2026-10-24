use super::account;
use crate::ranking::*;
#[test]
fn testPickBestAccountChoosesMostRemainingQuota() {
    let a = [
        account("c", 80., 90., false),
        account("b", 40., 30., false),
        account("a", 15., 30., false),
    ];
    assert_eq!(best(&a).unwrap().id, "a");
}
#[test]
fn testSortForDisplayPinsCurrentAccountFirst() {
    let mut a = [
        account("medium", 40., 40., false),
        account("best", 10., 10., false),
        account("current", 95., 95., true),
    ];
    sort_for_display(&mut a);
    assert_eq!(a.map(|a| a.id), ["current", "best", "medium"]);
}
#[test]
fn testSortForDisplayKeepsBestRemainingOrderForNonCurrentAccounts() {
    let mut a = [
        account("worst", 90., 90., false),
        account("current", 20., 20., true),
        account("best", 10., 10., false),
    ];
    sort_for_display(&mut a);
    assert_eq!(a.map(|a| a.id), ["current", "best", "worst"]);
}
#[test]
fn testAutoSwitchTargetIsNilWhenCurrentAccountNotExhausted() {
    assert!(auto_switch_target(&[
        account("current", 60., 70., true),
        account("better", 10., 15., false)
    ])
    .is_none());
}
#[test]
fn testAutoSwitchTargetChoosesBestAlternativeWhenCurrentIsExhausted() {
    let a = [
        account("current", 100., 95., true),
        account("other", 40., 25., false),
        account("best", 20., 15., false),
    ];
    assert_eq!(auto_switch_target(&a).unwrap().id, "best");
}
#[test]
fn testAutoSwitchTargetIsNilWhenCurrentHasOnePercentRemaining() {
    assert!(auto_switch_target(&[
        account("current", 99., 95., true),
        account("best", 20., 15., false),
        account("other", 40., 25., false)
    ])
    .is_none());
}
#[test]
fn testAutoSwitchTargetIsNilWhenCurrentHasOnePercentRemainingButAlternativesAreWorse() {
    assert!(auto_switch_target(&[
        account("current", 99., 100., true),
        account("exhausted", 100., 100., false)
    ])
    .is_none());
}
#[test]
fn testAutoSwitchTargetIsNilWhenNoCurrentAccount() {
    assert!(
        auto_switch_target(&[account("a", 100., 100., false), account("b", 5., 5., false)])
            .is_none()
    );
}
#[test]
fn testAutoSwitchTargetIsNilWhenCurrentExhaustedButNoAlternative() {
    assert!(auto_switch_target(&[account("current", 100., 100., true)]).is_none());
}
#[test]
fn testAutoSwitchTargetKeepsProviderWhileAnyOfItsCardsHasQuota() {
    // A Sub2api provider in use: all its cards are current.
    let a = [
        account("spent", 0., 100., true),
        account("left", 50., 0., true),
        account("other", 10., 10., false),
    ];
    assert!(auto_switch_target(&a).is_none());
}
#[test]
fn testAutoSwitchTargetLeavesProviderOnceEveryCardIsExhausted() {
    let mut paused = account("paused", 20., 20., true);
    paused.available = false;
    let a = [
        account("spent", 0., 100., true),
        account("worn", 100., 50., true),
        paused,
        account("other", 10., 10., false),
    ];
    assert_eq!(auto_switch_target(&a).unwrap().id, "other");
}
