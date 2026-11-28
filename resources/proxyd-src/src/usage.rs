use serde::Deserialize;
use std::error::Error as StdError;

use crate::models::CreditSnapshot;
use crate::models::UsageSnapshot;
use crate::models::UsageWindow;
use crate::utils::now_unix_seconds;

const DEFAULT_CHATGPT_BASE_URL: &str = "https://chatgpt.com";
const FIVE_HOUR_SECONDS: i64 = 5 * 60 * 60;
const ONE_WEEK_SECONDS: i64 = 7 * 24 * 60 * 60;
const ONE_MONTH_SECONDS: i64 = 30 * 24 * 60 * 60;
const CODEX_USAGE_PATH: &str = "/api/codex/usage";
const WHAM_USAGE_PATH: &str = "/wham/usage";
const BACKEND_API_PREFIX: &str = "/backend-api";

#[derive(Debug, Deserialize)]
struct UsageApiResponse {
    rate_limit_reset_credits: Option<serde_json::Value>,
    plan_type: Option<String>,
    rate_limit: Option<RateLimitDetails>,
    credits: Option<CreditDetails>,
}

#[derive(Debug, Deserialize)]
struct RateLimitDetails {
    primary_window: Option<UsageWindowRaw>,
    secondary_window: Option<UsageWindowRaw>,
}

/// A missing duration or reset time reads as 0: unknown.
#[derive(Debug, Deserialize)]
pub(crate) struct UsageWindowRaw {
    pub(crate) used_percent: f64,
    #[serde(default)]
    pub(crate) limit_window_seconds: i64,
    #[serde(default)]
    pub(crate) reset_at: i64,
}

#[derive(Debug, Deserialize)]
struct CreditDetails {
    has_credits: bool,
    unlimited: bool,
    balance: Option<String>,
}

/// Usage under the resolved ChatGPT origin (`chatgpt_base_origin`).
pub(crate) async fn fetch_usage_snapshot_from(
    origin: &str,
    access_token: &str,
    account_id: &str,
    proxy_url: &str,
) -> Result<UsageSnapshot, String> {
    let usage_urls = usage_urls(origin);

    let client = background_client(proxy_url)?;

    fetch_usage_from_urls(&client, &usage_urls, access_token, account_id).await
}

pub(crate) fn background_client(proxy_url: &str) -> Result<reqwest::Client, String> {
    crate::proxy_session::client(proxy_url, Default::default())
}

pub(crate) async fn fetch_usage_from_urls(
    client: &reqwest::Client,
    usage_urls: &[String],
    access_token: &str,
    account_id: &str,
) -> Result<UsageSnapshot, String> {
    static ENDPOINTS: std::sync::LazyLock<crate::endpoint::EndpointRequests> =
        std::sync::LazyLock::new(Default::default);
    let client_for_request = client.clone();
    let access = access_token.to_owned();
    let account = account_id.to_owned();
    let result = ENDPOINTS
        .fetch(
            &format!("usage:{}", usage_urls.join("|")),
            usage_urls,
            move |url| {
                client_for_request
                    .get(url)
                    .bearer_auth(&access)
                    .header("ChatGPT-Account-Id", &account)
                    .header("Accept", "application/json")
                    .header("User-Agent", "codex-tools-swift/0.1")
            },
            |value| serde_json::from_value::<UsageApiResponse>(value).map_err(|e| e.to_string()),
        )
        .await?;
    let snapshot = map_usage_payload(result.value);
    let confirm = || async {
        let response = client
            .get(&result.endpoint)
            .bearer_auth(access_token)
            .header("ChatGPT-Account-Id", account_id)
            .header("Accept", "application/json")
            .header("User-Agent", "codex-tools-swift/0.1")
            .header("Cache-Control", "no-cache")
            .send()
            .await
            .ok()
            .filter(|response| response.status().is_success())?;
        response
            .json::<UsageApiResponse>()
            .await
            .ok()
            .map(map_usage_payload)
    };
    let mut snapshot = confirm_idle_window(snapshot, confirm).await;
    if let Some(reset_url) = reset_credits_url(&result.endpoint) {
        if let Ok(response) = client
            .get(reset_url)
            .bearer_auth(access_token)
            .header("ChatGPT-Account-Id", account_id)
            .header("Accept", "application/json")
            .header("OpenAI-Beta", "codex-1")
            .header("originator", "Codex Desktop")
            .send()
            .await
        {
            if response.status().is_success() {
                if let Ok(value) = response.json::<serde_json::Value>().await {
                    apply_reset_credits(&mut snapshot, &value);
                }
            }
        }
    }
    Ok(snapshot)
}

