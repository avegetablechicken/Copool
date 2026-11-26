use super::{fixture, save_fixture, stored};
use serde_json::{json, Value};
async fn payload(app: &crate::Application, remote: Option<&Value>) -> Value {
    serde_json::from_slice(&app.codex_remote_payload(remote).await.unwrap()).unwrap()
}
#[tokio::test]
async fn testBuildPreservesUsableAccountsFromStore() {
    let (_d, app) = fixture();
    std::fs::write(
        app.paths.data_dir.join("settings.json"),
        json!({"accountProxyURLs":{"a":"http://127.0.0.1:8080"}}).to_string(),
    )
    .unwrap();
    save_fixture(
        &app,
        json!({"version":1,"accounts":[stored("a","test","acc-1")]}),
    );
    let built = payload(&app, None).await;
    assert_eq!(built["version"], 1);
    assert_eq!(built["accounts"].as_array().unwrap().len(), 1);
    assert_eq!(built["accounts"][0]["accountId"], "acc-1");
    assert_eq!(built["accounts"][0]["email"], "test@example.test");
    assert_eq!(built["accounts"][0]["proxyURL"], "http://127.0.0.1:8080");
}
#[tokio::test]
async fn testBuildClearsLocalCurrentSelectionForRemoteAutonomy() {
    let (_d, app) = fixture();
    save_fixture(
        &app,
        json!({"version":1,"accounts":[stored("acct-current","test","acc-1")],"currentAccountId":"acct-current"}),
    );
    let built = payload(&app, None).await;
    assert!(built["currentAccountId"].is_null());
    assert!(built["currentSelection"].is_null());
}
#[tokio::test]
async fn testBuildKeepsLoginsTheRemoteDaemonRenewed() {
    let mut local = stored("a", "test", "acc-1");
    local["authJson"]["last_refresh"] = "2026-01-01T00:00:00Z".into();
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
        let (_d, app) = fixture();
        save_fixture(&app, json!({"version":1,"accounts":[local.clone()]}));
        let remote = json!({"accounts":[remote, removed.clone()]});
        let built = payload(&app, Some(&remote)).await;
        assert_eq!(built["accounts"].as_array().unwrap().len(), 1);
        assert_eq!(
            built["accounts"][0]["authJson"]["tokens"]["refresh_token"],
            expected
        );
        // The pool keeps the node's renewal.
        assert_eq!(
            app.codex_load().unwrap().accounts[0].document["tokens"]["refresh_token"],
            expected
        );
    }
}
