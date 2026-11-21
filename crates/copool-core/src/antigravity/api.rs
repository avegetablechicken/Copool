//! Antigravity's Cloud Code API, as OpenQuota reads it: the quota of the
//! Gemini models and of the third-party (Claude) models, each in a five-hour
//! and a weekly window, the plan, and Google token renewal with Antigravity's
//! OAuth client.
#[cfg(test)]
pub(crate) use crate::antigravity_cloud::QUOTA_SUMMARY_PATH;
use crate::models::UsageSnapshot;
use serde_json::{json, Value};

const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// What Antigravity's sign-in asks for.
const SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform \
https://www.googleapis.com/auth/userinfo.email \
https://www.googleapis.com/auth/userinfo.profile \
https://www.googleapis.com/auth/cclog \
https://www.googleapis.com/auth/experimentsandconfigs openid";
pub(crate) use crate::antigravity_cloud::{
    agent, empty, Http, Network, Reply, FETCH_MODELS_PATH, GENERATE_PATH, LOAD_CODE_ASSIST_PATH,
};
#[cfg(test)]
use crate::antigravity_cloud::{available_models, quota_summary};
use crate::antigravity_cloud::{
    number, reset_at, snapshot, window, CLIENT_ID, CLIENT_SECRET_PARTS, FIVE_HOURS, ONE_WEEK,
    TOKEN_URL,
};

/// Where the browser signs in to Google with Antigravity's client; the
/// consent screen is asked for so that Google hands out a refresh token.
pub(crate) fn sign_in_url(redirect: &str, challenge: &str, state: &str) -> String {
    let mut url = reqwest::Url::parse(AUTHORIZE_URL).expect("valid URL");
    url.query_pairs_mut().extend_pairs([
        ("client_id", CLIENT_ID),
        ("redirect_uri", redirect),
        ("response_type", "code"),
        ("scope", SCOPES),
        ("access_type", "offline"),
        ("prompt", "consent"),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ]);
    url.into()
}
/// Exchanges a sign-in's code for its tokens, through the system proxy as
/// the other sign-ins.
pub(crate) async fn exchange(code: &str, verifier: &str, redirect: &str) -> Result<Value, String> {
    let secret = CLIENT_SECRET_PARTS.concat();
    let response = crate::utils::with_account_proxy(reqwest::Client::builder(), "")?
        .user_agent(agent())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?
        .post(TOKEN_URL)
        .form(&[
            ("client_id", CLIENT_ID),
            ("client_secret", secret.as_str()),
            ("code", code),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect),
        ])
        .send()
        .await
        .map_err(|e| crate::usage::format_reqwest_error(&e))?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let reason = body["error_description"]
            .as_str()
            .or_else(|| body["error"].as_str())
            .unwrap_or_default();
        return Err(format!("OAuth token exchange failed: {status} {reason}")
            .trim()
            .to_owned());
    }
    Ok(body)
}

/// A Sub2api Antigravity account's usage (`/admin/accounts/{id}/usage`): the
/// same pools as `retrieveUserQuotaSummary`, which Sub2api reads, as groups
/// of windows with the share used (0-100); else, for an account without
/// the summary, each model's, pooled as `available_models` does. An account
/// Sub2api cannot use says why.
pub(crate) fn sub2api_usage(value: &Value, now: i64) -> Result<UsageSnapshot, String> {
    if value.get("is_forbidden").and_then(Value::as_bool) == Some(true) {
        return Err(value
            .get("forbidden_reason")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .unwrap_or("Google refuses this account in Sub2api")
            .to_owned());
    }
    if value.get("needs_reauth").and_then(Value::as_bool) == Some(true) {
        return Err("The account must sign in to Sub2api again".into());
    }
    let mut usage = snapshot(now);
    usage.plan_type = value
        .get("subscription_tier")
        .and_then(Value::as_str)
        .map(|tier| tier.trim().to_ascii_lowercase())
        .filter(|tier| !tier.is_empty() && tier != "unknown");
    let left = |entry: &Value| {
        entry
            .get("utilization")
            .and_then(number)
            .map(|used| 1.0 - used / 100.0)
    };
    let mut found = false;
    for group in value
        .get("antigravity_quota_groups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let third_party = match group.get("key").and_then(Value::as_str) {
            Some("gemini") => false,
            Some("3p") => true,
            _ => continue,
        };
        for entry in group
            .get("windows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(remaining) = left(entry) else {
                continue;
            };
            let (slot, seconds) = match (entry.get("window").and_then(Value::as_str), third_party) {
                (Some("5h"), false) => (&mut usage.five_hour, FIVE_HOURS),
                (Some("weekly"), false) => (&mut usage.one_week, ONE_WEEK),
                (Some("5h"), true) => (&mut usage.third_party_five_hour, FIVE_HOURS),
                (Some("weekly"), true) => (&mut usage.third_party_one_week, ONE_WEEK),
                _ => continue,
            };
            if slot.is_none() {
                *slot = Some(window(
                    remaining,
                    entry.get("resets_at").and_then(reset_at),
                    seconds,
                    now,
                ));
                found = true;
            }
        }
    }
    if !found {
        for (model, quota) in value
            .get("antigravity_quota")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let Some(remaining) = left(quota) else {
                continue;
            };
            let slot = if model.to_ascii_lowercase().contains("gemini") {
                &mut usage.five_hour
            } else {
                &mut usage.third_party_five_hour
            };
            let candidate = window(
                remaining,
                quota.get("reset_time").and_then(reset_at),
                FIVE_HOURS,
                now,
            );
            if slot
                .as_ref()
                .is_none_or(|w| candidate.used_percent > w.used_percent)
            {
                *slot = Some(candidate);
            }
            found = true;
        }
    }
    if found {
        Ok(usage)
    } else {
        Err("Sub2api reports no Antigravity quota for this account".into())
    }
}
/// The Cloud Code project of the account's messages, from `loadCodeAssist`.
pub(super) fn project(value: &Value) -> Option<String> {
    value
        .get("cloudaicompanionProject")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
}

