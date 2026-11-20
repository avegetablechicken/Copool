use super::*;
use crate::{account_network::Response, Preferences};
use serde_json::json;
use std::sync::Mutex;

/// Antigravity's login item in memory.
#[derive(Default)]
pub(crate) struct Live {
    bytes: Mutex<Option<Vec<u8>>>,
    writes: Mutex<usize>,
}
impl LiveStore for Live {
    fn read(&self, _: KeychainAccess) -> Response<'_, Option<Vec<u8>>> {
        let bytes = self.bytes.lock().unwrap().clone();
        Box::pin(async move { Ok(bytes) })
    }
    fn write(&self, bytes: Vec<u8>, _: KeychainAccess) -> Response<'_, ()> {
        *self.bytes.lock().unwrap() = Some(bytes);
        *self.writes.lock().unwrap() += 1;
        Box::pin(async { Ok(()) })
    }
}
impl Live {
    fn sign_in(&self, document: &Value) {
        *self.bytes.lock().unwrap() = Some(login::encode(&Document {
            value: document.clone(),
            wrapped: true,
        }));
    }
    fn document(&self) -> Document {
        login::decode(self.bytes.lock().unwrap().as_deref().unwrap()).unwrap()
    }
}

/// Google, answering by the access token: tokens starting with "expired"
/// are refused, and a renewal hands out "fresh-<refresh token>".
#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    /// Refresh tokens Google refuses.
    revoked: Mutex<Vec<String>>,
    /// Remaining fraction of the Gemini five-hour pool.
    remaining: Mutex<f64>,
    /// The summary reports only the third-party pool, the Gemini models
    /// their quota one by one, as for a client Cloud Code does not show the
    /// Gemini pool.
    per_model_gemini: std::sync::atomic::AtomicBool,
}
impl Fake {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn renewals(&self) -> usize {
        self.calls()
            .iter()
            .filter(|c| c.starts_with("renew"))
            .count()
    }
}
impl Network for Fake {
    fn cloud<'a>(
        &'a self,
        path: &'a str,
        token: &'a str,
        _: Value,
        agent: &'a str,
        proxy: &'a str,
    ) -> Response<'a, api::Reply> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{path} {token} {agent} {proxy}"));
        let remaining = *self.remaining.lock().unwrap();
        let per_model = self
            .per_model_gemini
            .load(std::sync::atomic::Ordering::Relaxed);
        let versioned = agent.starts_with("antigravity/");
        let agent = agent.to_owned();
        Box::pin(async move {
            if token.starts_with("expired") {
                return Ok((401, json!({"error": {"message": "expired"}})));
            }
            Ok(match path {
                // As a Google AI Pro account's read without the Gemini pool.
                api::QUOTA_SUMMARY_PATH if per_model => (
                    200,
                    json!({"groups": [{"displayName": "Claude and GPT models", "buckets": [
                        {"bucketId": "3p-weekly", "window": "weekly", "resetTime": "2026-11-27T17:59:22Z", "remainingFraction": 1},
                        {"bucketId": "3p-5h", "window": "5h", "resetTime": "2026-11-20T22:59:22Z", "remainingFraction": 0.75}
                    ]}]}),
                ),
                api::FETCH_MODELS_PATH if per_model => (
                    200,
                    json!({"models": {
                        "tab_flash_lite_preview": {"model": "MODEL_PLACEHOLDER_M19", "quotaInfo": {"remainingFraction": 1}},
                        "gemini-3-flash": {"displayName": "Gemini 3 Flash", "model": "MODEL_PLACEHOLDER_M18",
                            "quotaInfo": {"remainingFraction": remaining, "resetTime": "2026-11-20T21:18:53Z"}},
                        "gemini-3.1-flash-lite": {"displayName": "Gemini 3.1 Flash Lite", "model": "MODEL_PLACEHOLDER_M50",
                            "quotaInfo": {"remainingFraction": 0.9793814, "resetTime": "2026-11-20T21:18:53Z"}},
                        "claude-opus-5-5-high": {"displayName": "Claude Opus 5.5 (High)", "model": "MODEL_PLACEHOLDER_M402",
                            "quotaInfo": {"remainingFraction": 0.1, "resetTime": "2026-11-20T22:59:41Z"}}
                    }}),
                ),
                // As Cloud Code answers: the Gemini pool only to a client
                // naming its version.
                api::QUOTA_SUMMARY_PATH if !versioned => (
                    200,
                    json!({"groups": [{"buckets": [
                        {"bucketId": "3p-5h", "remainingFraction": 0.5}
                    ]}]}),
                ),
                api::QUOTA_SUMMARY_PATH => (
                    200,
                    json!({"groups": [
                        {"displayName": "Gemini Models", "buckets": [
                            {"bucketId": "gemini-weekly", "window": "weekly", "remainingFraction": 0.9},
                            {"bucketId": "gemini-5h", "window": "5h", "remainingFraction": remaining, "resetTime": "2026-07-12T10:00:00Z"}
                        ]},
                        {"displayName": "Claude and GPT models", "buckets": [
                            {"bucketId": "3p-5h", "remainingFraction": 0.5}
                        ]}
                    ]}),
                ),
                api::LOAD_CODE_ASSIST_PATH if agent != api::agent() => (
                    200,
                    json!({"ineligibleTiers": [{"reasonCode": "UNSUPPORTED_CLIENT"}]}),
                ),
                api::LOAD_CODE_ASSIST_PATH => (200, json!({"paidTier": {"name": "Google AI Pro"}})),
                _ => (404, Value::Null),
            })
        })
    }
    fn renew<'a>(&'a self, refresh_token: &'a str, proxy: &'a str) -> Response<'a, api::Reply> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("renew {refresh_token} {proxy}"));
        let revoked = self
            .revoked
            .lock()
            .unwrap()
            .contains(&refresh_token.to_owned());
        Box::pin(async move {
            Ok(if revoked {
                (400, json!({"error": "invalid_grant"}))
            } else {
                (
                    200,
                    json!({"access_token": format!("fresh-{refresh_token}"), "expires_in": 3599}),
                )
            })
        })
    }
    fn identity<'a>(&'a self, token: &'a str, _: &'a str) -> Response<'a, api::Reply> {
        self.calls.lock().unwrap().push(format!("identity {token}"));
        // The account is named by the refresh token the access token came from.
        let user = token
            .trim_start_matches("fresh-")
            .trim_start_matches("access-")
            .trim_start_matches("refresh-")
            .split('-')
            .next()
            .unwrap_or_default()
            .to_owned();
        Box::pin(async move {
            Ok((
                200,
                json!({"id": format!("id-{user}"), "email": format!("{user}@gmail.com")}),
            ))
        })
    }
}

