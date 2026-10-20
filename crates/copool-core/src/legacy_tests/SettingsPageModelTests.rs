use super::{http_fixture::serve, MemorySecrets};
use crate::{
    credentials::SecretStore, settings::Provider, utils::now_unix_seconds, Application, Paths,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
fn fixture() -> (tempfile::TempDir, Application, Arc<MemorySecrets>) {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let app = Application::open_with_secrets(
        Paths {
            data_dir: dir.path().join("data"),
            codex_auth: dir.path().join("codex/auth.json"),
        },
        secrets.clone(),
    )
    .unwrap();
    (dir, app, secrets)
}
fn provider(name: &str) -> Provider {
    Provider {
        provider_id: name.into(),
        username: "admin@example.test".into(),
        password: "test-secret".into(),
        ..Default::default()
    }
}
fn cached(id: i64, provider: &str, used: f64) -> Value {
    json!({"id":id,"name":format!("account{id}"),"email":format!("account{id}@example.test"),"accountType":"oauth","status":"active","planType":"pro","providerID":provider,"usage":{"fetchedAt":now_unix_seconds(),"fiveHour":{"usedPercent":used,"windowSeconds":18000},"oneWeek":{"usedPercent":used,"windowSeconds":604800}}})
}
fn config(app: &Application, definitions: &[(&str, &str)], selected: &str) {
    std::fs::create_dir_all(app.codex_config_path().parent().unwrap()).unwrap();
    let mut text = format!("model_provider = {selected:?}\n");
    for (id, url) in definitions {
        text += &format!("[model_providers.{id}]\nbase_url = {url:?}\nwire_api = \"responses\"\n");
    }
    std::fs::write(app.codex_config_path(), text).unwrap();
}
#[tokio::test]
async fn testSaveSub2APIProviderStoresPasswordOutsideSettings() {
    let (_dir, app, secrets) = fixture();
    let mut draft = provider("my");
    draft.admin_base_url = " https://admin.test/api/v1/ ".into();
    draft.allow_insecure_tls = true;
    let saved = app.save_provider(draft).await.unwrap();
    assert_eq!(saved.password, "");
    assert_eq!(saved.admin_base_url, "https://admin.test/api/v1");
    assert_eq!(saved.username, "admin@example.test");
    assert_eq!(
        secrets.get(&saved.id).unwrap().as_deref(),
        Some("test-secret")
    );
    let raw = std::fs::read_to_string(app.paths.data_dir.join("settings.json")).unwrap();
    assert!(!raw.contains("test-secret"));
    let round: Provider = serde_json::from_value(serde_json::to_value(&saved).unwrap()).unwrap();
    assert_eq!(round.admin_base_url, saved.admin_base_url);
    let paths = app.paths.clone();
    drop(app);
    let reloaded = Application::open_with_secrets(paths, secrets).unwrap();
    let p = &reloaded
        .configuration()
        .unwrap()
        .sub2_api_provider
        .providers[0];
    assert_eq!(p.password, "");
    assert_eq!(p.admin_base_url, "https://admin.test/api/v1");
}
#[tokio::test]
async fn testEditingProviderPreservesOtherCredentialsWithoutKeychainAccess() {
    let (_dir, app, secrets) = fixture();
    let mut first = app.save_provider(provider("first")).await.unwrap();
    let second = app.save_provider(provider("second")).await.unwrap();
    secrets.gets.lock().unwrap().clear();
    secrets.sets.lock().unwrap().clear();
    secrets
        .deny_reads
        .store(true, std::sync::atomic::Ordering::SeqCst);
    first.admin_base_url = "https://admin.example.test".into();
    let mut first = app.save_provider(first).await.unwrap();
    assert!(secrets.gets.lock().unwrap().is_empty());
    assert!(secrets.sets.lock().unwrap().is_empty());
    assert_eq!(
        app.configuration().unwrap().sub2_api_provider.providers[1],
        second
    );
    first.password = "replacement".into();
    let first = app.save_provider(first).await.unwrap();
    assert_eq!(*secrets.sets.lock().unwrap(), [first.id.clone()]);
    assert!(secrets.gets.lock().unwrap().is_empty());
    assert_eq!(secrets.values.lock().unwrap()[&first.id], "replacement");
    assert_eq!(secrets.values.lock().unwrap()[&second.id], "test-secret");
    app.save_provider(first.clone()).await.unwrap();
    assert_eq!(*secrets.sets.lock().unwrap(), [first.id]);
    let mut new = provider("new");
    new.password.clear();
    assert_eq!(
        app.save_provider(new).await.unwrap_err(),
        "error.sub2api.configuration_incomplete"
    );
    assert_eq!(
        app.configuration()
            .unwrap()
            .sub2_api_provider
            .providers
            .len(),
        2
    );
}
#[tokio::test]
async fn testSettingsPageAddsAndSavesArbitrarySub2APIProviderConfigurations() {
    let (_dir, app, _) = fixture();
    for name in ["my", "ShareCoder", "custom-provider"] {
        app.save_provider(provider(name)).await.unwrap();
    }
    let saved = app.configuration().unwrap();
    assert_eq!(
        saved
            .sub2_api_provider
            .providers
            .iter()
            .map(|p| p.provider_id.as_str())
            .collect::<Vec<_>>(),
        ["my", "ShareCoder", "custom-provider"]
    );
    assert!(saved
        .sub2_api_provider
        .providers
        .iter()
        .all(|p| p.legacy_admin_base_url.is_empty()));
    assert!(!app.codex_config_path().exists());
}
#[test]
fn testLegacySub2APIConfigurationUsesCodexBaseURLDuringMigration() {
    let (_dir, app, secrets) = fixture();
    config(
        &app,
        &[
            ("my", "https://my-sub2.test:6060"),
            ("ShareCoder", "https://sharecoder.test"),
        ],
        "my",
    );
    let mut settings = serde_json::to_value(app.configuration().unwrap()).unwrap();
    let p = provider("my");
    settings["sub2APIProvider"] = json!({"confirmedProviderIDs":["ShareCoder","my"],"providers":[{"id":p.id,"providerID":"my","username":"admin@example.test","password":"secret","legacyAdminBaseURL":"https://sharecoder.test/api/v1","importedAccountIDs":[42],"cachedAccounts":[cached(42,"my",20.)]}]});
    std::fs::write(
        app.paths.data_dir.join("settings.json"),
        serde_json::to_vec(&settings).unwrap(),
    )
    .unwrap();
    app.migrate_provider_credentials().unwrap();
    let saved = app.configuration().unwrap();
    let p = &saved.sub2_api_provider.providers[0];
    assert_eq!(p.provider_id, "ShareCoder");
    assert_eq!(p.password, "");
    assert_eq!(secrets.get(&p.id).unwrap().as_deref(), Some("secret"));
    assert_eq!(p.legacy_admin_base_url, "");
    assert_eq!(p.cached_accounts[0]["providerID"], "ShareCoder");
    assert_eq!(
        saved.sub2_api_provider.confirmed_provider_ids,
        ["ShareCoder", "my"]
    );
}
#[tokio::test]
async fn testAccountsPageRestoresCachedSub2APIAccountsWithoutNetwork() {
    let (_dir, app, secrets) = fixture();
    let mut settings = app.configuration().unwrap();
    let mut p = provider("my");
    p.password.clear();
    p.imported_account_ids = vec![42];
    p.cached_accounts = vec![cached(42, "my", 20.)];
    settings.sub2_api_provider.providers = vec![p];
    app.persist_configuration(&settings).unwrap();
    secrets
        .deny_reads
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let cards = app.accounts().await.unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].id, "sub2api-my-42");
    assert_eq!(cards[0].source_tag.as_deref(), Some("SUB2API"));
    assert_eq!(
        cards[0]
            .usage
            .as_ref()
            .unwrap()
            .five_hour
            .as_ref()
            .unwrap()
            .used_percent,
        20.
    );
    assert!(secrets.gets.lock().unwrap().is_empty());
}
async fn upstream(
    ids: Arc<Mutex<Vec<i64>>>,
    calls: Arc<Mutex<Vec<String>>>,
) -> super::http_fixture::Server {
    serve(move|r|{calls.lock().unwrap().push(r.path.clone());let payload=if r.path.ends_with("/auth/login"){json!({"access_token":"admin-test"})}else if r.path.ends_with("/admin/accounts"){json!({"items":ids.lock().unwrap().iter().map(|id|json!({"id":id,"name":format!("account{id}@example.test"),"platform":"openai","type":"oauth","status":"active"})).collect::<Vec<_>>(),"pages":1})}else{json!({"email":"account@example.test","plan_type":"pro","rate_limit":{"primary_window":{"used_percent":20,"limit_window_seconds":18000,"reset_at":1800000000},"secondary_window":{"used_percent":30,"limit_window_seconds":604800,"reset_at":1800604800}}})};(axum::http::StatusCode::OK,json!({"code":0,"data":payload}).to_string())}).await
}
#[tokio::test]
async fn testAccountsPageImportsSub2APIAccountsAndPersistsIDs() {
    let (_dir, app, _) = fixture();
    let server = upstream(Arc::new(Mutex::new(vec![42])), Default::default()).await;
    config(&app, &[("my", &server.base)], "my");
    app.save_provider(provider("my")).await.unwrap();
    assert_eq!(app.refresh_provider("my", true).await.unwrap(), 1);
    let settings = app.configuration().unwrap();
    let p = &settings.sub2_api_provider.providers[0];
    assert_eq!(p.imported_account_ids, [42]);
    assert_eq!(p.cached_accounts[0]["id"], 42);
    assert_eq!(
        app.accounts().await.unwrap()[0].source_tag.as_deref(),
        Some("SUB2API")
    );
    assert_eq!(
        app.accounts().await.unwrap()[0]
            .usage
            .as_ref()
            .unwrap()
            .five_hour
            .as_ref()
            .unwrap()
            .used_percent,
        20.
    );
}
#[tokio::test]
async fn testAccountsPageSub2APIImportPreservesOtherProviderCaches() {
    let (_dir, app, _) = fixture();
    let server = upstream(Arc::new(Mutex::new(vec![42])), Default::default()).await;
    config(&app, &[("my", &server.base)], "my");
    app.save_provider(provider("my")).await.unwrap();
    let mut settings = app.configuration().unwrap();
    let mut other = provider("other");
    other.password.clear();
    other.imported_account_ids = vec![7];
    other.cached_accounts = vec![cached(7, "other", 10.)];
    settings.sub2_api_provider.providers.push(other.clone());
    app.persist_configuration(&settings).unwrap();
    app.refresh_provider("my", true).await.unwrap();
    let saved = app.configuration().unwrap();
    assert_eq!(saved.sub2_api_provider.provider("other").unwrap(), other);
    assert_eq!(app.accounts().await.unwrap().len(), 2);
}
#[tokio::test]
async fn testAccountsPageSub2APIImportRequiresCurrentProviderConfiguration() {
    let (_dir, app, _) = fixture();
    config(
        &app,
        &[("missing", "https://not-requested.invalid")],
        "missing",
    );
    assert_eq!(
        app.refresh_provider("missing", true).await.unwrap_err(),
        crate::sub2api::PROVIDER_NOT_FOUND
    );
    assert!(app.accounts().await.unwrap().is_empty());
}
#[tokio::test]
async fn testManualSub2APIRefreshUsesCardProviderWithoutConfirmation() {
    let (_dir, app, _) = fixture();
    let calls = Arc::new(Mutex::new(vec![]));
    let server = upstream(Arc::new(Mutex::new(vec![42])), calls.clone()).await;
    config(&app, &[("my", &server.base)], "openai");
    app.save_provider(provider("my")).await.unwrap();
    app.refresh_provider("my", true).await.unwrap();
    calls.lock().unwrap().clear();
    app.refresh_usage("sub2api-my-42").await.unwrap();
    assert!(calls
        .lock()
        .unwrap()
        .iter()
        .any(|p| p.ends_with("/42/quota")));
    assert_eq!(
        crate::codex_config::resolve_path(&app.codex_config_path()).id,
        "openai"
    );
    assert!(app
        .configuration()
        .unwrap()
        .sub2_api_provider
        .confirmed_provider_ids
        .is_empty());
}
#[tokio::test]
async fn testRefreshPreservesValidSub2APICardsAndWarnsWhenUpstreamMembershipChanges() {
    let (_dir, app, _) = fixture();
    let ids = Arc::new(Mutex::new(vec![42, 43]));
    let server = upstream(ids.clone(), Default::default()).await;
    config(&app, &[("my", &server.base)], "my");
    app.save_provider(provider("my")).await.unwrap();
    app.refresh_provider("my", true).await.unwrap();
    *ids.lock().unwrap() = vec![43, 44];
    app.refresh_provider("my", false).await.unwrap();
    let settings = app.configuration().unwrap();
    let p = settings.sub2_api_provider.provider("my").unwrap();
    assert_eq!(p.imported_account_ids, [42, 43]);
    assert_eq!(
        p.cached_accounts
            .iter()
            .map(|v| v["id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [43]
    );
    assert_eq!(settings.extra["providerSyncNeeded:my"], true);
    assert_eq!(app.accounts().await.unwrap().len(), 1);
    app.refresh_provider("my", true).await.unwrap();
    let settings = app.configuration().unwrap();
    assert_eq!(
        settings
            .sub2_api_provider
            .provider("my")
            .unwrap()
            .imported_account_ids,
        [43, 44]
    );
    assert_eq!(settings.extra["providerSyncNeeded:my"], false);
    assert_eq!(app.accounts().await.unwrap().len(), 2);
}
#[tokio::test]
async fn testAccountsPageSwitchesToSub2APIAccountProvider() {
    let (_dir, app, _) = fixture();
    config(&app, &[("my", "https://provider.invalid/v1")], "openai");
    let mut settings = app.configuration().unwrap();
    let mut p = provider("my");
    p.password.clear();
    p.imported_account_ids = vec![42];
    p.cached_accounts = vec![cached(42, "my", 20.)];
    settings.sub2_api_provider.providers.push(p);
    app.persist_configuration(&settings).unwrap();
    app.switch_account("sub2api-my-42").await.unwrap();
    assert_eq!(
        crate::codex_config::resolve_path(&app.codex_config_path()).id,
        "my"
    );
    assert_eq!(
        app.accounts()
            .await
            .unwrap()
            .iter()
            .find(|a| a.is_current)
            .unwrap()
            .id,
        "sub2api-my-42"
    );
    assert!(!app.paths.codex_auth.exists());
}
#[tokio::test]
async fn testAccountsPageSwitchesProviderToOpenAIForLocalAccount() {
    let (_dir, app, _) = fixture();
    config(&app, &[("my", "https://provider.invalid/v1")], "my");
    app.import_json(&super::auth("a", "a").to_string())
        .await
        .unwrap();
    let id = app.accounts().await.unwrap()[0].id.clone();
    app.switch_account(&id).await.unwrap();
    assert_eq!(
        crate::codex_config::resolve_path(&app.codex_config_path()).id,
        "openai"
    );
    let auth: Value =
        serde_json::from_slice(&std::fs::read(&app.paths.codex_auth).unwrap()).unwrap();
    assert_eq!(crate::auth::extract_auth(&auth).unwrap().account_id, "a");
    assert!(app.accounts().await.unwrap()[0].is_current);
}
#[tokio::test]
async fn testCacheRefreshCannotOverwriteNewAccountProxy() {
    let (_dir, app, _) = fixture();
    let mut p = provider("p");
    p.password.clear();
    p.imported_account_ids = vec![42];
    p.cached_accounts = vec![cached(42, "p", 10.)];
    let mut settings = app.configuration().unwrap();
    settings.sub2_api_provider.providers.push(p);
    app.persist_configuration(&settings).unwrap();
    let stale: crate::sub2api::Account = serde_json::from_value(cached(42, "p", 20.)).unwrap();
    app.save_account_proxy("sub2api-p-42", "socks5://127.0.0.1:1080")
        .await
        .unwrap();
    app.apply_provider_refresh("p", vec![stale], false)
        .await
        .unwrap();
    assert_eq!(
        app.configuration()
            .unwrap()
            .sub2_api_provider
            .provider("p")
            .unwrap()
            .proxy_url,
        "socks5://127.0.0.1:1080"
    );
    assert_eq!(
        app.accounts().await.unwrap()[0].proxy_url,
        "socks5://127.0.0.1:1080"
    );
}
#[tokio::test]
async fn testTrayRecurringRefreshUpdatesSub2APIAccountsAndCache() {
    let (_dir, app, _) = fixture();
    let server = upstream(Arc::new(Mutex::new(vec![42])), Default::default()).await;
    config(&app, &[("ShareCoder", &server.base)], "ShareCoder");
    let p = app.save_provider(provider("ShareCoder")).await.unwrap();
    let mut settings = app.configuration().unwrap();
    settings
        .sub2_api_provider
        .providers
        .iter_mut()
        .find(|v| v.id == p.id)
        .unwrap()
        .imported_account_ids = vec![42];
    app.persist_configuration(&settings).unwrap();
    app.refresh_recurring(0).await.unwrap();
    assert_eq!(app.accounts().await.unwrap()[0].id, "sub2api-ShareCoder-42");
    assert_eq!(
        app.configuration()
            .unwrap()
            .sub2_api_provider
            .provider("ShareCoder")
            .unwrap()
            .cached_accounts[0]["providerID"],
        "ShareCoder"
    );
}
#[tokio::test]
async fn testManualRefreshUpdatesAllSub2APIProvidersWithOfficialDefault() {
    let (_dir, app, _) = fixture();
    let first_calls = Arc::new(Mutex::new(vec![]));
    let second_calls = Arc::new(Mutex::new(vec![]));
    let first = upstream(Arc::new(Mutex::new(vec![42])), first_calls.clone()).await;
    let second = upstream(Arc::new(Mutex::new(vec![7])), second_calls.clone()).await;
    config(
        &app,
        &[("first", &first.base), ("second", &second.base)],
        "openai",
    );
    for (name, id) in [("first", 42), ("second", 7)] {
        app.save_provider(provider(name)).await.unwrap();
        let mut s = app.configuration().unwrap();
        s.sub2_api_provider
            .providers
            .iter_mut()
            .find(|p| p.provider_id == name)
            .unwrap()
            .imported_account_ids = vec![id];
        app.persist_configuration(&s).unwrap();
    }
    app.refresh_all().await.unwrap();
    assert!(first_calls
        .lock()
        .unwrap()
        .iter()
        .any(|p| p.ends_with("/42/quota")));
    assert!(second_calls
        .lock()
        .unwrap()
        .iter()
        .any(|p| p.ends_with("/7/quota")));
    assert_eq!(app.accounts().await.unwrap().len(), 2);
    assert_eq!(
        crate::codex_config::resolve_path(&app.codex_config_path()).id,
        "openai"
    );
}
#[tokio::test]
async fn recurringRefreshReachesEveryProviderOnlyOnFullRounds() {
    let (_dir, app, _) = fixture();
    let first_calls = Arc::new(Mutex::new(vec![]));
    let second_calls = Arc::new(Mutex::new(vec![]));
    let first = upstream(Arc::new(Mutex::new(vec![42])), first_calls.clone()).await;
    let second = upstream(Arc::new(Mutex::new(vec![7])), second_calls.clone()).await;
    config(
        &app,
        &[("first", &first.base), ("second", &second.base)],
        "first",
    );
    for (name, id) in [("first", 42), ("second", 7)] {
        app.save_provider(provider(name)).await.unwrap();
        let mut s = app.configuration().unwrap();
        s.sub2_api_provider
            .providers
            .iter_mut()
            .find(|p| p.provider_id == name)
            .unwrap()
            .imported_account_ids = vec![id];
        app.persist_configuration(&s).unwrap();
    }
    let asked = |calls: &Arc<Mutex<Vec<String>>>, id: &str| {
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.ends_with(&format!("/{id}/quota")))
            .count()
    };
    // A round between full ones refreshes only the provider in use...
    app.refresh_recurring(1).await.unwrap();
    assert_eq!(
        (asked(&first_calls, "42"), asked(&second_calls, "7")),
        (1, 0)
    );
    // ...and a full round every provider.
    app.refresh_recurring(6).await.unwrap();
    assert_eq!(
        (asked(&first_calls, "42"), asked(&second_calls, "7")),
        (2, 1)
    );
    // Their cards show it: the provider in use's next round, the other's
    // next full round (tick 6).
    app.set_refresh_schedule(crate::refresh_policy::Schedule {
        at: 1_000,
        tick: 2,
        interval: 300,
    });
    let accounts = app.accounts().await.unwrap();
    let next = |id: &str| {
        accounts
            .iter()
            .find(|a| a.id == id)
            .unwrap()
            .next_refresh_at
    };
    assert_eq!(next("sub2api-first-42"), Some(1_000));
    assert_eq!(next("sub2api-second-7"), Some(2_200));
}
#[tokio::test]
async fn testAccountsPageSyncsConfiguredProviderWithoutConfirmation() {
    let (_dir, app, _) = fixture();
    let server = upstream(Arc::new(Mutex::new(vec![42])), Default::default()).await;
    config(&app, &[("my", &server.base)], "my");
    app.save_provider(provider("my")).await.unwrap();
    assert!(app
        .configuration()
        .unwrap()
        .sub2_api_provider
        .confirmed_provider_ids
        .is_empty());
    app.refresh_provider("my", true).await.unwrap();
    assert_eq!(
        app.configuration()
            .unwrap()
            .sub2_api_provider
            .provider("my")
            .unwrap()
            .imported_account_ids,
        [42]
    );
    assert_eq!(app.accounts().await.unwrap().len(), 1);
}
#[tokio::test]
async fn testSmartSwitchIncludesSub2APIProviderCandidate() {
    let (_dir, app, _) = fixture();
    config(&app, &[("my", "https://unused.invalid/v1")], "openai");
    let mut settings = app.configuration().unwrap();
    let mut p = provider("my");
    p.password.clear();
    p.imported_account_ids = vec![42];
    p.cached_accounts = vec![cached(42, "my", 1.)];
    settings.sub2_api_provider.providers.push(p);
    app.persist_configuration(&settings).unwrap();
    let mut local = super::stored("local", "local", "local");
    local["usage"] = json!({"fetchedAt":now_unix_seconds(),"fiveHour":{"usedPercent":50,"windowSeconds":18000},"oneWeek":{"usedPercent":80,"windowSeconds":604800}});
    super::save_fixture(
        &app,
        json!({"accounts":[local],"cachedAccounts":crate::repository::load_accounts(&app.paths.store()).unwrap().extra["cachedAccounts"]}),
    );
    app.smart_switch().await.unwrap();
    assert_eq!(
        crate::codex_config::resolve_path(&app.codex_config_path()).id,
        "my"
    );
    assert_eq!(
        app.accounts()
            .await
            .unwrap()
            .iter()
            .find(|a| a.is_current)
            .unwrap()
            .id,
        "sub2api-my-42"
    );
}

#[tokio::test]
async fn sub2api_refresh_failure_marks_only_affected_cards_and_recovers() {
    let (_dir, app, secrets) = fixture();
    let server = upstream(Arc::new(Mutex::new(vec![42, 43])), Default::default()).await;
    config(&app, &[("my", &server.base)], "openai");
    app.save_provider(provider("my")).await.unwrap();
    app.refresh_provider("my", true).await.unwrap();
    let previous = app.accounts().await.unwrap()[0].usage.clone();
    secrets
        .deny_reads
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let error = app.refresh_usage("sub2api-my-42").await.unwrap_err();
    let cards = app.accounts().await.unwrap();
    let failed = cards.iter().find(|a| a.id == "sub2api-my-42").unwrap();
    assert_eq!(failed.usage_error.as_deref(), Some(error.as_str()));
    assert_eq!(
        serde_json::to_value(&failed.usage).unwrap(),
        serde_json::to_value(&previous).unwrap()
    );
    assert!(cards
        .iter()
        .find(|a| a.id == "sub2api-my-43")
        .unwrap()
        .usage_error
        .is_none());
    let error = app.refresh_provider("my", false).await.unwrap_err();
    assert!(app
        .accounts()
        .await
        .unwrap()
        .iter()
        .all(|a| a.usage_error.as_deref() == Some(error.as_str())));
    secrets
        .deny_reads
        .store(false, std::sync::atomic::Ordering::SeqCst);
    app.refresh_usage("sub2api-my-42").await.unwrap();
    let cards = app.accounts().await.unwrap();
    assert!(cards
        .iter()
        .find(|a| a.id == "sub2api-my-42")
        .unwrap()
        .usage_error
        .is_none());
    assert!(cards
        .iter()
        .find(|a| a.id == "sub2api-my-43")
        .unwrap()
        .usage_error
        .is_some());
    app.refresh_provider("my", false).await.unwrap();
    assert!(app
        .accounts()
        .await
        .unwrap()
        .iter()
        .all(|a| a.usage_error.is_none()));
}

#[tokio::test]
async fn sub2api_requests_go_through_the_provider_proxy() {
    let unavailable = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let (_dir, app, _) = fixture();
    let calls = Arc::new(Mutex::new(vec![]));
    let server = upstream(Arc::new(Mutex::new(vec![42, 43])), calls.clone()).await;
    config(&app, &[("my", "http://sub2api.invalid/v1")], "openai");
    let mut p = provider("my");
    p.proxy_url = server.base.clone();
    app.save_provider(p).await.unwrap();
    app.refresh_provider("my", true).await.unwrap();
    // Any card sets its provider's proxy; the default account proxy is not used.
    app.save_account_proxy("sub2api-my-43", &unavailable)
        .await
        .unwrap();
    app.save_account_proxy("sub2api-my-42", &server.base)
        .await
        .unwrap();
    let mut preferences = app.preferences().unwrap();
    preferences.account_proxy_url = unavailable.clone();
    app.save_preferences(preferences).await.unwrap();
    calls.lock().unwrap().clear();
    app.refresh_all().await.unwrap();
    let requests = calls.lock().unwrap().clone();
    assert_eq!(
        requests
            .iter()
            .filter(|p| p.ends_with("/auth/login"))
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|p| p.ends_with("/admin/accounts"))
            .count(),
        1
    );
    assert!(requests.iter().any(|p| p.ends_with("/42/quota")));
    assert!(requests.iter().any(|p| p.ends_with("/43/quota")));
    let cards = app.accounts().await.unwrap();
    assert!(cards
        .iter()
        .all(|a| a.proxy_url == server.base && a.usage_error.is_none()));
    app.save_account_proxy("sub2api-my-43", &unavailable)
        .await
        .unwrap();
    calls.lock().unwrap().clear();
    assert!(app.refresh_all().await.is_err());
    assert!(calls.lock().unwrap().is_empty());
    assert!(app
        .accounts()
        .await
        .unwrap()
        .iter()
        .all(|a| a.usage_error.is_some()));
}
#[tokio::test]
async fn testSavedCloudflareTokenIsMarkedWithoutBeingReturned() {
    let (_dir, app, secrets) = fixture();
    let mut settings = serde_json::to_value(app.configuration().unwrap()).unwrap();
    settings["proxyConfiguration"]["cloudflared"] = json!({"enabled":true,"tunnelMode":"named",
        "accountId":"account","zoneId":"zone","namedHostname":"api.example.test"});
    std::fs::write(
        app.paths.data_dir.join("settings.json"),
        serde_json::to_vec(&settings).unwrap(),
    )
    .unwrap();
    secrets.set("cloudflare-api-token", "token").unwrap();
    let cloudflared = || {
        serde_json::to_value(app.configuration().unwrap()).unwrap()["proxyConfiguration"]
            ["cloudflared"]
            .clone()
    };
    // A token saved before the mark existed is marked once.
    app.migrate_provider_credentials().unwrap();
    assert_eq!(cloudflared()["hasAPIToken"], true);
    assert!(cloudflared().get("apiToken").is_none());
    // Saving without typing the token again keeps it, and the mark.
    let config: crate::tunnel::Configuration = serde_json::from_value(cloudflared()).unwrap();
    app.save_tunnel_configuration(&config).await.unwrap();
    assert_eq!(cloudflared()["hasAPIToken"], true);
    secrets.remove("cloudflare-api-token").unwrap();
    app.save_tunnel_configuration(&config).await.unwrap();
    assert_eq!(cloudflared()["hasAPIToken"], false);
}
