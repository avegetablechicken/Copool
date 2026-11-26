//! The Claude pool's Login: Claude Code's credentials document, whose
//! `claudeAiOauth` entry holds the tokens, their expiry and scopes and the
//! subscription, with the account Claude Code keeps in `oauthAccount`.
#![allow(dead_code)]
use super::{credential_expiry, non_empty, Identity, OAuthCredential};
use crate::claude_api::{number, text, SCOPES};
use serde_json::{json, Value};

type Result<T> = std::result::Result<T, String>;

/// When the access token expires (Unix milliseconds, as Claude Code writes
/// it), read as every pool's expiry is.
pub(crate) fn expires_at_ms(oauth: &Value) -> Option<i64> {
    oauth.get("expiresAt").and_then(super::timestamp_ms)
}

pub(crate) fn expires_at(oauth: &Value) -> i64 {
    expires_at_ms(oauth).unwrap_or(0)
}

/// The space-separated scopes recorded on `oauth["scopes"]`, if any.
pub(crate) fn granted_scope(oauth: &Value) -> Option<String> {
    let scopes: Vec<&str> = oauth["scopes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .collect();
    (!scopes.is_empty()).then(|| scopes.join(" "))
}

/// The scopes a login was granted, as Claude Code asks for them on renewal;
/// the default ones for a login that records none.
pub(crate) fn refresh_scope(oauth: &Value) -> String {
    granted_scope(oauth).unwrap_or_else(|| SCOPES.into())
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

pub(crate) fn identity(account: &Value) -> Option<String> {
    let account_id = text(&account["accountUuid"])?;
    Some(format!(
        "{account_id}|{}",
        text(&account["organizationUuid"]).unwrap_or_default()
    ))
}

/// A card's name: the email, else the display name, else Claude Code for a
/// login not yet identified.
pub(crate) fn label_of(account: &Value) -> String {
    text(&account["emailAddress"])
        .or_else(|| text(&account["displayName"]))
        .unwrap_or_else(|| "Claude Code".into())
}

/// `oauthAccount` and the subscription from the OAuth profile, else from the
/// account and organization of a token response.
pub(crate) fn profile_account(
    profile: Option<&Value>,
    tokens: &Value,
) -> (Value, Option<String>, Option<String>) {
    let mut account = serde_json::Map::new();
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(value) = value.filter(|v| !v.is_null()) {
            account.insert(key.into(), value);
        }
    };
    let (subscription, tier);
    if let Some(p) = profile {
        put("accountUuid", p["account"].get("uuid").cloned());
        put("emailAddress", p["account"].get("email").cloned());
        put("organizationUuid", p["organization"].get("uuid").cloned());
        put("organizationName", p["organization"].get("name").cloned());
        put("displayName", p["account"].get("display_name").cloned());
        put(
            "billingType",
            p["organization"].get("billing_type").cloned(),
        );
        put(
            "hasExtraUsageEnabled",
            p["organization"].get("has_extra_usage_enabled").cloned(),
        );
        subscription = text(&p["organization"]["organization_type"])
            .map(|t| t.strip_prefix("claude_").unwrap_or(&t).to_owned());
        tier = text(&p["organization"]["rate_limit_tier"]);
    } else {
        put("accountUuid", tokens["account"].get("uuid").cloned());
        put(
            "emailAddress",
            tokens["account"].get("email_address").cloned(),
        );
        put(
            "organizationUuid",
            tokens["organization"].get("uuid").cloned(),
        );
        put(
            "organizationName",
            tokens["organization"].get("name").cloned(),
        );
        subscription = None;
        tier = None;
    }
    (Value::Object(account), subscription, tier)
}

/// Both tokens are the same.
pub(crate) fn same_login(a: &Value, b: &Value) -> bool {
    ["accessToken", "refreshToken"]
        .iter()
        .all(|key| text(&a[key]) == text(&b[key]))
}

pub(crate) fn same_tokens(a: &Value, b: &Value) -> bool {
    ["accessToken", "refreshToken"]
        .iter()
        .any(|key| text(&a[key]).is_some_and(|token| text(&b[key]) == Some(token)))
}

// Compare only the credential generation, not unrelated profile/MCP metadata.
pub(crate) fn same_oauth_generation(a: &Value, b: &Value) -> bool {
    ["accessToken", "refreshToken", "expiresAt"]
        .iter()
        .all(|key| a[*key] == b[*key])
}

/// Whether `a` is a later generation of the same login than `b`.
pub(crate) fn newer_login(a: &Value, b: &Value) -> bool {
    text(&a["accessToken"]).is_some()
        && text(&a["refreshToken"]) != text(&b["refreshToken"])
        && expires_at(a) > expires_at(b)
}

/// The credentials document, as JSON or the hexadecimal Keychain encoding.
pub(crate) fn parse_credentials(bytes: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    serde_json::from_str::<Value>(text)
        .ok()
        .or_else(|| {
            if text.len() % 2 != 0 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let decoded = (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
                .collect::<Option<Vec<_>>>()?;
            serde_json::from_slice(&decoded).ok()
        })
        .filter(Value::is_object)
}

/// The `claudeAiOauth` entry of an exported credentials document, or the
/// document itself when it is that entry, if it is a well-formed login.
pub(crate) fn exported_oauth(document: &Value) -> Option<&Value> {
    let oauth = document.get("claudeAiOauth").unwrap_or(document);
    let invalid = !oauth.is_object()
        || text(&oauth["accessToken"]).is_none()
        || oauth.get("refreshToken").is_some_and(|v| text(v).is_none())
        || oauth.get("expiresAt").is_some_and(|v| !v.is_number())
        || oauth.get("scopes").is_some_and(|v| {
            v.as_array()
                .is_none_or(|items| items.iter().any(|v| !v.is_string()))
        });
    (!invalid).then_some(oauth)
}

/// The Credential a Claude Login grants: `claudeAiOauth`'s tokens, expiry
/// and scopes (the document's own when it is that entry, a `scope` string
/// for one without a `scopes` list), and the account and organization
/// Claude Code keeps beside it in `oauthAccount`, given as `identity`
/// ([`account_identity`]).
pub(crate) fn credential(login: &Value, identity: &Identity) -> Result<OAuthCredential> {
    let oauth = login
        .get("claudeAiOauth")
        .filter(|v| v.is_object())
        .unwrap_or(login);
    let text = |key: &str| non_empty(oauth.get(key).and_then(Value::as_str));
    let access = text("accessToken").ok_or("Missing access token")?;
    let refresh = text("refreshToken").ok_or("Missing refresh token")?;
    Ok(OAuthCredential {
        access,
        refresh,
        expires_ms: credential_expiry(expires_at_ms(oauth)),
        account_id: identity.account_id.clone(),
        org_id: identity.org_id.clone(),
        email: identity.email.clone(),
        org_name: identity.org_name.clone(),
        scope: granted_scope(oauth).or_else(|| text("scope")),
    })
}

/// Whose a login is, as Claude Code's `oauthAccount` beside it says.
pub(crate) fn account_identity(account: &Value) -> Identity {
    let field = |key: &str| non_empty(account.get(key).and_then(Value::as_str));
    Identity {
        account_id: field("accountUuid"),
        org_id: field("organizationUuid"),
        email: field("emailAddress"),
        org_name: field("organizationName"),
    }
}

/// `oauth` over the members a live login keeps beside it.
pub(crate) fn merged_oauth(live: Option<&Value>, oauth: &Value) -> Value {
    let mut merged = live
        .filter(|live| live.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let (Some(target), Some(source)) = (merged.as_object_mut(), oauth.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    merged
}

/// Applies a token response to a `claudeAiOauth` entry.
pub(crate) fn apply_tokens(oauth: &mut Value, tokens: &Value, now_ms: i64) -> Result<()> {
    let access = text(&tokens["access_token"]).ok_or("Claude returned no access token")?;
    oauth["accessToken"] = access.into();
    if let Some(refresh) = text(&tokens["refresh_token"]) {
        oauth["refreshToken"] = refresh.into();
    }
    // Saturating: a lifetime too long for the clock is as good as forever.
    let at = |seconds: f64| now_ms.saturating_add((seconds * 1000.0) as i64);
    if let Some(seconds) = number(&tokens["expires_in"]) {
        oauth["expiresAt"] = at(seconds).into();
    }
    if let Some(scope) = text(&tokens["scope"]) {
        oauth["scopes"] = scope.split_whitespace().collect::<Vec<_>>().into();
    }
    // Claude Code records when the refresh token itself expires.
    if let Some(seconds) = number(&tokens["refresh_token_expires_in"]) {
        oauth["refreshTokenExpiresAt"] = at(seconds).into();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renewal_asks_again_for_the_scopes_the_login_holds() {
        let default = crate::claude_api::SCOPES;
        assert_eq!(
            refresh_scope(&json!({"scopes": ["user:inference", "user:profile"]})),
            "user:inference user:profile"
        );
        assert_eq!(
            refresh_scope(&json!({"scopes": [" user:inference ", "", 7]})),
            "user:inference"
        );
        for oauth in [
            json!({}),
            json!({"scopes": []}),
            json!({"scopes": "user:inference"}),
            json!({"scopes": [""]}),
        ] {
            assert_eq!(refresh_scope(&oauth), default, "{oauth}");
        }
    }

    #[test]
    fn login_and_identity_give_the_credential() {
        let identity = Identity {
            account_id: Some("acct-alice".into()),
            org_id: Some("org-team".into()),
            email: Some("Alice@Example.com".into()),
            org_name: Some("Alice's Organization".into()),
        };
        let expected = |expires_ms: i64, scope: Option<&str>| OAuthCredential {
            access: "sk-ant-oat".into(),
            refresh: "rt".into(),
            expires_ms,
            account_id: Some("acct-alice".into()),
            org_id: Some("org-team".into()),
            // As the account spells it.
            email: Some("Alice@Example.com".into()),
            org_name: Some("Alice's Organization".into()),
            scope: scope.map(Into::into),
        };
        let oauth = |extra: Value| {
            let mut oauth = json!({"accessToken": "sk-ant-oat", "refreshToken": "rt"});
            oauth
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            oauth
        };
        for (name, login, credential_expected) in [
            (
                "a credentials document",
                json!({"claudeAiOauth": oauth(json!({"expiresAt": 1_780_000_000_000_i64, "scopes": ["user:inference", "user:profile"]}))}),
                expected(1_780_000_000_000, Some("user:inference user:profile")),
            ),
            (
                "the OAuth entry by itself, read at its root",
                oauth(json!({"expiresAt": 1_780_000_000_000_i64, "scopes": ["user:inference"]})),
                expected(1_780_000_000_000, Some("user:inference")),
            ),
            (
                "a `scope` string without a `scopes` list",
                oauth(json!({"expiresAt": 1_780_000_000_000_i64, "scope": "user:inference"})),
                expected(1_780_000_000_000, Some("user:inference")),
            ),
            (
                "a fractional expiry in milliseconds",
                oauth(json!({"expiresAt": 1_780_000_000_123.5})),
                expected(1_780_000_000_123, None),
            ),
            (
                "an expiry in seconds",
                oauth(json!({"expiresAt": 1_780_000_000})),
                expected(1_780_000_000_000, None),
            ),
            (
                "an RFC 3339 expiry",
                oauth(json!({"expiresAt": "2026-05-28T20:26:40Z"})),
                expected(1_780_000_000_000, None),
            ),
        ] {
            assert_eq!(
                credential(&login, &identity),
                Ok(credential_expected),
                "{name}"
            );
        }
        // No expiry, or none known: the default.
        for login in [oauth(json!({})), oauth(json!({"expiresAt": 0}))] {
            assert!(credential(&login, &identity).unwrap().expires_ms > 1_700_000_000_000);
        }
        // The identity Claude Code's account gives, its blank members none.
        let account = json!({
            "accountUuid": "acct-alice", "organizationUuid": "org-team",
            "emailAddress": "Alice@Example.com", "organizationName": "Alice's Organization",
            "displayName": "Alice"
        });
        assert_eq!(account_identity(&account), identity);
        assert_eq!(
            account_identity(&json!({"accountUuid": " ", "emailAddress": 7})),
            Identity::default()
        );
        assert_eq!(account_identity(&Value::Null), Identity::default());
        // Without the account, the login is of no one in particular.
        let unnamed = credential(&oauth(json!({"expiresAt": 1})), &Identity::default()).unwrap();
        assert_eq!((unnamed.account_id, unnamed.email), (None, None));
        for (login, error) in [
            (
                json!({"claudeAiOauth": {"refreshToken": "rt"}}),
                "Missing access token",
            ),
            (
                json!({"accessToken": "a", "refreshToken": " "}),
                "Missing refresh token",
            ),
        ] {
            assert_eq!(credential(&login, &identity).unwrap_err(), error);
        }
    }

    #[test]
    fn an_import_takes_a_fractional_expiry() {
        let document =
            json!({"claudeAiOauth": {"accessToken": "a", "expiresAt": 1_780_000_000_123.5}});
        assert_eq!(
            expires_at(exported_oauth(&document).unwrap()),
            1_780_000_000_123
        );
        let text = json!({"claudeAiOauth": {"accessToken": "a", "expiresAt": "soon"}});
        assert!(exported_oauth(&text).is_none());
    }

    #[test]
    fn credentials_accept_the_hexadecimal_keychain_form() {
        let document = json!({"claudeAiOauth": {"accessToken": "t"}});
        let encoded: String = serde_json::to_vec(&document)
            .unwrap()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(parse_credentials(encoded.as_bytes()), Some(document));
    }
}
