use super::http_fixture::{serve, serve_headers};
use crate::usage::*;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
#[tokio::test]
async fn testFetchUsageShowsDirectServerErrorMessageForExpiredToken() {
    let server=serve(|r|if r.path=="/backend-api/wham/usage"{(StatusCode::UNAUTHORIZED,r#"{"error":{"message":"Provided authentication token is expired. Please try signing in again.","code":"token_expired"}}"#.into())}else{(StatusCode::FORBIDDEN,"<html>forbidden</html>".into())}).await;
    let urls = ["/backend-api/wham/usage", "/wham/usage", "/api/codex/usage"]
        .map(|p| format!("{}{p}", server.base));
    let error = fetch_usage_from_urls(
        &background_client("").unwrap(),
        &urls,
        "expired-token",
        "account-1",
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        "Provided authentication token is expired. Please try signing in again."
    );
}
#[tokio::test]
async fn testBackgroundNetworkSessionDisablesPersistentHTTPStorage() {
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    let server = serve_headers(move |r| {
        assert!(r.headers.get("cookie").is_none());
        observed.fetch_add(1, Ordering::SeqCst);
        let mut h = HeaderMap::new();
        h.insert(
            "set-cookie",
            HeaderValue::from_static("session=test-only; Path=/"),
        );
        h.insert(
            "cache-control",
            HeaderValue::from_static("public, max-age=3600"),
        );
        (StatusCode::OK, h, "{}".into())
    })
    .await;
    let client = background_client("").unwrap();
    for _ in 0..2 {
        client
            .get(&server.base)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
    }
    assert_eq!(count.load(Ordering::SeqCst), 2);
}
#[test]
fn testUsageDebugRequestSummaryIncludesRequestDetails() {
    let request = background_client("")
        .unwrap()
        .get("https://chatgpt.com/backend-api/wham/usage")
        .header("Accept", "application/json")
        .header("ChatGPT-Account-Id", "account-1")
        .header("User-Agent", "codex-tools-swift/0.1")
        .build()
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&debug_request_summary(&request)).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"method":"GET","url":"https://chatgpt.com/backend-api/wham/usage","headers":{"Accept":"application/json","ChatGPT-Account-Id":"account-1","User-Agent":"codex-tools-swift/0.1"}})
    );
}
#[test]
fn testUsageDebugResponseBodyReturnsRawJSONPayload() {
    assert_eq!(
        debug_response_body(br#"{"detail":{"code":"deactivated_workspace"}}"#),
        r#"{"detail":{"code":"deactivated_workspace"}}"#
    );
}

use crate::codex_config::*;
#[test]
fn testCodexModelProviderResolverUsesActiveProfileProviderAndBaseURL() {
    let p=resolve("model_provider = \"openai\"\nprofile = \"work\"\n[profiles.work]\nmodel_provider = \"my\"\n[model_providers.my]\nbase_url = \"https://sub2.test:6060/v1/\"\n");
    assert_eq!(
        p,
        Definition {
            id: "my".into(),
            base_url: Some("https://sub2.test:6060/v1".into()),
            wire_api: None,
            env_key: None,
            requires_openai_auth: None
        }
    );
    assert!(!p.is_official());
}
#[test]
fn testCodexModelProviderResolverReadsProxyRoutingMetadata() {
    let p=resolve("model_provider = \"my\"\n[model_providers.my]\nbase_url = \"https://sub2.test/v1\"\nwire_api = \"responses\"\nenv_key = \"MY_SUB2API_API_KEY\"\nrequires_openai_auth = false\n");
    assert_eq!(p.id, "my");
    assert_eq!(p.base_url.as_deref(), Some("https://sub2.test/v1"));
    assert_eq!(p.wire_api.as_deref(), Some("responses"));
    assert_eq!(p.env_key.as_deref(), Some("MY_SUB2API_API_KEY"));
    assert_eq!(p.requires_openai_auth, Some(false));
}
#[test]
fn testCodexModelProviderDefinitionsIncludeSiblingProfileConfigs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "model_provider = \"openai\"\n").unwrap();
    std::fs::write(dir.path().join("work.config.toml"),"model_provider = \"profile-sub2api\"\n[model_providers.profile-sub2api]\nbase_url = \"https://profile-sub2.test/v1/\"\nwire_api = \"responses\"\nenv_key = \"PROFILE_SUB2API_KEY\"\nrequires_openai_auth = false\n").unwrap();
    let p = definitions(&path)
        .into_iter()
        .find(|p| p.id == "profile-sub2api")
        .unwrap();
    assert_eq!(p.base_url.as_deref(), Some("https://profile-sub2.test/v1"));
    assert_eq!(p.wire_api.as_deref(), Some("responses"));
    assert_eq!(p.env_key.as_deref(), Some("PROFILE_SUB2API_KEY"));
    assert_eq!(p.requires_openai_auth, Some(false));
    assert_eq!(resolve_path(&path).id, "openai");
}
#[test]
fn testCodexModelProviderDefinitionsIncludeProfileRootProviderWithoutDefinition() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "model_provider = \"openai\"\n").unwrap();
    std::fs::write(
        dir.path().join("work.config.toml"),
        "model_provider = \"profile-sub2api\"\n",
    )
    .unwrap();
    assert!(definitions(&path).iter().any(|p| p.id == "profile-sub2api"));
}
#[test]
fn testCodexProviderResolverMatchesLegacyAdminURLForMigration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path,"model_provider = \"my\"\n[model_providers.my]\nbase_url = \"https://local-sub2.test:6060\"\n[model_providers.ShareCoder]\nbase_url = \"https://sharecoder.test\"\n").unwrap();
    assert_eq!(
        matching_provider("https://sharecoder.test/api/v1", &path).as_deref(),
        Some("ShareCoder")
    );
}
#[test]
fn testCodexModelProviderSwitchServiceUpdatesRootProviderAndPreservesTables() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path,"model_provider = \"openai\" # selected provider\n\n[model_providers.ShareCoder]\nbase_url = \"https://sub2.test/v1\"").unwrap();
    switch(&path, "ShareCoder").unwrap();
    let raw = std::fs::read_to_string(path).unwrap();
    assert!(raw.contains("model_provider = \"ShareCoder\" # selected provider"));
    assert!(raw.contains("[model_providers.ShareCoder]"));
    assert_eq!(resolve(&raw).id, "ShareCoder");
}
#[test]
fn testCodexModelProviderSwitchServiceUpdatesActiveProfileOverride() {
    let raw="model_provider = \"openai\"\nprofile = \"work\"\n\n[profiles.work]\nmodel_provider = \"my\"\n\n[model_providers.ShareCoder]\nbase_url = \"https://sub2.test/v1\"";
    let raw = updating(raw, "ShareCoder").unwrap();
    assert!(raw.contains("model_provider = \"openai\""));
    assert!(raw.contains("[profiles.work]\nmodel_provider = \"ShareCoder\""));
    assert_eq!(resolve(&raw).id, "ShareCoder");
}
#[test]
fn testCodexModelProviderSwitchServiceAddsRootProviderBeforeFirstTable() {
    let raw=updating("# Keep provider definitions intact.\n[model_providers.ShareCoder]\nbase_url = \"https://sub2.test/v1\"","ShareCoder").unwrap();
    assert!(raw.contains("model_provider = \"ShareCoder\"\n[model_providers.ShareCoder]"));
    assert_eq!(resolve(&raw).id, "ShareCoder");
}
#[test]
fn testCodexModelProviderSwitchServiceKeepsRepeatedSwitchesAtRootWithTrailingNewlines() {
    for trailing in ["\n", "\n\n"] {
        let original=format!("# Keep provider definitions intact.\n[model_providers.ShareCoder]\nbase_url = \"https://sub2.test/v1\"\n\n[profiles.work]\nmodel_provider = \"work-provider\"\n\n[features]\nshell_tool = true{trailing}");
        let mut raw = original.clone();
        for id in ["ShareCoder", "openai", "openai", "ShareCoder"] {
            raw = updating(&raw, id).unwrap();
            assert_eq!(
                raw,
                original.replace(
                    "# Keep provider definitions intact.\n",
                    &format!("# Keep provider definitions intact.\nmodel_provider = \"{id}\"\n")
                )
            );
            assert_eq!(resolve(&raw).id, id);
        }
    }
}

