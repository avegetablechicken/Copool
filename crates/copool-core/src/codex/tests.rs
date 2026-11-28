use super::workspaces::Metadata;
use super::*;
use crate::{codex::api::AccountNetwork, pool::Response};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::sync::Mutex;

/// A token, its claims readable: `token` names it to the tests.
fn token(name: &str, expires: Option<i64>) -> String {
    let mut claims = json!({"token": name});
    if let Some(expires) = expires {
        claims["exp"] = expires.into();
    }
    format!(
        "e30.{}.test",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
    )
}
fn name_of(token: &str) -> Option<String> {
    let claims = codex::decode_jwt_payload(token).ok()?;
    claims["token"].as_str().map(str::to_owned)
}
fn timestamp(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .unwrap()
        .to_rfc3339()
}

/// ChatGPT, answering by the access token: those named "expired..." are
/// refused, and a renewal of `user`'s login hands out "fresh-<user>".
#[derive(Default)]
struct Fake {
    renewals: Mutex<usize>,
    /// What Codex writes to `auth.json` while a renewal is asked.
    meanwhile: Mutex<Option<(std::path::PathBuf, Value)>>,
}
impl AccountNetwork for Fake {
    fn usage<'a>(&'a self, auth: &'a Value, _: &'a str) -> Response<'a, UsageSnapshot> {
        Box::pin(async move {
            let access = codex::extract_auth(auth)?.access_token;
            if name_of(&access).is_some_and(|n| n.starts_with("expired")) {
                return Err("token_expired".into());
            }
            Ok(
                serde_json::from_value(json!({"fetchedAt": now_unix_seconds(),
                    "fiveHour": {"usedPercent": 10, "windowSeconds": 18000},
                    "oneWeek": {"usedPercent": 20, "windowSeconds": 604800}}))
                .unwrap(),
            )
        })
    }
    fn refresh<'a>(&'a self, auth: &'a Value, _: &'a str) -> Response<'a, Value> {
        *self.renewals.lock().unwrap() += 1;
        if let Some((path, document)) = self.meanwhile.lock().unwrap().take() {
            write_json(&path, &document).unwrap();
        }
        Box::pin(async move {
            let user = codex::extract_auth(auth)?.principal_id.unwrap_or_default();
            let mut renewed = auth.clone();
            let now = now_unix_seconds();
            renewed["tokens"]["access_token"] =
                token(&format!("fresh-{user}"), Some(now + 3600)).into();
            renewed["last_refresh"] = timestamp(now).into();
            Ok(renewed)
        })
    }
    fn workspaces<'a>(&'a self, _: &'a str, _: &'a str) -> Response<'a, Vec<Metadata>> {
        Box::pin(async { Ok(vec![]) })
    }
}

