//! Grok account pool (Grok Build with SuperGrok): the logins Copool keeps,
//! the one Grok Build uses (`crate::pool_login::grok`), the usage limit of
//! each (`api`) and switching. As in the Antigravity pool, the logins are
//! kept in the pool's file in Copool's data directory, with the pool's
//! Sub2api providers (`sub2api`), whose cards put Grok Build on their
//! gateway. Copool has no Grok API proxy or hi.
use crate::{
    application::{Application, SwitchExecution},
    login_store::{Account, Asked, LoginPool, Logins, Nothing},
    models::{AccountSummary, UsageSnapshot},
    persistence::write_json,
    pool::{Pool, PoolAccount},
    pool_login::{self, grok::CREDENTIALS_INVALID, Identity},
    utils::now_unix_seconds,
    vault::KeychainAccess,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, path::PathBuf, sync::Arc};

mod api;
mod sub2api;
pub(crate) use sub2api::ProviderAdapter;

pub(crate) use api::{sign_in_url, sub2api_usage, Network};

// Variables as Grok Build started from a terminal has them: with `now`, as a
// new shell exports them (including `~/.grok/.env`); otherwise the user's own
// exports alone (`OWN_EXPORTS_ONLY`).
use crate::env_bridge::Environment;

type Result<T> = std::result::Result<T, String>;

pub(crate) const LOGIN_NOT_FOUND: &str = "error.grok.login_not_found";
pub(crate) const SIGN_IN_EXPIRED: &str = "error.grok.sign_in_expired";
const NOT_AVAILABLE: &str = "This is not available for Grok accounts";
/// A card of a login that names no account.
const UNNAMED: &str = "Grok";

/// A browser sign-in's login, from its code.
pub(crate) async fn sign_in(code: &str, verifier: &str, redirect: &str) -> Result<Value> {
    pool_login::grok::signed_in(
        &api::exchange(code, verifier, redirect).await?,
        now_unix_seconds(),
    )
}

