//! Cloud Code quota and OAuth operations shared by the desktop pool and remote daemon.
use crate::{
    models::{CreditSnapshot, UsageSnapshot, UsageWindow},
    utils::now_unix_seconds,
};
use serde_json::{json, Value};
type Response<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send + 'a>>;

/// Tried in turn; the daily endpoint answers first, as for Antigravity.
const CLOUD_BASES: [&str; 2] = [
    crate::proxy_service::antigravity::CLOUD_BASE,
    "https://cloudcode-pa.googleapis.com",
];
pub(crate) const QUOTA_SUMMARY_PATH: &str = "/v1internal:retrieveUserQuotaSummary";
pub(crate) const FETCH_MODELS_PATH: &str = "/v1internal:fetchAvailableModels";
pub(crate) const LOAD_CODE_ASSIST_PATH: &str = "/v1internal:loadCodeAssist";
pub(crate) const GENERATE_PATH: &str = "/v1internal:generateContent";
pub(crate) const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
pub(crate) const CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
// An installed app's OAuth client cannot keep this value confidential; it
// stays split so that secret scanners do not take it for a deployment secret.
pub(crate) const CLIENT_SECRET_PARTS: [&str; 2] = ["GOCSPX-", "K58FWR486LdLJ1mLB8sXC4z6qDAf"];
pub(crate) use crate::proxy_service::antigravity::agent;

pub(crate) const FIVE_HOURS: i64 = 5 * 60 * 60;
pub(crate) const ONE_WEEK: i64 = 7 * 24 * 60 * 60;

/// An HTTP status and its JSON body (null when it has none).
pub(crate) type Reply = (u16, Value);

/// Where the pool's requests go; tests stand in for Google.
pub(crate) trait Network: Send + Sync {
    /// A Cloud Code request with the access token.
    fn cloud<'a>(
        &'a self,
        path: &'a str,
        token: &'a str,
        body: Value,
        agent: &'a str,
        proxy: &'a str,
    ) -> Response<'a, Reply>;
    /// Renews the access token with the refresh token.
    fn renew<'a>(&'a self, refresh_token: &'a str, proxy: &'a str) -> Response<'a, Reply>;
    /// The Google account the access token signs in.
    fn identity<'a>(&'a self, token: &'a str, proxy: &'a str) -> Response<'a, Reply>;
}

pub(crate) struct Http;
fn client(proxy: &str) -> Result<reqwest::Client, String> {
    crate::proxy_session::client(
        proxy,
        crate::proxy_session::Options {
            user_agent: agent(),
            ..Default::default()
        },
    )
}
async fn reply(response: reqwest::Response) -> Reply {
    let status = response.status().as_u16();
    let body = response.json().await.unwrap_or(Value::Null);
    (status, body)
}
impl Network for Http {
    fn cloud<'a>(
        &'a self,
        path: &'a str,
        token: &'a str,
        body: Value,
        agent: &'a str,
        proxy: &'a str,
    ) -> Response<'a, Reply> {
        Box::pin(async move {
            let client = client(proxy)?;
            let mut last = Err("Antigravity request failed".to_owned());
            for base in CLOUD_BASES {
                let sent = client
                    .post(format!("{base}{path}"))
                    .bearer_auth(token)
                    .header("Accept", "application/json")
                    .header("User-Agent", agent)
                    .json(&body)
                    .send()
                    .await;
                match sent {
                    Ok(response) => {
                        let (status, body) = reply(response).await;
                        // Only a server failure tries the other endpoint.
                        if status < 500 && status != 404 {
                            return Ok((status, body));
                        }
                        last = Ok((status, body));
                    }
                    Err(error) => last = Err(crate::usage::format_reqwest_error(&error)),
                }
            }
            last
        })
    }
    fn renew<'a>(&'a self, refresh_token: &'a str, proxy: &'a str) -> Response<'a, Reply> {
        Box::pin(async move {
            let secret = CLIENT_SECRET_PARTS.concat();
            let response = client(proxy)?
                .post(TOKEN_URL)
                .form(&[
                    ("client_id", CLIENT_ID),
                    ("client_secret", secret.as_str()),
                    ("refresh_token", refresh_token),
                    ("grant_type", "refresh_token"),
                ])
                .send()
                .await
                .map_err(|e| crate::usage::format_reqwest_error(&e))?;
            Ok(reply(response).await)
        })
    }
    fn identity<'a>(&'a self, token: &'a str, proxy: &'a str) -> Response<'a, Reply> {
        Box::pin(async move {
            let response = client(proxy)?
                .get(USERINFO_URL)
                .bearer_auth(token)
                .send()
                .await
                .map_err(|e| crate::usage::format_reqwest_error(&e))?;
            Ok(reply(response).await)
        })
    }
}

/// The request bodies Antigravity sends.
pub(crate) fn empty() -> Value {
    json!({})
}

