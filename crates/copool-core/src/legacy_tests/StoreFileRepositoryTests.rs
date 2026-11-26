use crate::{
    models::AccountsStore,
    repository::*,
    settings::{Provider, Providers, Settings},
};
use serde_json::{json, Value};
use std::{fs, path::Path};
fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}
fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn account(id: &str, workspace: &str) -> Value {
    json!({"id":id,"label":id,"email":format!("{id}@example.com"),"accountId":workspace,"authJson":{},"addedAt":1,"updatedAt":2})
}
fn cached() -> Value {
    json!({"id":42,"name":"Cached","accountType":"oauth","status":"active"})
}
#[test]
fn testLegacyAccountProxyMigratesToSettingsAndCanBeCleared() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("accounts.json");
    let settings = d.path().join("settings.json");
    let mut a = account("legacy", "legacy");
    a["proxyURL"] = "http://127.0.0.1:8080".into();
    write(&path, &json!({"accounts":[a]}));
    assert_eq!(
        load_accounts(&path).unwrap().accounts[0].proxy_url,
        "http://127.0.0.1:8080"
    );
    assert_eq!(
        load_settings(&settings).unwrap().account_proxy_urls["legacy"],
        "http://127.0.0.1:8080"
    );
    assert!(!fs::read_to_string(&path).unwrap().contains("proxyURL"));
    mutate_accounts(&path, |s| {
        s.accounts[0].proxy_url.clear();
        Ok(())
    })
    .unwrap();
    assert_eq!(load_accounts(&path).unwrap().accounts[0].proxy_url, "");
    assert_eq!(
        load_settings(&settings).unwrap().account_proxy_urls["legacy"],
        ""
    );
}
#[test]
fn testSub2APICacheIsKeptInTheAccountStoreAndSurvivesStaleAccountStoreSave() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("accounts.json");
    let settings_path = d.path().join("settings.json");
    let stale = load_accounts(&path).unwrap();
    let first = Provider {
        provider_id: "first".into(),
        imported_account_ids: vec![42],
        cached_accounts: vec![cached()],
        ..Default::default()
    };
    let second = Provider {
        provider_id: "second".into(),
        imported_account_ids: vec![42],
        cached_accounts: vec![cached()],
        ..Default::default()
    };
    let mut settings = Settings {
        sub2_api_provider: Providers {
            providers: vec![first.clone(), second.clone()],
            ..Default::default()
        },
        ..Default::default()
    };
    save_settings(&settings_path, &settings).unwrap();
    assert_eq!(load_settings(&settings_path).unwrap(), settings);
    assert!(!fs::read_to_string(&settings_path)
        .unwrap()
        .contains("cachedAccounts"));
    let store = load_accounts(&path).unwrap();
    assert_eq!(store.extra["cachedAccounts"][&first.id], json!([cached()]));
    assert_eq!(store.extra["cachedAccounts"][&second.id], json!([cached()]));
    save_accounts(&path, &stale).unwrap();
    assert_eq!(load_settings(&settings_path).unwrap(), settings);
    settings.sub2_api_provider.providers[0]
        .cached_accounts
        .clear();
    save_settings(&settings_path, &settings).unwrap();
    assert_eq!(load_settings(&settings_path).unwrap(), settings);
    settings.sub2_api_provider.providers.remove(0);
    save_settings(&settings_path, &settings).unwrap();
    let store = load_accounts(&path).unwrap();
    assert!(store.extra["cachedAccounts"].get(&first.id).is_none());
    assert_eq!(store.extra["cachedAccounts"][&second.id], json!([cached()]));
}
#[test]
fn codexProvidersSurviveStaleAccountStoreSave() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("accounts.json");
    let stale = load_accounts(&path).unwrap();
    let providers = json!([{"id": "p", "name": "my", "cachedAccounts": [cached()]}]);
    mutate_accounts(&path, |store| {
        store.extra.insert("providers".into(), providers.clone());
        Ok(())
    })
    .unwrap();
    // A store loaded before the providers were edited keeps them as edited.
    save_accounts(&path, &stale).unwrap();
    assert_eq!(load_accounts(&path).unwrap().extra["providers"], providers);
    // A save never writes them: only an edit of the store as it is now.
    let mut latest = load_accounts(&path).unwrap();
    latest.extra.insert("providers".into(), json!([]));
    save_accounts(&path, &latest).unwrap();
    assert_eq!(load_accounts(&path).unwrap().extra["providers"], providers);
    mutate_accounts(&path, |store| {
        store.extra.remove("providers");
        Ok(())
    })
    .unwrap();
    save_accounts(&path, &stale).unwrap();
    assert!(load_accounts(&path)
        .unwrap()
        .extra
        .get("providers")
        .is_none());
}
#[test]
fn testAccountProxyReloadsFromDiskIntoCardSnapshotAfterRepositoryRestart() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("accounts.json");
    let settings_path = d.path().join("settings.json");
    let mut a = account("a", "a");
    a["proxyURL"] = "socks5://127.0.0.1:1080".into();
    let store: AccountsStore = serde_json::from_value(json!({"accounts":[a]})).unwrap();
    save_accounts(&path, &store).unwrap();
    assert!(!fs::read_to_string(&path).unwrap().contains("proxyURL"));
    let stale = load_settings(&settings_path).unwrap();
    assert_eq!(stale.account_proxy_urls["a"], "socks5://127.0.0.1:1080");
    assert_eq!(
        load_accounts(&path).unwrap().account_summaries()[0].proxy_url,
        "socks5://127.0.0.1:1080"
    );
    mutate_accounts(&path, |s| {
        s.accounts[0].proxy_url = "http://127.0.0.1:8080".into();
        Ok(())
    })
    .unwrap();
    save_settings(&settings_path, &stale).unwrap();
    assert_eq!(
        load_accounts(&path).unwrap().account_summaries()[0].proxy_url,
        "http://127.0.0.1:8080"
    );
}
fn corruption(raw: &[u8]) {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("accounts.json");
    fs::write(&path, raw).unwrap();
    let value = load_accounts(&path).unwrap();
    assert!(value.accounts.is_empty());
    assert_eq!(value.version, 1);
    assert!(read(&path)["accounts"].as_array().unwrap().is_empty());
    let backups = fs::read_dir(d.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("accounts.corrupt-")
        })
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read(backups[0].path()).unwrap(), raw);
}
#[test]
fn testLoadStoreTreatsTrailingGarbageAsCorruption() {
    corruption(b"{\"version\":1,\"accounts\":[],\"settings\":{\"locale\":\"zh-CN\"}}\nINVALID");
}
#[test]
fn testLoadStoreBacksUpInvalidStoreAndResetsPrimaryStore() {
    corruption(b"{\"version\":1,\"accounts\":[");
}
#[test]
fn testLoadStoreUsesCurrentAccountIDAsSingleCurrentSource() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("accounts.json");
    write(
        &path,
        &json!({"accounts":[account("acct-1","account-1"),account("acct-2","account-2")],"currentAccountId":"acct-2"}),
    );
    assert_eq!(
        load_accounts(&path)
            .unwrap()
            .account_summaries()
            .iter()
            .filter(|a| a.is_current)
            .map(|a| a.id.as_str())
            .collect::<Vec<_>>(),
        ["acct-2"]
    );
}
#[test]
fn testLoadStoreDefaultsMissingWorkspaceStatusToActive() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("accounts.json");
    write(
        &path,
        &json!({"accounts":[account("acct-1","legacy-account")]}),
    );
    let store = load_accounts(&path).unwrap();
    assert_eq!(store.accounts.len(), 1);
    assert!(store.accounts[0].is_available());
    assert!(!fs::read_dir(d.path())
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e
            .file_name()
            .to_string_lossy()
            .starts_with("accounts.corrupt-")));
}
#[test]
fn testStoreRoundTripsWorkspaceDirectoryEntries() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("accounts.json");
    let entries = json!([{"workspaceId":"workspace-1","workspaceName":"Workspace One","email":"team@example.com","planType":"team","kind":"workspace","status":"deactivated","visibility":"deleted","lastSeenAt":123,"lastStatusCheckedAt":456}]);
    let store: AccountsStore =
        serde_json::from_value(json!({"accounts":[],"workspaceDirectory":entries})).unwrap();
    save_accounts(&path, &store).unwrap();
    assert_eq!(
        load_accounts(&path).unwrap().extra["workspaceDirectory"],
        entries
    );
}
#[test]
fn testAccountSummariesMarkOnlyMatchingVariantAsCurrent() {
    let mut a = account("acct-1", "account-1");
    a["principalId"] = "principal-1".into();
    let mut b = account("acct-2", "account-1");
    b["principalId"] = "principal-2".into();
    let store:AccountsStore=serde_json::from_value(json!({"accounts":[a,b],"currentAccountId":"acct-2","currentSelection":{"cardId":"acct-2","selectedAt":123,"sourceDeviceID":"device-a"}})).unwrap();
    let summaries = store.account_summaries();
    assert_eq!(summaries.iter().filter(|a| a.is_current).count(), 1);
    assert_eq!(
        summaries.iter().find(|a| a.is_current).unwrap().id,
        "acct-2"
    );
}
#[test]
fn testAccountSummariesUseCurrentAccountIDAsSingleCurrentSource() {
    let store:AccountsStore=serde_json::from_value(json!({"accounts":[account("acct-1","remote-account"),account("acct-2","local-account")],"currentAccountId":"acct-1","currentSelection":{"cardId":"remote-account","selectedAt":123,"sourceDeviceID":"device-a"}})).unwrap();
    assert_eq!(
        store
            .account_summaries()
            .iter()
            .filter(|a| a.is_current)
            .map(|a| a.id.as_str())
            .collect::<Vec<_>>(),
        ["acct-1"]
    );
}
/// A remote node's data dir: each deploy uploads `accounts.json` with every
/// account's proxy inline, while the node's own `settings.json` survives.
#[test]
fn uploadedAccountProxyReplacesTheOneTheNodeKept() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("accounts.json");
    let settings_path = d.path().join("settings.json");
    let upload = |proxy: &str| {
        let mut a = account("a", "a");
        a["proxyURL"] = proxy.into();
        let store: AccountsStore = serde_json::from_value(json!({"accounts":[a]})).unwrap();
        fs::write(&path, serde_json::to_vec_pretty(&store).unwrap()).unwrap();
    };
    upload("socks5://127.0.0.1:1080");
    assert_eq!(
        load_accounts(&path).unwrap().accounts[0].proxy_url,
        "socks5://127.0.0.1:1080"
    );
    upload("http://127.0.0.1:8080");
    assert_eq!(
        load_accounts(&path).unwrap().accounts[0].proxy_url,
        "http://127.0.0.1:8080"
    );
    assert_eq!(
        load_settings(&settings_path).unwrap().account_proxy_urls["a"],
        "http://127.0.0.1:8080"
    );
    assert!(!fs::read_to_string(&path).unwrap().contains("proxyURL"));
    upload("");
    assert_eq!(load_accounts(&path).unwrap().accounts[0].proxy_url, "");
    assert_eq!(
        load_settings(&settings_path).unwrap().account_proxy_urls["a"],
        ""
    );
    // Once moved, the node's settings keep the proxy across restarts.
    upload("http://127.0.0.1:8080");
    load_accounts(&path).unwrap();
    assert_eq!(
        load_accounts(&path).unwrap().accounts[0].proxy_url,
        "http://127.0.0.1:8080"
    );
}