/// The Codex pool as the store's suite drives it.
struct Fixture {
    app: Application,
    fake: std::sync::Arc<Fake>,
    _dir: tempfile::TempDir,
}
impl crate::login_store::suite::Fixture for Fixture {
    const POOL: Pool = Pool::Codex;
    const LIVE_UNREADABLE: &'static str = <CodexLogins as LoginPool>::LIVE_UNREADABLE;
    async fn open(request_new_accounts: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut app = Application::open(Paths {
            data_dir: dir.path().join("data"),
            codex_auth: dir.path().join("codex/auth.json"),
        })
        .unwrap();
        let fake = std::sync::Arc::new(Fake::default());
        app.network = fake.clone();
        let preferences = crate::Preferences {
            request_new_accounts,
            ..app.preferences().unwrap()
        };
        app.save_preferences(preferences).await.unwrap();
        Self {
            app,
            fake,
            _dir: dir,
        }
    }
    fn app(&self) -> &Application {
        &self.app
    }
    /// `user`'s login to their workspace, refreshed an hour before its
    /// token expires.
    fn login(user: &str, access: &str, expires: i64) -> Value {
        let id = format!(
            "e30.{}.test",
            URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(
                    &json!({"sub": user, "email": format!("{user}@example.test"),
                        "https://api.openai.com/auth": {"chatgpt_account_id": format!("ws-{user}"),
                            "chatgpt_plan_type": "plus"}})
                )
                .unwrap()
            )
        );
        json!({"auth_mode": "chatgpt", "last_refresh": timestamp(expires - 3600),
                "tokens": {"access_token": token(access, Some(expires)),
                    "refresh_token": format!("refresh-{user}"), "id_token": id,
                    "account_id": format!("ws-{user}")}})
    }
    fn with_refresh(login: &Value, refresh: &str) -> Value {
        let mut login = login.clone();
        login["tokens"]["refresh_token"] = refresh.into();
        login
    }
    fn without_expiry(login: &Value) -> Value {
        let mut login = login.clone();
        let access = login["tokens"]["access_token"].as_str().unwrap().to_owned();
        login["tokens"]["access_token"] = token(&name_of(&access).unwrap(), None).into();
        login
    }
    fn no_login() -> Value {
        json!({"tokens": {"access_token": "", "account_id": "ws-x"}})
    }
    fn access(document: &Value) -> Option<String> {
        name_of(&CodexLogins::access_token(document)?)
    }
    fn renewed(user: &str) -> String {
        format!("fresh-{user}")
    }
    fn sign_in(&self, login: &Value) {
        write_json(&self.app.paths.codex_auth, login).unwrap();
    }
    fn live(&self) -> Value {
        self.app.codex_read_live().unwrap().unwrap()
    }
    fn corrupt_live(&self) {
        fs::write(&self.app.paths.codex_auth, b"not a login").unwrap();
    }
    fn delay_live_reads(&self, ms: u64) {
        self.app
            .codex_live_read_delay_ms
            .store(ms, std::sync::atomic::Ordering::Relaxed);
    }
    fn meanwhile(&self, login: Value) {
        *self.fake.meanwhile.lock().unwrap() = Some((self.app.paths.codex_auth.clone(), login));
    }
    fn renewals(&self) -> usize {
        *self.fake.renewals.lock().unwrap()
    }
    fn id_of(&self, user: &str) -> String {
        let email = format!("{user}@example.test");
        self.app
            .codex_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| a.email.as_deref() == Some(email.as_str()))
            .unwrap()
            .id
    }
    fn stored(&self, id: &str) -> Value {
        self.app
            .codex_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| a.id == id)
            .unwrap()
            .document
    }
    fn current(&self) -> Option<String> {
        self.app.codex_load().unwrap().current_account_id
    }
}
crate::login_store::login_store_suite!(super::Fixture);

/// An account file's document (`{"accounts": [...]}`, as an earlier
/// version or a remote node keeps it) imports each account's login,
/// `authJson`, and nothing else of it.
#[tokio::test]
async fn an_account_files_logins_are_imported_without_the_rest() {
    use crate::login_store::suite::Fixture as _;
    let pool = Fixture::open(false).await;
    let expires = now_unix_seconds() + 3600;
    let document = json!({"version": 1, "currentAccountId": "b", "accounts": [
            {"id": "a", "label": "Kept label", "proxyURL": "socks5://127.0.0.1:1080",
                "authJson": Fixture::login("alice", "access-alice", expires)},
            {"id": "b", "authJson": Fixture::login("bob", "access-bob", expires)}]});
    let app = pool.app();
    assert_eq!(
        app.codex_import_json(&document.to_string()).await.unwrap(),
        2
    );
    let store = app.codex_load().unwrap();
    let mut emails: Vec<_> = store
        .accounts
        .iter()
        .filter_map(|a| a.email.clone())
        .collect();
    emails.sort();
    assert_eq!(emails, ["alice@example.test", "bob@example.test"]);
    let alice = store
        .accounts
        .iter()
        .find(|a| a.email.as_deref() == Some("alice@example.test"))
        .unwrap();
    assert_ne!(alice.id, "a");
    assert_ne!(alice.label, "Kept label");
    assert_eq!(alice.proxy_url, "");
    assert_eq!(store.current_account_id, None);
    // An entry without a usable login fails the whole document.
    let broken = json!({"accounts": [{"authJson": Fixture::no_login()}]});
    assert!(app.codex_import_json(&broken.to_string()).await.is_err());
    assert_eq!(app.codex_load().unwrap().accounts.len(), 2);
}

