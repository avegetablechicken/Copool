//! What each pool's tool signs in with, its Login, read and written as a
//! document: the tokens, expiry and identity it holds, which of two logins
//! is newer, and how a renewal is applied to it, and the Credential it
//! grants. Shared by the app and the remote daemon; nothing here reads or
//! writes where a Login is kept, nor contacts a server.
pub(crate) mod antigravity;
pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod grok;

use crate::models::Pool;
use serde_json::Value;

/// What a Login grants, read the same way for every pool: its tokens, when
/// the access token expires (Unix milliseconds) and whose account it is.
#[allow(dead_code)] // given to third-party agents by the desktop core only
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OAuthCredential {
    pub(crate) access: String,
    pub(crate) refresh: String,
    pub(crate) expires_ms: i64,
    pub(crate) account_id: Option<String>,
    pub(crate) org_id: Option<String>,
    /// As the Login spells it; compare it case-insensitively.
    pub(crate) email: Option<String>,
    pub(crate) org_name: Option<String>,
    pub(crate) scope: Option<String>,
}

/// What Copool knows of an account outside its Login document. It only
/// supplements the document, as each pool's `credential` says.
#[allow(dead_code)] // given to third-party agents by the desktop core only
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) account_id: Option<String>,
    pub(crate) org_id: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) org_name: Option<String>,
}

/// What Copool's proxy sends upstream with a login, read from the document
/// a candidate carries as its `authJson`: the pool's Login for Codex and
/// Claude, [`antigravity::proxy_form`] for Antigravity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProxyLogin {
    pub(crate) access_token: String,
    /// The account the document names; none when only what Copool stored
    /// beside it does.
    pub(crate) account_id: Option<String>,
    pub(crate) plan_type: Option<String>,
}

/// The [`ProxyLogin`] of a `pool`'s candidate document: Codex's as Copool
/// identifies the account ([`codex::extract_auth`]), Claude's from its
/// `claudeAiOauth` entry ([`claude::oauth`]), Antigravity's from its proxy
/// form. Grok has no API proxy.
pub(crate) fn proxy_login(pool: Pool, auth: &Value) -> Result<ProxyLogin, String> {
    let token_only = |access_token: Option<String>| {
        access_token
            .map(|access_token| ProxyLogin {
                access_token,
                ..ProxyLogin::default()
            })
            .ok_or_else(|| "Missing access token".to_owned())
    };
    match pool {
        // An empty access token is none, and one is sent trimmed, as for
        // the other pools.
        Pool::Codex => {
            let auth = codex::extract_auth(auth)?;
            let access_token = non_empty(Some(auth.access_token.as_str()))
                .ok_or_else(|| crate::utils::localized("error.auth.missing_access_token", &[]))?;
            Ok(ProxyLogin {
                access_token,
                account_id: Some(auth.account_id),
                plan_type: auth.plan_type,
            })
        }
        Pool::Claude => token_only(claude::access_token(auth)),
        Pool::Antigravity => token_only(antigravity::proxy_form::access_token(auth)),
        Pool::Grok => Err("Grok accounts have no API proxy".into()),
    }
}

/// A numeric timestamp from this value on is in milliseconds, below it in
/// seconds (1e11 seconds is in the year 5138, 1e11 ms in 1973).
pub(crate) const MILLISECONDS_FROM: i64 = 100_000_000_000;

/// A Login's expiry as Unix milliseconds: RFC 3339, or a number of seconds
/// or, from [`MILLISECONDS_FROM`] on, milliseconds; either may be fractional.
pub(crate) fn timestamp_ms(value: &Value) -> Option<i64> {
    if let Some(text) = value.as_str() {
        return chrono::DateTime::parse_from_rfc3339(text.trim())
            .ok()
            .map(|date| date.timestamp_millis());
    }
    if let Some(number) = value.as_i64() {
        return Some(if number >= MILLISECONDS_FROM {
            number
        } else {
            number.saturating_mul(1000)
        });
    }
    let number = value.as_f64().filter(|n| n.is_finite())?;
    Some(if number >= MILLISECONDS_FROM as f64 {
        number as i64
    } else {
        (number * 1000.0) as i64
    })
}

/// A JWT's `exp` (a NumericDate, in seconds, possibly fractional) as Unix
/// milliseconds.
pub(crate) fn jwt_exp_ms(claims: &Value) -> Option<i64> {
    claims
        .get("exp")
        .and_then(Value::as_f64)
        .filter(|exp| exp.is_finite())
        .map(|exp| (exp * 1000.0) as i64)
}

/// Milliseconds as whole Unix seconds.
pub(crate) fn seconds(ms: i64) -> i64 {
    ms.div_euclid(1000)
}

/// The expiry a Credential carries: the Login's when it names one, else
/// 55 minutes from now, as an access token usually still lasts.
#[allow(dead_code)] // given to third-party agents by the desktop core only
pub(crate) fn credential_expiry(expires_ms: Option<i64>) -> i64 {
    expires_ms
        .filter(|ms| *ms > 0)
        .unwrap_or_else(|| crate::utils::now_unix_seconds().saturating_mul(1000) + 55 * 60 * 1000)
}

