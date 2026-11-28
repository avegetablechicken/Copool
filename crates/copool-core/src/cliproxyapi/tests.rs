use super::*;
use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::any,
    Json, Router,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

/// A request the fake CLIProxyAPI saw: its method and path, its key, and
/// its JSON body.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    key: String,
    body: Value,
}
/// How the fake answers a relayed request: its status, headers and body.
type Upstream = Arc<dyn Fn(&Value) -> (u16, Value, Value) + Send + Sync>;

struct Fake {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fake {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fake {
    fn relayed(&self) -> Vec<Value> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.path == "/v8/management/requests/api-call")
            .map(|s| s.body.clone())
            .collect()
    }
}
async fn serve(files: Value, upstream: Upstream, verify: (u16, Value)) -> Fake {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let router = Router::new().fallback(any(move |headers: HeaderMap, request: Request| {
        let (log, files, upstream, verify) =
            (log.clone(), files.clone(), upstream.clone(), verify.clone());
        async move {
            let method = request.method().to_string();
            let path = request.uri().path().to_owned();
            let bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let key = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            log.lock().unwrap().push(Seen {
                method,
                path: path.clone(),
                key: key.clone(),
                body: body.clone(),
            });
            if key != "Bearer management-key" {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error": "invalid key"})),
                )
                    .into_response();
            }
            match path.as_str() {
                "/v8/management/credentials" => Json(json!({"files": files})).into_response(),
                "/v8/management/requests/api-call" => {
                    let (status, header, answer) = upstream(&body);
                    Json(json!({
                        "status_code": status,
                        "header": header,
                        "body": answer.to_string(),
                    }))
                    .into_response()
                }
                "/v8/management/credentials/verify" => {
                    (StatusCode::from_u16(verify.0).unwrap(), Json(verify.1)).into_response()
                }
                _ => StatusCode::NOT_FOUND.into_response(),
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Fake { base, seen, server }
}
fn connection(fake: &Fake) -> Connection {
    Connection {
        management: management_base(&fake.base).unwrap(),
        key: "management-key".into(),
        proxy_url: "direct://".into(),
        insecure: false,
        provider_id: "cpa".into(),
        cached: Vec::new(),
    }
}
fn credential(provider: &str, index: &str, extra: Value) -> Value {
    let mut item = json!({
        "id": format!("{index}.json"), "auth_index": index, "name": format!("{index}@example.com.json"),
        "provider": provider, "status": "active", "disabled": false, "unavailable": false,
    });
    for (key, value) in extra.as_object().unwrap() {
        item[key] = value.clone();
    }
    item
}
fn header<'a>(relayed: &'a Value, name: &str) -> &'a str {
    relayed["header"][name].as_str().unwrap_or_default()
}

#[test]
fn the_management_api_is_found_under_any_address_of_the_server() {
    for raw in [
        "http://127.0.0.1:8317",
        "http://127.0.0.1:8317/",
        "http://127.0.0.1:8317/v1",
        "http://127.0.0.1:8317/v1beta/",
        "http://127.0.0.1:8317/v0/management",
        "http://127.0.0.1:8317/v8/management/",
    ] {
        assert_eq!(
            management_base(raw).unwrap(),
            "http://127.0.0.1:8317/v8/management",
            "{raw}"
        );
    }
    assert!(management_base("ftp://host").is_err());
    assert!(management_base("not a url").is_err());
}

#[test]
fn a_credential_is_always_the_same_positive_account() {
    let id = account_id("0123456789abcdef");
    assert_eq!(id, account_id(" 0123456789abcdef "));
    assert!(id > 0);
    assert_ne!(id, account_id("fedcba9876543210"));
    // A card's id ends with its account, as every provider card's does.
    let card = crate::provider_accounts::card_id(Pool::Claude, "provider", id);
    assert_eq!(
        crate::provider_accounts::card_provider(Pool::Claude, &card),
        Some("provider")
    );
}

#[tokio::test]
async fn codex_usage_is_asked_of_chatgpt_as_the_credential() {
    let files = json!([
        credential(
            "codex",
            "aaaa",
            json!({"id_token": {"chatgpt_account_id": "acct-1", "plan_type": "plus"}})
        ),
        credential("codex", "bbbb", json!({"disabled": true})),
        credential("codex", "cccc", json!({"status": "disabled"})),
        credential("claude", "dddd", json!({})),
        credential("codex", "", json!({})),
    ]);
    let upstream: Upstream = Arc::new(|request| {
        if request["url"]
            .as_str()
            .unwrap()
            .ends_with("/rate-limit-reset-credits")
        {
            return (
                200,
                json!({}),
                json!({"available_count": 2, "credits": [
                    {"status": "available", "expires_at": "2099-01-02T00:00:00Z"},
                    {"status": "available", "expires_at": "2099-01-01T00:00:00Z"}]}),
            );
        }
        (
            200,
            json!({}),
            json!({"plan_type": "plus", "rate_limit": {"primary_window": {
                "used_percent": 25, "limit_window_seconds": 18000, "reset_at": 4_000_000_000i64}}}),
        )
    });
    let fake = serve(files, upstream, (200, json!({"status": "ok"}))).await;
    let before = crate::utils::now_unix_seconds();
    let accounts = connection(&fake)
        .fetch_accounts(Pool::Codex, None, 100)
        .await
        .unwrap();
    assert_eq!(accounts.len(), 1);
    let account = &accounts[0];
    assert_eq!(account.id, account_id("aaaa"));
    assert_eq!(account.credential.as_deref(), Some("aaaa"));
    assert_eq!(account.email.as_deref(), Some("aaaa@example.com"));
    assert_eq!(account.account_id.as_deref(), Some("acct-1"));
    assert_eq!(account.provider_id.as_deref(), Some("cpa"));
    assert_eq!(account.plan_type.as_deref(), Some("plus"));
    let usage = account.usage.as_ref().unwrap();
    // As of its answer, as the pool's own readings are.
    assert!(usage.fetched_at >= before);
    assert_eq!(usage.five_hour.as_ref().unwrap().used_percent, 25.0);
    assert_eq!(usage.remaining_reset_count, Some(2));
    assert_eq!(usage.reset_expiries.len(), 2);
    assert!(usage.reset_expiries[0] < usage.reset_expiries[1]);
    // A window in use is read once; the reset credits as the pool reads them.
    let relayed = fake.relayed();
    assert_eq!(relayed.len(), 2);
    assert_eq!(relayed[0]["auth_index"], "aaaa");
    assert_eq!(relayed[0]["method"], "GET");
    assert_eq!(
        relayed[0]["url"],
        "https://chatgpt.com/backend-api/wham/usage"
    );
    assert_eq!(header(&relayed[0], "Authorization"), "Bearer $TOKEN$");
    assert_eq!(header(&relayed[0], "ChatGPT-Account-Id"), "acct-1");
    assert_eq!(
        relayed[1]["url"],
        "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits"
    );
    assert_eq!(header(&relayed[1], "OpenAI-Beta"), "codex-1");
    assert_eq!(header(&relayed[1], "ChatGPT-Account-Id"), "acct-1");
}

#[tokio::test]
async fn an_idle_codex_plus_window_is_confirmed_and_its_placeholder_reset_dropped() {
    let files = json!([credential("codex", "aaaa", json!({}))]);
    // ChatGPT's idle Plus window: empty, resetting five hours after each
    // request.
    let upstream: Upstream = Arc::new(|request| {
        if request["url"]
            .as_str()
            .unwrap()
            .ends_with("/rate-limit-reset-credits")
        {
            return (404, json!({}), json!({}));
        }
        let reset = crate::utils::now_unix_seconds() + 18000;
        (
            200,
            json!({}),
            json!({"plan_type": "plus", "rate_limit": {"primary_window": {
                "used_percent": 0, "limit_window_seconds": 18000, "reset_at": reset}}}),
        )
    });
    let fake = serve(files, upstream, (200, json!({"status": "ok"}))).await;
    let accounts = connection(&fake)
        .fetch_accounts(Pool::Codex, None, 0)
        .await
        .unwrap();
    let five = accounts[0]
        .usage
        .as_ref()
        .unwrap()
        .five_hour
        .clone()
        .unwrap();
    assert_eq!(five.reset_at, None);
    assert_eq!(five.used_percent, 0.0);
    let relayed = fake.relayed();
    assert_eq!(relayed.len(), 3);
    assert_eq!(header(&relayed[1], "Cache-Control"), "no-cache");
    assert_eq!(relayed[1]["url"], relayed[0]["url"]);
}

#[tokio::test]
async fn claude_usage_retries_a_refused_agent_and_keeps_the_server_clock() {
    let files = json!([credential("claude", "aaaa", json!({}))]);
    let upstream: Upstream = Arc::new(|request| {
        if header(request, "User-Agent") == crate::claude_api::USAGE_USER_AGENT {
            return (429, json!({}), json!({"error": {"message": "slow down"}}));
        }
        (
            200,
            json!({"Date": ["Thu, 01 Jan 2099 00:00:00 GMT"]}),
            json!({"five_hour": {"utilization": 40, "resets_at": "2099-01-01T05:00:00Z"},
                   "seven_day": {"utilization": 10, "resets_at": "2099-01-07T00:00:00Z"}}),
        )
    });
    let fake = serve(files, upstream, (200, json!({"status": "ok"}))).await;
    let accounts = connection(&fake)
        .fetch_accounts(Pool::Claude, None, 0)
        .await
        .unwrap();
    let usage = accounts[0].usage.as_ref().unwrap();
    let five = usage.five_hour.as_ref().unwrap();
    assert_eq!(five.used_percent, 40.0);
    assert!(five.clock_offset_seconds.is_some());
    let relayed = fake.relayed();
    assert_eq!(relayed.len(), 2);
    assert!(relayed[0]["url"]
        .as_str()
        .unwrap()
        .starts_with("https://api.anthropic.com/api/oauth/usage"));
    assert_eq!(
        header(&relayed[1], "User-Agent"),
        crate::claude_api::USER_AGENT
    );
    assert_eq!(
        header(&relayed[1], "anthropic-beta"),
        crate::claude_api::OAUTH_BETA
    );
}

#[tokio::test]
async fn each_pool_asks_its_service_and_a_refused_token_says_so() {
    for (pool, provider, host) in [
        (Pool::Codex, "codex", "https://chatgpt.com/"),
        (Pool::Antigravity, "antigravity", "googleapis.com"),
        (Pool::Grok, "xai", "https://cli-chat-proxy.grok.com/v1/"),
    ] {
        let files = json!([credential(provider, "aaaa", json!({}))]);
        let upstream: Upstream = Arc::new(|_| {
            (
                401,
                json!({}),
                json!({"error": {"status": "UNAUTHENTICATED"}}),
            )
        });
        let fake = serve(files, upstream, (200, json!({"status": "ok"}))).await;
        let accounts = connection(&fake)
            .fetch_accounts(pool, None, 0)
            .await
            .unwrap();
        assert_eq!(
            accounts[0].usage_error.as_deref(),
            Some(crate::claude_api::SIGN_IN_EXPIRED),
            "{pool:?}"
        );
        let relayed = fake.relayed();
        assert!(!relayed.is_empty(), "{pool:?}");
        for request in &relayed {
            assert!(request["url"].as_str().unwrap().contains(host), "{request}");
            assert_eq!(header(request, "Authorization"), "Bearer $TOKEN$");
        }
    }
}

#[tokio::test]
async fn a_failed_reading_keeps_the_last_one_and_only_asked_cards_are_read() {
    let files = json!([
        credential("claude", "aaaa", json!({})),
        credential("claude", "bbbb", json!({})),
    ]);
    let upstream: Upstream = Arc::new(|_| (500, json!({}), json!({"error": {"message": "down"}})));
    let fake = serve(files, upstream, (200, json!({"status": "ok"}))).await;
    let mut connection = connection(&fake);
    let last: Account = serde_json::from_value(json!({
        "id": account_id("aaaa"), "name": "a", "email": "kept@example.com",
        "accountType": "oauth", "status": "active", "planType": "max",
        "usage": {"fetchedAt": 5, "fiveHour": {"usedPercent": 12, "windowSeconds": 18000}},
    }))
    .unwrap();
    connection.cached = vec![last];
    let asked = [account_id("aaaa")];
    let accounts = connection
        .fetch_accounts(Pool::Claude, Some(&asked), 0)
        .await
        .unwrap();
    // Only the card asked for is answered: one unread would take the place
    // of its last reading.
    assert_eq!(accounts.len(), 1);
    let read = &accounts[0];
    assert!(read.usage_error.as_deref().unwrap().contains("HTTP 500"));
    assert_eq!(read.usage.as_ref().unwrap().fetched_at, 5);
    assert_eq!(read.plan_type.as_deref(), Some("max"));
    assert_eq!(fake.relayed().len(), 1);
}

// CLIProxyAPI's own reason a credential is cooling down or refused stays on
// its card, though its usage is read; a failed reading's own reason replaces
// it.
#[tokio::test]
async fn a_credentials_status_shows_beside_its_usage() {
    let cooling = json!({"unavailable": true, "status_message": "quota exhausted"});
    let files = json!([
        credential("claude", "aaaa", cooling.clone()),
        credential("claude", "bbbb", cooling),
        credential("claude", "cccc", json!({"status_message": "fine"})),
    ]);
    let upstream: Upstream = Arc::new(|request| {
        if request["auth_index"] == "bbbb" {
            return (500, json!({}), json!({"error": {"message": "down"}}));
        }
        (
            200,
            json!({}),
            json!({"five_hour": {"utilization": 10, "resets_at": "2099-01-01T00:00:00Z"}}),
        )
    });
    let fake = serve(files, upstream, (200, json!({"status": "ok"}))).await;
    let accounts = connection(&fake)
        .fetch_accounts(Pool::Claude, None, 0)
        .await
        .unwrap();
    let ids: Vec<_> = accounts.iter().map(|a| a.id).collect();
    assert_eq!(ids, ["aaaa", "bbbb", "cccc"].map(account_id));
    assert!(accounts[0].usage.is_some());
    assert_eq!(accounts[0].usage_error.as_deref(), Some("quota exhausted"));
    assert!(accounts[1]
        .usage_error
        .as_deref()
        .unwrap()
        .contains("HTTP 500"));
    assert!(accounts[2].usage.is_some() && accounts[2].usage_error.is_none());
}

#[tokio::test]
async fn a_hi_is_the_credentials_inference_check() {
    let account: Account = serde_json::from_value(json!({
        "id": account_id("aaaa"), "name": "a", "accountType": "oauth",
        "status": "active", "credential": "aaaa",
    }))
    .unwrap();
    let fake = serve(
        json!([]),
        Arc::new(|_| (200, json!({}), json!({}))),
        (200, json!({"status": "ok", "verified": true})),
    )
    .await;
    connection(&fake)
        .say_hi(Pool::Claude, &account, None)
        .await
        .unwrap();
    let seen = fake.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].path, "/v8/management/credentials/verify");
    assert_eq!(seen[0].body, json!({"auth_index": "aaaa"}));

    let refused = serve(
        json!([]),
        Arc::new(|_| (200, json!({}), json!({}))),
        (
            502,
            json!({"status": "error", "verified": false, "error": "upstream said no"}),
        ),
    )
    .await;
    assert_eq!(
        connection(&refused)
            .say_hi(Pool::Claude, &account, None)
            .await
            .unwrap_err(),
        "upstream said no"
    );
}

