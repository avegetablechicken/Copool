//! The Antigravity pool's Login: the document `agy` keeps in its item of
//! the system's credential store, an OAuth token as JSON (`{"token":
//! {"access_token", "refresh_token", "expiry", …}}`, or the token's members
//! by themselves), as go-keyring stores it, on macOS wrapped as
//! `go-keyring-base64:`.
#![allow(dead_code)]
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{Map, Value};

type Result<T> = std::result::Result<T, String>;

const WRAPPED: &str = "go-keyring-base64:";

/// A login document and whether its item wraps it as go-keyring does.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Document {
    pub(crate) value: Value,
    pub(crate) wrapped: bool,
}

/// The document an item holds; none when it is no login with a refresh
/// token, which alone keeps an account usable.
pub(crate) fn decode(bytes: &[u8]) -> Option<Document> {
    let text = std::str::from_utf8(bytes)
        .ok()?
        .trim()
        .trim_start_matches('\u{feff}')
        .trim();
    let (text, wrapped) = match text.strip_prefix(WRAPPED) {
        Some(encoded) => (
            String::from_utf8(STANDARD.decode(encoded.trim()).ok()?).ok()?,
            true,
        ),
        None => (text.to_owned(), false),
    };
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    refresh_token(&value)?;
    Some(Document { value, wrapped })
}
/// The bytes an item holds for a document.
pub(crate) fn encode(document: &Document) -> Vec<u8> {
    let json = document.value.to_string();
    if document.wrapped {
        format!("{WRAPPED}{}", STANDARD.encode(json)).into_bytes()
    } else {
        json.into_bytes()
    }
}

const ACCESS_KEYS: [&str; 2] = ["access_token", "accessToken"];
const REFRESH_KEYS: [&str; 2] = ["refresh_token", "refreshToken"];
const EXPIRY_KEYS: [&str; 3] = ["expiry", "expires_at", "expiresAt"];

