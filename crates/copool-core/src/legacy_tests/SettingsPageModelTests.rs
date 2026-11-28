use super::{http_fixture::serve, MemorySecrets};
use crate::Pool;
use crate::{
    credentials::SecretStore,
    provider_accounts::password_secret,
    provider_registry::{Adapter, ProviderRecord},
    utils::now_unix_seconds,
    Application, Paths,
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
/// A Codex provider form, named after its `config.toml` provider.
fn provider(name: &str) -> Value {
    json!({"name": name, "username": "admin@example.test", "password": "test-secret"})
}
/// Saves a form as the settings page does.
async fn save(app: &Application, form: Value) -> ProviderRecord {
    serde_json::from_value(app.save_provider(Pool::Codex, form).await.unwrap()).unwrap()
}
/// The Codex providers as stored.
fn stored(app: &Application) -> Vec<ProviderRecord> {
    crate::codex::codex_providers(&app.paths).unwrap()
}
fn named(app: &Application, name: &str) -> ProviderRecord {
    stored(app).into_iter().find(|p| p.name == name).unwrap()
}
/// Edits the stored Codex providers directly.
async fn put(app: &Application, edit: impl FnOnce(&mut Vec<ProviderRecord>) + Send) {
    crate::codex::ProviderAdapter(app)
        .edit_providers(|providers| {
            edit(providers);
            Ok(())
        })
        .await
        .unwrap();
}
/// A stored provider, as an import left it: its id unlike its name.
fn imported(name: &str, accounts: &[(i64, f64)]) -> ProviderRecord {
    ProviderRecord {
        id: format!("id-{name}"),
        name: name.into(),
        username: "admin@example.test".into(),
        imported_account_ids: accounts.iter().map(|(id, _)| *id).collect(),
        cached_accounts: accounts
            .iter()
            .map(|(id, used)| serde_json::from_value(cached(*id, name, *used)).unwrap())
            .collect(),
        ..Default::default()
    }
}
/// A provider's card: its provider's id and its account.
fn card(p: &ProviderRecord, account: i64) -> String {
    format!("sub2api-{}-{account}", p.id)
}
fn same(a: &ProviderRecord, b: &ProviderRecord) -> bool {
    serde_json::to_value(a).unwrap() == serde_json::to_value(b).unwrap()
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
async fn testSaveSub2APIProviderWhosePasswordIsRefusedKeepsNoneAdded() {
    let (_dir, app, secrets) = fixture();
    let kept = save(&app, provider("kept")).await;
    secrets
        .deny_writes
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(app
        .save_provider(Pool::Codex, provider("new"))
        .await
        .is_err());
    // An edit refused its password keeps the provider as it was.
    let mut edit = provider("kept");
    edit["id"] = kept.id.clone().into();
    edit["username"] = "other@example.test".into();
    edit["adminBaseURL"] = "https://other.test/api/v1".into();
    assert!(app.save_provider(Pool::Codex, edit).await.is_err());
    let providers = stored(&app);
    let names: Vec<_> = providers.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["kept"]);
    assert_eq!(providers[0].username, kept.username);
    assert_eq!(providers[0].admin_base_url, kept.admin_base_url);
}
#[tokio::test]
async fn testSaveSub2APIProviderStoresPasswordOutsideSettings() {
    let (_dir, app, secrets) = fixture();
    let mut draft = provider("my");
    draft["adminBaseURL"] = " https://admin.test/api/v1/ ".into();
    draft["allowInsecureTLS"] = true.into();
    let saved = save(&app, draft).await;
    assert_eq!(saved.password, "");
    assert_eq!(saved.admin_base_url, "https://admin.test/api/v1");
    assert_eq!(saved.username, "admin@example.test");
    assert!(saved.allow_insecure_tls);
    assert_eq!(
        secrets
            .get(&password_secret(Pool::Codex, &saved.id))
            .unwrap()
            .as_deref(),
        Some("test-secret")
    );
    // Nor where the providers are kept now, nor where they were.
    for file in ["settings.json", crate::codex::FILE, "accounts.json"] {
        let raw = std::fs::read_to_string(app.paths.data_dir.join(file)).unwrap();
        assert!(!raw.contains("test-secret"), "{file}");
    }
    let round: ProviderRecord =
        serde_json::from_value(serde_json::to_value(&saved).unwrap()).unwrap();
    assert_eq!(round.admin_base_url, saved.admin_base_url);
    let paths = app.paths.clone();
    drop(app);
    let reloaded = Application::open_with_secrets(paths, secrets).unwrap();
    let p = &stored(&reloaded)[0];
    assert_eq!(p.password, "");
    assert_eq!(p.admin_base_url, "https://admin.test/api/v1");
}
#[tokio::test]
async fn testEditingProviderPreservesOtherCredentialsWithoutKeychainAccess() {
    let (_dir, app, secrets) = fixture();
    let first = save(&app, provider("first")).await;
    let second = save(&app, provider("second")).await;
    secrets.gets.lock().unwrap().clear();
    secrets.sets.lock().unwrap().clear();
    secrets
        .deny_reads
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let mut edited = serde_json::to_value(&first).unwrap();
    edited["adminBaseURL"] = "https://admin.example.test".into();
    let first = save(&app, edited).await;
    assert!(secrets.gets.lock().unwrap().is_empty());
    assert!(secrets.sets.lock().unwrap().is_empty());
    assert!(same(&stored(&app)[1], &second));
    let mut edited = serde_json::to_value(&first).unwrap();
    edited["password"] = "replacement".into();
    let first = save(&app, edited).await;
    let first_secret = password_secret(Pool::Codex, &first.id);
    assert_eq!(*secrets.sets.lock().unwrap(), [first_secret.clone()]);
    assert!(secrets.gets.lock().unwrap().is_empty());
    assert_eq!(secrets.values.lock().unwrap()[&first_secret], "replacement");
    assert_eq!(
        secrets.values.lock().unwrap()[&password_secret(Pool::Codex, &second.id)],
        "test-secret"
    );
    save(&app, serde_json::to_value(&first).unwrap()).await;
    assert_eq!(*secrets.sets.lock().unwrap(), [first_secret]);
    let mut new = provider("new");
    new["password"] = "".into();
    assert_eq!(
        app.save_provider(Pool::Codex, new).await.unwrap_err(),
        "error.sub2api.configuration_incomplete"
    );
    assert_eq!(stored(&app).len(), 2);
}
#[tokio::test]
async fn testSettingsPageAddsAndSavesArbitrarySub2APIProviderConfigurations() {
    let (_dir, app, _) = fixture();
    for name in ["my", "ShareCoder", "custom-provider"] {
        save(&app, provider(name)).await;
    }
    let saved = stored(&app);
    assert_eq!(
        saved.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["my", "ShareCoder", "custom-provider"]
    );
    // Their gateways stay in the Codex configuration, which saving never writes.
    assert!(saved.iter().all(|p| p.base_url.is_empty()));
    assert!(!app.codex_config_path().exists());
}
#[tokio::test]
async fn testAccountsPageRestoresCachedSub2APIAccountsWithoutNetwork() {
    let (_dir, app, secrets) = fixture();
    let p = imported("my", &[(42, 20.)]);
    put(&app, |providers| providers.push(p.clone())).await;
    secrets
        .deny_reads
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let cards = app.accounts(Pool::Codex).await.unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].id, "sub2api-id-my-42");
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
    let p = save(&app, provider("my")).await;
    assert_eq!(
        app.refresh_provider(Pool::Codex, &p.id, true)
            .await
            .unwrap(),
        1
    );
    let p = &stored(&app)[0];
    assert_eq!(p.imported_account_ids, [42]);
    assert_eq!(p.cached_accounts[0].id, 42);
    let cards = app.accounts(Pool::Codex).await.unwrap();
    assert_eq!(cards[0].id, card(p, 42));
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
}
#[tokio::test]
async fn testAccountsPageSub2APIImportPreservesOtherProviderCaches() {
    let (_dir, app, _) = fixture();
    let server = upstream(Arc::new(Mutex::new(vec![42])), Default::default()).await;
    config(&app, &[("my", &server.base)], "my");
    let p = save(&app, provider("my")).await;
    let other = imported("other", &[(7, 10.)]);
    put(&app, |providers| providers.push(other.clone())).await;
    app.refresh_provider(Pool::Codex, &p.id, true)
        .await
        .unwrap();
    assert!(same(&named(&app, "other"), &other));
    assert_eq!(app.accounts(Pool::Codex).await.unwrap().len(), 2);
}
#[tokio::test]
async fn testAccountsPageSub2APIImportRequiresCurrentProviderConfiguration() {
    let (_dir, app, _) = fixture();
    config(
        &app,
        &[("missing", "https://not-requested.invalid")],
        "missing",
    );
    // No provider is saved for the configured one...
    assert_eq!(
        app.refresh_provider(Pool::Codex, "missing", true)
            .await
            .unwrap_err(),
        crate::provider_registry::PROVIDER_NOT_FOUND
    );
    // ...and the Codex configuration defines none for the saved one.
    let p = save(&app, provider("absent")).await;
    assert_eq!(
        app.refresh_provider(Pool::Codex, &p.id, true)
            .await
            .unwrap_err(),
        crate::sub2api::PROVIDER_NOT_FOUND
    );
    assert!(app.accounts(Pool::Codex).await.unwrap().is_empty());
}
#[tokio::test]
async fn testManualSub2APIRefreshUsesCardProviderWithoutConfirmation() {
    let (_dir, app, _) = fixture();
    let calls = Arc::new(Mutex::new(vec![]));
    let server = upstream(Arc::new(Mutex::new(vec![42])), calls.clone()).await;
    config(&app, &[("my", &server.base)], "openai");
    let p = save(&app, provider("my")).await;
    app.refresh_provider(Pool::Codex, &p.id, true)
        .await
        .unwrap();
    calls.lock().unwrap().clear();
    app.refresh_usage(Pool::Codex, &card(&p, 42)).await.unwrap();
    assert!(calls
        .lock()
        .unwrap()
        .iter()
        .any(|p| p.ends_with("/42/quota")));
    assert_eq!(
        crate::codex::config::resolve_path(&app.codex_config_path()).id,
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
async fn testRefreshAdoptsUpstreamMembershipChanges() {
    let (_dir, app, _) = fixture();
    let ids = Arc::new(Mutex::new(vec![42, 43]));
    let server = upstream(ids.clone(), Default::default()).await;
    config(&app, &[("my", &server.base)], "my");
    let p = save(&app, provider("my")).await;
    app.refresh_provider(Pool::Codex, &p.id, true)
        .await
        .unwrap();
    *ids.lock().unwrap() = vec![43, 44];
    app.refresh_provider(Pool::Codex, &p.id, false)
        .await
        .unwrap();
    // A refresh adds the account Sub2api now lists and hides the one it no
    // longer lists, just like a new sync.
    let p = named(&app, "my");
    assert_eq!(p.imported_account_ids, [42, 43, 44]);
    assert_eq!(
        p.cached_accounts
            .iter()
            .map(|a| (a.id, a.status.as_str()))
            .collect::<Vec<_>>(),
        [(43, "active"), (44, "active")]
    );
    assert_eq!(app.accounts(Pool::Codex).await.unwrap().len(), 2);
    assert!(p.cached_accounts.iter().all(|a| a.usage.is_some()));
    // A sync takes the accounts as they are.
    app.refresh_provider(Pool::Codex, &p.id, true)
        .await
        .unwrap();
    assert_eq!(named(&app, "my").imported_account_ids, [43, 44]);
    assert_eq!(app.accounts(Pool::Codex).await.unwrap().len(), 2);
}
#[tokio::test]
async fn testAccountsPageSwitchesToSub2APIAccountProvider() {
    let (_dir, app, _) = fixture();
    config(&app, &[("my", "https://provider.invalid/v1")], "openai");
    let p = imported("my", &[(42, 20.)]);
    put(&app, |providers| providers.push(p.clone())).await;
    let execution = app
        .switch_account(Pool::Codex, &card(&p, 42))
        .await
        .unwrap();
    // Codex selects the provider by the name its configuration gives it.
    assert_eq!(execution.provider.as_deref(), Some("my"));
    assert_eq!(
        crate::codex::config::resolve_path(&app.codex_config_path()).id,
        "my"
    );
    assert_eq!(
        app.accounts(Pool::Codex)
            .await
            .unwrap()
            .iter()
            .find(|a| a.is_current)
            .unwrap()
            .id,
        card(&p, 42)
    );
    assert!(!app.paths.codex_auth.exists());
}
#[tokio::test]
async fn testAccountsPageSwitchesProviderToOpenAIForLocalAccount() {
    let (_dir, app, _) = fixture();
    config(&app, &[("my", "https://provider.invalid/v1")], "my");
    app.import_json(Pool::Codex, &super::auth("a", "a").to_string())
        .await
        .unwrap();
    let id = app.accounts(Pool::Codex).await.unwrap()[0].id.clone();
    app.switch_account(Pool::Codex, &id).await.unwrap();
    assert_eq!(
        crate::codex::config::resolve_path(&app.codex_config_path()).id,
        "openai"
    );
    let auth: Value =
        serde_json::from_slice(&std::fs::read(&app.paths.codex_auth).unwrap()).unwrap();
    assert_eq!(
        crate::pool_login::codex::extract_auth(&auth)
            .unwrap()
            .account_id,
        "a"
    );
    assert!(app.accounts(Pool::Codex).await.unwrap()[0].is_current);
}
#[tokio::test]
async fn testCacheRefreshCannotOverwriteNewAccountProxy() {
    let (_dir, app, _) = fixture();
    let p = imported("p", &[(42, 10.)]);
    put(&app, |providers| providers.push(p.clone())).await;
    let stale: crate::provider_accounts::Account =
        serde_json::from_value(cached(42, "p", 20.)).unwrap();
    app.save_account_proxy(Pool::Codex, &card(&p, 42), "socks5://127.0.0.1:1080")
        .await
        .unwrap();
    app.apply_provider_refresh(Pool::Codex, &p.id, vec![stale], None, false)
        .await
        .unwrap();
    assert_eq!(named(&app, "p").proxy_url, "socks5://127.0.0.1:1080");
    assert_eq!(
        app.accounts(Pool::Codex).await.unwrap()[0].proxy_url,
        "socks5://127.0.0.1:1080"
    );
}
#[tokio::test]
async fn testTrayRecurringRefreshUpdatesSub2APIAccountsAndCache() {
    let (_dir, app, _) = fixture();
    let server = upstream(Arc::new(Mutex::new(vec![42])), Default::default()).await;
    config(&app, &[("ShareCoder", &server.base)], "ShareCoder");
    let p = save(&app, provider("ShareCoder")).await;
    put(&app, |providers| {
        providers[0].imported_account_ids = vec![42]
    })
    .await;
    app.refresh_recurring(Pool::Codex, 0).await.unwrap();
    assert_eq!(app.accounts(Pool::Codex).await.unwrap()[0].id, card(&p, 42));
    assert_eq!(
        named(&app, "ShareCoder").cached_accounts[0]
            .provider_id
            .as_deref(),
        Some("ShareCoder")
    );
}
/// Saves the providers named, each with its imported account.
async fn save_imported(app: &Application, providers: &[(&str, i64)]) -> Vec<ProviderRecord> {
    let mut saved = vec![];
    for (name, id) in providers {
        let p = save(app, provider(name)).await;
        put(app, |providers| {
            providers
                .iter_mut()
                .find(|v| v.id == p.id)
                .unwrap()
                .imported_account_ids = vec![*id]
        })
        .await;
        saved.push(p);
    }
    saved
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
    save_imported(&app, &[("first", 42), ("second", 7)]).await;
    app.refresh_all(Pool::Codex).await.unwrap();
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
    assert_eq!(app.accounts(Pool::Codex).await.unwrap().len(), 2);
    assert_eq!(
        crate::codex::config::resolve_path(&app.codex_config_path()).id,
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
    let saved = save_imported(&app, &[("first", 42), ("second", 7)]).await;
    let asked = |calls: &Arc<Mutex<Vec<String>>>, id: &str| {
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.ends_with(&format!("/{id}/quota")))
            .count()
    };
    // A round between full ones refreshes only the provider in use...
    app.refresh_recurring(Pool::Codex, 1).await.unwrap();
    assert_eq!(
        (asked(&first_calls, "42"), asked(&second_calls, "7")),
        (1, 0)
    );
    // ...and a full round every provider.
    app.refresh_recurring(Pool::Codex, 6).await.unwrap();
    assert_eq!(
        (asked(&first_calls, "42"), asked(&second_calls, "7")),
        (2, 1)
    );
    // Their cards show it: the provider in use's next round, the other's
    // next full round (tick 6).
    app.set_schedule(
        Pool::Codex,
        crate::refresh_policy::Schedule {
            at: 1_000,
            tick: 2,
            interval: 300,
            full_every: 6,
        },
    );
    let accounts = app.accounts(Pool::Codex).await.unwrap();
    let next = |id: &str| {
        accounts
            .iter()
            .find(|a| a.id == id)
            .unwrap()
            .next_refresh_at
    };
    assert_eq!(next(&card(&saved[0], 42)), Some(1_000));
    assert_eq!(next(&card(&saved[1], 7)), Some(2_200));
}
#[tokio::test]
async fn testAccountsPageSyncsConfiguredProviderWithoutConfirmation() {
    let (_dir, app, _) = fixture();
    let server = upstream(Arc::new(Mutex::new(vec![42])), Default::default()).await;
    config(&app, &[("my", &server.base)], "my");
    let p = save(&app, provider("my")).await;
    assert!(app
        .configuration()
        .unwrap()
        .sub2_api_provider
        .confirmed_provider_ids
        .is_empty());
    app.refresh_provider(Pool::Codex, &p.id, true)
        .await
        .unwrap();
    assert_eq!(named(&app, "my").imported_account_ids, [42]);
    assert_eq!(app.accounts(Pool::Codex).await.unwrap().len(), 1);
}
#[tokio::test]
async fn testSmartSwitchIncludesSub2APIProviderCandidate() {
    let (_dir, app, _) = fixture();
    config(&app, &[("my", "https://unused.invalid/v1")], "openai");
    let p = imported("my", &[(42, 1.)]);
    put(&app, |providers| providers.push(p.clone())).await;
    let mut local = super::stored("local", "local", "local");
    local["usage"] = json!({"fetchedAt":now_unix_seconds(),"fiveHour":{"usedPercent":50,"windowSeconds":18000},"oneWeek":{"usedPercent":80,"windowSeconds":604800}});
    let providers = app.codex_load().unwrap().providers;
    super::save_fixture(&app, json!({"accounts":[local], "providers":providers}));
    app.smart_switch(Pool::Codex).await.unwrap();
    assert_eq!(
        crate::codex::config::resolve_path(&app.codex_config_path()).id,
        "my"
    );
    assert_eq!(
        app.accounts(Pool::Codex)
            .await
            .unwrap()
            .iter()
            .find(|a| a.is_current)
            .unwrap()
            .id,
        card(&p, 42)
    );
}

#[tokio::test]
async fn sub2api_refresh_failure_marks_only_affected_cards_and_recovers() {
    let (_dir, app, secrets) = fixture();
    let server = upstream(Arc::new(Mutex::new(vec![42, 43])), Default::default()).await;
    config(&app, &[("my", &server.base)], "openai");
    let p = save(&app, provider("my")).await;
    let (first, second) = (card(&p, 42), card(&p, 43));
    app.refresh_provider(Pool::Codex, &p.id, true)
        .await
        .unwrap();
    let previous = app.accounts(Pool::Codex).await.unwrap()[0].usage.clone();
    secrets
        .deny_reads
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let error = app.refresh_usage(Pool::Codex, &first).await.unwrap_err();
    let cards = app.accounts(Pool::Codex).await.unwrap();
    let failed = cards.iter().find(|a| a.id == first).unwrap();
    assert_eq!(failed.usage_error.as_deref(), Some(error.as_str()));
    assert_eq!(
        serde_json::to_value(&failed.usage).unwrap(),
        serde_json::to_value(&previous).unwrap()
    );
    assert!(cards
        .iter()
        .find(|a| a.id == second)
        .unwrap()
        .usage_error
        .is_none());
    let error = app
        .refresh_provider(Pool::Codex, &p.id, false)
        .await
        .unwrap_err();
    assert!(app
        .accounts(Pool::Codex)
        .await
        .unwrap()
        .iter()
        .all(|a| a.usage_error.as_deref() == Some(error.as_str())));
    secrets
        .deny_reads
        .store(false, std::sync::atomic::Ordering::SeqCst);
    app.refresh_usage(Pool::Codex, &first).await.unwrap();
    let cards = app.accounts(Pool::Codex).await.unwrap();
    assert!(cards
        .iter()
        .find(|a| a.id == first)
        .unwrap()
        .usage_error
        .is_none());
    assert!(cards
        .iter()
        .find(|a| a.id == second)
        .unwrap()
        .usage_error
        .is_some());
    app.refresh_provider(Pool::Codex, &p.id, false)
        .await
        .unwrap();
    assert!(app
        .accounts(Pool::Codex)
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
    let mut form = provider("my");
    form["proxyURL"] = server.base.clone().into();
    let p = save(&app, form).await;
    app.refresh_provider(Pool::Codex, &p.id, true)
        .await
        .unwrap();
    // Any card sets its provider's proxy; the default account proxy is not used.
    app.save_account_proxy(Pool::Codex, &card(&p, 43), &unavailable)
        .await
        .unwrap();
    app.save_account_proxy(Pool::Codex, &card(&p, 42), &server.base)
        .await
        .unwrap();
    let mut preferences = app.preferences().unwrap();
    preferences.account_proxy_url = unavailable.clone();
    app.save_preferences(preferences).await.unwrap();
    calls.lock().unwrap().clear();
    app.refresh_all(Pool::Codex).await.unwrap();
    let requests = calls.lock().unwrap().clone();
    // The first refresh's login through the same proxy is reused.
    assert_eq!(
        requests
            .iter()
            .filter(|p| p.ends_with("/auth/login"))
            .count(),
        0
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
    let cards = app.accounts(Pool::Codex).await.unwrap();
    assert!(cards
        .iter()
        .all(|a| a.proxy_url == server.base && a.usage_error.is_none()));
    app.save_account_proxy(Pool::Codex, &card(&p, 43), &unavailable)
        .await
        .unwrap();
    calls.lock().unwrap().clear();
    assert!(app.refresh_all(Pool::Codex).await.is_err());
    assert!(calls.lock().unwrap().is_empty());
    assert!(app
        .accounts(Pool::Codex)
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
    app.mark_kept_tunnel_token().unwrap();
    assert_eq!(cloudflared()["hasAPIToken"], true);
    assert!(cloudflared().get("apiToken").is_none());
    // Saving without typing the token again keeps it, and the mark.
    let config: crate::tunnel::Configuration = serde_json::from_value(cloudflared()).unwrap();
    app.save_tunnel_configuration(Pool::Codex, &config)
        .await
        .unwrap();
    assert_eq!(cloudflared()["hasAPIToken"], true);
    secrets.remove("cloudflare-api-token").unwrap();
    app.save_tunnel_configuration(Pool::Codex, &config)
        .await
        .unwrap();
    assert_eq!(cloudflared()["hasAPIToken"], false);
}

#[tokio::test]
async fn deleted_sub2api_provider_stays_removed_until_explicit_sync() {
    let (_dir, app, _) = fixture();
    let calls = Arc::new(Mutex::new(vec![]));
    let server = upstream(Arc::new(Mutex::new(vec![42, 43])), calls.clone()).await;
    config(&app, &[("my", &server.base)], "my");
    let p = save(&app, provider("my")).await;
    app.refresh_provider(Pool::Codex, &p.id, true)
        .await
        .unwrap();
    for account in app.accounts(Pool::Codex).await.unwrap() {
        app.delete_account(Pool::Codex, &account.id).await.unwrap();
    }
    calls.lock().unwrap().clear();
    app.refresh_all(Pool::Codex).await.unwrap();
    app.refresh_recurring(Pool::Codex, 0).await.unwrap();
    app.refresh_recurring(Pool::Codex, 1).await.unwrap();
    assert!(calls.lock().unwrap().is_empty());
    assert!(app.accounts(Pool::Codex).await.unwrap().is_empty());
    // A response from a refresh started before removal cannot resurrect it either.
    let stale = serde_json::from_value(cached(42, "my", 20.)).unwrap();
    app.apply_provider_refresh(Pool::Codex, &p.id, vec![stale], None, false)
        .await
        .unwrap();
    assert!(app.accounts(Pool::Codex).await.unwrap().is_empty());
    app.refresh_provider(Pool::Codex, &p.id, true)
        .await
        .unwrap();
    assert_eq!(app.accounts(Pool::Codex).await.unwrap().len(), 2);
}

#[tokio::test]
async fn refreshing_one_card_hides_an_account_sub2api_no_longer_lists() {
    let (_dir, app, _) = fixture();
    let ids = Arc::new(Mutex::new(vec![42, 43]));
    let server = upstream(ids.clone(), Default::default()).await;
    config(&app, &[("my", &server.base)], "openai");
    let p = save(&app, provider("my")).await;
    app.refresh_provider(Pool::Codex, &p.id, true)
        .await
        .unwrap();
    // Only the account Sub2api no longer lists disappears; the other stays.
    *ids.lock().unwrap() = vec![43];
    app.refresh_usage(Pool::Codex, &card(&p, 42)).await.unwrap();
    assert_eq!(cards(&app).await, [card(&p, 43)]);
    let remaining = app.accounts(Pool::Codex).await.unwrap().remove(0);
    assert!(remaining.available && remaining.usage.is_some() && remaining.usage_error.is_none());
    // Its last account gone, the provider has no cards but stays imported.
    ids.lock().unwrap().clear();
    app.refresh_usage(Pool::Codex, &card(&p, 43)).await.unwrap();
    assert!(cards(&app).await.is_empty());
    assert_eq!(named(&app, "my").imported_account_ids, [42, 43]);
    // A full background round finds accounts reactivated in Sub2api.
    *ids.lock().unwrap() = vec![42];
    app.refresh_recurring(Pool::Codex, 0).await.unwrap();
    assert_eq!(cards(&app).await, [card(&p, 42)]);
}
/// The Codex pool's card ids, sorted.
async fn cards(app: &Application) -> Vec<String> {
    let mut ids: Vec<String> = app
        .accounts(Pool::Codex)
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.id)
        .collect();
    ids.sort();
    ids
}
#[test]
fn testRefreshPreferencesAnEarlierBuildSavedStillOpen() {
    let (_dir, app, secrets) = fixture();
    let mut settings = serde_json::to_value(app.configuration().unwrap()).unwrap();
    settings["refresh"] = json!({"claude": {"seconds": 600, "fullEvery": 2, "adaptive": false}});
    std::fs::write(
        app.paths.data_dir.join("settings.json"),
        serde_json::to_vec(&settings).unwrap(),
    )
    .unwrap();
    let paths = app.paths.clone();
    drop(app);
    let reloaded = Application::open_with_secrets(paths, secrets).unwrap();
    let refresh = reloaded.preferences().unwrap().refresh;
    assert_eq!(
        (refresh.claude.seconds, refresh.claude.full_every),
        (600, 2)
    );
    assert!(!refresh.claude.adaptive && refresh.claude.backoff);
}