/// A reading whose five-hour window may be the placeholder ChatGPT reports
/// for an idle Plus window (empty, resetting five hours from now), read
/// again through `confirm` across a second boundary: a reset that moved
/// with the clock is no reset, and the window keeps its quota without a
/// timer. Each reading's `fetched_at` is when it was answered. A failed
/// confirmation (`None`) keeps the first reading.
pub(crate) async fn confirm_idle_window<F, Fut>(first: UsageSnapshot, confirm: F) -> UsageSnapshot
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Option<UsageSnapshot>>,
{
    if !idle_window_candidate(&first) {
        return first;
    }
    // reset_at has whole-second precision. Cross a second boundary so
    // an immediate confirmation can distinguish a moving placeholder
    // from a fixed deadline, even when both requests are very fast.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    // Confirm only on the endpoint that answered. A failed confirmation
    // must not discard the successful reading or try other origins.
    let Some(mut latest) = confirm().await else {
        return first;
    };
    if moving_idle_window(&first, &latest) {
        if let Some(window) = latest.five_hour.as_mut() {
            // A rolling placeholder is not an actual reset.
            // Preserve the window and quota, but omit its timer.
            window.reset_at = None;
        }
    }
    latest
}
/// Where the reset credits of a usage endpoint are. The /api/codex/usage
/// fallback has no sibling reset endpoint; as in the Swift usage service,
/// reset credits always come from backend-api/wham.
pub(crate) fn reset_credits_url(endpoint: &str) -> Option<String> {
    if let Some(origin) = endpoint.strip_suffix(CODEX_USAGE_PATH) {
        Some(format!(
            "{origin}{BACKEND_API_PREFIX}/wham/rate-limit-reset-credits"
        ))
    } else {
        endpoint
            .rsplit_once('/')
            .map(|(base, _)| format!("{base}/rate-limit-reset-credits"))
    }
}
/// Takes the reset credits a reset-credits answer reports, when it reports
/// how many are left.
pub(crate) fn apply_reset_credits(snapshot: &mut UsageSnapshot, value: &serde_json::Value) {
    if let Some(count) = reset_count(value) {
        snapshot.remaining_reset_count = Some(count);
        snapshot.reset_expiries = reset_expiries(value);
    }
}

fn idle_window_candidate(snapshot: &UsageSnapshot) -> bool {
    snapshot
        .plan_type
        .as_deref()
        .is_some_and(|plan| plan.trim().eq_ignore_ascii_case("plus"))
        && snapshot.five_hour.as_ref().is_some_and(|window| {
            window.used_percent == 0.0
                && [0, FIVE_HOUR_SECONDS].contains(&window.window_seconds)
                && window.reset_at.is_some_and(|reset| {
                    (reset - snapshot.fetched_at - FIVE_HOUR_SECONDS).abs() <= 60
                })
        })
}

fn moving_idle_window(first: &UsageSnapshot, latest: &UsageSnapshot) -> bool {
    if !idle_window_candidate(first) || !idle_window_candidate(latest) {
        return false;
    }
    let elapsed = latest.fetched_at - first.fetched_at;
    let first_reset = first.five_hour.as_ref().and_then(|w| w.reset_at);
    let latest_reset = latest.five_hour.as_ref().and_then(|w| w.reset_at);
    match (first_reset, latest_reset) {
        (Some(before), Some(after)) => {
            // Allow a second of rounding/network jitter, never a fixed reset.
            elapsed > 0 && after > before && (after - before - elapsed).abs() <= 1
        }
        _ => false,
    }
}

/// Try compatible paths on the configured origin. Falling back to a different
/// host would hide a broken configured relay and bypass the user's route.
pub(crate) fn usage_urls(normalized: &str) -> Vec<String> {
    let mut candidates = Vec::new();

    if let Some(origin) = normalized.strip_suffix(BACKEND_API_PREFIX) {
        // 优先 backend-api/wham/usage（兼容性最好）
        candidates.push(format!("{normalized}{WHAM_USAGE_PATH}"));
        candidates.push(format!("{origin}{BACKEND_API_PREFIX}{WHAM_USAGE_PATH}"));
        candidates.push(format!("{origin}{CODEX_USAGE_PATH}"));
    } else {
        candidates.push(format!("{normalized}{BACKEND_API_PREFIX}{WHAM_USAGE_PATH}"));
        candidates.push(format!("{normalized}{WHAM_USAGE_PATH}"));
        candidates.push(format!("{normalized}{CODEX_USAGE_PATH}"));
    }

    let mut deduped = Vec::new();
    for url in candidates {
        if !deduped.iter().any(|existing| existing == &url) {
            deduped.push(url);
        }
    }
    deduped
}

