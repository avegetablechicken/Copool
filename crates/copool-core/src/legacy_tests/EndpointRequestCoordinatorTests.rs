use super::http_fixture::serve;
use crate::endpoint::EndpointRequests;
use axum::http::StatusCode;
use std::sync::{Arc, Mutex};
#[tokio::test]
async fn testFetchFirstSuccessfulRecordsPreferredEndpointForNextRequest() {
    let urls = Arc::new(Mutex::new(Vec::new()));
    let seen = urls.clone();
    let server = serve(move |r| {
        // A cancelled first-round HTTP request can still reach the server.
        // Count only requests belonging to the second fetch under test.
        if r.headers.get("x-test-round").and_then(|v| v.to_str().ok()) == Some("second") {
            seen.lock().unwrap().push(r.path.clone());
        }
        if r.path == "/fallback" {
            (StatusCode::OK, r#"{"ok":true}"#.into())
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, "primary failed".into())
        }
    })
    .await;
    let candidates = ["primary", "fallback", "tertiary"].map(|p| format!("{}/{p}", server.base));
    let coordinator = EndpointRequests::default();
    coordinator
        .fetch(
            "usage",
            &candidates,
            |url| reqwest::Client::new().get(url),
            Ok,
        )
        .await
        .unwrap();
    let result = coordinator
        .fetch(
            "usage",
            &candidates,
            |url| {
                reqwest::Client::new()
                    .get(url)
                    .header("x-test-round", "second")
            },
            Ok,
        )
        .await
        .unwrap();
    assert_eq!(result.endpoint, candidates[1]);
    assert_eq!(*urls.lock().unwrap(), vec!["/fallback"]);
}
#[tokio::test]
async fn testFetchFirstSuccessfulFallsBackWhenPreferredEndpointValidationFails() {
    let urls = Arc::new(Mutex::new(Vec::new()));
    let seen = urls.clone();
    let server = serve(move |r| {
        seen.lock().unwrap().push(r.path.clone());
        (
            StatusCode::OK,
            if r.path == "/primary" {
                r#"{"unexpected":true}"#
            } else {
                r#"{"ok":true}"#
            }
            .into(),
        )
    })
    .await;
    let candidates = ["primary", "fallback"].map(|p| format!("{}/{p}", server.base));
    let coordinator = EndpointRequests::default();
    let validate = |v: serde_json::Value| {
        v.get("ok")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| "Missing boolean ok".to_string())
    };
    let first = coordinator
        .fetch(
            "usage",
            &candidates,
            |url| reqwest::Client::new().get(url),
            validate,
        )
        .await
        .unwrap();
    assert!(first.value);
    urls.lock().unwrap().clear();
    let second = coordinator
        .fetch(
            "usage",
            &candidates,
            |url| reqwest::Client::new().get(url),
            validate,
        )
        .await
        .unwrap();
    assert!(second.value);
    assert_eq!(*urls.lock().unwrap(), vec!["/fallback"]);
}
