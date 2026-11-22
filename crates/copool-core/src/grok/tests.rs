use super::*;
use crate::{account_network::Response, Preferences};
use serde_json::json;
use std::sync::Mutex;

/// xAI, answering by the access token: tokens starting with "expired" are
/// refused, and a renewal of `refresh-<user>` hands out `fresh-<user>` and
/// rotates the refresh token to `rotated-<user>`.
#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    /// Refresh tokens xAI refuses.
    revoked: Mutex<Vec<String>>,
    /// The share of the period used.
    used: Mutex<f64>,
    /// The settings fail, as a server error.
    settings_down: std::sync::atomic::AtomicBool,
    /// A file Grok Build writes while a renewal is under way.
    meanwhile: Mutex<Option<(PathBuf, Value)>>,
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
    fn api<'a>(
        &'a self,
        path: &'a str,
        token: &'a str,
        proxy: &'a str,
    ) -> Response<'a, api::Reply> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{path} {token} {proxy}"));
        let used = *self.used.lock().unwrap();
        let settings_down = self
            .settings_down
            .load(std::sync::atomic::Ordering::Relaxed);
        Box::pin(async move {
            if token.starts_with("expired") {
                return Ok((401, json!({"error": "unauthorized"})));
            }
            Ok(match path {
                api::USAGE_PATH => (
                    200,
                    json!({"config": {
                        "currentPeriod": {
                            "type": "USAGE_PERIOD_TYPE_WEEKLY",
                            "start": "2026-11-21T15:17:54.020+00:00",
                            "end": "2026-11-28T15:17:54.020+00:00"
                        },
                        "creditUsagePercent": used,
                        "productUsage": [{"product": "GrokBuild", "usagePercent": used}],
                        "isUnifiedBillingUser": true,
                        "prepaidBalance": {"val": 0},
                        "billingPeriodStart": "2026-11-21T15:17:54.020+00:00",
                        "billingPeriodEnd": "2026-11-28T15:17:54.020+00:00"
                    }}),
                ),
                api::SETTINGS_PATH if settings_down => (503, Value::Null),
                api::SETTINGS_PATH => (
                    200,
                    json!({"subscription_tier": null, "subscription_tier_display": "SuperGrok"}),
                ),
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
        if let Some((path, file)) = self.meanwhile.lock().unwrap().take() {
            write_json(&path, &file).unwrap();
        }
        let user = refresh_token
            .trim_start_matches("refresh-")
            .trim_start_matches("rotated-")
            .to_owned();
        Box::pin(async move {
            Ok(if revoked {
                (400, json!({"error": "invalid_grant"}))
            } else {
                (
                    200,
                    json!({
                        "access_token": format!("fresh-{user}"),
                        "refresh_token": format!("rotated-{user}"),
                        "expires_in": 21600,
                        "token_type": "Bearer"
                    }),
                )
            })
        })
    }
}

/// A sign-in of `user` as Grok Build keeps it.
fn sign_in_of(user: &str, access: &str, expires: i64) -> Value {
    json!({
        "key": access,
        "auth_mode": "oidc",
        "create_time": login::timestamp(expires - 21600),
        "user_id": format!("uid-{user}"),
        "email": format!("{user}@example.com"),
        "first_name": user,
        "principal_type": "User",
        "team_id": format!("team-{user}"),
        "refresh_token": format!("refresh-{user}"),
        "expires_at": login::timestamp(expires),
        "oidc_issuer": login::ISSUER,
        "oidc_client_id": login::CLIENT_ID
    })
}
/// Grok Build's `auth.json` holding the sign-in.
fn auth_file(document: &Value) -> Value {
    json!({ login::entry_key(document): document })
}
fn later() -> i64 {
    now_unix_seconds() + 3600
}

struct Harness {
    app: Application,
    fake: Arc<Fake>,
    auth: PathBuf,
    _dir: tempfile::TempDir,
}
impl Harness {
    fn sign_in(&self, document: &Value) {
        write_json(&self.auth, &auth_file(document)).unwrap();
    }
    fn file(&self) -> Value {
        serde_json::from_slice(&fs::read(&self.auth).unwrap()).unwrap()
    }
    fn live(&self) -> Value {
        login::sign_in_of(&self.file()).unwrap()
    }
    fn stored(&self, id: &str) -> Value {
        self.app
            .grok_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| a.id == id)
            .unwrap()
            .document
    }
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
    let auth = dir.path().join("grok/auth.json");
    app.set_grok_auth_file(auth.clone());
    let fake = Arc::new(Fake::default());
    *fake.used.lock().unwrap() = 7.0;
    app.set_grok_network(fake.clone());
    let preferences = Preferences {
        request_new_accounts,
        ..app.preferences().unwrap()
    };
    app.save_preferences(preferences).await.unwrap();
    Harness {
        app,
        fake,
        auth,
        _dir: dir,
    }
}
async fn cards(app: &Application) -> Vec<AccountSummary> {
    app.accounts(Pool::Grok).await.unwrap()
}
fn card_of(cards: &[AccountSummary], user: &str) -> AccountSummary {
    let email = format!("{user}@example.com");
    cards
        .iter()
        .find(|c| c.email.as_deref() == Some(email.as_str()))
        .unwrap()
        .clone()
}

#[tokio::test]
async fn an_imported_login_waits_then_shows_its_weekly_usage_and_plan() {
    let h = harness(false).await;
    assert_eq!(
        h.app.import_current(Pool::Grok).await.unwrap_err(),
        LOGIN_NOT_FOUND
    );
    h.sign_in(&sign_in_of("alice", "access-alice", later()));
    assert_eq!(h.app.import_current(Pool::Grok).await.unwrap(), 1);
    let card = cards(&h.app).await.remove(0);
    assert_eq!(card.label, "alice@example.com");
    assert_eq!(card.account_id, "uid-alice");
    assert!(card.awaiting_first_request && card.is_current);
    assert!(h.fake.calls().is_empty());

    h.app
        .save_account_proxy(Pool::Grok, &card.id, "socks5://127.0.0.1:7")
        .await
        .unwrap();
    h.app.refresh_now(Pool::Grok, Some(&card.id)).await.unwrap();
    let card = cards(&h.app).await.remove(0);
    assert_eq!(card.plan_type.as_deref(), Some("SuperGrok"));
    assert!(!card.awaiting_first_request && card.is_current);
    let usage = card.usage.unwrap();
    assert_eq!(usage.plan_type.as_deref(), Some("SuperGrok"));
    assert!(usage.five_hour.is_none() && usage.one_month.is_none());
    let week = usage.one_week.unwrap();
    assert_eq!(week.used_percent, 7.0);
    assert_eq!(week.window_seconds, 7 * 24 * 60 * 60);
    assert_eq!(
        week.reset_at,
        Some(
            chrono::DateTime::parse_from_rfc3339("2026-11-28T15:17:54.020+00:00")
                .unwrap()
                .timestamp()
        )
    );
    // Every request goes through the account's proxy.
    assert_eq!(
        h.fake.calls(),
        [
            format!("{} access-alice socks5://127.0.0.1:7", api::USAGE_PATH),
            format!("{} access-alice socks5://127.0.0.1:7", api::SETTINGS_PATH),
        ]
    );
    // A failed settings request keeps the plan the card had.
    h.fake
        .settings_down
        .store(true, std::sync::atomic::Ordering::Relaxed);
    h.app.refresh_usage(Pool::Grok, &card.id).await.unwrap();
    assert_eq!(
        cards(&h.app).await[0].plan_type.as_deref(),
        Some("SuperGrok")
    );
}

#[tokio::test]
async fn switching_writes_the_sign_in_keeps_other_issuers_and_backs_up_a_stranger() {
    let h = harness(true).await;
    let alice = sign_in_of("alice", "access-alice", later());
    let bob = sign_in_of("bob", "access-bob", later());
    h.app
        .import_json(Pool::Grok, &json!([alice, {"document": bob}]).to_string())
        .await
        .unwrap();
    // Grok Build uses a sign-in the pool does not hold, beside another
    // issuer's.
    let carol = sign_in_of("carol", "access-carol", later());
    let mut file = auth_file(&carol);
    file["https://acme.okta.com::client"] = json!({"key": "okta", "refresh_token": "okta-r"});
    write_json(&h.auth, &file).unwrap();
    let accounts = cards(&h.app).await;
    assert_eq!(accounts.len(), 2);
    assert!(accounts.iter().all(|a| !a.is_current));
    let alice_card = card_of(&accounts, "alice");
    h.app
        .switch_account(Pool::Grok, &alice_card.id)
        .await
        .unwrap();
    let file = h.file();
    assert_eq!(file[login::entry_key(&alice)], alice);
    assert_eq!(file["https://acme.okta.com::client"]["key"], "okta");
    assert_eq!(file.as_object().unwrap().len(), 2);
    let backup: Value = serde_json::from_slice(
        &fs::read(h.app.paths.data_dir.join("grok-before-switch.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(backup, carol);
    assert!(card_of(&cards(&h.app).await, "alice").is_current);

    // Grok Build renews the sign-in, rotating its refresh token: the pool
    // follows the account and adopts the newer generation.
    let mut renewed = sign_in_of("alice", "renewed-by-grok", later() + 60);
    renewed["refresh_token"] = "rotated-by-grok".into();
    h.sign_in(&renewed);
    assert!(card_of(&cards(&h.app).await, "alice").is_current);
    assert_eq!(h.stored(&alice_card.id), renewed);

    // Smart switch goes to the account with more of its allowance left.
    h.app.release_new_accounts(Pool::Grok, None).await.unwrap();
    let bob_card = card_of(&cards(&h.app).await, "bob");
    h.app.refresh_usage(Pool::Grok, &bob_card.id).await.unwrap();
    *h.fake.used.lock().unwrap() = 90.0;
    h.app
        .refresh_usage(Pool::Grok, &alice_card.id)
        .await
        .unwrap();
    let outcome = h.app.smart_switch(Pool::Grok).await.unwrap();
    assert!(matches!(
        outcome,
        crate::SmartSwitchOutcome::Switched { .. }
    ));
    assert_eq!(login::email(&h.live()).as_deref(), Some("bob@example.com"));
    // A pool sign-in is not backed up again.
    let backup: Value = serde_json::from_slice(
        &fs::read(h.app.paths.data_dir.join("grok-before-switch.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(backup, carol);
}

#[tokio::test]
async fn expired_tokens_are_renewed_and_given_to_grok_build_when_it_holds_them() {
    let h = harness(true).await;
    h.sign_in(&sign_in_of(
        "alice",
        "expired-alice",
        now_unix_seconds() - 10,
    ));
    h.app.import_current(Pool::Grok).await.unwrap();
    h.app
        .import_sign_in(Pool::Grok, sign_in_of("bob", "expired-bob", later()), None)
        .await
        .unwrap();
    let alice = card_of(&cards(&h.app).await, "alice");
    let bob = card_of(&cards(&h.app).await, "bob");

    h.app.refresh_usage(Pool::Grok, &alice.id).await.unwrap();
    // The renewal, its rotated refresh token included, is the pool's and
    // Grok Build's: the refresh token Grok Build held no longer works.
    let stored = h.stored(&alice.id);
    assert_eq!(login::access_token(&stored).as_deref(), Some("fresh-alice"));
    assert_eq!(
        login::refresh_token(&stored).as_deref(),
        Some("rotated-alice")
    );
    let live = h.live();
    assert_eq!(login::access_token(&live).as_deref(), Some("fresh-alice"));
    assert_eq!(
        login::refresh_token(&live).as_deref(),
        Some("rotated-alice")
    );
    assert_eq!(live["first_name"], "alice");
    assert_eq!(card_of(&cards(&h.app).await, "alice").usage_error, None);

    // A refused token of a card Grok Build does not use is renewed, and
    // Grok Build's sign-in stays as it was.
    h.app.refresh_usage(Pool::Grok, &bob.id).await.unwrap();
    assert_eq!(
        login::access_token(&h.stored(&bob.id)).as_deref(),
        Some("fresh-bob")
    );
    assert_eq!(h.live(), live);
    assert_eq!(h.fake.renewals(), 2);

    // A refresh token xAI refuses shows on the card; renewal off asks none.
    h.fake.revoked.lock().unwrap().push("refresh-carol".into());
    h.app
        .import_sign_in(
            Pool::Grok,
            sign_in_of("carol", "expired-carol", now_unix_seconds() - 10),
            None,
        )
        .await
        .unwrap();
    let carol = card_of(&cards(&h.app).await, "carol");
    assert_eq!(
        h.app
            .refresh_now(Pool::Grok, Some(&carol.id))
            .await
            .unwrap_err(),
        SIGN_IN_EXPIRED
    );
    assert_eq!(
        card_of(&cards(&h.app).await, "carol")
            .usage_error
            .as_deref(),
        Some(SIGN_IN_EXPIRED)
    );
    h.app
        .set_renewal(Pool::Grok, &carol.id, "never")
        .await
        .unwrap();
    let before = h.fake.renewals();
    assert_eq!(
        h.app
            .refresh_usage(Pool::Grok, &carol.id)
            .await
            .unwrap_err(),
        RENEWAL_OFF
    );
    assert_eq!(h.fake.renewals(), before);
}

#[tokio::test]
async fn a_renewal_never_replaces_a_sign_in_grok_build_renewed_meanwhile() {
    let h = harness(true).await;
    h.sign_in(&sign_in_of("alice", "access-alice", later()));
    h.app.import_current(Pool::Grok).await.unwrap();
    let id = cards(&h.app).await[0].id.clone();
    // Grok Build renews from the same refresh token first; Copool's own
    // renewal, from the refresh token it held, leaves that sign-in alone.
    let mut theirs = sign_in_of("alice", "grok-renewed", later() + 7200);
    theirs["refresh_token"] = "grok-rotated".into();
    *h.fake.meanwhile.lock().unwrap() = Some((h.auth.clone(), auth_file(&theirs)));
    h.app.renew_now(Pool::Grok, &id).await.unwrap();
    assert_eq!(h.live(), theirs);
    assert_eq!(
        login::access_token(&h.stored(&id)).as_deref(),
        Some("fresh-alice")
    );
    // Its own chain stays Grok Build's from then on, as Copool's stays its.
    h.app.renew_now(Pool::Grok, &id).await.unwrap();
    assert_eq!(h.live(), theirs);
}

#[tokio::test]
async fn imports_are_checked_whole_and_folded_into_known_accounts() {
    let h = harness(true).await;
    let alice = sign_in_of("alice", "access-alice", later());
    // Grok Build's whole file, as copied from another computer.
    assert_eq!(
        h.app
            .import_json(Pool::Grok, &auth_file(&alice).to_string())
            .await
            .unwrap(),
        1
    );
    // One sign-in without a refresh token refuses the whole import.
    let bad = json!({"key": "a", "email": "x@example.com"});
    assert_eq!(
        h.app
            .import_json(
                Pool::Grok,
                &json!({"accounts": [sign_in_of("bob", "access-bob", later()), bad]}).to_string()
            )
            .await
            .unwrap_err(),
        CREDENTIALS_INVALID
    );
    assert_eq!(cards(&h.app).await.len(), 1);
    // A newer sign-in of the same account, with a rotated refresh token,
    // updates its card.
    let mut newer = sign_in_of("alice", "access-alice-2", later() + 60);
    newer["refresh_token"] = "rotated".into();
    h.app
        .import_json(Pool::Grok, &newer.to_string())
        .await
        .unwrap();
    let accounts = cards(&h.app).await;
    assert_eq!(accounts.len(), 1);
    assert_eq!(h.stored(&accounts[0].id), newer);
    // The same user in another team is another account.
    let mut team = sign_in_of("alice", "access-team", later());
    team["team_id"] = "team-shared".into();
    h.app
        .import_json(Pool::Grok, &team.to_string())
        .await
        .unwrap();
    assert_eq!(cards(&h.app).await.len(), 2);
}

#[tokio::test]
async fn the_pool_has_no_proxy_providers_or_hi() {
    let h = harness(true).await;
    h.sign_in(&sign_in_of("alice", "access-alice", later()));
    h.app.import_current(Pool::Grok).await.unwrap();
    let id = cards(&h.app).await[0].id.clone();
    let calls = h.fake.calls().len();
    assert!(!h.app.proxy_status(Pool::Grok).await.unwrap().running);
    assert_eq!(
        h.app.start_proxy(Pool::Grok).await.unwrap_err(),
        NOT_AVAILABLE
    );
    assert_eq!(h.app.providers(Pool::Grok).unwrap(), json!([]));
    assert_eq!(
        h.app
            .save_provider(Pool::Grok, json!({}))
            .await
            .unwrap_err(),
        NOT_AVAILABLE
    );
    assert_eq!(
        h.app.say_hi(Pool::Grok, &id).await.unwrap_err(),
        NOT_AVAILABLE
    );
    assert_eq!(h.fake.calls().len(), calls);
}

#[test]
fn usage_and_plan_are_read_as_grok_build_reports_them() {
    let now = 1_795_000_000;
    let monthly = json!({"config": {
        "currentPeriod": {"type": "USAGE_PERIOD_TYPE_MONTHLY",
            "start": "2026-11-01T00:00:00+00:00", "end": "2026-12-01T00:00:00+00:00"},
        "creditUsagePercent": "12.5"
    }});
    let usage = api::usage(&monthly, now).unwrap();
    assert!(usage.one_week.is_none());
    let month = usage.one_month.unwrap();
    assert_eq!(month.used_percent, 12.5);
    assert_eq!(month.window_seconds, 30 * 24 * 60 * 60);
    assert_eq!(usage.fetched_at, now);
    // Without a period, a week ends nowhere known; above 100 is full.
    let bare = api::usage(&json!({"config": {"creditUsagePercent": 140}}), now).unwrap();
    let week = bare.one_week.unwrap();
    assert_eq!((week.used_percent, week.reset_at), (100.0, None));
    assert!(api::usage(&json!({"config": {}}), now).is_none());
    assert_eq!(
        api::plan(&json!({"settings": {"subscription_tier_display": " Free "}})).as_deref(),
        Some("Free")
    );
    assert_eq!(api::plan(&json!({"subscription_tier_display": ""})), None);
}

#[test]
fn a_browser_sign_in_is_kept_as_grok_build_keeps_its_sign_in() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let jwt = |claims: Value| {
        format!(
            "e30.{}.sig",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
        )
    };
    let access = jwt(json!({"sub": "uid-dana", "team_id": "team-dana",
        "principal_type": "User", "principal_id": "pid"}));
    let tokens = json!({
        "access_token": access,
        "refresh_token": "refresh-dana",
        "expires_in": 21600,
        "id_token": jwt(json!({"sub": "uid-dana", "email": "dana@example.com",
            "given_name": "Dana", "family_name": "Lee"}))
    });
    let document = login::signed_in(&tokens, 1_795_000_000).unwrap();
    assert_eq!(document["key"], access);
    assert_eq!(document["user_id"], "uid-dana");
    assert_eq!(document["team_id"], "team-dana");
    assert_eq!(document["email"], "dana@example.com");
    assert_eq!(document["first_name"], "Dana");
    assert_eq!(document["last_name"], "Lee");
    assert_eq!(document["auth_mode"], "oidc");
    assert_eq!(login::expiry(&document), Some(1_795_000_000 + 21600));
    assert_eq!(
        login::entry_key(&document),
        format!("{}::{}", login::ISSUER, login::CLIENT_ID)
    );
    // Without a refresh token the sign-in would not last.
    let mut short = tokens;
    short.as_object_mut().unwrap().remove("refresh_token");
    assert_eq!(
        login::signed_in(&short, 0).unwrap_err(),
        CREDENTIALS_INVALID
    );
    // The time format is Grok Build's own.
    assert_eq!(login::timestamp(0), "1970-01-01T00:00:00.000000Z");
    let url = sign_in_url("http://127.0.0.1:5/callback", "c", "s");
    assert!(url.starts_with("https://auth.x.ai/oauth2/authorize?"));
    assert!(url.contains("client_id=b1a00492-073a-47ea-816f-4c329264a828"));
    assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A5%2Fcallback"));
}
