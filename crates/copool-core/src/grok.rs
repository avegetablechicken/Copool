//! Grok account pool (Grok Build with SuperGrok): the logins Copool keeps,
//! the one Grok Build uses (`login`), the usage limit of each (`api`) and
//! switching. As in the Antigravity pool, the logins are kept in the pool's
//! file in Copool's data directory, with the pool's Sub2api providers
//! (`sub2api`), whose cards put Grok Build on their gateway. Copool has no
//! Grok API proxy or hi.
use crate::{
    application::{Application, SwitchExecution},
    models::{AccountSummary, Renewal, UsageSnapshot, RENEWAL_OFF},
    persistence::write_json,
    pool::{Pool, PoolAccount},
    utils::now_unix_seconds,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, path::PathBuf, sync::Arc};

mod api;
pub(crate) mod login;
mod sub2api;

pub(crate) use api::{sign_in_url, sub2api_usage, Network};

/// Variables as Grok Build started from a terminal has them: with `now`, as a
/// new shell exports them (including `~/.grok/.env`); otherwise the user's own
/// exports alone (`OWN_EXPORTS_ONLY`).
type Environment = Arc<dyn Fn(&[&str], bool) -> Vec<Option<String>> + Send + Sync>;

type Result<T> = std::result::Result<T, String>;

pub(crate) const LOGIN_NOT_FOUND: &str = "error.grok.login_not_found";
pub(crate) const CREDENTIALS_INVALID: &str = "error.grok.credentials_invalid";
pub(crate) const SIGN_IN_EXPIRED: &str = "error.grok.sign_in_expired";
const NOT_AVAILABLE: &str = "This is not available for Grok accounts";
/// A card of a login that names no account.
const UNNAMED: &str = "Grok";
/// An access token this close to its expiry is renewed first.
const EXPIRY_MARGIN: i64 = 60;