/// Where a document keeps its tokens: its `token` object, else itself.
fn token(document: &Value) -> Option<&Map<String, Value>> {
    let object = document.as_object()?;
    Some(
        object
            .get("token")
            .and_then(Value::as_object)
            .unwrap_or(object),
    )
}
fn token_mut(document: &mut Value) -> Option<&mut Map<String, Value>> {
    let nested = document.get("token").is_some_and(Value::is_object);
    let object = document.as_object_mut()?;
    if nested {
        object.get_mut("token").and_then(Value::as_object_mut)
    } else {
        Some(object)
    }
}
fn first(document: &Value, keys: &[&str]) -> Option<String> {
    let token = token(document)?;
    keys.iter()
        .find_map(|key| super::non_empty(token.get(*key).and_then(Value::as_str)))
}
pub(crate) fn access_token(document: &Value) -> Option<String> {
    first(document, &ACCESS_KEYS)
}
pub(crate) fn refresh_token(document: &Value) -> Option<String> {
    first(document, &REFRESH_KEYS)
}
/// When the access token expires (Unix milliseconds): RFC 3339, as Go
/// writes it, or a number of seconds or milliseconds, read as every pool's
/// expiry is.
pub(crate) fn expiry_ms(document: &Value) -> Option<i64> {
    let token = token(document)?;
    EXPIRY_KEYS
        .iter()
        .find_map(|key| token.get(*key).and_then(super::timestamp_ms))
}
/// When the access token expires (Unix seconds).
pub(crate) fn expiry(document: &Value) -> Option<i64> {
    expiry_ms(document).map(super::seconds)
}
/// The email a document names, if any.
pub(crate) fn email(document: &Value) -> Option<String> {
    [
        "/id_token_email",
        "/email",
        "/account/email",
        "/user/email",
        "/token/email",
    ]
    .iter()
    .find_map(|path| {
        let email = document.pointer(path).and_then(Value::as_str)?.trim();
        email.contains('@').then(|| email.to_owned())
    })
}
/// The Google account a document names: its own id, else its ID token's
/// subject.
pub(crate) fn account_id(document: &Value) -> Option<String> {
    ["google_id", "account_id", "accountId", "googleId"]
        .iter()
        .find_map(|key| super::non_empty(document.get(*key).and_then(Value::as_str)))
        .or_else(|| {
            let id_token = document.get("id_token").and_then(Value::as_str)?;
            let claims = super::codex::decode_jwt_payload(id_token).ok()?;
            super::non_empty(claims.get("sub").and_then(Value::as_str))
        })
}
/// The account Copool's proxy routes an Antigravity login as: the one the
/// login names ([`account_id`]), else the Google id Copool stored for it.
pub(crate) fn proxy_account_id(login: &Value, stored_google_id: Option<&str>) -> Option<String> {
    account_id(login).or_else(|| super::non_empty(stored_google_id))
}
/// The Credential an Antigravity Login grants: its token's tokens and
/// expiry, and the account the document names. What Copool stores of the
/// account, given as `identity`, stands in for the document's root `email`
/// and `account_id` where it has no such member, or an empty or null one:
/// the stored email follows the ID token's and precedes the document's
/// nested ones, the stored id follows `google_id` and precedes the
/// document's other ids and its ID token's subject. One that names no
/// account at all is refused: omp's `antigravity-cli` keeps each account
/// apart by its email or id.
pub(crate) fn credential(
    login: &Value,
    identity: &super::Identity,
) -> Result<super::OAuthCredential> {
    let access = access_token(login).ok_or("Missing access token")?;
    let refresh = refresh_token(login).ok_or("Missing refresh token")?;
    let mut named = login.clone();
    if let Some(root) = named.as_object_mut() {
        for (key, stored) in [
            ("email", &identity.email),
            ("account_id", &identity.account_id),
        ] {
            let own = super::non_empty(root.get(key).and_then(Value::as_str));
            if let (None, Some(stored)) = (own, super::non_empty(stored.as_deref())) {
                root.insert(key.into(), stored.into());
            }
        }
    }
    // Else the root `email` as it stands, though it names no address.
    let email =
        email(&named).or_else(|| super::non_empty(named.get("email").and_then(Value::as_str)));
    let account_id = account_id(&named);
    if email.is_none() && account_id.is_none() {
        return Err("Missing Antigravity account identity".into());
    }
    Ok(super::OAuthCredential {
        access,
        refresh,
        expires_ms: super::credential_expiry(expiry_ms(login)),
        account_id,
        org_id: None,
        email,
        org_name: None,
        scope: None,
    })
}
/// Whether two documents are the same login: the same refresh token.
pub(crate) fn same_login(a: &Value, b: &Value) -> bool {
    refresh_token(a).is_some() && refresh_token(a) == refresh_token(b)
}
/// Puts a renewal's tokens in the document, under the keys it uses.
pub(crate) fn apply_renewal(
    document: &mut Value,
    access_token: &str,
    expires_at: i64,
    expires_in: i64,
    refresh_token: Option<&str>,
) {
    let Some(token) = token_mut(document) else {
        return;
    };
    let key = |keys: [&'static str; 2], token: &Map<String, Value>| {
        keys.into_iter()
            .find(|key| token.contains_key(*key))
            .unwrap_or(keys[0])
    };
    let access = key(ACCESS_KEYS, token);
    token.insert(access.into(), access_token.into());
    if let Some(refresh) = refresh_token.filter(|r| !r.trim().is_empty()) {
        let key = key(REFRESH_KEYS, token);
        token.insert(key.into(), refresh.into());
    }
    let expiry_key = EXPIRY_KEYS
        .into_iter()
        .find(|key| token.contains_key(*key))
        .unwrap_or("expiry");
    // A number stays a number of the same unit; otherwise RFC 3339 in UTC.
    let value = match token.get(expiry_key) {
        Some(Value::Number(n))
            if n.as_f64()
                .is_some_and(|n| n >= super::MILLISECONDS_FROM as f64) =>
        {
            Value::from(expires_at * 1000)
        }
        Some(Value::Number(_)) => Value::from(expires_at),
        _ => chrono::DateTime::from_timestamp(expires_at, 0)
            .map(|date| {
                date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                    .into()
            })
            .unwrap_or(Value::Null),
    };
    token.insert(expiry_key.into(), value);
    // Antigravity also keeps the token's lifetime.
    if token.contains_key("expires_in") {
        token.insert("expires_in".into(), expires_in.into());
    }
}

