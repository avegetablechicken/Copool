use super::http_fixture::serve;
use crate::{models::UsageSnapshot, usage};
use axum::http::StatusCode;
async fn fetch(dedicated: &str, embedded: &str, status: u16) -> UsageSnapshot {
    let dedicated = dedicated.to_owned();
    let embedded = embedded.to_owned();
    let server = serve(move |request| {
        if request.path.ends_with("/rate-limit-reset-credits") {
            assert_eq!(request.method, "GET");
            assert_eq!(request.headers["authorization"], "Bearer test-token");
            assert_eq!(request.headers["chatgpt-account-id"], "account-1");
            assert_eq!(request.headers["openai-beta"], "codex-1");
            assert_eq!(request.headers["originator"], "Codex Desktop");
            (StatusCode::from_u16(status).unwrap(), dedicated.clone())
        } else {
            (
                StatusCode::OK,
                format!("{{\"plan_type\":\"plus\",\"rate_limit_reset_credits\":{embedded}}}"),
            )
        }
    })
    .await;
    usage::fetch_usage_from_urls(
        &reqwest::Client::new(),
        &[format!("{}/usage", server.base)],
        "test-token",
        "account-1",
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn testDedicatedCountAndHeadersTakePrecedence() {
    let usage = fetch(r#"{"available_count":3}"#, r#"{"available_count":9}"#, 200).await;
    assert_eq!(usage.remaining_reset_count, Some(3));
    let encoded = serde_json::to_string(&usage).unwrap();
    let decoded: UsageSnapshot = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.remaining_reset_count, Some(3));
}
#[tokio::test]
async fn testUnavailableEndpointFallsBackToEmbeddedCount() {
    assert_eq!(
        fetch("{}", r#"{"available_count":"2"}"#, 404)
            .await
            .remaining_reset_count,
        Some(2)
    );
}
#[tokio::test]
async fn testZeroAndMalformedCounts() {
    for (body, expected) in [
        (r#"{"available_count":0}"#, Some(0)),
        (r#"{"available_count":-1}"#, None),
        (r#"{"available_count":true}"#, None),
        (r#"{"available_count":"invalid"}"#, None),
        ("invalid json", None),
    ] {
        let usage = fetch(body, "null", 200).await;
        assert_eq!(usage.remaining_reset_count, expected);
        assert_eq!(usage.plan_type.as_deref(), Some("plus"));
    }
}
#[test]
fn testOlderSnapshotDecodesWithoutResetCount() {
    let usage: UsageSnapshot = serde_json::from_str(r#"{"fetchedAt":1}"#).unwrap();
    assert_eq!(usage.remaining_reset_count, None);
}
