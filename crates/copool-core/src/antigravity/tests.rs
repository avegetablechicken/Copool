use super::*;
use crate::{account_network::Response, Preferences};
use serde_json::json;
use std::fs;
use std::sync::Mutex;

/// Antigravity's login item in memory.
#[derive(Default)]
pub(crate) struct Live {
    bytes: Mutex<Option<Vec<u8>>>,
    writes: Mutex<usize>,
    /// Writes are refused, as when the item needs authorization.
    refused: std::sync::atomic::AtomicBool,
    /// Writes take this long, as while the user is asked to allow them.
    write_delay_ms: std::sync::atomic::AtomicU64,
    /// Reads answer this late with what was there when asked, as a slow
    /// `security` does.
    read_delay_ms: std::sync::atomic::AtomicU64,
}
impl LiveStore for Live {
    fn read(&self, _: KeychainAccess) -> Response<'_, Option<Vec<u8>>> {
        let bytes = self.bytes.lock().unwrap().clone();
        let delay = self
            .read_delay_ms
            .load(std::sync::atomic::Ordering::Relaxed);
        Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            Ok(bytes)
        })
    }
    fn write(&self, bytes: Vec<u8>, _: KeychainAccess) -> Response<'_, ()> {
        if self.refused.load(std::sync::atomic::Ordering::Relaxed) {
            return Box::pin(async { Err("User interaction is not allowed.".into()) });
        }
        let delay = self
            .write_delay_ms
            .load(std::sync::atomic::Ordering::Relaxed);
        Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            *self.bytes.lock().unwrap() = Some(bytes);
            *self.writes.lock().unwrap() += 1;
            Ok(())
        })
    }
}
impl Live {
    fn sign_in(&self, document: &Value) {
        self.sign_in_as(document, true);
    }
    fn sign_in_as(&self, document: &Value, wrapped: bool) {
        *self.bytes.lock().unwrap() = Some(pool_login::antigravity::encode(&Document {
            value: document.clone(),
            wrapped,
        }));
    }
    fn writes(&self) -> usize {
        *self.writes.lock().unwrap()
    }
    fn document(&self) -> Document {
        pool_login::antigravity::decode(self.bytes.lock().unwrap().as_deref().unwrap()).unwrap()
    }
}