/// A login of `user`: its refresh token `refresh-<user>`.
fn login_of(user: &str, access: &str, expires: i64) -> Value {
    json!({"token": {
        "access_token": access,
        "token_type": "Bearer",
        "refresh_token": format!("refresh-{user}"),
        "expiry": chrono::DateTime::from_timestamp(expires, 0).unwrap().to_rfc3339()
    }})
}
fn later() -> i64 {
    now_unix_seconds() + 3600
}

struct Harness {
    app: Application,
    live: Arc<Live>,
    fake: Arc<Fake>,
    _dir: tempfile::TempDir,
}
async fn harness(request_new_accounts: bool) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let item = crate::vault::Item::new(Arc::new(crate::vault::tests::Memory::default()));
    let app = Application::open_with_secrets(
        crate::Paths {
            data_dir: dir.path().join("data"),
            codex_auth: dir.path().join("codex/auth.json"),
        },
        Arc::new(crate::vault::ItemSecrets::new(item)),
    )
    .unwrap();
    let live = Arc::new(Live::default());
    app.set_antigravity_live(live.clone());
    let fake = Arc::new(Fake::default());
    *fake.remaining.lock().unwrap() = 0.75;
    app.set_antigravity_network(fake.clone());
    let preferences = Preferences {
        request_new_accounts,
        ..app.preferences().unwrap()
    };
    app.save_preferences(preferences).await.unwrap();
    Harness {
        app,
        live,
        fake,
        _dir: dir,
    }
}
async fn cards(app: &Application) -> Vec<AccountSummary> {
    app.accounts(Pool::Antigravity).await.unwrap()
}

