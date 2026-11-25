use super::read_json_body;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

pub(crate) fn server(
    status: u16,
    body: &'static [u8],
    truncated: bool,
    stall: bool,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        while !request.windows(4).any(|s| s == b"\r\n\r\n") {
            let mut chunk = [0; 4096];
            let read = stream.read(&mut chunk).unwrap();
            assert!(read > 0);
            request.extend_from_slice(&chunk[..read]);
        }
        let length = body.len() + if truncated || stall { 100 } else { 0 };
        write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {length}\r\nRetry-After: 17\r\nConnection: close\r\n\r\n").unwrap();
        stream.write_all(body).unwrap();
        if stall {
            thread::sleep(Duration::from_secs(1));
        }
    });
    (origin, worker)
}

pub(crate) async fn response(
    status: u16,
    body: &'static [u8],
    truncated: bool,
    stall: bool,
) -> (reqwest::Response, thread::JoinHandle<()>) {
    let (origin, worker) = server(status, body, truncated, stall);
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(500))
        .build()
        .unwrap()
        .get(format!("{origin}/?private=synthetic-query-secret"))
        .bearer_auth("synthetic-header-secret")
        .send()
        .await
        .unwrap();
    (response, worker)
}

#[tokio::test]
async fn interrupted_successful_bodies_report_transport_cause_without_secrets() {
    for stall in [false, true] {
        let (response, worker) = response(200, b"{", true, stall).await;
        let error = read_json_body(response, "Test quota").await.unwrap_err();
        assert!(error.contains("response body read"), "{error}");
        assert!(error.contains("HTTP 200"), "{error}");
        assert!(error.contains(" -> "), "{error}");
        if stall {
            assert!(error.contains("timed out"), "{error}");
        }
        assert!(!error.contains("synthetic-query-secret"));
        assert!(!error.contains("synthetic-header-secret"));
        worker.join().unwrap();
    }
}

#[tokio::test]
async fn malformed_successful_json_is_distinct_and_does_not_echo_body() {
    let (response, worker) = response(
        200,
        b"{\"private\":\"synthetic-body-secret\",BAD}",
        false,
        false,
    )
    .await;
    let error = read_json_body(response, "Test quota").await.unwrap_err();
    assert!(error.contains("invalid JSON response"), "{error}");
    assert!(!error.contains("synthetic-body-secret"));
    worker.join().unwrap();
    let (response, worker) = self::response(200, b"{\"used\":25}", false, false).await;
    assert_eq!(
        read_json_body(response, "Test quota").await.unwrap(),
        json!({"used":25})
    );
    worker.join().unwrap();
}

#[tokio::test]
async fn failed_http_statuses_keep_body_failure_details_for_status_handling() {
    for status in [401, 403, 429, 503] {
        for truncated in [false, true] {
            let (response, worker) = response(status, b"not-json", truncated, false).await;
            let body = read_json_body(response, "Test quota").await.unwrap();
            let error = body["error"]["message"].as_str().unwrap();
            assert!(error.contains(&format!("HTTP {status}")), "{error}");
            assert!(
                error.contains(if truncated {
                    "response body read"
                } else {
                    "invalid JSON"
                }),
                "{error}"
            );
            worker.join().unwrap();
        }
    }
}

#[tokio::test]
async fn codex_endpoint_reports_body_timeout_instead_of_only_decoding_error() {
    let (origin, worker) = server(200, b"{", true, true);
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(500))
        .build()
        .unwrap();
    let result = crate::endpoint::EndpointRequests::default()
        .fetch(
            "timeout",
            &[origin],
            move |url| client.get(url).bearer_auth("synthetic-header-secret"),
            Ok::<Value, String>,
        )
        .await;
    let error = result.err().unwrap();
    assert!(error.contains("timed out"), "{error}");
    assert!(error.contains("Codex usage"), "{error}");
    assert!(!error.contains("synthetic-header-secret"));
    worker.join().unwrap();
}

#[tokio::test]
async fn claude_usage_and_token_renewal_propagate_interrupted_bodies() {
    let (origin, worker) = server(200, b"{", true, false);
    let error = crate::claude_api::request_usage(&origin, "synthetic-access", "test", "direct://")
        .await
        .unwrap_err();
    assert!(
        error.contains("Claude usage: response body read failed"),
        "{error}"
    );
    worker.join().unwrap();
    let (origin, worker) = server(200, b"{", true, false);
    let error =
        crate::claude_api::refresh_at(&origin, "synthetic-refresh", "user:profile", "direct://")
            .await
            .unwrap_err();
    assert!(
        error.contains("Claude token refresh: response body read failed"),
        "{error}"
    );
    assert!(!error.contains("synthetic-refresh"));
    worker.join().unwrap();
}

#[tokio::test]
async fn claude_rate_limit_headers_survive_an_unreadable_body() {
    let (origin, worker) = server(429, b"{", true, false);
    let (status, body, retry, _) =
        crate::claude_api::request_usage(&origin, "synthetic-access", "test", "direct://")
            .await
            .unwrap();
    assert_eq!(status, 429);
    assert_eq!(retry, Some(17));
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("response body read failed"));
    worker.join().unwrap();
}