/// A Google sign-in's token response as Antigravity keeps its login: the
/// token, how it signed in, and the email of its ID token.
pub(crate) fn signed_in(tokens: &Value, now: i64) -> Result<Value> {
    let text = |key: &str| {
        tokens[key]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let access = text("access_token").ok_or("Google returned no access token")?;
    let refresh = text("refresh_token").ok_or("Google returned no refresh token")?;
    let expires_in = tokens["expires_in"]
        .as_i64()
        .filter(|s| *s > 0)
        .unwrap_or(3599);
    let mut document = serde_json::json!({"token": {
        "access_token": access,
        "token_type": text("token_type").unwrap_or("Bearer"),
        "refresh_token": refresh,
        "expiry": "",
        "expires_in": expires_in,
    }, "auth_method": "consumer"});
    apply_renewal(&mut document, access, now + expires_in, expires_in, None);
    let email = text("id_token")
        .and_then(|jwt| jwt.split('.').nth(1))
        .and_then(|payload| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(payload.trim_end_matches('='))
                .ok()
        })
        .and_then(|claims| serde_json::from_slice::<Value>(&claims).ok())
        .and_then(|claims| claims["email"].as_str().map(str::to_owned));
    if let Some(email) = email {
        document["id_token_email"] = email.into();
    }
    Ok(document)
}

/// The proxy form of an Antigravity login: not a Login, but what Copool's
/// proxy keeps of one, locally and in a remote daemon's `accounts.json`,
/// as a candidate's `authJson`. Its shape is what deployed daemons read and
/// write, so it does not change:
///
/// - `accessToken`: the access token;
/// - `project`: the Cloud Code project of its requests, once reported;
/// - `refreshToken`: the refresh token, given only to a daemon allowed to
///   renew the login;
/// - `sourceGeneration`: which login uploaded to a daemon it was renewed
///   from, so that only a renewal of the login still kept is taken back;
/// - `expiresAt`: when the access token expires, in Unix **seconds**
///   (unlike a Login's expiry, never milliseconds);
/// - `expiresIn`: the access token's lifetime in seconds, as a renewal
///   reported it.
pub(crate) mod proxy_form {
    use serde_json::Value;