/// The accounts `accounts.json` kept move to the pool's file once, as
/// they were: their workspace, standing, renewal choice and wait, the
/// account current, the providers and the workspace directory.
/// `accounts.json` stays as it was.
#[test]
fn the_accounts_json_store_moves_to_the_pools_file_once() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        data_dir: dir.path().join("data"),
        codex_auth: dir.path().join("codex/auth.json"),
    };
    let login = |user: &str| {
        <Fixture as crate::login_store::suite::Fixture>::login(
            user,
            "access",
            now_unix_seconds() + 3600,
        )
    };
    let old = json!({"version": 1, "currentAccountId": "b", "accounts": [
            {"id": "a", "label": "A", "accountId": "old-ws", "authJson": login("alice"),
                "addedAt": 1, "updatedAt": 2, "renewal": "never", "firstRequestPending": true,
                "teamName": "Team A", "proxyURL": "socks5://127.0.0.1:1080"},
            {"id": "b", "label": "B", "accountId": "ws-bob", "authJson": login("bob"),
                "addedAt": 1, "updatedAt": 2, "workspaceStatus": "deactivated",
                "displayStatus": "deactivated"},
            {"id": "c", "label": "C", "accountId": "ws-carol", "authJson": login("carol"),
                "addedAt": 1, "updatedAt": 2, "workspaceStatus": "deactivated",
                "displayStatus": "deleted"}],
            "providers": [{"id": "p", "name": "my"}],
            "workspaceDirectory": [{"workspaceId": "ws-x", "visibility": "deleted"}],
            "currentSelection": {"cardId": "b", "selectedAt": 1, "sourceDeviceId": "macos-local"}});
    fs::create_dir_all(&paths.data_dir).unwrap();
    write_json(&paths.store(), &old).unwrap();
    let before = fs::read(paths.store()).unwrap();
    migrate(&paths).unwrap();
    let store = load(&paths).unwrap();
    let a = &store.accounts[0];
    // The login names its workspace; the rest is as kept.
    assert_eq!(a.own.workspace_id, "ws-alice");
    assert_eq!(a.own.team_name.as_deref(), Some("Team A"));
    assert_eq!(a.email.as_deref(), Some("alice@example.test"));
    assert_eq!(a.renewal, Some(Renewal::Never));
    assert!(a.first_request_pending);
    assert_eq!(a.proxy_url, "socks5://127.0.0.1:1080");
    assert_eq!(a.standing, None);
    assert_eq!(store.accounts[1].standing, Some(Standing::Deactivated));
    assert_eq!(store.accounts[2].standing, Some(Standing::Deleted));
    assert_eq!(store.current_account_id.as_deref(), Some("b"));
    assert_eq!(store.providers[0].name, "my");
    assert_eq!(store.workspace_directory[0]["workspaceId"], "ws-x");
    assert_eq!(fs::read(paths.store()).unwrap(), before);
    // Once: a later run keeps what the pool's file holds.
    write_json(&paths.store(), &json!({"accounts": []})).unwrap();
    migrate(&paths).unwrap();
    assert_eq!(load(&paths).unwrap().accounts.len(), 3);
}