pub(crate) fn resolve_chatgpt_base_origin() -> String {
    chatgpt_base_origin(read_chatgpt_base_url_from_config().as_deref())
}

/// The configured `chatgpt_base_url`, or the official origin without one.
pub(crate) fn chatgpt_base_origin(configured: Option<&str>) -> String {
    let base_url = configured.unwrap_or(DEFAULT_CHATGPT_BASE_URL);
    base_url.trim().trim_end_matches('/').to_owned()
}

/// Reads a JSON API reply without confusing an interrupted body with missing quota.
/// Successful responses must contain valid JSON. For an HTTP failure, retain its
/// status-driven handling (401 renewal, 429 backoff, 5xx fallback) and attach the
/// body failure as an error message instead of discarding the transport cause.
pub(crate) async fn read_json_body(
    response: reqwest::Response,
    operation: &str,
) -> Result<serde_json::Value, String> {
    let status = response.status();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            let problem = if error.is_timeout() {
                "response body read timed out"
            } else {
                "response body read failed"
            };
            let message = format!(
                "{operation}: {problem} (HTTP {}): {}",
                status.as_u16(),
                format_reqwest_error(&error.without_url())
            );
            return if status.is_success() {
                Err(message)
            } else {
                Ok(serde_json::json!({"error": {"message": message}}))
            };
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(value) => Ok(value),
        Err(error) => {
            // Do not include body text or serde's Display: either may quote a
            // credential or other private value from a malformed response.
            let message = format!(
                "{operation}: invalid JSON response (HTTP {}; {:?}, line {}, column {})",
                status.as_u16(),
                error.classify(),
                error.line(),
                error.column()
            );
            if status.is_success() {
                Err(message)
            } else {
                Ok(serde_json::json!({"error": {"message": message}}))
            }
        }
    }
}

pub(crate) fn format_reqwest_error(err: &reqwest::Error) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(next) = source {
        let text = next.to_string();
        if !parts.iter().any(|item| item == &text) {
            parts.push(text);
        }
        source = next.source();
    }
    let message = parts.join(" -> ");
    if certificate_verification_failure(&message) {
        format!("{message} — Certificate verification failed. If you updated the system certificates, fully quit and restart Copool, then retry. Otherwise, check the proxy certificate and system trust settings.")
    } else {
        message
    }
}

/// The daemon's Codex configuration: `CODEX_HOME`, else `~/.codex`. The
/// desktop reads its own Codex configuration instead (`codex_config`).
fn read_chatgpt_base_url_from_config() -> Option<String> {
    let home = std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))?;
    chatgpt_base_url_in(&std::fs::read_to_string(home.join("config.toml")).ok()?)
}

/// The top-level `chatgpt_base_url` of a Codex `config.toml`. Values inside
/// tables such as `[profiles.name]` are not the global setting.
pub(crate) fn chatgpt_base_url_in(contents: &str) -> Option<String> {
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            return None;
        }
        let Some(value) = line
            .strip_prefix("chatgpt_base_url")
            .and_then(|rest| rest.trim_start().strip_prefix('='))
        else {
            continue;
        };
        let value = value.trim();
        let quote = value.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let end = value[1..].find(quote)?;
        let url = value[1..1 + end].trim();
        return (!url.is_empty()).then(|| url.to_owned());
    }

    None
}

fn map_usage_payload(payload: UsageApiResponse) -> UsageSnapshot {
    let (five_hour, one_week, one_month) = classify_windows(payload.rate_limit);
    let (five_hour, one_week, one_month) = (
        five_hour.map(to_usage_window),
        one_week.map(to_usage_window),
        one_month.map(to_usage_window),
    );

    let resets = payload.rate_limit_reset_credits.as_ref();
    UsageSnapshot {
        remaining_reset_count: resets.and_then(reset_count),
        resets_last_confirmed_at: None,
        reset_expiries: resets.map(reset_expiries).unwrap_or_default(),
        reset_grants: Vec::new(),
        fetched_at: now_unix_seconds(),
        plan_type: payload.plan_type,
        five_hour,
        one_week,
        one_month,
        credits: payload.credits.map(|credit| CreditSnapshot {
            // WHAM usage reports the balance without its expiry dates.
            expiries: Vec::new(),
            has_credits: credit.has_credits,
            unlimited: credit.unlimited,
            balance: credit.balance,
        }),
        third_party_five_hour: None,
        third_party_one_week: None,
        fable_one_week: None,
    }
}

