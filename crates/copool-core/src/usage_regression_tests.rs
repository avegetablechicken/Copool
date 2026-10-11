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
    for scheme in ["http", "https"] {
        for host in ["127.0.0.1", "localhost", "[::1]"] {
            let origin = format!("{scheme}://{host}:8787");
            assert_eq!(
                usage_urls(&chatgpt_base_origin(Some(&format!(
                    "{origin}/backend-api/"
                )))),
                vec![
                    format!("{origin}/backend-api/wham/usage"),
                    format!("{origin}/api/codex/usage"),
                    "https://chatgpt.com/backend-api/wham/usage".into(),
                    "https://chatgpt.com/api/codex/usage".into(),
                ]
            );
        }
    }
}

#[test]
fn only_the_top_level_chatgpt_base_url_is_the_setting() {
    let config = "model = \"gpt-5\"\nchatgpt_base_url = 'http://127.0.0.1:8787/backend-api' # relay\n[profiles.work]\nchatgpt_base_url = \"https://work.invalid\"\n";
    assert_eq!(
        chatgpt_base_url_in(config).as_deref(),
        Some("http://127.0.0.1:8787/backend-api")
    );
    assert_eq!(
        chatgpt_base_url_in("[profiles.work]\nchatgpt_base_url = \"https://work.invalid\"\n"),
        None
    );
    assert_eq!(
        chatgpt_base_url_in("chatgpt_base_url_extra = \"x\"\n"),
        None
    );
}

/// Usage, workspaces and the proxy follow the Codex configuration Copool
/// uses (CODEX_HOME or COPOOL_CODEX_AUTH_FILE), not ~/.codex.
#[test]
fn every_codex_request_reads_the_configuration_copool_uses() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::Application::open(crate::Paths {
        data_dir: dir.path().join("data"),
        codex_auth: dir.path().join("codex/auth.json"),
    })
    .unwrap();
    let config = dir.path().join("codex/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        "chatgpt_base_url = \"https://127.0.0.1:8787/backend-api/\"\n[profiles.work]\nchatgpt_base_url = \"https://work.invalid\"\n",
    )
    .unwrap();
    let configured = "https://127.0.0.1:8787/backend-api";
    let network = crate::account_network::Network {
        codex_config: config.clone(),
    };
    assert_eq!(network.origin(), configured);
    assert_eq!(app.storage().upstream_base_url.as_deref(), Some(configured));
    // Without a setting both use the official origin, never ~/.codex.
    std::fs::write(&config, "model = \"gpt-5\"\n").unwrap();
    assert_eq!(network.origin(), "https://chatgpt.com");
    assert_eq!(
        app.storage().upstream_base_url.as_deref(),
        Some("https://chatgpt.com")
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

type Window = Option<(f64, i64, Option<i64>)>;

/// Five-hour and weekly windows, as `(used percent, duration, reset)`.
fn windows(body: serde_json::Value) -> (Window, Window) {
    let usage = parse_payload(body, 0).unwrap();
    let view = |w: Option<crate::models::UsageWindow>| {
        w.map(|w| (w.used_percent, w.window_seconds, w.reset_at))
    };
    (view(usage.five_hour), view(usage.one_week))
}

#[test]
fn usage_windows_are_classified_by_duration() {
    let five =
        serde_json::json!({"used_percent": 10, "limit_window_seconds": 18000, "reset_at": 100});
    let week =
        serde_json::json!({"used_percent": 20, "limit_window_seconds": 604800, "reset_at": 200});
    let expected = (
        Some((10., 18000, Some(100))),
        Some((20., 604800, Some(200))),
    );
    assert_eq!(
        windows(
            serde_json::json!({"rate_limit": {"primary_window": five, "secondary_window": week}})
        ),
        expected
    );
    // The slots do not decide a window of known duration.
    assert_eq!(
        windows(
            serde_json::json!({"rate_limit": {"primary_window": week, "secondary_window": five}})
        ),
        expected
    );
}

#[test]
fn a_sole_weekly_limit_leaves_the_five_hour_window_empty() {
    assert_eq!(
        windows(serde_json::json!({"rate_limit": {
            "primary_window": {"used_percent": 38, "limit_window_seconds": 604800, "reset_at": 300},
            "secondary_window": null
        }})),
        (None, Some((38., 604800, Some(300))))
    );
}

#[test]
fn codex_spark_limits_do_not_stand_in_for_the_account_windows() {
    assert_eq!(
        windows(serde_json::json!({
            "rate_limit": {"primary_window": {"used_percent": 38, "limit_window_seconds": 604800, "reset_at": 300}},
            "additional_rate_limits": [{
                "limit_name": "GPT-5.3-Codex-Spark",
                "rate_limit": {"primary_window": {"used_percent": 0, "limit_window_seconds": 18000, "reset_at": 400}}
            }]
        })),
        (None, Some((38., 604800, Some(300))))
    );
}

#[test]
fn windows_of_other_durations_are_not_shown() {
    assert_eq!(
        windows(serde_json::json!({"rate_limit": {
            "primary_window": {"used_percent": 100, "limit_window_seconds": 2592000, "reset_at": 600},
            "secondary_window": {"used_percent": 30, "limit_window_seconds": 86400, "reset_at": 700}
        }})),
        (None, None)
    );
}

#[test]
fn stored_windows_of_other_durations_are_not_shown() {
    let window = |window_seconds| crate::models::UsageWindow {
        used_percent: 63.,
        window_seconds,
        reset_at: Some(1),
    };
    let stored = |five, week| {
        crate::models::UsageSnapshot {
            remaining_reset_count: None,
            fetched_at: 0,
            plan_type: None,
            five_hour: Some(window(five)),
            one_week: Some(window(week)),
            credits: None,
        }
        .with_matching_windows()
    };
    let shown = |u: crate::models::UsageSnapshot| (u.five_hour.is_some(), u.one_week.is_some());
    // Earlier versions copied the weekly window into the five-hour slot, or
    // a free plan's 30-day window into either.
    assert_eq!(shown(stored(604800, 604800)), (false, true));
    assert_eq!(shown(stored(2592000, 2592000)), (false, false));
    assert_eq!(shown(stored(18000, 604800)), (true, true));
    assert_eq!(shown(stored(0, 0)), (true, true));
}

#[test]
fn windows_without_a_duration_keep_their_slots() {
    assert_eq!(
        windows(serde_json::json!({"rate_limit": {
            "primary_window": {"used_percent": 11, "reset_at": 500},
            "secondary_window": {"used_percent": 22}
        }})),
        (Some((11., 0, Some(500))), Some((22., 0, None)))
    );
}

#[test]
fn a_missing_five_hour_window_counts_as_no_five_hour_limit() {
    let usage = |five: Option<f64>, week: Option<f64>| crate::models::UsageSnapshot {
        remaining_reset_count: None,
        fetched_at: 0,
        plan_type: None,
        five_hour: five.map(|used_percent| crate::models::UsageWindow {
            used_percent,
            window_seconds: 18000,
            reset_at: None,
        }),
        one_week: week.map(|used_percent| crate::models::UsageWindow {
            used_percent,
            window_seconds: 604800,
            reset_at: None,
        }),
        credits: None,
    };
    assert_eq!(usage(Some(60.), Some(20.)).five_hour_remaining(), Some(40.));
    assert_eq!(usage(None, Some(20.)).five_hour_remaining(), Some(100.));
    assert_eq!(usage(None, None).five_hour_remaining(), None);
}
