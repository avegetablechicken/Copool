use super::stored;
use crate::remote::accounts_payload;
use serde_json::{json, Value};
#[test]
fn testBuildPreservesUsableAccountsFromStore() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("accounts.json");
    std::fs::write(
        &path,
        json!({"version":1,"accounts":[stored("a","test","acc-1")]}).to_string(),
    )
    .unwrap();
    std::fs::write(
        d.path().join("settings.json"),
        json!({"accountProxyURLs":{"a":"http://127.0.0.1:8080"}}).to_string(),
    )
    .unwrap();
    let built: Value = serde_json::from_slice(&accounts_payload(&path, None).unwrap()).unwrap();
    assert_eq!(built["version"], 1);
    assert_eq!(built["accounts"].as_array().unwrap().len(), 1);
    assert_eq!(built["accounts"][0]["accountId"], "acc-1");
    assert_eq!(built["accounts"][0]["email"], "test@example.test");
    assert_eq!(built["accounts"][0]["proxyURL"], "http://127.0.0.1:8080");
}
#[test]
fn testBuildClearsLocalCurrentSelectionForRemoteAutonomy() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("accounts.json");
    std::fs::write(&path,json!({"version":1,"accounts":[stored("acct-current","test","acc-1")],"currentAccountId":"acct-current","currentSelection":{"cardId":"acct-current","selectedAt":123,"sourceDeviceID":"macos-local"}}).to_string()).unwrap();
    let built: Value = serde_json::from_slice(&accounts_payload(&path, None).unwrap()).unwrap();
    assert!(built["currentAccountId"].is_null());
    assert!(built["currentSelection"].is_null());
}
#[test]
fn testBuildKeepsLoginsTheRemoteDaemonRenewed() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("accounts.json");
    let mut local = stored("a", "test", "acc-1");
    local["authJson"]["last_refresh"] = "2026-01-01T00:00:00Z".into();
    std::fs::write(
        &path,
        json!({"version":1,"accounts":[local.clone()]}).to_string(),
    )
    .unwrap();
    let mut renewed = local.clone();
    renewed["authJson"]["tokens"]["refresh_token"] = "renewed-by-daemon".into();
    renewed["authJson"]["last_refresh"] = "2099-01-01T00:00:00Z".into();
    let mut older = local.clone();
    older["authJson"]["tokens"]["refresh_token"] = "older".into();
    older["authJson"]["last_refresh"] = "2000-01-01T00:00:00Z".into();
    // Only the credentials of accounts kept locally come from the remote.
    let removed = stored("b", "removed", "acc-2");
    let kept = local["authJson"]["tokens"]["refresh_token"].clone();
    for (remote, expected) in [(renewed, json!("renewed-by-daemon")), (older, kept)] {
        let remote = json!({"accounts":[remote, removed.clone()]});
        let built: Value =
            serde_json::from_slice(&accounts_payload(&path, Some(&remote)).unwrap()).unwrap();
        assert_eq!(built["accounts"].as_array().unwrap().len(), 1);
        assert_eq!(
            built["accounts"][0]["authJson"]["tokens"]["refresh_token"],
            expected
        );
    }
}