/// A browser sign-in's login, from its code.
pub(crate) async fn sign_in(code: &str, verifier: &str, redirect: &str) -> Result<Value> {
    login::signed_in(
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
    /// Held across a load and save of the store.
    lock: tokio::sync::Mutex<()>,
    /// Serializes imports, switches and refreshes.
    operations: tokio::sync::Mutex<()>,
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
            shell_rc: std::sync::RwLock::new(shell_rc()),
            #[cfg(test)]
            shell_rc: Default::default(),
            lock: Default::default(),
            operations: Default::default(),
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
/// The interactive startup file of the user's `$SHELL`, zsh's or bash's;
/// none for another shell, or where Copool asks no shell. A smoke test
/// names its own in `COPOOL_GROK_SHELL_RC`.
#[cfg(not(test))]
fn shell_rc() -> Option<PathBuf> {
    if let Some(rc) = std::env::var_os("COPOOL_GROK_SHELL_RC").filter(|v| !v.is_empty()) {
        return Some(rc.into());
    }
    if !cfg!(unix) {
        return None;
    }
    let shell = PathBuf::from(std::env::var_os("SHELL")?);
    let home = dirs::home_dir()?;
    match shell.file_name()?.to_str()? {
        "zsh" => Some(
            std::env::var_os("ZDOTDIR")
                .filter(|v| !v.is_empty())
                .map_or(home, PathBuf::from)
                .join(".zshrc"),
        ),
        "bash" => Some(home.join(".bashrc")),
        _ => None,
    }
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
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    id: String,
    label: String,
    #[serde(default)]
    email: Option<String>,
    /// The sign-in as Grok Build's `auth.json` holds it.
    document: Value,
    #[serde(default)]
    proxy_url: String,
    added_at: i64,
    updated_at: i64,
    #[serde(default)]
    usage: Option<UsageSnapshot>,
    #[serde(default)]
    usage_error: Option<String>,
    #[serde(default)]
    plan: Option<String>,
    /// Newly imported; nothing goes upstream for it until a manual refresh.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    first_request_pending: bool,
    /// The user's choice of whether to renew the tokens; none for the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    renewal: Option<Renewal>,
}
impl Stored {
    fn summary(&self, current: bool) -> AccountSummary {
        AccountSummary {
            display_status: "list".into(),
            workspace_status: "active".into(),
            source_tag: None,
            team_name: None,
            available: true,
            proxy_url: self.proxy_url.clone(),
            id: self.id.clone(),
            label: self.label.clone(),
            email: self.email.clone(),
            account_id: login::user_id(&self.document).unwrap_or_default(),
            plan_type: self.plan.clone(),
            added_at: self.added_at,
            updated_at: self.updated_at,
            usage: self.usage.clone(),
            usage_error: self.usage_error.clone(),
            is_current: current,
            awaiting_first_request: self.first_request_pending,
            token_expires_at: login::expiry(&self.document)
                .and_then(crate::models::displayed_expiry),
            renewal: Some(Renewal::of(self.renewal).as_str().into()),
            next_refresh_at: None,
            outdated_sub2api_version: None,
        }
    }
    /// Takes a newer generation of the same login (Grok Build renewed it).
    fn adopt(&mut self, document: &Value) -> bool {
        let newer =
            login::expiry(document).unwrap_or(0) > login::expiry(&self.document).unwrap_or(0);
        if !newer || login::access_token(document) == login::access_token(&self.document) {
            return false;
        }
        self.document = document.clone();
        self.usage_error = None;
        true
    }
}
impl PoolAccount for Stored {
    fn id(&self) -> &str {
        &self.id
    }
    fn set_renewal(&mut self, renewal: Renewal) {
        self.renewal = Some(renewal);
    }
    fn set_proxy(&mut self, proxy: String) {
        self.proxy_url = proxy;
    }
    fn release(&mut self) -> bool {
        std::mem::take(&mut self.first_request_pending)
    }
}

/// Follows the login Grok Build uses: its account becomes current, and a
/// newer generation of it is adopted. None read leaves the selection as last
/// seen. Returns whether the store changed.
fn follow_live(store: &mut Store, live: Option<&Value>) -> bool {
    let Some(live) = live else {
        return false;
    };
    let mut changed = false;
    let current = store
        .accounts
        .iter_mut()
        .find(|a| login::same_login(&a.document, live))
        .map(|account| {
            changed |= account.adopt(live);
            account.id.clone()
        });
    if store.current_account_id != current {
        store.current_account_id = current;
        changed = true;
    }
    changed
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
                login::sign_in_of(&object["document"])
            }
            Value::String(text) => login::decode(text.as_bytes()),
            value => login::sign_in_of(&value),
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
        login::apply_renewal(
            document,
            &self.access,
            self.expires_at,
            self.refresh.as_deref(),
        );
    }
}
/// Renews the document's access token; an error when xAI refuses.
async fn renew(network: &dyn Network, document: &mut Value, proxy: &str) -> Result<Renewed> {
    let refresh = login::refresh_token(document).ok_or(SIGN_IN_EXPIRED)?;
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
                expires_at: now_unix_seconds() + lifetime,
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
/// A usage request's failure.
enum Failure {
    /// The access token was refused: renewing it may help.
    Expired,
    Other(String),
}
async fn fetch_usage(
    network: &dyn Network,
    token: &str,
    proxy: &str,
    now: i64,
) -> std::result::Result<UsageSnapshot, Failure> {
    let (status, body) = network
        .api(api::USAGE_PATH, token, proxy)
        .await
        .map_err(Failure::Other)?;
    match status {
        200..=299 => {
            api::usage(&body, now).ok_or(Failure::Other("Grok reported no usage limit".into()))
        }
        401 => Err(Failure::Expired),
        _ => Err(Failure::Other(api::status_error(
            "Grok usage request",
            status,
            &body,
        ))),
    }
}

impl Application {
    fn grok_store_path(&self) -> PathBuf {
        self.paths.data_dir.join("grok-accounts.json")
    }
    fn grok_load(&self) -> Result<Store> {
        match fs::read(self.grok_store_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("Invalid Grok account store: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
            Err(e) => Err(e.to_string()),
        }
    }
    fn grok_save(&self, store: &Store) -> Result<()> {
        write_json(&self.grok_store_path(), store)?;
        self.publish();
        Ok(())
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
            Some(bytes) => login::decode(&bytes)
                .map(Some)
                .ok_or(CREDENTIALS_INVALID.into()),
        }
    }
    /// Gives Grok Build the sign-in, keeping the other issuers' sign-ins.
    fn grok_write_live(&self, document: &Value) -> Result<()> {
        let existing = self.grok_read_file().ok().flatten();
        write_json(
            &self.grok_auth_file(),
            &login::encode(existing.as_deref(), document),
        )
    }
    /// The pool, following the login Grok Build uses.
    pub(crate) async fn grok_accounts(&self) -> Result<Vec<AccountSummary>> {
        let _lock = self.grok.lock.lock().await;
        // Read under the lock a switch writes the sign-in under, which a
        // sign-in read before it would undo.
        let live = self.grok_read_live().ok().flatten();
        let mut store = self.grok_load()?;
        if follow_live(&mut store, live.as_ref()) {
            self.grok_save(&store)?;
        }
        // On a provider's gateway Grok Build uses the provider; else its login.
        let route = sub2api::current_route(&self.grok_config_path(), &store);
        let provider = match route {
            sub2api::CurrentRoute::Provider(id) => Some(id),
            _ => None,
        };
        let mut summaries: Vec<_> = store
            .accounts
            .iter()
            .map(|a| {
                a.summary(
                    route == sub2api::CurrentRoute::Login
                        && store.current_account_id.as_deref() == Some(a.id.as_str()),
                )
            })
            .chain(self.provider_cards(Pool::Grok, provider)?)
            .collect();
        crate::ranking::sort_for_display(&mut summaries);
        if let Some(schedule) = self.schedule(Pool::Grok) {
            for card in &mut summaries {
                // Every round asks for the card in use, full rounds for all;
                // a card speeding up sooner.
                let every = card.is_current;
                let fast = self.fast_refresh_at(Pool::Grok, &card.id);
                card.next_refresh_at = match store.accounts.iter().find(|a| a.id == card.id) {
                    None => Some(schedule.next_refresh_at(every, 0)),
                    Some(a) if a.first_request_pending => None,
                    Some(a) => {
                        Some(schedule.next_refresh_at(every, a.updated_at + schedule.interval))
                    }
                }
                .map(|next| crate::pool::sooner(next, fast));
            }
        }
        Ok(summaries)
    }
    /// Adds or updates the account of a sign-in; returns its id and whether
    /// it waits for a manual refresh.
    async fn grok_upsert(&self, document: Value, current: bool) -> Result<(String, bool)> {
        let defer = !self.preferences()?.request_new_accounts;
        let now = now_unix_seconds();
        let _lock = self.grok.lock.lock().await;
        let mut store = self.grok_load()?;
        let email = login::email(&document);
        let found = store
            .accounts
            .iter_mut()
            .find(|a| login::same_login(&a.document, &document));
        let (id, waiting) = match found {
            Some(existing) => {
                existing.document = document;
                existing.updated_at = now;
                existing.usage_error = None;
                if let Some(email) = email {
                    existing.label = email.clone();
                    existing.email = Some(email);
                }
                (existing.id.clone(), existing.first_request_pending)
            }
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                store.accounts.push(Stored {
                    id: id.clone(),
                    label: email.clone().unwrap_or_else(|| UNNAMED.into()),
                    email,
                    document,
                    proxy_url: String::new(),
                    added_at: now,
                    updated_at: now,
                    usage: None,
                    usage_error: None,
                    plan: None,
                    first_request_pending: defer,
                    renewal: None,
                });
                (id, defer)
            }
        };
        if current {
            store.current_account_id = Some(id.clone());
        }
        self.grok_save(&store)?;
        Ok((id, waiting))
    }
    /// Imports the login Grok Build uses.
    pub(crate) async fn grok_import_current(&self) -> Result<usize> {
        let op = self.grok.operations.lock().await;
        let live = self.grok_read_live()?.ok_or(LOGIN_NOT_FOUND)?;
        let (id, waiting) = self.grok_upsert(live, true).await?;
        drop(op);
        if !waiting {
            let _ = self.grok_refresh(&id, None).await;
        }
        Ok(1)
    }
    /// Stores a browser sign-in's login without switching to it.
    pub(crate) async fn grok_import_login(&self, document: Value) -> Result<usize> {
        let document = login::sign_in_of(&document).ok_or(CREDENTIALS_INVALID)?;
        let op = self.grok.operations.lock().await;
        let (id, waiting) = self.grok_upsert(document, false).await?;
        drop(op);
        if !waiting {
            let _ = self.grok_refresh(&id, None).await;
        }
        Ok(1)
    }
    /// Imports exported logins without switching to them; the whole
    /// document is checked before any is stored.
    pub(crate) async fn grok_import_json(&self, content: &str) -> Result<usize> {
        let documents = parse_import(content).ok_or(CREDENTIALS_INVALID)?;
        let op = self.grok.operations.lock().await;
        let count = documents.len();
        let mut fresh = Vec::new();
        for document in documents {
            let (id, waiting) = self.grok_upsert(document, false).await?;
            if !waiting {
                fresh.push(id);
            }
        }
        drop(op);
        for id in fresh {
            let _ = self.grok_refresh(&id, None).await;
        }
        Ok(count)
    }
    pub(crate) async fn grok_delete_account(&self, id: &str) -> Result<()> {
        let _lock = self.grok.lock.lock().await;
        let mut store = self.grok_load()?;
        let before = store.accounts.len();
        store.accounts.retain(|a| a.id != id);
        if store.accounts.len() == before {
            return Err("Account not found".into());
        }
        if store.current_account_id.as_deref() == Some(id) {
            store.current_account_id = None;
        }
        self.grok_save(&store)
    }
    pub(crate) async fn grok_edit_accounts(
        &self,
        edit: impl FnOnce(&mut [&mut dyn PoolAccount]) -> Result<bool>,
    ) -> Result<()> {
        let _lock = self.grok.lock.lock().await;
        let mut store = self.grok_load()?;
        let changed = {
            let mut accounts: Vec<&mut dyn PoolAccount> = store
                .accounts
                .iter_mut()
                .map(|a| a as &mut dyn PoolAccount)
                .collect();
            edit(&mut accounts)?
        };
        if changed {
            self.grok_save(&store)?;
        }
        Ok(())
    }
    /// Gives Grok Build this account's sign-in, off the gateway a provider
    /// put its models on, or a Sub2api card's provider. A sign-in there
    /// that the pool does not hold is kept in `grok-before-switch.json`
    /// first; a failed step is rolled back.
    pub(crate) async fn grok_switch_account(&self, id: &str) -> Result<SwitchExecution> {
        let _op = self.grok.operations.lock().await;
        if crate::sub2api::is_card(Pool::Grok, id) {
            return self.grok_switch_sub2api(id).await;
        }
        let live = self.grok_read_live().ok().flatten();
        let _lock = self.grok.lock.lock().await;
        let mut store = self.grok_load()?;
        follow_live(&mut store, live.as_ref());
        let target = store
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?
            .clone();
        if let Some(live) = live.as_ref().filter(|_| store.current_account_id.is_none()) {
            let backup = self.paths.data_dir.join("grok-before-switch.json");
            write_json(&backup, live)?;
        }
        // Grok Build's models keep to a provider's gateway until put back.
        let config = self.grok_config_path();
        let left = sub2api::leave_gateway(&config, &mut store)?;
        if let Err(error) = self.grok_write_live(&target.document) {
            sub2api::restore_left(&config, &mut store, left);
            return Err(error);
        }
        store.current_account_id = Some(target.id);
        self.grok_save(&store)?;
        drop(_lock);
        drop(_op);
        self.after_switch(Pool::Grok, Some(&target.document)).await
    }
    /// Whether the account failed: its usage refused, or its token expired.
    pub(crate) async fn grok_login_failed(&self, id: &str) -> Result<bool> {
        let store = self.grok_load()?;
        let account = store
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?;
        Ok(account.usage_error.is_some()
            || login::expiry(&account.document).is_some_and(|e| e <= now_unix_seconds()))
    }
    /// Grok Build's sign-in when it holds the document's account with
    /// another access token, unexpired: Grok Build renewed it.
    fn grok_live_renewal(&self, document: &Value, now: i64) -> Option<Value> {
        let live = self.grok_read_live().ok().flatten()?;
        let renewed = login::same_login(&live, document)
            && login::access_token(&live).is_some()
            && login::access_token(&live) != login::access_token(document)
            && login::expiry(&live).is_none_or(|e| e > now + EXPIRY_MARGIN);
        renewed.then_some(live)
    }
    /// Renews the document's access token, then gives the renewal to Grok
    /// Build's sign-in when it holds this account: xAI rotates the refresh
    /// token, and the one Grok Build holds stops working.
    async fn grok_renew(
        &self,
        network: &dyn Network,
        document: &mut Value,
        proxy: &str,
    ) -> Result<()> {
        let before = document.clone();
        let renewed = renew(network, document, proxy).await?;
        self.grok_write_back(&renewed, &before);
        Ok(())
    }
    /// Puts a renewal in Grok Build's sign-in when it still holds the
    /// renewed login and no token lasting longer; never another account or
    /// a newer token. A failure is only logged, for Copool's store keeps
    /// the renewal.
    fn grok_write_back(&self, renewed: &Renewed, before: &Value) {
        let outcome: Result<()> = (|| {
            let Some(mut live) = self.grok_read_live()? else {
                return Ok(());
            };
            if !login::same_login(&live, before)
                || login::expiry(&live).unwrap_or(0) > renewed.expires_at
                || login::access_token(&live).as_deref() == Some(renewed.access.as_str())
                // Grok Build renewed it from another refresh token meanwhile.
                || login::refresh_token(&live) != login::refresh_token(before)
            {
                return Ok(());
            }
            renewed.apply(&mut live);
            self.grok_write_live(&live)
        })();
        if let Err(error) = outcome {
            log::warn!("Grok Build's login was not given the renewal; Copool keeps it: {error}");
        }
    }
    /// Refreshes an account's usage, renewing its access token as its
    /// renewal allows; with `renew_now`, only renews it, whatever the choice,
    /// and reports that it did. The plan is asked along.
    pub(crate) async fn grok_refresh(&self, id: &str, renew_now: Option<&mut bool>) -> Result<()> {
        let forced = renew_now.is_some();
        let _activity = (!forced).then(|| self.begin_refresh(id));
        let _op = self.grok.operations.lock().await;
        let live = self.grok_read_live().ok().flatten();
        let account = {
            let _lock = self.grok.lock.lock().await;
            let mut store = self.grok_load()?;
            if follow_live(&mut store, live.as_ref()) {
                self.grok_save(&store)?;
            }
            store
                .accounts
                .into_iter()
                .find(|a| a.id == id)
                .ok_or("Account not found")?
        };
        if account.first_request_pending {
            return Ok(());
        }
        let network = self.grok_network();
        let network = &*network;
        let proxy = self.account_proxy(&account.proxy_url)?;
        let now = now_unix_seconds();
        let mut document = account.document.clone();
        let may_renew = forced || Renewal::of(account.renewal) != Renewal::Never;
        let mut renewed = false;
        // A sign-in Grok Build renewed, taken in place of the card's.
        let mut adopted = false;
        let outcome: Result<Option<UsageSnapshot>> = async {
            if forced {
                self.grok_renew(network, &mut document, &proxy).await?;
                renewed = true;
                return Ok(None);
            }
            let expired = login::expiry(&document).is_some_and(|e| e <= now + EXPIRY_MARGIN)
                || login::access_token(&document).is_none();
            if expired {
                if let Some(live) = self.grok_live_renewal(&document, now) {
                    document = live;
                    adopted = true;
                } else {
                    if !may_renew {
                        return Err(RENEWAL_OFF.into());
                    }
                    self.grok_renew(network, &mut document, &proxy).await?;
                    renewed = true;
                }
            }
            let token = login::access_token(&document).unwrap_or_default();
            match fetch_usage(network, &token, &proxy, now).await {
                Ok(usage) => Ok(Some(usage)),
                Err(Failure::Other(error)) => Err(error),
                Err(Failure::Expired) if renewed => Err(SIGN_IN_EXPIRED.into()),
                Err(Failure::Expired) => {
                    // Grok Build may have renewed the login since it was
                    // read: its token is tried once before renewing.
                    if let Some(live) = self.grok_live_renewal(&document, now) {
                        document = live;
                        adopted = true;
                        let token = login::access_token(&document).unwrap_or_default();
                        match fetch_usage(network, &token, &proxy, now).await {
                            Ok(usage) => return Ok(Some(usage)),
                            Err(Failure::Other(error)) => return Err(error),
                            Err(Failure::Expired) => {}
                        }
                    }
                    if !may_renew {
                        return Err(RENEWAL_OFF.into());
                    }
                    self.grok_renew(network, &mut document, &proxy).await?;
                    renewed = true;
                    let token = login::access_token(&document).unwrap_or_default();
                    match fetch_usage(network, &token, &proxy, now).await {
                        Ok(usage) => Ok(Some(usage)),
                        Err(Failure::Expired) => Err(SIGN_IN_EXPIRED.into()),
                        Err(Failure::Other(error)) => Err(error),
                    }
                }
            }
        }
        .await;
        // The plan and official model catalog, with the token that worked; when
        // either fails, the card keeps what it had.
        let mut plan = None;
        let mut models = None;
        if matches!(outcome, Ok(Some(_))) {
            let token = login::access_token(&document).unwrap_or_default();
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
        let _lock = self.grok.lock.lock().await;
        let mut store = self.grok_load()?;
        if let Some(models) = models {
            store.known_models = models;
        }
        let Some(stored) = store.accounts.iter_mut().find(|a| a.id == id) else {
            return outcome.map(|_| ());
        };
        // Grok Build may have renewed the login meanwhile; keep the newer.
        // A sign-in taken from Grok Build is kept whatever its expiry, for
        // its refresh token is the one that still works, unless the card
        // changed meanwhile.
        if renewed
            || (adopted && stored.document == account.document)
            || login::expiry(&document) > login::expiry(&stored.document)
        {
            stored.document = document;
        }
        if let Some(plan) = plan {
            stored.plan = Some(plan);
        }
        match &outcome {
            Ok(Some(usage)) => {
                let mut usage = usage.clone();
                usage.plan_type = stored.plan.clone();
                stored.usage = Some(usage);
                stored.usage_error = None;
                stored.updated_at = now;
            }
            Ok(None) => {}
            Err(error) => {
                stored.usage_error = Some(error.clone());
                stored.updated_at = now;
            }
        }
        self.grok_save(&store)?;
        if let Some(reported) = renew_now {
            *reported = renewed;
        }
        outcome.map(|_| ())
    }
    pub(crate) fn grok_unavailable<T>(&self) -> Result<T> {
        Err(NOT_AVAILABLE.into())
    }
}

#[cfg(test)]
mod tests;
