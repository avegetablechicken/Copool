use super::http_fixture::serve;
use crate::workspaces::*;
use axum::http::StatusCode;
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
#[tokio::test]
async fn testFetchWorkspaceMetadataUsesUsageTimeoutBudget() {
    let server = serve(|_| (StatusCode::OK, r#"{"items":[]}"#.into())).await;
    let client = Client::new(
        reqwest::Client::new(),
        vec![format!("{}/backend-api/accounts", server.base)],
    );
    assert_eq!(
        client
            .request(&server.base, "token-1")
            .build()
            .unwrap()
            .timeout(),
        Some(&std::time::Duration::from_secs(18))
    );
    assert_eq!(client.fetch("token-1").await.unwrap(), vec![]);
}
#[test]
fn testWorkspaceMetadataPrefersConcreteServerErrorOverTimeoutFallback() {
    assert_eq!(preferred_failure(&["https://chatgpt.com/backend-api/accounts -> The request timed out.".into(),"https://chatgpt.com/backend-api/accounts -> 401: Provided authentication token is expired. Please try signing in again.".into()]).as_deref(),Some("Provided authentication token is expired. Please try signing in again."));
}
#[test]
fn testWorkspaceMetadataDebugRequestSummaryIncludesRequestDetails() {
    let request = reqwest::Client::new()
        .get("https://chatgpt.com/backend-api/accounts")
        .header("Accept", "application/json")
        .header("User-Agent", "codex-tools-swift/0.1")
        .build()
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&debug_request(&request)).unwrap();
    assert_eq!(
        value,
        json!({"method":"GET","url":"https://chatgpt.com/backend-api/accounts","headers":{"Accept":"application/json","User-Agent":"codex-tools-swift/0.1"}})
    );
}
#[test]
fn testWorkspaceMetadataDebugResponseBodyReturnsRawJSONPayload() {
    let raw = r#"{"items":[{"id":"account-1","name":"Workspace A","structure":"workspace"}]}"#;
    assert_eq!(debug_response(raw.as_bytes()), raw);
}
#[tokio::test]
async fn testFetchWorkspaceMetadataRetriesOnceAfterHTML403Forbidden() {
    let count = Arc::new(AtomicUsize::new(0));
    let received = count.clone();
    let server=serve(move |_|{if received.fetch_add(1,Ordering::SeqCst)==0{(StatusCode::FORBIDDEN,r#"<html><head><meta name="viewport" content="width=device-width"></head><body>forbidden</body></html>"#.into())}else{(StatusCode::OK,r#"{"items":[{"id":"account-1","name":"Workspace A","structure":"workspace"}]}"#.into())}}).await;
    let client = Client::new(
        reqwest::Client::new(),
        vec![format!("{}/backend-api/accounts", server.base)],
    );
    let metadata = client.fetch("token-1").await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(
        metadata,
        vec![Metadata {
            account_id: "account-1".into(),
            workspace_name: Some("Workspace A".into()),
            structure: Some("workspace".into())
        }]
    );
}
