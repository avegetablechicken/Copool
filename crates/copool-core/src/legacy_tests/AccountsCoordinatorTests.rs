use super::{account, auth, fixture, save_fixture, stored};
use crate::{refresh_policy::*, utils::now_unix_seconds, Preferences};
use serde_json::{json, Value};
fn reset(id: &str, current: bool, at: i64) -> crate::models::AccountSummary {
    let mut a = account(id, 0., 0., current);
    a.usage
        .as_mut()
        .unwrap()
        .five_hour
        .as_mut()
        .unwrap()
        .reset_at = Some(at);
    a
}
#[test]
fn testAccountsUsageRefreshPlanningTargetsCurrentAndNearResetAccounts() {
    let now = 1763216000;
    let mut error = reset("acct-error", false, now + 600);
    error.usage_error = Some("timeout".into());
    assert_eq!(
        target_ids(
            &[
                reset("acct-current", true, now + 600),
                reset("acct-near-reset", false, now + 45),
                reset("acct-idle", false, now + 600),
                error
            ],
            now,
            60
        ),
        ["acct-current", "acct-near-reset", "acct-error"]
    );
}
#[test]
fn testAccountsUsageRefreshPlanningTargetsNonCurrentAccountsJustAfterReset() {
    let now = 1763216000;
    assert_eq!(
        target_ids(
            &[
                reset("acct-current", true, now + 600),
                reset("acct-post-reset", false, now - 30),
                reset("acct-stale", false, now - 600)
            ],
            now,
            60
        ),
        ["acct-current", "acct-post-reset"]
    );
}
#[test]
fn testAccountsUsageRefreshPlanningUsesActiveThenFullRecurringTicks() {
    let a = [
        account("acct-current", 0., 0., true),
        account("acct-idle", 0., 0., false),
    ];
    assert_eq!(recurring_ids(1, &a), Some(vec!["acct-current".into()]));
    assert_eq!(recurring_ids(6, &a), None);
}
#[test]
fn testBackgroundRefreshPolicyStartsAtTheAdaptiveFiveMinuteInterval() {
    let (_dir, app) = fixture();
    assert_eq!(
        app.refresh_interval(),
        crate::refresh_policy::REFRESH_INTERVAL
    );
}
#[test]
fn testBackgroundRefreshIntervalAdaptsAsOpenQuota() {
    use std::time::Duration;
    let (five, one) = (Duration::from_secs(300), Duration::from_secs(60));
    assert_eq!((REFRESH_INTERVAL, FAST_REFRESH_INTERVAL), (five, one));
    let mut unchanged = 0;
    // Without a change the interval stays at five minutes.
    for _ in 0..3 {
        assert_eq!(refresh_interval_for(five, false, &mut unchanged), five);
        assert_eq!(unchanged, 0);
    }
    // A change speeds it up, from either interval.
    assert_eq!(refresh_interval_for(five, true, &mut unchanged), one);
    assert_eq!(refresh_interval_for(one, true, &mut unchanged), one);
    // Two unchanged one-minute rounds slow it down again...
    assert_eq!(refresh_interval_for(one, false, &mut unchanged), one);
    assert_eq!(unchanged, 1);
    assert_eq!(refresh_interval_for(one, false, &mut unchanged), five);
    assert_eq!(unchanged, 0);
    // ...counted anew after each change.
    assert_eq!(refresh_interval_for(five, true, &mut unchanged), one);
    assert_eq!(refresh_interval_for(one, false, &mut unchanged), one);
    assert_eq!(refresh_interval_for(one, true, &mut unchanged), one);
    assert_eq!(unchanged, 0);
    assert_eq!(refresh_interval_for(one, false, &mut unchanged), one);
    assert_eq!(refresh_interval_for(one, false, &mut unchanged), five);
    // An interval other than one minute (a fresh start) counts as five.
    assert_eq!(
        refresh_interval_for(Duration::from_secs(10), false, &mut unchanged),
        five
    );
}
#[test]
fn testBackgroundRefreshComparesUsageWithoutFetchTimes() {
    let cards = |hour: f64, fetched: i64| {
        let mut a = account("acct", 40., hour, true);
        a.usage.as_mut().unwrap().fetched_at = fetched;
        let mut b = account("acct-empty", 0., 0., false);
        b.usage = None;
        usage_values(&[a, b])
    };
    let before = cards(25., 1);
    assert_eq!(before.len(), 1);
    assert!(before["acct"].get("fetchedAt").is_none());
    // A new fetch time alone is no change.
    assert!(!usage_changed(&before, &cards(25., 2)));
    assert!(usage_changed(&before, &cards(26., 2)));
    let mut reset = before.clone();
    reset.get_mut("acct").unwrap()["fiveHour"]["resetAt"] = json!(1_800_000_000);
    assert!(usage_changed(&before, &reset));
    let mut credits = before.clone();
    credits.get_mut("acct").unwrap()["credits"] =
        json!({"hasCredits": true, "unlimited": false, "balance": "5"});
    assert!(usage_changed(&before, &credits));
    // A card with usage for the first time is a change; one without any
    // window or credits, or one losing its usage, is not.
    let empty = Default::default();
    assert!(usage_changed(&empty, &before));
    let mut blank = std::collections::BTreeMap::new();
    blank.insert(
        "acct".to_string(),
        json!({"fiveHour": null, "oneWeek": null, "credits": null, "planType": "pro"}),
    );
    assert!(!usage_changed(&empty, &blank));
    assert!(!usage_changed(&before, &empty));
}