/// Splits the account's rate limit into its five-hour, weekly and (for Codex
/// Free) 30-day monthly windows: a window is matched by its duration, so an
/// account with only a weekly or monthly limit has no five-hour window rather
/// than a copy of it. Only a window without a duration falls back to its slot
/// (primary five-hour, secondary weekly). Windows of any other duration are
/// ignored. Additional limits, such as Codex Spark, are separate quotas and
/// never stand in for these.
fn classify_windows(
    rate_limit: Option<RateLimitDetails>,
) -> (
    Option<UsageWindowRaw>,
    Option<UsageWindowRaw>,
    Option<UsageWindowRaw>,
) {
    let (primary, secondary) = rate_limit
        .map(|limit| (limit.primary_window, limit.secondary_window))
        .unwrap_or((None, None));
    let mut known = Vec::new();
    let mut undated = [None, None];
    for (slot, window) in [primary, secondary].into_iter().enumerate() {
        match window {
            Some(w)
                if [FIVE_HOUR_SECONDS, ONE_WEEK_SECONDS, ONE_MONTH_SECONDS]
                    .contains(&w.limit_window_seconds) =>
            {
                known.push(w)
            }
            Some(w) if w.limit_window_seconds <= 0 => undated[slot] = Some(w),
            _ => {}
        }
    }
    let exact =
        |target| pick_nearest_window(&known, target).filter(|w| w.limit_window_seconds == target);
    let [primary, secondary] = undated;
    (
        exact(FIVE_HOUR_SECONDS).or(primary),
        exact(ONE_WEEK_SECONDS).or(secondary),
        exact(ONE_MONTH_SECONDS),
    )
}

pub(crate) fn pick_nearest_window(
    windows: &[UsageWindowRaw],
    target_seconds: i64,
) -> Option<UsageWindowRaw> {
    windows
        .iter()
        .min_by_key(|window| (window.limit_window_seconds - target_seconds).abs())
        .map(|window| UsageWindowRaw {
            used_percent: window.used_percent,
            limit_window_seconds: window.limit_window_seconds,
            reset_at: window.reset_at,
        })
}

fn to_usage_window(window: UsageWindowRaw) -> UsageWindow {
    UsageWindow {
        clock_offset_seconds: None,
        used_percent: window.used_percent,
        window_seconds: window.limit_window_seconds,
        reset_at: Some(window.reset_at).filter(|at| *at > 0),
    }
}

fn reset_count(value: &serde_json::Value) -> Option<i64> {
    let value = value.get("available_count")?;
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
        .filter(|value| *value >= 0)
}

