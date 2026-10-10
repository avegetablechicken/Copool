use super::http_fixture::serve;
use crate::usage::*;
use axum::http::StatusCode;
use std::sync::Arc;

#[test]
fn account_usage_endpoints_are_official() {
    // Without a chatgpt_base_url the endpoints are the official ones; the
    // configured value is read from ~/.codex, so the test passes it in.
    assert_eq!(
        usage_urls(&chatgpt_base_origin(None)),
        vec![
            "https://chatgpt.com/backend-api/wham/usage",
            "https://chatgpt.com/wham/usage",
            "https://chatgpt.com/api/codex/usage",
        ]
    );
}

#[test]
fn loopback_chatgpt_base_url_is_tried_before_the_official_endpoints() {
    assert_eq!(
        usage_urls(&chatgpt_base_origin(Some(
            "https://127.0.0.1:8787/backend-api/"
        ))),
        vec![
            "http://127.0.0.1:8787/backend-api/wham/usage",
            "http://127.0.0.1:8787/api/codex/usage",
            "https://chatgpt.com/backend-api/wham/usage",
            "https://chatgpt.com/api/codex/usage",
        ]
    );
}

#[tokio::test]
async fn codex_usage_fallback_uses_original_headers_and_official_reset_path() {
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = requests.clone();
    let server = serve(move |r| {
        seen.lock().unwrap().push(r.path.clone());
        assert_eq!(r.headers["authorization"], "Bearer fixture-account-token");
        assert_eq!(r.headers["chatgpt-account-id"], "fixture-account");
        assert_eq!(r.headers["accept"], "application/json");
        if r.path == "/api/codex/usage" {
            assert_eq!(r.headers["user-agent"], "codex-tools-swift/0.1");
            (StatusCode::OK, r#"{"plan_type":"plus"}"#.into())
        } else if r.path == "/backend-api/wham/rate-limit-reset-credits" {
            assert_eq!(r.headers["openai-beta"], "codex-1");
            assert_eq!(r.headers["originator"], "Codex Desktop");
            (StatusCode::OK, r#"{"available_count":4}"#.into())
        } else {
            (StatusCode::NOT_FOUND, "{}".into())
        }
    })
    .await;
    let usage = fetch_usage_from_urls(
        &background_client("").unwrap(),
        &[format!("{}/api/codex/usage", server.base)],
        "fixture-account-token",
        "fixture-account",
    )
    .await
    .unwrap();
    assert_eq!(usage.remaining_reset_count, Some(4));
    assert_eq!(
        *requests.lock().unwrap(),
        vec![
            "/api/codex/usage",
            "/backend-api/wham/rate-limit-reset-credits"
        ]
    );
}
