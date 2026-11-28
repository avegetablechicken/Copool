//! Codex account pool: the logins Copool keeps, in `codex-accounts.json`
//! through `crate::login_store`, and the one Codex uses, its `auth.json`.
//! An account is one workspace of a ChatGPT login (`pool_login::codex`);
//! its workspace may be deactivated, and the pool keeps a directory of the
//! workspaces it has seen (`workspace_directory`). Its Sub2api
//! providers are kept beside the accounts (`sub2api`).
pub(crate) mod api;
pub(crate) mod cliproxyapi;
pub mod config;
mod sub2api;
#[cfg(test)]
pub(crate) use sub2api::CODEX_PROVIDERS_MIGRATED;
pub(crate) use sub2api::{codex_providers, ProviderAdapter};
mod import_login;
mod proxy;
mod workspace_directory;
pub mod workspaces;

use crate::{
    application::{Application, Paths, SwitchExecution},
    login_store::{Account, Asked, LoginPool, Logins, Standing},
    models::{AccountSummary, Renewal, StoredAccount, UsageSnapshot, RENEWAL_OFF},
    persistence::write_json,
    pool::{Edit, Hello, Pool, PoolOps, Response},
    pool_login::{codex, Identity},
    provider_registry::ProviderRecord,
    utils::now_unix_seconds,
    vault::KeychainAccess,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;

type Result<T> = std::result::Result<T, String>;

/// The pool's file in Copool's data directory.
pub(crate) const FILE: &str = "codex-accounts.json";
pub(crate) const SIGN_IN_EXPIRED: &str = "error.accounts.sign_in_expired";
pub(crate) const SAY_HI_REQUIRES_PLUS: &str = "Say hi requires a ChatGPT Plus account";
/// Whether an account of this plan can say hi: only a Plus account's hi
/// starts its window, whoever's the account is.
pub(crate) fn greets(plan: Option<&str>) -> Result<()> {
    if plan.unwrap_or_default().trim().eq_ignore_ascii_case("plus") {
        Ok(())
    } else {
        Err(SAY_HI_REQUIRES_PLUS.into())
    }
}

/// What the pool adds to its accounts.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Own {
    /// The workspace the login signs in to: ChatGPT's account id.
    #[serde(default)]
    pub(crate) workspace_id: String,
    /// Whose login it is in that workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) principal_id: Option<String>,
    /// The workspace's name, as its login or ChatGPT names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) team_name: Option<String>,
}
pub(crate) type Stored = Account<Own>;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Store {
    pub(crate) accounts: Vec<Stored>,
    /// The account whose login Codex was last seen to use.
    pub(crate) current_account_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) providers: Vec<ProviderRecord>,
    /// The workspaces seen, those without an account included.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) workspace_directory: Vec<Value>,
}

/// The store, read by its path, for readers without the application.
pub(crate) fn load(paths: &Paths) -> Result<Store> {
    match fs::read(paths.data_dir.join(FILE)) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|e| format!("Invalid Codex account store: {e}"))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
        Err(e) => Err(e.to_string()),
    }
}

