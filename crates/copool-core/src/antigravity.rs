//! Antigravity account pool (Google AI Pro): the logins Copool keeps, the
//! one Antigravity uses (`login`), the quota of each (`api`) and switching.
//! As in the Codex pool, the logins are kept in the pool's file in Copool's
//! data directory, with the pool's Sub2api providers (`sub2api`), whose
//! cards put agy on the Gemini API. The local proxy serves agy in that mode.
use crate::{
    application::{Application, SwitchExecution},
    login_store::{Account, Asked, LoginPool, Logins, EXPIRY_MARGIN},
    models::{AccountSummary, Renewal, RENEWAL_OFF},
    pool::{Hello, Pool, PoolAccount},
    utils::now_unix_seconds,
    vault::KeychainAccess,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};

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
    pub(crate) logins: crate::login_store::Locks,
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
            logins: Default::default(),
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
/// What the pool adds to its accounts.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Own {
    /// Google's id of the account, once a refresh asked for it.
    #[serde(default)]
    google_id: Option<String>,
    /// Whether the item wraps the document as go-keyring does.
    #[serde(default)]
    wrapped: bool,
    /// The Cloud Code project of its messages, as last reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<String>,
}
type Stored = Account<Own>;

/// The Antigravity pool's facts for its store: agy's item in the system's
/// credential store is its Live login.
struct AntigravityLogins<'a>(&'a Application);
impl LoginPool for AntigravityLogins<'_> {
    type Own = Own;
    type Store = Store;
    type Login = Document;
    type Renewed = Renewed;
    const FILE: &'static str = "antigravity-accounts.json";
    const NAME: &'static str = "Antigravity";
    const UNNAMED: &'static str = UNNAMED;
    const SIGN_IN_EXPIRED: &'static str = SIGN_IN_EXPIRED;
    const LIVE_UNREADABLE: &'static str =
        "Cannot read the Antigravity login; allow access before renewing the account it uses";

    fn app(&self) -> &Application {
        self.0
    }
    fn locks(&self) -> &crate::login_store::Locks {
        &self.0.antigravity.logins
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
    async fn read_live(&self, access: KeychainAccess) -> Result<Option<Document>> {
        self.0.antigravity_read_live(access).await
    }
    async fn write_live(&self, login: &Document, access: KeychainAccess) -> Result<()> {
        self.0
            .antigravity_live()
            .write(pool_login::antigravity::encode(login), access)
            .await
    }
    fn document(login: &Document) -> &Value {
        &login.value
    }
    fn document_mut(login: &mut Document) -> &mut Value {
        &mut login.value
    }
    fn login_of(account: &Stored) -> Document {
        Document {
            value: account.document.clone(),
            wrapped: account.own.wrapped,
        }
    }
    fn keep(account: &mut Stored, login: Document) {
        account.document = login.value;
        account.own.wrapped = login.wrapped;
    }
    fn access_token(document: &Value) -> Option<String> {
        pool_login::antigravity::access_token(document)
    }
    fn refresh_token(document: &Value) -> Option<String> {
        pool_login::antigravity::refresh_token(document)
    }
    fn expiry(document: &Value) -> Option<i64> {
        pool_login::antigravity::expiry(document)
    }
    fn email(document: &Value) -> Option<String> {
        pool_login::antigravity::email(document)
    }
    /// One refresh token: the document names no account of its own.
    fn same_login(a: &Value, b: &Value) -> bool {
        pool_login::antigravity::same_login(a, b)
    }
    /// Its login, or one of the same Google account's email: Google may
    /// have rotated the refresh token.
    fn same_account(account: &Stored, login: &Document) -> bool {
        let document = Self::document(login);
        let email = pool_login::antigravity::email(document);
        Self::same_login(&account.document, document) || email.is_some() && account.email == email
    }
    fn account_id(account: &Stored) -> String {
        account.own.google_id.clone().unwrap_or_default()
    }
    async fn renew(&self, document: &mut Value, proxy: &str) -> Result<Renewed> {
        renew(&*self.0.antigravity_network(), document, proxy).await
    }
    fn apply(renewed: &Renewed, document: &mut Value) {
        renewed.apply(document);
    }
}

