//! Antigravity account pool (Google AI Pro): the logins Copool keeps, the
//! one Antigravity uses (`login`), the quota of each (`api`) and switching.
//! As in the Codex pool, the logins are kept in the pool's file in Copool's
//! data directory, with the pool's Sub2api providers (`sub2api`), whose
//! cards put agy on the Gemini API. The local proxy serves agy in that mode.
use crate::{
    application::{Application, SwitchExecution},
    models::{AccountSummary, Renewal, UsageSnapshot, RENEWAL_OFF},
    persistence::write_json,
    pool::{Hello, Pool, PoolAccount},
    utils::now_unix_seconds,
    vault::KeychainAccess,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
};

mod api;
mod login;
mod proxy;
#[cfg(any(target_os = "macos", test))]
mod security;
mod sub2api;
pub(crate) use sub2api::ProviderAdapter;

pub(crate) use api::{sign_in_url, sub2api_usage, ultra_by_id, Network};

// Environment variables as agy would have them; with `now`, as agy started
// from a terminal opened now would, all from one shell, after Copool
// changed what it loads.
use crate::env_bridge::Environment;

/// A Google sign-in's login document, from its code.
pub(crate) async fn sign_in(code: &str, verifier: &str, redirect: &str) -> Result<Value> {
    pool_login::antigravity::signed_in(
        &api::exchange(code, verifier, redirect).await?,
        now_unix_seconds(),
    )
}
use crate::pool_login::{self, antigravity::Document, Identity};
use login::LiveStore;

type Result<T> = std::result::Result<T, String>;

pub(crate) const LOGIN_NOT_FOUND: &str = "error.antigravity.login_not_found";
pub(crate) const CREDENTIALS_INVALID: &str = "error.antigravity.credentials_invalid";
pub(crate) const SIGN_IN_EXPIRED: &str = "error.antigravity.sign_in_expired";
const NOT_AVAILABLE: &str = "This is not available for Antigravity accounts";
/// A card of a login no refresh has named yet.
const UNNAMED: &str = "Antigravity";
/// An access token this close to its expiry is renewed first.
const EXPIRY_MARGIN: i64 = 60;