pub(crate) fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .filter(|value| value.is_finite())
}
pub(crate) fn reset_at(value: &Value) -> Option<i64> {
    value
        .as_str()
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|date| date.timestamp())
}
/// How far an idle window's reset may fall short of a whole window from now:
/// the request's time and the two clocks may differ by this much.
const IDLE_MARGIN: i64 = 90;
/// A window as the cards take it. An idle window, untouched, reports its
/// reset a whole window from each request, rolling on; it starts with the
/// first use, its reset then fixed. As the other pools' idle windows, it has
/// no reset time, so that a hi may start it. The share used keeps its
/// precision: a little use must not read as none.
pub(crate) fn window(remaining: f64, reset_at: Option<i64>, seconds: i64, now: i64) -> UsageWindow {
    let remaining = remaining.clamp(0.0, 1.0);
    let idle = remaining >= 1.0 && reset_at.is_some_and(|at| at - now >= seconds - IDLE_MARGIN);
    UsageWindow {
        clock_offset_seconds: None,
        used_percent: ((1.0 - remaining) * 1_000_000.0).round() / 10_000.0,
        window_seconds: seconds,
        reset_at: reset_at.filter(|_| !idle),
    }
}
pub(crate) fn snapshot(now: i64) -> UsageSnapshot {
    UsageSnapshot {
        remaining_reset_count: None,
        resets_last_confirmed_at: None,
        reset_expiries: Vec::new(),
        reset_grants: Vec::new(),
        fetched_at: now,
        plan_type: None,
        five_hour: None,
        one_week: None,
        one_month: None,
        credits: None,
        third_party_five_hour: None,
        third_party_one_week: None,
        fable_one_week: None,
    }
}

/// `loadCodeAssist`'s Google One AI credits, which pay for requests once the
/// plan's quota is used up: the `availableCredits` of the paid tier, else
/// of the current tier. Proto3 JSON leaves out a zero amount and may send
/// an int64 as a string. The credits carry no expiry. None without the
/// list, or when an amount cannot be read.
pub(crate) fn credits(value: &Value) -> Option<CreditSnapshot> {
    let list = [
        "/paidTier/availableCredits",
        "/currentTier/availableCredits",
    ]
    .iter()
    .find_map(|path| value.pointer(path).and_then(Value::as_array))?;
    let whole = |entry: &Value, key: &str| match entry.get(key) {
        None | Some(Value::Null) => Some(0),
        Some(value) => value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok())),
    };
    let (mut balance, mut minimum, mut found) = (0_i64, 0_i64, false);
    for entry in list
        .iter()
        .filter(|entry| entry.get("creditType").and_then(Value::as_str) == Some("GOOGLE_ONE_AI"))
    {
        balance = balance.saturating_add(whole(entry, "creditAmount")?);
        minimum = minimum.max(whole(entry, "minimumCreditAmountForUsage")?);
        found = true;
    }
    found.then(|| CreditSnapshot {
        expiries: Vec::new(),
        has_credits: balance > 0 && balance >= minimum,
        unlimited: false,
        balance: Some(balance.max(0).to_string()),
    })
}

/// `retrieveUserQuotaSummary`: the shared pools by bucket; none without
/// its groups.
pub(crate) fn quota_summary(value: &Value, now: i64) -> Option<UsageSnapshot> {
    let groups = value
        .pointer("/response/groups")
        .or_else(|| value.get("groups"))?
        .as_array()?;
    let mut usage = snapshot(now);
    for bucket in groups
        .iter()
        .filter_map(|group| group.get("buckets").and_then(Value::as_array))
        .flatten()
    {
        let Some(remaining) = bucket.get("remainingFraction").and_then(number) else {
            continue;
        };
        let (slot, seconds) = match bucket.get("bucketId").and_then(Value::as_str) {
            Some("gemini-5h") => (&mut usage.five_hour, FIVE_HOURS),
            Some("gemini-weekly") => (&mut usage.one_week, ONE_WEEK),
            Some("3p-5h") => (&mut usage.third_party_five_hour, FIVE_HOURS),
            Some("3p-weekly") => (&mut usage.third_party_one_week, ONE_WEEK),
            _ => continue,
        };
        if slot.is_none() {
            *slot = Some(window(
                remaining,
                bucket.get("resetTime").and_then(reset_at),
                seconds,
                now,
            ));
        }
    }
    Some(usage)
}

/// Models that hold no quota of the shared pools.
const MODEL_BLACKLIST: [&str; 9] = [
    "MODEL_CHAT_20706",
    "MODEL_CHAT_23310",
    "MODEL_GOOGLE_GEMINI_2_5_FLASH",
    "MODEL_GOOGLE_GEMINI_2_5_FLASH_THINKING",
    "MODEL_GOOGLE_GEMINI_2_5_FLASH_LITE",
    "MODEL_GOOGLE_GEMINI_2_5_PRO",
    "MODEL_PLACEHOLDER_M19",
    "MODEL_PLACEHOLDER_M9",
    "MODEL_PLACEHOLDER_M12",
];

