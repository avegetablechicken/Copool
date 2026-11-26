//! The Codex pool's Login: `auth.json` as Codex writes it, its `tokens`
//! (or a flat document of them) and the claims of their JWTs.
use base64::engine::general_purpose::URL_SAFE;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::Map;
use serde_json::Value;

use super::{credential_expiry, jwt_exp_ms, non_empty, seconds, Identity, OAuthCredential};
use crate::models::ExtractedAuth;
use crate::utils::localized;

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

/// What a Codex Login's tokens say, read once for Copool's own identity of
/// the account and for the Credential it grants alike.
struct Claims {
    id: Option<Value>,
    access: Option<Value>,
}

impl Claims {
    fn of(tokens: &Map<String, Value>) -> Self {
        let decode = |key: &str| {
            tokens
                .get(key)
                .and_then(Value::as_str)
                .and_then(|token| decode_jwt_payload(token.trim()).ok())
        };
        Self {
            id: decode("id_token"),
            access: decode("access_token"),
        }
    }

    /// `tokens.account_id`, else the ID token's account, else the access
    /// token's.
    fn account_id(&self, tokens: &Map<String, Value>) -> Option<String> {
        non_empty(tokens.get("account_id").and_then(Value::as_str))
            .or_else(|| self.id.as_ref().and_then(jwt_chatgpt_account_id))
            .or_else(|| self.access.as_ref().and_then(jwt_chatgpt_account_id))
    }

    /// The email Copool's card shows: the ID token's, else its profile's,
    /// else the access token's profile's, else the access token's, else the
    /// document's own.
    fn email(&self, document: &Value) -> Option<String> {
        let id = self.id.as_ref();
        let access = self.access.as_ref();
        id.and_then(jwt_plain_email)
            .or_else(|| id.and_then(jwt_openai_profile_email))
            .or_else(|| access.and_then(jwt_openai_profile_email))
            .or_else(|| access.and_then(jwt_plain_email))
            .or_else(|| non_empty(document.get("email").and_then(Value::as_str)))
    }

    fn plan_type(&self) -> Option<String> {
        self.id
            .as_ref()
            .and_then(jwt_chatgpt_plan_type)
            .or_else(|| self.access.as_ref().and_then(jwt_chatgpt_plan_type))
    }
}