#[tokio::test]
async fn an_imported_login_waits_then_shows_its_quota_name_and_plan() {
    let h = harness(false).await;
    h.live.sign_in(&login_of("alice", "access-alice", later()));
    assert_eq!(h.app.import_current(Pool::Antigravity).await.unwrap(), 1);
    let card = cards(&h.app).await.remove(0);
    assert_eq!(card.label, UNNAMED);
    assert!(card.awaiting_first_request && card.is_current);
    assert!(h.fake.calls().is_empty());

    h.app
        .save_account_proxy(Pool::Antigravity, &card.id, "socks5://127.0.0.1:7")
        .await
        .unwrap();
    h.app
        .refresh_now(Pool::Antigravity, Some(&card.id))
        .await
        .unwrap();
    let card = cards(&h.app).await.remove(0);
    assert_eq!(card.label, "alice@gmail.com");
    assert_eq!(card.email.as_deref(), Some("alice@gmail.com"));
    assert_eq!(card.plan_type.as_deref(), Some("pro"));
    assert!(!card.awaiting_first_request && card.is_current);
    let usage = card.usage.unwrap();
    assert_eq!(usage.plan_type.as_deref(), Some("pro"));
    assert_eq!(usage.five_hour.unwrap().used_percent, 25.0);
    assert_eq!(usage.one_week.unwrap().used_percent, 10.0);
    assert_eq!(usage.third_party_five_hour.unwrap().used_percent, 50.0);
    assert!(usage.third_party_one_week.is_none());
    // Every request goes through the account's proxy, as Antigravity's.
    assert_eq!(
        h.fake.calls(),
        [
            format!(
                "{} access-alice {} socks5://127.0.0.1:7",
                api::QUOTA_SUMMARY_PATH,
                api::agent()
            ),
            "identity access-alice".into(),
            format!(
                "{} access-alice {} socks5://127.0.0.1:7",
                api::LOAD_CODE_ASSIST_PATH,
                api::agent()
            ),
        ]
    );
    // Nothing is written to Antigravity's login by a refresh.
    assert_eq!(*h.live.writes.lock().unwrap(), 0);
}