/// A deactivated workspace's account is listed so, cannot be switched
/// to nor used by the proxy, and once deleted stays hidden, its
/// workspace remembered as gone.
#[tokio::test]
async fn a_deactivated_workspace_stays_listed_until_deleted_then_hidden() {
    use crate::login_store::suite::Fixture as _;
    let f = Fixture::open(true).await;
    let login = Fixture::login("alice", "access-alice", now_unix_seconds() + 3600);
    f.app
        .import_json(Pool::Codex, &login.to_string())
        .await
        .unwrap();
    let id = f.id_of("alice");
    f.app
        .codex_logins()
        .change(|store| {
            let account = &mut store.accounts[0];
            Logins::<CodexLogins>::record_usage(account, &Err("workspace_deactivated".into()), 1);
            Ok(())
        })
        .await
        .unwrap();
    let card = &f.app.accounts(Pool::Codex).await.unwrap()[0];
    assert_eq!(
        (card.display_status.as_str(), card.available),
        ("deactivated", false)
    );
    assert!(f.app.switch_account(Pool::Codex, &id).await.is_err());
    assert!(proxy::proxy_candidates(&f.app.paths).unwrap().is_empty());
    f.app.delete_account(Pool::Codex, &id).await.unwrap();
    let store = f.app.codex_load().unwrap();
    assert_eq!(store.accounts[0].standing, Some(Standing::Deleted));
    assert_eq!(store.workspace_directory[0]["workspaceId"], "ws-alice");
    assert_eq!(store.workspace_directory[0]["visibility"], "deleted");
    // Deleted once more, it is gone.
    f.app.delete_account(Pool::Codex, &id).await.unwrap();
    assert!(f.app.codex_load().unwrap().accounts.is_empty());
}

#[tokio::test]
async fn switches_run_their_effects_in_the_order_they_were_made() {
    use crate::login_store::suite::Fixture as _;
    use std::sync::Arc;
    let fixture = Fixture::open(false).await;
    let expires = now_unix_seconds() + 3600;
    fixture
        .app
        .import_json(
            Pool::Codex,
            &json!([
                Fixture::login("alice", "access-alice", expires),
                Fixture::login("bob", "access-bob", expires)
            ])
            .to_string(),
        )
        .await
        .unwrap();
    let alice = fixture.id_of("alice");
    let bob = fixture.id_of("bob");
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let synced = Arc::new(Mutex::new(String::new()));
    let (entered_effect, release_effect, synced_effect) =
        (entered.clone(), release.clone(), synced.clone());
    fixture
        .app
        .on_switch_effects(Arc::new(move |_, _, credential| {
            let (entered, release, synced) = (
                entered_effect.clone(),
                release_effect.clone(),
                synced_effect.clone(),
            );
            Box::pin(async move {
                let name = name_of(&credential.unwrap().unwrap().access).unwrap();
                if name == "access-alice" {
                    entered.notify_one();
                    release.notified().await;
                }
                *synced.lock().unwrap() = name;
                Ok(crate::SwitchExecution::default())
            })
        }));
    let (first, second) = tokio::join!(fixture.app.switch_account(Pool::Codex, &alice), async {
        entered.notified().await;
        // Alice's effects end well after Bob's switch would have run its own.
        let release = async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            release.notify_one();
        };
        tokio::join!(fixture.app.switch_account(Pool::Codex, &bob), release).0
    });
    first.unwrap();
    second.unwrap();
    assert_eq!(
        Fixture::access(&fixture.live()).as_deref(),
        Some("access-bob")
    );
    assert_eq!(*synced.lock().unwrap(), "access-bob");
}