/// When each available reset credit expires, earliest first, as OpenQuota
/// reads them: a credit without a status is available.
fn reset_expiries(value: &serde_json::Value) -> Vec<i64> {
    let mut expiries: Vec<i64> = value["credits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|credit| credit["status"].as_str().is_none_or(|s| s == "available"))
        .filter_map(|credit| {
            let at = &credit["expires_at"];
            at.as_str()
                .and_then(|text| chrono::DateTime::parse_from_rfc3339(text.trim()).ok())
                .map(|date| date.timestamp())
                .or_else(|| at.as_f64().filter(|s| s.is_finite()).map(|s| s as i64))
        })
        .collect();
    expiries.sort_unstable();
    expiries
}

pub(crate) fn debug_request_summary(request: &reqwest::Request) -> String {
    let headers: serde_json::Map<String, serde_json::Value> = request
        .headers()
        .iter()
        .filter(|(name, _)| name.as_str() != "authorization")
        .map(|(name, value)| {
            let display = match name.as_str() {
                "accept" => "Accept",
                "chatgpt-account-id" => "ChatGPT-Account-Id",
                "user-agent" => "User-Agent",
                "content-type" => "Content-Type",
                "authorization" => "Authorization",
                other => other,
            };
            (display.into(), value.to_str().unwrap_or("<binary>").into())
        })
        .collect();
    serde_json::json!({"method":request.method().as_str(),"url":request.url().as_str(),"headers":headers}).to_string()
}
pub(crate) fn debug_response_body(data: &[u8]) -> String {
    std::str::from_utf8(data)
        .map(str::to_owned)
        .unwrap_or_else(|_| format!("<non-utf8 body: {} bytes>", data.len()))
}

pub(crate) fn parse_payload(value: serde_json::Value, now: i64) -> Result<UsageSnapshot, String> {
    let payload: UsageApiResponse = serde_json::from_value(value).map_err(|e| e.to_string())?;
    let mut snapshot = map_usage_payload(payload);
    snapshot.fetched_at = now;
    Ok(snapshot)
}

#[cfg(test)]
mod idle_window_tests {
    use super::*;
    use serde_json::json;

    fn reading(now: i64, reset: i64, used: f64) -> UsageSnapshot {
        parse_payload(
            json!({
                "plan_type": "plus",
                "rate_limit": {"primary_window": {
                    "used_percent": used, "limit_window_seconds": 18000, "reset_at": reset
                }}
            }),
            now,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_moving_placeholder_loses_its_timer_and_a_fixed_reset_keeps_it() {
        let now = now_unix_seconds();
        let first = reading(now, now + FIVE_HOUR_SECONDS, 0.0);
        // The reset moved with the clock: a placeholder.
        let moved = confirm_idle_window(first.clone(), || async {
            Some(reading(now + 2, now + 2 + FIVE_HOUR_SECONDS, 0.0))
        })
        .await;
        assert_eq!(moved.five_hour.as_ref().unwrap().reset_at, None);
        assert_eq!(moved.five_hour.as_ref().unwrap().used_percent, 0.0);
        // It stayed: a window that started.
        let fixed = confirm_idle_window(first.clone(), || async {
            Some(reading(now + 2, now + FIVE_HOUR_SECONDS, 0.0))
        })
        .await;
        assert_eq!(
            fixed.five_hour.as_ref().unwrap().reset_at,
            Some(now + FIVE_HOUR_SECONDS)
        );
        assert_eq!(fixed.fetched_at, now + 2);
        // A failed confirmation keeps the first reading.
        let failed = confirm_idle_window(first.clone(), || async { None }).await;
        assert_eq!(failed.fetched_at, now);
        assert!(failed.five_hour.unwrap().reset_at.is_some());
        // A window in use is never asked again.
        let used = reading(now, now + FIVE_HOUR_SECONDS, 3.0);
        let kept = confirm_idle_window(used, || async { panic!("asked again") }).await;
        assert!(kept.five_hour.unwrap().reset_at.is_some());
    }

    #[test]
    fn only_empty_plus_windows_near_five_hours_need_confirmation() {
        assert!(idle_window_candidate(&reading(1000, 19000, 0.0)));
        assert!(idle_window_candidate(&reading(1000, 18940, 0.0)));
        assert!(!idle_window_candidate(&reading(1000, 18939, 0.0)));
        assert!(!idle_window_candidate(&reading(1000, 19061, 0.0)));
        assert!(!idle_window_candidate(&reading(1000, 19000, 0.1)));
        let mut pro = reading(1000, 19000, 0.0);
        pro.plan_type = Some("pro".into());
        assert!(!idle_window_candidate(&pro));
    }

    #[test]
    fn fixed_resets_or_new_usage_never_confirm_an_idle_window() {
        let first = reading(1000, 19000, 0.0);
        assert!(moving_idle_window(&first, &reading(1002, 19002, 0.0)));
        assert!(!moving_idle_window(&first, &reading(1002, 19000, 0.0)));
        assert!(!moving_idle_window(&first, &reading(1002, 19002, 0.1)));
        assert!(!moving_idle_window(&first, &reading(1000, 19000, 0.0)));
        assert!(!moving_idle_window(&first, &reading(1002, 19020, 0.0)));
        assert!(!moving_idle_window(&first, &reading(999, 19001, 0.0)));
    }
}

/// Certificate verification failures can persist in cached clients after CA updates.
fn certificate_verification_failure(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "invalid peer certificate",
        "certificate verify failed",
        "certificate verification failed",
        "unknownissuer",
        "unknown issuer",
        "unable to get local issuer certificate",
        "self-signed certificate",
        "self signed certificate",
        "certificate has expired",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
}

#[cfg(test)]
mod certificate_hint_tests {
    use super::certificate_verification_failure;

    #[test]
    fn recognizes_verification_errors_without_labeling_other_network_failures() {
        for message in [
            "invalid peer certificate: BadSignature",
            "certificate verify failed: unable to get local issuer certificate",
            "invalid peer certificate: UnknownIssuer",
        ] {
            assert!(certificate_verification_failure(message));
        }
        for message in [
            "connection refused",
            "operation timed out",
            "HTTP 401",
            "TLS handshake EOF",
        ] {
            assert!(!certificate_verification_failure(message));
        }
    }
}

#[cfg(test)]
#[path = "response_body_tests.rs"]
pub(crate) mod response_body_tests;
