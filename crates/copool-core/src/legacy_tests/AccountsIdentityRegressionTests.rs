use super::{auth, fixture, save_fixture, stored};
use serde_json::{json, Value};
#[tokio::test]
async fn testImportCurrentAuthKeepsDistinctPrincipalsForSameAccountID() {
    let (_dir, app) = fixture();
    for p in ["principal-1", "principal-2"] {
        crate::persistence::write_json(&app.paths.codex_auth, &auth(p, "shared-account")).unwrap();
        app.import_current().await.unwrap();
    }
    let saved: Value =
        serde_json::from_slice(&std::fs::read(app.paths.data_dir.join("accounts.json")).unwrap())
            .unwrap();
    let mut keys = saved["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            format!(
                "{}|{}",
                a["principalId"].as_str().unwrap(),
                a["accountId"].as_str().unwrap()
            )
        })
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        ["principal-1|shared-account", "principal-2|shared-account"]
    );
}
#[tokio::test]
async fn testListAccountsMarksOnlyCurrentMatchingVariant() {
    let (_dir, app) = fixture();
    save_fixture(
        &app,
        json!({"accounts":[stored("acct-1","first","shared-account"),stored("acct-2","second","shared-account")],"currentAccountId":"acct-2"}),
    );
    crate::persistence::write_json(&app.paths.codex_auth, &auth("second", "shared-account"))
        .unwrap();
    let accounts = app.accounts().await.unwrap();
    assert_eq!(accounts.iter().filter(|a| a.is_current).count(), 1);
    assert_eq!(
        accounts
            .iter()
            .find(|a| a.is_current)
            .unwrap()
            .email
            .as_deref(),
        Some("second@example.test")
    );
}
#[tokio::test]
async fn testListAccountsKeepsCurrentCardIDWhenSelectionIsStale() {
    let (_dir, app) = fixture();
    save_fixture(
        &app,
        json!({"accounts":[stored("acct-1","first","account-1"),stored("acct-2","second","account-2")],"currentAccountId":"acct-1","currentSelection":{"cardId":"acct-2","selectedAt":2000,"sourceDeviceId":"macos-local"}}),
    );
    let accounts = app.accounts().await.unwrap();
    assert_eq!(accounts.iter().filter(|a| a.is_current).count(), 1);
    assert_eq!(accounts.iter().find(|a| a.is_current).unwrap().id, "acct-1");
}
#[tokio::test]
async fn testListAccountsFollowsAuthChangedOutsideCopool() {
    let (_dir, app) = fixture();
    save_fixture(
        &app,
        json!({"accounts":[stored("acct-1","first","account-1"),stored("acct-2","second","account-2")],"currentAccountId":"acct-1","currentSelection":{"cardId":"acct-1","selectedAt":2000,"sourceDeviceId":"macos-local"}}),
    );
    crate::persistence::write_json(&app.paths.codex_auth, &auth("second", "account-2")).unwrap();
    let accounts = app.accounts().await.unwrap();
    assert_eq!(accounts.iter().filter(|a| a.is_current).count(), 1);
    assert_eq!(accounts.iter().find(|a| a.is_current).unwrap().id, "acct-2");
    let saved: Value =
        serde_json::from_slice(&std::fs::read(app.paths.data_dir.join("accounts.json")).unwrap())
            .unwrap();
    assert_eq!(saved["currentAccountId"], "acct-2");
    assert_eq!(saved["currentSelection"]["cardId"], "acct-2");
}
#[tokio::test]
async fn testListAccountsMarksNoneCurrentForUnknownAuth() {
    let (_dir, app) = fixture();
    save_fixture(
        &app,
        json!({"accounts":[stored("acct-1","first","account-1")],"currentAccountId":"acct-1"}),
    );
    crate::persistence::write_json(&app.paths.codex_auth, &auth("other", "account-9")).unwrap();
    let accounts = app.accounts().await.unwrap();
    assert!(accounts.iter().all(|a| !a.is_current));
}
#[tokio::test]
async fn testImportCurrentAuthPrefersExactPrincipalMatchOverLegacyWildcardRow() {
    let (_dir, app) = fixture();
    let mut legacy = stored("legacy", "principal-1", "shared-account");
    legacy["email"] = Value::Null;
    legacy["principalId"] = Value::Null;
    save_fixture(
        &app,
        json!({"accounts":[legacy,stored("exact","principal-2","shared-account")]}),
    );
    crate::persistence::write_json(
        &app.paths.codex_auth,
        &auth("principal-2", "shared-account"),
    )
    .unwrap();
    app.import_current_as(Some("Updated Exact")).await.unwrap();
    let saved: Value =
        serde_json::from_slice(&std::fs::read(app.paths.data_dir.join("accounts.json")).unwrap())
            .unwrap();
    let accounts = saved["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 2);
    assert_eq!(
        accounts.iter().find(|a| a["id"] == "exact").unwrap()["label"],
        "Updated Exact"
    );
    assert!(accounts.iter().find(|a| a["id"] == "legacy").unwrap()["email"].is_null());
}
