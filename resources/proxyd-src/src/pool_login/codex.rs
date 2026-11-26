//! The Codex pool's Login: `auth.json` as Codex writes it, its `tokens`
//! (or a flat document of them) and the claims of their JWTs.
use base64::engine::general_purpose::URL_SAFE;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::Map;
use serde_json::Value;

use crate::models::ExtractedAuth;
use crate::utils::localized;

pub(crate) struct CodexOAuthTokens {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) account_id: Option<String>,
    pub(crate) expires_at_ms: Option<i64>,
    pub(crate) email: Option<String>,
    pub(crate) plan_type: Option<String>,
}

pub(crate) fn normalize_imported_auth_json(mut auth_json: Value) -> Value {
    let Some(root) = auth_json.as_object_mut() else {
        return auth_json;
    };
    if root.get("tokens").and_then(Value::as_object).is_some() {
        return auth_json;
    }
    if root.get("access_token").and_then(Value::as_str).is_none() {
        return auth_json;
    }
    let mut tokens = Map::new();
    for key in ["access_token", "refresh_token", "id_token", "account_id"] {
        if let Some(value) = root.remove(key) {
            tokens.insert(key.into(), value);
        }
    }
    root.insert("tokens".into(), tokens.into());
    root.entry("auth_mode").or_insert("chatgpt".into());
    auth_json
}

