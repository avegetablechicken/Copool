//! Antigravity account pool (Google AI Pro): the logins Copool keeps, the
//! one Antigravity uses (`login`), the quota of each (`api`) and switching.
//! As in the Codex pool, the logins are kept in the pool's file in Copool's
//! data directory. Antigravity has no API proxy, Sub2api or greeting here.
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
use std::{fs, path::PathBuf, sync::Arc};

mod api;
mod login;

pub(crate) use api::{sign_in_url, Network};

/// A Google sign-in's login document, from its code.
pub(crate) async fn sign_in(code: &str, verifier: &str, redirect: &str) -> Result<Value> {
    login::signed_in(
        &api::exchange(code, verifier, redirect).await?,
        now_unix_seconds(),
    )
}
use login::{Document, LiveStore};

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
    /// Held across a load and save of the store.
    lock: tokio::sync::Mutex<()>,
    /// Serializes imports, switches and refreshes.
    operations: tokio::sync::Mutex<()>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            network: std::sync::RwLock::new(Arc::new(api::Http)),
            // Tests never reach the user's own login.
            #[cfg(not(test))]
            live: std::sync::RwLock::new(Arc::new(login::SystemStore)),
            #[cfg(test)]
            live: std::sync::RwLock::new(Arc::new(tests::Live::default())),
            lock: Default::default(),
            operations: Default::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Store {
    accounts: Vec<Stored>,
    /// The account whose login Antigravity was last seen to use.
    current_account_id: Option<String>,
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
            token_expires_at: login::expiry(&self.document)
                .and_then(crate::models::displayed_expiry),
            renewal: Some(Renewal::of(self.renewal).as_str().into()),
            next_refresh_at: None,
        }
    }
    /// Takes a newer generation of the same login (Antigravity renewed it).
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
        .find(|a| login::same_login(&a.document, &live.value))
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
            Value::String(text) => login::decode(text.as_bytes()),
            Value::Object(ref object) if object.get("document").is_some_and(Value::is_object) => {
                let value = object["document"].clone();
                login::refresh_token(&value)?;
                Some(Document {
                    value,
                    wrapped: object
                        .get("wrapped")
                        .and_then(Value::as_bool)
                        .unwrap_or(cfg!(target_os = "macos")),
                })
            }
            value => {
                login::refresh_token(&value)?;
                Some(Document {
                    value,
                    wrapped: cfg!(target_os = "macos"),
                })
            }
        })
        .collect()
}

