//! The login Grok Build uses: `~/.grok/auth.json` (`$GROK_HOME/auth.json`),
//! an object of sign-ins keyed `<issuer>::<client id>`, each holding its
//! access token (`key`), `refresh_token`, `expires_at` (RFC 3339) and the
//! account's `user_id`, `email` and names. Grok Build picks up changes to
//! the file without a restart. A switch puts another account's sign-in
//! under its key, leaving the sign-ins of other issuers alone; a renewal of
//! the sign-in it holds updates its tokens in place.
#![allow(dead_code)]
use serde_json::{Map, Value};

pub(crate) const CREDENTIALS_INVALID: &str = "error.grok.credentials_invalid";
pub(crate) const ISSUER: &str = "https://auth.x.ai";
pub(crate) const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";

fn text(document: &Value, key: &str) -> Option<String> {
    document
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
pub(crate) fn access_token(document: &Value) -> Option<String> {
    text(document, "key")
}
pub(crate) fn refresh_token(document: &Value) -> Option<String> {
    text(document, "refresh_token")
}
/// When the access token expires (Unix milliseconds), read as every
/// pool's expiry is.
pub(crate) fn expiry_ms(document: &Value) -> Option<i64> {
    document.get("expires_at").and_then(super::timestamp_ms)
}
/// When the access token expires (Unix seconds).
pub(crate) fn expiry(document: &Value) -> Option<i64> {
    expiry_ms(document).map(super::seconds)
}
pub(crate) fn email(document: &Value) -> Option<String> {
    text(document, "email").filter(|email| email.contains('@'))
}
pub(crate) fn user_id(document: &Value) -> Option<String> {
    text(document, "user_id")
}
/// The sign-in's key in `auth.json`.
pub(crate) fn entry_key(document: &Value) -> String {
    format!(
        "{}::{}",
        text(document, "oidc_issuer").unwrap_or_else(|| ISSUER.into()),
        text(document, "oidc_client_id").unwrap_or_else(|| CLIENT_ID.into())
    )
}
/// Whether two sign-ins are of the same account: the same user in the same
/// team, or, for one that names no user, the same refresh token. Grok
/// rotates refresh tokens, so the token alone does not follow a renewal.
pub(crate) fn same_login(a: &Value, b: &Value) -> bool {
    match (user_id(a), user_id(b)) {
        (Some(x), Some(y)) => x == y && text(a, "team_id") == text(b, "team_id"),
        _ => refresh_token(a).is_some() && refresh_token(a) == refresh_token(b),
    }
}
/// A sign-in: an object with an access or refresh token.
fn is_sign_in(value: &Value) -> bool {
    value.is_object() && refresh_token(value).is_some()
}
/// The sign-in an `auth.json` holds, xAI's first; or a sign-in given by
/// itself. None when it has none with a refresh token, which alone keeps
/// an account usable.
pub(crate) fn decode(bytes: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(bytes).ok()?;
    let value: Value = serde_json::from_str(text.trim().trim_start_matches('\u{feff}')).ok()?;
    sign_in_of(&value)
}
pub(crate) fn sign_in_of(value: &Value) -> Option<Value> {
    if is_sign_in(value) {
        return Some(value.clone());
    }
    let object = value.as_object()?;
    let xai = format!("{ISSUER}::");
    object
        .iter()
        .filter(|(_, entry)| is_sign_in(entry))
        .min_by_key(|(key, _)| !key.starts_with(&xai))
        .map(|(_, entry)| entry.clone())
}
/// `auth.json` holding the sign-in in place of any other of its issuer;
/// sign-ins of other issuers stay.
pub(crate) fn encode(existing: Option<&[u8]>, document: &Value) -> Value {
    // As `decode` reads it: a byte order mark first would lose the other
    // sign-ins.
    let mut file = existing
        .map(|bytes| bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes))
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        .and_then(|value| match value {
            Value::Object(object) => Some(object),
            _ => None,
        })
        .unwrap_or_default();
    let key = entry_key(document);
    let issuer = key.split("::").next().unwrap_or_default().to_owned();
    file.retain(|k, _| k.split("::").next() != Some(issuer.as_str()));
    file.insert(key, document.clone());
    Value::Object(file)
}
/// RFC 3339 in UTC with microseconds, as Grok Build writes its times.
pub(crate) fn timestamp(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S%.6fZ")
        .to_string()
}
/// Puts a renewal's tokens in the sign-in.
pub(crate) fn apply_renewal(
    document: &mut Value,
    access_token: &str,
    expires_at: i64,
    refresh_token: Option<&str>,
) {
    let Some(object) = document.as_object_mut() else {
        return;
    };
    object.insert("key".into(), access_token.into());
    object.insert("expires_at".into(), timestamp(expires_at).into());
    if let Some(refresh) = refresh_token.filter(|r| !r.trim().is_empty()) {
        object.insert("refresh_token".into(), refresh.into());
    }
}
/// The claims of a JWT, unverified: it came from xAI's token endpoint.
pub(crate) fn claims(token: &str) -> Map<String, Value> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    token
        .split('.')
        .nth(1)
        .and_then(|payload| URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| match value {
            Value::Object(object) => Some(object),
            _ => None,
        })
        .unwrap_or_default()
}
/// The Credential a Grok Login grants: the sign-in's tokens (an
/// `auth.json`'s, or a sign-in by itself), its expiry, else the access
/// token's, and its user, else the access token's subject and email. Grok
/// has no organization, and needs nothing outside the Login.
pub(crate) fn credential(
    login: &Value,
    _identity: &super::Identity,
) -> Result<super::OAuthCredential, String> {
    let sign_in = sign_in_of(login);
    let document = sign_in.as_ref().unwrap_or(login);
    let access = access_token(document).ok_or("Missing access token")?;
    let refresh = refresh_token(document).ok_or("Missing refresh token")?;
    let access_claims = Value::Object(claims(&access));
    let claim = |key: &str| super::non_empty(access_claims.get(key).and_then(Value::as_str));
    let expires_ms = expiry_ms(document).or_else(|| super::jwt_exp_ms(&access_claims));
    Ok(super::OAuthCredential {
        expires_ms: super::credential_expiry(expires_ms),
        // The xAI subject, as omp's `xai-oauth` keeps it in `accountId`.
        account_id: user_id(document).or_else(|| claim("sub")),
        email: email(document).or_else(|| claim("email").filter(|e| e.contains('@'))),
        access,
        refresh,
        org_id: None,
        org_name: None,
        scope: None,
    })
}
/// A browser sign-in's tokens as Grok Build keeps a sign-in.
pub(crate) fn signed_in(tokens: &Value, now: i64) -> Result<Value, String> {
    let access = tokens["access_token"]
        .as_str()
        .filter(|t| !t.trim().is_empty())
        .ok_or("xAI returned no access token")?;
    let refresh = tokens["refresh_token"]
        .as_str()
        .filter(|t| !t.trim().is_empty())
        .ok_or(CREDENTIALS_INVALID)?;
    let lifetime = tokens["expires_in"]
        .as_i64()
        .filter(|s| *s > 0)
        .unwrap_or(6 * 60 * 60);
    let access_claims = claims(access);
    let identity = tokens["id_token"].as_str().map(claims).unwrap_or_default();
    let claim = |key: &str| {
        identity
            .get(key)
            .or_else(|| access_claims.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let mut document = Map::new();
    document.insert("key".into(), access.into());
    document.insert("auth_mode".into(), "oidc".into());
    document.insert("create_time".into(), timestamp(now).into());
    for (key, name) in [
        ("user_id", "sub"),
        ("email", "email"),
        ("first_name", "given_name"),
        ("last_name", "family_name"),
        ("principal_type", "principal_type"),
        ("principal_id", "principal_id"),
        ("team_id", "team_id"),
    ] {
        if let Some(value) = claim(name) {
            document.insert(key.into(), value.into());
        }
    }
    document.insert("refresh_token".into(), refresh.into());
    document.insert(
        "expires_at".into(),
        timestamp(now.saturating_add(lifetime)).into(),
    );
    document.insert("oidc_issuer".into(), ISSUER.into());
    document.insert("oidc_client_id".into(), CLIENT_ID.into());
    Ok(Value::Object(document))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use serde_json::json;

    fn jwt(claims: Value) -> String {
        format!("h.{}.s", URL_SAFE_NO_PAD.encode(claims.to_string()))
    }

    #[test]
    fn login_and_identity_give_the_credential() {
        use super::super::{Identity, OAuthCredential};
        let expected = |access: &str, expires_ms: i64, user: Option<&str>, email: Option<&str>| {
            OAuthCredential {
                access: access.into(),
                refresh: "rt".into(),
                expires_ms,
                account_id: user.map(Into::into),
                email: email.map(Into::into),
                ..OAuthCredential::default()
            }
        };
        let claimed =
            jwt(json!({"sub": "uid-claim", "email": "Claim@x.example", "exp": 1_800_000_123.5}));
        for (name, login, credential_expected) in [
            (
                "a sign-in: its user, email as spelled, and RFC 3339 expiry",
                json!({"key": claimed, "refresh_token": "rt", "expires_at": "2026-11-26T12:00:00.000000Z", "user_id": "uid-alice", "email": "Alice@x.example"}),
                expected(
                    &claimed,
                    1_795_694_400_000,
                    Some("uid-alice"),
                    Some("Alice@x.example"),
                ),
            ),
            (
                "an auth.json, xAI's sign-in, the access token's fractional expiry and identity",
                json!({
                    "https://other.example::c": {"key": "other", "refresh_token": "rt-other"},
                    "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {"key": claimed, "refresh_token": "rt"},
                }),
                expected(
                    &claimed,
                    1_800_000_123_500,
                    Some("uid-claim"),
                    Some("Claim@x.example"),
                ),
            ),
            (
                "an expiry in seconds",
                json!({"key": "opaque", "refresh_token": "rt", "expires_at": 1_800_000_000}),
                expected("opaque", 1_800_000_000_000, None, None),
            ),
            (
                "an expiry in milliseconds",
                json!({"key": "opaque", "refresh_token": "rt", "expires_at": 1_800_000_000_000_i64}),
                expected("opaque", 1_800_000_000_000, None, None),
            ),
        ] {
            assert_eq!(
                credential(&login, &Identity::default()),
                Ok(credential_expected),
                "{name}"
            );
        }
        let unnamed = json!({"key": "opaque", "refresh_token": "rt"});
        assert!(
            credential(&unnamed, &Identity::default())
                .unwrap()
                .expires_ms
                > 1_700_000_000_000
        );
        for (login, error) in [
            (
                json!({"refresh_token": "only-refresh"}),
                "Missing access token",
            ),
            (json!({"key": "only-access"}), "Missing refresh token"),
        ] {
            assert_eq!(credential(&login, &Identity::default()).unwrap_err(), error);
        }
        assert_eq!(
            expiry(&json!({"expires_at": 1_800_000_000_000_i64})),
            Some(1_800_000_000)
        );
    }
}