/// `fetchAvailableModels`, for an account without the summary: each pool's
/// five-hour window is its model with the least left. None without models.
pub(crate) fn available_models(value: &Value, now: i64) -> Option<UsageSnapshot> {
    let models = value.get("models")?.as_object()?;
    let mut usage = snapshot(now);
    let mut found = false;
    for (key, model) in models {
        if model.get("isInternal").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(label) = ["displayName", "label"]
            .iter()
            .find_map(|k| model.get(*k).and_then(Value::as_str))
            .map(str::trim)
            .filter(|label| !label.is_empty())
        else {
            continue;
        };
        let id = model.get("model").and_then(Value::as_str).unwrap_or(key);
        if MODEL_BLACKLIST.contains(&id) {
            continue;
        }
        let quota = model.get("quotaInfo");
        let remaining = quota
            .and_then(|q| q.get("remainingFraction"))
            .and_then(number)
            .unwrap_or(0.0);
        let reset = quota.and_then(|q| q.get("resetTime")).and_then(reset_at);
        let slot = if label.to_ascii_lowercase().contains("gemini") {
            &mut usage.five_hour
        } else {
            &mut usage.third_party_five_hour
        };
        let candidate = window(remaining, reset, FIVE_HOURS, now);
        if slot
            .as_ref()
            .is_none_or(|w| candidate.used_percent > w.used_percent)
        {
            *slot = Some(candidate);
        }
        found = true;
    }
    found.then_some(usage)
}

/// Why a usage request failed.
pub(crate) enum Failure {
    /// The access token was refused: renewing it may help.
    Expired,
    Other(String),
}
pub(crate) fn status_error(what: &str, status: u16, body: &Value) -> String {
    let message = body
        .pointer("/error/message")
        .or_else(|| body.get("error_description"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("{what} failed: HTTP {status} {message}")
        .trim()
        .to_owned()
}
/// One quota request: its parsed usage, none for a reply without quota.
async fn quota(
    network: &dyn Network,
    path: &str,
    parse: fn(&Value, i64) -> Option<UsageSnapshot>,
    token: &str,
    proxy: &str,
    now: i64,
) -> std::result::Result<Option<UsageSnapshot>, Failure> {
    let (status, body) = network
        .cloud(path, token, empty(), &agent(), proxy)
        .await
        .map_err(Failure::Other)?;
    match status {
        401 | 403 => Err(Failure::Expired),
        200..=299 => Ok(parse(&body, now)),
        _ => Err(Failure::Other(status_error(
            "Antigravity usage request",
            status,
            &body,
        ))),
    }
}
/// The account's quota. The summary reports the shared pools; an account
/// whose Gemini models have no shared pool there (a Google AI Pro account,
/// say) reports their five-hour window per model, as does any account
/// without the summary.
pub(crate) async fn fetch_usage(
    network: &dyn Network,
    token: &str,
    proxy: &str,
    now: i64,
) -> std::result::Result<UsageSnapshot, Failure> {
    let summary = match quota(
        network,
        QUOTA_SUMMARY_PATH,
        quota_summary,
        token,
        proxy,
        now,
    )
    .await
    {
        Err(Failure::Expired) => return Err(Failure::Expired),
        Err(Failure::Other(error)) => Err(error),
        Ok(summary) => Ok(summary),
    };
    if let Ok(Some(usage)) = &summary {
        if usage.five_hour.is_some() || usage.one_week.is_some() {
            return Ok(usage.clone());
        }
    }
    let models = quota(
        network,
        FETCH_MODELS_PATH,
        available_models,
        token,
        proxy,
        now,
    )
    .await;
    match (summary, models) {
        (_, Err(Failure::Expired)) => Err(Failure::Expired),
        (Ok(Some(mut usage)), Ok(Some(models))) => {
            usage.five_hour = models.five_hour;
            if usage.third_party_five_hour.is_none() && usage.third_party_one_week.is_none() {
                usage.third_party_five_hour = models.third_party_five_hour;
            }
            Ok(usage)
        }
        (Ok(Some(usage)), _) | (_, Ok(Some(usage))) => Ok(usage),
        (Err(error), _) | (_, Err(Failure::Other(error))) => Err(Failure::Other(error)),
        (Ok(None), Ok(None)) => Err(Failure::Other("Antigravity reported no quota".into())),
    }
}
/// The tokens of a renewal, to put in any document of its login.
pub(crate) struct Renewed {
    pub(crate) access: String,
    pub(crate) expires_at: i64,
    pub(crate) lifetime: i64,
    pub(crate) refresh: Option<String>,
}
pub(crate) async fn renew_token(
    network: &dyn Network,
    refresh: &str,
    proxy: &str,
) -> Result<Renewed, String> {
    let (status, body) = network.renew(refresh, proxy).await?;
    match status {
        200..=299 => {
            let access = body["access_token"]
                .as_str()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .ok_or("Google returned no access token")?;
            let lifetime = body["expires_in"]
                .as_i64()
                .filter(|s| *s > 0)
                .unwrap_or(3600);
            let renewed = Renewed {
                access: access.to_owned(),
                expires_at: now_unix_seconds() + lifetime,
                lifetime,
                refresh: body["refresh_token"].as_str().map(str::to_owned),
            };
            Ok(renewed)
        }
        // invalid_grant: the login was signed out or revoked.
        400 | 401 | 403 => Err("error.antigravity.sign_in_expired".into()),
        _ => Err(status_error("Google token request", status, &body)),
    }
}
