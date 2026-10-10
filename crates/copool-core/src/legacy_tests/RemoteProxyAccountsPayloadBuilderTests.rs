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
    let built: Value = serde_json::from_slice(&accounts_payload(&path).unwrap()).unwrap();
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
    let built: Value = serde_json::from_slice(&accounts_payload(&path).unwrap()).unwrap();
    assert!(built["currentAccountId"].is_null());
    assert!(built["currentSelection"].is_null());
}
