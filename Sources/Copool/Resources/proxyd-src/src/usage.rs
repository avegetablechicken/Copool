use serde::Deserialize;
use std::error::Error as StdError;

use crate::models::CreditSnapshot;
use crate::models::UsageSnapshot;
use crate::models::UsageWindow;
use crate::utils::now_unix_seconds;

const DEFAULT_CHATGPT_BASE_URL: &str = "https://chatgpt.com";
const CODEX_USAGE_PATH: &str = "/api/codex/usage";
const WHAM_USAGE_PATH: &str = "/wham/usage";
const BACKEND_API_PREFIX: &str = "/backend-api";

#[derive(Debug, Deserialize)]
struct UsageApiResponse {
    rate_limit_reset_credits: Option<serde_json::Value>,
    plan_type: Option<String>,
    rate_limit: Option<RateLimitDetails>,
    additional_rate_limits: Option<Vec<AdditionalRateLimitDetails>>,
    credits: Option<CreditDetails>,
}

#[derive(Debug, Deserialize)]
struct RateLimitDetails {
    primary_window: Option<UsageWindowRaw>,
    secondary_window: Option<UsageWindowRaw>,
}

#[derive(Debug, Deserialize)]
struct AdditionalRateLimitDetails {
    rate_limit: Option<RateLimitDetails>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UsageWindowRaw {
    pub(crate) used_percent: f64,
    pub(crate) limit_window_seconds: i64,
    pub(crate) reset_at: i64,
}

#[derive(Debug, Deserialize)]
struct CreditDetails {
    has_credits: bool,
    unlimited: bool,
    balance: Option<String>,
}

pub(crate) async fn fetch_usage_snapshot(
    access_token: &str,
    account_id: &str,
    proxy_url: &str,
) -> Result<UsageSnapshot, String> {
    let usage_urls = resolve_usage_urls();

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
            },
            |value| serde_json::from_value::<UsageApiResponse>(value).map_err(|e| e.to_string()),
        )
        .await?;
    let mut snapshot = map_usage_payload(result.value);
    if let Some((base, _)) = result.endpoint.rsplit_once('/') {
        if let Ok(response) = client
            .get(format!("{base}/rate-limit-reset-credits"))
            .bearer_auth(access_token)
            .header("ChatGPT-Account-Id", account_id)
            .header("OpenAI-Beta", "codex-1")
            .header("originator", "Codex Desktop")
            .send()
            .await
        {
            if response.status().is_success() {
                if let Ok(value) = response.json::<serde_json::Value>().await {
                    snapshot.remaining_reset_count =
                        reset_count(&value).or(snapshot.remaining_reset_count);
                }
            }
        }
    }
    Ok(snapshot)
}

fn resolve_usage_urls() -> Vec<String> {
    let normalized = resolve_chatgpt_base_origin();
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

    candidates.push("https://chatgpt.com/backend-api/wham/usage".to_string());
    candidates.push(format!("https://chatgpt.com{CODEX_USAGE_PATH}"));

    let mut deduped = Vec::new();
    for url in candidates {
        if !deduped.iter().any(|existing| existing == &url) {
            deduped.push(url);
        }
    }
    deduped
}

pub(crate) fn resolve_chatgpt_base_origin() -> String {
    let base_url =
        read_chatgpt_base_url_from_config().unwrap_or_else(|| DEFAULT_CHATGPT_BASE_URL.to_string());
    base_url.trim_end_matches('/').to_string()
}

fn format_reqwest_error(err: &reqwest::Error) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(next) = source {
        let text = next.to_string();
        if !parts.iter().any(|item| item == &text) {
            parts.push(text);
        }
        source = next.source();
    }
    parts.join(" -> ")
}

fn read_chatgpt_base_url_from_config() -> Option<String> {
    let home = dirs::home_dir()?;
    let config_path = home.join(".codex").join("config.toml");
    let contents = std::fs::read_to_string(config_path).ok()?;

    for line in contents.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("chatgpt_base_url") {
            continue;
        }

        let (_, value) = trimmed.split_once('=')?;
        let cleaned = value.trim().trim_matches('"').trim_matches('\'');
        if !cleaned.is_empty() {
            return Some(cleaned.to_string());
        }
    }

    None
}

fn map_usage_payload(payload: UsageApiResponse) -> UsageSnapshot {
    let mut windows: Vec<UsageWindowRaw> = Vec::new();

    if let Some(rate_limit) = payload.rate_limit {
        if let Some(primary) = rate_limit.primary_window {
            windows.push(primary);
        }
        if let Some(secondary) = rate_limit.secondary_window {
            windows.push(secondary);
        }
    }

    if let Some(additional) = payload.additional_rate_limits {
        for limit in additional {
            if let Some(rate_limit) = limit.rate_limit {
                if let Some(primary) = rate_limit.primary_window {
                    windows.push(primary);
                }
                if let Some(secondary) = rate_limit.secondary_window {
                    windows.push(secondary);
                }
            }
        }
    }

    let five_hour = pick_nearest_window(&windows, 5 * 60 * 60).map(to_usage_window);
    let one_week = pick_nearest_window(&windows, 7 * 24 * 60 * 60).map(to_usage_window);

    UsageSnapshot {
        remaining_reset_count: payload
            .rate_limit_reset_credits
            .as_ref()
            .and_then(reset_count),
        fetched_at: now_unix_seconds(),
        plan_type: payload.plan_type,
        five_hour,
        one_week,
        credits: payload.credits.map(|credit| CreditSnapshot {
            has_credits: credit.has_credits,
            unlimited: credit.unlimited,
            balance: credit.balance,
        }),
    }
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
        used_percent: window.used_percent,
        window_seconds: window.limit_window_seconds,
        reset_at: Some(window.reset_at),
    }
}

fn reset_count(value: &serde_json::Value) -> Option<i64> {
    let value = value.get("available_count")?;
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
        .filter(|value| *value >= 0)
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
