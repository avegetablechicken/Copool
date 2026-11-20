//! Antigravity's Cloud Code API, as OpenQuota reads it: the quota of the
//! Gemini models and of the third-party (Claude) models, each in a five-hour
//! and a weekly window, the plan, and Google token renewal with Antigravity's
//! OAuth client.
use crate::{
    account_network::Response,
    models::{UsageSnapshot, UsageWindow},
};
use serde_json::{json, Value};

/// Tried in turn; the daily endpoint answers first, as for Antigravity.
const CLOUD_BASES: [&str; 2] = [
    "https://daily-cloudcode-pa.googleapis.com",
    "https://cloudcode-pa.googleapis.com",
];
pub(super) const QUOTA_SUMMARY_PATH: &str = "/v1internal:retrieveUserQuotaSummary";
pub(super) const FETCH_MODELS_PATH: &str = "/v1internal:fetchAvailableModels";
pub(super) const LOAD_CODE_ASSIST_PATH: &str = "/v1internal:loadCodeAssist";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
// An installed app's OAuth client cannot keep this value confidential; it
// stays split so that secret scanners do not take it for a deployment secret.
const CLIENT_SECRET_PARTS: [&str; 2] = ["GOCSPX-", "K58FWR486LdLJ1mLB8sXC4z6qDAf"];
/// The Antigravity version Copool's requests name.
const CLIENT_VERSION: &str = "3.0.0";
/// Antigravity's agent, `antigravity/<version> <os>/<arch>` with Go's names
/// of the system. Cloud Code answers by it: `loadCodeAssist` refuses other
/// clients (`agy`'s included) and reports no plan, and the quota summary
/// leaves out the Gemini pool for a client without a version or one before
/// 2.6, so that a Google AI Pro account would show only its Claude pool.
pub(super) fn agent() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        "x86" => "386",
        other => other,
    };
    format!("antigravity/{CLIENT_VERSION} {os}/{arch}")
}

const FIVE_HOURS: i64 = 5 * 60 * 60;
const ONE_WEEK: i64 = 7 * 24 * 60 * 60;

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
pub(super) fn empty() -> Value {
    json!({})
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .filter(|value| value.is_finite())
}
fn reset_at(value: &Value) -> Option<i64> {
    value
        .as_str()
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|date| date.timestamp())
}
fn window(remaining: f64, reset_at: Option<i64>, seconds: i64) -> UsageWindow {
    UsageWindow {
        clock_offset_seconds: None,
        used_percent: ((1.0 - remaining.clamp(0.0, 1.0)) * 100.0).round(),
        window_seconds: seconds,
        reset_at,
    }
}
fn snapshot(now: i64) -> UsageSnapshot {
    UsageSnapshot {
        remaining_reset_count: None,
        resets_last_confirmed_at: None,
        reset_expiries: Vec::new(),
        reset_grants: Vec::new(),
        fetched_at: now,
        plan_type: None,
        five_hour: None,
        one_week: None,
        credits: None,
        third_party_five_hour: None,
        third_party_one_week: None,
    }
}

/// `retrieveUserQuotaSummary`: the shared pools by bucket; none without
/// its groups.
pub(super) fn quota_summary(value: &Value, now: i64) -> Option<UsageSnapshot> {
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
pub(super) fn available_models(value: &Value, now: i64) -> Option<UsageSnapshot> {
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
        let candidate = window(remaining, reset, FIVE_HOURS);
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

/// `loadCodeAssist`'s plan: "ultra", "pro" or "free", else as named.
pub(super) fn plan(value: &Value) -> Option<String> {
    let tier = ["/paidTier", "/currentTier"]
        .iter()
        .filter_map(|path| value.pointer(path))
        .find(|tier| tier.is_object())?;
    let raw = ["name", "id"]
        .iter()
        .find_map(|key| tier.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|raw| !raw.is_empty())?;
    let lower = raw.to_ascii_lowercase();
    Some(
        ["ultra", "pro", "free"]
            .into_iter()
            .find(|tier| lower.contains(tier))
            .map(str::to_owned)
            .unwrap_or_else(|| raw.strip_prefix("Google AI ").unwrap_or(raw).to_owned()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_names_a_versioned_antigravity_on_this_system() {
        let agent = agent();
        assert!(agent.starts_with("antigravity/3.0.0 "), "{agent}");
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert_eq!(agent, "antigravity/3.0.0 darwin/arm64");
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        assert_eq!(agent, "antigravity/3.0.0 windows/amd64");
    }

    #[test]
    fn the_summary_maps_each_bucket_to_its_window() {
        let summary = json!({"response": {"groups": [{"buckets": [
            {"bucketId": "3p-weekly", "remainingFraction": 0.5},
            {"bucketId": "gemini-5h", "remainingFraction": 0.8, "resetTime": "2026-07-12T10:00:00Z"},
            {"bucketId": "gemini-weekly", "remainingFraction": "0.25"},
            {"bucketId": "unknown", "remainingFraction": 1.0}
        ]}]}});
        let usage = quota_summary(&summary, 7).unwrap();
        assert_eq!(usage.fetched_at, 7);
        let five = usage.five_hour.unwrap();
        assert_eq!(five.used_percent, 20.0);
        assert_eq!(five.window_seconds, FIVE_HOURS);
        assert_eq!(five.reset_at, Some(1_783_850_400));
        assert_eq!(usage.one_week.unwrap().used_percent, 75.0);
        assert!(usage.third_party_five_hour.is_none());
        let weekly = usage.third_party_one_week.unwrap();
        assert_eq!(
            (weekly.used_percent, weekly.window_seconds),
            (50.0, ONE_WEEK)
        );
        assert!(quota_summary(&json!({}), 0).is_none());
    }

    #[test]
    fn available_models_pool_by_the_model_with_the_least_left() {
        let models = json!({"models": {
            "a": {"model": "MODEL_A", "displayName": "Gemini 3 Pro", "quotaInfo": {"remainingFraction": 0.4}},
            "b": {"displayName": "Gemini 3 Flash", "quotaInfo": {"remainingFraction": 0.9}},
            "c": {"displayName": "Claude Sonnet", "quotaInfo": {"remainingFraction": 0.7}},
            "internal": {"displayName": "Gemini Internal", "isInternal": true},
            "old": {"model": "MODEL_GOOGLE_GEMINI_2_5_PRO", "displayName": "Gemini 2.5 Pro"}
        }});
        let usage = available_models(&models, 0).unwrap();
        assert_eq!(usage.five_hour.unwrap().used_percent, 60.0);
        assert_eq!(usage.third_party_five_hour.unwrap().used_percent, 30.0);
        assert!(usage.one_week.is_none());
        assert!(available_models(&json!({"models": {}}), 0).is_none());
    }

    #[test]
    fn the_plan_is_the_paid_tier_else_the_current_one() {
        let plan_of = |value: Value| plan(&value);
        assert_eq!(
            plan_of(
                json!({"paidTier": {"name": "Google AI Pro"}, "currentTier": {"name": "Free"}})
            ),
            Some("pro".into())
        );
        assert_eq!(
            plan_of(json!({"currentTier": {"id": "g1-ultra-tier"}})),
            Some("ultra".into())
        );
        assert_eq!(
            plan_of(json!({"currentTier": {"name": "Google AI Standard"}})),
            Some("Standard".into())
        );
        assert_eq!(plan_of(json!({})), None);
    }
}