/// A trimmed, non-empty string.
pub(crate) fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_proxy_reads_each_pools_login() {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let jwt = |claims: Value| format!("h.{}.s", URL_SAFE_NO_PAD.encode(claims.to_string()));
        let id_token = jwt(json!({"sub": "user-1", "email": "a@b.c",
            "https://api.openai.com/auth": {"chatgpt_account_id": "acct-1", "chatgpt_plan_type": "pro"}}));
        let found = |access: &str, account: Option<&str>, plan: Option<&str>| {
            Ok(ProxyLogin {
                access_token: access.into(),
                account_id: account.map(Into::into),
                plan_type: plan.map(Into::into),
            })
        };
        let missing = || Err("Missing access token".to_owned());
        for (name, pool, auth, expected) in [
            (
                "Codex's auth.json, naming its account and plan",
                Pool::Codex,
                json!({"tokens": {"access_token": "codex-at", "id_token": id_token}}),
                found("codex-at", Some("acct-1"), Some("pro")),
            ),
            (
                "Codex's tokens' own account",
                Pool::Codex,
                json!({"tokens": {"access_token": "codex-at", "id_token": id_token, "account_id": "acct-own"}}),
                found("codex-at", Some("acct-own"), Some("pro")),
            ),
            (
                "Claude's credentials document",
                Pool::Claude,
                json!({"claudeAiOauth": {"accessToken": "claude-at", "refreshToken": "rt", "expiresAt": 1}}),
                found("claude-at", None, None),
            ),
            (
                "Claude's OAuth entry by itself, read at its root",
                Pool::Claude,
                json!({"accessToken": " claude-at ", "refreshToken": "rt", "scope": "user:inference"}),
                found("claude-at", None, None),
            ),
            (
                "a Claude document whose entry is no object, read at its root",
                Pool::Claude,
                json!({"claudeAiOauth": null, "accessToken": "claude-at"}),
                found("claude-at", None, None),
            ),
            (
                "Claude's entry without a token",
                Pool::Claude,
                json!({"claudeAiOauth": {"accessToken": " ", "refreshToken": "rt"}}),
                missing(),
            ),
            (
                "Antigravity's proxy form",
                Pool::Antigravity,
                json!({"accessToken": "agy-at", "project": "p", "expiresAt": 1_783_850_400}),
                found("agy-at", None, None),
            ),
            (
                "an Antigravity Login, which is no proxy form",
                Pool::Antigravity,
                json!({"token": {"access_token": "agy-at", "refresh_token": "r"}}),
                missing(),
            ),
            (
                "Grok, which has no proxy",
                Pool::Grok,
                json!({"access_token": "grok-at"}),
                Err("Grok accounts have no API proxy".into()),
            ),
        ] {
            assert_eq!(proxy_login(pool, &auth), expected, "{name}");
        }
        assert!(proxy_login(Pool::Codex, &json!({"tokens": {"access_token": "at"}})).is_err());
        // Codex's identity holds without a token; its proxy login does not.
        let blank = json!({"tokens": {"access_token": " ", "id_token": id_token}});
        assert!(codex::extract_auth(&blank).is_ok());
        assert_eq!(
            proxy_login(Pool::Codex, &blank),
            Err(crate::utils::localized(
                "error.auth.missing_access_token",
                &[]
            ))
        );
        let padded = json!({"tokens": {"access_token": " token\n", "id_token": id_token}});
        assert_eq!(
            proxy_login(Pool::Codex, &padded).unwrap().access_token,
            "token"
        );
        // The Claude entry the proxy reads is the one its Credential is of,
        // whose scope a `scope` string gives without a `scopes` list.
        let root =
            json!({"accessToken": "claude-at", "refreshToken": "rt", "scope": "user:inference"});
        assert_eq!(claude::oauth(&root), &root);
        let credential = claude::credential(&root, &Identity::default()).unwrap();
        assert_eq!(
            (credential.access.as_str(), credential.scope.as_deref()),
            ("claude-at", Some("user:inference"))
        );
        let mut document = json!({"claudeAiOauth": root.clone(), "other": 1});
        assert_eq!(claude::oauth(&document), &root);
        claude::oauth_mut(&mut document)["accessToken"] = "renewed".into();
        assert_eq!(document["claudeAiOauth"]["accessToken"], "renewed");
        let mut entry = root.clone();
        claude::oauth_mut(&mut entry)["accessToken"] = "renewed".into();
        assert_eq!(entry["accessToken"], "renewed");
    }

    #[test]
    fn expiries_read_seconds_milliseconds_fractions_and_rfc3339() {
        for (value, expected) in [
            (json!(1_783_850_400), Some(1_783_850_400_000)),
            (json!(1_783_850_400_000_i64), Some(1_783_850_400_000)),
            (json!(1_783_850_400.25), Some(1_783_850_400_250)),
            (json!(1_783_850_400_000.9), Some(1_783_850_400_000)),
            // Around 1e11: below it seconds, from it milliseconds.
            (json!(99_999_999_999_i64), Some(99_999_999_999_000)),
            (json!(100_000_000_000_i64), Some(100_000_000_000)),
            (json!(99_999_999_999.5), Some(99_999_999_999_500)),
            (json!("2026-07-12T10:00:00Z"), Some(1_783_850_400_000)),
            (
                json!("2026-07-12T18:00:00.5+08:00"),
                Some(1_783_850_400_500),
            ),
            (json!("tomorrow"), None),
            (json!(null), None),
        ] {
            assert_eq!(timestamp_ms(&value), expected, "{value}");
        }
        assert_eq!(
            jwt_exp_ms(&json!({"exp": 1_780_000_123.75})),
            Some(1_780_000_123_750)
        );
        assert_eq!(
            jwt_exp_ms(&json!({"exp": 1_780_000_123})),
            Some(1_780_000_123_000)
        );
        assert_eq!(jwt_exp_ms(&json!({"exp": "1780000123"})), None);
        assert_eq!(seconds(1_780_000_123_750), 1_780_000_123);
        assert_eq!(credential_expiry(Some(5)), 5);
        assert!(credential_expiry(Some(0)) > 1_700_000_000_000);
        assert!(credential_expiry(None) > 1_700_000_000_000);
    }
}