#[tokio::test]
async fn a_renewal_is_kept_though_its_refresh_is_cancelled() {
    use crate::login_store::suite::Fixture as _;
    struct RotationThenSlowUsage;
    impl AccountNetwork for RotationThenSlowUsage {
        fn usage<'a>(&'a self, _: &'a Value, _: &'a str) -> Response<'a, UsageSnapshot> {
            Box::pin(std::future::pending())
        }
        fn refresh<'a>(&'a self, auth: &'a Value, _: &'a str) -> Response<'a, Value> {
            Box::pin(async move {
                let mut next = auth.clone();
                next["tokens"]["access_token"] =
                    token("fresh-alice", Some(now_unix_seconds() + 3600)).into();
                next["tokens"]["refresh_token"] = "rotated-refresh-alice".into();
                next["last_refresh"] = timestamp(now_unix_seconds()).into();
                Ok(next)
            })
        }
        fn workspaces<'a>(&'a self, _: &'a str, _: &'a str) -> Response<'a, Vec<Metadata>> {
            Box::pin(async { Ok(vec![]) })
        }
    }
    let mut fixture = Fixture::open(false).await;
    fixture
        .app
        .import_json(
            Pool::Codex,
            &Fixture::login("alice", "expired-alice", now_unix_seconds() - 1).to_string(),
        )
        .await
        .unwrap();
    let id = fixture.id_of("alice");
    fixture
        .app
        .release_new_accounts(Pool::Codex, Some(&id))
        .await
        .unwrap();
    fixture.app.network = std::sync::Arc::new(RotationThenSlowUsage);
    // As a greeting's deadline cancels it while its usage request waits.
    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(30),
        fixture.app.say_hi(Pool::Codex, &id)
    )
    .await
    .is_err());
    assert_eq!(
        fixture.stored(&id)["tokens"]["refresh_token"],
        "rotated-refresh-alice"
    );
}

#[tokio::test]
async fn a_deleted_account_is_never_refreshed_in_the_background() {
    use crate::login_store::suite::Fixture as _;
    let fixture = Fixture::open(false).await;
    fixture
        .app
        .import_json(
            Pool::Codex,
            &Fixture::login("alice", "expired-alice", now_unix_seconds() - 1).to_string(),
        )
        .await
        .unwrap();
    let id = fixture.id_of("alice");
    fixture
        .app
        .release_new_accounts(Pool::Codex, Some(&id))
        .await
        .unwrap();
    fixture
        .app
        .codex_logins()
        .change(|store| {
            store.accounts[0].standing = Some(Standing::Deactivated);
            store.accounts[0].updated_at = 0;
            Ok(())
        })
        .await
        .unwrap();
    fixture.app.delete_account(Pool::Codex, &id).await.unwrap();
    assert_eq!(
        fixture.app.codex_load().unwrap().accounts[0].standing,
        Some(Standing::Deleted)
    );
    fixture.app.refresh_recurring(Pool::Codex, 0).await.unwrap();
    assert_eq!(fixture.renewals(), 0);
}