/// The models a hi starts each five-hour window with, from
/// `fetchAvailableModels`: the Gemini model Antigravity runs commands with,
/// else another Gemini model; for the third-party pool, which its models
/// share, GPT-OSS, the cheapest, else Claude Sonnet. Only models that hold
/// quota (a reset) count.
pub(super) fn hello_models(value: &Value) -> (Option<String>, Option<String>) {
    let Some(models) = value.get("models").and_then(Value::as_object) else {
        return (None, None);
    };
    let quota: Vec<&str> = models
        .iter()
        .filter(|(_, m)| m.pointer("/quotaInfo/resetTime").is_some())
        .filter(|(_, m)| m.get("isInternal").and_then(Value::as_bool) != Some(true))
        .map(|(key, _)| key.as_str())
        .collect();
    let gemini = value
        .get("commandModelIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|id| quota.contains(id))
        .or_else(|| quota.iter().copied().find(|id| id.starts_with("gemini")));
    let third_party = ["gpt-oss", "claude-sonnet", "claude"]
        .iter()
        .find_map(|prefix| quota.iter().copied().find(|id| id.starts_with(prefix)));
    (gemini.map(str::to_owned), third_party.map(str::to_owned))
}
/// One short hi with a model, as Antigravity asks Cloud Code.
pub(super) fn hello(project: &str, model: &str) -> Value {
    crate::proxy_service::antigravity::envelope(
        project,
        model,
        &json!({
            "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
            "generationConfig": {"maxOutputTokens": 16},
        }),
    )
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
    fn a_hi_goes_to_a_quota_model_of_each_pool() {
        // As fetchAvailableModels lists a Google AI Pro account's models.
        let models = json!({
            "models": {
                "tab_flash_lite_preview": {"model": "MODEL_PLACEHOLDER_M19", "quotaInfo": {"remainingFraction": 1}},
                "gemini-3.1-flash-lite": {"displayName": "Gemini 3.1 Flash Lite", "quotaInfo": {"remainingFraction": 0.98, "resetTime": "2026-11-20T21:18:53Z"}},
                "claude-sonnet-5-5-medium": {"displayName": "Claude Sonnet 5.5 (Medium)", "quotaInfo": {"remainingFraction": 1, "resetTime": "2026-11-20T22:59:41Z"}},
                "gemini-3-flash": {"displayName": "Gemini 3 Flash", "quotaInfo": {"remainingFraction": 0.98, "resetTime": "2026-11-20T21:18:53Z"}},
                "gpt-oss-120b-medium": {"displayName": "GPT-OSS 120B (Medium)", "quotaInfo": {"remainingFraction": 1, "resetTime": "2026-11-20T22:59:41Z"}}
            },
            "commandModelIds": ["gemini-3-flash"]
        });
        assert_eq!(
            hello_models(&models),
            (
                Some("gemini-3-flash".into()),
                Some("gpt-oss-120b-medium".into())
            )
        );
        let mut fewer = models.clone();
        let list = fewer["models"].as_object_mut().unwrap();
        list.remove("gpt-oss-120b-medium");
        list.remove("gemini-3-flash");
        assert_eq!(
            hello_models(&fewer),
            (
                Some("gemini-3.1-flash-lite".into()),
                Some("claude-sonnet-5-5-medium".into())
            )
        );
        assert_eq!(hello_models(&json!({})), (None, None));
        let body = hello("project-1", "gemini-3-flash");
        assert_eq!(body["project"], "project-1");
        assert_eq!(body["request"]["contents"][0]["parts"][0]["text"], "hi");
        assert_eq!(
            project(&json!({"cloudaicompanionProject": "p"})).as_deref(),
            Some("p")
        );
    }

    #[test]
    fn idle_windows_roll_their_reset_and_show_none() {
        let now = 1_795_197_000;
        let at = |offset: i64| {
            chrono::DateTime::from_timestamp(now + offset, 0)
                .unwrap()
                .to_rfc3339()
        };
        let summary = json!({"groups": [{"buckets": [
            // Untouched: a whole window from now, give or take the clocks.
            {"bucketId": "3p-5h", "remainingFraction": 1, "resetTime": at(FIVE_HOURS - 30)},
            {"bucketId": "3p-weekly", "remainingFraction": 1, "resetTime": at(ONE_WEEK + 20)},
            // Started by a hi that used nothing yet: its reset stays put.
            {"bucketId": "gemini-5h", "remainingFraction": 1, "resetTime": at(FIVE_HOURS - 180)},
            // Used a little: started, and not rounded down to nothing.
            {"bucketId": "gemini-weekly", "remainingFraction": 0.999991, "resetTime": at(ONE_WEEK)}
        ]}]});
        let usage = quota_summary(&summary, now).unwrap();
        let idle = usage.third_party_five_hour.unwrap();
        assert_eq!((idle.used_percent, idle.reset_at), (0.0, None));
        assert_eq!(usage.third_party_one_week.unwrap().reset_at, None);
        assert_eq!(
            usage.five_hour.unwrap().reset_at,
            Some(now + FIVE_HOURS - 180)
        );
        let week = usage.one_week.unwrap();
        assert_eq!(week.used_percent, 0.0009);
        assert_eq!(week.reset_at, Some(now + ONE_WEEK));
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