/// The Grok pool's place in the application.
pub(crate) struct State {
    network: std::sync::RwLock<Arc<dyn Network>>,
    /// Grok Build's `auth.json`; its `config.toml` and model catalog are
    /// beside it.
    auth_file: std::sync::RwLock<PathBuf>,
    /// Grok Build's environment, where a provider's card checks its key and
    /// bridged `XAI_API_KEY`.
    environment: std::sync::RwLock<Environment>,
    /// The startup file of the user's shell that a provider's card has load
    /// `~/.grok/.env` when bridging a custom variable to `XAI_API_KEY`.
    shell_rc: std::sync::RwLock<Option<PathBuf>>,
    pub(crate) logins: crate::login_store::Locks,
    /// Reads of the Live login answer this late with what was there when
    /// asked.
    #[cfg(test)]
    live_read_delay_ms: std::sync::atomic::AtomicU64,
}
impl Default for State {
    fn default() -> Self {
        Self {
            network: std::sync::RwLock::new(Arc::new(api::Http)),
            // Tests never reach the user's own login or shell.
            #[cfg(not(test))]
            auth_file: std::sync::RwLock::new(auth_file_path()),
            #[cfg(test)]
            auth_file: std::sync::RwLock::new(
                std::env::temp_dir()
                    .join(format!("copool-test-grok-{}", uuid::Uuid::new_v4()))
                    .join("auth.json"),
            ),
            #[cfg(not(test))]
            environment: std::sync::RwLock::new(Arc::new(|names: &[&str], now: bool| {
                use crate::proxy_integration::{new_shell_keys, new_shell_own_keys, Shell};
                if now {
                    new_shell_keys(names, Shell::Interactive)
                } else {
                    new_shell_own_keys(names, Shell::Interactive)
                }
            })),
            #[cfg(test)]
            environment: std::sync::RwLock::new(Arc::new(|names: &[&str], _: bool| {
                vec![None; names.len()]
            })),
            #[cfg(not(test))]
            shell_rc: std::sync::RwLock::new(sub2api::BRIDGE.shell_rc()),
            #[cfg(test)]
            shell_rc: Default::default(),
            logins: Default::default(),
            #[cfg(test)]
            live_read_delay_ms: Default::default(),
        }
    }
}
/// Grok Build's login: `COPOOL_GROK_AUTH_FILE` for a smoke test, else
/// `auth.json` in `$GROK_HOME`, by default `~/.grok`.
#[cfg(not(test))]
fn auth_file_path() -> PathBuf {
    let set = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    if let Some(file) = set("COPOOL_GROK_AUTH_FILE") {
        return file.into();
    }
    set("GROK_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".grok")))
        .unwrap_or_default()
        .join("auth.json")
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Store {
    accounts: Vec<Stored>,
    /// The account whose login Grok Build was last seen to use.
    current_account_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    providers: Vec<sub2api::Provider>,
    /// The provider Grok Build's models were last given the gateway of.
    #[serde(skip_serializing_if = "Option::is_none")]
    config_provider: Option<String>,
    /// What that replaced in Grok Build's `config.toml`, until an account's
    /// switch puts it back.
    #[serde(skip_serializing_if = "Option::is_none")]
    config_record: Option<sub2api::ConfigRecord>,
    /// Official Grok text models last observed from xAI `/models` or
    /// `models_cache.json` while not on a Sub2API gateway, so models omitted
    /// by a gateway's `/v1/models` stay selectable across switches.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    known_models: Vec<String>,
}
type Stored = Account<Nothing>;

/// The Grok pool's facts for its store: Grok Build's `auth.json` is its
/// Live login.
struct GrokLogins<'a>(&'a Application);
impl LoginPool for GrokLogins<'_> {
    type Own = Nothing;
    type Store = Store;
    type Login = Value;
    type Renewed = Renewed;
    const FILE: &'static str = "grok-accounts.json";
    const NAME: &'static str = "Grok";
    const UNNAMED: &'static str = UNNAMED;
    const SIGN_IN_EXPIRED: &'static str = SIGN_IN_EXPIRED;
    const LIVE_UNREADABLE: &'static str =
        "Cannot read Grok Build's login; fix auth.json before renewing the account it uses";

    fn app(&self) -> &Application {
        self.0
    }
    fn locks(&self) -> &crate::login_store::Locks {
        &self.0.grok.logins
    }
    fn accounts(store: &Store) -> &Vec<Stored> {
        &store.accounts
    }
    fn accounts_mut(store: &mut Store) -> &mut Vec<Stored> {
        &mut store.accounts
    }
    fn current(store: &mut Store) -> &mut Option<String> {
        &mut store.current_account_id
    }
    async fn read_live(&self, _: KeychainAccess) -> Result<Option<Value>> {
        let live = self.0.grok_read_live();
        #[cfg(test)]
        {
            let delay = self
                .0
                .grok
                .live_read_delay_ms
                .load(std::sync::atomic::Ordering::Relaxed);
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        live
    }
    async fn write_live(&self, login: &Value, _: KeychainAccess) -> Result<()> {
        self.0.grok_write_live(login)
    }
    fn document(login: &Value) -> &Value {
        login
    }
    fn document_mut(login: &mut Value) -> &mut Value {
        login
    }
    fn login_of(account: &Stored) -> Value {
        account.document.clone()
    }
    fn keep(account: &mut Stored, login: Value) {
        account.document = login;
    }
    fn access_token(document: &Value) -> Option<String> {
        pool_login::grok::access_token(document)
    }
    fn refresh_token(document: &Value) -> Option<String> {
        pool_login::grok::refresh_token(document)
    }
    fn expiry(document: &Value) -> Option<i64> {
        pool_login::grok::expiry(document)
    }
    fn email(document: &Value) -> Option<String> {
        pool_login::grok::email(document)
    }
    /// The same user in the same team: one email may sign in to several.
    fn same_login(a: &Value, b: &Value) -> bool {
        pool_login::grok::same_login(a, b)
    }
    fn account_id(account: &Stored) -> String {
        pool_login::grok::user_id(&account.document).unwrap_or_default()
    }
    async fn renew(&self, document: &mut Value, proxy: &str) -> Result<Renewed> {
        renew(&*self.0.grok_network(), document, proxy).await
    }
    fn apply(renewed: &Renewed, document: &mut Value) {
        renewed.apply(document);
    }
}

/// The sign-ins of an import: Grok Build's `auth.json`, a sign-in, a list
/// of either, or `{"accounts": [...]}`, each perhaps a card with its
/// `document`. None when any is not a sign-in.
fn parse_import(content: &str) -> Option<Vec<Value>> {
    let value: Value = serde_json::from_str(content.trim().trim_start_matches('\u{feff}')).ok()?;
    let entries = match value {
        Value::Array(entries) => entries,
        Value::Object(ref object) if object.get("accounts").is_some_and(Value::is_array) => {
            object["accounts"].as_array().cloned().unwrap_or_default()
        }
        other => vec![other],
    };
    if entries.is_empty() {
        return None;
    }
    entries
        .into_iter()
        .map(|entry| match entry {
            Value::Object(ref object) if object.get("document").is_some_and(Value::is_object) => {
                pool_login::grok::sign_in_of(&object["document"])
            }
            Value::String(text) => pool_login::grok::decode(text.as_bytes()),
            value => pool_login::grok::sign_in_of(&value),
        })
        .collect()
}

/// A renewal of the access token.
struct Renewed {
    access: String,
    expires_at: i64,
    refresh: Option<String>,
}
impl Renewed {
    fn apply(&self, document: &mut Value) {
        pool_login::grok::apply_renewal(
            document,
            &self.access,
            self.expires_at,
            self.refresh.as_deref(),
        );
    }
}
/// Renews the document's access token; an error when xAI refuses.
async fn renew(network: &dyn Network, document: &mut Value, proxy: &str) -> Result<Renewed> {
    let refresh = pool_login::grok::refresh_token(document).ok_or(SIGN_IN_EXPIRED)?;
    let (status, body) = network.renew(&refresh, proxy).await?;
    let renewed = match status {
        200..=299 => {
            let access = body["access_token"]
                .as_str()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .ok_or("xAI returned no access token")?;
            let lifetime = body["expires_in"]
                .as_i64()
                .filter(|s| *s > 0)
                .unwrap_or(6 * 60 * 60);
            Renewed {
                access: access.to_owned(),
                expires_at: now_unix_seconds().saturating_add(lifetime),
                refresh: body["refresh_token"].as_str().map(str::to_owned),
            }
        }
        // invalid_grant: the login was signed out, revoked or rotated away.
        400 | 401 | 403 => return Err(SIGN_IN_EXPIRED.into()),
        _ => return Err(api::status_error("xAI token request", status, &body)),
    };
    renewed.apply(document);
    Ok(renewed)
}
async fn fetch_usage(
    network: &dyn Network,
    token: &str,
    proxy: &str,
    now: i64,
) -> Asked<UsageSnapshot> {
    let (status, body) = match network.api(api::USAGE_PATH, token, proxy).await {
        Ok(answer) => answer,
        Err(error) => return Asked::Failed(error),
    };
    match status {
        200..=299 => match api::usage(&body, now) {
            Some(usage) => Asked::Answered(usage),
            None => Asked::Failed("Grok reported no usage limit".into()),
        },
        401 => Asked::Refused,
        _ => Asked::Failed(api::status_error("Grok usage request", status, &body)),
    }
}

impl Application {
    fn grok_logins(&self) -> Logins<GrokLogins<'_>> {
        Logins(GrokLogins(self))
    }
    fn grok_load(&self) -> Result<Store> {
        self.grok_logins().load()
    }
    fn grok_save(&self, store: &Store) -> Result<()> {
        self.grok_logins().save(store)
    }
    fn grok_network(&self) -> Arc<dyn Network> {
        self.grok.network.read().unwrap().clone()
    }
    #[cfg(test)]
    fn set_grok_network(&self, network: Arc<dyn Network>) {
        *self.grok.network.write().unwrap() = network;
    }
    fn grok_auth_file(&self) -> PathBuf {
        self.grok.auth_file.read().unwrap().clone()
    }
    #[cfg(test)]
    fn set_grok_auth_file(&self, path: PathBuf) {
        *self.grok.auth_file.write().unwrap() = path;
    }
    #[cfg(test)]
    fn set_grok_environment(&self, environment: Environment) {
        *self.grok.environment.write().unwrap() = environment;
    }
    fn grok_shell_rc(&self) -> Option<PathBuf> {
        self.grok.shell_rc.read().unwrap().clone()
    }
    #[cfg(test)]
    fn set_grok_shell_rc(&self, rc: PathBuf) {
        *self.grok.shell_rc.write().unwrap() = Some(rc);
    }
    /// Grok Build's `config.toml`, beside its `auth.json`.
    fn grok_config_path(&self) -> PathBuf {
        self.grok_auth_file().with_file_name("config.toml")
    }
    /// The bytes of Grok Build's `auth.json`: none when absent.
    fn grok_read_file(&self) -> Result<Option<Vec<u8>>> {
        match fs::read(self.grok_auth_file()) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    /// Grok Build's sign-in: none when absent; an error when it cannot be
    /// read or holds no sign-in.
    fn grok_read_live(&self) -> Result<Option<Value>> {
        match self.grok_read_file()? {
            None => Ok(None),
            Some(bytes) => pool_login::grok::decode(&bytes)
                .map(Some)
                .ok_or(CREDENTIALS_INVALID.into()),
        }
    }
    /// Gives Grok Build the sign-in, keeping the other issuers' sign-ins.
    fn grok_write_live(&self, document: &Value) -> Result<()> {
        let existing = self.grok_read_file().ok().flatten();
        write_json(
            &self.grok_auth_file(),
            &pool_login::grok::encode(existing.as_deref(), document),
        )
    }
    /// The pool, following the login Grok Build uses.
    pub(crate) async fn grok_accounts(&self) -> Result<Vec<AccountSummary>> {
        let store = self.grok_logins().followed().await?;
        let route = sub2api::current_route(&self.grok_config_path(), &store);
        let provider = self.grok_provider(&store);
        let mut summaries: Vec<_> = store
            .accounts
            .iter()
            .map(|a| {
                Logins::<GrokLogins>::summary(
                    a,
                    route == sub2api::CurrentRoute::Login
                        && store.current_account_id.as_deref() == Some(a.id.as_str()),
                )
            })
            .chain(self.provider_cards(Pool::Grok, provider)?)
            .collect();
        crate::ranking::sort_for_display(&mut summaries);
        Ok(summaries)
    }
    /// The provider Grok Build uses: on a provider's gateway, the provider;
    /// else none, its login.
    fn grok_provider<'a>(&self, store: &'a Store) -> Option<&'a str> {
        match sub2api::current_route(&self.grok_config_path(), store) {
            sub2api::CurrentRoute::Provider(id) => Some(id),
            _ => None,
        }
    }
    /// The provider Grok Build uses now, if any.
    pub(crate) fn grok_provider_in_use(&self) -> Option<String> {
        let store = self.grok_load().ok()?;
        self.grok_provider(&store).map(str::to_owned)
    }
    /// Imports the login Grok Build uses.
    pub(crate) async fn grok_import_current(&self) -> Result<usize> {
        let fresh = self
            .grok_logins()
            .import_live(KeychainAccess::Interactive, LOGIN_NOT_FOUND)
            .await?;
        if let Some(id) = fresh {
            let _ = self.grok_refresh(&id, None).await;
        }
        Ok(1)
    }
    /// Stores a browser sign-in's login without switching to it.
    pub(crate) async fn grok_import_login(&self, document: Value) -> Result<usize> {
        let document = pool_login::grok::sign_in_of(&document).ok_or(CREDENTIALS_INVALID)?;
        self.grok_import(vec![document], crate::login_store::Origin::SignIn)
            .await
    }
    /// Imports exported logins without switching to them; the whole
    /// document is checked before any is stored.
    pub(crate) async fn grok_import_json(&self, content: &str) -> Result<usize> {
        self.grok_import(
            parse_import(content).ok_or(CREDENTIALS_INVALID)?,
            crate::login_store::Origin::Export,
        )
        .await
    }
    async fn grok_import(
        &self,
        documents: Vec<Value>,
        origin: crate::login_store::Origin,
    ) -> Result<usize> {
        let count = documents.len();
        for id in self.grok_logins().import(documents, origin).await? {
            let _ = self.grok_refresh(&id, None).await;
        }
        Ok(count)
    }
    pub(crate) async fn grok_delete_account(&self, id: &str) -> Result<()> {
        self.grok_logins().delete(id, |_, _| {}).await
    }
    pub(crate) async fn grok_edit_accounts(
        &self,
        edit: impl FnOnce(&mut [&mut dyn PoolAccount]) -> Result<bool>,
    ) -> Result<()> {
        self.grok_logins().edit(edit).await
    }
    /// Gives Grok Build this account's sign-in, off the gateway a provider
    /// put its models on, or a Sub2api card's provider. A sign-in there
    /// that the pool does not hold is kept in `grok-before-switch.json`
    /// first; a failed step is rolled back.
    pub(crate) async fn grok_switch_account(
        &self,
        id: &str,
        access: KeychainAccess,
    ) -> Result<SwitchExecution> {
        let _op = self.grok.logins.operations.lock().await;
        if crate::sub2api::is_card(Pool::Grok, id) {
            return self.grok_switch_sub2api(id).await;
        }
        // An auth.json holding no xAI sign-in is written over, its other
        // issuers kept.
        let live = self.grok_read_live().ok().flatten();
        // Grok Build's models keep to a provider's gateway until put back.
        let config = self.grok_config_path();
        let target = self
            .grok_logins()
            .switch(
                id,
                live,
                access,
                "grok-before-switch.json",
                |store| sub2api::leave_gateway(&config, store),
                |left| sub2api::restore_left(&config, left),
                |store| {
                    store.config_record = None;
                    store.config_provider = None;
                },
            )
            .await?;
        drop(_op);
        let credential = pool_login::grok::credential(&target.document, &Identity::default());
        self.after_switch(Pool::Grok, Some(credential)).await
    }
    /// Whether the account failed: its usage refused, or its token expired.
    pub(crate) async fn grok_login_failed(&self, id: &str) -> Result<bool> {
        self.grok_logins().login_failed(id)
    }
    /// Refreshes an account's usage, renewing its access token as its
    /// renewal allows; with `renew_now`, only renews it, whatever the choice,
    /// and reports that it did. The plan is asked along.
    pub(crate) async fn grok_refresh(&self, id: &str, renew_now: Option<&mut bool>) -> Result<()> {
        let forced = renew_now.is_some();
        let _activity = (!forced).then(|| self.begin_refresh(id));
        let logins = self.grok_logins();
        let _op = self.grok.logins.operations.lock().await;
        let begun = logins.begin(id).await?;
        if begun.account.first_request_pending {
            return Ok(());
        }
        let network = self.grok_network();
        let network = &*network;
        let proxy = self.account_proxy(&begun.account.proxy_url)?;
        let now = now_unix_seconds();
        let asked = proxy.as_str();
        let used = logins
            .use_token(&begun, forced, &proxy, now, |document| async move {
                let token = pool_login::grok::access_token(&document).unwrap_or_default();
                fetch_usage(network, &token, asked, now).await
            })
            .await;
        // The plan and official model catalog, with the token that worked; when
        // either fails, the card keeps what it had.
        let mut plan = None;
        let mut models = None;
        if matches!(used.outcome, Ok(Some(_))) {
            let token = pool_login::grok::access_token(&used.document).unwrap_or_default();
            plan = network
                .api(api::SETTINGS_PATH, &token, &proxy)
                .await
                .ok()
                .filter(|(status, _)| (200..300).contains(status))
                .and_then(|(_, body)| api::plan(&body));
            models = network
                .api(api::MODELS_PATH, &token, &proxy)
                .await
                .ok()
                .filter(|(status, _)| (200..300).contains(status))
                .map(|(_, body)| api::models(&body))
                .filter(|list| !list.is_empty());
        }
        logins
            .record(&begun, &used, |store, index| {
                if let Some(models) = models {
                    store.known_models = models;
                }
                let stored = &mut store.accounts[index];
                if let Some(plan) = plan {
                    stored.plan = Some(plan);
                }
                Logins::<GrokLogins>::record_usage(stored, &used.outcome, now);
            })
            .await?;
        if let Some(reported) = renew_now {
            // A renewal the tool made meanwhile serves as one.
            *reported = used.renewed || used.adopted;
        }
        used.outcome.map(|_| ())
    }
    pub(crate) fn grok_unavailable<T>(&self) -> Result<T> {
        Err(NOT_AVAILABLE.into())
    }
}

#[cfg(test)]
mod tests;