/// Why a usage request failed.
enum Failure {
    /// The access token was refused: renewing it may help.
    Expired,
    Other(String),
}
fn status_error(what: &str, status: u16, body: &Value) -> String {
    let message = body
        .pointer("/error/message")
        .or_else(|| body.get("error_description"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("{what} failed: HTTP {status} {message}")
        .trim()
        .to_owned()
}
/// One quota request: its parsed usage, none for a reply without quota.
async fn quota(
    network: &dyn Network,
    path: &str,
    parse: fn(&Value, i64) -> Option<UsageSnapshot>,
    token: &str,
    proxy: &str,
    now: i64,
) -> std::result::Result<Option<UsageSnapshot>, Failure> {
    let (status, body) = network
        .cloud(path, token, api::empty(), &api::agent(), proxy)
        .await
        .map_err(Failure::Other)?;
    match status {
        401 | 403 => Err(Failure::Expired),
        200..=299 => Ok(parse(&body, now)),
        _ => Err(Failure::Other(status_error(
            "Antigravity usage request",
            status,
            &body,
        ))),
    }
}
/// The account's quota. The summary reports the shared pools; an account
/// whose Gemini models have no shared pool there (a Google AI Pro account,
/// say) reports their five-hour window per model, as does any account
/// without the summary.
async fn fetch_usage(
    network: &dyn Network,
    token: &str,
    proxy: &str,
    now: i64,
) -> std::result::Result<UsageSnapshot, Failure> {
    let summary = match quota(
        network,
        api::QUOTA_SUMMARY_PATH,
        api::quota_summary,
        token,
        proxy,
        now,
    )
    .await
    {
        Err(Failure::Expired) => return Err(Failure::Expired),
        Err(Failure::Other(error)) => Err(error),
        Ok(summary) => Ok(summary),
    };
    if let Ok(Some(usage)) = &summary {
        if usage.five_hour.is_some() || usage.one_week.is_some() {
            return Ok(usage.clone());
        }
    }
    let models = quota(
        network,
        api::FETCH_MODELS_PATH,
        api::available_models,
        token,
        proxy,
        now,
    )
    .await;
    match (summary, models) {
        (_, Err(Failure::Expired)) => Err(Failure::Expired),
        (Ok(Some(mut usage)), Ok(Some(models))) => {
            usage.five_hour = models.five_hour;
            if usage.third_party_five_hour.is_none() && usage.third_party_one_week.is_none() {
                usage.third_party_five_hour = models.third_party_five_hour;
            }
            Ok(usage)
        }
        (Ok(Some(usage)), _) | (_, Ok(Some(usage))) => Ok(usage),
        (Err(error), _) | (_, Err(Failure::Other(error))) => Err(Failure::Other(error)),
        (Ok(None), Ok(None)) => Err(Failure::Other("Antigravity reported no quota".into())),
    }
}
/// The tokens of a renewal, to put in any document of its login.
struct Renewed {
    access: String,
    expires_at: i64,
    lifetime: i64,
    refresh: Option<String>,
}
impl Renewed {
    fn apply(&self, document: &mut Value) {
        login::apply_renewal(
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
    let refresh = login::refresh_token(document).ok_or(SIGN_IN_EXPIRED)?;
    let (status, body) = network.renew(&refresh, proxy).await?;
    match status {
        200..=299 => {
            let access = body["access_token"]
                .as_str()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .ok_or("Google returned no access token")?;
            let lifetime = body["expires_in"]
                .as_i64()
                .filter(|s| *s > 0)
                .unwrap_or(3600);
            let renewed = Renewed {
                access: access.to_owned(),
                expires_at: now_unix_seconds() + lifetime,
                lifetime,
                refresh: body["refresh_token"].as_str().map(str::to_owned),
            };
            renewed.apply(document);
            Ok(renewed)
        }
        // invalid_grant: the login was signed out or revoked.
        400 | 401 | 403 => Err(SIGN_IN_EXPIRED.into()),
        _ => Err(status_error("Google token request", status, &body)),
    }
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
    /// Antigravity's login: none when absent; an error when it cannot be
    /// read or is no login.
    async fn antigravity_read_live(&self, access: KeychainAccess) -> Result<Option<Document>> {
        match self.antigravity_live().read(access).await? {
            None => Ok(None),
            Some(bytes) => login::decode(&bytes)
                .map(Some)
                .ok_or(CREDENTIALS_INVALID.into()),
        }
    }
    /// The pool, following the login Antigravity uses.
    pub(crate) async fn antigravity_accounts(&self) -> Result<Vec<AccountSummary>> {
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
        if follow_live(&mut store, live.as_ref()) {
            self.antigravity_save(&store)?;
        }
        let mut summaries: Vec<_> = store
            .accounts
            .iter()
            .map(|a| a.summary(store.current_account_id.as_deref() == Some(a.id.as_str())))
            .collect();
        crate::ranking::sort_for_display(&mut summaries);
        if let Some(schedule) = self.schedule(Pool::Antigravity) {
            for card in &mut summaries {
                let account = store.accounts.iter().find(|a| a.id == card.id);
                card.next_refresh_at = account
                    .filter(|a| !a.first_request_pending)
                    .map(|a| schedule.next_refresh_at(true, a.updated_at + schedule.interval));
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
        let email = login::email(&document.value);
        let found = store.accounts.iter_mut().find(|a| {
            login::same_login(&a.document, &document.value) || email.is_some() && a.email == email
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
        login::refresh_token(&document).ok_or(CREDENTIALS_INVALID)?;
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
    /// Gives Antigravity this account's login. A login there that the pool
    /// does not hold is kept in `antigravity-before-switch.json` first.
    pub(crate) async fn antigravity_switch_account(
        &self,
        id: &str,
        access: KeychainAccess,
    ) -> Result<SwitchExecution> {
        let _op = self.antigravity.operations.lock().await;
        let live = self.antigravity_read_live(access).await?;
        let _lock = self.antigravity.lock.lock().await;
        let mut store = self.antigravity_load()?;
        follow_live(&mut store, live.as_ref());
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
        self.antigravity_live()
            .write(
                login::encode(&Document {
                    value: target.document.clone(),
                    wrapped: target.wrapped,
                }),
                access,
            )
            .await?;
        store.current_account_id = Some(target.id);
        self.antigravity_save(&store)?;
        Ok(SwitchExecution::default())
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
            || login::expiry(&account.document).is_some_and(|e| e <= now_unix_seconds()))
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
        let renewed = login::same_login(&live.value, document)
            && login::access_token(&live.value).is_some()
            && login::access_token(&live.value) != login::access_token(document)
            && login::expiry(&live.value).is_none_or(|e| e > now + EXPIRY_MARGIN);
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
        let renewed = renew(network, document, proxy).await?;
        self.antigravity_write_back(&renewed, document).await;
        Ok(())
    }
    /// Puts a renewal in Antigravity's login, in the item's own form, when
    /// the item still holds the renewed login and no token lasting longer;
    /// never another login or a newer token. Silent: no authorization UI,
    /// and a failure only logged, for Copool's store keeps the renewal.
    async fn antigravity_write_back(&self, renewed: &Renewed, document: &Value) {
        let outcome: Result<()> = async {
            let Some(mut live) = self
                .antigravity_read_live(KeychainAccess::Background)
                .await?
            else {
                return Ok(());
            };
            if !login::same_login(&live.value, document)
                || login::expiry(&live.value).unwrap_or(0) > renewed.expires_at
                || login::access_token(&live.value).as_deref() == Some(renewed.access.as_str())
            {
                return Ok(());
            }
            renewed.apply(&mut live.value);
            self.antigravity_live()
                .write(login::encode(&live), KeychainAccess::Background)
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
        let forced = renew_now.is_some();
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
            let expired = login::expiry(&document).is_some_and(|e| e <= now + EXPIRY_MARGIN)
                || login::access_token(&document).is_none();
            if expired {
                if !may_renew {
                    return Err(RENEWAL_OFF.into());
                }
                self.antigravity_renew(network, &mut document, &proxy)
                    .await?;
                renewed = true;
            }
            let token = login::access_token(&document).unwrap_or_default();
            match fetch_usage(network, &token, &proxy, now).await {
                Ok(usage) => Ok(Some(usage)),
                Err(Failure::Other(error)) => Err(error),
                Err(Failure::Expired) if renewed => Err(SIGN_IN_EXPIRED.into()),
                Err(Failure::Expired) => {
                    // Antigravity may have renewed the login since it was
                    // read: its token is tried once before renewing.
                    if let Some(live) = self.antigravity_live_renewal(&document, now).await {
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
                    self.antigravity_renew(network, &mut document, &proxy)
                        .await?;
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
        // The account's name and plan, with the token that worked.
        let token = login::access_token(&document).unwrap_or_default();
        let (mut identity, mut plan, mut project) = (None, None, None);
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
        }
        let _lock = self.antigravity.lock.lock().await;
        let mut store = self.antigravity_load()?;
        let Some(stored) = store.accounts.iter_mut().find(|a| a.id == id) else {
            return outcome.map(|_| ());
        };
        // Antigravity may have renewed the login meanwhile; keep the newer.
        // Its token taken after a refusal replaces the refused one.
        if renewed
            || login::expiry(&document) > login::expiry(&stored.document)
            || adopted && stored.document == account.document
        {
            stored.document = document;
        }
        if renewed {
            stored.renewal = Some(Renewal::of(stored.renewal).after_renewal());
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
    /// Gemini's when none is), which starts it.
    pub(crate) async fn antigravity_hello(&self, id: &str) -> Result<Hello> {
        let account = self
            .antigravity_load()?
            .accounts
            .into_iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?;
        let Some(token) = login::access_token(&account.document).filter(|_| {
            login::expiry(&account.document).is_none_or(|e| e > now_unix_seconds() + EXPIRY_MARGIN)
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
            401 | 403 => Ok(None),
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