    pub(crate) fn access_token(auth: &Value) -> Option<String> {
        super::super::non_empty(auth.get("accessToken").and_then(Value::as_str))
    }
    pub(crate) fn refresh_token(auth: &Value) -> Option<String> {
        super::super::non_empty(auth.get("refreshToken").and_then(Value::as_str))
    }
    pub(crate) fn project(auth: &Value) -> Option<&str> {
        auth.get("project").and_then(Value::as_str)
    }
    pub(crate) fn source_generation(auth: &Value) -> Option<&str> {
        auth.get("sourceGeneration").and_then(Value::as_str)
    }
    /// When the access token expires (Unix seconds, a whole number).
    pub(crate) fn expires_at(auth: &Value) -> Option<i64> {
        auth.get("expiresAt").and_then(Value::as_i64)
    }
    /// When the access token expires (Unix milliseconds), as a Credential
    /// keeps it.
    pub(crate) fn expiry_ms(auth: &Value) -> Option<i64> {
        expires_at(auth).map(|at| at.saturating_mul(1000))
    }
    /// The access token's lifetime (seconds, a whole number).
    pub(crate) fn lifetime(auth: &Value) -> Option<i64> {
        auth.get("expiresIn").and_then(Value::as_i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::json;

    #[test]
    fn documents_keep_their_form_through_a_round_trip() {
        let value = json!({"token": {"access_token": "a", "refresh_token": "r", "expiry": "2026-07-12T10:00:00Z"}});
        let wrapped = format!("go-keyring-base64:{}", STANDARD.encode(value.to_string()));
        let document = decode(wrapped.as_bytes()).unwrap();
        assert!(document.wrapped);
        assert_eq!(document.value, value);
        assert_eq!(encode(&document), wrapped.into_bytes());
        let plain = decode(value.to_string().as_bytes()).unwrap();
        assert!(!plain.wrapped);
        assert_eq!(encode(&plain), value.to_string().into_bytes());
        assert_eq!(access_token(&value).as_deref(), Some("a"));
        assert_eq!(refresh_token(&value).as_deref(), Some("r"));
        assert_eq!(expiry(&value), Some(1_783_850_400));
    }

    #[test]
    fn only_logins_with_a_refresh_token_are_documents() {
        for bytes in [
            b"raw-access-token".as_slice(),
            br#"{"token": {"access_token": "a"}}"#,
            br#"[]"#,
            b"go-keyring-base64:not-base64!",
        ] {
            assert!(decode(bytes).is_none());
        }
        let flat =
            decode(br#"{"accessToken": "a", "refreshToken": "r", "expiresAt": 1783850400000}"#)
                .unwrap();
        assert_eq!(refresh_token(&flat.value).as_deref(), Some("r"));
        assert_eq!(expiry(&flat.value), Some(1_783_850_400));
    }

    #[test]
    fn a_sign_in_is_kept_as_antigravity_keeps_its_login() {
        let claims = URL_SAFE_NO_PAD.encode(r#"{"email":"alice@gmail.com","sub":"1"}"#);
        let tokens = json!({
            "access_token": "a", "refresh_token": "r", "expires_in": 3599,
            "token_type": "Bearer", "scope": "openid", "id_token": format!("h.{claims}.s")
        });
        let document = signed_in(&tokens, 1_783_846_801).unwrap();
        // The keys, in the order of Antigravity's own login.
        assert_eq!(
            document.to_string(),
            json!({"token": {
                "access_token": "a", "token_type": "Bearer", "refresh_token": "r",
                "expiry": "2026-07-12T10:00:00Z", "expires_in": 3599
            }, "auth_method": "consumer", "id_token_email": "alice@gmail.com"})
            .to_string()
        );
        assert_eq!(email(&document).as_deref(), Some("alice@gmail.com"));
        // A renewal keeps the lifetime current too.
        let mut renewed = document.clone();
        apply_renewal(&mut renewed, "b", 1_783_850_400, 1800, None);
        assert_eq!(renewed["token"]["expires_in"], 1800);
        assert!(signed_in(&json!({"access_token": "a"}), 0).is_err());
        let unnamed = signed_in(&json!({"access_token": "a", "refresh_token": "r"}), 0).unwrap();
        assert!(unnamed.get("id_token_email").is_none());
    }

    #[test]
    fn login_and_identity_give_the_credential() {
        use super::super::{Identity, OAuthCredential};
        let stored = Identity {
            account_id: Some("google-stored".into()),
            email: Some("Stored@Gmail.com".into()),
            ..Identity::default()
        };
        let expected =
            |expires_ms: i64, account: Option<&str>, email: Option<&str>| OAuthCredential {
                access: "a".into(),
                refresh: "r".into(),
                expires_ms,
                account_id: account.map(Into::into),
                email: email.map(Into::into),
                ..OAuthCredential::default()
            };
        let token =
            |expiry: Value| json!({"access_token": "a", "refresh_token": "r", "expiry": expiry});
        let id_token = format!("h.{}.s", URL_SAFE_NO_PAD.encode(r#"{"sub":"google-sub"}"#));
        for (name, login, identity, credential_expected) in [
            (
                "the document's own id and email, kept as spelled, over the stored ones",
                json!({"token": token(json!("2026-07-12T10:00:00Z")), "id_token_email": "Alice@Gmail.com", "google_id": "google-doc"}),
                stored.clone(),
                expected(
                    1_783_850_400_000,
                    Some("google-doc"),
                    Some("Alice@Gmail.com"),
                ),
            ),
            (
                "the stored id over the ID token's subject",
                json!({"token": token(json!(1_783_850_400)), "id_token": id_token}),
                stored.clone(),
                expected(
                    1_783_850_400_000,
                    Some("google-stored"),
                    Some("Stored@Gmail.com"),
                ),
            ),
            (
                "the ID token's subject when nothing else names the account",
                json!({"token": token(json!(1_783_850_400)), "id_token": id_token}),
                Identity::default(),
                expected(1_783_850_400_000, Some("google-sub"), None),
            ),
            (
                "the document's root `account_id` and `email` over the stored ones",
                json!({"token": token(json!(1_783_850_400)), "id_token": id_token, "account_id": "google-root", "email": "root@gmail.com"}),
                stored.clone(),
                expected(
                    1_783_850_400_000,
                    Some("google-root"),
                    Some("root@gmail.com"),
                ),
            ),
            (
                "the stored id and email over the document's other places",
                json!({"token": token(json!(1_783_850_400)), "accountId": "google-camel", "user": {"email": "user@gmail.com"}}),
                stored.clone(),
                expected(
                    1_783_850_400_000,
                    Some("google-stored"),
                    Some("Stored@Gmail.com"),
                ),
            ),
            (
                "the stored id and email over an empty root email and a null root id",
                json!({"token": token(json!(1_783_850_400)), "email": " ", "account_id": null}),
                stored.clone(),
                expected(
                    1_783_850_400_000,
                    Some("google-stored"),
                    Some("Stored@Gmail.com"),
                ),
            ),
            (
                "a root email naming no address, kept when nothing else names one",
                json!({"token": token(json!(1_783_850_400)), "email": " alice "}),
                stored.clone(),
                expected(1_783_850_400_000, Some("google-stored"), Some("alice")),
            ),
            (
                "the stored id and email when the document names neither",
                json!({"accessToken": "a", "refreshToken": "r", "expiresAt": 1_783_850_400_250_i64}),
                stored.clone(),
                expected(
                    1_783_850_400_250,
                    Some("google-stored"),
                    Some("Stored@Gmail.com"),
                ),
            ),
            (
                "an email alone, and a fractional expiry in seconds",
                json!({"token": token(json!(1_783_850_400.5)), "email": "bob@gmail.com"}),
                Identity::default(),
                expected(1_783_850_400_500, None, Some("bob@gmail.com")),
            ),
        ] {
            assert_eq!(
                credential(&login, &identity),
                Ok(credential_expected),
                "{name}"
            );
        }
        for (login, error) in [
            (
                json!({"token": token(json!(1))}),
                "Missing Antigravity account identity",
            ),
            (
                json!({"token": {"refresh_token": "r"}, "email": "a@b.c"}),
                "Missing access token",
            ),
            (
                json!({"token": {"access_token": "a"}, "email": "a@b.c"}),
                "Missing refresh token",
            ),
        ] {
            assert_eq!(
                credential(&login, &Identity::default()).unwrap_err(),
                error,
                "{login}"
            );
        }
        // Seconds below 1e11, milliseconds from it.
        assert_eq!(
            expiry(&json!({"expiry": 99_999_999_999_i64})),
            Some(99_999_999_999)
        );
        assert_eq!(
            expiry(&json!({"expiry": 100_000_000_000_i64})),
            Some(100_000_000)
        );
    }

    #[test]
    fn an_empty_member_gives_way_to_the_next_one() {
        let document = json!({"token": {"access_token": "", "accessToken": "a",
            "refresh_token": " ", "refreshToken": "r"},
            "id_token_email": "", "email": "alice", "account": {"email": " alice@gmail.com "}});
        assert_eq!(access_token(&document).as_deref(), Some("a"));
        assert_eq!(refresh_token(&document).as_deref(), Some("r"));
        assert_eq!(email(&document).as_deref(), Some("alice@gmail.com"));
        assert!(decode(document.to_string().as_bytes()).is_some());
        let none = json!({"access_token": " ", "refresh_token": "r", "email": ""});
        assert_eq!(access_token(&none), None);
        assert_eq!(email(&none), None);
    }

    #[test]
    fn the_proxy_form_reads_its_expiry_in_seconds() {
        let auth = json!({
            "accessToken": " a ", "refreshToken": "r", "project": "project-1",
            "sourceGeneration": "g", "expiresAt": 1_783_850_400, "expiresIn": 3599
        });
        assert_eq!(proxy_form::access_token(&auth).as_deref(), Some("a"));
        assert_eq!(proxy_form::refresh_token(&auth).as_deref(), Some("r"));
        assert_eq!(proxy_form::project(&auth), Some("project-1"));
        assert_eq!(proxy_form::source_generation(&auth), Some("g"));
        assert_eq!(proxy_form::expires_at(&auth), Some(1_783_850_400));
        assert_eq!(proxy_form::expiry_ms(&auth), Some(1_783_850_400_000));
        assert_eq!(proxy_form::lifetime(&auth), Some(3599));
        // Seconds, even past 1e11, where a Login's expiry is milliseconds.
        let far = json!({"expiresAt": 100_000_000_000_i64});
        assert_eq!(proxy_form::expires_at(&far), Some(100_000_000_000));
        assert_eq!(proxy_form::expiry_ms(&far), Some(100_000_000_000_000));
        // A login sent to a daemon not allowed to renew it, before a renewal.
        let bare = json!({"accessToken": "a", "project": null, "expiresAt": null});
        assert_eq!(proxy_form::refresh_token(&bare), None);
        assert_eq!(proxy_form::project(&bare), None);
        assert_eq!(proxy_form::source_generation(&bare), None);
        assert_eq!(proxy_form::expires_at(&bare), None);
        assert_eq!(proxy_form::lifetime(&bare), None);
        assert_eq!(proxy_form::access_token(&json!({"accessToken": " "})), None);
    }

    #[test]
    fn the_proxy_names_a_login_by_its_own_id_else_the_stored_one() {
        let id_token = format!("h.{}.s", URL_SAFE_NO_PAD.encode(r#"{"sub":"google-sub"}"#));
        let token = json!({"access_token": "a", "refresh_token": "r"});
        for (login, stored, expected) in [
            (
                json!({"token": token, "google_id": "google-doc"}),
                Some("google-stored"),
                Some("google-doc"),
            ),
            (
                json!({"token": token, "id_token": id_token}),
                Some("google-stored"),
                Some("google-sub"),
            ),
            (
                json!({"token": token}),
                Some("google-stored"),
                Some("google-stored"),
            ),
            (json!({"token": token}), Some(" "), None),
            (json!({"token": token}), None, None),
        ] {
            assert_eq!(
                proxy_account_id(&login, stored).as_deref(),
                expected,
                "{login} {stored:?}"
            );
        }
    }

    #[test]
    fn a_renewal_writes_the_keys_and_forms_the_document_uses() {
        let mut nested = json!({"token": {"access_token": "a", "refresh_token": "r", "expiry": "2026-01-01T00:00:00.5+08:00", "token_type": "Bearer"}, "other": 1});
        apply_renewal(&mut nested, "b", 1_783_850_400, 3599, None);
        assert_eq!(
            nested,
            json!({"token": {"access_token": "b", "refresh_token": "r", "expiry": "2026-07-12T10:00:00Z", "token_type": "Bearer"}, "other": 1})
        );
        let mut flat =
            json!({"accessToken": "a", "refreshToken": "r", "expiresAt": 1_000_000_000_000_i64});
        apply_renewal(&mut flat, "b", 1_783_850_400, 3599, Some("s"));
        assert_eq!(
            flat,
            json!({"accessToken": "b", "refreshToken": "s", "expiresAt": 1_783_850_400_000_i64})
        );
    }
}