// The inference check sends one model alone: an Antigravity credential
// says hi through its relay as the pool's own accounts do, its project and
// models asked, then each idle window's model.
#[tokio::test]
async fn an_antigravity_hi_goes_to_each_idle_window_through_the_relay() {
    let account: Account = serde_json::from_value(json!({
        "id": account_id("aaaa"), "name": "a", "accountType": "oauth",
        "status": "active", "credential": "aaaa",
    }))
    .unwrap();
    let upstream: Upstream = Arc::new(|request| {
        let url = request["url"].as_str().unwrap();
        let reset = json!({"remainingFraction": 1, "resetTime": "2099-01-01T00:00:00Z"});
        let body = if url.ends_with(":loadCodeAssist") {
            json!({"cloudaicompanionProject": "project-1"})
        } else if url.ends_with(":fetchAvailableModels") {
            json!({
                "models": {
                    "gemini-3-flash": {"quotaInfo": reset},
                    "gemini-3.8-flash-low": {"quotaInfo": reset},
                    "gpt-oss-120b-medium": {"quotaInfo": reset}
                },
                "agentModelSorts": [{"groups": [{"modelIds": ["gemini-3.8-flash-low"]}]}],
                "commandModelIds": ["gemini-3-flash"]
            })
        } else {
            json!({})
        };
        (200, json!({}), body)
    });
    let fake = serve(json!([]), upstream, (500, json!({}))).await;
    let connection = connection(&fake);
    connection
        .say_hi(Pool::Antigravity, &account, None)
        .await
        .unwrap();
    connection
        .say_hi(Pool::Antigravity, &account, Some(true))
        .await
        .unwrap();
    let relayed = fake.relayed();
    assert!(fake
        .seen
        .lock()
        .unwrap()
        .iter()
        .all(|s| s.path != "/v8/management/credentials/verify"));
    let said: Vec<String> = relayed
        .iter()
        .filter(|r| r["url"].as_str().unwrap().ends_with(":generateContent"))
        .map(|r| {
            assert_eq!(r["auth_index"], "aaaa");
            assert_eq!(header(r, "Authorization"), "Bearer $TOKEN$");
            let data: Value = serde_json::from_str(r["data"].as_str().unwrap()).unwrap();
            assert_eq!(data["project"], "project-1");
            data["model"].as_str().unwrap().to_owned()
        })
        .collect();
    // No usage yet: both windows are idle; then the third party's alone.
    assert_eq!(
        said,
        [
            "gemini-3.8-flash-low",
            "gpt-oss-120b-medium",
            "gpt-oss-120b-medium"
        ]
    );
}

#[tokio::test]
async fn a_wrong_key_or_a_closed_management_api_is_explained() {
    let fake = serve(
        json!([]),
        Arc::new(|_| (200, json!({}), json!({}))),
        (200, json!({})),
    )
    .await;
    let mut wrong = connection(&fake);
    wrong.key = "other".into();
    assert_eq!(
        wrong
            .fetch_accounts(Pool::Codex, None, 0)
            .await
            .unwrap_err(),
        "error.cliproxyapi.management_key_refused"
    );
    let mut closed = connection(&fake);
    closed.management = format!("{}/elsewhere", fake.base);
    assert_eq!(
        closed
            .fetch_accounts(Pool::Codex, None, 0)
            .await
            .unwrap_err(),
        "error.cliproxyapi.management_unavailable"
    );
}