/// Folds a card just named into the pool's card of the same Google account:
/// its newer login and usage stay, as does the selection.
fn merge_identified(store: &mut Store, id: &str) {
    let Some(index) = store.accounts.iter().position(|a| a.id == id) else {
        return;
    };
    let Some(key) = store.accounts[index].own.google_id.clone() else {
        return;
    };
    let Some(other) = store
        .accounts
        .iter()
        .position(|a| a.id != id && a.own.google_id.as_deref() == Some(key.as_str()))
    else {
        return;
    };
    let merged = store.accounts.remove(index);
    let other = if other > index { other - 1 } else { other };
    let existing = &mut store.accounts[other];
    existing.document = merged.document;
    existing.own.wrapped = merged.own.wrapped;
    existing.usage = merged.usage;
    existing.usage_error = merged.usage_error;
    existing.plan = merged.plan.or(existing.plan.take());
    existing.own.project = merged.own.project.or(existing.own.project.take());
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
/// A usage request's answer, as the store reads it.
fn asked<T>(answer: std::result::Result<T, Failure>) -> Asked<T> {
    match answer {
        Ok(answer) => Asked::Answered(answer),
        Err(Failure::Expired) => Asked::Refused,
        Err(Failure::Denied(error) | Failure::Other(error)) => Asked::Failed(error),
    }
}
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
    fn antigravity_logins(&self) -> Logins<AntigravityLogins<'_>> {
        Logins(AntigravityLogins(self))
    }
    fn antigravity_load(&self) -> Result<Store> {
        self.antigravity_logins().load()
    }
    fn antigravity_save(&self, store: &Store) -> Result<()> {
        self.antigravity_logins().save(store)
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
        let store = self.antigravity_logins().followed().await?;
        let api = sub2api::api_key_mode(&self.antigravity_settings_path());
        let provider = self.antigravity_provider(&store);
        let mut summaries: Vec<_> = store
            .accounts
            .iter()
            .map(|a| {
                let current = !api && store.current_account_id.as_deref() == Some(a.id.as_str());
                Logins::<AntigravityLogins>::summary(a, current)
            })
            .chain(self.provider_cards(Pool::Antigravity, provider)?)
            .collect();
        crate::ranking::sort_for_display(&mut summaries);
        Ok(summaries)
    }
    /// The provider agy uses: on the Gemini API, the provider last applied;
    /// else none, its login.
    fn antigravity_provider<'a>(&self, store: &'a Store) -> Option<&'a str> {
        let api = sub2api::api_key_mode(&self.antigravity_settings_path());
        store.settings_provider.as_deref().filter(|_| api)
    }
    /// The provider agy uses now, if any.
    pub(crate) fn antigravity_provider_in_use(&self) -> Option<String> {
        let store = self.antigravity_load().ok()?;
        self.antigravity_provider(&store).map(str::to_owned)
    }
    /// Imports the login Antigravity uses.
    pub(crate) async fn antigravity_import_current(&self) -> Result<usize> {
        let fresh = self
            .antigravity_logins()
            .import_live(KeychainAccess::Interactive, LOGIN_NOT_FOUND)
            .await?;
        if let Some(id) = fresh {
            let _ = self.antigravity_refresh(&id, None).await;
        }
        Ok(1)
    }
    /// Stores a browser sign-in's login without switching to it.
    pub(crate) async fn antigravity_import_login(&self, document: Value) -> Result<usize> {
        pool_login::antigravity::refresh_token(&document).ok_or(CREDENTIALS_INVALID)?;
        self.antigravity_import(
            vec![Document {
                value: document,
                wrapped: cfg!(target_os = "macos"),
            }],
            crate::login_store::Origin::SignIn,
        )
        .await
    }
    /// Imports exported logins without switching to them; the whole
    /// document is checked before any is stored.
    pub(crate) async fn antigravity_import_json(&self, content: &str) -> Result<usize> {
        self.antigravity_import(
            parse_import(content).ok_or(CREDENTIALS_INVALID)?,
            crate::login_store::Origin::Export,
        )
        .await
    }
    async fn antigravity_import(
        &self,
        documents: Vec<Document>,
        origin: crate::login_store::Origin,
    ) -> Result<usize> {
        let count = documents.len();
        for id in self.antigravity_logins().import(documents, origin).await? {
            let _ = self.antigravity_refresh(&id, None).await;
        }
        Ok(count)
    }
    pub(crate) async fn antigravity_delete_account(&self, id: &str) -> Result<()> {
        self.antigravity_logins().delete(id, |_, _| {}).await
    }
    pub(crate) async fn antigravity_edit_accounts(
        &self,
        edit: impl FnOnce(&mut [&mut dyn PoolAccount]) -> Result<bool>,
    ) -> Result<()> {
        self.antigravity_logins().edit(edit).await
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
        let _op = self.antigravity.logins.operations.lock().await;
        if crate::sub2api::is_card(Pool::Antigravity, id) {
            return self.antigravity_switch_sub2api(id).await;
        }
        // An item that cannot be read would not take the write either.
        let live = self.antigravity_read_live(access).await?;
        // agy keeps to the Gemini API while its settings say so.
        let path = self.antigravity_settings_path();
        let target = self
            .antigravity_logins()
            .switch(
                id,
                live,
                access,
                "antigravity-before-switch.json",
                |store| sub2api::leave_api_key(&path, store),
                |left| sub2api::restore_settings(&path, left),
                |store| {
                    store.settings_record = None;
                    store.settings_provider = None;
                },
            )
            .await?;
        drop(_op);
        let identity = Identity {
            email: target.email,
            account_id: target.own.google_id,
            ..Identity::default()
        };
        let credential = pool_login::antigravity::credential(&target.document, &identity);
        self.after_switch(Pool::Antigravity, Some(credential)).await
    }
    /// Whether the account failed: its usage refused, or its token expired.
    pub(crate) async fn antigravity_login_failed(&self, id: &str) -> Result<bool> {
        self.antigravity_logins().login_failed(id)
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
        let logins = self.antigravity_logins();
        let _op = self.antigravity.logins.operations.lock().await;
        let begun = logins.begin(id).await?;
        let account = &begun.account;
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
        let asking = proxy.as_str();
        let used = logins
            .use_token(&begun, forced, &proxy, now, |document| async move {
                let token = pool_login::antigravity::access_token(&document).unwrap_or_default();
                asked(fetch_usage(network, &token, asking, now).await)
            })
            .await;
        // The account's name and plan, with the token that worked.
        let token = pool_login::antigravity::access_token(&used.document).unwrap_or_default();
        let (mut identity, mut plan, mut project) = (None, None, None);
        // The credits of a successful `loadCodeAssist`; when it fails, the
        // card keeps those it had.
        let mut credits = None;
        if matches!(used.outcome, Ok(Some(_))) {
            if account.own.google_id.is_none() {
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
        logins
            .record(&begun, &used, |store, index| {
                let stored = &mut store.accounts[index];
                if let Some(body) = identity {
                    let email = body["email"].as_str().map(str::to_owned);
                    stored.own.google_id = body["id"]
                        .as_str()
                        .map(str::to_owned)
                        .or(stored.own.google_id.take());
                    if let Some(email) = email {
                        stored.label = email.clone();
                        stored.email = Some(email);
                    }
                }
                if let Some(plan) = plan {
                    stored.plan = Some(plan);
                }
                if let Some(project) = project {
                    stored.own.project = Some(project);
                }
                let kept = stored.usage.as_ref().and_then(|u| u.credits.clone());
                Logins::<AntigravityLogins>::record_usage(stored, &used.outcome, now);
                if let (Ok(Some(_)), Some(usage)) = (&used.outcome, stored.usage.as_mut()) {
                    usage.credits = credits.unwrap_or(kept);
                }
                merge_identified(store, id);
            })
            .await?;
        if let Some(reported) = renew_now {
            // A renewal the tool made meanwhile serves as one.
            *reported = used.renewed || used.adopted;
        }
        used.outcome.map(|_| ())
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
        let project = match account.own.project.clone() {
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