pub(crate) fn normalized_auth_for_write(document: Value) -> Result<Value, String> {
    let mut document = normalize_imported_auth_json(document);
    let extracted = extract_auth(&document)?;
    let root = document.as_object_mut().ok_or("Invalid auth document")?;
    root.entry("auth_mode").or_insert("chatgpt".into());
    if let Some(tokens) = root.get_mut("tokens").and_then(Value::as_object_mut) {
        tokens.insert("account_id".into(), extracted.account_id.into());
    }
    let timestamp = root
        .get("last_refresh")
        .and_then(Value::as_str)
        .and_then(|value| {
            chrono::DateTime::parse_from_rfc3339(value)
                .ok()
                .or_else(|| chrono::DateTime::parse_from_rfc3339(&format!("{value}Z")).ok())
        })
        .map(|date| date.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    root.insert("last_refresh".into(), timestamp.into());
    Ok(document)
}

fn workspace_name(claims: &Value) -> Option<String> {
    let auth = claims.get("https://api.openai.com/auth")?;
    let clean = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    if let Some(name) = clean(auth.get("chatgpt_team_name")) {
        return Some(name);
    }
    let organizations = auth.get("organizations").and_then(Value::as_array)?;
    let active = auth.get("active_organization_id").and_then(Value::as_str);
    let organization = organizations
        .iter()
        .find(|o| active.is_some() && o.get("id").and_then(Value::as_str) == active)
        .or_else(|| {
            organizations
                .iter()
                .find(|o| o.get("is_active").and_then(Value::as_bool) == Some(true))
        })?;
    clean(organization.get("slug"))
        .filter(|s| s.to_lowercase() != "personal")
        .or_else(|| clean(organization.get("title")).filter(|s| s.to_lowercase() != "personal"))
}

/// Parses the current auth.json for the account identity and the tokens the usage API needs.
///
/// `auth_mode` is missing in some versions, so the `tokens` field decides availability first.
pub(crate) fn extract_auth(auth_json: &Value) -> Result<ExtractedAuth, String> {
    let mode = auth_json
        .get("auth_mode")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();

    let tokens = auth_token_object(auth_json);
    let tokens = match tokens {
        Some(value) => value,
        None => {
            if !mode.is_empty() && mode != "chatgpt" && mode != "chatgpt_auth_tokens" {
                return Err(localized("error.auth.not_chatgpt_mode", &[]));
            }
            return Err(localized("error.auth.no_chatgpt_token", &[]));
        }
    };

    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| localized("error.auth.missing_access_token", &[]))?
        .to_string();

    let id_token = tokens
        .get("id_token")
        .and_then(Value::as_str)
        .ok_or_else(|| localized("error.auth.missing_id_token", &[]))?;

    let mut account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let mut email = None;
    let mut plan_type = None;
    let mut principal_id = None;
    let mut team_name = None;

    if let Ok(claims) = decode_jwt_payload(id_token) {
        principal_id = claims.get("sub").and_then(Value::as_str).map(str::to_owned);
        team_name = workspace_name(&claims);
        email = claims
            .get("email")
            .and_then(Value::as_str)
            .map(ToString::to_string);

        let auth_claim = claims.get("https://api.openai.com/auth");
        if account_id.is_none() {
            account_id = auth_claim
                .and_then(|value| value.get("chatgpt_account_id"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
        }
        plan_type = auth_claim
            .and_then(|value| value.get("chatgpt_plan_type"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }

    let account_id =
        account_id.ok_or_else(|| localized("error.auth.missing_chatgpt_account_id", &[]))?;

    if email.is_none() {
        email = auth_json
            .get("email")
            .and_then(Value::as_str)
            .map(str::to_owned);
    }
    Ok(ExtractedAuth {
        principal_id,
        team_name,
        account_id,
        access_token,
        email,
        plan_type,
    })
}

/// Stable identity: a workspace can contain multiple independently logged-in users.
pub(crate) fn account_identity(document: &Value) -> Result<(String, String), String> {
    let extracted = extract_auth(document)?;
    let principal = auth_token_object(document)
        .and_then(|v| v.get("id_token"))
        .and_then(Value::as_str)
        .and_then(|t| decode_jwt_payload(t).ok())
        .and_then(|v| v.get("sub").and_then(Value::as_str).map(str::to_owned))
        .or(extracted.email)
        .unwrap_or_default();
    Ok((extracted.account_id, principal))
}

/// Whether `live` is a later login of the same account than `stored`: other
/// tokens, refreshed later. A login without a refresh time is older.
pub(crate) fn newer_login(live: &Value, stored: &Value) -> bool {
    let refreshed = |document: &Value| {
        document["last_refresh"]
            .as_str()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t.trim()).ok())
    };
    account_identity(live).is_ok()
        && account_identity(live).ok() == account_identity(stored).ok()
        && live["tokens"] != stored["tokens"]
        && match (refreshed(live), refreshed(stored)) {
            (Some(live), Some(stored)) => live > stored,
            (live, stored) => live.is_some() || stored.is_none(),
        }
}

/// Whether a login renewed from `from` may replace `current`, wherever it is
/// kept: `current` still holds that generation, or an older one. A renewal
/// answered late must not replace a login renewed since.
pub(crate) fn renewal_replaces(renewed: &Value, from: &Value, current: &Value) -> bool {
    current["tokens"] == from["tokens"] || newer_login(renewed, current)
}

fn non_empty_trimmed_str(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn jwt_nested_str(claims: Option<&Value>, path: &[&str]) -> Option<String> {
    let mut current = claims?;
    for key in path {
        current = current.get(*key)?;
    }
    non_empty_trimmed_str(current.as_str())
}

fn jwt_exp_seconds(claims: &Value) -> Option<i64> {
    claims
        .get("exp")
        .and_then(Value::as_f64)
        .filter(|exp| exp.is_finite())
        .map(|exp| exp as i64)
}

pub(crate) fn jwt_chatgpt_account_id(claims: &Value) -> Option<String> {
    jwt_nested_str(
        Some(claims),
        &["https://api.openai.com/auth", "chatgpt_account_id"],
    )
}

pub(crate) fn jwt_openai_profile_email(claims: &Value) -> Option<String> {
    jwt_nested_str(Some(claims), &["https://api.openai.com/profile", "email"])
}

pub(crate) fn jwt_plain_email(claims: &Value) -> Option<String> {
    jwt_nested_str(Some(claims), &["email"])
}

pub(crate) fn jwt_openai_email(claims: &Value) -> Option<String> {
    jwt_openai_profile_email(claims).or_else(|| jwt_plain_email(claims))
}

pub(crate) fn jwt_chatgpt_plan_type(claims: &Value) -> Option<String> {
    jwt_nested_str(
        Some(claims),
        &["https://api.openai.com/auth", "chatgpt_plan_type"],
    )
}

/// Extracts the reusable OpenAI OAuth tokens when syncing a login to third-party clients.
pub(crate) fn extract_codex_oauth_tokens(auth_json: &Value) -> Result<CodexOAuthTokens, String> {
    let tokens = auth_token_object(auth_json)
        .or_else(|| {
            let root = auth_json.as_object()?;
            root.contains_key("access_token").then_some(root)
        })
        .ok_or_else(|| localized("error.auth.auth_json_invalid_structure", &[]))?;

    let access_token = non_empty_trimmed_str(tokens.get("access_token").and_then(Value::as_str))
        .ok_or_else(|| localized("error.auth.missing_access_token", &[]))?;
    let refresh_token = non_empty_trimmed_str(tokens.get("refresh_token").and_then(Value::as_str))
        .ok_or_else(|| localized("error.opencode.missing_refresh_token", &[]))?;
    let access_claims = decode_jwt_payload(&access_token).ok();
    let id_claims = tokens
        .get("id_token")
        .and_then(Value::as_str)
        .and_then(|id_token| decode_jwt_payload(id_token).ok());

    let account_id = non_empty_trimmed_str(tokens.get("account_id").and_then(Value::as_str))
        .or_else(|| access_claims.as_ref().and_then(jwt_chatgpt_account_id))
        .or_else(|| id_claims.as_ref().and_then(jwt_chatgpt_account_id));
    let email = access_claims
        .as_ref()
        .and_then(jwt_openai_profile_email)
        .or_else(|| id_claims.as_ref().and_then(jwt_openai_profile_email))
        .or_else(|| access_claims.as_ref().and_then(jwt_plain_email))
        .or_else(|| id_claims.as_ref().and_then(jwt_plain_email))
        .or_else(|| non_empty_trimmed_str(auth_json.get("email").and_then(Value::as_str)))
        .map(|value| value.to_ascii_lowercase());
    let plan_type = access_claims
        .as_ref()
        .and_then(jwt_chatgpt_plan_type)
        .or_else(|| id_claims.as_ref().and_then(jwt_chatgpt_plan_type))
        .map(|value| value.to_ascii_lowercase());

    let expires_at_ms = access_claims
        .as_ref()
        .and_then(jwt_exp_seconds)
        .or_else(|| id_claims.as_ref().and_then(jwt_exp_seconds))
        .map(|value| value * 1000);

    Ok(CodexOAuthTokens {
        access_token,
        refresh_token,
        account_id,
        expires_at_ms,
        email,
        plan_type,
    })
}

pub(crate) fn auth_token_object(auth_json: &Value) -> Option<&Map<String, Value>> {
    auth_json
        .get("tokens")
        .and_then(Value::as_object)
        .or_else(|| {
            let root = auth_json.as_object()?;
            if root.contains_key("access_token") && root.contains_key("id_token") {
                Some(root)
            } else {
                None
            }
        })
}

/// When the stored access token expires (Unix seconds), from its JWT claims;
/// a JWT NumericDate may be fractional.
pub(crate) fn access_token_expiry(auth_json: &Value) -> Option<i64> {
    auth_json
        .pointer("/tokens/access_token")
        .or_else(|| auth_json.get("access_token"))
        .and_then(Value::as_str)
        .and_then(|t| decode_jwt_payload(t).ok())
        .and_then(|v| jwt_exp_seconds(&v))
}

pub(crate) fn decode_jwt_payload(token: &str) -> Result<Value, String> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| localized("error.auth.id_token_invalid_format", &[]))?;

    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| {
            let remainder = payload.len() % 4;
            let padded = if remainder == 0 {
                payload.to_string()
            } else {
                format!("{payload}{}", "=".repeat(4 - remainder))
            };
            URL_SAFE.decode(padded)
        })
        .map_err(|_| localized("error.auth.decode_id_token_failed", &[]))?;

    serde_json::from_slice(&decoded)
        .map_err(|_| localized("error.auth.decode_id_token_failed", &[]))
}
