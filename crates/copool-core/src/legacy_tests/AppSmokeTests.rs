use super::{auth, network_fixture::fixture, save_fixture, stored};
use crate::{utils::now_unix_seconds, Preferences};
use serde_json::{json, Value};
#[tokio::test]
async fn testAccountsPageLoadSwitchAndSettingsFlow() {
    let (_dir, app, _) = fixture();
    app.import_json(
        &json!([auth("first", "workspace-1"), auth("second", "workspace-2")]).to_string(),
    )
    .await
    .unwrap();
    let accounts = app.accounts().await.unwrap();
    assert_eq!(accounts.len(), 2);
    let target = accounts
        .iter()
        .find(|a| a.account_id == "workspace-2")
        .unwrap()
        .id
        .clone();
    app.switch_account(&target).await.unwrap();
    let rows = app.accounts().await.unwrap();
    assert_eq!(rows.iter().find(|a| a.is_current).unwrap().id, target);
    let mut preferences = app.preferences().unwrap();
    preferences.locale = "zh-Hans".into();
    preferences.auto_switch = true;
    preferences.usage_display = "remaining".into();
    app.save_preferences(preferences).await.unwrap();
    let paths = app.paths.clone();
    drop(app);
    let reloaded = crate::Application::open(paths).unwrap();
    assert_eq!(reloaded.preferences().unwrap().locale, "zh-Hans");
    assert!(reloaded.preferences().unwrap().auto_switch);
    assert_eq!(reloaded.preferences().unwrap().usage_display, "remaining");
    assert_eq!(
        reloaded
            .accounts()
            .await
            .unwrap()
            .iter()
            .find(|a| a.is_current)
            .unwrap()
            .id,
        target
    );
}
fn pool() -> Value {
    let mut first = stored("first", "first", "one");
    let mut second = stored("second", "second", "two");
    first["usage"] = json!({"fetchedAt":now_unix_seconds(),"fiveHour":{"usedPercent":100,"windowSeconds":18000},"oneWeek":{"usedPercent":100,"windowSeconds":604800}});
    second["usage"] = json!({"fetchedAt":now_unix_seconds(),"fiveHour":{"usedPercent":10,"windowSeconds":18000},"oneWeek":{"usedPercent":20,"windowSeconds":604800}});
    json!({"accounts":[first,second],"currentAccountId":"first"})
}
fn current(app: &crate::Application) -> String {
    let document: Value =
        serde_json::from_slice(&std::fs::read(&app.paths.codex_auth).unwrap()).unwrap();
    crate::auth::extract_auth(&document).unwrap().account_id
}
#[tokio::test]
async fn testAccountsPageSmartSwitchUpdatesCurrentStateAndAuth() {
    let (_dir, app, _) = fixture();
    save_fixture(&app, pool());
    app.smart_switch().await.unwrap();
    assert_eq!(current(&app), "two");
    assert_eq!(
        app.accounts()
            .await
            .unwrap()
            .iter()
            .find(|a| a.is_current)
            .unwrap()
            .id,
        "second"
    );
    assert_eq!(
        crate::repository::load_accounts(&app.paths.store())
            .unwrap()
            .current_selection
            .unwrap()
            .account_id,
        "second"
    );
}
#[tokio::test]
async fn testProxyPageStartAndStopFlow() {
    let (_dir, app, _) = fixture();
    save_fixture(&app, json!({"accounts":[stored("a","a","a")]}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    app.save_preferences(Preferences {
        proxy_port: port,
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(!app.proxy_status().await.unwrap().running);
    let running = app.start_proxy().await.unwrap();
    assert!(running.running);
    assert_eq!(running.port, Some(port));
    assert!(running.api_key.is_some());
    assert!(app.proxy_status().await.unwrap().running);
    let stopped = app.stop_proxy().await.unwrap();
    assert!(!stopped.running);
    assert!(!app.proxy_status().await.unwrap().running);
}
#[tokio::test]
async fn testTrayAutoSmartSwitchUpdatesAccountsPageCurrentState() {
    let (_dir, app, _) = fixture();
    save_fixture(&app, pool());
    app.save_preferences(Preferences {
        auto_switch: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let notifications = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let changed = notifications.clone();
    app.on_change(std::sync::Arc::new(move || {
        changed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    assert_eq!(
        app.auto_switch_if_needed().await.unwrap().as_deref(),
        Some("second")
    );
    assert_eq!(current(&app), "two");
    assert!(notifications.load(std::sync::atomic::Ordering::SeqCst) > 0);
    assert_eq!(
        app.accounts()
            .await
            .unwrap()
            .iter()
            .find(|a| a.is_current)
            .unwrap()
            .id,
        "second"
    );
}