use crate::{
    credentials::SecretStore,
    settings::{Provider, Providers},
    sub2api,
};
async fn check_sub2api(explicit: Option<&str>) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = seen.clone();
    let prefix = if explicit == Some("management") {
        "/management"
    } else {
        ""
    };
    let server=serve(move|r|{
        let url=reqwest::Url::parse(&format!("http://fixture{}",r.uri)).unwrap();let path=r.path.strip_prefix(prefix).unwrap();
        recorder.lock().unwrap().push((path.to_owned(),url.query_pairs().map(|(k,v)|(k.into_owned(),v.into_owned())).collect::<std::collections::HashMap<_,_>>(),r.headers.get("authorization").and_then(|v|v.to_str().ok()).map(str::to_owned),r.body.clone()));
        let value=match path {
            "/api/v1/auth/login"=>json!({"code":0,"data":{"access_token":"admin-token"}}),
            "/api/v1/admin/accounts"=>if url.query_pairs().any(|(key,value)|key=="page"&&value=="2"){json!({"code":0,"data":{"items":[{"id":2,"name":"spark-shadow","platform":"openai","type":"oauth","status":"active","parent_account_id":1},{"id":3,"name":"paused@example.com","platform":"openai","type":"oauth","status":"inactive","parent_account_id":null}],"pages":2}})}else{json!({"code":0,"data":{"items":[{"id":1,"name":"openai-2026","platform":"openai","type":"oauth","status":"active","parent_account_id":null}],"pages":2}})},
            "/api/v1/admin/openai/accounts/1/quota"=>json!({"code":0,"data":{"email":"you@example.com","plan_type":"prolite","rate_limit":{"primary_window":{"used_percent":38,"limit_window_seconds":604800,"reset_at":1787810131}},"additional_rate_limits":[{"limit_name":"GPT-5.3-Codex-Spark","rate_limit":{"primary_window":{"used_percent":0,"limit_window_seconds":18000,"reset_at":1787449578}}}]}}),
            _=>panic!("Unexpected request: {path}")
        };(StatusCode::OK,value.to_string())
    }).await;
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("config.toml");
    let model_base = if explicit.is_some() {
        "https://sub2.test:6060/v1".into()
    } else {
        format!("{}/v1", server.base)
    };
    std::fs::write(
        &path,
        format!("model_provider = \"my\"\n[model_providers.my]\nbase_url = \"{model_base}\"\n"),
    )
    .unwrap();
    let admin_base = match explicit {
        None => String::new(),
        Some("management") => format!("{}/management/api/v1", server.base),
        Some("api") => format!("{}/api/v1/", server.base),
        _ => server.base.clone(),
    };
    let provider = Provider {
        provider_id: "my".into(),
        username: "you@example.com".into(),
        admin_base_url: admin_base,
        ..Default::default()
    };
    let secrets = super::MemorySecrets::default();
    secrets.set(&provider.id, "secret").unwrap();
    let settings = Providers {
        confirmed_provider_ids: vec!["my".into()],
        providers: vec![provider],
    };
    let connection = sub2api::connection(&path, &settings, None, &secrets).unwrap();
    let accounts = connection.fetch_accounts(None, 1787431578).await.unwrap();
    assert_eq!(accounts.len(), 1);
    let a = &accounts[0];
    assert_eq!(a.display_email(), "you@example.com");
    assert_eq!(a.summary().normalized_plan_label(), "TEAM");
    assert!(a.summary().team_name.is_none());
    let u = a.usage.as_ref().unwrap();
    assert_eq!(u.fetched_at, 1787431578);
    assert_eq!(u.plan_type.as_deref(), Some("prolite"));
    // Deliberately unlike the Swift original, which showed the Codex Spark
    // limit as the account's five-hour window: as in OpenQuota, an account
    // with only a weekly limit has no five-hour data.
    assert!(u.five_hour.is_none());
    let week = u.one_week.as_ref().unwrap();
    assert_eq!(
        (week.used_percent, week.window_seconds, week.reset_at),
        (38., 604800, Some(1787810131))
    );
    assert_eq!(
        resolve_path(&path).base_url.as_deref(),
        Some(model_base.as_str())
    );
    let requests = seen.lock().unwrap();
    assert_eq!(
        requests.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        [
            "/api/v1/auth/login",
            "/api/v1/admin/accounts",
            "/api/v1/admin/accounts",
            "/api/v1/admin/openai/accounts/1/quota"
        ]
    );
    for (key, value) in [
        ("platform", "openai"),
        ("type", "oauth"),
        ("status", "active"),
    ] {
        assert_eq!(requests[1].1.get(key).map(String::as_str), Some(value));
    }
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&requests[0].3).unwrap(),
        json!({"email":"you@example.com","password":"secret"})
    );
    assert_eq!(
        requests.last().unwrap().2.as_deref(),
        Some("Bearer admin-token")
    );
}
#[tokio::test]
async fn testCustomDefaultProviderListsAccountsAndMapsSub2APIQuota() {
    check_sub2api(None).await;
}
#[tokio::test]
async fn testExplicitAdminURLRoutesLoginAccountsAndQuotaIndependently() {
    for mode in ["root", "api", "management"] {
        check_sub2api(Some(mode)).await;
    }
}
#[tokio::test]
async fn testFailedSub2APIQuotaKeepsLastKnownAccount() {
    let server = serve(|r| {
        let value = match r.path.as_str() {
            "/api/v1/auth/login" => json!({"code":0,"data":{"access_token":"admin-token"}}),
            "/api/v1/admin/accounts" => json!({"code":0,"data":{"items":[{"id":10,"name":"10","platform":"openai","type":"oauth","status":"active","parent_account_id":null}],"pages":1}}),
            "/api/v1/admin/openai/accounts/10/quota" => json!({"code":500,"message":"upstream timeout"}),
            path => panic!("Unexpected request: {path}"),
        };
        (StatusCode::OK, value.to_string())
    })
    .await;
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("config.toml");
    std::fs::write(
        &path,
        format!(
            "model_provider = \"my\"\n[model_providers.my]\nbase_url = \"{}/v1\"\n",
            server.base
        ),
    )
    .unwrap();
    let provider = Provider {
        provider_id: "my".into(),
        username: "admin@example.com".into(),
        imported_account_ids: vec![10],
        cached_accounts: vec![json!({
            "id": 10, "name": "10", "email": "you@example.com", "accountID": "acct-10",
            "accountType": "oauth", "status": "active", "planType": "plus",
            "usage": {"fetchedAt": 1787431000, "planType": "plus", "oneWeek": {"usedPercent": 38, "windowSeconds": 604800, "resetAt": 1787810131}},
            "usageError": null
        })],
        ..Default::default()
    };
    let secrets = super::MemorySecrets::default();
    secrets.set(&provider.id, "secret").unwrap();
    let settings = Providers {
        confirmed_provider_ids: vec!["my".into()],
        providers: vec![provider],
    };
    let connection = sub2api::connection(&path, &settings, None, &secrets).unwrap();
    let accounts = connection.fetch_accounts(None, 1787431578).await.unwrap();
    let a = &accounts[0];
    assert_eq!(a.display_email(), "you@example.com");
    assert_eq!(a.account_id.as_deref(), Some("acct-10"));
    assert_eq!(a.plan_type.as_deref(), Some("plus"));
    assert_eq!(a.usage.as_ref().unwrap().fetched_at, 1787431000);
    assert_eq!(a.usage_error.as_deref(), Some("upstream timeout"));
}
#[test]
fn testConfiguredSub2APIIsNotQueriedWhenItIsNotTheDefaultProvider() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("config.toml");
    std::fs::write(&path,"model_provider = \"other\"\n[model_providers.my]\nbase_url = \"https://sub2.test:6060/v1\"").unwrap();
    let settings = Providers {
        confirmed_provider_ids: vec!["my".into()],
        providers: vec![Provider {
            provider_id: "my".into(),
            username: "you@example.com".into(),
            password: "secret".into(),
            ..Default::default()
        }],
    };
    let secrets = super::MemorySecrets::default();
    match sub2api::connection(&path, &settings, None, &secrets) {
        Err(error) => assert_eq!(error, sub2api::PROVIDER_NOT_FOUND),
        Ok(_) => panic!("Expected provider mismatch"),
    };
    assert!(secrets.gets.lock().unwrap().is_empty());
}
#[tokio::test]
async fn testMultipleSub2APIConfigurationsUseCurrentProviderCredentialsAndBaseURL() {
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = requests.clone();
    let server = serve(move |r| {
        seen.lock().unwrap().push((r.path.clone(), r.body));
        let data = if r.path == "/api/v1/auth/login" {
            json!({"access_token":"share-token"})
        } else {
            json!({"items":[],"pages":1})
        };
        (StatusCode::OK, json!({"code":0,"data":data}).to_string())
    })
    .await;
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("config.toml");
    std::fs::write(&path,format!("model_provider = \"ShareCoder\"\n[model_providers.my]\nbase_url = \"https://my-sub2.test:6060/v1\"\n[model_providers.ShareCoder]\nbase_url = \"{}/v1\"",server.base)).unwrap();
    let settings = Providers {
        confirmed_provider_ids: vec!["my".into(), "ShareCoder".into()],
        providers: vec![
            Provider {
                provider_id: "my".into(),
                username: "my-admin@example.com".into(),
                password: "my-secret".into(),
                ..Default::default()
            },
            Provider {
                provider_id: "ShareCoder".into(),
                username: "share-admin@example.com".into(),
                password: "share-secret".into(),
                ..Default::default()
            },
        ],
    };
    let c = sub2api::connection(&path, &settings, None, &super::MemorySecrets::default()).unwrap();
    assert_eq!(c.admin_base, format!("{}/api/v1", server.base));
    assert!(c.fetch_accounts(None, 1).await.unwrap().is_empty());
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&requests[0].1).unwrap(),
        json!({"email":"share-admin@example.com","password":"share-secret"})
    );
}
#[test]
fn testConfiguredProviderDoesNotRequireExplicitConfirmation() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("config.toml");
    std::fs::write(
        &path,
        "model_provider = \"my\"\n[model_providers.my]\nbase_url = \"https://sub2.test:6060/v1\"",
    )
    .unwrap();
    let settings = Providers {
        providers: vec![Provider {
            provider_id: "my".into(),
            username: "you@example.com".into(),
            password: "secret".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let connection =
        sub2api::connection(&path, &settings, None, &super::MemorySecrets::default()).unwrap();
    assert_eq!(connection.definition.id, "my");
    assert!(connection.configuration.is_enabled());
}
#[test]
fn testSub2APICardWithoutEmailShowsItsAccountName() {
    let account: sub2api::Account = serde_json::from_value(json!({
        "id": 10, "name": " team-pool ", "accountType": "oauth", "status": "active",
        "usageError": "upstream timeout", "providerID": "my"
    }))
    .unwrap();
    let card = account.summary();
    assert_eq!(card.email.as_deref(), Some("team-pool"));
    assert_eq!(card.label, "team-pool");
}