/// Parses the current auth.json for the account identity and the tokens the usage API needs.
///
/// `auth_mode` is missing in some versions, so the `tokens` field decides availability first.
/// Copool names an account by its ID token, so a login without one is refused.
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

    if tokens.get("id_token").and_then(Value::as_str).is_none() {
        return Err(localized("error.auth.missing_id_token", &[]));
    }
    let claims = Claims::of(tokens);
    let principal_id = claims
        .id
        .as_ref()
        .and_then(|claims| claims.get("sub"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let team_name = claims.id.as_ref().and_then(workspace_name);
    let account_id = claims
        .account_id(tokens)
        .ok_or_else(|| localized("error.auth.missing_chatgpt_account_id", &[]))?;
    Ok(ExtractedAuth {
        principal_id,
        team_name,
        account_id,
        access_token,
        email: claims.email(auth_json),
        plan_type: claims.plan_type(),
    })
}

/// The Credential a Codex Login grants, read as Copool reads the account's
/// identity; a flat document of tokens without an ID token is enough. The
/// account is also the organization, and the plan its name, as
/// third-party agents key a ChatGPT workspace. Nothing outside the Login
/// is needed.
#[allow(dead_code)] // given to third-party agents by the desktop core only
pub(crate) fn credential(login: &Value, _identity: &Identity) -> Result<OAuthCredential, String> {
    let tokens = login
        .get("tokens")
        .and_then(Value::as_object)
        .or_else(|| {
            let root = login.as_object()?;
            root.contains_key("access_token").then_some(root)
        })
        .ok_or_else(|| localized("error.auth.auth_json_invalid_structure", &[]))?;
    let access = non_empty(tokens.get("access_token").and_then(Value::as_str))
        .ok_or_else(|| localized("error.auth.missing_access_token", &[]))?;
    let refresh = non_empty(tokens.get("refresh_token").and_then(Value::as_str))
        .ok_or_else(|| localized("error.opencode.missing_refresh_token", &[]))?;
    let claims = Claims::of(tokens);
    let account_id = claims.account_id(tokens);
    let expires_ms = claims
        .access
        .as_ref()
        .and_then(jwt_exp_ms)
        .or_else(|| claims.id.as_ref().and_then(jwt_exp_ms));
    Ok(OAuthCredential {
        access,
        refresh,
        expires_ms: credential_expiry(expires_ms),
        org_id: account_id.clone(),
        account_id,
        email: claims.email(login),
        org_name: claims.plan_type().map(|plan| plan.to_ascii_lowercase()),
        scope: None,
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

fn jwt_nested_str(claims: Option<&Value>, path: &[&str]) -> Option<String> {
    let mut current = claims?;
    for key in path {
        current = current.get(*key)?;
    }
    non_empty(current.as_str())
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
        .and_then(|v| jwt_exp_ms(&v))
        .map(seconds)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn jwt(claims: Value) -> String {
        format!("h.{}.s", URL_SAFE_NO_PAD.encode(claims.to_string()))
    }

    /// The Credential expected of a login whose tokens are `a` and `r`.
    fn credential_of(
        account_id: Option<&str>,
        email: Option<&str>,
        plan: Option<&str>,
        expires_ms: Option<i64>,
    ) -> OAuthCredential {
        OAuthCredential {
            access: "a".into(),
            refresh: "r".into(),
            // A login naming no expiry gets the default: compared apart.
            expires_ms: expires_ms.unwrap_or_default(),
            account_id: account_id.map(Into::into),
            org_id: account_id.map(Into::into),
            email: email.map(Into::into),
            org_name: plan.map(Into::into),
            scope: None,
        }
    }

    #[test]
    fn login_and_identity_give_the_credential() {
        let auth = |id: Value, access: Value, top: Value| {
            json!({"tokens": {
                "access_token": jwt(access), "refresh_token": "r",
                "id_token": jwt(id),
            }, "email": top})
        };
        let profile = |email: &str| json!({"email": email});
        let account = |id: &str| json!({"chatgpt_account_id": id, "chatgpt_plan_type": "Team"});
        let cases = [
            (
                "a normal login: the ID token's email and account, the plan lower-cased",
                {
                    let mut login = auth(
                        json!({"email": "Dev@Example.com", "https://api.openai.com/auth": account("acct-id")}),
                        json!({"exp": 1_780_000_000, "https://api.openai.com/profile": profile("access-profile@example.com"), "https://api.openai.com/auth": account("acct-access")}),
                        json!("top@example.com"),
                    );
                    login["tokens"]["account_id"] = "acct-tokens".into();
                    login
                },
                credential_of(
                    Some("acct-tokens"),
                    Some("Dev@Example.com"),
                    Some("team"),
                    Some(1_780_000_000_000),
                ),
            ),
            (
                "the ID token's email over its profile's, and over the access token's",
                auth(
                    json!({"email": "id-top@example.com", "https://api.openai.com/profile": profile("Id-Profile@Example.com")}),
                    json!({"email": "access-top@example.com", "https://api.openai.com/profile": profile("access-profile@example.com"), "https://api.openai.com/auth": account("acct-access")}),
                    json!("top@example.com"),
                ),
                credential_of(
                    Some("acct-access"),
                    Some("id-top@example.com"),
                    Some("team"),
                    None,
                ),
            ),
            (
                "the ID token's profile email, kept as spelled",
                auth(
                    json!({"https://api.openai.com/profile": profile("Id-Profile@Example.com"), "https://api.openai.com/auth": account("acct-id")}),
                    json!({"email": "access-top@example.com", "https://api.openai.com/profile": profile("access-profile@example.com"), "https://api.openai.com/auth": account("acct-access")}),
                    json!("top@example.com"),
                ),
                credential_of(
                    Some("acct-id"),
                    Some("Id-Profile@Example.com"),
                    Some("team"),
                    None,
                ),
            ),
            (
                "the access token's profile email over its own",
                auth(
                    json!({}),
                    json!({"email": "access-top@example.com", "https://api.openai.com/profile": profile("access-profile@example.com")}),
                    json!("top@example.com"),
                ),
                credential_of(None, Some("access-profile@example.com"), None, None),
            ),
            (
                "the access token's email over the document's",
                auth(
                    json!({}),
                    json!({"email": "access-top@example.com"}),
                    json!("top@example.com"),
                ),
                credential_of(None, Some("access-top@example.com"), None, None),
            ),
            (
                "the document's email last",
                auth(json!({}), json!({}), json!(" Top@Example.com ")),
                credential_of(None, Some("Top@Example.com"), None, None),
            ),
            (
                "a fractional access expiry, not the ID token's",
                auth(
                    json!({"exp": 1_700_000_000}),
                    json!({"exp": 1_780_000_123.75}),
                    json!(null),
                ),
                credential_of(None, None, None, Some(1_780_000_123_750)),
            ),
            (
                "the ID token's expiry when the access token names none",
                auth(json!({"exp": 1_700_000_000}), json!({}), json!(null)),
                credential_of(None, None, None, Some(1_700_000_000_000)),
            ),
        ];
        for (name, login, mut expected) in cases {
            expected.access = login["tokens"]["access_token"].as_str().unwrap().into();
            let mut granted = credential(&login, &Identity::default()).expect(name);
            if expected.expires_ms == 0 {
                assert!(granted.expires_ms > 1_700_000_000_000, "{name}");
                granted.expires_ms = 0;
            }
            assert_eq!(granted, expected, "{name}");
            // Copool's own identity of the account reads it the same way.
            if let Ok(extracted) = extract_auth(&login) {
                assert_eq!(Some(extracted.account_id), expected.account_id, "{name}");
                assert_eq!(extracted.email, expected.email, "{name}");
            }
        }

        // A flat document of tokens without an ID token grants a Credential
        // of no one in particular, but Copool cannot name its account.
        let flat = json!({"access_token": "a", "refresh_token": "r"});
        let mut granted = credential(&flat, &Identity::default()).unwrap();
        assert!(granted.expires_ms > 1_700_000_000_000);
        granted.expires_ms = 0;
        assert_eq!(granted, credential_of(None, None, None, None));
        assert_eq!(
            extract_auth(&flat).unwrap_err(),
            localized("error.auth.no_chatgpt_token", &[])
        );
        let access_only = json!({"tokens": {"access_token": jwt(json!({"https://api.openai.com/auth": account("acct-access")})), "refresh_token": "r"}});
        assert_eq!(
            credential(&access_only, &Identity::default())
                .unwrap()
                .account_id
                .as_deref(),
            Some("acct-access")
        );
        assert_eq!(
            extract_auth(&access_only).unwrap_err(),
            localized("error.auth.missing_id_token", &[])
        );

        for (login, error) in [
            (json!([]), "error.auth.auth_json_invalid_structure"),
            (
                json!({"tokens": {"refresh_token": "r"}}),
                "error.auth.missing_access_token",
            ),
            (
                json!({"access_token": " ", "refresh_token": "r"}),
                "error.auth.missing_access_token",
            ),
            (
                json!({"tokens": {"access_token": "a"}}),
                "error.opencode.missing_refresh_token",
            ),
        ] {
            assert_eq!(
                credential(&login, &Identity::default()).unwrap_err(),
                localized(error, &[]),
                "{login}"
            );
        }
    }

    #[test]
    fn the_access_token_expiry_is_read_in_seconds() {
        let login = json!({"tokens": {"access_token": jwt(json!({"exp": 1_780_000_123.75}))}});
        assert_eq!(access_token_expiry(&login), Some(1_780_000_123));
    }
}
