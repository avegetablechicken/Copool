//! What each pool's tool signs in with, its Login, read and written as a
//! document: the tokens, expiry and identity it holds, which of two logins
//! is newer, and how a renewal is applied to it, and the Credential it
//! grants. Shared by the app and the remote daemon; nothing here reads or
//! writes where a Login is kept, nor contacts a server.
pub(crate) mod antigravity;
pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod grok;

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
