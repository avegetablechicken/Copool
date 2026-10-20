use super::{fixture, http_fixture::serve, save_fixture, stored};
use crate::{
    proxy_integration, proxy_service as proxy, settings::Provider, ApiProxyStatus, Application,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
fn setup_provider(app: &Application, id: &str, base: &str, profile: bool) -> Provider {
    std::fs::create_dir_all(app.codex_config_path().parent().unwrap()).unwrap();
    std::fs::write(
        app.codex_config_path(),
        format!(
            "model_provider = {:?}\n",
            if profile { "openai" } else { id }
        ),
    )
    .unwrap();
    let path = if profile {
        app.codex_config_path().with_file_name("work.config.toml")
    } else {
        app.codex_config_path()
    };
    let original = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(path,format!("{original}\n[model_providers.{id}]\nbase_url = {base:?}\nwire_api = \"responses\"\nenv_key = \"PROVIDER_TEST_KEY\"\nrequires_openai_auth = false\n")).unwrap();
    let p = Provider {
        provider_id: id.into(),
        username: "admin@example.test".into(),
        allow_insecure_tls: true,
        imported_account_ids: vec![42, 43],
        ..Default::default()
    };
    let mut settings = app.configuration().unwrap();
    settings.sub2_api_provider.providers.push(p.clone());
    app.persist_configuration(&settings).unwrap();
    p
}
#[tokio::test]
async fn testLoadCandidatesIncludesImportedSub2APIProviderUsingLoginShellFallback() {
    let (_dir, app) = fixture();
    let mut p = setup_provider(&app, "my", "https://sub2.test/v1", false);
    p.proxy_url = "socks5://127.0.0.1:1080".into();
    let mut settings = app.configuration().unwrap();
    settings.sub2_api_provider.upsert(p);
    app.persist_configuration(&settings).unwrap();
    let candidates = proxy_integration::candidates_with_keys(&app.paths, |d| {
        assert_eq!(d.env_key.as_deref(), Some("PROVIDER_TEST_KEY"));
        Some("provider-api-key".into())
    })
    .unwrap();
    assert_eq!(candidates.len(), 1);
    let c = &candidates[0];
    assert_eq!(c.label, "my");
    assert_eq!(c.auth_json["copoolProvider"]["apiKey"], "provider-api-key");
    assert_eq!(c.auth_json["copoolProvider"]["preferred"], true);
    assert_eq!(c.auth_json["copoolProvider"]["allowInsecureTLS"], true);
    assert_eq!(c.proxy_url, "socks5://127.0.0.1:1080");
    assert_eq!(
        c.auth_json["copoolProvider"]["baseUrl"],
        "https://sub2.test/v1"
    );
}
#[tokio::test]
async fn testLoadCandidatesIncludesSub2APIProviderFromProfileConfig() {
    let (_dir, app) = fixture();
    setup_provider(
        &app,
        "profile-provider",
        "https://profile-sub2.test/v1",
        true,
    );
    let candidates =
        proxy_integration::candidates_with_keys(&app.paths, |_| Some("provider-api-key".into()))
            .unwrap();
    assert_eq!(candidates.len(), 1);
    let c = &candidates[0];
    assert_eq!(c.label, "profile-provider");
    assert_eq!(c.auth_json["copoolProvider"]["preferred"], false);
    assert_eq!(
        c.auth_json["copoolProvider"]["baseUrl"],
        "https://profile-sub2.test/v1"
    );
    assert_eq!(c.auth_json["copoolProvider"]["apiKey"], "provider-api-key");
}
async fn start(app: &Application, upstream: Option<&str>) -> ApiProxyStatus {
    let mut storage = app.storage();
    // Only an explicit upstream replaces the one resolved for this app: clearing
    // it would send the usage refresh to whatever `~/.codex/config.toml` names.
    if let Some(upstream) = upstream {
        storage.upstream_base_url = Some(upstream.to_owned());
    }
    proxy::start_api_proxy_with_runtime(&storage, &app.state.api_proxy, Some(0), "127.0.0.1")
        .await
        .unwrap()
}
async fn request(status: &ApiProxyStatus) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/v1/responses",
            status.port.unwrap()
        ))
        .bearer_auth(status.api_key.as_ref().unwrap())
        .json(&json!({"model":"gpt-5","input":"hello"}))
        .send()
        .await
        .unwrap()
}
fn complete() -> String {
    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r\",\"object\":\"response\",\"model\":\"gpt-5\",\"output\":[]}}\n\n".into()
}
#[tokio::test]
async fn testModelProviderCandidateUsesProviderResponsesEndpointWithoutChatGPTAccountHeader() {
    let (_dir, app) = fixture();
    let seen = Arc::new(Mutex::new(vec![]));
    let requests = seen.clone();
    let server = serve(move |r| {
        requests.lock().unwrap().push((r.path, r.headers));
        (axum::http::StatusCode::OK, complete())
    })
    .await;
    setup_provider(&app, "my", &format!("{}/v1", server.base), false);
    let mut storage = app.storage();
    let paths = app.paths.clone();
    storage.provider_candidates = Some(Arc::new(move || {
        proxy_integration::candidates_with_keys(&paths, |_| Some("provider-api-key".into()))
    }));
    let status =
        proxy::start_api_proxy_with_runtime(&storage, &app.state.api_proxy, Some(0), "127.0.0.1")
            .await
            .unwrap();
    assert_eq!(request(&status).await.status(), 200);
    let records = seen.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].0, "/v1/responses");
    assert_eq!(records[0].1["authorization"], "Bearer provider-api-key");
    assert!(!records[0].1.contains_key("chatgpt-account-id"));
    drop(records);
    app.stop_proxy().await.unwrap();
}
#[tokio::test]
async fn testRecordSuccessfulProviderCandidateUsesProviderSwitchHandler() {
    let (_dir, app) = fixture();
    setup_provider(&app, "my", "https://unused.invalid/v1", true);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    app.on_change(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    let auth = json!({"copoolProvider":{"id":"my"}});
    app.storage().on_selected.unwrap()("sub2api-provider:my".into(), auth.clone())
        .await
        .unwrap();
    assert_eq!(
        crate::codex_config::resolve_path(&app.codex_config_path()).id,
        "my"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!app.paths.codex_auth.exists());
}
#[tokio::test]
async fn testRecordSuccessfulCandidateCallsStoreChangeHandler() {
    let (_dir, app) = fixture();
    save_fixture(&app, json!({"accounts":[stored("a","a","a")]}));
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    app.on_change(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    app.storage().on_selected.unwrap()("a".into(), super::auth("a", "a"))
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(app.accounts().await.unwrap()[0].is_current, true);
}
#[tokio::test]
async fn testRecordSuccessfulCandidateUsesSuccessfulCandidateAsCurrentSelection() {
    let (_dir, app) = fixture();
    save_fixture(
        &app,
        json!({"accounts":[stored("a","a","a"),stored("b","b","b")],"currentAccountId":"a"}),
    );
    app.storage().on_selected.unwrap()("b".into(), super::auth("b", "b"))
        .await
        .unwrap();
    let store = crate::repository::load_accounts(&app.paths.store()).unwrap();
    assert_eq!(store.extra["currentAccountId"], "b");
    assert_eq!(store.current_selection.unwrap().account_id, "b");
    let auth: Value =
        serde_json::from_slice(&std::fs::read(&app.paths.codex_auth).unwrap()).unwrap();
    assert_eq!(crate::auth::extract_auth(&auth).unwrap().account_id, "b");
}
#[tokio::test]
async fn testHostAPIOnlyDisablesCurrentAuthSyncAfterSuccessfulProxyResponse() {
    let (_dir, app) = fixture();
    save_fixture(&app, json!({"accounts":[stored("a","a","a")]}));
    let mut settings = app.configuration().unwrap();
    settings.local_proxy_host_api_only = true;
    app.persist_configuration(&settings).unwrap();
    assert!(!proxy_integration::select(&app.paths, "a", &super::auth("a", "a")).unwrap());
    assert!(!app.paths.codex_auth.exists());
    settings.local_proxy_host_api_only = false;
    app.persist_configuration(&settings).unwrap();
    assert!(proxy_integration::select(&app.paths, "a", &super::auth("a", "a")).unwrap());
    assert!(app.paths.codex_auth.exists());
}
#[tokio::test]
async fn testHostAPIOnlyRecordsActiveCandidateWithoutSwitchingSelection() {
    let (_dir, app) = fixture();
    save_fixture(&app, json!({"accounts":[stored("a","a","a")]}));
    let mut settings = app.configuration().unwrap();
    settings.local_proxy_host_api_only = true;
    app.persist_configuration(&settings).unwrap();
    let server = serve(|_| (axum::http::StatusCode::OK, complete())).await;
    let status = start(&app, Some(&server.base)).await;
    assert_eq!(request(&status).await.status(), 200);
    let state = app.proxy_status().await.unwrap();
    assert_eq!(state.active_account_id.as_deref(), Some("a"));
    assert!(!app.paths.codex_auth.exists());
    assert!(crate::repository::load_accounts(&app.paths.store())
        .unwrap()
        .current_selection
        .is_none());
    app.stop_proxy().await.unwrap();
}
#[tokio::test]
async fn testRepeatedSuccessForCurrentAccountDoesNotResyncSelection() {
    let (_dir, app) = fixture();
    save_fixture(&app, json!({"accounts":[stored("a","a","a")]}));
    let counter = Arc::new(AtomicUsize::new(0));
    let count = counter.clone();
    app.on_change(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));
    let storage = app.storage();
    let callback = storage.on_selected.unwrap();
    callback("a".into(), super::auth("a", "a")).await.unwrap();
    let before = std::fs::read(&app.paths.codex_auth).unwrap();
    callback("a".into(), super::auth("a", "a")).await.unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(&app.paths.codex_auth).unwrap(), before);
}
#[tokio::test]
async fn testRepeatedSuccessForCurrentProviderDoesNotResyncProvider() {
    let (_dir, app) = fixture();
    setup_provider(&app, "my", "https://unused.invalid/v1", false);
    let before = std::fs::read(app.codex_config_path()).unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let count = counter.clone();
    app.on_change(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));
    for _ in 0..2 {
        app.storage().on_selected.unwrap()(
            "sub2api-provider:my".into(),
            json!({"copoolProvider":{"id":"my"}}),
        )
        .await
        .unwrap();
    }
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(app.codex_config_path()).unwrap(), before);
}
#[tokio::test]
async fn testHealthAndModelsEndpoints() {
    let (_dir, app) = fixture();
    save_fixture(&app, json!({"accounts":[stored("a","a","a")]}));
    let started = start(&app, None).await;
    assert!(started.running);
    assert!(started.api_key.as_ref().unwrap().starts_with("sk-"));
    let base = format!("http://127.0.0.1:{}", started.port.unwrap());
    let client = reqwest::Client::new();
    let health = client.get(format!("{base}/health")).send().await.unwrap();
    assert_eq!(health.status(), 200);
    assert_eq!(health.json::<Value>().await.unwrap()["ok"], true);
    let models = client
        .get(format!("{base}/v1/models"))
        .bearer_auth(started.api_key.unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(models.status(), 200);
    assert!(!models.json::<Value>().await.unwrap()["data"]
        .as_array()
        .unwrap()
        .is_empty());
    app.stop_proxy().await.unwrap();
}
#[tokio::test]
async fn testStartKeepsLegacyPersistedAPIKey() {
    let (_dir, app) = fixture();
    save_fixture(&app, json!({"accounts":[stored("a","a","a")]}));
    std::fs::write(app.paths.data_dir.join("api-proxy.key"), "sk-legacy-test\n").unwrap();
    let started = start(&app, None).await;
    assert_eq!(started.api_key.as_deref(), Some("sk-legacy-test"));
    app.stop_proxy().await.unwrap();
    let restarted = start(&app, None).await;
    assert_eq!(restarted.api_key.as_deref(), Some("sk-legacy-test"));
    app.stop_proxy().await.unwrap();
}
#[tokio::test]
async fn testResponsesRejectsMissingModel() {
    let (_dir, app) = fixture();
    save_fixture(&app, json!({"accounts":[stored("a","a","a")]}));
    let status = start(&app, None).await;
    let response = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/v1/responses",
            status.port.unwrap()
        ))
        .bearer_auth(status.api_key.unwrap())
        .json(&json!({"input":"hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert!(response
        .json::<Value>()
        .await
        .unwrap()
        .get("error")
        .is_some());
    app.stop_proxy().await.unwrap();
}
