//! Codex account pool: the logins Copool keeps, in `codex-accounts.json`
//! through `crate::login_store`, and the one Codex uses, its `auth.json`.
//! An account is one workspace of a ChatGPT login (`pool_login::codex`);
//! its workspace may be deactivated, and the pool keeps a directory of the
//! workspaces it has seen (`crate::workspace_directory`). Its Sub2api
//! providers are kept beside the accounts (`crate::providers`).
use crate::{
    account_network,
    application::{Application, Paths, SwitchExecution},
    codex_config,
    login_store::{Account, Asked, LoginPool, Logins, Standing},
    models::{AccountSummary, Renewal, StoredAccount, UsageSnapshot, RENEWAL_OFF},
    persistence::write_json,
    pool::{Pool, PoolAccount},
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

/// The account as the proxy engine and remote nodes keep one.
pub(crate) fn engine_account(account: &Stored) -> Result<StoredAccount> {
    let mut engine: StoredAccount = serde_json::from_value(json!({
        "id": account.id, "label": account.label, "email": account.email,
        "accountId": account.own.workspace_id, "planType": account.plan,
        "authJson": account.document, "addedAt": account.added_at,
        "updatedAt": account.updated_at, "usage": account.usage,
        "usageError": account.usage_error, "principalId": account.own.principal_id,
        "proxyURL": account.proxy_url,
    }))
    .map_err(|e| e.to_string())?;
    if let Some(name) = &account.own.team_name {
        engine.extra.insert("teamName".into(), name.clone().into());
    }
    let (display, workspace) = match account.standing {
        None => ("list", "active"),
        Some(Standing::Deactivated) => ("deactivated", "deactivated"),
        Some(Standing::Deleted) => ("deleted", "deactivated"),
    };
    engine.extra.insert("displayStatus".into(), display.into());
    engine
        .extra
        .insert("workspaceStatus".into(), workspace.into());
    if account.first_request_pending {
        engine
            .extra
            .insert(crate::models::FIRST_REQUEST_PENDING.into(), true.into());
    }
    if let Some(choice) = account.renewal {
        engine.set_renewal(choice);
    }
    Ok(engine)
}

/// The pool's logins as the proxy's candidates: those in use, the one
/// Codex uses marked current, unless Codex is on a provider or the proxy
/// only hosts the API.
pub(crate) fn proxy_candidates(paths: &Paths) -> Result<Vec<StoredAccount>> {
    let store = load(paths)?;
    let settings = crate::repository::load_settings(&paths.data_dir.join("settings.json"))?;
    let official =
        codex_config::resolve_path(&paths.codex_auth.with_file_name("config.toml")).is_official();
    let current = store
        .current_account_id
        .as_deref()
        .filter(|_| official && !settings.local_proxy_host_api_only);
    store
        .accounts
        .iter()
        .filter(|a| a.standing.is_none() && !a.first_request_pending)
        .map(|account| {
            let mut engine = engine_account(account)?;
            if current == Some(account.id.as_str()) {
                engine.extra.insert("copoolCurrent".into(), true.into());
            }
            Ok(engine)
        })
        .collect()
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
        account_network::deactivated(error)
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
                if account_network::expired_error(&error) {
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
    pub(crate) async fn codex_accounts(&self) -> Result<Vec<AccountSummary>> {
        let store = self.codex_logins().followed().await?;
        let official = codex_config::resolve_path(&self.codex_config_path()).is_official();
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
    pub(crate) async fn codex_import_current(&self) -> Result<usize> {
        let fresh = self
            .codex_logins()
            .import_live(
                KeychainAccess::Interactive,
                "Codex auth.json not found; log in with Codex first",
            )
            .await?;
        if let Some(id) = fresh {
            let _ = self.codex_refresh_account(&id, true, None).await;
        }
        self.reconcile_workspace_names(true).await?;
        Ok(1)
    }
    /// Imports `auth.json` documents, a list of them, or an account store's
    /// `{"accounts": [...]}`, each perhaps a card with its `authJson`; the
    /// whole batch is checked first. Those not waiting are refreshed.
    pub(crate) async fn codex_import_json(&self, content: &str) -> Result<usize> {
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
            let _ = self.codex_refresh_account(&id, true, None).await;
        }
        Ok(count)
    }
    pub(crate) async fn codex_edit_accounts(
        &self,
        edit: impl FnOnce(&mut [&mut dyn PoolAccount]) -> Result<bool>,
    ) -> Result<()> {
        self.codex_logins().edit(edit).await
    }
    /// Removes an account; a deactivated workspace's stays listed as deleted,
    /// and so in the workspace directory.
    pub(crate) async fn codex_delete_account(&self, id: &str) -> Result<()> {
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
    pub(crate) async fn codex_switch_account(&self, id: &str) -> Result<SwitchExecution> {
        let _op = self.codex.operations.lock().await;
        if crate::sub2api::is_card(Pool::Codex, id) {
            let (provider, account) = self.card(Pool::Codex, id)?;
            if !account.status.eq_ignore_ascii_case("active") {
                return Err("Account is deactivated".into());
            }
            // Codex selects the `config.toml` provider it is named after.
            codex_config::switch(&self.codex_config_path(), &provider.name)?;
            self.publish();
            drop(_op);
            let mut execution = self.after_switch(Pool::Codex, None).await?;
            execution.provider = Some(provider.name);
            return Ok(execution);
        }
        let target = self.codex_switch_login(id).await?;
        drop(_op);
        let login = codex::normalized_auth_for_write(target.document)?;
        let credential = codex::credential(&login, &Identity::default());
        self.after_switch(Pool::Codex, Some(credential)).await
    }
    /// Writes the account's login to `auth.json`, putting Codex back on its
    /// official provider; undone should the write fail.
    async fn codex_switch_login(&self, id: &str) -> Result<Stored> {
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
                KeychainAccess::Interactive,
                "auth-before-switch.json",
                |_| {
                    let selected = codex_config::resolve_path(&config);
                    if selected.is_official() {
                        return Ok(None);
                    }
                    codex_config::switch(&config, "openai")?;
                    Ok(Some(selected.id))
                },
                |left: Option<String>| {
                    if let Some(provider) = left {
                        let _ = codex_config::switch(&config, &provider);
                    }
                },
                |_| {},
            )
            .await
    }
    /// The account the proxy chose, which it reached with `auth`: unless the
    /// proxy only hosts the API, Codex is switched to it, as any switch is,
    /// under the operations lock: a refresh renewing the account meanwhile
    /// has kept its renewal before the switch reads it. Returns whether it
    /// was.
    pub(crate) async fn codex_follow_proxy(&self, id: &str, auth: &Value) -> Result<bool> {
        if self.preferences()?.host_api_only {
            return Ok(false);
        }
        let _op = self.codex.operations.lock().await;
        let config = self.codex_config_path();
        if let Some(provider) = auth.get("copoolProvider") {
            let provider = provider["id"].as_str().ok_or("Invalid provider")?;
            if codex_config::resolve_path(&config)
                .id
                .eq_ignore_ascii_case(provider)
            {
                return Ok(false);
            }
            codex_config::switch(&config, provider)?;
            self.publish();
            return Ok(true);
        }
        let store = self.codex_load()?;
        let Some(account) = store.accounts.iter().find(|a| a.id == id) else {
            return Ok(false);
        };
        let live = self.codex_read_live().ok().flatten();
        let uses = codex_config::resolve_path(&config).is_official()
            && store.current_account_id.as_deref() == Some(id)
            && live
                .as_ref()
                .is_some_and(|live| CodexLogins::same_login(live, &account.document));
        if uses {
            return Ok(false);
        }
        self.codex_switch_login(id).await?;
        Ok(true)
    }
    /// Whether the account failed: its usage refused, or its token expired.
    pub(crate) async fn codex_login_failed(&self, id: &str) -> Result<bool> {
        self.codex_logins().login_failed(id)
    }
    /// Says hi with a ChatGPT Plus account, with the login Codex renewed if
    /// it is newer.
    pub(crate) async fn codex_hello(&self, id: &str) -> Result<crate::pool::Hello> {
        let account = self
            .codex_load()?
            .accounts
            .into_iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?;
        let plan = account
            .usage
            .as_ref()
            .and_then(|u| u.plan_type.as_deref())
            .filter(|p| !p.trim().is_empty())
            .or(account.plan.as_deref())
            .unwrap_or_default();
        if !plan.trim().eq_ignore_ascii_case("plus") {
            return Err(SAY_HI_REQUIRES_PLUS.into());
        }
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
                codex_config::chatgpt_base_url(&self.codex_config_path()).as_deref(),
            );
            crate::proxy_service::say_hi(&base, auth, &self.account_proxy(&account.proxy_url)?)
                .await
        };
        // An explicit greeting never preflights or skips based on usage.
        match result {
            Ok(()) => Ok(crate::pool::Hello::Said),
            Err(error) if account_network::expired_error(&error) => {
                Ok(crate::pool::Hello::Expired(error))
            }
            Err(error) => Err(error),
        }
    }
    /// Refreshes an account's usage, renewing its token as its renewal
    /// allows; with `renew_now`, only renews it, whatever the choice, and
    /// reports that it did. A refresh not forced skips usage fetched within
    /// the last 25 seconds.
    pub(crate) async fn codex_refresh_account(
        &self,
        id: &str,
        force: bool,
        renew_now: Option<&mut bool>,
    ) -> Result<()> {
        self.codex_refresh_login(id, force, renew_now, None).await
    }
    /// Renews a login the proxy's upstream refused (`rejected`, its access
    /// token), and returns the account's login in the proxy's form; a login
    /// renewed since is returned as it is.
    pub(crate) async fn codex_renew_login(&self, id: &str, rejected: &str) -> Result<Value> {
        let mut renewed = false;
        let result = self
            .codex_refresh_login(id, true, Some(&mut renewed), Some(rejected))
            .await;
        if !renewed {
            result?;
        }
        Ok(self
            .codex_load()?
            .accounts
            .into_iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?
            .document)
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
                    Err(error) if account_network::expired_error(&error) => Asked::Refused,
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
    /// Merges a node's renewals, then the accounts to upload, in the
    /// engine's form: one per account, its newest login kept.
    pub(crate) async fn codex_remote_payload(&self, remote: Option<&Value>) -> Result<Vec<u8>> {
        let renewed: Vec<Value> = remote
            .and_then(|rows| rows["accounts"].as_array())
            .into_iter()
            .flatten()
            .map(|row| row["authJson"].clone())
            .collect();
        let newer = |account: &Stored| {
            renewed
                .iter()
                .find(|auth| codex::newer_login(auth, &account.document))
                .cloned()
        };
        let mut store = self.codex_load()?;
        if store.accounts.iter().any(|a| newer(a).is_some()) {
            store = self
                .codex_logins()
                .change(|store| {
                    for account in &mut store.accounts {
                        if let Some(login) = newer(account) {
                            account.document = login;
                        }
                    }
                    Ok(store.clone())
                })
                .await?;
        }
        let mut merged: Vec<&Stored> = Vec::new();
        for incoming in &store.accounts {
            let key = codex::account_identity(&incoming.document).ok();
            if let Some(old) = merged
                .iter_mut()
                .find(|a| key.is_some() && codex::account_identity(&a.document).ok() == key)
            {
                if incoming.updated_at > old.updated_at {
                    *old = incoming;
                }
            } else {
                merged.push(incoming);
            }
        }
        if !merged
            .iter()
            .any(|a| codex::extract_auth(&a.document).is_ok())
        {
            return Err("No usable accounts for remote deployment".into());
        }
        let accounts = merged
            .into_iter()
            .map(engine_account)
            .collect::<Result<Vec<_>>>()?;
        serde_json::to_vec_pretty(
            &json!({"version": 1, "accounts": accounts, "currentSelection": null}),
        )
        .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        account_network::{AccountNetwork, Response},
        workspaces::Metadata,
    };
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
                Logins::<CodexLogins>::record_usage(
                    account,
                    &Err("workspace_deactivated".into()),
                    1,
                );
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
        assert!(proxy_candidates(&f.app.paths).unwrap().is_empty());
        f.app.delete_account(Pool::Codex, &id).await.unwrap();
        let store = f.app.codex_load().unwrap();
        assert_eq!(store.accounts[0].standing, Some(Standing::Deleted));
        assert_eq!(store.workspace_directory[0]["workspaceId"], "ws-alice");
        assert_eq!(store.workspace_directory[0]["visibility"], "deleted");
        // Deleted once more, it is gone.
        f.app.delete_account(Pool::Codex, &id).await.unwrap();
        assert!(f.app.codex_load().unwrap().accounts.is_empty());
    }
}
