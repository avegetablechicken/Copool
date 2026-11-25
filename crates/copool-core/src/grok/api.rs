//! xAI's API as Grok Build reads it: the plan's usage limit
//! (`/billing?format=credits`, a share of the current period's allowance),
//! the plan (`/settings`), and token renewal with Grok Build's OAuth client
//! at `auth.x.ai`.
use super::login::{CLIENT_ID, ISSUER};
use crate::models::{CreditSnapshot, UsageSnapshot, UsageWindow};
use serde_json::Value;

pub(crate) const API_BASE: &str = "https://cli-chat-proxy.grok.com/v1";
pub(crate) const USAGE_PATH: &str = "/billing?format=credits";
pub(crate) const SETTINGS_PATH: &str = "/settings";
pub(crate) const MODELS_PATH: &str = "/models";
const AUTHORIZE_PATH: &str = "/oauth2/authorize";
const TOKEN_PATH: &str = "/oauth2/token";
/// What Grok Build's sign-in asks for (`default_oauth2_scopes` in its
/// `xai-grok-login`): without the conversation and workspace scopes, its
/// cloud conversations and workspaces refuse the login.
const SCOPES: &str = "openid profile email offline_access grok-cli:access api:access \
conversations:read conversations:write workspaces:read workspaces:write";
/// The Grok Build release whose requests Copool's resemble.
pub(crate) const CLIENT_VERSION: &str = "1.0.92";
const ONE_WEEK: i64 = 7 * 24 * 60 * 60;
const ONE_DAY: i64 = 24 * 60 * 60;

type Response<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send + 'a>>;
/// An HTTP status and its JSON body (null when it has none).
pub(crate) type Reply = (u16, Value);

/// Where the pool's requests go; tests stand in for xAI.
pub(crate) trait Network: Send + Sync {
    /// A GET of the Grok API with the access token.
    fn api<'a>(&'a self, path: &'a str, token: &'a str, proxy: &'a str) -> Response<'a, Reply>;
    /// Renews the access token with the refresh token.
    fn renew<'a>(&'a self, refresh_token: &'a str, proxy: &'a str) -> Response<'a, Reply>;
    /// A Sub2api gateway's `/v1/models`, with the key Grok Build sends it.
    fn gateway_models<'a>(
        &'a self,
        url: &'a str,
        key: &'a str,
        proxy: &'a str,
        insecure: bool,
    ) -> Response<'a, Reply>;
}

pub(crate) struct Http;
fn client(proxy: &str) -> Result<reqwest::Client, String> {
    crate::utils::with_account_proxy(reqwest::Client::builder(), proxy)?
        .user_agent(format!("grok-cli/{CLIENT_VERSION}"))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())
}
async fn reply(response: reqwest::Response) -> Reply {
    let status = response.status().as_u16();
    let body = response.json().await.unwrap_or(Value::Null);
    (status, body)
}
impl Network for Http {
    fn api<'a>(&'a self, path: &'a str, token: &'a str, proxy: &'a str) -> Response<'a, Reply> {
        Box::pin(async move {
            let response = client(proxy)?
                .get(format!("{API_BASE}{path}"))
                .bearer_auth(token)
                .header("Accept", "application/json")
                .header("x-grok-client-version", CLIENT_VERSION)
                .send()
                .await
                .map_err(|e| crate::usage::format_reqwest_error(&e))?;
            Ok(reply(response).await)
        })
    }
    fn renew<'a>(&'a self, refresh_token: &'a str, proxy: &'a str) -> Response<'a, Reply> {
        Box::pin(async move {
            let response = client(proxy)?
                .post(format!("{ISSUER}{TOKEN_PATH}"))
                .form(&[
                    ("grant_type", "refresh_token"),
                    ("client_id", CLIENT_ID),
                    ("refresh_token", refresh_token),
                ])
                .send()
                .await
                .map_err(|e| crate::usage::format_reqwest_error(&e))?;
            Ok(reply(response).await)
        })
    }
    fn gateway_models<'a>(
        &'a self,
        url: &'a str,
        key: &'a str,
        proxy: &'a str,
        insecure: bool,
    ) -> Response<'a, Reply> {
        Box::pin(async move {
            let response = crate::utils::with_account_proxy(reqwest::Client::builder(), proxy)?
                .danger_accept_invalid_certs(insecure)
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| e.to_string())?
                .get(url)
                .bearer_auth(key)
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(|e| crate::usage::format_reqwest_error(&e))?;
            Ok(reply(response).await)
        })
    }
}

