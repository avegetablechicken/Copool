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
    /// The Sub2api gateway's `/v1/models`, for the key `sub2api-key`.
    gateway: Mutex<Option<Value>>,
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
    fn gateway_models<'a>(
        &'a self,
        url: &'a str,
        key: &'a str,
        proxy: &'a str,
        insecure: bool,
    ) -> Response<'a, api::Reply> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("models {url} {key} {proxy} {insecure}"));
        let listed = self.gateway.lock().unwrap().clone();
        Box::pin(async move {
            Ok(match listed {
                Some(body) if key == "sub2api-key" => (200, body),
                _ => (401, json!({"error": {"message": "Invalid API key"}})),
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
async fn cards_read_while_a_switch_writes_keep_the_account_switched_to() {
    let h = harness(true).await;
    let alice = sign_in_of("alice", "access-alice", later());
    let bob = sign_in_of("bob", "access-bob", later());
    h.app
        .import_json(Pool::Grok, &json!([alice, bob]).to_string())
        .await
        .unwrap();
    h.sign_in(&alice);
    let bob_id = card_of(&cards(&h.app).await, "bob").id;
    // The cards are asked for while a switch to Bob holds the store to
    // write his sign-in.
    let lock = h.app.grok.lock.lock().await;
    let switch = async {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        h.sign_in(&bob);
        let mut store = h.app.grok_load().unwrap();
        store.current_account_id = Some(bob_id.clone());
        h.app.grok_save(&store).unwrap();
        drop(lock);
    };
    tokio::join!(cards(&h.app), switch);
    assert_eq!(
        h.app.grok_load().unwrap().current_account_id.as_deref(),
        Some(bob_id.as_str())
    );
}

#[test]
fn a_file_with_a_byte_order_mark_keeps_its_other_issuers() {
    let alice = sign_in_of("alice", "access-alice", later());
    let mut file = auth_file(&sign_in_of("carol", "access-carol", later()));
    file["https://acme.okta.com::client"] = json!({"key": "okta", "refresh_token": "okta-r"});
    let mut bytes = b"\xEF\xBB\xBF".to_vec();
    bytes.extend(serde_json::to_vec(&file).unwrap());
    assert!(login::decode(&bytes).is_some());
    let written = login::encode(Some(&bytes), &alice);
    assert_eq!(written[login::entry_key(&alice)], alice);
    assert_eq!(written["https://acme.okta.com::client"]["key"], "okta");
    assert_eq!(written.as_object().unwrap().len(), 2);
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
async fn a_sign_in_grok_build_renewed_is_kept_whatever_its_expiry() {
    let h = harness(true).await;
    let expires = later();
    for (user, live_expiry) in [("dana", true), ("erin", false)] {
        // Grok Build renewed the card's refused token, with no later expiry.
        let mut live = sign_in_of(user, &format!("live-{user}"), expires);
        live["refresh_token"] = format!("rotated-by-grok-{user}").into();
        if !live_expiry {
            live.as_object_mut().unwrap().remove("expires_at");
        }
        h.sign_in(&live);
        h.app
            .import_sign_in(
                Pool::Grok,
                sign_in_of(user, &format!("expired-{user}"), expires),
                None,
            )
            .await
            .unwrap();
        let card = card_of(&cards(&h.app).await, user);
        h.app.refresh_usage(Pool::Grok, &card.id).await.unwrap();
        assert_eq!(h.stored(&card.id), live, "{user}");
    }
    assert_eq!(h.fake.renewals(), 0);
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
async fn the_pool_has_no_proxy_or_hi() {
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
    // A provider needs its Sub2api address.
    assert_eq!(h.app.providers(Pool::Grok).unwrap(), json!([]));
    assert_eq!(
        h.app
            .save_provider(Pool::Grok, json!({"name": "Relay"}))
            .await
            .unwrap_err(),
        "error.sub2api.configuration_incomplete"
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
    // Text that parses to no finite number is no usage.
    for text in ["NaN", "inf", "-infinity"] {
        let body = json!({"config": {"creditUsagePercent": text}});
        assert!(api::usage(&body, now).is_none(), "{text}");
        let sub2api = json!({"seven_day": {"utilization": text}});
        let usage = api::sub2api_usage(&sub2api, now).unwrap();
        assert!(
            usage.one_week.is_none() && usage.one_month.is_none(),
            "{text}"
        );
    }
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

/// What a Sub2api reports for a Grok account's usage: before 0.1.155 no
/// billing, from it the billing (`grok_billing`), from 0.1.173 its weekly
/// progress too (`seven_day`), as 0.1.177 and 0.2.24 do alike.
fn grok_usage(version: &str) -> Value {
    let billing = json!({
        "period_type": "weekly", "usage_percent": 7.0,
        "period_start": "2099-01-01T00:00:00Z", "period_end": "2099-01-08T00:00:00Z",
        "plan": "SuperGrok", "status_code": 200
    });
    let parts: Vec<u64> = version.split('.').map(|p| p.parse().unwrap()).collect();
    match parts[..] {
        [0, 1, minor] if minor < 155 => json!({
            "source": "passive", "error_code": "quota_unknown",
            "error": "Grok quota is unknown until billing is probed or an upstream response includes xAI rate-limit headers",
            "grok_request_quota": {"limit": 60, "remaining": 59}
        }),
        [0, 1, minor] if minor < 173 => json!({
            "source": "passive", "grok_billing": billing, "subscription_tier": "SuperGrok"
        }),
        _ => json!({
            "source": "passive", "grok_billing": billing, "subscription_tier": "SuperGrok",
            "seven_day": {"utilization": 7.0, "resets_at": "2099-01-08T00:00:00Z", "remaining_seconds": 600},
            "thirty_day": {"utilization": 0.0, "resets_at": "2099-02-01T00:00:00Z"}
        }),
    }
}
/// A Sub2api admin API with Grok accounts, recording request paths: one
/// usable, one whose sign-in expired, and an API-key account and an OpenAI
/// account the pool must not show. It reports `version`.
async fn sub2api_server(
    requests: Arc<Mutex<Vec<String>>>,
    version: Arc<Mutex<&'static str>>,
) -> String {
    use axum::{extract::Request, response::IntoResponse, routing::any, Json, Router};
    let router = Router::new().fallback(any(move |request: Request| {
        let requests = requests.clone();
        let version = *version.lock().unwrap();
        async move {
            let uri = request.uri().path_and_query().unwrap().to_string();
            requests.lock().unwrap().push(uri.clone());
            let data = if uri.starts_with("/api/v1/auth/login") {
                json!({"access_token": "admin"})
            } else if uri == "/api/v1/admin/system/version" {
                json!({"version": version})
            } else if uri.starts_with("/api/v1/admin/accounts?") {
                assert!(uri.contains("platform=grok"), "{uri}");
                json!({"items": [
                    {"id": 7, "name": "alias@example.com", "platform": "grok", "type": "oauth", "status": "active",
                     "extra": {"email": " super@example.com "}, "credentials": {"email": "other@example.com",
                     "subscription_tier": "supergrok_heavy"}},
                    {"id": 8, "name": "lapsed@example.com", "platform": "grok", "type": "oauth", "status": "active"},
                    {"id": 9, "name": "key", "platform": "grok", "type": "apikey", "status": "active"},
                    {"id": 10, "name": "codex@example.com", "platform": "openai", "type": "oauth", "status": "active"}
                ], "pages": 1})
            } else if uri == "/api/v1/admin/accounts/8/usage" {
                json!({"needs_reauth": true, "error_code": "unauthenticated", "subscription_tier": "x_premium_plus",
                       "grok_billing": {"status_code": 401}})
            } else {
                assert_eq!(uri, "/api/v1/admin/accounts/7/usage");
                grok_usage(version)
            };
            Json(json!({"code": 0, "data": data})).into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}
#[tokio::test]
async fn grok_requires_the_providers_own_key_variable() {
    let h = harness(true).await;
    let mut value = json!({"name":"Relay", "baseURL":"https://relay.invalid", "username":"admin@example.com", "password":"secret"});
    for variable in [None, Some("   "), Some("sk-not-a-variable")] {
        if let Some(name) = variable {
            value["apiKeyVariable"] = json!(name);
        }
        assert_eq!(
            h.app
                .save_provider(Pool::Grok, value.clone())
                .await
                .unwrap_err(),
            sub2api::API_KEY_VARIABLE_INVALID
        );
    }
    assert!(h.app.grok_providers().unwrap().is_empty());
    value["apiKeyVariable"] = json!("  TEAM_RELAY_TOKEN  ");
    let saved = h.app.save_provider(Pool::Grok, value).await.unwrap();
    assert_eq!(saved["apiKeyVariable"], "TEAM_RELAY_TOKEN");
    assert_eq!(
        h.app.grok_providers().unwrap()[0].api_key_variable,
        "TEAM_RELAY_TOKEN"
    );

    // Older records without a name stay editable without acquiring a key.
    let mut store = h.app.grok_load().unwrap();
    store.providers[0].api_key_variable.clear();
    h.app.grok_save(&store).unwrap();
    assert!(h.app.grok_providers().unwrap()[0]
        .api_key_variable
        .is_empty());
    assert!(h
        .app
        .grok_member_connection(&store.providers[0].id)
        .unwrap()
        .definition
        .env_key
        .is_none());
}

async fn grok_provider(h: &Harness, base: &str) -> String {
    let saved = h
        .app
        .save_provider(
            Pool::Grok,
            json!({"name": "Relay", "baseURL": format!("{base}/v1/"), "apiKeyVariable": "XAI_API_KEY",
                   "username": "admin@example.com", "password": "secret"}),
        )
        .await
        .unwrap();
    // Normalize the URL and keep the explicitly selected key variable.
    assert_eq!(saved["baseURL"], base);
    assert_eq!(saved["apiKeyVariable"], "XAI_API_KEY");
    saved["id"].as_str().unwrap().to_owned()
}
fn sub2api_cards(accounts: Vec<AccountSummary>) -> Vec<AccountSummary> {
    accounts
        .into_iter()
        .filter(|a| crate::sub2api::is_card(Pool::Grok, &a.id))
        .collect()
}

// Sub2api 0.1.177 and 0.2.24 report the weekly progress, 0.1.160 only the
// billing it comes from: the cards read alike. 0.1.150 has neither, and its
// cards name its version.
#[tokio::test]
async fn sub2api_grok_cards_show_the_weekly_usage_of_old_and_new_releases() {
    let h = harness(true).await;
    let requests = Arc::new(Mutex::new(Vec::new()));
    let version = Arc::new(Mutex::new("0.2.24"));
    let base = sub2api_server(requests.clone(), version.clone()).await;
    let id = grok_provider(&h, &base).await;
    let reset = chrono::DateTime::parse_from_rfc3339("2099-01-08T00:00:00Z")
        .unwrap()
        .timestamp();
    for release in ["0.2.24", "0.1.177", "0.1.160", "0.1.150"] {
        *version.lock().unwrap() = release;
        h.app.refresh_provider(Pool::Grok, &id, true).await.unwrap();
        let cards = sub2api_cards(cards(&h.app).await);
        assert_eq!(cards.len(), 2, "{release}");
        let lapsed = cards
            .iter()
            .find(|c| c.email.as_deref() == Some("lapsed@example.com"))
            .unwrap();
        assert_eq!(
            lapsed.usage_error.as_deref(),
            Some("The account must sign in to Sub2api again")
        );
        assert_eq!(lapsed.plan_type.as_deref(), Some("x premium plus"));
        let card = cards
            .iter()
            .find(|c| c.email.as_deref() == Some("super@example.com"))
            .unwrap();
        let usage = card.usage.as_ref().unwrap();
        assert_eq!(card.label, "super@example.com");
        if release == "0.1.150" {
            assert!(usage.one_week.is_none(), "{release}");
            assert_eq!(usage.plan_type.as_deref(), Some("supergrok heavy"));
            assert_eq!(card.plan_type, usage.plan_type);
            assert!(card.usage_error.is_none(), "{release}");
            assert_eq!(card.outdated_sub2api_version.as_deref(), Some("0.1.150"));
            continue;
        }
        let week = usage.one_week.as_ref().unwrap();
        assert_eq!(
            (week.used_percent, week.reset_at, week.window_seconds),
            (7.0, Some(reset), 7 * 24 * 60 * 60),
            "{release}"
        );
        assert!(usage.one_month.is_none() && usage.five_hour.is_none());
        assert_eq!(usage.plan_type.as_deref(), Some("SuperGrok"));
        assert_eq!(card.outdated_sub2api_version, None, "{release}");
    }
    // Usage is read without probing a model, which would spend quota.
    assert!(requests
        .lock()
        .unwrap()
        .iter()
        .all(|uri| !uri.contains("/quota")));
}

// Grok Build's configuration, environment and the Sub2api server are all
// temporary: no real `~/.grok`, shell or relay is used.
#[tokio::test]
async fn a_sub2api_card_gives_grok_builds_models_the_gateway_and_an_account_takes_it_back() {
    let h = harness(true).await;
    h.sign_in(&sign_in_of("alice", "access-alice", later()));
    h.app.import_current(Pool::Grok).await.unwrap();
    let alice = cards(&h.app).await.remove(0);
    assert!(alice.is_current);
    let config = h.auth.with_file_name("config.toml");
    let original =
        "[models]\ndefault = \"grok-4.7\"\nweb_search = \"grok-4.5\"\n\n[model.\"grok-4.7\"]\ntemperature = 0.3\n";
    fs::write(&config, original).unwrap();
    fs::write(
        h.auth.with_file_name("models_cache.json"),
        r#"{"models": {"grok-4.7": {"info": {"id": "grok-4.7", "model": "grok-4.7"}}}}"#,
    )
    .unwrap();
    let version = Arc::new(Mutex::new("0.1.177"));
    let base = sub2api_server(Arc::new(Mutex::new(Vec::new())), version).await;
    let id = grok_provider(&h, &base).await;
    h.app.refresh_provider(Pool::Grok, &id, true).await.unwrap();
    let card = sub2api_cards(cards(&h.app).await)
        .into_iter()
        .find(|c| c.email.as_deref() == Some("super@example.com"))
        .unwrap();
    // 0.1.177 lists no grok-4.7, but forwards it as it is.
    *h.fake.gateway.lock().unwrap() = Some(json!({"object": "list", "data": [
        {"id": "grok-4.6"}, {"id": "grok-3-mini"}, {"id": "grok-imagine-image"}
    ]}));

    // Without the variable exported, nothing changes.
    assert_eq!(
        h.app
            .switch_account(Pool::Grok, &card.id)
            .await
            .unwrap_err(),
        sub2api::API_KEY_MISSING
    );
    // A key Sub2api refuses is refused before Grok Build is changed.
    let key = Arc::new(Mutex::new("wrong"));
    let exported = key.clone();
    // The provider's address, edited while the shell is asked.
    let moved = Arc::new(Mutex::new(None::<String>));
    let (edit, store_file) = (
        moved.clone(),
        h.app.paths.data_dir.join("grok-accounts.json"),
    );
    h.app.set_grok_environment(Arc::new(move |name: &str| {
        if let Some(url) = edit.lock().unwrap().as_deref() {
            let mut store: Value = serde_json::from_slice(&fs::read(&store_file).unwrap()).unwrap();
            store["providers"][0]["baseURL"] = url.into();
            fs::write(&store_file, store.to_string()).unwrap();
        }
        (name == "XAI_API_KEY").then(|| (*exported.lock().unwrap()).to_owned())
    }));
    assert!(h
        .app
        .switch_account(Pool::Grok, &card.id)
        .await
        .unwrap_err()
        .contains("HTTP 401"));
    assert_eq!(fs::read_to_string(&config).unwrap(), original);

    *key.lock().unwrap() = "sub2api-key";
    // A provider edited during its switch leaves Grok Build as it was.
    *moved.lock().unwrap() = Some("https://moved.invalid".into());
    assert_eq!(
        h.app
            .switch_account(Pool::Grok, &card.id)
            .await
            .unwrap_err(),
        "The Grok provider changed during the switch; switch again"
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    *moved.lock().unwrap() = None;
    h.app
        .save_provider(
            Pool::Grok,
            json!({"id": id, "name": "Relay", "baseURL": base, "apiKeyVariable": "XAI_API_KEY",
                   "username": "admin@example.com"}),
        )
        .await
        .unwrap();
    // Every model it would route names an endpoint of the user's own: the
    // switch is refused, and Grok Build left as it was.
    let mut taken = "[models]\nweb_search = \"grok-4.5\"\n".to_owned();
    for model in ["grok-4.7", "grok-4.5", "grok-4.6", "grok-3-mini"] {
        taken += &format!("\n[model.\"{model}\"]\nbase_url = \"http://localhost:11434/v1\"\n");
    }
    fs::write(&config, &taken).unwrap();
    assert_eq!(
        h.app
            .switch_account(Pool::Grok, &card.id)
            .await
            .unwrap_err(),
        sub2api::MODELS_TAKEN
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), taken);
    assert!(h.app.grok_load().unwrap().config_record.is_none());
    fs::write(&config, original).unwrap();
    let execution = h.app.switch_account(Pool::Grok, &card.id).await.unwrap();
    assert_eq!(execution.provider.as_deref(), Some("Relay"));
    let switched: toml_edit::DocumentMut = fs::read_to_string(&config).unwrap().parse().unwrap();
    assert_eq!(
        switched["endpoints"]["models_base_url"].as_str(),
        Some(format!("{base}/v1").as_str())
    );
    assert_eq!(
        switched["endpoints"]["models_list_url"].as_str(),
        Some(format!("{base}/v1/models").as_str())
    );
    assert_eq!(
        switched["auth"]["preferred_method"].as_str(),
        Some("api_key")
    );
    // Global model discovery needs no overrides, including for existing roles.
    let before: toml_edit::DocumentMut = original.parse().unwrap();
    assert_eq!(switched["model"].to_string(), before["model"].to_string());
    assert_eq!(switched["models"].to_string(), before["models"].to_string());
    // The login stays for an account's switch; the provider's card is the
    // one in use.
    assert_eq!(
        login::email(&h.live()).as_deref(),
        Some("alice@example.com")
    );
    let accounts = cards(&h.app).await;
    assert!(
        !accounts
            .iter()
            .find(|a| a.id == alice.id)
            .unwrap()
            .is_current
    );
    assert!(
        accounts
            .iter()
            .find(|a| a.id == card.id)
            .unwrap()
            .is_current
    );

    // Editing the active provider does not apply its new route. A refused
    // switch must leave both the old file and the edited card's status honest.
    let old_config = fs::read_to_string(&config).unwrap();
    let change = |url: &str, variable: &str| {
        json!({
            "id": id, "name": "Relay", "baseURL": url, "adminBaseURL": base,
            "apiKeyVariable": variable, "username": "admin@example.com"
        })
    };
    let coport = "http://127.0.0.1:8787/v1";
    h.app
        .save_provider(Pool::Grok, change(coport, "XAI_API_KEY"))
        .await
        .unwrap();
    assert!(!cards(&h.app).await.iter().any(|a| a.is_current));
    *key.lock().unwrap() = "rejected";
    assert!(h
        .app
        .switch_account(Pool::Grok, &card.id)
        .await
        .unwrap_err()
        .contains("401"));
    assert_eq!(fs::read_to_string(&config).unwrap(), old_config);
    assert!(!cards(&h.app).await.iter().any(|a| a.is_current));
    h.app
        .save_provider(Pool::Grok, change(&base, "ANOTHER_KEY"))
        .await
        .unwrap();
    assert!(!cards(&h.app).await.iter().any(|a| a.is_current));
    h.app
        .save_provider(Pool::Grok, change(coport, "XAI_API_KEY"))
        .await
        .unwrap();
    *key.lock().unwrap() = "sub2api-key";
    h.app.switch_account(Pool::Grok, &card.id).await.unwrap();
    let applied: toml_edit::DocumentMut = fs::read_to_string(&config).unwrap().parse().unwrap();
    assert_eq!(
        applied["endpoints"]["models_base_url"].as_str(),
        Some(coport)
    );
    assert!(cards(&h.app)
        .await
        .iter()
        .any(|a| a.id == card.id && a.is_current));

    // An account's switch puts Grok Build's configuration back.
    h.app.switch_account(Pool::Grok, &alice.id).await.unwrap();
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    let accounts = cards(&h.app).await;
    assert!(
        accounts
            .iter()
            .find(|a| a.id == alice.id)
            .unwrap()
            .is_current
    );
    assert!(
        !accounts
            .iter()
            .find(|a| a.id == card.id)
            .unwrap()
            .is_current
    );
}

#[tokio::test]
async fn coport_subscription_endpoints_survive_switches_to_both_sub2api_key_modes() {
    let h = harness(true).await;
    h.sign_in(&sign_in_of("alice", "access-alice", later()));
    h.app.import_current(Pool::Grok).await.unwrap();
    let subscription = cards(&h.app).await.remove(0);
    let config = h.auth.with_file_name("config.toml");
    let listener = "http://127.0.0.1:8787/grok/v1";
    let original = format!(
        "[endpoints]\ncli_chat_proxy_base_url = '{listener}'\nxai_api_base_url = '{listener}' # Coport\n"
    );
    fs::write(&config, &original).unwrap();
    let base = sub2api_server(Arc::default(), Arc::new(Mutex::new("0.1.177"))).await;
    let id = grok_provider(&h, &base).await;
    h.app.refresh_provider(Pool::Grok, &id, true).await.unwrap();
    let card = sub2api_cards(cards(&h.app).await)
        .into_iter()
        .find(|c| c.available)
        .unwrap();
    let gateway = "http://127.0.0.1:8787/grok/https://relay.example:6060/v1";
    *h.fake.gateway.lock().unwrap() = Some(json!({"data": [{"id":"grok-test"}]}));
    for variable in ["XAI_API_KEY", "SUB2API_KEY"] {
        h.app.set_grok_environment(Arc::new(move |name| {
            (name == variable).then(|| "sub2api-key".into())
        }));
        h.app
            .save_provider(
                Pool::Grok,
                json!({
                    "id":id, "name":"Relay", "baseURL":gateway.trim_end_matches("/v1"),
                    "apiKeyVariable":variable, "username":"admin@example.com"
                }),
            )
            .await
            .unwrap();
        assert_eq!(
            h.app.grok_member_connection(&id).unwrap().admin_base,
            "https://relay.example:6060/api/v1"
        );
        h.app.switch_account(Pool::Grok, &card.id).await.unwrap();
        assert!(h
            .fake
            .calls()
            .iter()
            .any(|call| call.starts_with(&format!("models {gateway}/models "))));
        let doc: toml_edit::DocumentMut = fs::read_to_string(&config).unwrap().parse().unwrap();
        let endpoints = &doc["endpoints"];
        assert_eq!(
            endpoints["cli_chat_proxy_base_url"].as_str(),
            Some(listener)
        );
        if variable == "XAI_API_KEY" {
            assert_eq!(endpoints["models_base_url"].as_str(), Some(gateway));
            assert_eq!(
                endpoints["models_list_url"].as_str(),
                Some(format!("{gateway}/models").as_str())
            );
            assert_eq!(endpoints["xai_api_base_url"].as_str(), Some(gateway));
            assert_eq!(doc["auth"]["preferred_method"].as_str(), Some("api_key"));
            assert!(doc.get("model").is_none());
        } else {
            assert_eq!(endpoints["xai_api_base_url"].as_str(), Some(listener));
            assert!(endpoints.get("models_base_url").is_none());
            let model = &doc["model"]["grok-test"];
            assert_eq!(model["env_key"].as_str(), Some(variable));
            assert_eq!(model["base_url"].as_str(), Some(gateway));
            assert_eq!(model["api_backend"].as_str(), Some("responses"));
        }
        assert!(cards(&h.app)
            .await
            .iter()
            .any(|c| c.id == card.id && c.is_current));
        h.app
            .switch_account(Pool::Grok, &subscription.id)
            .await
            .unwrap();
        assert_eq!(fs::read_to_string(&config).unwrap(), original);
        assert!(cards(&h.app)
            .await
            .iter()
            .any(|c| c.id == subscription.id && c.is_current));
    }
}
