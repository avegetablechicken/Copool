//! Claude's OAuth API as Claude Code uses it, shared by the app and the
//! remote daemon: token renewal and the usage of a login.
#![allow(dead_code)]
use crate::models::{ResetGrant, ResetKind, UsageSnapshot, UsageWindow};
use crate::utils::now_unix_seconds;
use serde_json::{json, Value};

type Result<T> = std::result::Result<T, String>;

/// Claude Code's public OAuth client, which Claude Code accepts as its own login.
pub(crate) const CLIENT: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub(crate) const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
pub(crate) const SCOPES: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
/// What a subscription login adds to its API requests.
pub(crate) const OAUTH_BETA: &str = "oauth-2025-04-20";

// `cedar_ember=1` adds the banked usage-limit resets to the usage response.
pub(crate) const USAGE_PATH: &str = "/api/oauth/usage?cedar_ember=1&skip_spend=1";

/// Where Claude Code sends its login's requests without a base URL.
pub(crate) const API_ROOT: &str = "https://api.anthropic.com";

pub(crate) const USER_AGENT: &str = "claude-code/2.1.69";

// The banked resets are returned only to Claude Code 2.1.280 or later, by its
// User-Agent; others read `eligible: false` with the rest of the usage intact.
// 2.1.322 also understands Fable grants and their weekly-refund flag.
pub(crate) const USAGE_USER_AGENT: &str = "claude-cli/2.1.322 (external, cli)";

pub(crate) const SIGN_IN_EXPIRED: &str = "error.accounts.sign_in_expired";

pub(crate) const TOKEN_REFRESH_RATE_LIMITED: &str = "Claude token refresh failed: HTTP 429";

// reqwest's Display omits the cause (TLS trust, proxy connection, DNS, etc.).
// Keep the chain without Debug-formatting requests or exposing auth headers.
pub(crate) fn request_error(error: reqwest::Error) -> String {
    use std::error::Error;
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(cause) = source {
        let message = cause.to_string();
        if !parts.contains(&message) {
            parts.push(message);
        }
        source = cause.source();
    }
    parts.join(" -> ")
}

/// Renews a login with the request Claude Code sends: a JSON body asking
/// again for the login's scopes, and no User-Agent of its own. The token
/// endpoint answers HTTP 429 to some agents (`claude-code/…`, `curl/…`)
/// whatever the token, while Claude Code's request passes.
pub(crate) async fn refresh_at(
    url: &str,
    refresh_token: &str,
    scope: &str,
    proxy: &str,
) -> Result<Value> {
    let response = crate::utils::with_account_proxy(reqwest::Client::builder(), proxy)?
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?
        .post(url)
        .json(&json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": CLIENT,
            "scope": scope,
        }))
        .send()
        .await
        .map_err(request_error)?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    let code = body["error"]
        .as_str()
        .or_else(|| body["error"]["type"].as_str());
    if matches!(status.as_u16(), 400 | 401) && code == Some("invalid_grant") {
        return Err(SIGN_IN_EXPIRED.into());
    }
    if status.as_u16() == 429 {
        return Err(TOKEN_REFRESH_RATE_LIMITED.into());
    }
    // A proxy in between, or Claude, may say why; shown as usage errors are.
    let message = body["error"]["message"]
        .as_str()
        .or_else(|| body["error_description"].as_str())
        .unwrap_or_default();
    Err(format!(
        "Claude token refresh failed: HTTP {} {message}",
        status.as_u16()
    )
    .trim()
    .to_owned())
}

/// The scopes a login was granted, as Claude Code asks for them on renewal;
/// the default ones for a login that records none.
pub(crate) fn refresh_scope(oauth: &Value) -> String {
    let scopes: Vec<&str> = oauth["scopes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .collect();
    if scopes.is_empty() {
        SCOPES.into()
    } else {
        scopes.join(" ")
    }
}

pub(crate) fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

pub(crate) fn expires_at(oauth: &Value) -> i64 {
    oauth["expiresAt"].as_f64().map_or(0, |v| v as i64)
}

/// "max 20x" from the subscription and its rate limit tier.
pub(crate) fn plan(oauth: &Value) -> Option<String> {
    let subscription = text(&oauth["subscriptionType"])?.to_lowercase();
    let multiplier = text(&oauth["rateLimitTier"]).and_then(|tier| {
        tier.split(|c: char| !c.is_ascii_alphanumeric())
            .find(|p| p.ends_with('x') && p[..p.len() - 1].parse::<u32>().is_ok())
            .map(str::to_owned)
    });
    Some(match multiplier {
        Some(m) => format!("{subscription} {m}"),
        None => subscription,
    })
}

pub(crate) fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
}