/// Where the browser signs in to xAI with Grok Build's client.
pub(crate) fn sign_in_url(redirect: &str, challenge: &str, state: &str) -> String {
    let mut url = reqwest::Url::parse(&format!("{ISSUER}{AUTHORIZE_PATH}")).expect("valid URL");
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", redirect),
        ("scope", SCOPES),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ]);
    url.into()
}
/// Exchanges a sign-in's code for its tokens, through the system proxy as
/// the other sign-ins.
pub(crate) async fn exchange(code: &str, verifier: &str, redirect: &str) -> Result<Value, String> {
    let response = client("")?
        .post(format!("{ISSUER}{TOKEN_PATH}"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect),
        ])
        .send()
        .await
        .map_err(|e| crate::usage::format_reqwest_error(&e))?;
    let (status, body) = reply(response).await;
    if !(200..300).contains(&status) {
        return Err(status_error("OAuth token exchange", status, &body));
    }
    Ok(body)
}

pub(crate) fn status_error(what: &str, status: u16, body: &Value) -> String {
    let message = body
        .pointer("/error/message")
        .or_else(|| body.get("error_description"))
        .or_else(|| body.get("message"))
        .or_else(|| body.get("error"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("{what} failed: HTTP {status} {message}")
        .trim()
        .to_owned()
}

/// A number, or one given as text; never NaN or infinite, which `"NaN"`
/// and `"inf"` parse to and the store cannot keep.
fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        .filter(|number: &f64| number.is_finite())
}
fn time(value: &Value) -> Option<i64> {
    value
        .as_str()
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text.trim()).ok())
        .map(|date| date.timestamp())
}
/// The prepaid extra-usage balance of `/billing?format=credits`
/// (`prepaidBalance.val`, in USD cents; proto3 JSON may omit a zero `val`).
/// None when the response carries no `prepaidBalance` or an unreadable amount.
fn prepaid_credits(config: &Value) -> Option<CreditSnapshot> {
    let raw = config.get("prepaidBalance")?;
    let cents = match raw {
        Value::Null => return None,
        Value::Object(map) => match map.get("val") {
            None | Some(Value::Null) => 0.0,
            Some(value) => number(value)?,
        },
        value => number(value)?,
    };
    let dollars = cents.max(0.0) / 100.0;
    Some(CreditSnapshot {
        expiries: Vec::new(),
        has_credits: dollars > 0.0,
        unlimited: false,
        balance: Some(format!("${dollars:.2}")),
    })
}
/// The usage limit of `/billing?format=credits`: the share of the current
/// period's allowance used, and when the period ends. None for a reply
/// without the share.
pub(crate) fn usage(body: &Value, now: i64) -> Option<UsageSnapshot> {
    let config = body.get("config").unwrap_or(body);
    let used = config.get("creditUsagePercent").and_then(number)?;
    let period = config.get("currentPeriod");
    let start = period
        .and_then(|p| p.get("start"))
        .and_then(time)
        .or_else(|| config.get("billingPeriodStart").and_then(time));
    let end = period
        .and_then(|p| p.get("end"))
        .and_then(time)
        .or_else(|| config.get("billingPeriodEnd").and_then(time));
    let monthly = period
        .and_then(|p| p.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.ends_with("MONTHLY"));
    let mut usage = snapshot(used, start, end, monthly, now);
    usage.credits = prepaid_credits(config);
    Some(usage)
}
/// A usage limit's snapshot: a period of about a week is the card's weekly
/// window, a longer one its monthly window.
fn snapshot(
    used: f64,
    start: Option<i64>,
    end: Option<i64>,
    monthly: bool,
    now: i64,
) -> UsageSnapshot {
    let seconds = match (start, end) {
        (Some(start), Some(end)) if end > start => end - start,
        _ if monthly => 30 * ONE_DAY,
        _ => ONE_WEEK,
    };
    let window = UsageWindow {
        clock_offset_seconds: None,
        used_percent: used.clamp(0.0, 100.0),
        window_seconds: seconds,
        reset_at: end,
    };
    let monthly = seconds > 8 * ONE_DAY;
    UsageSnapshot {
        remaining_reset_count: None,
        resets_last_confirmed_at: None,
        reset_expiries: Vec::new(),
        reset_grants: Vec::new(),
        fetched_at: now,
        plan_type: None,
        five_hour: None,
        one_week: (!monthly).then(|| window.clone()),
        one_month: monthly.then_some(window),
        credits: None,
        third_party_five_hour: None,
        third_party_one_week: None,
        fable_one_week: None,
    }
}
fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}
/// A Sub2api Grok account's usage (`/admin/accounts/{id}/usage`), from the
/// xAI billing Sub2api keeps for it: its `seven_day` progress since Sub2api
/// 0.1.173; before, from 0.1.155, the billing itself (`grok_billing`), which
/// both carry. Before 0.1.155 there is no billing, only the rate limits of
/// xAI's response headers, and no window; the card says so beside its
/// weekly row. An account Sub2api cannot use says why.
pub(crate) fn sub2api_usage(value: &Value, now: i64) -> Result<UsageSnapshot, String> {
    if value.get("is_forbidden").and_then(Value::as_bool) == Some(true) {
        return Err(text(&value["forbidden_reason"])
            .unwrap_or_else(|| "xAI refuses this account in Sub2api".into()));
    }
    if value.get("needs_reauth").and_then(Value::as_bool) == Some(true) {
        return Err("The account must sign in to Sub2api again".into());
    }
    let billing = &value["grok_billing"];
    let start = billing.get("period_start").and_then(time);
    let end = billing.get("period_end").and_then(time);
    let monthly = billing["period_type"].as_str() == Some("monthly");
    let progress = value
        .get("seven_day")
        .filter(|w| w.is_object() && w["utilization_unknown"].as_bool() != Some(true))
        .and_then(|w| {
            let used = w.get("utilization").and_then(number)?;
            Some((used, w.get("resets_at").and_then(time).or(end)))
        });
    let window = progress.or_else(|| {
        billing
            .get("usage_percent")
            .and_then(number)
            .map(|used| (used, end))
    });
    let plan = crate::sub2api::reported_plan(crate::Pool::Grok, value);
    let mut usage = match window {
        Some((used, reset)) => snapshot(used, start, reset, monthly, now),
        None => {
            // Not yet probed, or a Sub2api without billing: no window.
            let code = value["error_code"].as_str().unwrap_or_default();
            if !code.is_empty() && code != "quota_unknown" {
                return Err(text(&value["error"]).unwrap_or_else(|| code.to_owned()));
            }
            let mut usage = snapshot(0.0, None, None, false, now);
            usage.one_week = None;
            usage
        }
    };
    usage.plan_type = plan;
    Ok(usage)
}
/// The plan `/settings` names, as Grok Build shows it (`Free`, `SuperGrok`).
pub(crate) fn plan(body: &Value) -> Option<String> {
    let settings = body.get("settings").unwrap_or(body);
    ["subscription_tier_display", "subscription_tier"]
        .iter()
        .find_map(|key| settings.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
        .map(str::to_owned)
}

/// Whether `id` is a selectable Grok text model (not image, video, or router).
pub(crate) fn is_grok_text_model(id: &str) -> bool {
    let lower = id.trim().to_ascii_lowercase();
    lower.starts_with("grok-")
        && !["imagine", "image", "video", "auto"]
            .iter()
            .any(|skip| lower.contains(skip))
}

fn selectable(entry: &Value) -> bool {
    let info = entry.get("info").unwrap_or(entry);
    info["hidden"].as_bool() != Some(true) && info["user_selectable"].as_bool() != Some(false)
}

/// Selectable Grok text models from xAI's `/models` response or Grok Build's
/// `models_cache.json` (`data` array, `models` array, or `models` object map).
pub(crate) fn models(body: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |id: &str| {
        let id = id.trim();
        if is_grok_text_model(id) && !out.iter().any(|m: &String| m.eq_ignore_ascii_case(id)) {
            out.push(id.to_owned());
        }
    };
    for key in ["data", "models"] {
        if let Some(items) = body.get(key).and_then(Value::as_array) {
            for item in items {
                if !selectable(item) {
                    continue;
                }
                if let Some(id) = item["id"].as_str().or_else(|| item["info"]["id"].as_str()) {
                    push(id);
                }
            }
        }
    }
    if let Some(map) = body.get("models").and_then(Value::as_object) {
        for (id, entry) in map {
            if selectable(entry) {
                push(id);
            }
        }
    }
    out
}