#[tokio::test]
async fn a_renewal_cancelled_waiting_for_the_store_is_saved_next() {
    use crate::login_store::suite::Fixture as _;
    use std::sync::Arc;
    struct Network {
        started: Arc<tokio::sync::Notify>,
        answer: Arc<tokio::sync::Notify>,
        rotated: Arc<std::sync::atomic::AtomicBool>,
    }
    impl AccountNetwork for Network {
        fn usage<'a>(&'a self, _: &'a Value, _: &'a str) -> Response<'a, UsageSnapshot> {
            Box::pin(std::future::pending())
        }
        fn refresh<'a>(&'a self, auth: &'a Value, _: &'a str) -> Response<'a, Value> {
            Box::pin(async move {
                self.started.notify_one();
                self.answer.notified().await;
                let mut next = auth.clone();
                next["tokens"]["access_token"] =
                    token("fresh-alice", Some(now_unix_seconds() + 3600)).into();
                next["tokens"]["refresh_token"] = "rotated-refresh-alice".into();
                next["last_refresh"] = timestamp(now_unix_seconds()).into();
                self.rotated
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(next)
            })
        }
        fn workspaces<'a>(&'a self, _: &'a str, _: &'a str) -> Response<'a, Vec<Metadata>> {
            Box::pin(async { Ok(vec![]) })
        }
    }
    let mut fixture = Fixture::open(false).await;
    fixture
        .app
        .import_json(
            Pool::Codex,
            &Fixture::login("alice", "expired-alice", now_unix_seconds() - 1).to_string(),
        )
        .await
        .unwrap();
    let id = fixture.id_of("alice");
    fixture
        .app
        .release_new_accounts(Pool::Codex, Some(&id))
        .await
        .unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let answer = Arc::new(tokio::sync::Notify::new());
    let rotated = Arc::new(std::sync::atomic::AtomicBool::new(false));
    fixture.app.network = Arc::new(Network {
        started: started.clone(),
        answer: answer.clone(),
        rotated: rotated.clone(),
    });
    let cancelled = Arc::new(tokio::sync::Notify::new());
    tokio::join!(
        async {
            assert!(tokio::time::timeout(
                std::time::Duration::from_millis(50),
                fixture.app.say_hi(Pool::Codex, &id)
            )
            .await
            .is_err());
            cancelled.notify_one();
        },
        async {
            started.notified().await;
            let _store = fixture.app.codex.store.lock().await;
            answer.notify_one();
            cancelled.notified().await;
        }
    );
    // The issuer rotated the token before the refresh was cancelled.
    assert!(rotated.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        fixture.stored(&id)["tokens"]["refresh_token"],
        "rotated-refresh-alice"
    );
    // The next save, whatever it changes, writes it to the file.
    fixture.app.codex_logins().change(|_| Ok(())).await.unwrap();
    let file: Value = serde_json::from_slice(
        &std::fs::read(fixture.app.paths.data_dir.join("codex-accounts.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        file["accounts"][0]["document"]["tokens"]["refresh_token"],
        "rotated-refresh-alice"
    );
}

#[tokio::test]
async fn refreshing_the_whole_pool_leaves_out_a_deleted_account() {
    use crate::login_store::suite::Fixture as _;
    let fixture = Fixture::open(false).await;
    fixture
        .app
        .import_json(
            Pool::Codex,
            &Fixture::login("alice", "expired-alice", now_unix_seconds() - 1).to_string(),
        )
        .await
        .unwrap();
    let id = fixture.id_of("alice");
    fixture
        .app
        .release_new_accounts(Pool::Codex, Some(&id))
        .await
        .unwrap();
    fixture
        .app
        .codex_logins()
        .change(|store| {
            store.accounts[0].standing = Some(Standing::Deactivated);
            store.accounts[0].updated_at = 0;
            Ok(())
        })
        .await
        .unwrap();
    fixture.app.delete_account(Pool::Codex, &id).await.unwrap();
    fixture.app.refresh_now(Pool::Codex, None).await.unwrap();
    assert_eq!(fixture.renewals(), 0);
}
#[tokio::test]
async fn an_unsaved_renewal_gives_way_to_a_newer_login_saved_since() {
    use crate::login_store::suite::Fixture as _;
    use std::sync::Arc;
    struct Network {
        started: Arc<tokio::sync::Notify>,
        answer: Arc<tokio::sync::Notify>,
        rotated: Arc<std::sync::atomic::AtomicBool>,
    }
    impl AccountNetwork for Network {
        fn usage<'a>(&'a self, _: &'a Value, _: &'a str) -> Response<'a, UsageSnapshot> {
            Box::pin(std::future::pending())
        }
        fn refresh<'a>(&'a self, auth: &'a Value, _: &'a str) -> Response<'a, Value> {
            Box::pin(async move {
                self.started.notify_one();
                self.answer.notified().await;
                let mut next = auth.clone();
                next["tokens"]["access_token"] =
                    token("fresh-alice", Some(now_unix_seconds() + 3600)).into();
                next["tokens"]["refresh_token"] = "rotated-refresh-alice".into();
                next["last_refresh"] = timestamp(now_unix_seconds()).into();
                self.rotated
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(next)
            })
        }
        fn workspaces<'a>(&'a self, _: &'a str, _: &'a str) -> Response<'a, Vec<Metadata>> {
            Box::pin(async { Ok(vec![]) })
        }
    }
    let mut fixture = Fixture::open(false).await;
    fixture
        .app
        .import_json(
            Pool::Codex,
            &Fixture::login("alice", "expired-alice", now_unix_seconds() - 1).to_string(),
        )
        .await
        .unwrap();
    let id = fixture.id_of("alice");
    fixture
        .app
        .release_new_accounts(Pool::Codex, Some(&id))
        .await
        .unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let answer = Arc::new(tokio::sync::Notify::new());
    let rotated = Arc::new(std::sync::atomic::AtomicBool::new(false));
    fixture.app.network = Arc::new(Network {
        started: started.clone(),
        answer: answer.clone(),
        rotated: rotated.clone(),
    });
    let cancelled = Arc::new(tokio::sync::Notify::new());
    tokio::join!(
        async {
            assert!(tokio::time::timeout(
                std::time::Duration::from_millis(50),
                fixture.app.say_hi(Pool::Codex, &id)
            )
            .await
            .is_err());
            cancelled.notify_one();
        },
        async {
            started.notified().await;
            let _store = fixture.app.codex.store.lock().await;
            answer.notify_one();
            cancelled.notified().await;
        }
    );
    // The issuer rotated the token before the refresh was cancelled.
    assert!(rotated.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        fixture.stored(&id)["tokens"]["refresh_token"],
        "rotated-refresh-alice"
    );
    // The next save, whatever it changes, writes it to the file.
    let newer = Fixture::login("alice", "newer-alice", now_unix_seconds() + 7200);
    fixture
        .app
        .codex_logins()
        .change(|store| {
            store.accounts[0].document = newer;
            Ok(())
        })
        .await
        .unwrap();
    let file: Value = serde_json::from_slice(
        &std::fs::read(fixture.app.paths.data_dir.join("codex-accounts.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        file["accounts"][0]["document"]["tokens"]["refresh_token"],
        "refresh-alice"
    );
}

#[tokio::test]
async fn a_deleted_account_has_no_scheduled_greeting() {
    use crate::login_store::suite::Fixture as _;
    use axum::{extract::Request, routing::any, Router};
    use std::sync::Arc;
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let said = Arc::new(tokio::sync::Notify::new());
    let (recorder, signal) = (seen.clone(), said.clone());
    let router = Router::new().fallback(any(move |request: Request| {
        let (recorder, signal) = (recorder.clone(), signal.clone());
        async move {
            let path = request.uri().path().to_owned();
            recorder.lock().unwrap().push(path.clone());
            if path.ends_with("/models") {
                json!({"models": [{"slug": "gpt-5.6-luna", "supported_in_api": true}]}).to_string()
            } else {
                signal.notify_one();
                "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n".to_owned()
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let fixture = Fixture::open(false).await;
    fixture
        .app
        .import_json(
            Pool::Codex,
            &Fixture::login("alice", "access-alice", now_unix_seconds() + 3600).to_string(),
        )
        .await
        .unwrap();
    let id = fixture.id_of("alice");
    fixture
        .app
        .release_new_accounts(Pool::Codex, Some(&id))
        .await
        .unwrap();
    fixture
        .app
        .save_account_proxy(Pool::Codex, &id, "direct://")
        .await
        .unwrap();
    std::fs::create_dir_all(fixture.app.codex_config_path().parent().unwrap()).unwrap();
    std::fs::write(
        fixture.app.codex_config_path(),
        format!("chatgpt_base_url = \"{base}\"\n"),
    )
    .unwrap();
    fixture
        .app
        .set_greeting_schedule(
            Pool::Codex,
            &id,
            false,
            vec![chrono::Local::now().format("%H:%M").to_string()],
        )
        .await
        .unwrap();
    fixture
        .app
        .codex_logins()
        .change(|store| {
            store.accounts[0].standing = Some(Standing::Deactivated);
            Ok(())
        })
        .await
        .unwrap();
    fixture.app.delete_account(Pool::Codex, &id).await.unwrap();
    assert_eq!(
        fixture.app.codex_load().unwrap().accounts[0].standing,
        Some(Standing::Deleted)
    );
    let app = fixture.app.into_shared();
    let task = tokio::spawn(app.background_greetings());
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), said.notified()).await;
    task.abort();
    server.abort();
    assert!(result.is_err(), "{:?}", seen.lock().unwrap());
}

#[tokio::test]
async fn a_deleted_account_never_says_hi() {
    use crate::login_store::suite::Fixture as _;
    let fixture = Fixture::open(false).await;
    fixture
        .app
        .import_json(
            Pool::Codex,
            &Fixture::login("alice", "expired-alice", now_unix_seconds() - 1).to_string(),
        )
        .await
        .unwrap();
    let id = fixture.id_of("alice");
    fixture
        .app
        .codex_logins()
        .change(|store| {
            store.accounts[0].standing = Some(Standing::Deactivated);
            Ok(())
        })
        .await
        .unwrap();
    fixture.app.delete_account(Pool::Codex, &id).await.unwrap();
    assert_eq!(
        fixture.app.say_hi(Pool::Codex, &id).await,
        Err("Account not found".into())
    );
    assert_eq!(fixture.renewals(), 0);
}

#[tokio::test]
async fn an_account_deleted_while_its_hi_renews_says_no_hi() {
    use crate::login_store::suite::Fixture as _;
    use axum::{extract::Request, routing::any, Router};
    use std::sync::Arc;
    struct Paused {
        fake: Fake,
        started: Arc<tokio::sync::Notify>,
        answer: Arc<tokio::sync::Notify>,
    }
    impl AccountNetwork for Paused {
        fn usage<'a>(&'a self, auth: &'a Value, proxy: &'a str) -> Response<'a, UsageSnapshot> {
            self.fake.usage(auth, proxy)
        }
        fn refresh<'a>(&'a self, auth: &'a Value, proxy: &'a str) -> Response<'a, Value> {
            Box::pin(async move {
                self.started.notify_one();
                self.answer.notified().await;
                self.fake.refresh(auth, proxy).await
            })
        }
        fn workspaces<'a>(&'a self, _: &'a str, _: &'a str) -> Response<'a, Vec<Metadata>> {
            Box::pin(async { Ok(vec![]) })
        }
    }
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let recorder = seen.clone();
    let router = Router::new().fallback(any(move |request: Request| {
        let recorder = recorder.clone();
        async move {
            let path = request.uri().path().to_owned();
            recorder.lock().unwrap().push(path.clone());
            if path.ends_with("/models") {
                json!({"models": [{"slug": "gpt-5.6-luna", "supported_in_api": true}]}).to_string()
            } else {
                "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n".to_owned()
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut fixture = Fixture::open(false).await;
    fixture
        .app
        .import_json(
            Pool::Codex,
            &Fixture::login("alice", "expired-alice", now_unix_seconds() - 1).to_string(),
        )
        .await
        .unwrap();
    let id = fixture.id_of("alice");
    fixture
        .app
        .release_new_accounts(Pool::Codex, Some(&id))
        .await
        .unwrap();
    fixture
        .app
        .save_account_proxy(Pool::Codex, &id, "direct://")
        .await
        .unwrap();
    std::fs::create_dir_all(fixture.app.codex_config_path().parent().unwrap()).unwrap();
    std::fs::write(
        fixture.app.codex_config_path(),
        format!("chatgpt_base_url = \"{base}\"\n"),
    )
    .unwrap();
    fixture
        .app
        .codex_logins()
        .change(|store| {
            store.accounts[0].standing = Some(Standing::Deactivated);
            Ok(())
        })
        .await
        .unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let answer = Arc::new(tokio::sync::Notify::new());
    fixture.app.network = Arc::new(Paused {
        fake: Fake::default(),
        started: started.clone(),
        answer: answer.clone(),
    });
    let (greeting, ()) = tokio::join!(fixture.app.say_hi(Pool::Codex, &id), async {
        started.notified().await;
        fixture.app.delete_account(Pool::Codex, &id).await.unwrap();
        assert_eq!(
            fixture.app.codex_load().unwrap().accounts[0].standing,
            Some(Standing::Deleted)
        );
        answer.notify_one();
    });
    server.abort();
    assert!(
        greeting.is_err() && seen.lock().unwrap().is_empty(),
        "{greeting:?} {:?}",
        seen.lock().unwrap()
    );
}