#[tokio::test]
async fn expired_or_refused_tokens_are_renewed_as_the_renewal_allows() {
    let h = harness(true).await;
    // Expired: renewed first.
    h.app
        .import_json(
            Pool::Antigravity,
            &login_of("alice", "access-alice", now_unix_seconds() - 10).to_string(),
        )
        .await
        .unwrap();
    let alice = cards(&h.app).await.remove(0);
    assert!(alice.usage_error.is_none());
    assert_eq!(h.fake.renewals(), 1);
    let store = h.app.antigravity_load().unwrap();
    let document = &store.accounts[0].document;
    assert_eq!(
        login::access_token(document).as_deref(),
        Some("fresh-refresh-alice")
    );
    assert!(login::expiry(document).unwrap() > now_unix_seconds() + 3000);

    // Refused though unexpired: renewed once, then asked again.
    h.app
        .import_json(
            Pool::Antigravity,
            &login_of("bob", "expired-bob", later()).to_string(),
        )
        .await
        .unwrap();
    let bob = cards(&h.app)
        .await
        .into_iter()
        .find(|c| c.email.as_deref() == Some("bob@gmail.com"))
        .unwrap();
    assert!(bob.usage.is_some() && bob.usage_error.is_none());
    assert_eq!(h.fake.renewals(), 2);

    // Renewal off: an expired account reports it and is not renewed.
    h.app
        .set_renewal(Pool::Antigravity, &bob.id, "never")
        .await
        .unwrap();
    {
        let _lock = h.app.antigravity.lock.lock().await;
        let mut store = h.app.antigravity_load().unwrap();
        let account = store.accounts.iter_mut().find(|a| a.id == bob.id).unwrap();
        account.document = login_of("bob", "access-bob", now_unix_seconds() - 1);
        h.app.antigravity_save(&store).unwrap();
    }
    let error = h
        .app
        .refresh_usage(Pool::Antigravity, &bob.id)
        .await
        .unwrap_err();
    assert_eq!(error, RENEWAL_OFF);
    assert_eq!(h.fake.renewals(), 2);
    // Renew now renews whatever the choice, which stays.
    h.app.renew_now(Pool::Antigravity, &bob.id).await.unwrap();
    assert_eq!(h.fake.renewals(), 3);
    let bob = cards(&h.app)
        .await
        .into_iter()
        .find(|c| c.id == bob.id)
        .unwrap();
    assert_eq!(bob.renewal.as_deref(), Some("never"));
    assert!(bob.usage_error.is_none());

    // "Once" turns into "never" after a renewal.
    h.app
        .set_renewal(Pool::Antigravity, &alice.id, "once")
        .await
        .unwrap();
    {
        let _lock = h.app.antigravity.lock.lock().await;
        let mut store = h.app.antigravity_load().unwrap();
        let account = store
            .accounts
            .iter_mut()
            .find(|a| a.id == alice.id)
            .unwrap();
        account.document = login_of("alice", "expired-alice", later());
        h.app.antigravity_save(&store).unwrap();
    }
    h.app
        .refresh_usage(Pool::Antigravity, &alice.id)
        .await
        .unwrap();
    let alice = cards(&h.app)
        .await
        .into_iter()
        .find(|c| c.id == alice.id)
        .unwrap();
    assert_eq!(alice.renewal.as_deref(), Some("never"));

    // A revoked login says the sign-in expired.
    h.fake.revoked.lock().unwrap().push("refresh-alice".into());
    {
        let _lock = h.app.antigravity.lock.lock().await;
        let mut store = h.app.antigravity_load().unwrap();
        let account = store
            .accounts
            .iter_mut()
            .find(|a| a.id == alice.id)
            .unwrap();
        account.document = login_of("alice", "expired-alice", later());
        account.renewal = None;
        h.app.antigravity_save(&store).unwrap();
    }
    let error = h
        .app
        .refresh_usage(Pool::Antigravity, &alice.id)
        .await
        .unwrap_err();
    assert_eq!(error, SIGN_IN_EXPIRED);
    let alice = cards(&h.app)
        .await
        .into_iter()
        .find(|c| c.id == alice.id)
        .unwrap();
    assert_eq!(alice.usage_error.as_deref(), Some(SIGN_IN_EXPIRED));
}

