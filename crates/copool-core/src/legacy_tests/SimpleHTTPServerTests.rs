use super::{fixture, save_fixture, stored};
use crate::proxy_service;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[tokio::test]
async fn testStartThrowsWhenPortAlreadyInUse() {
    let (_dir, app) = fixture();
    save_fixture(&app, json!({"accounts":[stored("a","a","a")]}));
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = occupied.local_addr().unwrap().port();
    let error = proxy_service::start_api_proxy_with_runtime(
        &app.storage(),
        &app.state.api_proxy,
        Some(port),
        "127.0.0.1",
    )
    .await
    .unwrap_err();
    assert!(!error.is_empty());
    assert!(!app.proxy_status().await.unwrap().running);
}
#[tokio::test]
async fn testStreamingResponseUsesChunkedEncoding() {
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = upstream.local_addr().unwrap();
    let router=axum::Router::new().fallback(axum::routing::any(||async{axum::response::Response::builder().header("content-type","text/event-stream").body(axum::body::Body::from_stream(async_stream::stream!{yield Ok::<_,std::convert::Infallible>(axum::body::Bytes::from_static(b"data: first\n\n"));yield Ok::<_,std::convert::Infallible>(axum::body::Bytes::from_static(b"data: second\n\n"));})).unwrap()}));
    let task = tokio::spawn(async move {
        axum::serve(upstream, router).await.unwrap();
    });
    let (_dir, app) = fixture();
    save_fixture(&app, json!({"accounts":[stored("a","a","a")]}));
    let mut storage = app.storage();
    storage.upstream_base_url = Some(format!("http://{address}"));
    let started = proxy_service::start_api_proxy_with_runtime(
        &storage,
        &app.state.api_proxy,
        Some(0),
        "127.0.0.1",
    )
    .await
    .unwrap();
    let body = json!({"model":"gpt-5","input":"hello","stream":true}).to_string();
    let request=format!("POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",started.api_key.unwrap(),body.len(),body);
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", started.port.unwrap()))
        .await
        .unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut data = vec![];
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.read_to_end(&mut data),
    )
    .await
    .unwrap()
    .unwrap();
    let text = String::from_utf8(data).unwrap();
    assert!(
        text.to_lowercase().contains("transfer-encoding: chunked"),
        "{text}"
    );
    assert!(
        text.contains("\r\n\r\nD\r\ndata: first\n\n\r\n")
            || text.contains("\r\n\r\nd\r\ndata: first\n\n\r\n"),
        "{text}"
    );
    assert!(
        text.contains("\r\nE\r\ndata: second\n\n\r\n0\r\n\r\n")
            || text.contains("\r\ne\r\ndata: second\n\n\r\n0\r\n\r\n"),
        "{text}"
    );
    app.stop_proxy().await.unwrap();
    task.abort();
}
