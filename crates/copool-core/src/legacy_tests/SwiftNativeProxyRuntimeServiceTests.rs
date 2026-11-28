use super::{fixture, http_fixture::serve, save_fixture, stored};
use crate::Pool;
use crate::{
    provider_registry::ProviderRecord, proxy_integration, proxy_service as proxy, ApiProxyStatus,
    Application,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
fn setup_provider(app: &Application, id: &str, base: &str, profile: bool) -> ProviderRecord {
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
    // Named after the `config.toml` provider, keyed by an id unlike it.
    let p = ProviderRecord {
        id: format!("id-{id}"),
        name: id.into(),
        username: "admin@example.test".into(),
        allow_insecure_tls: true,
        imported_account_ids: vec![42, 43],
        cached_accounts: [42, 43]
            .map(|id| {
                serde_json::from_value(
                    json!({"id": id, "name": "Cached", "accountType": "oauth", "status": "active"}),
                )
                .unwrap()
            })
            .into(),
        ..Default::default()
    };
    store_provider(app, &p);
    p
}
#[tokio::test]
async fn testLoadCandidatesSkipsAProviderNoneOfWhoseAccountsSub2APIStillServes() {
    let (_dir, app) = fixture();
    let mut p = setup_provider(&app, "my", "https://sub2.test/v1", false);
    for account in &mut p.cached_accounts {
        account.status = "missing".into();
    }
    store_provider(&app, &p);
    let candidates =
        proxy_integration::candidates_with_keys(&app.paths, |_| Some("provider-api-key".into()))
            .unwrap();
    assert!(candidates.is_empty());
}
/// Puts a provider in the Codex account store, in place of one with its id.
fn store_provider(app: &Application, p: &ProviderRecord) {
    let mut store = crate::codex::load(&app.paths).unwrap();
    store.providers.retain(|v| v.id != p.id);
    store.providers.push(p.clone());
    crate::persistence::write_json(&app.paths.data_dir.join(crate::codex::FILE), &store).unwrap();
}
#[tokio::test]
async fn testLoadCandidatesIncludesImportedSub2APIProviderUsingLoginShellFallback() {
    let (_dir, app) = fixture();
    let mut p = setup_provider(&app, "my", "https://sub2.test/v1", false);
    p.proxy_url = "socks5://127.0.0.1:1080".into();
    store_provider(&app, &p);
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
    let mut storage = app.proxy_storage(Pool::Codex);
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
    let mut storage = app.proxy_storage(Pool::Codex);
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
    app.stop_proxy(Pool::Codex).await.unwrap();
}
#[tokio::test]
async fn testRecordSuccessfulProviderCandidateUsesProviderSwitchHandler() {
    let (_dir, app) = fixture();
    let app = app.into_shared();
    setup_provider(&app, "my", "https://unused.invalid/v1", true);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    app.on_change(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    let auth = json!({"copoolProvider":{"id":"my"}});
    app.proxy_storage(Pool::Codex).on_selected.unwrap()("sub2api-provider:my".into(), auth.clone())
        .await
        .unwrap();
    assert_eq!(
        crate::codex::config::resolve_path(&app.codex_config_path()).id,
        "my"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!app.paths.codex_auth.exists());
}
#[tokio::test]
async fn testRecordSuccessfulCandidateCallsStoreChangeHandler() {
    let (_dir, app) = fixture();
    let app = app.into_shared();
    save_fixture(&app, json!({"accounts":[stored("a","a","a")]}));
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    app.on_change(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    app.proxy_storage(Pool::Codex).on_selected.unwrap()("a".into(), super::auth("a", "a"))
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(app.accounts(Pool::Codex).await.unwrap()[0].is_current, true);
}
#[tokio::test]
async fn testRecordSuccessfulCandidateUsesSuccessfulCandidateAsCurrentSelection() {
    let (_dir, app) = fixture();
    let app = app.into_shared();
    save_fixture(
        &app,
        json!({"accounts":[stored("a","a","a"),stored("b","b","b")],"currentAccountId":"a"}),
    );
    app.proxy_storage(Pool::Codex).on_selected.unwrap()("b".into(), super::auth("b", "b"))
        .await
        .unwrap();
    assert_eq!(
        app.codex_load().unwrap().current_account_id.as_deref(),
        Some("b")
    );
    let auth: Value =
        serde_json::from_slice(&std::fs::read(&app.paths.codex_auth).unwrap()).unwrap();
    assert_eq!(
        crate::pool_login::codex::extract_auth(&auth)
            .unwrap()
            .account_id,
        "b"
    );
}
#[tokio::test]
async fn testHostAPIOnlyDisablesCurrentAuthSyncAfterSuccessfulProxyResponse() {
    let (_dir, app) = fixture();
    save_fixture(&app, json!({"accounts":[stored("a","a","a")]}));
    let mut settings = app.configuration().unwrap();
    settings.local_proxy_host_api_only = true;
    app.persist_configuration(&settings).unwrap();
    assert!(!app
        .codex_follow_proxy("a", &super::auth("a", "a"))
        .await
        .unwrap());
    assert!(!app.paths.codex_auth.exists());
    settings.local_proxy_host_api_only = false;
    app.persist_configuration(&settings).unwrap();
    assert!(app
        .codex_follow_proxy("a", &super::auth("a", "a"))
        .await
        .unwrap());
    assert!(app.paths.codex_auth.exists());
}
#[tokio::test]
async fn testHostAPIOnlyRecordsActiveCandidateWithoutSwitchingSelection() {
    let (_dir, app) = fixture();
    // Shared, as the app is: the proxy follows its choice unless told not to.
    let app = app.into_shared();
    save_fixture(&app, json!({"accounts":[stored("a","a","a")]}));
    let mut settings = app.configuration().unwrap();
    settings.local_proxy_host_api_only = true;
    app.persist_configuration(&settings).unwrap();
    let server = serve(|_| (axum::http::StatusCode::OK, complete())).await;
    let status = start(&app, Some(&server.base)).await;
    assert_eq!(request(&status).await.status(), 200);
    let state = app.proxy_status(Pool::Codex).await.unwrap();
    assert_eq!(state.active_account_id.as_deref(), Some("a"));
    assert!(!app.paths.codex_auth.exists());
    assert!(app.codex_load().unwrap().current_account_id.is_none());
    app.stop_proxy(Pool::Codex).await.unwrap();
}
#[tokio::test]
async fn testRepeatedSuccessForCurrentAccountDoesNotResyncSelection() {
    let (_dir, app) = fixture();
    let app = app.into_shared();
    save_fixture(&app, json!({"accounts":[stored("a","a","a")]}));
    let counter = Arc::new(AtomicUsize::new(0));
    let count = counter.clone();
    app.on_change(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));
    let storage = app.proxy_storage(Pool::Codex);
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
    let app = app.into_shared();
    setup_provider(&app, "my", "https://unused.invalid/v1", false);
    let before = std::fs::read(app.codex_config_path()).unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let count = counter.clone();
    app.on_change(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));
    for _ in 0..2 {
        app.proxy_storage(Pool::Codex).on_selected.unwrap()(
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
    let seen = Arc::new(Mutex::new(Vec::new()));
    let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (log, failing) = (seen.clone(), fail.clone());
    let server = serve(move |request| {
        let header = |name: &str| {
            request
                .headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        if request.path.ends_with("/models") {
            log.lock().unwrap().push((
                request.uri.clone(),
                header("authorization"),
                header("chatgpt-account-id"),
                header("version"),
            ));
        }
        if failing.load(Ordering::SeqCst) {
            return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "{}".into());
        }
        let models = json!({"models": [{"slug": "gpt-6.2-sol", "priority": 1, "visibility": "list",
            "supported_in_api": true, "supported_reasoning_levels": [{"effort": "high"}]}]});
        (axum::http::StatusCode::OK, models.to_string())
    })
    .await;
    let started = start(&app, Some(&server.base)).await;
    assert!(started.running);
    assert!(started.api_key.as_ref().unwrap().starts_with("sk-"));
    let base = format!("http://127.0.0.1:{}", started.port.unwrap());
    let client = reqwest::Client::new();
    let health = client.get(format!("{base}/health")).send().await.unwrap();
    assert_eq!(health.status(), 200);
    assert_eq!(health.json::<Value>().await.unwrap()["ok"], true);
    let models = || async {
        let response = client
            .get(format!("{base}/v1/models"))
            .bearer_auth(started.api_key.as_ref().unwrap())
            .header("User-Agent", "codex_cli_rs/0.166.0")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let listed = response.json::<Value>().await.unwrap()["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        listed
    };
    // The upstream's list, asked as Codex asks with the account's login.
    assert_eq!(models().await, ["GPT-6.2-Sol", "GPT-6.2-Sol-High"]);
    assert_eq!(
        seen.lock().unwrap().clone(),
        [(
            "/backend-api/codex/models?client_version=0.166.0".to_owned(),
            "Bearer access-a-a".to_owned(),
            "a".to_owned(),
            "0.166.0".to_owned(),
        )]
    );
    // Within its lifetime the list is served without asking again.
    assert_eq!(models().await, ["GPT-6.2-Sol", "GPT-6.2-Sol-High"]);
    assert_eq!(seen.lock().unwrap().len(), 1);
    app.stop_proxy(Pool::Codex).await.unwrap();

    // Once the upstream fails, a proxy without a list it fetched says so.
    let started = start(&app, Some(&server.base)).await;
    fail.store(true, Ordering::SeqCst);
    let response = client
        .get(format!(
            "http://127.0.0.1:{}/v1/models",
            started.port.unwrap()
        ))
        .bearer_auth(started.api_key.unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 502);
    app.stop_proxy(Pool::Codex).await.unwrap();
}
#[tokio::test]
async fn testStartKeepsLegacyPersistedAPIKey() {
    let (_dir, app) = fixture();
    save_fixture(&app, json!({"accounts":[stored("a","a","a")]}));
    std::fs::write(app.paths.data_dir.join("api-proxy.key"), "sk-legacy-test\n").unwrap();
    let started = start(&app, None).await;
    assert_eq!(started.api_key.as_deref(), Some("sk-legacy-test"));
    app.stop_proxy(Pool::Codex).await.unwrap();
    let restarted = start(&app, None).await;
    assert_eq!(restarted.api_key.as_deref(), Some("sk-legacy-test"));
    app.stop_proxy(Pool::Codex).await.unwrap();
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
    app.stop_proxy(Pool::Codex).await.unwrap();
}