#[tokio::test]
async fn testSwitchAccountWritesCurrentAccountIDToStore() {
    let (_dir, app) = fixture();
    save_fixture(&app, json!({"accounts":[stored("acct-1","p","workspace")]}));
    app.switch_account("acct-1").await.unwrap();
    let saved: Value =
        serde_json::from_slice(&std::fs::read(app.paths.data_dir.join("accounts.json")).unwrap())
            .unwrap();
    assert_eq!(saved["currentAccountId"], "acct-1");
    assert_eq!(saved["currentSelection"]["cardId"], "acct-1");
    assert_eq!(
        app.accounts()
            .await
            .unwrap()
            .iter()
            .filter(|a| a.is_current)
            .count(),
        1
    );
}
#[tokio::test]
async fn testAutoSmartSwitchReturnsCurrentAccountAfterSwitch() {
    let (_dir, app) = fixture();
    let mut a = stored("old", "old", "old");
    let mut b = stored("best", "best", "best");
    a["usage"] = json!({"fetchedAt":now_unix_seconds(),"fiveHour":{"usedPercent":100,"windowSeconds":18000},"oneWeek":{"usedPercent":100,"windowSeconds":604800}});
    b["usage"] = json!({"fetchedAt":now_unix_seconds(),"fiveHour":{"usedPercent":10,"windowSeconds":18000},"oneWeek":{"usedPercent":20,"windowSeconds":604800}});
    save_fixture(&app, json!({"accounts":[a,b],"currentAccountId":"old"}));
    app.save_preferences(Preferences {
        auto_switch: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let switched = app.auto_switch_if_needed().await.unwrap();
    assert_eq!(switched.as_deref(), Some("best"));
    assert!(
        app.accounts()
            .await
            .unwrap()
            .iter()
            .find(|a| a.id == "best")
            .unwrap()
            .is_current
    );
}
#[tokio::test]
async fn testAutoSmartSwitchUsesSelectedCardWhenAccountsShareIdentity() {
    let (_dir, app) = fixture();
    let mut a = stored("old", "p", "shared");
    let mut b = stored("best", "p", "shared");
    a["usage"] = json!({"fetchedAt":now_unix_seconds(),"fiveHour":{"usedPercent":100,"windowSeconds":18000},"oneWeek":{"usedPercent":100,"windowSeconds":604800}});
    b["usage"] = json!({"fetchedAt":now_unix_seconds(),"fiveHour":{"usedPercent":1,"windowSeconds":18000},"oneWeek":{"usedPercent":1,"windowSeconds":604800}});
    save_fixture(&app, json!({"accounts":[a,b],"currentAccountId":"old"}));
    app.save_preferences(Preferences {
        auto_switch: true,
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(
        app.auto_switch_if_needed().await.unwrap().as_deref(),
        Some("best")
    );
    assert_eq!(
        app.accounts()
            .await
            .unwrap()
            .iter()
            .filter(|a| a.is_current)
            .count(),
        1
    );
}
#[tokio::test]
async fn testListAccountsKeepsCurrentSelectionWhenReconciledAccountKeyChanges() {
    let (_dir, app) = fixture();
    let mut a = stored("card", "old", "old");
    a["authJson"] = auth("new", "new");
    save_fixture(
        &app,
        json!({"accounts":[a],"currentAccountId":"card","currentSelection":{"cardId":"card","accountKey":"old|old","selectedAt":1,"sourceDeviceId":"macos-local"}}),
    );
    let accounts = app.accounts().await.unwrap();
    assert_eq!(accounts[0].account_id, "new");
    assert!(accounts[0].is_current);
    let saved: Value =
        serde_json::from_slice(&std::fs::read(app.paths.data_dir.join("accounts.json")).unwrap())
            .unwrap();
    assert_eq!(saved["currentAccountId"], "card");
    assert_eq!(saved["currentSelection"]["accountKey"], "new|new");
}
#[tokio::test]
async fn testListAccountsDoesNotClearStoredWorkspaceNameWhenAuthLacksTeamName() {
    let (_dir, app) = fixture();
    let mut a = stored("card", "p", "workspace");
    a["teamName"] = "Existing Team".into();
    save_fixture(&app, json!({"accounts":[a]}));
    assert_eq!(
        app.accounts().await.unwrap()[0].team_name.as_deref(),
        Some("Existing Team")
    );
}
#[tokio::test]
async fn testListAccountsReconcilesStoredWorkspaceMetadataFromAuthJSON() {
    let (_dir, app) = fixture();
    let mut a = stored("acct-1", "p", "account-1");
    a["email"] = Value::Null;
    a["planType"] = Value::Null;
    a["authJson"]["tokens"]["id_token"]=super::jwt(json!({"sub":"p","email":"test@example.com","https://api.openai.com/auth":{"chatgpt_account_id":"account-1","chatgpt_plan_type":"pro","chatgpt_team_name":"workspace-x"}})).into();
    save_fixture(&app, json!({"accounts":[a]}));
    let accounts = app.accounts().await.unwrap();
    assert_eq!(accounts[0].email.as_deref(), Some("test@example.com"));
    assert_eq!(accounts[0].plan_type.as_deref(), Some("pro"));
    assert_eq!(accounts[0].team_name.as_deref(), Some("workspace-x"));
    let saved: Value =
        serde_json::from_slice(&std::fs::read(app.paths.data_dir.join("accounts.json")).unwrap())
            .unwrap();
    assert_eq!(saved["accounts"][0]["teamName"], "workspace-x");
}

#[test]
fn testWorkspaceMetadataCancellationPrefersFriendlyMessage() {
    assert_eq!(
        crate::workspaces::preferred_failure(&[
            "https://chatgpt.com/backend-api/accounts -> cancelled".into()
        ])
        .as_deref(),
        Some("Workspace discovery was cancelled. Please try again.")
    );
}
#[test]
fn testWorkspaceMetadataTimeoutPrefersFriendlyMessage() {
    assert_eq!(
        crate::workspaces::preferred_failure(&[
            "https://chatgpt.com/backend-api/accounts -> The request timed out.".into()
        ])
        .as_deref(),
        Some("Workspace discovery timed out. Please try again.")
    );
}
#[test]
fn testWorkspaceMetadataHTMLForbiddenPrefersFriendlyMessage() {
    assert_eq!(crate::workspaces::preferred_failure(&["https://chatgpt.com/backend-api/accounts -> 403: <html>forbidden</html>".into()]).as_deref(),Some("Workspace discovery was denied. Refresh or re-import this account auth, then try again."));
}

#[tokio::test]
async fn testAccountProxyIsPersistedUsedForRefreshAndCanBeCleared() {
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[stored("a","a","account-1"),stored("b","b","account-2")]}),
    );
    app.save_account_proxy("a", " http://127.0.0.1:8080 ")
        .await
        .unwrap();
    app.save_account_proxy("b", "socks5://127.0.0.1:1080")
        .await
        .unwrap();
    app.refresh_accounts(None, true).await.unwrap();
    let proxies: std::collections::BTreeSet<_> = network
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|v| v.2.clone())
        .collect();
    assert_eq!(
        proxies,
        [
            "http://127.0.0.1:8080".into(),
            "socks5://127.0.0.1:1080".into()
        ]
        .into()
    );
    let saved = crate::repository::load_accounts(&app.paths.store()).unwrap();
    assert_eq!(saved.accounts[0].proxy_url, "http://127.0.0.1:8080");
    assert_eq!(
        saved.accounts[1].to_summary(None).proxy_url,
        "socks5://127.0.0.1:1080"
    );
    let round: crate::models::AccountsStore =
        serde_json::from_value(serde_json::to_value(&saved).unwrap()).unwrap();
    assert_eq!(
        round
            .accounts
            .iter()
            .map(|a| &a.proxy_url)
            .collect::<Vec<_>>(),
        saved
            .accounts
            .iter()
            .map(|a| &a.proxy_url)
            .collect::<Vec<_>>()
    );
    assert!(app.save_account_proxy("a", "invalid").await.is_err());
    assert_eq!(
        crate::repository::load_accounts(&app.paths.store())
            .unwrap()
            .accounts[0]
            .proxy_url,
        "http://127.0.0.1:8080"
    );
    app.save_account_proxy("a", " ").await.unwrap();
    let saved = crate::repository::load_accounts(&app.paths.store()).unwrap();
    assert_eq!(saved.accounts[0].proxy_url, "");
    assert_eq!(saved.accounts[1].proxy_url, "socks5://127.0.0.1:1080");
}
#[tokio::test]
async fn testForcedRefreshBypassesUsageThrottle() {
    let (_dir, app, network) = super::network_fixture::fixture();
    let mut a = stored("a", "a", "account-1");
    a["usage"] = json!({"fetchedAt":now_unix_seconds()});
    save_fixture(&app, json!({"accounts":[a]}));
    app.refresh_accounts(None, false).await.unwrap();
    assert!(network.calls.lock().unwrap().is_empty());
    app.refresh_accounts(None, true).await.unwrap();
    assert_eq!(network.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn testRefreshUsagePersistsUsageStateTimestamp() {
    let (_dir, app, _) = super::network_fixture::fixture();
    save_fixture(&app, json!({"accounts":[stored("a","a","account-1")]}));
    let before = now_unix_seconds();
    app.refresh_accounts(None, true).await.unwrap();
    let saved = crate::repository::load_accounts(&app.paths.store()).unwrap();
    let a = &saved.accounts[0];
    assert!(a.extra["usageStateUpdatedAt"].as_i64().unwrap() >= before);
    assert!(a.updated_at >= before);
    assert!(a.usage.is_some());
    assert!(a.usage_error.is_none());
}
#[tokio::test]
async fn testSelectiveUsageRefreshOnlyRequestsTargetAccounts() {
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[stored("a","a","account-1"),stored("b","b","account-2")]}),
    );
    app.refresh_accounts(Some(&["a".into()]), true)
        .await
        .unwrap();
    assert_eq!(
        network
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|v| v.1.clone())
            .collect::<Vec<_>>(),
        ["account-1"]
    );
    let saved = crate::repository::load_accounts(&app.paths.store()).unwrap();
    assert!(saved.accounts[0].usage.is_some());
    assert!(saved.accounts[1].usage.is_none());
}
#[tokio::test]
async fn testAccountCardsCarryTheAccessTokenExpiry() {
    let (_dir, app, _) = super::network_fixture::fixture();
    let mut a = stored("a", "a", "account-1");
    a["authJson"]["tokens"]["access_token"] = super::jwt(json!({"exp":1_900_000_000})).into();
    let mut b = stored("b", "b", "account-2");
    b["authJson"]["tokens"]["access_token"] = "opaque".into();
    // A JWT NumericDate may be fractional; malformed claims show nothing,
    // including milliseconds taken for seconds.
    let mut accounts = vec![a, b];
    for (i, exp) in [
        json!(1_900_000_000.75),
        json!(0),
        json!(-5),
        json!(1_900_000_000_000i64),
        json!("1900000000"),
        json!(null),
    ]
    .into_iter()
    .enumerate()
    {
        let mut c = stored(&format!("c{i}"), &format!("c{i}"), &format!("account-c{i}"));
        c["authJson"]["tokens"]["access_token"] = super::jwt(json!({ "exp": exp })).into();
        accounts.push(c);
    }
    save_fixture(&app, json!({ "accounts": accounts }));
    let cards = app.accounts().await.unwrap();
    let expiry = |id: &str| cards.iter().find(|c| c.id == id).unwrap().token_expires_at;
    assert_eq!(expiry("a"), Some(1_900_000_000));
    assert_eq!(expiry("b"), None);
    assert_eq!(expiry("c0"), Some(1_900_000_000));
    for i in 1..6 {
        assert_eq!(expiry(&format!("c{i}")), None, "case {i}");
    }
}
#[tokio::test]
async fn testRefreshUsageRefreshesExpiredStoredAuthBeforeFetching() {
    let (_dir, app, network) = super::network_fixture::fixture();
    let mut a = stored("a", "a", "account-1");
    a["authJson"]["tokens"]["access_token"] = super::jwt(json!({"exp":1})).into();
    save_fixture(&app, json!({"accounts":[a]}));
    app.refresh_accounts(None, true).await.unwrap();
    assert_eq!(
        network
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|v| v.0.clone())
            .collect::<Vec<_>>(),
        ["refresh", "usage"]
    );
    assert_eq!(
        crate::repository::load_accounts(&app.paths.store())
            .unwrap()
            .accounts[0]
            .auth_json["tokens"]["access_token"],
        "refreshed-test-only"
    );
}
#[tokio::test]
async fn testAccountProxyIsUsedForBothTokenRefreshAndUsage() {
    let (_dir, app, network) = super::network_fixture::fixture();
    let mut a = stored("a", "a", "account-1");
    a["authJson"]["tokens"]["access_token"] = super::jwt(json!({"exp":1})).into();
    save_fixture(&app, json!({"accounts":[a]}));
    app.save_account_proxy("a", "socks5://127.0.0.1:1080")
        .await
        .unwrap();
    app.refresh_accounts(None, true).await.unwrap();
    assert_eq!(
        *network.calls.lock().unwrap(),
        [
            (
                "refresh".into(),
                "account-1".into(),
                "socks5://127.0.0.1:1080".into()
            ),
            (
                "usage".into(),
                "account-1".into(),
                "socks5://127.0.0.1:1080".into()
            )
        ]
    );
}
#[tokio::test]
async fn testRefreshUsageShowsRefreshFailureMessageWhenRefreshTokenIsReused() {
    let (_dir, app, network) = super::network_fixture::fixture();
    let mut a = stored("a", "a", "account-1");
    a["authJson"]["tokens"]["access_token"] = super::jwt(json!({"exp":1})).into();
    save_fixture(&app, json!({"accounts":[a]}));
    *network.refresh_error.lock().unwrap() =
        Some("Your refresh token has already been used".into());
    assert!(app.refresh_accounts(None, true).await.is_err());
    assert_eq!(
        crate::repository::load_accounts(&app.paths.store())
            .unwrap()
            .accounts[0]
            .usage_error
            .as_deref(),
        Some("error.accounts.sign_in_expired")
    );
    assert_eq!(network.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn testLateRefreshDoesNotOverwriteALoginRenewedSince() {
    let (_dir, app, network) = super::network_fixture::fixture();
    let mut a = stored("a", "a", "account-1");
    a["authJson"]["tokens"]["access_token"] = super::jwt(json!({"exp":1})).into();
    save_fixture(&app, json!({"accounts":[a],"currentAccountId":"a"}));
    let app = std::sync::Arc::new(app);
    app.switch_account("a").await.unwrap();
    network
        .block
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let task = {
        let app = app.clone();
        tokio::spawn(async move { app.refresh_usage("a").await })
    };
    network.started.notified().await;
    // Codex renews the same login while the request is answered late.
    let mut renewed: Value =
        serde_json::from_slice(&std::fs::read(&app.paths.codex_auth).unwrap()).unwrap();
    renewed["tokens"]["access_token"] = "renewed-by-codex".into();
    renewed["last_refresh"] = "2099-01-01T00:00:00Z".into();
    crate::persistence::write_json(&app.paths.codex_auth, &renewed).unwrap();
    network.resume.notify_one();
    task.await.unwrap().unwrap();
    let current: Value =
        serde_json::from_slice(&std::fs::read(&app.paths.codex_auth).unwrap()).unwrap();
    assert_eq!(current["tokens"]["access_token"], "renewed-by-codex");
}
#[tokio::test]
async fn testRefreshUsageDoesNotOverwriteConcurrentProxySwitchProjection() {
    let (_dir, app, network) = super::network_fixture::fixture();
    let first = stored("a", "a", "account-1");
    let second = stored("b", "b", "account-2");
    save_fixture(
        &app,
        json!({"accounts":[first,second],"currentAccountId":"a"}),
    );
    let app = std::sync::Arc::new(app);
    app.switch_account("a").await.unwrap();
    network
        .block
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let task = {
        let app = app.clone();
        tokio::spawn(async move { app.refresh_usage("a").await })
    };
    network.started.notified().await;
    let b = crate::repository::load_accounts(&app.paths.store())
        .unwrap()
        .accounts[1]
        .clone();
    crate::proxy_integration::select(&app.paths, &b.id, &b.auth_json).unwrap();
    network.resume.notify_one();
    task.await.unwrap().unwrap();
    let store = crate::repository::load_accounts(&app.paths.store()).unwrap();
    assert_eq!(store.extra["currentAccountId"], "b");
    assert_eq!(store.current_selection.unwrap().account_id, "b");
    let current: Value =
        serde_json::from_slice(&std::fs::read(&app.paths.codex_auth).unwrap()).unwrap();
    assert_eq!(
        crate::auth::extract_auth(&current).unwrap().account_id,
        "account-2"
    );
    assert_eq!(
        app.accounts()
            .await
            .unwrap()
            .iter()
            .find(|a| a.is_current)
            .unwrap()
            .id,
        "b"
    );
}
#[tokio::test]
async fn testTrayRefreshPreservesAndPublishesProxySavedAfterItsSettingsSnapshot() {
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(&app, json!({"accounts":[stored("a","a","account-1")]}));
    let app = std::sync::Arc::new(app);
    network
        .block
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let task = {
        let app = app.clone();
        tokio::spawn(async move { app.refresh_usage("a").await })
    };
    network.started.notified().await;
    app.save_account_proxy("a", "http://127.0.0.1:8080")
        .await
        .unwrap();
    network.resume.notify_one();
    task.await.unwrap().unwrap();
    assert_eq!(
        app.accounts().await.unwrap()[0].proxy_url,
        "http://127.0.0.1:8080"
    );
    assert_eq!(
        app.configuration().unwrap().account_proxy_urls["a"],
        "http://127.0.0.1:8080"
    );
}
fn team(id: &str, workspace: &str, name: Option<&str>) -> Value {
    let mut a = stored(id, id, workspace);
    a["planType"] = "team".into();
    a["authJson"]["tokens"]["access_token"] = format!("token-{id}").into();
    a["authJson"]["tokens"]["id_token"]=super::jwt(json!({"sub":id,"email":format!("{id}@example.test"),"https://api.openai.com/auth":{"chatgpt_account_id":workspace,"chatgpt_plan_type":"team"}})).into();
    if let Some(name) = name {
        a["teamName"] = name.into();
    }
    a
}
fn metadata(id: &str, name: &str, structure: &str) -> crate::workspaces::Metadata {
    crate::workspaces::Metadata {
        account_id: id.into(),
        workspace_name: Some(name.into()),
        structure: Some(structure.into()),
    }
}
fn directory(id: &str, name: &str, status: &str, visibility: &str) -> Value {
    json!({"workspaceId":id,"workspaceName":name,"email":"a@example.test","planType":"team","kind":"workspace","source":"legacyMetadata","status":status,"visibility":visibility,"lastSeenAt":1,"lastStatusCheckedAt":2})
}
#[tokio::test]
async fn testListAccountsBackfillsWorkspaceNameFromRemoteMetadata() {
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(&app, json!({"accounts":[team("a","account-1",None)]}));
    *network.metadata.lock().unwrap() = vec![metadata("account-1", "remote-space", "workspace")];
    let rows = app.accounts_with_metadata().await.unwrap();
    assert_eq!(rows[0].team_name.as_deref(), Some("remote-space"));
    assert_eq!(
        crate::repository::load_accounts(&app.paths.store())
            .unwrap()
            .accounts[0]
            .extra["teamName"],
        "remote-space"
    );
}
#[tokio::test]
async fn testListAccountsSkipsRemoteWorkspaceMetadataLookupWhenStoredWorkspaceNameExists() {
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[team("a","account-1",Some("stored-space"))]}),
    );
    *network.metadata.lock().unwrap() = vec![metadata("account-1", "remote-space", "workspace")];
    let rows = app.accounts_with_metadata().await.unwrap();
    assert_eq!(rows[0].team_name.as_deref(), Some("stored-space"));
    assert!(network.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn testImportCurrentAuthPrefersRemoteWorkspaceMetadata() {
    let (_dir, app, network) = super::network_fixture::fixture();
    let a = team("a", "account-1", None);
    std::fs::create_dir_all(app.paths.codex_auth.parent().unwrap()).unwrap();
    std::fs::write(
        &app.paths.codex_auth,
        serde_json::to_vec(&a["authJson"]).unwrap(),
    )
    .unwrap();
    *network.metadata.lock().unwrap() = vec![metadata("account-1", "remote-space", "workspace")];
    app.import_current().await.unwrap();
    assert_eq!(
        app.accounts().await.unwrap()[0].team_name.as_deref(),
        Some("remote-space")
    );
    assert_eq!(
        crate::repository::load_accounts(&app.paths.store())
            .unwrap()
            .accounts[0]
            .extra["teamName"],
        "remote-space"
    );
}
#[tokio::test]
async fn testListAccountsIgnoresDeactivatedRemoteWorkspaceName() {
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(&app, json!({"accounts":[team("a","account-1",None)]}));
    *network.metadata.lock().unwrap() = vec![metadata("account-1", "dead-space", "deactivated")];
    let rows = app.accounts_with_metadata().await.unwrap();
    assert!(rows[0].team_name.is_none());
    assert!(rows[0].available);
}
#[tokio::test]
async fn testListAccountsMarksWorkspaceDeactivatedWhenRemoteMetadataSaysSo() {
    // The original regression deliberately expects active: named accounts skip metadata lookup.
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[team("a","account-1",Some("workspace-a"))]}),
    );
    *network.metadata.lock().unwrap() = vec![metadata("account-1", "workspace-a", "deactivated")];
    let rows = app.accounts_with_metadata().await.unwrap();
    assert_eq!(rows[0].workspace_status, "active");
    assert_eq!(rows[0].team_name.as_deref(), Some("workspace-a"));
    assert!(network.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn testRefreshAllUsageDoesNotClearStoredWorkspaceNameWhenAuthLacksTeamName() {
    let (_dir, app, _) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[team("a","account-1",Some("stored-space"))]}),
    );
    app.refresh_all().await.unwrap();
    assert_eq!(
        app.accounts().await.unwrap()[0].team_name.as_deref(),
        Some("stored-space")
    );
}
#[tokio::test]
async fn testRefreshAllUsageDoesNotBlockOnWorkspaceMetadataLookup() {
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(&app, json!({"accounts":[team("a","account-1",None)]}));
    *network.metadata_error.lock().unwrap() = Some("metadata unavailable".into());
    app.refresh_all().await.unwrap();
    assert!(network.calls.lock().unwrap().iter().all(|c| c.0 == "usage"));
    assert!(app.accounts().await.unwrap()[0].usage.is_some());
}
#[tokio::test]
async fn testRefreshAllUsageSeriallyStreamsPartialAccountUpdates() {
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[stored("a","a","a"),stored("b","b","b")]}),
    );
    let counts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let count = counts.clone();
    let path = app.paths.store();
    app.on_change(std::sync::Arc::new(move || {
        count.lock().unwrap().push(
            crate::repository::load_accounts(&path)
                .unwrap()
                .accounts
                .iter()
                .filter(|a| a.usage.is_some())
                .count(),
        );
    }));
    app.refresh_all().await.unwrap();
    assert!(counts.lock().unwrap().contains(&1));
    assert_eq!(counts.lock().unwrap().last(), Some(&2));
    assert_eq!(
        network
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|v| v.1.clone())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
}
#[tokio::test]
async fn testSyncWorkspaceDirectoryKeepsUnauthorizedCandidatesAndPreservesDeletedDeactivatedEntries(
) {
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[team("a","account-1",Some("remote-space"))],"workspaceDirectory":[directory("account-2","ops-space","active","visible"),directory("account-3","old-space","deactivated","deleted")]}),
    );
    *network.metadata.lock().unwrap() = vec![metadata("account-2", "ops-space", "workspace")];
    let entries = app.sync_workspace_directory().await.unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|v| v["workspaceId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["account-3", "account-2"]
    );
    assert_eq!(entries[0]["status"], "deactivated");
    assert_eq!(entries[0]["visibility"], "deleted");
    assert_eq!(entries[0]["lastSeenAt"], 1);
    assert_eq!(entries[1]["lastStatusCheckedAt"], 2);
    assert!(entries[1]["lastSeenAt"].as_i64().unwrap() > 1);
    assert_eq!(entries, app.workspace_directory().await.unwrap());
    assert_eq!(network.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn testSyncWorkspaceDirectoryIgnoresDeletedAccountsForDiscovery() {
    let (_dir, app, network) = super::network_fixture::fixture();
    let mut a = team("a", "account-1", None);
    a["displayStatus"] = "deleted".into();
    save_fixture(&app, json!({"accounts":[a]}));
    assert!(app.sync_workspace_directory().await.unwrap().is_empty());
    assert!(network.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn testSyncWorkspaceDirectoryThrowsWhenUnauthorizedWorkspaceDiscoveryFails() {
    let (_dir, app, network) = super::network_fixture::fixture();
    let existing = directory("pending", "Pending", "active", "visible");
    save_fixture(
        &app,
        json!({"accounts":[team("a","account-1",None)],"workspaceDirectory":[existing.clone()]}),
    );
    *network.metadata_error.lock().unwrap() = Some("metadata failed".into());
    assert_eq!(
        app.sync_workspace_directory().await.unwrap_err(),
        "metadata failed"
    );
    assert_eq!(app.workspace_directory().await.unwrap(), [existing]);
}
#[tokio::test]
async fn testSyncWorkspaceDirectoryBuildsPendingEntriesFromWorkspaceMetadataAndPreservesDeletedVisibility(
) {
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[team("a","account-1",None)],"workspaceDirectory":[directory("account-2","old-name","active","deleted")]}),
    );
    *network.metadata.lock().unwrap() = vec![
        metadata("account-1", "Authorized", "workspace"),
        metadata("account-2", "New-name", "workspace"),
        metadata("account-3", "Personal", "personal"),
        metadata("account-4", "Team four", "workspace"),
    ];
    let entries = app.sync_workspace_directory().await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["workspaceId"], "account-2");
    assert_eq!(entries[0]["workspaceName"], "New-name");
    assert_eq!(entries[0]["visibility"], "deleted");
    assert_eq!(entries[1]["workspaceId"], "account-4");
    assert_eq!(entries[1]["visibility"], "visible");
}
#[tokio::test]
async fn testSyncWorkspaceDirectoryPreservesConsentEntriesNotPresentInWorkspaceMetadata() {
    let (_dir, app, network) = super::network_fixture::fixture();
    let mut entry = directory("consent-workspace", "Consent space", "active", "visible");
    entry["source"] = "consent".into();
    save_fixture(
        &app,
        json!({"accounts":[team("a","account-1",None)],"workspaceDirectory":[entry.clone()]}),
    );
    *network.metadata.lock().unwrap() = vec![];
    assert_eq!(app.sync_workspace_directory().await.unwrap(), [entry]);
}
#[tokio::test]
async fn testSyncWorkspaceDirectoryMergesWorkspaceMetadataAcrossAllEligibleAccounts() {
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[team("a","account-1",None),team("b","account-2",None)]}),
    );
    network.metadata_by_token.lock().unwrap().insert(
        "token-a".into(),
        vec![metadata("account-3", "Three", "workspace")],
    );
    network.metadata_by_token.lock().unwrap().insert(
        "token-b".into(),
        vec![metadata("account-4", "Four", "workspace")],
    );
    let entries = app.sync_workspace_directory().await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries
            .iter()
            .map(|v| v["workspaceId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["account-4", "account-3"]
    );
    assert_eq!(network.calls.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn testSyncWorkspaceDirectoryRestoresVisibilityWhenWorkspaceBecomesActive() {
    // Despite the historic test name, original assertions preserve deleted + deactivated.
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[team("a","account-1",None)],"workspaceDirectory":[directory("account-2","ops-space","deactivated","deleted")]}),
    );
    *network.metadata.lock().unwrap() = vec![metadata("account-2", "ops-space", "workspace")];
    let entries = app.sync_workspace_directory().await.unwrap();
    assert_eq!(entries[0]["status"], "deactivated");
    assert_eq!(entries[0]["visibility"], "deleted");
    assert_eq!(entries[0]["source"], "legacyMetadata");
    assert_eq!(entries[0]["lastStatusCheckedAt"], 2);
    assert!(entries[0]["lastSeenAt"].as_i64().unwrap() > 1);
}
#[tokio::test]
async fn testRefreshUsageMarksDeactivatedAccountWithoutWritingWorkspaceDirectoryEntry() {
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[team("a","account-1",Some("team-space"))]}),
    );
    *network.usage_error.lock().unwrap() = Some("workspace_deactivated".into());
    assert!(app.refresh_usage("a").await.is_err());
    let a = app.accounts().await.unwrap().remove(0);
    assert_eq!(a.workspace_status, "deactivated");
    assert_eq!(a.display_status, "deactivated");
    assert!(!a.available);
    assert!(app.workspace_directory().await.unwrap().is_empty());
}
#[tokio::test]
async fn testImportAccountRemovesStoredWorkspaceDirectoryEntryForAuthorizedWorkspace() {
    let (_dir, app, _) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[],"workspaceDirectory":[directory("account-1","Team one","active","visible"),directory("account-2","Team two","active","visible")]}),
    );
    app.import_json(&team("a", "account-1", None)["authJson"].to_string())
        .await
        .unwrap();
    let entries = app.workspace_directory().await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["workspaceId"], "account-2");
}
#[tokio::test]
async fn testDeleteDeactivatedPendingAccountMarksUnifiedAccountDeleted() {
    let (_dir, app, _) = super::network_fixture::fixture();
    let mut a = team("a", "account-1", Some("Team one"));
    a["displayStatus"] = "deactivated".into();
    a["workspaceStatus"] = "deactivated".into();
    save_fixture(&app, json!({"accounts":[a]}));
    app.delete_account("a").await.unwrap();
    let stored = crate::repository::load_accounts(&app.paths.store()).unwrap();
    assert_eq!(stored.accounts.len(), 1);
    assert_eq!(stored.accounts[0].extra["displayStatus"], "deleted");
    assert_eq!(
        app.workspace_directory().await.unwrap()[0]["visibility"],
        "deleted"
    );
}
#[tokio::test]
async fn testDeletingDeactivatedPendingAccountAlsoDismissesWorkspaceFromFutureDiscovery() {
    let (_dir, app, network) = super::network_fixture::fixture();
    let mut dead = team("b", "account-2", Some("Team two"));
    dead["displayStatus"] = "deactivated".into();
    save_fixture(&app, json!({"accounts":[team("a","account-1",None),dead]}));
    app.delete_account("b").await.unwrap();
    *network.metadata.lock().unwrap() = vec![metadata("account-2", "Team two", "workspace")];
    let entries = app.sync_workspace_directory().await.unwrap();
    assert_eq!(entries[0]["visibility"], "deleted");
    assert_eq!(entries[0]["status"], "deactivated");
    assert_eq!(
        app.accounts()
            .await
            .unwrap()
            .iter()
            .find(|a| a.id == "b")
            .unwrap()
            .display_status,
        "deleted"
    );
}
#[tokio::test]
async fn testTrayMenuModelStartBackgroundRefreshRefreshesUsageOnRecurringTick() {
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[stored("a","a","a")],"currentAccountId":"a"}),
    );
    app.refresh_recurring(1).await.unwrap();
    assert_eq!(network.calls.lock().unwrap().len(), 1);
    assert!(app.accounts().await.unwrap()[0].usage.is_some());
}
#[tokio::test]
async fn testTrayMenuModelStartBackgroundRefreshDoesNotRunWorkspaceHealthCheckOnRecurringTick() {
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[team("a","a",None)],"currentAccountId":"a"}),
    );
    app.refresh_recurring(1).await.unwrap();
    assert!(network.calls.lock().unwrap().iter().all(|c| c.0 == "usage"));
    assert!(app.workspace_directory().await.unwrap().is_empty());
}
#[tokio::test]
async fn testTrayMenuModelRecurringRefreshDoesNotTriggerWorkspaceMetadataLookup() {
    let (_dir, app, network) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[team("a","a",None)],"currentAccountId":"a"}),
    );
    for tick in [0, 1, 6] {
        app.refresh_recurring(tick).await.unwrap();
    }
    assert_eq!(network.calls.lock().unwrap().len(), 3);
    assert!(network.calls.lock().unwrap().iter().all(|c| c.0 == "usage"));
}
#[tokio::test]
async fn testAddAccountViaLoginOnMacOSImportsSingleCurrentAccount() {
    let (_dir, app, network) = super::network_fixture::fixture();
    *network.metadata.lock().unwrap() = vec![
        metadata("account-1", "remote-space", "workspace"),
        metadata("account-2", "ops-space", "workspace"),
    ];
    app.import_authorized(team("a", "account-1", None)["authJson"].clone(), None, &[])
        .await
        .unwrap();
    let accounts = app.accounts().await.unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].account_id, "account-1");
    assert_eq!(accounts[0].team_name.as_deref(), Some("remote-space"));
    assert!(accounts[0].usage.is_some());
    assert_eq!(
        network
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.0 == "usage")
            .map(|c| c.1.clone())
            .collect::<Vec<_>>(),
        ["account-1"]
    );
    assert!(!app.paths.codex_auth.exists());
}
#[tokio::test]
async fn testAddAccountViaLoginReimportsCurrentAccountWithoutCreatingDuplicates() {
    let (_dir, app, network) = super::network_fixture::fixture();
    *network.metadata.lock().unwrap() = vec![metadata("account-1", "remote-space", "workspace")];
    for _ in 0..2 {
        app.import_authorized(team("a", "account-1", None)["authJson"].clone(), None, &[])
            .await
            .unwrap();
    }
    let accounts = app.accounts().await.unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].account_id, "account-1");
    assert_eq!(accounts[0].team_name.as_deref(), Some("remote-space"));
    assert_eq!(
        network
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.0 == "usage")
            .count(),
        2
    );
}
#[tokio::test]
async fn testAuthorizeWorkspaceViaLoginImportsSpecificWorkspace() {
    let (_dir, app, _) = super::network_fixture::fixture();
    save_fixture(
        &app,
        json!({"accounts":[],"workspaceDirectory":[directory("account-2","ops-space","active","visible")]}),
    );
    let document = team("a", "account-2", None)["authJson"].clone();
    app.import_authorized(document.clone(), Some("account-2"), &[])
        .await
        .unwrap();
    let accounts = app.accounts().await.unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].account_id, "account-2");
    assert_eq!(accounts[0].team_name.as_deref(), Some("ops-space"));
    assert!(app.workspace_directory().await.unwrap().is_empty());
    assert_eq!(
        app.import_authorized(document, Some("wrong-workspace"), &[])
            .await
            .unwrap_err(),
        "error.oauth.workspace_mismatch"
    );
}
#[tokio::test]
async fn testAddAccountViaLoginImportsAccountWhenWorkspaceMetadataLookupFails() {
    let (_dir, app, network) = super::network_fixture::fixture();
    *network.metadata_error.lock().unwrap() = Some("workspace metadata failed".into());
    app.import_authorized(team("a", "account-1", None)["authJson"].clone(), None, &[])
        .await
        .unwrap();
    let accounts = app.accounts().await.unwrap();
    assert_eq!(accounts[0].account_id, "account-1");
    assert!(accounts[0].team_name.is_none());
    assert_eq!(
        app.sync_workspace_directory().await.unwrap_err(),
        "workspace metadata failed"
    );
}
#[tokio::test]
async fn testTrayMenuModelAutoSmartSwitchWritesAuthAndLaunchesCodex() {
    let (_dir, app, _) = super::network_fixture::fixture();
    let mut a = stored("a", "a", "a");
    let mut b = stored("b", "b", "b");
    a["usage"] = json!({"fetchedAt":now_unix_seconds(),"fiveHour":{"usedPercent":100,"windowSeconds":18000},"oneWeek":{"usedPercent":100,"windowSeconds":604800}});
    b["usage"] = json!({"fetchedAt":now_unix_seconds(),"fiveHour":{"usedPercent":1,"windowSeconds":18000},"oneWeek":{"usedPercent":1,"windowSeconds":604800}});
    save_fixture(&app, json!({"accounts":[a,b],"currentAccountId":"a"}));
    app.save_preferences(Preferences {
        auto_switch: true,
        launch_codex_after_switch: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let events = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let observed = events.clone();
    app.on_switch_effects(std::sync::Arc::new(move |settings, auth| {
        let observed = observed.clone();
        Box::pin(async move {
            observed.lock().unwrap().push((
                settings.launch_codex_after_switch,
                crate::auth::extract_auth(&auth.unwrap())
                    .unwrap()
                    .account_id,
            ));
            Ok(Default::default())
        })
    }));
    assert_eq!(
        app.auto_switch_if_needed().await.unwrap().as_deref(),
        Some("b")
    );
    assert_eq!(*events.lock().unwrap(), [(true, "b".into())]);
    let auth: Value =
        serde_json::from_slice(&std::fs::read(&app.paths.codex_auth).unwrap()).unwrap();
    assert_eq!(crate::auth::extract_auth(&auth).unwrap().account_id, "b");
}
#[tokio::test]
async fn testRefreshUsageKeepsAuthSwitchedOutsideCopool() {
    let (_dir, app, _) = super::network_fixture::fixture();
    let mut a = stored("a", "a", "account-1");
    a["authJson"]["tokens"]["access_token"] = super::jwt(json!({"exp":1})).into();
    let b = stored("b", "b", "account-2");
    let b_auth = b["authJson"].clone();
    save_fixture(
        &app,
        json!({"accounts":[a.clone(),b],"currentAccountId":"a"}),
    );
    crate::persistence::write_json(&app.paths.codex_auth, &b_auth).unwrap();
    app.refresh_usage("a").await.unwrap();
    let current: Value =
        serde_json::from_slice(&std::fs::read(&app.paths.codex_auth).unwrap()).unwrap();
    assert_eq!(current, b_auth);

    crate::persistence::write_json(&app.paths.codex_auth, &a["authJson"]).unwrap();
    save_fixture(&app, json!({"accounts":[a],"currentAccountId":"a"}));
    app.refresh_usage("a").await.unwrap();
    let current: Value =
        serde_json::from_slice(&std::fs::read(&app.paths.codex_auth).unwrap()).unwrap();
    assert_eq!(current["tokens"]["access_token"], "refreshed-test-only");
}