#[tokio::test]
async fn switching_writes_the_login_in_its_form_and_keeps_an_unknown_one() {
    let h = harness(true).await;
    for user in ["alice", "bob"] {
        h.app
            .import_json(
                Pool::Antigravity,
                &json!({"document": login_of(user, &format!("access-{user}"), later()), "wrapped": true})
                    .to_string(),
            )
            .await
            .unwrap();
    }
    // Antigravity uses a login the pool does not hold.
    let stranger = login_of("carol", "access-carol", later());
    h.live.sign_in(&stranger);
    let accounts = cards(&h.app).await;
    assert!(accounts.iter().all(|a| !a.is_current));
    let alice = accounts
        .iter()
        .find(|a| a.email.as_deref() == Some("alice@gmail.com"))
        .unwrap();
    h.app
        .switch_account(Pool::Antigravity, &alice.id)
        .await
        .unwrap();
    let written = h.live.document();
    assert!(written.wrapped);
    assert_eq!(
        login::refresh_token(&written.value).as_deref(),
        Some("refresh-alice")
    );
    let backup: Value = serde_json::from_slice(
        &fs::read(h.app.paths.data_dir.join("antigravity-before-switch.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(backup, stranger);
    let accounts = cards(&h.app).await;
    assert!(accounts[0].is_current && accounts[0].id == alice.id);

    // Antigravity renews the login: the pool adopts the newer token.
    h.live
        .sign_in(&login_of("alice", "renewed-by-antigravity", later() + 60));
    cards(&h.app).await;
    let store = h.app.antigravity_load().unwrap();
    let stored = store.accounts.iter().find(|a| a.id == alice.id).unwrap();
    assert_eq!(
        login::access_token(&stored.document).as_deref(),
        Some("renewed-by-antigravity")
    );

    // Smart switch goes to the account with more quota left.
    *h.fake.remaining.lock().unwrap() = 0.0;
    h.app
        .refresh_usage(Pool::Antigravity, &alice.id)
        .await
        .unwrap();
    let outcome = h.app.smart_switch(Pool::Antigravity).await.unwrap();
    assert!(matches!(
        outcome,
        crate::SmartSwitchOutcome::Switched { .. }
    ));
    assert_eq!(
        login::refresh_token(&h.live.document().value).as_deref(),
        Some("refresh-bob")
    );
    // A pool login is not backed up again.
    assert_eq!(*h.live.writes.lock().unwrap(), 2);
}

#[tokio::test]
async fn imports_are_checked_whole_and_folded_into_known_accounts() {
    let h = harness(false).await;
    let alice = login_of("alice", "access-alice", later());
    let invalid = json!([alice, {"token": {"access_token": "no-refresh"}}]).to_string();
    assert_eq!(
        h.app
            .import_json(Pool::Antigravity, &invalid)
            .await
            .unwrap_err(),
        CREDENTIALS_INVALID
    );
    assert!(cards(&h.app).await.is_empty());
    let wrapped = String::from_utf8(login::encode(&Document {
        value: login_of("bob", "access-bob", later()),
        wrapped: true,
    }))
    .unwrap();
    let document =
        json!({"accounts": [alice, wrapped, login_of("alice", "access-alice-2", later())]});
    assert_eq!(
        h.app
            .import_json(Pool::Antigravity, &document.to_string())
            .await
            .unwrap(),
        3
    );
    // The same refresh token is the same account.
    assert_eq!(cards(&h.app).await.len(), 2);
    assert!(h.fake.calls().is_empty());

    // Two logins of one Google account become one card once named.
    let again = login_of("alice-2", "access-alice-other", later());
    h.app
        .import_json(Pool::Antigravity, &again.to_string())
        .await
        .unwrap();
    assert_eq!(cards(&h.app).await.len(), 3);
    h.app.refresh_now(Pool::Antigravity, None).await.unwrap();
    let accounts = cards(&h.app).await;
    assert_eq!(accounts.len(), 2);
    let alice = accounts
        .iter()
        .find(|a| a.email.as_deref() == Some("alice@gmail.com"))
        .unwrap();
    assert!(alice.usage.is_some());
}

#[tokio::test]
async fn the_pool_has_no_proxy_providers_or_greetings() {
    let h = harness(true).await;
    assert!(h.app.proxy_status(Pool::Antigravity).await.is_err());
    assert!(h.app.start_proxy(Pool::Antigravity).await.is_err());
    assert_eq!(h.app.providers(Pool::Antigravity).unwrap(), json!([]));
    h.app
        .import_json(
            Pool::Antigravity,
            &login_of("alice", "access-alice", later()).to_string(),
        )
        .await
        .unwrap();
    let id = cards(&h.app).await[0].id.clone();
    assert!(h.app.say_hi(Pool::Antigravity, &id).await.is_err());
    // Other pools are unaffected by it.
    assert!(h.app.accounts(Pool::Claude).await.unwrap().is_empty());
    h.app.delete_account(Pool::Antigravity, &id).await.unwrap();
    assert!(cards(&h.app).await.is_empty());
}

#[tokio::test]
async fn gemini_models_without_a_shared_pool_report_their_five_hour_window() {
    let h = harness(true).await;
    h.fake
        .per_model_gemini
        .store(true, std::sync::atomic::Ordering::Relaxed);
    h.app
        .import_json(
            Pool::Antigravity,
            &login_of("alice", "access-alice", later()).to_string(),
        )
        .await
        .unwrap();
    let card = cards(&h.app).await.remove(0);
    assert!(card.usage_error.is_none());
    let usage = card.usage.unwrap();
    // The Gemini model with the least left; Gemini has no weekly window.
    let five = usage.five_hour.unwrap();
    assert_eq!(
        (five.used_percent, five.window_seconds),
        (25.0, 5 * 60 * 60)
    );
    assert_eq!(five.reset_at, Some(1_795_209_533));
    assert!(usage.one_week.is_none());
    // The summary's pool wins over its models one by one.
    assert_eq!(usage.third_party_five_hour.unwrap().used_percent, 25.0);
    assert_eq!(usage.third_party_one_week.unwrap().used_percent, 0.0);
    assert_eq!(usage.plan_type.as_deref(), Some("pro"));
}