pub(crate) fn clock_offset_seconds(value: &str, now: i64) -> Option<i64> {
    let server = chrono::DateTime::parse_from_rfc2822(value.trim()).ok()?;
    server.timestamp().checked_sub(now)
}

pub(crate) fn reset_at(value: &Value) -> Option<i64> {
    if let Some(text) = value.as_str() {
        return chrono::DateTime::parse_from_rfc3339(text.trim())
            .map(|d| d.timestamp())
            .ok()
            .or_else(|| {
                chrono::NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%dT%H:%M:%S%.f")
                    .ok()
                    .map(|d| d.and_utc().timestamp())
            });
    }
    let raw = number(value)?;
    Some(if raw.abs() < 10_000_000_000.0 {
        raw as i64
    } else {
        (raw / 1000.0) as i64
    })
}

/// The five-hour and seven-day windows of the OAuth usage response.
pub(crate) fn usage_snapshot(
    body: &Value,
    plan: Option<String>,
    now: i64,
) -> Result<UsageSnapshot> {
    if !body.is_object() {
        return Err("Invalid Claude usage response".into());
    }
    let window = |key: &str, seconds| {
        body.get(key)
            .filter(|w| w.is_object())
            .map(|w| UsageWindow {
                clock_offset_seconds: None,
                used_percent: number(&w["utilization"]).unwrap_or(0.0).clamp(0.0, 100.0),
                window_seconds: seconds,
                reset_at: reset_at(&w["resets_at"]),
            })
    };
    let (resets, reset_expiries, reset_grants) = banked_resets(&body["cedar_ember"], now).map_or(
        (None, Vec::new(), Vec::new()),
        |(count, expiries, grants)| (Some(count), expiries, grants),
    );
    Ok(UsageSnapshot {
        remaining_reset_count: resets,
        resets_last_confirmed_at: None,
        reset_expiries,
        reset_grants,
        fetched_at: now,
        plan_type: plan,
        five_hour: window("five_hour", 5 * 60 * 60),
        one_week: window("seven_day", 7 * 24 * 60 * 60),
        credits: None,
    })
}

/// Missing inventory leaves the last confirmed resets usable until their expiry.
pub(crate) fn retain_known_resets(latest: &mut UsageSnapshot, previous: Option<&UsageSnapshot>) {
    if latest.remaining_reset_count.is_some() {
        return;
    }
    let Some((previous, count)) = previous.and_then(|p| {
        p.remaining_reset_count
            .filter(|n| *n >= 0)
            .map(|count| (p, count))
    }) else {
        return;
    };
    let now = latest.fetched_at;
    latest.resets_last_confirmed_at = Some(
        previous
            .resets_last_confirmed_at
            .unwrap_or(previous.fetched_at),
    );
    latest.reset_expiries = previous
        .reset_expiries
        .iter()
        .copied()
        .filter(|at| *at > now)
        .collect();
    let remaining = if previous.reset_grants.is_empty() {
        // Older snapshots stored one expiry per reset, where known.
        let expired = previous.reset_expiries.len() - latest.reset_expiries.len();
        (count - expired as i64).max(0)
    } else {
        latest.reset_grants = previous
            .reset_grants
            .iter()
            .filter(|g| g.expires_at.is_none_or(|at| at > now))
            .cloned()
            .collect();
        latest.reset_grants.iter().map(|g| g.count).sum()
    };
    latest.remaining_reset_count = Some(remaining);
}

/// The usage-limit resets the account has banked: what its unexpired grants
/// have left, as claude.ai shows them. An ineligible block says nothing about
/// the account, so it is unknown rather than zero. With the count come when
/// its resets expire, earliest first: each reset a grant has left ends with
/// it. Grant ids, which spend a reset, are never read.
pub(crate) fn banked_resets(block: &Value, now: i64) -> Option<(i64, Vec<i64>, Vec<ResetGrant>)> {
    if block["eligible"].as_bool() != Some(true) {
        return None;
    }
    let grants = block["grants"].as_array()?;
    let (mut count, mut expiries, mut inventory) = (0, Vec::new(), Vec::new());
    for grant in grants {
        let ends = reset_at(&grant["ends_at"]);
        if ends.is_some_and(|end| end <= now) {
            continue;
        }
        let Some(left) = number(&grant["resets_left"]).map(|left| left.max(0.0) as i64) else {
            continue;
        };
        if left == 0 {
            continue;
        }
        // Claude Code classifies the weekly Fable refund by this flag.
        // It does not clear other models' weekly usage, even when it clears 5h.
        let kind = if grant["gives_back_weekly"].as_bool() == Some(true) {
            ResetKind::Fable
        } else if let Some(clears) = grant["clears"].as_array() {
            if clears.iter().any(|v| v == "seven_day") && clears.iter().any(|v| v == "five_hour") {
                ResetKind::Full
            } else {
                ResetKind::Partial
            }
        } else {
            ResetKind::Unknown
        };
        inventory.push(ResetGrant {
            count: left,
            kind,
            expires_at: ends,
        });
        count += left;
        expiries.extend(
            ends.into_iter()
                .flat_map(|end| std::iter::repeat_n(end, left as usize)),
        );
    }
    expiries.sort_unstable();
    Some((count, expiries, inventory))
}

