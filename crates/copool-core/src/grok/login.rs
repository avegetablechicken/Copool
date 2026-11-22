//! The login Grok Build uses: `~/.grok/auth.json` (`$GROK_HOME/auth.json`),
//! an object of sign-ins keyed `<issuer>::<client id>`, each holding its
//! access token (`key`), `refresh_token`, `expires_at` (RFC 3339) and the
//! account's `user_id`, `email` and names. Grok Build picks up changes to
//! the file without a restart. A switch puts another account's sign-in
//! under its key, leaving the sign-ins of other issuers alone; a renewal of
//! the sign-in it holds updates its tokens in place.
use serde_json::{Map, Value};

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
/// When the access token expires (Unix seconds).
pub(crate) fn expiry(document: &Value) -> Option<i64> {
    let value = document.get("expires_at")?;
    if let Some(text) = value.as_str() {
        return chrono::DateTime::parse_from_rfc3339(text.trim())
            .ok()
            .map(|date| date.timestamp());
    }
    value.as_i64()
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
/// A browser sign-in's tokens as Grok Build keeps a sign-in.
pub(crate) fn signed_in(tokens: &Value, now: i64) -> Result<Value, String> {
    let access = tokens["access_token"]
        .as_str()
        .filter(|t| !t.trim().is_empty())
        .ok_or("xAI returned no access token")?;
    let refresh = tokens["refresh_token"]
        .as_str()
        .filter(|t| !t.trim().is_empty())
        .ok_or(super::CREDENTIALS_INVALID)?;
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
    document.insert("expires_at".into(), timestamp(now + lifetime).into());
    document.insert("oidc_issuer".into(), ISSUER.into());
    document.insert("oidc_client_id".into(), CLIENT_ID.into());
    Ok(Value::Object(document))
}
