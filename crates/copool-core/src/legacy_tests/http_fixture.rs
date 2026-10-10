use axum::{
    body::{to_bytes, Body},
    http::{HeaderMap, Request, StatusCode},
    response::IntoResponse,
    routing::any,
    Router,
};
use std::sync::Arc;
pub struct RequestData {
    pub uri: String,
    pub path: String,
    pub method: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}
pub struct Server {
    pub base: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub async fn serve(
    handler: impl Fn(RequestData) -> (StatusCode, String) + Send + Sync + 'static,
) -> Server {
    serve_headers(move |request| {
        let (status, body) = handler(request);
        (status, HeaderMap::new(), body)
    })
    .await
}
pub async fn serve_headers(
    handler: impl Fn(RequestData) -> (StatusCode, HeaderMap, String) + Send + Sync + 'static,
) -> Server {
    let handler = Arc::new(handler);
    let router = Router::new().fallback(any(move |request: Request<Body>| {
        let handler = handler.clone();
        async move {
            let (parts, body) = request.into_parts();
            let body = to_bytes(body, 1024 * 1024).await.unwrap();
            let (status, mut headers, body) = handler(RequestData {
                uri: parts.uri.to_string(),
                path: parts.uri.path().into(),
                method: parts.method.to_string(),
                headers: parts.headers,
                body: body.to_vec(),
            });
            headers.insert(
                "content-type",
                axum::http::HeaderValue::from_static("application/json"),
            );
            (status, headers, body).into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server { base, task }
}