/// Copies the accounts `accounts.json` kept to the pool's own file, once:
/// while that file does not exist. `accounts.json` stays as it was, for an
/// earlier version to read (docs/adr/0001-keep-codex-provider-data-after-migration.md).
pub(crate) fn migrate(paths: &Paths) -> Result<()> {
    let target = paths.data_dir.join(FILE);
    if target.exists() || !paths.store().exists() {
        return Ok(());
    }
    // Read as it is: the old loader rewrote the file it read.
    let bytes = fs::read(paths.store()).map_err(|e| e.to_string())?;
    let Ok(mut old) = serde_json::from_slice::<crate::models::AccountsStore>(&bytes) else {
        log::warn!("accounts.json cannot be read; the Codex pool starts empty");
        return Ok(());
    };
    // Each account's proxy, as the settings kept it beside the file.
    let settings: Value = fs::read(paths.data_dir.join("settings.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    for account in &mut old.accounts {
        if let Some(proxy) = settings["accountProxyURLs"][&account.id].as_str() {
            account.proxy_url = proxy.to_owned();
        }
    }
    let text = |account: &StoredAccount, key: &str| {
        account
            .extra
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let accounts = old
        .accounts
        .iter()
        .map(|account| {
            let standing = match (
                text(account, "workspaceStatus").as_deref(),
                text(account, "displayStatus").as_deref(),
            ) {
                (_, Some("deleted")) => Some(Standing::Deleted),
                (Some("deactivated"), _) | (_, Some("deactivated")) => Some(Standing::Deactivated),
                _ => None,
            };
            Stored {
                id: account.id.clone(),
                label: account.label.clone(),
                email: account.email.clone(),
                document: account.auth_json.clone(),
                own: Own {
                    workspace_id: account.account_id.clone(),
                    principal_id: account.principal_id.clone(),
                    team_name: text(account, "teamName"),
                },
                proxy_url: account.proxy_url.clone(),
                added_at: account.added_at,
                updated_at: account.updated_at,
                usage: account.usage.clone(),
                usage_error: account.usage_error.clone(),
                plan: account.plan_type.clone(),
                first_request_pending: account.awaiting_first_request(),
                renewal: account
                    .extra
                    .get("renewal")
                    .and_then(Value::as_str)
                    .and_then(|chosen| Renewal::parse(chosen).ok()),
                standing,
            }
        })
        .map(|mut account: Stored| {
            // What the login says of itself wins over what was kept of it.
            if let Ok(extracted) = codex::extract_auth(&account.document) {
                account.own.workspace_id = extracted.account_id;
                if extracted.principal_id.is_some() {
                    account.own.principal_id = extracted.principal_id;
                }
                if extracted.email.is_some() {
                    account.email = extracted.email;
                }
                if extracted.plan_type.is_some() {
                    account.plan = extracted.plan_type;
                }
                if extracted.team_name.is_some() {
                    account.own.team_name = extracted.team_name;
                }
            }
            account
        })
        .collect();
    let store = Store {
        accounts,
        current_account_id: old
            .extra
            .get("currentAccountId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        providers: old
            .extra
            .get("providers")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        workspace_directory: old
            .extra
            .get("workspaceDirectory")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    };
    write_json(&target, &store)
}

/// The Codex pool's facts for its store: Codex's `auth.json` is its Live
/// login.
pub(crate) struct CodexLogins<'a>(pub(crate) &'a Application);
impl LoginPool for CodexLogins<'_> {
    type Own = Own;
    type Store = Store;
    type Login = Value;
    type Renewed = Value;
    const FILE: &'static str = FILE;
    const NAME: &'static str = "Codex";
    const UNNAMED: &'static str = "Codex";
    const SIGN_IN_EXPIRED: &'static str = SIGN_IN_EXPIRED;
    const LIVE_UNREADABLE: &'static str =
        "Cannot read Codex's auth.json; fix it before renewing the account it uses";

    fn app(&self) -> &Application {
        self.0
    }
    fn locks(&self) -> &crate::login_store::Locks {
        &self.0.codex
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
    /// Codex's `auth.json`: an API key alone signs in to no account.
    async fn read_live(&self, _: KeychainAccess) -> Result<Option<Value>> {
        let live = self.0.codex_read_live();
        #[cfg(test)]
        {
            let delay = self
                .0
                .codex_live_read_delay_ms
                .load(std::sync::atomic::Ordering::Relaxed);
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        Ok(live?.filter(|document| codex::extract_auth(document).is_ok()))
    }
    /// Writes the login as Codex reads it, keeping the API key `auth.json`
    /// holds beside it.
    async fn write_live(&self, login: &Value, _: KeychainAccess) -> Result<()> {
        let mut next = codex::normalized_auth_for_write(login.clone())?;
        let key = self
            .0
            .codex_read_live()
            .ok()
            .flatten()
            .and_then(|current| current.get("OPENAI_API_KEY").cloned())
            .filter(|key| !key.is_null());
        if let Some(key) = key.filter(|_| next.get("OPENAI_API_KEY").is_none_or(Value::is_null)) {
            next["OPENAI_API_KEY"] = key;
        }
        write_json(&self.0.paths.codex_auth, &next)
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
        if let Ok(extracted) = codex::extract_auth(&login) {
            account.own.workspace_id = extracted.account_id;
            account.own.principal_id = extracted.principal_id;
            if let Some(name) = extracted.team_name {
                account.own.team_name = Some(name);
            }
            if extracted.plan_type.is_some() {
                account.plan = extracted.plan_type;
            }
        }
        account.document = login;
    }
    fn access_token(document: &Value) -> Option<String> {
        codex::extract_auth(document)
            .ok()
            .map(|e| e.access_token)
            .filter(|t| !t.trim().is_empty())
    }
    fn refresh_token(document: &Value) -> Option<String> {
        codex::auth_token_object(document)?
            .get("refresh_token")?
            .as_str()
            .filter(|t| !t.trim().is_empty())
            .map(str::to_owned)
    }
    fn expiry(document: &Value) -> Option<i64> {
        codex::access_token_expiry(document)
    }
    fn email(document: &Value) -> Option<String> {
        codex::extract_auth(document).ok()?.email
    }
    /// Its email, else its workspace.
    fn name(document: &Value) -> Option<String> {
        let extracted = codex::extract_auth(document).ok()?;
        extracted.email.or(Some(extracted.account_id))
    }
    /// One principal in one workspace: a login of several workspaces is an
    /// account in each.
    fn same_login(a: &Value, b: &Value) -> bool {
        codex::account_identity(a).is_ok()
            && codex::account_identity(a).ok() == codex::account_identity(b).ok()
    }
    /// Refreshed later: Codex's logins tell their generation by
    /// `last_refresh`.
    fn newer(a: &Value, b: &Value) -> bool {
        codex::newer_login(a, b)
    }
    fn deactivated(error: &str) -> bool {
        api::deactivated(error)
    }
    fn account_id(account: &Stored) -> String {
        account.own.workspace_id.clone()
    }
    fn summarize(account: &Stored, summary: &mut AccountSummary) {
        summary.team_name = account.own.team_name.clone();
        summary.usage = account
            .usage
            .clone()
            .map(UsageSnapshot::with_matching_windows);
    }
    async fn renew(&self, document: &mut Value, proxy: &str) -> Result<Value> {
        let renewed = self
            .0
            .network
            .refresh(document, proxy)
            .await
            .map_err(|error| {
                if api::expired_error(&error) {
                    SIGN_IN_EXPIRED.to_owned()
                } else {
                    error
                }
            })?;
        *document = renewed.clone();
        Ok(renewed)
    }
    /// The renewal's tokens and refresh time; Codex's other members stay.
    fn apply(renewed: &Value, document: &mut Value) {
        document["tokens"] = renewed["tokens"].clone();
        if let Some(refreshed) = renewed.get("last_refresh") {
            document["last_refresh"] = refreshed.clone();
        }
    }
}

/// The pool's own parts, which `pool.rs` reaches it through.
pub(crate) fn ops(app: &Application) -> Box<dyn PoolOps + '_> {
    Box::new(CodexLogins(app))
}
impl PoolOps for CodexLogins<'_> {
    fn cards(&self) -> Response<'_, Vec<AccountSummary>> {
        Box::pin(self.0.codex_accounts())
    }
    fn provider_in_use(&self) -> Option<String> {
        self.0.codex_provider_in_use().ok().flatten()
    }
    fn authorize(&self) -> Response<'_, ()> {
        Box::pin(self.0.authorize_keychain())
    }
    fn import_current(&self) -> Response<'_, usize> {
        Box::pin(self.0.codex_import_current())
    }
    fn import_json<'a>(&'a self, content: &'a str) -> Response<'a, usize> {
        Box::pin(self.0.codex_import_json(content))
    }
    fn import_sign_in<'a>(
        &'a self,
        document: Value,
        workspace: Option<&'a str>,
    ) -> Response<'a, usize> {
        Box::pin(self.0.codex_import_login(document, workspace))
    }
    fn delete<'a>(&'a self, id: &'a str) -> Response<'a, ()> {
        Box::pin(self.0.codex_delete_account(id))
    }
    fn edit<'a>(&'a self, edit: Edit<'a>) -> Response<'a, ()> {
        Box::pin(async move { self.0.codex_logins().edit(edit).await })
    }
    fn switch<'a>(&'a self, id: &'a str, access: KeychainAccess) -> Response<'a, SwitchExecution> {
        Box::pin(self.0.codex_switch_account(id, access))
    }
    fn login_failed<'a>(&'a self, id: &'a str) -> Response<'a, bool> {
        Box::pin(async move { self.0.codex_logins().login_failed(id) })
    }
    fn refresh<'a>(&'a self, id: &'a str, renew_now: Option<&'a mut bool>) -> Response<'a, ()> {
        Box::pin(self.0.codex_refresh(id, renew_now))
    }
    fn hello<'a>(&'a self, id: &'a str, _: Option<bool>) -> Response<'a, Hello> {
        Box::pin(self.0.codex_hello(id))
    }
}

