use super::http_fixture::serve;
use crate::{proxy_session::*, settings::Provider, utils::validate_account_proxy};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
#[test]
fn testLegacyConfigurationAndRoundTrip() {
    let p: Provider = serde_json::from_str("{}").unwrap();
    assert_eq!(p.proxy_url, "");
    let p = Provider {
        provider_id: "one".into(),
        proxy_url: " socks5://127.0.0.1:1080 ".into(),
        ..Default::default()
    }
    .normalized();
    let p: Provider = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(p.proxy_url, "socks5://127.0.0.1:1080");
}
#[test]
fn testRejectsInvalidProxyRatherThanFallingBack() {
    for value in [
        "localhost:1080",
        "ftp://localhost:21",
        "http://localhost",
        "http://localhost:0",
        "http://localhost:65536",
        "http://localhost:80/path",
        "http://localhost:80?q=1",
        "http://user:password@localhost:80",
    ] {
        assert!(validate_account_proxy(value).is_err(), "{value}");
    }
}
#[test]
fn testSupportedProtocols() {
    for scheme in ["http", "https", "socks5"] {
        assert!(
            validate_account_proxy(&format!("{scheme}://127.0.0.1:1080"))
                .unwrap()
                .is_some()
        );
    }
}
#[test]
fn testSessionsAreIsolatedAndSystemDefaultsRemainUnchanged() {
    let pool = Sessions::default();
    let fallback = Session::new(Options::default(), None).unwrap();
    assert!(Arc::ptr_eq(
        &pool.session("  ", &fallback).unwrap(),
        &fallback
    ));
    let first = pool.session("http://127.0.0.1:8080", &fallback).unwrap();
    let second = pool.session("socks5://127.0.0.1:1080", &fallback).unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert!(Arc::ptr_eq(
        &pool.session("http://127.0.0.1:8080", &fallback).unwrap(),
        &first
    ));
    assert!(first.proxy_url.is_some());
    assert!(!first.allow_failover);
    assert!(second.proxy_url.is_some());
    assert!(fallback.proxy_url.is_none());
}
#[tokio::test]
async fn testRequestsActuallyReachTheirSelectedHTTPProxy() {
    let a = Arc::new(AtomicUsize::new(0));
    let b = Arc::new(AtomicUsize::new(0));
    let a_count = a.clone();
    let b_count = b.clone();
    let first_server = serve(move |r| {
        assert_eq!(r.method, "CONNECT");
        a_count.fetch_add(1, Ordering::SeqCst);
        (axum::http::StatusCode::BAD_GATEWAY, "test proxy".into())
    })
    .await;
    let second_server = serve(move |r| {
        assert_eq!(r.method, "CONNECT");
        b_count.fetch_add(1, Ordering::SeqCst);
        (axum::http::StatusCode::BAD_GATEWAY, "test proxy".into())
    })
    .await;
    let pool = Sessions::default();
    let fallback = Session::new(
        Options {
            timeout: std::time::Duration::from_secs(3),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    let first = pool.session(&first_server.base, &fallback).unwrap();
    let second = pool.session(&second_server.base, &fallback).unwrap();
    assert!(first
        .client
        .get("https://account-proxy-test.invalid/")
        .send()
        .await
        .is_err());
    assert!(second
        .client
        .get("https://account-proxy-test.invalid/")
        .send()
        .await
        .is_err());
    assert!(a.load(Ordering::SeqCst) > 0);
    assert!(b.load(Ordering::SeqCst) > 0);
}

#[test]
fn testDirectSurvivesRoundTripAndDoesNotReuseProxySession() {
    let p = Provider {
        proxy_url: "direct://".into(),
        ..Default::default()
    };
    let decoded: Provider = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(decoded.proxy_url, "direct://");
    let fallback = Session::new(Options::default(), Some("http://127.0.0.1:9".into())).unwrap();
    let pool = Sessions::default();
    let direct = pool.session(&decoded.proxy_url, &fallback).unwrap();
    assert!(!Arc::ptr_eq(&direct, &fallback));
    assert_eq!(direct.proxy_url.as_deref(), Some("direct://"));
    assert!(Arc::ptr_eq(
        &direct,
        &pool.session("direct://", &fallback).unwrap()
    ));
}