/// Applies a token response to a `claudeAiOauth` entry.
pub(crate) fn apply_tokens(oauth: &mut Value, tokens: &Value, now_ms: i64) -> Result<()> {
    let access = text(&tokens["access_token"]).ok_or("Claude returned no access token")?;
    oauth["accessToken"] = access.into();
    if let Some(refresh) = text(&tokens["refresh_token"]) {
        oauth["refreshToken"] = refresh.into();
    }
    if let Some(seconds) = number(&tokens["expires_in"]) {
        oauth["expiresAt"] = (now_ms + (seconds * 1000.0) as i64).into();
    }
    if let Some(scope) = text(&tokens["scope"]) {
        oauth["scopes"] = scope.split_whitespace().collect::<Vec<_>>().into();
    }
    // Claude Code records when the refresh token itself expires.
    if let Some(seconds) = number(&tokens["refresh_token_expires_in"]) {
        oauth["refreshTokenExpiresAt"] = (now_ms + (seconds * 1000.0) as i64).into();
    }
    Ok(())
}

/// The HTTP status and body of the OAuth usage endpoint, with the seconds of
/// a `Retry-After` header and the server clock's offset.
pub(crate) type UsageResponse = (u16, Value, Option<u64>, Option<i64>);
/// Asks the OAuth usage endpoint as `agent`.
pub(crate) async fn request_usage(
    api: &str,
    token: &str,
    agent: &str,
    proxy: &str,
) -> Result<UsageResponse> {
    let response = crate::proxy_session::client(
        proxy,
        crate::proxy_session::Options {
            user_agent: USER_AGENT.into(),
            ..Default::default()
        },
    )?
    .get(format!("{api}{USAGE_PATH}"))
    .header(reqwest::header::USER_AGENT, agent)
    .bearer_auth(token.trim())
    .header("Accept", "application/json")
    .header("anthropic-beta", OAUTH_BETA)
    .send()
    .await
    .map_err(request_error)?;
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok());
    let clock_offset = response
        .headers()
        .get(reqwest::header::DATE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| clock_offset_seconds(value, now_unix_seconds()));
    let body = response.json().await.unwrap_or(Value::Null);
    Ok((status, body, retry_after, clock_offset))
}
/// Renews a login's `claudeAiOauth` entry in place.
pub(crate) async fn renew(oauth: &mut Value, proxy: &str) -> Result<()> {
    let token = text(&oauth["refreshToken"]).ok_or(SIGN_IN_EXPIRED)?;
    let tokens = refresh_at(TOKEN_URL, &token, &refresh_scope(oauth), proxy).await?;
    apply_tokens(oauth, &tokens, now_unix_seconds() * 1000)
}
/// Whether `a` is a later generation of the same login than `b`.
pub(crate) fn newer_login(a: &Value, b: &Value) -> bool {
    text(&a["accessToken"]).is_some()
        && text(&a["refreshToken"]) != text(&b["refreshToken"])
        && expires_at(a) > expires_at(b)
}
/// A login's usage, asked as Claude Code asks: as `claude-cli/…`, else as
/// `claude-code/…` when the endpoint refuses that agent.
pub(crate) async fn fetch_usage(api: &str, oauth: &Value, proxy: &str) -> Result<UsageSnapshot> {
    let token = text(&oauth["accessToken"]).ok_or(SIGN_IN_EXPIRED)?;
    let mut response = request_usage(api, &token, USAGE_USER_AGENT, proxy).await?;
    if response.0 == 429 {
        response = request_usage(api, &token, USER_AGENT, proxy).await?;
    }
    let (status, body, _, clock_offset) = response;
    match status {
        200..=299 => {
            let mut usage = usage_snapshot(&body, plan(oauth), now_unix_seconds())?;
            for window in [&mut usage.five_hour, &mut usage.one_week]
                .into_iter()
                .flatten()
            {
                window.clock_offset_seconds = clock_offset;
            }
            Ok(usage)
        }
        401 => Err(SIGN_IN_EXPIRED.into()),
        _ => {
            let message = body["error"]["message"].as_str().unwrap_or_default();
            Err(
                format!("Claude usage request failed: HTTP {status} {message}")
                    .trim()
                    .to_owned(),
            )
        }
    }
}