/// The Antigravity pool's place in the application.
pub(crate) struct State {
    network: std::sync::RwLock<Arc<dyn Network>>,
    live: std::sync::RwLock<Arc<dyn LiveStore>>,
    /// agy's settings, whose `modelProvider` a provider's card sets; the
    /// `.env` with its key and gateway is two levels up, as `~/.gemini/.env`
    /// is.
    settings: std::sync::RwLock<PathBuf>,
    /// agy's environment, which a provider's card checks its key and
    /// gateway in.
    environment: std::sync::RwLock<Environment>,
    /// The startup file of the user's shell that a provider's card has load
    /// the `.env` when it does not: none for a shell Copool cannot write.
    shell_rc: std::sync::RwLock<Option<PathBuf>>,
    /// Held across a load and save of the store.
    lock: tokio::sync::Mutex<()>,
    /// Serializes imports, switches and refreshes.
    operations: tokio::sync::Mutex<()>,
    /// Counts switches made: Antigravity's login read before one is not
    /// followed after it.
    switches: std::sync::atomic::AtomicU64,
}
impl Default for State {
    fn default() -> Self {
        Self {
            network: std::sync::RwLock::new(Arc::new(api::Http)),
            // Tests never reach the user's own login, settings or shell.
            #[cfg(not(test))]
            live: std::sync::RwLock::new(login::system_store()),
            #[cfg(test)]
            live: std::sync::RwLock::new(Arc::new(tests::Live::default())),
            #[cfg(not(test))]
            settings: std::sync::RwLock::new(settings_path()),
            #[cfg(test)]
            settings: std::sync::RwLock::new(
                std::env::temp_dir()
                    .join(format!("copool-test-agy-{}", uuid::Uuid::new_v4()))
                    .join("antigravity-cli/settings.json"),
            ),
            #[cfg(not(test))]
            environment: std::sync::RwLock::new(Arc::new(|names: &[&str], now: bool| {
                use crate::proxy_integration::{environment_key, new_shell_keys, Shell};
                if now {
                    new_shell_keys(names, Shell::Interactive)
                } else {
                    let lookup = |name: &&str| environment_key(name, Shell::Interactive);
                    names.iter().map(lookup).collect()
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
            lock: Default::default(),
            operations: Default::default(),
            switches: Default::default(),
        }
    }
}
/// agy's global settings: `COPOOL_ANTIGRAVITY_SETTINGS_FILE` for a smoke
/// test, else `~/.gemini/antigravity-cli/settings.json`. A smoke test's
/// `.env` is two levels above its settings too.
#[cfg(not(test))]
fn settings_path() -> PathBuf {
    std::env::var_os("COPOOL_ANTIGRAVITY_SETTINGS_FILE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".gemini/antigravity-cli/settings.json")))
        .unwrap_or_default()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Store {
    accounts: Vec<Stored>,
    /// The account whose login Antigravity was last seen to use.
    current_account_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    providers: Vec<sub2api::Provider>,
    /// The provider agy's settings were last pointed at.
    #[serde(skip_serializing_if = "Option::is_none")]
    settings_provider: Option<String>,
    /// What a provider replaced in agy's settings, until an account's
    /// switch returns it.
    #[serde(skip_serializing_if = "Option::is_none")]
    settings_record: Option<sub2api::SettingsRecord>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    id: String,
    label: String,
    #[serde(default)]
    email: Option<String>,
    /// Google's id of the account, once a refresh asked for it.
    #[serde(default)]
    google_id: Option<String>,
    /// The login as Antigravity's item holds it.
    document: Value,
    /// Whether the item wraps the document as go-keyring does.
    #[serde(default)]
    wrapped: bool,
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
    /// The Cloud Code project of its messages, as last reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<String>,
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
            account_id: self.google_id.clone().unwrap_or_default(),
            plan_type: self.plan.clone(),
            added_at: self.added_at,
            updated_at: self.updated_at,
            usage: self.usage.clone(),
            usage_error: self.usage_error.clone(),
            is_current: current,
            awaiting_first_request: self.first_request_pending,
            token_expires_at: pool_login::antigravity::expiry(&self.document)
                .and_then(crate::models::displayed_expiry),
            renewal: Some(Renewal::of(self.renewal).as_str().into()),
            next_refresh_at: None,
            outdated_sub2api_version: None,
        }
    }
    /// Takes a newer generation of the same login (Antigravity renewed it).
    fn adopt(&mut self, document: &Value) -> bool {
        let newer = pool_login::antigravity::expiry(document).unwrap_or(0)
            > pool_login::antigravity::expiry(&self.document).unwrap_or(0);
        if !newer
            || pool_login::antigravity::access_token(document)
                == pool_login::antigravity::access_token(&self.document)
        {
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

/// Follows the login Antigravity uses: its account becomes current, and a
/// newer generation of it is adopted. None read leaves the selection as last
/// seen. Returns whether the store changed.
fn follow_live(store: &mut Store, live: Option<&Document>) -> bool {
    let Some(live) = live else {
        return false;
    };
    let mut changed = false;
    let current = store
        .accounts
        .iter_mut()
        .find(|a| pool_login::antigravity::same_login(&a.document, &live.value))
        .map(|account| {
            changed |= account.adopt(&live.value);
            account.id.clone()
        });
    if store.current_account_id != current {
        store.current_account_id = current;
        changed = true;
    }
    changed
}
/// Folds a card just named into the pool's card of the same Google account:
/// its newer login and usage stay, as does the selection.
fn merge_identified(store: &mut Store, id: &str) {
    let Some(index) = store.accounts.iter().position(|a| a.id == id) else {
        return;
    };
    let Some(key) = store.accounts[index].google_id.clone() else {
        return;
    };
    let Some(other) = store
        .accounts
        .iter()
        .position(|a| a.id != id && a.google_id.as_deref() == Some(key.as_str()))
    else {
        return;
    };
    let merged = store.accounts.remove(index);
    let other = if other > index { other - 1 } else { other };
    let existing = &mut store.accounts[other];
    existing.document = merged.document;
    existing.wrapped = merged.wrapped;
    existing.usage = merged.usage;
    existing.usage_error = merged.usage_error;
    existing.plan = merged.plan.or(existing.plan.take());
    existing.project = merged.project.or(existing.project.take());
    existing.updated_at = merged.updated_at;
    if existing.proxy_url.is_empty() {
        existing.proxy_url = merged.proxy_url;
    }
    existing.first_request_pending = false;
    if store.current_account_id.as_deref() == Some(id) {
        store.current_account_id = Some(existing.id.clone());
    }
}
/// The documents of an import: a login as Antigravity's item holds it, a
/// list of them, or `{"accounts": [...]}`; each a document, its item text,
/// or a card with its `document`. None when any is not a login.
fn parse_import(content: &str) -> Option<Vec<Document>> {
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
            Value::String(text) => pool_login::antigravity::decode(text.as_bytes()),
            Value::Object(ref object) if object.get("document").is_some_and(Value::is_object) => {
                let value = object["document"].clone();
                pool_login::antigravity::refresh_token(&value)?;
                Some(Document {
                    value,
                    wrapped: object
                        .get("wrapped")
                        .and_then(Value::as_bool)
                        .unwrap_or(cfg!(target_os = "macos")),
                })
            }
            value => {
                pool_login::antigravity::refresh_token(&value)?;
                Some(Document {
                    value,
                    wrapped: cfg!(target_os = "macos"),
                })
            }
        })
        .collect()
}

use crate::antigravity_cloud::{fetch_usage, status_error, Failure, Renewed};
impl Renewed {
    fn apply(&self, document: &mut Value) {
        pool_login::antigravity::apply_renewal(
            document,
            &self.access,
            self.expires_at,
            self.lifetime,
            self.refresh.as_deref(),
        );
    }
}
/// Renews the document's access token; an error when Google refuses.
async fn renew(network: &dyn Network, document: &mut Value, proxy: &str) -> Result<Renewed> {
    let refresh = pool_login::antigravity::refresh_token(document).ok_or(SIGN_IN_EXPIRED)?;
    let renewed = crate::antigravity_cloud::renew_token(network, &refresh, proxy).await?;
    renewed.apply(document);
    Ok(renewed)
}

impl Application {
    fn antigravity_store_path(&self) -> PathBuf {
        self.paths.data_dir.join("antigravity-accounts.json")
    }
    fn antigravity_load(&self) -> Result<Store> {
        match fs::read(self.antigravity_store_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("Invalid Antigravity account store: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
            Err(e) => Err(e.to_string()),
        }
    }
    fn antigravity_save(&self, store: &Store) -> Result<()> {
        write_json(&self.antigravity_store_path(), store)?;
        self.publish();
        Ok(())
    }
    fn antigravity_network(&self) -> Arc<dyn Network> {
        self.antigravity.network.read().unwrap().clone()
    }
    #[cfg(test)]
    pub(crate) fn set_antigravity_network(&self, network: Arc<dyn Network>) {
        *self.antigravity.network.write().unwrap() = network;
    }
    fn antigravity_live(&self) -> Arc<dyn LiveStore> {
        self.antigravity.live.read().unwrap().clone()
    }
    #[cfg(test)]
    fn set_antigravity_live(&self, live: Arc<dyn LiveStore>) {
        *self.antigravity.live.write().unwrap() = live;
    }
    fn antigravity_settings_path(&self) -> PathBuf {
        self.antigravity.settings.read().unwrap().clone()
    }
    #[cfg(test)]
    fn set_antigravity_settings(&self, path: PathBuf) {
        *self.antigravity.settings.write().unwrap() = path;
    }
    #[cfg(test)]
    fn set_antigravity_environment(&self, environment: Environment) {
        *self.antigravity.environment.write().unwrap() = environment;
    }
    fn antigravity_shell_rc(&self) -> Option<PathBuf> {
        self.antigravity.shell_rc.read().unwrap().clone()
    }
    #[cfg(test)]
    fn set_antigravity_shell_rc(&self, rc: PathBuf) {
        *self.antigravity.shell_rc.write().unwrap() = Some(rc);
    }
    /// Antigravity's login: none when absent; an error when it cannot be
    /// read or is no login.
    async fn antigravity_read_live(&self, access: KeychainAccess) -> Result<Option<Document>> {
        match self.antigravity_live().read(access).await? {
            None => Ok(None),
            Some(bytes) => pool_login::antigravity::decode(&bytes)
                .map(Some)
                .ok_or(CREDENTIALS_INVALID.into()),
        }
    }
    /// The pool, following the login Antigravity uses.
    pub(crate) async fn antigravity_accounts(&self) -> Result<Vec<AccountSummary>> {
        let switches = self.antigravity.switches.load(Ordering::SeqCst);
        let saved = self.antigravity_load()?;
        // A pool without logins never reads the Keychain.
        let live = if saved.accounts.is_empty() {
            None
        } else {
            self.antigravity_read_live(KeychainAccess::Background)
                .await
                .ok()
                .flatten()
        };
        let _lock = self.antigravity.lock.lock().await;
        let mut store = self.antigravity_load()?;
        // A switch made since may have replaced the login read.
        if self.antigravity.switches.load(Ordering::SeqCst) == switches
            && follow_live(&mut store, live.as_ref())
        {
            self.antigravity_save(&store)?;
        }
        // On the Gemini API agy uses the provider last applied; else its login.
        let api = sub2api::api_key_mode(&self.antigravity_settings_path());
        let provider = store.settings_provider.as_deref().filter(|_| api);
        let mut summaries: Vec<_> = store
            .accounts
            .iter()
            .map(|a| a.summary(!api && store.current_account_id.as_deref() == Some(a.id.as_str())))
            .chain(self.provider_cards(Pool::Antigravity, provider)?)
            .collect();
        crate::ranking::sort_for_display(&mut summaries);
        if let Some(schedule) = self.schedule(Pool::Antigravity) {
            for card in &mut summaries {
                // Every round asks for the card in use, full rounds for all;
                // a card speeding up sooner.
                let every = card.is_current;
                let fast = self.fast_refresh_at(Pool::Antigravity, &card.id);
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
    /// Adds or updates the account of a login; returns its id and whether
    /// it waits for a manual refresh.
    async fn antigravity_upsert(
        &self,
        document: Document,
        current: bool,
    ) -> Result<(String, bool)> {
        let defer = !self.preferences()?.request_new_accounts;
        let now = now_unix_seconds();
        let _lock = self.antigravity.lock.lock().await;
        let mut store = self.antigravity_load()?;
        let email = pool_login::antigravity::email(&document.value);
        let found = store.accounts.iter_mut().find(|a| {
            pool_login::antigravity::same_login(&a.document, &document.value)
                || email.is_some() && a.email == email
        });
        let (id, waiting) = match found {
            Some(existing) => {
                existing.document = document.value;
                existing.wrapped = document.wrapped;
                existing.updated_at = now;
                existing.usage_error = None;
                (existing.id.clone(), existing.first_request_pending)
            }
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                store.accounts.push(Stored {
                    id: id.clone(),
                    label: email.clone().unwrap_or_else(|| UNNAMED.into()),
                    email,
                    google_id: None,
                    document: document.value,
                    wrapped: document.wrapped,
                    proxy_url: String::new(),
                    added_at: now,
                    updated_at: now,
                    usage: None,
                    usage_error: None,
                    plan: None,
                    project: None,
                    first_request_pending: defer,
                    renewal: None,
                });
                (id, defer)
            }
        };
        if current {
            store.current_account_id = Some(id.clone());
        }
        self.antigravity_save(&store)?;
        Ok((id, waiting))
    }
    /// Imports the login Antigravity uses.
    pub(crate) async fn antigravity_import_current(&self) -> Result<usize> {
        let op = self.antigravity.operations.lock().await;
        let live = self
            .antigravity_read_live(KeychainAccess::Interactive)
            .await?
            .ok_or(LOGIN_NOT_FOUND)?;
        let (id, waiting) = self.antigravity_upsert(live, true).await?;
        drop(op);
        if !waiting {
            let _ = self.antigravity_refresh(&id, None).await;
        }
        Ok(1)
    }
    /// Stores a browser sign-in's login without switching to it.
    pub(crate) async fn antigravity_import_login(&self, document: Value) -> Result<usize> {
        pool_login::antigravity::refresh_token(&document).ok_or(CREDENTIALS_INVALID)?;
        let op = self.antigravity.operations.lock().await;
        let (id, waiting) = self
            .antigravity_upsert(
                Document {
                    value: document,
                    wrapped: cfg!(target_os = "macos"),
                },
                false,
            )
            .await?;
        drop(op);
        if !waiting {
            let _ = self.antigravity_refresh(&id, None).await;
        }
        Ok(1)
    }
    /// Imports exported logins without switching to them; the whole
    /// document is checked before any is stored.
    pub(crate) async fn antigravity_import_json(&self, content: &str) -> Result<usize> {
        let documents = parse_import(content).ok_or(CREDENTIALS_INVALID)?;
        let _op = self.antigravity.operations.lock().await;
        let count = documents.len();
        let mut fresh = Vec::new();
        for document in documents {
            let (id, waiting) = self.antigravity_upsert(document, false).await?;
            if !waiting {
                fresh.push(id);
            }
        }
        drop(_op);
        for id in fresh {
            let _ = self.antigravity_refresh(&id, None).await;
        }
        Ok(count)
    }
    pub(crate) async fn antigravity_delete_account(&self, id: &str) -> Result<()> {
        let _lock = self.antigravity.lock.lock().await;
        let mut store = self.antigravity_load()?;
        let before = store.accounts.len();
        store.accounts.retain(|a| a.id != id);
        if store.accounts.len() == before {
            return Err("Account not found".into());
        }
        if store.current_account_id.as_deref() == Some(id) {
            store.current_account_id = None;
        }
        self.antigravity_save(&store)
    }
    pub(crate) async fn antigravity_edit_accounts(
        &self,
        edit: impl FnOnce(&mut [&mut dyn PoolAccount]) -> Result<bool>,
    ) -> Result<()> {
        let _lock = self.antigravity.lock.lock().await;
        let mut store = self.antigravity_load()?;
        let changed = {
            let mut accounts: Vec<&mut dyn PoolAccount> = store
                .accounts
                .iter_mut()
                .map(|a| a as &mut dyn PoolAccount)
                .collect();
            edit(&mut accounts)?
        };
        if changed {
            self.antigravity_save(&store)?;
        }
        Ok(())
    }
    /// Gives Antigravity this account's login, off the Gemini API a
    /// provider put it on, or a Sub2api card's provider. A login there that
    /// the pool does not hold is kept in `antigravity-before-switch.json`
    /// first; a failed step is rolled back.
    pub(crate) async fn antigravity_switch_account(
        &self,
        id: &str,
        access: KeychainAccess,
    ) -> Result<SwitchExecution> {
        let _op = self.antigravity.operations.lock().await;
        if crate::sub2api::is_card(Pool::Antigravity, id) {
            return self.antigravity_switch_sub2api(id).await;
        }
        let live = self.antigravity_read_live(access).await?;
        let path = self.antigravity_settings_path();
        // The store is not locked while the item is written, which may wait
        // for the user to allow it: the cards are read meanwhile.
        let (target, settings) = {
            let _lock = self.antigravity.lock.lock().await;
            let mut store = self.antigravity_load()?;
            if follow_live(&mut store, live.as_ref()) {
                self.antigravity_save(&store)?;
            }
            let target = store
                .accounts
                .iter()
                .find(|a| a.id == id)
                .ok_or("Account not found")?
                .clone();
            if let Some(live) = live.as_ref().filter(|_| store.current_account_id.is_none()) {
                let backup = self.paths.data_dir.join("antigravity-before-switch.json");
                write_json(&backup, &live.value)?;
            }
            // agy keeps to the Gemini API while its settings say so. The
            // store records leaving it once the switch is made.
            (target, sub2api::leave_api_key(&path, &mut store)?)
        };
        if let Err(error) = self
            .antigravity_live()
            .write(
                pool_login::antigravity::encode(&Document {
                    value: target.document.clone(),
                    wrapped: target.wrapped,
                }),
                access,
            )
            .await
        {
            sub2api::restore_settings(&path, settings);
            return Err(error);
        }
        {
            let _lock = self.antigravity.lock.lock().await;
            let mut store = self.antigravity_load()?;
            store.settings_record = None;
            store.settings_provider = None;
            // Unless the account was removed while its login was written.
            store.current_account_id = store
                .accounts
                .iter()
                .any(|a| a.id == target.id)
                .then_some(target.id);
            self.antigravity.switches.fetch_add(1, Ordering::SeqCst);
            self.antigravity_save(&store)?;
        }
        let identity = Identity {
            email: target.email,
            account_id: target.google_id,
            ..Identity::default()
        };
        let credential = pool_login::antigravity::credential(&target.document, &identity);
        self.after_switch(Pool::Antigravity, Some(credential)).await
    }
    /// Whether the account failed: its usage refused, or its token expired.
    pub(crate) async fn antigravity_login_failed(&self, id: &str) -> Result<bool> {
        let store = self.antigravity_load()?;
        let account = store
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?;
        Ok(account.usage_error.is_some()
            || pool_login::antigravity::expiry(&account.document)
                .is_some_and(|e| e <= now_unix_seconds()))
    }
    /// Antigravity's login when it holds the document's login with another
    /// access token, unexpired: Antigravity, or another app sharing its
    /// login, renewed it.
    async fn antigravity_live_renewal(&self, document: &Value, now: i64) -> Option<Value> {
        let live = self
            .antigravity_read_live(KeychainAccess::Background)
            .await
            .ok()
            .flatten()?;
        let renewed = pool_login::antigravity::same_login(&live.value, document)
            && pool_login::antigravity::access_token(&live.value).is_some()
            && pool_login::antigravity::access_token(&live.value)
                != pool_login::antigravity::access_token(document)
            && pool_login::antigravity::expiry(&live.value).is_none_or(|e| e > now + EXPIRY_MARGIN);
        renewed.then_some(live.value)
    }
    /// Renews the document's access token, then gives the renewal to
    /// Antigravity's login when it holds this login.
    async fn antigravity_renew(
        &self,
        network: &dyn Network,
        document: &mut Value,
        proxy: &str,
    ) -> Result<()> {
        // The login as renewed from: Google may have rotated its refresh
        // token, which tells the login apart.
        let before = document.clone();
        let renewed = renew(network, document, proxy).await?;
        self.antigravity_write_back(&renewed, &before).await;
        Ok(())
    }
    /// Puts a renewal in Antigravity's login, in the item's own form, when
    /// the item still holds the login renewed (`document`, as it was before)
    /// and no token lasting longer; never another login or a newer token.
    /// Silent: no authorization UI, and a failure only logged, for Copool's
    /// store keeps the renewal.
    async fn antigravity_write_back(&self, renewed: &Renewed, document: &Value) {
        let outcome: Result<()> = async {
            let Some(mut live) = self
                .antigravity_read_live(KeychainAccess::Background)
                .await?
            else {
                return Ok(());
            };
            if !pool_login::antigravity::same_login(&live.value, document)
                || pool_login::antigravity::expiry(&live.value).unwrap_or(0) > renewed.expires_at
                || pool_login::antigravity::access_token(&live.value).as_deref()
                    == Some(renewed.access.as_str())
            {
                return Ok(());
            }
            renewed.apply(&mut live.value);
            self.antigravity_live()
                .write(
                    pool_login::antigravity::encode(&live),
                    KeychainAccess::Background,
                )
                .await
        }
        .await;
        if let Err(error) = outcome {
            log::warn!("Antigravity's login was not given the renewal; Copool keeps it: {error}");
        }
    }
    /// Refreshes an account's usage, renewing its access token as its
    /// renewal allows; with `renew_now`, only renews it, whatever the choice,
    /// and reports that it did. The plan is asked along, and the account's
    /// name until it is known.
    pub(crate) async fn antigravity_refresh(
        &self,
        id: &str,
        renew_now: Option<&mut bool>,
    ) -> Result<()> {
        self.antigravity_refresh_login(id, renew_now, None).await
    }
    async fn antigravity_refresh_after_rejection(&self, id: &str, rejected: &str) -> Result<()> {
        self.antigravity_refresh_login(id, None, Some(rejected))
            .await
    }
    async fn antigravity_refresh_login(
        &self,
        id: &str,
        renew_now: Option<&mut bool>,
        rejected: Option<&str>,
    ) -> Result<()> {
        let forced = renew_now.is_some() || rejected.is_some();
        let _activity = (!forced).then(|| self.begin_refresh(id));
        let _op = self.antigravity.operations.lock().await;
        let live = self
            .antigravity_read_live(KeychainAccess::Background)
            .await
            .ok()
            .flatten();
        let account = {
            let _lock = self.antigravity.lock.lock().await;
            let mut store = self.antigravity_load()?;
            if follow_live(&mut store, live.as_ref()) {
                self.antigravity_save(&store)?;
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
        if let Some(rejected) = rejected {
            if pool_login::antigravity::access_token(&account.document).as_deref() != Some(rejected)
            {
                return Ok(());
            }
            if Renewal::of(account.renewal) == Renewal::Never {
                return Err(RENEWAL_OFF.into());
            }
        }
        let network = self.antigravity_network();
        let network = &*network;
        let proxy = self.account_proxy(&account.proxy_url)?;
        let now = now_unix_seconds();
        let mut document = account.document.clone();
        let may_renew = forced || Renewal::of(account.renewal) != Renewal::Never;
        let mut renewed = false;
        // Taken from Antigravity's login after the token was refused.
        let mut adopted = false;
        let outcome: Result<Option<UsageSnapshot>> = async {
            if forced {
                self.antigravity_renew(network, &mut document, &proxy)
                    .await?;
                renewed = true;
                return Ok(None);
            }
            let expired = pool_login::antigravity::expiry(&document)
                .is_some_and(|e| e <= now + EXPIRY_MARGIN)
                || pool_login::antigravity::access_token(&document).is_none();
            if expired {
                if !may_renew {
                    return Err(RENEWAL_OFF.into());
                }
                self.antigravity_renew(network, &mut document, &proxy)
                    .await?;
                renewed = true;
            }
            let token = pool_login::antigravity::access_token(&document).unwrap_or_default();
            match fetch_usage(network, &token, &proxy, now).await {
                Ok(usage) => Ok(Some(usage)),
                Err(Failure::Denied(error) | Failure::Other(error)) => Err(error),
                Err(Failure::Expired) if renewed => Err(SIGN_IN_EXPIRED.into()),
                Err(Failure::Expired) => {
                    // Antigravity may have renewed the login since it was
                    // read: its token is tried once before renewing.
                    if let Some(live) = self.antigravity_live_renewal(&document, now).await {
                        document = live;
                        adopted = true;
                        let token =
                            pool_login::antigravity::access_token(&document).unwrap_or_default();
                        match fetch_usage(network, &token, &proxy, now).await {
                            Ok(usage) => return Ok(Some(usage)),
                            Err(Failure::Denied(error) | Failure::Other(error)) => {
                                return Err(error)
                            }
                            Err(Failure::Expired) => {}
                        }
                    }
                    if !may_renew {
                        return Err(RENEWAL_OFF.into());
                    }
                    self.antigravity_renew(network, &mut document, &proxy)
                        .await?;
                    renewed = true;
                    let token =
                        pool_login::antigravity::access_token(&document).unwrap_or_default();
                    match fetch_usage(network, &token, &proxy, now).await {
                        Ok(usage) => Ok(Some(usage)),
                        Err(Failure::Expired) => Err(SIGN_IN_EXPIRED.into()),
                        Err(Failure::Denied(error) | Failure::Other(error)) => Err(error),
                    }
                }
            }
        }
        .await;
        // The account's name and plan, with the token that worked.
        let token = pool_login::antigravity::access_token(&document).unwrap_or_default();
        let (mut identity, mut plan, mut project) = (None, None, None);
        // The credits of a successful `loadCodeAssist`; when it fails, the
        // card keeps those it had.
        let mut credits = None;
        if matches!(outcome, Ok(Some(_))) {
            if account.google_id.is_none() {
                identity = network
                    .identity(&token, &proxy)
                    .await
                    .ok()
                    .filter(|(status, _)| (200..300).contains(status))
                    .map(|(_, body)| body);
            }
            let assist = network
                .cloud(
                    api::LOAD_CODE_ASSIST_PATH,
                    &token,
                    api::empty(),
                    &api::agent(),
                    &proxy,
                )
                .await
                .ok()
                .filter(|(status, _)| (200..300).contains(status))
                .map(|(_, body)| body);
            plan = assist.as_ref().and_then(api::plan);
            project = assist.as_ref().and_then(api::project);
            credits = assist.as_ref().map(api::credits);
        }
        let _lock = self.antigravity.lock.lock().await;
        let mut store = self.antigravity_load()?;
        let Some(stored) = store.accounts.iter_mut().find(|a| a.id == id) else {
            return outcome.map(|_| ());
        };
        // Antigravity may have renewed the login meanwhile; keep the newer.
        // Its token taken after a refusal replaces the refused one.
        if renewed
            || pool_login::antigravity::expiry(&document)
                > pool_login::antigravity::expiry(&stored.document)
            || adopted && stored.document == account.document
        {
            stored.document = document;
        }
        if let Some(body) = identity {
            let email = body["email"].as_str().map(str::to_owned);
            stored.google_id = body["id"]
                .as_str()
                .map(str::to_owned)
                .or(stored.google_id.take());
            if let Some(email) = email {
                stored.label = email.clone();
                stored.email = Some(email);
            }
        }
        if let Some(plan) = plan {
            stored.plan = Some(plan);
        }
        if let Some(project) = project {
            stored.project = Some(project);
        }
        match &outcome {
            Ok(Some(usage)) => {
                let mut usage = usage.clone();
                usage.plan_type = stored.plan.clone();
                usage.credits = credits
                    .unwrap_or_else(|| stored.usage.as_ref().and_then(|u| u.credits.clone()));
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
        merge_identified(&mut store, id);
        self.antigravity_save(&store)?;
        if let Some(reported) = renew_now {
            *reported = renewed;
        }
        outcome.map(|_| ())
    }
    /// Says hi with an account through its proxy, without switching to it:
    /// one short message to each pool whose five-hour window is idle (to
    /// Gemini's when none is), which starts it; or, with `third_party`, to
    /// that pool alone (Gemini's, or the third party's), idle or not.
    pub(crate) async fn antigravity_hello(
        &self,
        id: &str,
        third_party_only: Option<bool>,
    ) -> Result<Hello> {
        let account = self
            .antigravity_load()?
            .accounts
            .into_iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?;
        let Some(token) = pool_login::antigravity::access_token(&account.document).filter(|_| {
            pool_login::antigravity::expiry(&account.document)
                .is_none_or(|e| e > now_unix_seconds() + EXPIRY_MARGIN)
        }) else {
            return Ok(Hello::Expired(SIGN_IN_EXPIRED.into()));
        };
        let proxy = self.account_proxy(&account.proxy_url)?;
        let network = self.antigravity_network();
        let agent = api::agent();
        let call = |path: &'static str, body: Value| {
            let (network, token, agent, proxy) =
                (network.clone(), token.clone(), agent.clone(), proxy.clone());
            async move { network.cloud(path, &token, body, &agent, &proxy).await }
        };
        let answer = |what: &str, (status, body): api::Reply| match status {
            200..=299 => Ok(Some(body)),
            _ if crate::antigravity_cloud::unauthenticated(status, &body) => Ok(None),
            _ => Err(status_error(what, status, &body)),
        };
        let project = match account.project.clone() {
            Some(project) => project,
            None => {
                let Some(body) = answer(
                    "Antigravity project request",
                    call(api::LOAD_CODE_ASSIST_PATH, api::empty()).await?,
                )?
                else {
                    return Ok(Hello::Expired(SIGN_IN_EXPIRED.into()));
                };
                api::project(&body).ok_or("Antigravity reported no project")?
            }
        };
        let Some(models) = answer(
            "Antigravity models request",
            call(api::FETCH_MODELS_PATH, api::empty()).await?,
        )?
        else {
            return Ok(Hello::Expired(SIGN_IN_EXPIRED.into()));
        };
        let (gemini, third_party) = api::hello_models(&models);
        // Idle: no reset to come, as the windows read; all without usage yet.
        let now = now_unix_seconds();
        let idle = |window: Option<&crate::models::UsageWindow>| {
            account.usage.is_none() || window.is_some_and(|w| w.reset_at.is_none_or(|r| r <= now))
        };
        let usage = account.usage.as_ref();
        let targets: Vec<String> = match third_party_only {
            Some(true) => third_party.into_iter().collect(),
            Some(false) => gemini.into_iter().collect(),
            None => {
                let mut targets: Vec<String> = [
                    (
                        gemini.clone(),
                        idle(usage.and_then(|u| u.five_hour.as_ref())),
                    ),
                    (
                        third_party,
                        idle(usage.and_then(|u| u.third_party_five_hour.as_ref())),
                    ),
                ]
                .into_iter()
                .filter_map(|(model, idle)| model.filter(|_| idle))
                .collect();
                if targets.is_empty() {
                    targets.extend(gemini);
                }
                targets
            }
        };
        if targets.is_empty() {
            return Err("Antigravity lists no model to say hi to".into());
        }
        for model in targets {
            if answer(
                "Antigravity message request",
                call(api::GENERATE_PATH, api::hello(&project, &model)).await?,
            )?
            .is_none()
            {
                return Ok(Hello::Expired(SIGN_IN_EXPIRED.into()));
            }
        }
        Ok(Hello::Said)
    }
    pub(crate) fn antigravity_unavailable<T>(&self) -> Result<T> {
        Err(NOT_AVAILABLE.into())
    }
}

#[cfg(test)]
mod tests;