/// Google, answering by the access token: tokens starting with "expired"
/// are refused, and a renewal hands out "fresh-<refresh token>".
#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    /// The messages sent: project and model of each.
    messages: Mutex<Vec<(String, String)>>,
    /// Refresh tokens Google refuses.
    revoked: Mutex<Vec<String>>,
    /// A 403 reply for a specific Cloud Code method.
    denied: Mutex<Option<(&'static str, Value)>>,
    /// Remaining fraction of the Gemini five-hour pool.
    remaining: Mutex<f64>,
    /// The summary reports only the third-party pool, the Gemini models
    /// their quota one by one, as for a client Cloud Code does not show the
    /// Gemini pool.
    per_model_gemini: std::sync::atomic::AtomicBool,
    /// `loadCodeAssist` fails, as a server error.
    assist_down: std::sync::atomic::AtomicBool,
    /// Renewals rotate the refresh token, as Google may.
    rotates: std::sync::atomic::AtomicBool,
    /// Logins another app gives Antigravity's item during the first call
    /// starting with each prefix.
    meanwhile: Mutex<Vec<(String, Arc<Live>, Value)>>,
}
impl Fake {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    /// Records a call, and what another app does to the item meanwhile.
    fn call(&self, call: String) {
        let mut meanwhile = self.meanwhile.lock().unwrap();
        if let Some(index) = meanwhile.iter().position(|(p, ..)| call.starts_with(p)) {
            let (_, live, document) = meanwhile.remove(index);
            live.sign_in(&document);
        }
        self.calls.lock().unwrap().push(call);
    }
    fn meanwhile(&self, prefix: &str, live: &Arc<Live>, document: Value) {
        self.meanwhile
            .lock()
            .unwrap()
            .push((prefix.into(), live.clone(), document));
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
        body: Value,
        agent: &'a str,
        proxy: &'a str,
    ) -> Response<'a, api::Reply> {
        self.call(format!("{path} {token} {agent} {proxy}"));
        if let Some((_, error)) = self
            .denied
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(method, _)| *method == path)
        {
            let error = error.clone();
            return Box::pin(async move { Ok((403, error)) });
        }
        if path == api::GENERATE_PATH && !token.starts_with("expired") {
            self.messages.lock().unwrap().push((
                body["project"].as_str().unwrap_or_default().into(),
                body["model"].as_str().unwrap_or_default().into(),
            ));
        }
        let remaining = *self.remaining.lock().unwrap();
        let per_model = self
            .per_model_gemini
            .load(std::sync::atomic::Ordering::Relaxed);
        let assist_down = self.assist_down.load(std::sync::atomic::Ordering::Relaxed);
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
                api::LOAD_CODE_ASSIST_PATH if assist_down => (503, Value::Null),
                api::LOAD_CODE_ASSIST_PATH => (
                    200,
                    json!({"paidTier": {"name": "Google AI Pro", "availableCredits": [
                        {"creditType": "GOOGLE_ONE_AI", "creditAmount": "1000", "minimumCreditAmountForUsage": "50"}
                    ]}, "cloudaicompanionProject": "project-1"}),
                ),
                api::FETCH_MODELS_PATH => (
                    200,
                    json!({"models": {
                        "gemini-3-flash": {"displayName": "Gemini 3 Flash", "quotaInfo": {"remainingFraction": 1, "resetTime": "2026-11-20T21:18:53Z"}},
                        "gemini-3.8-flash-low": {"displayName": "Gemini 3.8 Flash (Low)", "quotaInfo": {"remainingFraction": 1, "resetTime": "2026-11-20T21:18:53Z"}},
                        "claude-sonnet-5-5-low": {"displayName": "Claude Sonnet 5.5 (Low)", "quotaInfo": {"remainingFraction": 1, "resetTime": "2026-11-20T22:59:41Z"}},
                        "gpt-oss-120b-medium": {"displayName": "GPT-OSS 120B (Medium)", "quotaInfo": {"remainingFraction": 1, "resetTime": "2026-11-20T22:59:41Z"}}
                    }, "defaultAgentModelId": "gemini-3.8-flash-low", "commandModelIds": ["gemini-3-flash"]}),
                ),
                api::GENERATE_PATH => (200, json!({"response": {"candidates": []}})),
                _ => (404, Value::Null),
            })
        })
    }
    fn renew<'a>(&'a self, refresh_token: &'a str, proxy: &'a str) -> Response<'a, api::Reply> {
        self.call(format!("renew {refresh_token} {proxy}"));
        let revoked = self
            .revoked
            .lock()
            .unwrap()
            .contains(&refresh_token.to_owned());
        let rotates = self.rotates.load(std::sync::atomic::Ordering::Relaxed);
        Box::pin(async move {
            Ok(if revoked {
                (400, json!({"error": "invalid_grant"}))
            } else {
                let mut body =
                    json!({"access_token": format!("fresh-{refresh_token}"), "expires_in": 3599});
                if rotates {
                    body["refresh_token"] = format!("rotated-{refresh_token}").into();
                }
                (200, body)
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
/// The Antigravity pool as the store's suite drives it.
struct Fixture(Harness);
impl crate::login_store::suite::Fixture for Fixture {
    const POOL: Pool = Pool::Antigravity;
    const LIVE_UNREADABLE: &'static str = <AntigravityLogins as LoginPool>::LIVE_UNREADABLE;
    async fn open(request_new_accounts: bool) -> Self {
        Self(harness(request_new_accounts).await)
    }
    fn app(&self) -> &Application {
        &self.0.app
    }
    fn login(user: &str, access: &str, expires: i64) -> Value {
        login_of(user, access, expires)
    }
    fn with_refresh(login: &Value, refresh: &str) -> Value {
        let mut login = login.clone();
        login["token"]["refresh_token"] = refresh.into();
        login
    }
    fn without_expiry(login: &Value) -> Value {
        let mut login = login.clone();
        login["token"].as_object_mut().unwrap().remove("expiry");
        login
    }
    fn no_login() -> Value {
        json!({"token": {"access_token": "a"}})
    }
    fn access(document: &Value) -> Option<String> {
        pool_login::antigravity::access_token(document)
    }
    fn renewed(user: &str) -> String {
        format!("fresh-refresh-{user}")
    }
    fn sign_in(&self, login: &Value) {
        self.0.live.sign_in(login);
    }
    fn live(&self) -> Value {
        self.0.live.document().value
    }
    fn corrupt_live(&self) {
        *self.0.live.bytes.lock().unwrap() = Some(b"not a login".to_vec());
    }
    fn delay_live_reads(&self, ms: u64) {
        self.0
            .live
            .read_delay_ms
            .store(ms, std::sync::atomic::Ordering::Relaxed);
    }
    fn meanwhile(&self, login: Value) {
        self.0.fake.meanwhile("renew", &self.0.live, login);
    }
    fn renewals(&self) -> usize {
        self.0.fake.renewals()
    }
    /// The account whose refresh token, renewed or not, is `user`'s.
    fn id_of(&self, user: &str) -> String {
        let refresh = format!("refresh-{user}");
        let store = self.0.app.antigravity_load().unwrap();
        store
            .accounts
            .into_iter()
            .find(|a| {
                pool_login::antigravity::refresh_token(&a.document)
                    .is_some_and(|r| r.ends_with(refresh.as_str()))
            })
            .unwrap()
            .id
    }
    fn stored(&self, id: &str) -> Value {
        stored_document(&self.0.app, id)
    }
    fn current(&self) -> Option<String> {
        self.0.app.antigravity_load().unwrap().current_account_id
    }
}
crate::login_store::login_store_suite!(super::Fixture);

/// agy signed in again to the same Google account: another refresh token,
/// the same email. Its card stays the one current and takes the login.
#[tokio::test]
async fn a_new_login_of_the_same_google_account_is_followed() {
    let h = harness(true).await;
    let named = |access: &str, refresh: &str, expires: i64| {
        let mut login = login_of("alice", access, expires);
        login["token"]["refresh_token"] = refresh.into();
        login["id_token_email"] = "alice@gmail.com".into();
        login
    };
    h.live
        .sign_in(&named("access-alice", "refresh-alice", later()));
    h.app.import_current(Pool::Antigravity).await.unwrap();
    let again = named("access-alice-again", "refresh-alice-again", later() + 60);
    h.live.sign_in(&again);
    let accounts = cards(&h.app).await;
    assert_eq!(accounts.len(), 1);
    assert!(accounts[0].is_current);
    assert_eq!(stored_document(&h.app, &accounts[0].id), again);
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
    let credits = usage.credits.unwrap();
    assert_eq!(credits.balance.as_deref(), Some("1000"));
    assert!(credits.has_credits);
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
    // A refresh that renews nothing writes nothing to Antigravity's login.
    assert_eq!(h.live.writes(), 0);
}

#[tokio::test]
async fn a_failed_code_assist_keeps_the_last_credits() {
    use std::sync::atomic::Ordering::Relaxed;
    let h = harness(false).await;
    h.app
        .import_json(
            Pool::Antigravity,
            &login_of("alice", "access-alice", later()).to_string(),
        )
        .await
        .unwrap();
    let id = cards(&h.app).await.remove(0).id;
    let balance = || async {
        cards(&h.app)
            .await
            .remove(0)
            .usage
            .and_then(|u| u.credits)
            .and_then(|c| c.balance)
    };
    h.app
        .refresh_now(Pool::Antigravity, Some(&id))
        .await
        .unwrap();
    assert_eq!(balance().await.as_deref(), Some("1000"));
    // The quota refreshes; the credits it could not read stay.
    h.fake.assist_down.store(true, Relaxed);
    *h.fake.remaining.lock().unwrap() = 0.5;
    h.app
        .refresh_now(Pool::Antigravity, Some(&id))
        .await
        .unwrap();
    let card = cards(&h.app).await.remove(0);
    assert_eq!(card.usage.unwrap().five_hour.unwrap().used_percent, 50.0);
    assert_eq!(card.plan_type.as_deref(), Some("pro"));
    assert_eq!(balance().await.as_deref(), Some("1000"));
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
        pool_login::antigravity::access_token(document).as_deref(),
        Some("fresh-refresh-alice")
    );
    assert!(pool_login::antigravity::expiry(document).unwrap() > now_unix_seconds() + 3000);

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
        let _lock = h.app.antigravity.logins.store.lock().await;
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

    // A revoked login says the sign-in expired.
    h.fake.revoked.lock().unwrap().push("refresh-alice".into());
    {
        let _lock = h.app.antigravity.logins.store.lock().await;
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
async fn quota_permission_errors_reach_the_card_without_consuming_a_renewal() {
    let h = harness(true).await;
    h.live.sign_in(&login_of("alice", "access-alice", later()));
    h.app.import_current(Pool::Antigravity).await.unwrap();
    let card = cards(&h.app).await.remove(0);
    h.app
        .set_renewal(Pool::Antigravity, &card.id, "always")
        .await
        .unwrap();
    let body = json!({"error": {
        "status": "PERMISSION_DENIED",
        "message": "You do not have a valid license of this product. (#3501)",
        "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "SUBSCRIPTION_REQUIRED"}]
    }});
    for method in [api::QUOTA_SUMMARY_PATH, api::FETCH_MODELS_PATH] {
        h.fake
            .per_model_gemini
            .store(true, std::sync::atomic::Ordering::Relaxed);
        *h.fake.denied.lock().unwrap() = Some((method, body.clone()));
        let start = h.fake.calls().len();
        let error = h
            .app
            .refresh_usage(Pool::Antigravity, &card.id)
            .await
            .unwrap_err();
        assert!(error.starts_with("error.antigravity.quota_license_format"));
        assert!(error.contains("HTTP 403 PERMISSION_DENIED SUBSCRIPTION_REQUIRED"));
        assert!(error.contains("(#3501)"));
        let calls = h.fake.calls();
        assert!(calls[start..].last().unwrap().starts_with(method));
        assert_eq!(h.fake.renewals(), 0);
        let updated = cards(&h.app).await.remove(0);
        assert_eq!(updated.usage_error.as_deref(), Some(error.as_str()));
        assert_eq!(updated.renewal.as_deref(), Some("always"));
        assert!(updated.usage.is_some());
    }
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
    // The cards are read while the login is written, as while the user is
    // asked to allow it.
    h.live
        .write_delay_ms
        .store(300, std::sync::atomic::Ordering::Relaxed);
    let (switched, read) =
        tokio::join!(h.app.switch_account(Pool::Antigravity, &alice.id), async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            tokio::time::timeout(std::time::Duration::from_millis(150), cards(&h.app))
                .await
                .is_ok()
        });
    switched.unwrap();
    assert!(read);
    h.live
        .write_delay_ms
        .store(0, std::sync::atomic::Ordering::Relaxed);
    let written = h.live.document();
    assert!(written.wrapped);
    assert_eq!(
        pool_login::antigravity::refresh_token(&written.value).as_deref(),
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
        pool_login::antigravity::access_token(&stored.document).as_deref(),
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
        pool_login::antigravity::refresh_token(&h.live.document().value).as_deref(),
        Some("refresh-bob")
    );
    // A pool login is not backed up again.
    assert_eq!(*h.live.writes.lock().unwrap(), 2);
}

/// The login agy's login file holds.
fn token_file(app: &Application) -> Option<Document> {
    pool_login::antigravity::decode(&fs::read(app.antigravity_token_file()).ok()?)
}

#[tokio::test]
async fn agys_login_file_takes_each_switch_and_its_later_logins_are_followed() {
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
    let id = |user: &str| {
        h.app
            .antigravity_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| a.email.as_deref() == Some(format!("{user}@gmail.com").as_str()))
            .unwrap()
            .id
    };
    h.live.sign_in(&login_of("alice", "access-alice", later()));
    // Before agy has run here, no file is made.
    h.app
        .switch_account(Pool::Antigravity, &id("bob"))
        .await
        .unwrap();
    assert!(token_file(&h.app).is_none());

    // Once agy has run, Linux and Windows create its login file. macOS
    // keeps using the Keychain unless agy already left a login file.
    fs::create_dir_all(h.app.antigravity_token_file().parent().unwrap()).unwrap();
    h.app
        .switch_account(Pool::Antigravity, &id("alice"))
        .await
        .unwrap();
    assert_eq!(
        h.app.antigravity_token_file().exists(),
        !cfg!(target_os = "macos")
    );
    if cfg!(target_os = "macos") {
        fs::write(
            h.app.antigravity_token_file(),
            stored_document(&h.app, &id("alice")).to_string(),
        )
        .unwrap();
    }

    // An existing file takes each switch on every platform.
    for user in ["bob", "alice"] {
        h.app
            .switch_account(Pool::Antigravity, &id(user))
            .await
            .unwrap();
        assert_eq!(
            pool_login::antigravity::refresh_token(&token_file(&h.app).unwrap().value).as_deref(),
            Some(format!("refresh-{user}").as_str())
        );
    }
    let file = token_file(&h.app).unwrap();
    assert!(!file.wrapped);
    assert_eq!(
        pool_login::antigravity::refresh_token(&file.value).as_deref(),
        Some("refresh-alice")
    );
    assert!(h.live.document().wrapped);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(h.app.antigravity_token_file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // agy over SSH renews there, then signs in to bob there: both followed.
    let write_file =
        |document: &Value| fs::write(h.app.antigravity_token_file(), document.to_string()).unwrap();
    write_file(&login_of("alice", "renewed-over-ssh", later() + 60));
    let shown = cards(&h.app).await;
    let alice = shown.iter().find(|a| a.id == id("alice")).unwrap();
    assert!(alice.is_current);
    assert_eq!(
        pool_login::antigravity::access_token(&stored_document(&h.app, &alice.id)).as_deref(),
        Some("renewed-over-ssh")
    );
    write_file(&login_of("bob", "bob-over-ssh", later() + 120));
    let current: Vec<_> = cards(&h.app)
        .await
        .into_iter()
        .filter(|a| a.is_current)
        .map(|a| a.id)
        .collect();
    assert_eq!(current, [id("bob")]);
    // Taken from the file, the login keeps the item's form.
    let bob = h.app.antigravity_load().unwrap().accounts;
    assert!(bob.iter().find(|a| a.id == id("bob")).unwrap().own.wrapped);

    // A login the item refuses leaves the file as it was.
    let before = fs::read(h.app.antigravity_token_file()).unwrap();
    h.live
        .refused
        .store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(h
        .app
        .switch_account(Pool::Antigravity, &id("alice"))
        .await
        .is_err());
    assert_eq!(fs::read(h.app.antigravity_token_file()).unwrap(), before);
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
    let wrapped = String::from_utf8(pool_login::antigravity::encode(&Document {
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
async fn the_empty_pool_has_no_proxy_candidates_or_providers() {
    let h = harness(true).await;
    let proxy = h.app.proxy_status(Pool::Antigravity).await.unwrap();
    assert!(!proxy.running);
    assert_eq!(proxy.available_accounts, 0);
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

#[tokio::test]
async fn import_time_requests_take_the_shared_account_proxy() {
    let h = harness(true).await;
    let preferences = Preferences {
        account_proxy_url: "http://127.0.0.1:7890".into(),
        ..h.app.preferences().unwrap()
    };
    h.app.save_preferences(preferences).await.unwrap();
    h.live
        .sign_in(&login_of("alice", "access-alice", now_unix_seconds() - 10));
    h.app.import_current(Pool::Antigravity).await.unwrap();
    // The renewal and every quota request go through the shared proxy, which
    // the new account does not keep as its own.
    let calls = h.fake.calls();
    assert!(calls[0].starts_with("renew refresh-alice http://127.0.0.1:7890"));
    assert!(calls
        .iter()
        .filter(|c| c.starts_with("/v1internal"))
        .all(|c| c.ends_with(" http://127.0.0.1:7890")));
    let card = cards(&h.app).await.remove(0);
    assert!(card.usage.is_some() && card.proxy_url.is_empty());
}

#[tokio::test]
async fn a_browser_sign_in_joins_the_pool_without_switching() {
    let h = harness(false).await;
    let mut document = login_of("alice", "access-alice", later());
    document["auth_method"] = "consumer".into();
    document["id_token_email"] = "alice@gmail.com".into();
    assert_eq!(
        h.app
            .import_sign_in(Pool::Antigravity, document.clone(), None)
            .await
            .unwrap(),
        1
    );
    // Named by its ID token while it waits; nothing goes upstream.
    let card = cards(&h.app).await.remove(0);
    assert_eq!(card.label, "alice@gmail.com");
    assert!(card.awaiting_first_request && !card.is_current);
    assert!(h.fake.calls().is_empty());
    assert_eq!(*h.live.writes.lock().unwrap(), 0);
    // Kept as Antigravity keeps its login, for a later switch.
    h.app
        .switch_account(Pool::Antigravity, &card.id)
        .await
        .unwrap();
    let written = h.live.document();
    assert_eq!(written.value, document);
    assert_eq!(written.wrapped, cfg!(target_os = "macos"));
    // A sign-in without a refresh token is refused.
    let mut broken = document;
    broken["token"]
        .as_object_mut()
        .unwrap()
        .remove("refresh_token");
    assert!(h
        .app
        .import_sign_in(Pool::Antigravity, broken, None)
        .await
        .is_err());
}

#[tokio::test]
async fn a_hi_starts_each_idle_five_hour_window() {
    let h = harness(true).await;
    h.app
        .import_json(
            Pool::Antigravity,
            &login_of("alice", "access-alice", later()).to_string(),
        )
        .await
        .unwrap();
    let id = cards(&h.app).await[0].id.clone();
    let set_usage = |gemini: Option<i64>, claude: Option<i64>| {
        let _lock =
            futures_util::FutureExt::now_or_never(h.app.antigravity.logins.store.lock()).unwrap();
        let mut store = h.app.antigravity_load().unwrap();
        let usage = store.accounts[0].usage.as_mut().unwrap();
        usage.five_hour.as_mut().unwrap().reset_at = gemini;
        usage.third_party_five_hour.as_mut().unwrap().reset_at = claude;
        h.app.antigravity_save(&store).unwrap();
    };
    let later_reset = now_unix_seconds() + 3600;
    // Gemini's window runs; Claude's is idle: one hi, to the cheapest
    // model of Claude's pool, in the project the refresh learned.
    set_usage(Some(later_reset), None);
    h.app.say_hi(Pool::Antigravity, &id).await.unwrap();
    assert_eq!(
        *h.fake.messages.lock().unwrap(),
        [("project-1".to_owned(), "gpt-oss-120b-medium".to_owned())]
    );
    // Both idle (one reset past): a hi to each pool.
    h.fake.messages.lock().unwrap().clear();
    set_usage(Some(now_unix_seconds() - 10), None);
    h.app.say_hi(Pool::Antigravity, &id).await.unwrap();
    let models: Vec<String> = h
        .fake
        .messages
        .lock()
        .unwrap()
        .iter()
        .map(|m| m.1.clone())
        .collect();
    assert_eq!(models, ["gemini-3.8-flash-low", "gpt-oss-120b-medium"]);
    // Both running: Gemini's, as asked.
    h.fake.messages.lock().unwrap().clear();
    set_usage(Some(later_reset), Some(later_reset));
    h.app.say_hi(Pool::Antigravity, &id).await.unwrap();
    assert_eq!(h.fake.messages.lock().unwrap().len(), 1);
    assert_eq!(h.fake.messages.lock().unwrap()[0].1, "gemini-3.8-flash-low");
    // A refused token is renewed by a refresh, then the hi goes again.
    h.fake.messages.lock().unwrap().clear();
    {
        let _lock = h.app.antigravity.logins.store.lock().await;
        let mut store = h.app.antigravity_load().unwrap();
        store.accounts[0].document = login_of("alice", "expired-alice", later());
        h.app.antigravity_save(&store).unwrap();
    }
    let renewals = h.fake.renewals();
    h.app.say_hi(Pool::Antigravity, &id).await.unwrap();
    assert_eq!(h.fake.renewals(), renewals + 1);
    // The refresh read both windows idle (Gemini's reset past, Claude's
    // none): a hi to each, with the renewed token.
    let models: Vec<String> = h
        .fake
        .messages
        .lock()
        .unwrap()
        .iter()
        .map(|m| m.1.clone())
        .collect();
    assert_eq!(models, ["gemini-3.8-flash-low", "gpt-oss-120b-medium"]);
    // The card's model switch chooses one pool alone, idle or not.
    for (third_party, model) in [
        (true, "gpt-oss-120b-medium"),
        (false, "gemini-3.8-flash-low"),
    ] {
        for (gemini, claude) in [(None, None), (Some(later_reset), Some(later_reset))] {
            h.fake.messages.lock().unwrap().clear();
            set_usage(gemini, claude);
            h.app
                .say_hi_to(Pool::Antigravity, &id, Some(third_party))
                .await
                .unwrap();
            let models: Vec<String> = h
                .fake
                .messages
                .lock()
                .unwrap()
                .iter()
                .map(|m| m.1.clone())
                .collect();
            assert_eq!(models, [model]);
        }
    }
    // Other pools have no third-party pool to greet.
    assert!(h.app.say_hi_to(Pool::Codex, &id, Some(true)).await.is_err());
}

fn stored_document(app: &Application, id: &str) -> Value {
    let store = app.antigravity_load().unwrap();
    store
        .accounts
        .into_iter()
        .find(|a| a.id == id)
        .unwrap()
        .document
}

#[tokio::test]
async fn renewing_the_login_antigravity_uses_writes_it_back_in_its_form() {
    for wrapped in [true, false] {
        let h = harness(false).await;
        // Antigravity's login as agy keeps it, its access token expired.
        let mut item = login_of("alice", "access-alice", now_unix_seconds() - 10);
        item["token"]["expires_in"] = 3599.into();
        item["auth_method"] = "consumer".into();
        item["id_token_email"] = "alice@gmail.com".into();
        h.live.sign_in_as(&item, wrapped);
        h.app.import_current(Pool::Antigravity).await.unwrap();
        let id = cards(&h.app).await[0].id.clone();
        // A field of the item that Copool's copy lacks.
        item["agy_setting"] = json!({"kept": true});
        h.live.sign_in_as(&item, wrapped);

        h.app
            .refresh_now(Pool::Antigravity, Some(&id))
            .await
            .unwrap();
        assert_eq!(h.fake.renewals(), 1);
        assert_eq!(h.live.writes(), 1);
        let stored = stored_document(&h.app, &id);
        assert_eq!(
            pool_login::antigravity::access_token(&stored).as_deref(),
            Some("fresh-refresh-alice")
        );
        let written = h.live.document();
        assert_eq!(written.wrapped, wrapped);
        // The item's own keys, order and fields; only the tokens are new.
        let mut expected = item.clone();
        expected["token"]["access_token"] = "fresh-refresh-alice".into();
        expected["token"]["expiry"] = written.value["token"]["expiry"].clone();
        assert_eq!(written.value.to_string(), expected.to_string());
        assert!(written.value["token"]["expiry"].is_string());
        assert_eq!(
            pool_login::antigravity::expiry(&written.value),
            pool_login::antigravity::expiry(&stored)
        );
        assert!(pool_login::antigravity::expiry(&stored).unwrap() > now_unix_seconds() + 3000);
        let card = cards(&h.app).await.remove(0);
        assert!(card.is_current && card.usage.is_some() && card.usage_error.is_none());
    }
}

#[tokio::test]
async fn a_renewal_that_rotates_the_refresh_token_is_written_back_too() {
    let h = harness(false).await;
    h.live
        .sign_in(&login_of("alice", "access-alice", now_unix_seconds() - 10));
    h.app.import_current(Pool::Antigravity).await.unwrap();
    let id = cards(&h.app).await[0].id.clone();
    h.fake
        .rotates
        .store(true, std::sync::atomic::Ordering::Relaxed);
    h.app
        .refresh_now(Pool::Antigravity, Some(&id))
        .await
        .unwrap();
    assert_eq!(h.fake.renewals(), 1);
    // Antigravity's login gets the new refresh token, and stays the
    // card in use.
    assert_eq!(h.live.writes(), 1);
    let written = h.live.document().value;
    assert_eq!(
        pool_login::antigravity::refresh_token(&written).as_deref(),
        Some("rotated-refresh-alice")
    );
    assert_eq!(written, stored_document(&h.app, &id));
    assert!(cards(&h.app).await[0].is_current);
}

#[tokio::test]
async fn a_refused_write_back_does_not_fail_the_refresh() {
    let h = harness(true).await;
    let expired = login_of("alice", "access-alice", now_unix_seconds() - 10);
    h.live.sign_in(&expired);
    h.live
        .refused
        .store(true, std::sync::atomic::Ordering::Relaxed);
    h.app.import_current(Pool::Antigravity).await.unwrap();
    let card = cards(&h.app).await.remove(0);
    assert!(card.usage.is_some() && card.usage_error.is_none());
    assert_eq!(
        pool_login::antigravity::access_token(&stored_document(&h.app, &card.id)).as_deref(),
        Some("fresh-refresh-alice")
    );
    // Renew now succeeds too; Antigravity's login stays as it was.
    h.app.renew_now(Pool::Antigravity, &card.id).await.unwrap();
    assert_eq!(h.fake.renewals(), 2);
    assert_eq!(h.live.writes(), 0);
    assert_eq!(h.live.document().value, expired);
    let card = cards(&h.app).await.remove(0);
    assert!(card.usage_error.is_none());
}

#[test]
fn antigravitys_item_is_reached_only_through_the_security_tool() {
    // No in-process Keychain call for the item: its partition list would
    // become Copool's, and agy's next access would ask for approval.
    for (file, source) in [
        ("antigravity.rs", include_str!("../antigravity.rs")),
        ("login.rs", include_str!("login.rs")),
        ("security.rs", include_str!("security.rs")),
    ] {
        for call in [
            "claude::keychain",
            "security_framework",
            "SecItem",
            "SecKeychain",
            "passwords::",
        ] {
            assert!(!source.contains(call), "{file} uses {call}");
        }
    }
}

/// A Sub2api admin API with Antigravity accounts, recording request paths:
/// one in use, and a Gemini CLI account the pool must not show. It reports
/// `version`, and refuses to without one.
async fn sub2api_server(
    requests: Arc<Mutex<Vec<String>>>,
    version: Arc<Mutex<Option<&'static str>>>,
) -> String {
    use axum::{extract::Request, response::IntoResponse, routing::any, Json, Router};
    let router = Router::new().fallback(any(move |request: Request| {
        let requests = requests.clone();
        let version = *version.lock().unwrap();
        async move {
            let uri = request.uri().path_and_query().unwrap().to_string();
            requests.lock().unwrap().push(uri.clone());
            if !uri.starts_with("/api/v1/auth/login") {
                assert_eq!(
                    request.headers().get("authorization").unwrap(),
                    "Bearer admin"
                );
            }
            let data = if uri.starts_with("/api/v1/auth/login") {
                json!({"access_token": "admin"})
            } else if uri == "/api/v1/admin/system/version" {
                match version {
                    Some(version) => json!({"version": version}),
                    None => {
                        return Json(json!({"code": 500, "message": "unavailable"}))
                            .into_response()
                    }
                }
            } else if uri.starts_with("/api/v1/admin/accounts?") {
                json!({"items": [
                    {"id": 7, "name": "relay-alias", "platform": "antigravity", "type": "oauth", "status": "active",
                     "extra": {"email": " "}, "credentials": {"email": "pro@gmail.com", "plan_type": "g1-pro-tier"}},
                    {"id": 8, "name": "cli@gmail.com", "platform": "gemini", "type": "oauth", "status": "active"}
                ], "pages": 1})
            } else {
                json!({"antigravity_quota_groups": [
                    {"key": "gemini", "windows": [
                        {"window": "5h", "utilization": 25, "resets_at": "2099-01-01T00:00:00Z"},
                        {"window": "weekly", "utilization": 10, "resets_at": "2099-01-05T00:00:00Z"}]},
                    {"key": "3p", "windows": [
                        {"window": "5h", "utilization": 50, "resets_at": "2099-01-01T00:00:00Z"}]}
                ]})
            };
            Json(json!({"code": 0, "data": data})).into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}

// A Sub2api before 0.2.20 reports no weekly windows: its cards name its
// version, kept while Sub2api does not say, until an upgrade reports them.
#[tokio::test]
async fn a_card_of_a_sub2api_without_weekly_usage_names_its_version() {
    let h = harness(true).await;
    let version = Arc::new(Mutex::new(Some("0.1.177")));
    let base = sub2api_server(Arc::new(Mutex::new(Vec::new())), version.clone()).await;
    let saved = h
        .app
        .save_provider(
            Pool::Antigravity,
            json!({"name": "Relay", "baseURL": base,
                   "username": "admin@example.com", "password": "secret"}),
        )
        .await
        .unwrap();
    let id = saved["id"].as_str().unwrap().to_owned();
    let outdated = |accounts: Vec<AccountSummary>| {
        accounts
            .into_iter()
            .find(|a| crate::sub2api::is_card(Pool::Antigravity, &a.id))
            .unwrap()
            .outdated_sub2api_version
    };
    for (reported, shown, sync) in [
        (Some("0.1.177"), Some("0.1.177"), true),
        (None, Some("0.1.177"), false),
        (Some("v0.2.20"), None, false),
    ] {
        *version.lock().unwrap() = reported;
        h.app
            .refresh_provider(Pool::Antigravity, &id, sync)
            .await
            .unwrap();
        assert_eq!(
            outdated(cards(&h.app).await).as_deref(),
            shown,
            "{reported:?}"
        );
    }
}

fn read_json(path: &std::path::Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

// agy's settings, environment and the Sub2api server are all temporary: no
// real settings, shell, Keychain item or relay is used.
#[tokio::test]
async fn a_sub2api_card_puts_agy_on_the_gemini_api_and_an_account_takes_it_back() {
    let h = harness(true).await;
    let settings = h._dir.path().join("agy/settings.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let original = "{\n  \"trustedWorkspaces\": [\n    \"/w\"\n  ]\n}\n";
    fs::write(&settings, original).unwrap();
    h.app.set_antigravity_settings(settings.clone());
    // Gemini's `.env` and the shell's startup file, beside agy's settings.
    let env = h._dir.path().join(".env");
    let env_original = "# mine\nexport GOOGLE_GEMINI_BASE_URL=\"https://old.invalid\"\n";
    fs::write(&env, env_original).unwrap();
    let rc = h._dir.path().join("zshrc");
    let rc_original = "alias ll='ls -l'";
    fs::write(&rc, rc_original).unwrap();
    h.app.set_antigravity_shell_rc(rc.clone());
    // A fake shell: its profile's exports, then the `.env` in order, which
    // its startup file loads itself ("loads"), once Copool has it ("not"),
    // or before another gateway is exported over it ("broken").
    type Exports = std::collections::BTreeMap<String, String>;
    let profile = Arc::new(Mutex::new(Exports::new()));
    let shell = Arc::new(Mutex::new("broken"));
    let (exports, mode) = (profile.clone(), shell.clone());
    let (env_file, rc_file) = (env.clone(), rc.clone());
    // The provider's address, edited while the shell is asked.
    let moved = Arc::new(Mutex::new(None::<String>));
    let (edit, store_file) = (
        moved.clone(),
        h.app.paths.data_dir.join("antigravity-accounts.json"),
    );
    h.app
        .set_antigravity_environment(Arc::new(move |names: &[&str], _now: bool| {
            if let Some(url) = edit.lock().unwrap().as_deref() {
                let mut store = read_json(&store_file);
                store["providers"][0]["baseURL"] = url.into();
                fs::write(&store_file, store.to_string()).unwrap();
            }
            let mut vars = exports.lock().unwrap().clone();
            let mode = *mode.lock().unwrap();
            let loads = mode != "not"
                || fs::read_to_string(&rc_file)
                    .unwrap_or_default()
                    .contains(&format!(". '{}'", env_file.display()));
            let text = fs::read_to_string(&env_file).unwrap_or_default();
            for line in text.lines().filter(|line| loads && !line.starts_with('#')) {
                let line = line.strip_prefix("export ").unwrap_or(line);
                let Some((name, value)) = line.split_once('=') else {
                    continue;
                };
                let value = match value.strip_prefix('\'') {
                    Some(literal) => literal.trim_end_matches('\'').to_owned(),
                    None => {
                        let value = value.trim_matches('"');
                        match value.strip_prefix("${").and_then(|v| v.strip_suffix('}')) {
                            Some(other) => vars.get(other).cloned().unwrap_or_default(),
                            None => value.to_owned(),
                        }
                    }
                };
                vars.insert(name.to_owned(), value);
            }
            if mode == "broken" {
                vars.insert(sub2api::BASE_URL.into(), "https://elsewhere.invalid".into());
            }
            names
                .iter()
                .map(|name| vars.get(*name).filter(|v| !v.is_empty()).cloned())
                .collect()
        }));
    let export = |name: &str, value: &str| {
        profile
            .lock()
            .unwrap()
            .insert(name.to_owned(), value.to_owned());
    };
    let requests = Arc::new(Mutex::new(Vec::new()));
    let base = sub2api_server(requests.clone(), Arc::new(Mutex::new(Some("0.2.24")))).await;
    // The address agy's gateway names works as well as the root.
    let saved = h
        .app
        .save_provider(
            Pool::Antigravity,
            json!({"name": "Relay", "baseURL": format!("{base}/antigravity/"),
                   "username": "admin@example.com", "password": "secret"}),
        )
        .await
        .unwrap();
    assert_eq!(saved["baseURL"], base);
    assert!(saved.get("password").is_none());
    // agy's key is GEMINI_API_KEY's unless a variable is named; a key
    // itself is refused, never kept.
    assert_eq!(saved["apiKeyVariable"], sub2api::API_KEY);
    let id = saved["id"].as_str().unwrap().to_owned();
    let edit = |variable: &str, url: &str| {
        json!({"id": id, "name": "Relay", "baseURL": url,
               "username": "admin@example.com", "apiKeyVariable": variable})
    };
    assert_eq!(
        h.app
            .save_provider(Pool::Antigravity, edit("sk-relay", &base))
            .await
            .unwrap_err(),
        sub2api::API_KEY_VARIABLE_INVALID
    );
    assert_eq!(
        h.app.providers(Pool::Antigravity).unwrap()[0]["name"],
        "Relay"
    );
    assert_eq!(
        h.app
            .refresh_provider(Pool::Antigravity, &id, true)
            .await
            .unwrap(),
        1
    );
    assert!(requests
        .lock()
        .unwrap()
        .iter()
        .any(|uri| uri.contains("platform=antigravity")));

    // An account beside the provider's card, in use.
    h.app
        .import_json(
            Pool::Antigravity,
            &json!({"document": login_of("alice", "access-alice", later()), "wrapped": true})
                .to_string(),
        )
        .await
        .unwrap();
    let accounts = cards(&h.app).await;
    assert_eq!(accounts.len(), 2);
    let is_card = |a: &&AccountSummary| crate::sub2api::is_card(Pool::Antigravity, &a.id);
    let card = accounts.iter().find(is_card).unwrap().clone();
    let alice = accounts.iter().find(|a| !is_card(a)).unwrap().clone();
    assert_eq!(card.source_tag.as_deref(), Some("SUB2API"));
    assert_eq!(card.outdated_sub2api_version, None);
    assert_eq!(card.email.as_deref(), Some("pro@gmail.com"));
    assert_eq!(card.label, "pro@gmail.com");
    assert_eq!(card.plan_type.as_deref(), Some("pro"));
    let usage = card.usage.as_ref().unwrap();
    assert_eq!(usage.plan_type, card.plan_type);
    assert_eq!(usage.five_hour.as_ref().unwrap().used_percent, 25.0);
    assert_eq!(usage.one_week.as_ref().unwrap().used_percent, 10.0);
    assert_eq!(
        usage.third_party_five_hour.as_ref().unwrap().used_percent,
        50.0
    );
    assert!(!card.is_current);
    h.app
        .switch_account(Pool::Antigravity, &alice.id)
        .await
        .unwrap();
    // The account's switch had nothing to take back.
    assert_eq!(fs::read_to_string(&settings).unwrap(), original);
    let writes = h.live.writes();

    // Without agy's key, with a blank one, or with a shell that has another
    // gateway even after loading the `.env` last, nothing changes.
    for (exported, error) in [
        (None, sub2api::API_KEY_MISSING),
        (Some("  "), sub2api::API_KEY_MISSING),
        (Some("sk-profile"), sub2api::ENV_NOT_LOADED),
    ] {
        if let Some(key) = exported {
            export(sub2api::API_KEY, key);
        }
        assert_eq!(
            h.app
                .switch_account(Pool::Antigravity, &card.id)
                .await
                .unwrap_err(),
            error
        );
        assert_eq!(fs::read_to_string(&settings).unwrap(), original);
        assert_eq!(fs::read_to_string(&env).unwrap(), env_original);
        assert_eq!(fs::read_to_string(&rc).unwrap(), rc_original);
    }
    // A shell that does not load the `.env` gets it loaded, last.
    *shell.lock().unwrap() = "not";
    let execution = h
        .app
        .switch_account(Pool::Antigravity, &card.id)
        .await
        .unwrap();
    assert_eq!(execution.provider.as_deref(), Some("Relay"));
    assert_eq!(
        read_json(&settings),
        json!({"trustedWorkspaces": ["/w"], "modelProvider": "gemini"})
    );
    // The user's own lines stay; Copool's block after them wins.
    let block = |key: &str, url: &str| {
        format!(
            "{env_original}\n# >>> Copool: agy's Sub2api provider (rewritten on each switch) >>>\n\
             {key}GOOGLE_GEMINI_BASE_URL='{url}/antigravity'\n# <<< Copool <<<\n"
        )
    };
    assert_eq!(fs::read_to_string(&env).unwrap(), block("", &base));
    let rc_loaded = fs::read_to_string(&rc).unwrap();
    assert!(
        rc_loaded.starts_with("alias ll='ls -l'\n\n# Copool:"),
        "{rc_loaded}"
    );
    // Once loaded, another switch adds nothing more.
    h.app
        .switch_account(Pool::Antigravity, &card.id)
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(&rc).unwrap(), rc_loaded);
    // Through Coport, the address is Coport's with Sub2api's in its path,
    // as Coport routes an upstream URL; the admin API is still Sub2api's,
    // reached directly. Another variable's value is agy's key, once a shell
    // exports it.
    let coport = format!("http://127.0.0.1:8787/antigravity/{base}");
    let saved = h
        .app
        .save_provider(
            Pool::Antigravity,
            edit("RELAY_KEY", &format!("{coport}/antigravity/")),
        )
        .await
        .unwrap();
    assert_eq!(saved["baseURL"], coport);
    requests.lock().unwrap().clear();
    assert_eq!(
        h.app
            .refresh_provider(Pool::Antigravity, &id, true)
            .await
            .unwrap(),
        1
    );
    assert!(!requests.lock().unwrap().is_empty());
    assert_eq!(
        h.app
            .switch_account(Pool::Antigravity, &card.id)
            .await
            .unwrap_err(),
        sub2api::API_KEY_MISSING
    );
    assert_eq!(fs::read_to_string(&env).unwrap(), block("", &base));
    export("RELAY_KEY", "sk-relay");
    h.app
        .switch_account(Pool::Antigravity, &card.id)
        .await
        .unwrap();
    let applied = block("GEMINI_API_KEY=\"${RELAY_KEY}\"\n", &coport);
    assert_eq!(fs::read_to_string(&env).unwrap(), applied);
    // The login stays in the Keychain, for an account's switch.
    assert_eq!(h.live.writes(), writes);
    for account in cards(&h.app).await {
        assert_eq!(account.is_current, account.id == card.id, "{}", account.id);
    }
    // agy's own edits while on the provider stay.
    fs::write(
        &settings,
        "{\"trustedWorkspaces\":[\"/w\",\"/x\"],\"modelProvider\":\"gemini\"}",
    )
    .unwrap();

    // A Keychain that refuses the login leaves agy on the provider.
    h.live
        .refused
        .store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(h
        .app
        .switch_account(Pool::Antigravity, &alice.id)
        .await
        .is_err());
    assert_eq!(read_json(&settings)["modelProvider"], "gemini");
    assert_eq!(fs::read_to_string(&env).unwrap(), applied);
    assert!(cards(&h.app)
        .await
        .iter()
        .any(|a| a.id == card.id && a.is_current));
    h.live
        .refused
        .store(false, std::sync::atomic::Ordering::Relaxed);

    h.app
        .switch_account(Pool::Antigravity, &alice.id)
        .await
        .unwrap();
    assert_eq!(
        read_json(&settings),
        json!({"trustedWorkspaces": ["/w", "/x"]})
    );
    // The user's `.env` is theirs again; the startup file keeps loading it.
    assert_eq!(fs::read_to_string(&env).unwrap(), env_original);
    assert_eq!(fs::read_to_string(&rc).unwrap(), rc_loaded);
    assert_eq!(
        pool_login::antigravity::refresh_token(&h.live.document().value).as_deref(),
        Some("refresh-alice")
    );
    for account in cards(&h.app).await {
        assert_eq!(account.is_current, account.id == alice.id, "{}", account.id);
    }
    let store = h.app.antigravity_load().unwrap();
    assert!(store.settings_provider.is_none() && store.settings_record.is_none());

    // Settings set to the Gemini API by hand show the login as not in use;
    // a provider's card is current only once applied.
    fs::write(&settings, "{\"modelProvider\":\"gemini\"}").unwrap();
    assert!(cards(&h.app).await.iter().all(|a| !a.is_current));
    // A provider's cards say no hi.
    assert_eq!(
        h.app.say_hi(Pool::Antigravity, &card.id).await.unwrap_err(),
        NOT_AVAILABLE
    );

    // A provider edited during its switch leaves agy as it was.
    *moved.lock().unwrap() = Some("https://moved.invalid".into());
    assert_eq!(
        h.app
            .switch_account(Pool::Antigravity, &card.id)
            .await
            .unwrap_err(),
        "The Antigravity provider changed during the switch; switch again"
    );
    assert_eq!(read_json(&settings), json!({"modelProvider": "gemini"}));
    assert_eq!(fs::read_to_string(&env).unwrap(), env_original);
    assert!(h
        .app
        .antigravity_load()
        .unwrap()
        .settings_provider
        .is_none());
}

#[test]
fn sub2apis_antigravity_usage_is_its_quota_groups_else_each_models() {
    let now = 1_000;
    let groups = sub2api_usage(
        &json!({"subscription_tier": "ULTRA", "subscription_tier_raw": "g1-ultra-lite-tier", "antigravity_quota_groups": [
            {"key": "gemini", "windows": [{"window": "5h", "utilization": 12.5, "resets_at": "2099-01-01T00:00:00Z"}]},
            {"key": "other", "windows": [{"window": "5h", "utilization": 99}]}
        ]}),
        now,
    )
    .unwrap();
    assert_eq!(groups.plan_type.as_deref(), Some("ultra 5x"));
    assert_eq!(groups.five_hour.as_ref().unwrap().used_percent, 12.5);
    assert!(groups.third_party_five_hour.is_none() && groups.one_week.is_none());
    // Each model's, pooled by the one with the least left.
    let models = sub2api_usage(
        &json!({"subscription_tier": "UNKNOWN", "antigravity_quota": {
            "gemini-3-pro": {"utilization": 40, "reset_time": "2099-01-01T00:00:00Z"},
            "gemini-3-flash": {"utilization": 20, "reset_time": "2099-01-01T00:00:00Z"},
            "claude-sonnet": {"utilization": 70, "reset_time": "2099-01-01T00:00:00Z"}
        }}),
        now,
    )
    .unwrap();
    assert!(models.plan_type.is_none());
    assert_eq!(models.five_hour.as_ref().unwrap().used_percent, 40.0);
    assert_eq!(
        models.third_party_five_hour.as_ref().unwrap().used_percent,
        70.0
    );
    // An account Sub2api cannot use says why.
    for (body, error) in [
        (
            json!({"is_forbidden": true, "forbidden_reason": "verify"}),
            "verify",
        ),
        (
            json!({"needs_reauth": true}),
            "The account must sign in to Sub2api again",
        ),
        (
            json!({}),
            "Sub2api reports no Antigravity quota for this account",
        ),
    ] {
        assert_eq!(sub2api_usage(&body, now).unwrap_err(), error);
    }
}

#[tokio::test]
async fn agy_proxy_fails_over_and_unwraps_cloud_code_streams() {
    use crate::proxy_service::start_api_proxy_with_runtime;
    use axum::{
        extract::Request, http::StatusCode, response::IntoResponse, routing::post, Json, Router,
    };

    let h = harness(true).await;
    let app = h.app.into_shared();
    for name in ["alice", "bob"] {
        h.live
            .sign_in(&login_of(name, &format!("access-{name}"), later()));
        app.antigravity_import_current().await.unwrap();
    }
    let calls = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new().fallback(post({
        let calls = calls.clone();
        move |req: Request| {
            let calls = calls.clone();
            async move {
                let token = req.headers()["authorization"].to_str().unwrap().to_owned();
                assert!(req.headers().get("x-goog-api-key").is_none());
                assert_eq!(
                    req.headers()["user-agent"],
                    crate::proxy_service::antigravity::cli_agent()
                );
                if req.uri().path() == api::FETCH_MODELS_PATH {
                    let bytes = axum::body::to_bytes(req.into_body(), 1024).await.unwrap();
                    assert_eq!(
                        serde_json::from_slice::<Value>(&bytes).unwrap()["project"],
                        "project-1"
                    );
                    // Neon is usable upstream with the CLI User-Agent; also test fallback when absent.
                    return Json(json!({"models":{
                        "gemini-3.1-pro-high": {},
                        "gemini-4-neon-high": {}
                    }}))
                    .into_response();
                }
                assert_eq!(
                    req.uri().path_and_query().unwrap().as_str(),
                    "/v1internal:streamGenerateContent?alt=sse"
                );
                let bytes = axum::body::to_bytes(req.into_body(), 4 * 1024 * 1024)
                    .await
                    .unwrap();
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                calls.lock().unwrap().push(token.clone());
                if token == "Bearer access-bob" {
                    return (
                        StatusCode::TOO_MANY_REQUESTS,
                        Json(json!({"error":{"message":"rate limited"}})),
                    )
                        .into_response();
                }
                if token == "Bearer access-alice" {
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({"error":{"message":"token expired"}})),
                    )
                        .into_response();
                }
                assert_eq!(body["project"], "project-1");
                assert_eq!(body["userAgent"], "antigravity");
                assert_eq!(body["requestType"], "agent");
                assert!(body["requestId"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("agent/")));
                let thinking = &body["request"]["generationConfig"]["thinkingConfig"];
                let expected = match thinking["thinkingLevel"].as_str() {
                    Some("HIGH") => "gemini-4-neon-high",
                    Some("EXTRA_HIGH") => "gemini-4-neon-xhigh",
                    _ => "gemini-3.1-pro-high",
                };
                assert_eq!(body["model"], expected);
                assert!(body["request"]["contents"].is_array());
                let frame = |text, finish| {
                    let mut candidate = json!({"content":{"role":"model","parts":[{"text":text}]}});
                    if finish {
                        candidate["finishReason"] = "STOP".into();
                    }
                    format!(
                        "data: {}\r\n\r\n",
                        json!({"response":{"candidates":[candidate]}})
                    )
                };
                (
                    [("content-type", "text/event-stream")],
                    frame("MOCK_", false).replace("\r\n", "\n") + &frame("OK", true),
                )
                    .into_response()
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut storage = app.proxy_storage(Pool::Antigravity);
    storage.upstream_base_url = Some(format!("http://{}", listener.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let status = start_api_proxy_with_runtime(
        &storage,
        &app.state.antigravity_api_proxy,
        Some(0),
        "127.0.0.1",
    )
    .await
    .unwrap();
    let base = status.base_url.unwrap();
    let key = status.api_key.unwrap();
    let post = |key: &str| {
        reqwest::Client::new().post(format!("{base}/v1beta/models/gemini-3.1-pro-preview:streamGenerateContent?alt=sse"))
        .header("x-goog-api-key", key).json(&json!({"contents":[{"parts":[{"text":"hi"}]}],"generationConfig":{"thinkingConfig":{"thinkingBudget":-1}}})).send()
    };
    assert_eq!(post("wrong").await.unwrap().status(), 401);
    assert!(calls.lock().unwrap().is_empty());
    let response = post(&key).await.unwrap();
    assert_eq!(response.status(), 200);
    let text = response.text().await.unwrap();
    let parts: Vec<Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(parts.len(), 2);
    assert_eq!(
        parts[0]["candidates"][0]["content"]["parts"][0]["text"],
        "MOCK_"
    );
    assert_eq!(parts[1]["candidates"][0]["finishReason"], "STOP");
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "Bearer access-bob",
            "Bearer access-alice",
            "Bearer fresh-refresh-alice"
        ]
    );
    assert!(!app.proxy_status(Pool::Codex).await.unwrap().running);
    let rotated = app.rotate_proxy_key(Pool::Antigravity).await.unwrap();
    assert_eq!(post(&key).await.unwrap().status(), 401);
    assert_eq!(
        post(rotated.api_key.as_deref().unwrap())
            .await
            .unwrap()
            .status(),
        200
    );
    let neon = reqwest::Client::new()
        .post(format!(
            "{base}/v1beta/models/gemini-4-neon:streamGenerateContent?alt=sse"
        ))
        .header("x-goog-api-key", rotated.api_key.as_deref().unwrap())
        .json(&json!({"contents":[{"parts":[{"text":"hi"}]}],
            "generationConfig":{"thinkingConfig":{"thinkingLevel":"HIGH"}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(neon.status(), 200);
    assert!(neon.text().await.unwrap().contains("MOCK_"));
    let neon_xhigh = reqwest::Client::new()
        .post(format!(
            "{base}/v1beta/models/gemini-4-neon:streamGenerateContent?alt=sse"
        ))
        .header("x-goog-api-key", rotated.api_key.as_deref().unwrap())
        .json(&json!({"contents":[{"parts":[{"text":"hi"}]}],
            "generationConfig":{"thinkingConfig":{"thinkingLevel":"EXTRA_HIGH"}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(neon_xhigh.status(), 200);
    assert!(neon_xhigh.text().await.unwrap().contains("MOCK_"));
    app.stop_proxy(Pool::Antigravity).await.unwrap();
}

#[tokio::test]
async fn agy_proxy_forwards_to_the_imported_gateway_with_its_own_key() {
    use axum::{http::HeaderMap, routing::post, Json, Router};
    let h = harness(true).await;
    let app = h.app.into_shared();
    app.set_antigravity_environment(Arc::new(|names, _| {
        names
            .iter()
            .map(|name| (*name == "RELAY_KEY").then(|| "upstream-key".to_owned()))
            .collect()
    }));
    let router = Router::new().route(
        "/antigravity/v1beta/models/gemini-4-neon:generateContent",
        post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["x-goog-api-key"], "upstream-key");
            assert!(headers.get("authorization").is_none());
            assert_eq!(body["tools"], json!([{"functionDeclarations":[]}]));
            Json(json!({"candidates":[{"finishReason":"STOP"}]}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut store = app.antigravity_load().unwrap();
    store.providers.push(serde_json::from_value(json!({
        "id":"relay", "name":"relay", "baseURL":base, "apiKeyVariable":"RELAY_KEY",
        "importedAccountIDs":[7], "cachedAccounts":[{"id":7,"name":"alice","accountType":"oauth","status":"active"}]
    })).unwrap());
    // A provider without its URL is left out, not every route.
    store.providers.push(serde_json::from_value(json!({
        "id":"unset", "name":"unset", "baseURL":"", "apiKeyVariable":"RELAY_KEY",
        "importedAccountIDs":[8], "cachedAccounts":[{"id":8,"name":"bob","accountType":"oauth","status":"active"}]
    })).unwrap());
    app.antigravity_save(&store).unwrap();
    let storage = app.proxy_storage(Pool::Antigravity);
    let status = crate::proxy_service::start_api_proxy_with_runtime(
        &storage,
        &app.state.antigravity_api_proxy,
        Some(0),
        "127.0.0.1",
    )
    .await
    .unwrap();
    let response = reqwest::Client::new()
        .post(format!(
            "{}/v1beta/models/gemini-4-neon:generateContent",
            status.base_url.unwrap()
        ))
        .header("x-goog-api-key", status.api_key.unwrap())
        .json(&json!({"tools":[{"functionDeclarations":[]}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["candidates"][0]["finishReason"],
        "STOP"
    );
    app.stop_proxy(Pool::Antigravity).await.unwrap();
}

#[tokio::test]
async fn a_remote_node_receives_only_released_logins_and_syncs_its_renewal() {
    let h = harness(true).await;
    let now = now_unix_seconds();
    h.app
        .antigravity_import_json(&login_of("alice", "access-alice", now + 600).to_string())
        .await
        .unwrap();
    let mut store = h.app.antigravity_load().unwrap();
    let id = store.accounts[0].id.clone();
    let mut pending = store.accounts[0].clone();
    pending.id = "pending".into();
    pending.first_request_pending = true;
    store.accounts.push(pending);
    let mut disabled = store.accounts[0].clone();
    disabled.id = "disabled-renewal".into();
    disabled.renewal = Some(Renewal::Never);
    store.accounts.push(disabled);
    h.app.antigravity_save(&store).unwrap();
    let payload = |bytes: Vec<u8>| serde_json::from_slice::<Value>(&bytes).unwrap();
    let mut node = payload(h.app.antigravity_remote_payload(None).await.unwrap());
    assert_eq!(node["accounts"].as_array().unwrap().len(), 2);
    assert!(node["accounts"][1]["authJson"]
        .get("refreshToken")
        .is_none());
    let row = &mut node["accounts"][0];
    assert_eq!(row["id"], id);
    assert_eq!(row["authJson"]["project"], "project-1");
    let mut renewed =
        crate::proxy_service::antigravity::renew_remote(&row["authJson"], "", &*h.fake)
            .await
            .unwrap();
    renewed["refreshToken"] = "rotated-on-node".into();
    row["authJson"] = renewed;
    // The node's renewal comes back, and goes up again with its token.
    let resynced = payload(h.app.antigravity_remote_payload(Some(&node)).await.unwrap());
    assert_eq!(
        resynced["accounts"][0]["authJson"]["refreshToken"],
        "rotated-on-node"
    );
    let mut stored = h.app.antigravity_load().unwrap();
    assert_eq!(
        pool_login::antigravity::refresh_token(&stored.accounts[0].document).as_deref(),
        Some("rotated-on-node")
    );
    // A delayed node snapshot cannot replace a new local sign-in.
    stored.accounts[0].document = login_of("alice", "new-login", now + 100);
    h.app.antigravity_save(&stored).unwrap();
    let kept = payload(h.app.antigravity_remote_payload(Some(&node)).await.unwrap());
    assert_eq!(kept["accounts"][0]["authJson"]["accessToken"], "new-login");
}

#[tokio::test]
async fn a_remote_node_reads_back_the_proxy_form_it_receives() {
    use pool_login::antigravity::proxy_form;
    let h = harness(true).await;
    let now = now_unix_seconds();
    h.app
        .antigravity_import_json(&login_of("alice", "access-alice", now + 600).to_string())
        .await
        .unwrap();
    let mut store = h.app.antigravity_load().unwrap();
    let mut kept = store.accounts[0].clone();
    kept.id = "never-renewed".into();
    kept.renewal = Some(Renewal::Never);
    store.accounts.push(kept);
    h.app.antigravity_save(&store).unwrap();
    let document = store.accounts[0].document.clone();
    let node: Value =
        serde_json::from_slice(&h.app.antigravity_remote_payload(None).await.unwrap()).unwrap();
    let auth = &node["accounts"][0]["authJson"];
    assert_eq!(
        proxy_form::access_token(auth).as_deref(),
        Some("access-alice")
    );
    assert_eq!(
        proxy_form::refresh_token(auth).as_deref(),
        Some("refresh-alice")
    );
    assert_eq!(proxy_form::project(auth), Some("project-1"));
    assert!(proxy_form::source_generation(auth).is_some_and(|g| g.len() == 64));
    // Seconds on the wire, the login's expiry in milliseconds read back.
    assert_eq!(proxy_form::expires_at(auth), Some(now + 600));
    assert_eq!(
        proxy_form::expiry_ms(auth),
        pool_login::antigravity::expiry_ms(&document)
    );
    assert_eq!(auth["expiresAt"], json!(now + 600));
    // A node that may not renew the login gets no refresh token to renew it with.
    let never = &node["accounts"][1]["authJson"];
    assert_eq!(proxy_form::refresh_token(never), None);
    assert_eq!(proxy_form::source_generation(never), None);
    assert_eq!(
        proxy_form::access_token(never).as_deref(),
        Some("access-alice")
    );
    // The node's renewal is read back as it wrote it.
    let renewed = crate::proxy_service::antigravity::renew_remote(auth, "", &*h.fake)
        .await
        .unwrap();
    let lifetime = proxy_form::lifetime(&renewed).unwrap();
    assert!(lifetime > 0);
    let at = proxy_form::expires_at(&renewed).unwrap();
    assert!((now + lifetime..=now_unix_seconds() + lifetime).contains(&at));
    assert_eq!(
        proxy_form::source_generation(&renewed),
        proxy_form::source_generation(auth)
    );
    let mut synced = node.clone();
    synced["accounts"][0]["authJson"] = renewed.clone();
    h.app
        .antigravity_remote_payload(Some(&synced))
        .await
        .unwrap();
    let stored = h.app.antigravity_load().unwrap();
    assert_eq!(
        pool_login::antigravity::access_token(&stored.accounts[0].document),
        proxy_form::access_token(&renewed)
    );
    assert_eq!(
        pool_login::antigravity::expiry(&stored.accounts[0].document),
        Some(at)
    );
}

#[tokio::test]
async fn the_proxy_names_a_login_by_its_own_google_id_else_the_stored_one() {
    let h = harness(true).await;
    for user in ["alice", "bob", "carol"] {
        h.app
            .antigravity_import_json(
                &login_of(user, &format!("access-{user}"), later()).to_string(),
            )
            .await
            .unwrap();
    }
    let mut store = h.app.antigravity_load().unwrap();
    let mut ids = std::collections::HashMap::new();
    for account in &mut store.accounts {
        let access = pool_login::antigravity::access_token(&account.document).unwrap();
        match access.as_str() {
            "access-alice" => {
                account.document["google_id"] = "google-login-alice".into();
                account.own.google_id = Some("google-stored-alice".into());
            }
            "access-bob" => account.own.google_id = Some("google-stored-bob".into()),
            _ => account.own.google_id = None,
        }
        ids.insert(access, account.id.clone());
    }
    h.app.antigravity_save(&store).unwrap();
    let expected = [
        ("access-alice", "google-login-alice".to_owned()),
        ("access-bob", "google-stored-bob".to_owned()),
        // Neither names the account: its card does.
        ("access-carol", ids["access-carol"].clone()),
    ];
    let candidates = h.app.antigravity_proxy_candidates().unwrap();
    let node: Value =
        serde_json::from_slice(&h.app.antigravity_remote_payload(None).await.unwrap()).unwrap();
    for (access, account_id) in expected {
        let id = &ids[access];
        let candidate = candidates.iter().find(|c| &c.id == id).unwrap();
        assert_eq!(candidate.account_id, account_id, "{access}");
        // A remote node's candidate is named the same.
        let row = node["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == json!(id))
            .unwrap();
        assert_eq!(row["accountId"], json!(account_id), "{access}");
    }
}

#[test]
fn a_card_folded_into_its_google_account_brings_its_project() {
    let card = |id: &str, project: Option<&str>| -> Stored {
        serde_json::from_value(json!({
            "id": id, "label": id, "googleId": "g-1", "document": login_of("alice", id, later()),
            "addedAt": 1, "updatedAt": 1, "project": project,
        }))
        .unwrap()
    };
    let mut store = Store {
        accounts: vec![card("old", None), card("new", Some("project-1"))],
        ..Default::default()
    };
    merge_identified(&mut store, "new");
    assert_eq!(store.accounts.len(), 1);
    assert_eq!(store.accounts[0].id, "old");
    assert_eq!(store.accounts[0].own.project.as_deref(), Some("project-1"));
}

#[tokio::test]
async fn switching_hands_the_login_and_identity_to_the_switch_effects() {
    let h = harness(false).await;
    h.live.sign_in(&login_of("alice", "access-alice", later()));
    h.app.import_current(Pool::Antigravity).await.unwrap();
    h.live.sign_in(&login_of("bob", "access-bob", later()));
    h.app.import_current(Pool::Antigravity).await.unwrap();
    let mut store = h.app.antigravity_load().unwrap();
    let alice_id = store
        .accounts
        .iter_mut()
        .find(|a| {
            pool_login::antigravity::access_token(&a.document).as_deref() == Some("access-alice")
        })
        .map(|a| {
            a.email = Some("alice@gmail.com".into());
            a.own.google_id = Some("google-sub-alice".into());
            a.id.clone()
        })
        .unwrap();
    h.app.antigravity_save(&store).unwrap();

    let seen = Arc::new(std::sync::Mutex::new(None));
    let observed = seen.clone();
    h.app
        .on_switch_effects(Arc::new(move |pool, _, credential| {
            let observed = observed.clone();
            Box::pin(async move {
                *observed.lock().unwrap() = Some((pool, credential));
                Ok(SwitchExecution::default())
            })
        }));

    h.app
        .switch_account(Pool::Antigravity, &alice_id)
        .await
        .unwrap();
    let (pool, credential) = seen.lock().unwrap().take().unwrap();
    assert_eq!(pool, Pool::Antigravity);
    let credential = credential.unwrap().unwrap();
    assert_eq!(credential.access, "access-alice");
    assert_eq!(credential.email.as_deref(), Some("alice@gmail.com"));
    assert_eq!(credential.account_id.as_deref(), Some("google-sub-alice"));

    // A login naming no account, of an account Copool knows nothing of,
    // hands over the error each agent to sync reports.
    let bob_id = store
        .accounts
        .iter()
        .find(|a| a.id != alice_id)
        .unwrap()
        .id
        .clone();
    h.app
        .switch_account(Pool::Antigravity, &bob_id)
        .await
        .unwrap();
    let (_, credential) = seen.lock().unwrap().take().unwrap();
    assert_eq!(
        credential.unwrap().unwrap_err(),
        "Missing Antigravity account identity"
    );
}