impl Application {
    pub(crate) fn codex_logins(&self) -> Logins<CodexLogins<'_>> {
        Logins(CodexLogins(self))
    }
    pub(crate) fn codex_load(&self) -> Result<Store> {
        self.codex_logins().load()
    }
    /// Codex's `auth.json`: none when absent; an error when it cannot be read.
    pub(crate) fn codex_read_live(&self) -> Result<Option<Value>> {
        match fs::read(&self.paths.codex_auth) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| format!("Invalid Codex auth: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    /// The pool, following the login Codex uses in `auth.json`; none is
    /// current while Codex is on a provider, whose cards then are.
    async fn codex_accounts(&self) -> Result<Vec<AccountSummary>> {
        let store = self.codex_logins().followed().await?;
        let official = config::resolve_path(&self.codex_config_path()).is_official();
        let mut summaries: Vec<_> = store
            .accounts
            .iter()
            .map(|a| {
                let current =
                    official && store.current_account_id.as_deref() == Some(a.id.as_str());
                Logins::<CodexLogins>::summary(a, current)
            })
            .collect();
        // Sub2api cards are current while Codex uses their provider.
        let provider = self.codex_provider_in_use()?;
        summaries.extend(self.provider_cards(Pool::Codex, provider.as_deref())?);
        crate::ranking::sort_for_display(&mut summaries);
        Ok(summaries)
    }
    /// Imports the login Codex uses.
    async fn codex_import_current(&self) -> Result<usize> {
        let fresh = self
            .codex_logins()
            .import_live(
                KeychainAccess::Interactive,
                "Codex auth.json not found; log in with Codex first",
            )
            .await?;
        if let Some(id) = fresh {
            let _ = self.codex_refresh(&id, None).await;
        }
        self.reconcile_workspace_names(true).await?;
        Ok(1)
    }
    /// Imports `auth.json` documents, a list of them, or an account store's
    /// `{"accounts": [...]}`, each perhaps a card with its `authJson`; the
    /// whole batch is checked first. Those not waiting are refreshed.
    async fn codex_import_json(&self, content: &str) -> Result<usize> {
        let value: Value = serde_json::from_str(content).map_err(|e| e.to_string())?;
        let entries: Vec<Value> = if let Some(accounts) = value.get("accounts") {
            accounts
                .as_array()
                .ok_or("accounts must be an array")?
                .clone()
        } else if let Some(values) = value.as_array() {
            values.clone()
        } else {
            vec![value]
        };
        let mut logins = Vec::new();
        for entry in entries {
            let document = codex::normalize_imported_auth_json(
                entry.get("authJson").unwrap_or(&entry).clone(),
            );
            let extracted = codex::extract_auth(&document)?;
            if extracted.access_token.trim().is_empty() || extracted.account_id.trim().is_empty() {
                return Err("Empty access token or account ID".into());
            }
            logins.push(document);
        }
        let count = logins.len();
        let fresh = self
            .codex_logins()
            .import(logins, crate::login_store::Origin::Export)
            .await?;
        self.codex_logins()
            .change(|store| {
                // A workspace with an account is no longer only seen.
                let authorized: Vec<_> = store
                    .accounts
                    .iter()
                    .filter(|a| a.standing.is_none())
                    .map(|a| a.own.workspace_id.trim().to_lowercase())
                    .collect();
                store.workspace_directory.retain(|v| {
                    !authorized.contains(
                        &v["workspaceId"]
                            .as_str()
                            .unwrap_or_default()
                            .trim()
                            .to_lowercase(),
                    )
                });
                Ok(())
            })
            .await?;
        for id in fresh {
            let _ = self.codex_refresh(&id, None).await;
        }
        Ok(count)
    }
    /// Removes an account; a deactivated workspace's stays listed as deleted,
    /// and so in the workspace directory.
    async fn codex_delete_account(&self, id: &str) -> Result<()> {
        self.codex_logins()
            .delete(id, |store, index| {
                let account = &store.accounts[index];
                let workspace = account.own.workspace_id.clone();
                let now = now_unix_seconds();
                let entry = json!({"workspaceId":workspace,"workspaceName":account.own.team_name,"email":account.email,"planType":account.plan,"kind":"workspace","source":"deactivated","status":"deactivated","visibility":"deleted","lastSeenAt":now,"lastStatusCheckedAt":now});
                let rows = &mut store.workspace_directory;
                if let Some(old) = rows.iter_mut().find(|v| v["workspaceId"] == json!(workspace)) {
                    old["visibility"] = "deleted".into();
                } else {
                    rows.push(entry);
                }
            })
            .await
    }
    /// Gives Codex this account's login, on its official provider, or a
    /// Sub2api card's provider. A login there that the pool does not hold
    /// is kept in `auth-before-switch.json` first.
    async fn codex_switch_account(
        &self,
        id: &str,
        access: KeychainAccess,
    ) -> Result<SwitchExecution> {
        let _op = self.codex.operations.lock().await;
        if crate::provider_accounts::is_card(Pool::Codex, id) {
            let (provider, account) = self.card(Pool::Codex, id)?;
            if !account.status.eq_ignore_ascii_case("active") {
                return Err("Account is deactivated".into());
            }
            // Codex selects the `config.toml` provider it is named after.
            config::switch(&self.codex_config_path(), &provider.name)?;
            self.publish();
            let turn = self.switch_turn(Pool::Codex).await;
            drop(_op);
            let mut execution = self.after_switch(turn, Pool::Codex, None).await?;
            execution.provider = Some(provider.name);
            return Ok(execution);
        }
        let target = self.codex_switch_login(id, access).await?;
        let turn = self.switch_turn(Pool::Codex).await;
        drop(_op);
        let login = codex::normalized_auth_for_write(target.document)?;
        let credential = codex::credential(&login, &Identity::default());
        self.after_switch(turn, Pool::Codex, Some(credential)).await
    }
    /// Writes the account's login to `auth.json`, putting Codex back on its
    /// official provider; undone should the write fail.
    async fn codex_switch_login(&self, id: &str, access: KeychainAccess) -> Result<Stored> {
        // An auth.json Codex cannot use is written over.
        let live = self
            .codex_read_live()
            .ok()
            .flatten()
            .filter(|document| codex::extract_auth(document).is_ok());
        let config = self.codex_config_path();
        self.codex_logins()
            .switch(
                id,
                live,
                access,
                "auth-before-switch.json",
                |_| {
                    let selected = config::resolve_path(&config);
                    if selected.is_official() {
                        return Ok(None);
                    }
                    config::switch(&config, "openai")?;
                    Ok(Some(selected.id))
                },
                |left: Option<String>| {
                    if let Some(provider) = left {
                        let _ = config::switch(&config, &provider);
                    }
                },
                |_| {},
            )
            .await
    }
    /// Says hi with a ChatGPT Plus account, with the login Codex renewed if
    /// it is newer.
    async fn codex_hello(&self, id: &str) -> Result<Hello> {
        let account = self
            .codex_load()?
            .accounts
            .into_iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?;
        greets(
            account
                .usage
                .as_ref()
                .and_then(|u| u.plan_type.as_deref())
                .filter(|p| !p.trim().is_empty())
                .or(account.plan.as_deref()),
        )?;
        let live = self
            .codex_read_live()
            .ok()
            .flatten()
            .filter(|live| codex::newer_login(live, &account.document));
        let auth = live.as_ref().unwrap_or(&account.document);
        let expired = codex::access_token_expiry(auth).is_some_and(|at| at <= now_unix_seconds());
        let result = if expired {
            Err("Provided authentication token is expired".into())
        } else {
            let base = crate::usage::chatgpt_base_origin(
                config::chatgpt_base_url(&self.codex_config_path()).as_deref(),
            );
            crate::proxy_service::say_hi(&base, auth, &self.account_proxy(&account.proxy_url)?)
                .await
        };
        // An explicit greeting never preflights or skips based on usage.
        match result {
            Ok(()) => Ok(Hello::Said),
            Err(error) if api::expired_error(&error) => Ok(Hello::Expired(error)),
            Err(error) => Err(error),
        }
    }
    /// Refreshes an account's usage, renewing its token as its renewal
    /// allows; with `renew_now`, only renews it, whatever the choice, and
    /// reports that it did.
    async fn codex_refresh(&self, id: &str, renew_now: Option<&mut bool>) -> Result<()> {
        self.codex_refresh_login(id, true, renew_now, None).await
    }
    /// As `codex_refresh`, but a refresh not forced skips usage fetched
    /// within the last 25 seconds, as Swift's refreshAccounts did.
    #[cfg(test)]
    pub(crate) async fn codex_refresh_account(
        &self,
        id: &str,
        force: bool,
        renew_now: Option<&mut bool>,
    ) -> Result<()> {
        self.codex_refresh_login(id, force, renew_now, None).await
    }
    async fn codex_refresh_login(
        &self,
        id: &str,
        force: bool,
        renew_now: Option<&mut bool>,
        rejected: Option<&str>,
    ) -> Result<()> {
        let forced = renew_now.is_some();
        let _activity = (!forced).then(|| self.begin_refresh(id));
        let logins = self.codex_logins();
        let _op = self.codex.operations.lock().await;
        let begun = logins.begin(id).await?;
        let account = &begun.account;
        if account.first_request_pending {
            return Ok(());
        }
        if let Some(rejected) = rejected {
            if CodexLogins::access_token(&account.document).as_deref() != Some(rejected) {
                return Ok(());
            }
            if Renewal::of(account.renewal) == Renewal::Never {
                return Err(RENEWAL_OFF.into());
            }
        }
        if !forced
            && !force
            && account
                .usage
                .as_ref()
                .is_some_and(|u| now_unix_seconds() - u.fetched_at < 25)
        {
            return Ok(());
        }
        let network = &*self.network;
        let proxy = self.account_proxy(&account.proxy_url)?;
        let now = now_unix_seconds();
        let asking = proxy.as_str();
        let used = logins
            .use_token(&begun, forced, &proxy, now, |document| async move {
                match network.usage(&document, asking).await {
                    Ok(usage) => Asked::Answered(usage),
                    Err(error) if api::expired_error(&error) => Asked::Refused,
                    Err(error) => Asked::Failed(error),
                }
            })
            .await;
        logins
            .record(&begun, &used, |store, index| {
                let stored = &mut store.accounts[index];
                if used.outcome.is_ok() {
                    if let Ok(extracted) = codex::extract_auth(&used.document) {
                        if let Some(name) = extracted.team_name {
                            stored.own.team_name = Some(name);
                        }
                        stored.plan = extracted.plan_type.or(stored.plan.take());
                        stored.email = extracted.email.or(stored.email.take());
                        stored.own.principal_id = extracted.principal_id;
                    }
                }
                Logins::<CodexLogins>::record_usage(stored, &used.outcome, now);
                // Codex's usage names its plan itself.
                if let (Ok(Some(usage)), Some(kept)) = (&used.outcome, stored.usage.as_mut()) {
                    kept.plan_type = usage.plan_type.clone();
                }
            })
            .await?;
        if let Some(reported) = renew_now {
            // A renewal the tool made meanwhile serves as one.
            *reported = used.renewed || used.adopted;
        }
        used.outcome.map(|_| ())
    }
}

#[cfg(test)]
mod tests;
