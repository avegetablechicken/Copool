use crate::{
    auth::{self, newer_login},
    models::*,
    persistence::write_json,
    proxy_service,
    state::AppState,
    store,
    utils::now_unix_seconds,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Paths {
    pub data_dir: PathBuf,
    pub codex_auth: PathBuf,
}
impl Paths {
    pub fn discover() -> Result<Self> {
        let data_dir = std::env::var_os("COPOOL_DATA_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| dirs::data_local_dir().map(|p| p.join("CopoolRust")))
            .ok_or("Cannot determine application data directory")?;
        let codex_home = std::env::var_os("CODEX_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|p| p.join(".codex")))
            .ok_or("Cannot determine Codex directory")?;
        Ok(Self {
            data_dir,
            codex_auth: std::env::var_os("COPOOL_CODEX_AUTH_FILE")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| codex_home.join("auth.json")),
        })
    }
    pub(crate) fn store(&self) -> PathBuf {
        self.data_dir.join("accounts.json")
    }
    fn settings(&self) -> PathBuf {
        self.data_dir.join("preferences.json")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    pub auto_switch: bool,
    pub auto_start_proxy: bool,
    pub proxy_port: u16,
    pub locale: String,
    pub usage_display: String,
    pub launch_at_startup: bool,
    pub launch_codex_after_switch: bool,
    pub sync_opencode_openai_auth: bool,
    pub host_api_only: bool,
    pub restart_editors_on_switch: bool,
    pub restart_editor_targets: Vec<String>,
    /// Contact upstream as soon as a new account is imported; otherwise it
    /// waits for a manual refresh, so that its proxy can be set first.
    pub request_new_accounts: bool,
    /// Default proxy for stored accounts without their own proxy; never for login/import.
    #[serde(alias = "importProxyUrl")]
    pub account_proxy_url: String,
    /// The order the account page's cards were dragged into, per pool.
    pub account_order: AccountOrder,
}
/// Card keys in display order: an account's id, or `sub2api:<provider>` for
/// all of a Sub2api provider's cards. Cards not listed follow, ranked.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountOrder {
    pub codex: Vec<String>,
    pub claude: Vec<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            auto_switch: false,
            auto_start_proxy: false,
            proxy_port: 8787,
            locale: String::new(),
            usage_display: "used".into(),
            launch_at_startup: false,
            launch_codex_after_switch: true,
            sync_opencode_openai_auth: false,
            host_api_only: false,
            restart_editors_on_switch: false,
            restart_editor_targets: vec![],
            request_new_accounts: false,
            account_proxy_url: String::new(),
            account_order: AccountOrder::default(),
        }
    }
}

/// What followed a switch, as Swift's SwitchAccountExecutionResult.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchExecution {
    /// The Codex provider switched to, for an unofficial provider's card.
    pub provider: Option<String>,
    pub opencode_synced: bool,
    pub opencode_sync_error: Option<String>,
    pub restarted_editors: Vec<String>,
    pub editor_restart_error: Option<String>,
    pub launch_error: Option<String>,
    /// Claude Code settings that bypass the switched-to account login.
    pub login_overrides: Vec<String>,
}
/// Smart Switch: no target, already on the best account, or switched.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SmartSwitchOutcome {
    NoTarget,
    AlreadyBest,
    Switched {
        label: String,
        execution: SwitchExecution,
    },
}
type SwitchEffect = Arc<
    dyn Fn(
            Preferences,
            Option<Value>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<SwitchExecution>> + Send>>
        + Send
        + Sync,
>;
impl Preferences {
    fn from_settings(settings: crate::settings::Settings) -> Result<Self> {
        let p = Preferences {
            auto_switch: settings.auto_smart_switch,
            auto_start_proxy: settings.auto_start_api_proxy,
            proxy_port: settings.proxy_configuration["preferredPortText"]
                .as_str()
                .unwrap_or("8787")
                .parse()
                .map_err(|_| "Invalid proxy port")?,
            locale: settings.locale,
            usage_display: settings.usage_progress_display_mode,
            launch_at_startup: settings.launch_at_startup,
            launch_codex_after_switch: settings.launch_codex_after_switch,
            sync_opencode_openai_auth: settings.sync_opencode_openai_auth,
            host_api_only: settings.local_proxy_host_api_only,
            restart_editors_on_switch: settings.restart_editors_on_switch,
            restart_editor_targets: settings.restart_editor_targets,
            account_proxy_url: settings
                .extra
                .get("accountProxyUrl")
                .or_else(|| settings.extra.get("importProxyUrl"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            request_new_accounts: settings
                .extra
                .get("requestNewAccounts")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            // An order this build cannot read is only a display preference.
            account_order: settings
                .extra
                .get("accountOrder")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default(),
        };
        validate_preferences(&p)?;
        Ok(p)
    }
}
pub struct Application {
    pub paths: Paths,
    pub(crate) secrets: Arc<dyn crate::credentials::SecretStore>,
    /// Copool's own Keychain item, holding `secrets` and the Claude pool's
    /// tokens; none off macOS and for injected secrets.
    pub(crate) keychain: Option<Arc<crate::vault::Item>>,
    pub(crate) network: Arc<dyn crate::account_network::AccountNetwork>,
    pub(crate) state: AppState,
    pub(crate) operations: Mutex<()>,
    proxy_operations: Mutex<()>,
    // Prevent two app processes from concurrently mutating the same account pool.
    _instance_lock: fs::File,
    switch_effects: Arc<std::sync::RwLock<Option<SwitchEffect>>>,
    refreshing: std::sync::Mutex<std::collections::BTreeMap<String, usize>>,
    pub(crate) changes: Arc<std::sync::RwLock<Option<Arc<dyn Fn() + Send + Sync>>>>,
    pub(crate) claude: crate::claude::State,
    /// The next background round; none before the first starts.
    refresh_schedule: std::sync::Mutex<Option<crate::refresh_policy::Schedule>>,
    claude_refresh_schedule: std::sync::Mutex<Option<crate::refresh_policy::Schedule>>,
    claude_unchanged_rounds: std::sync::Mutex<u8>,
    /// Each pool's wait for its next round, started over by a manual refresh.
    round_deferred: tokio::sync::Notify,
    claude_round_deferred: tokio::sync::Notify,
}
pub(crate) struct RefreshGuard<'a> {
    app: &'a Application,
    id: String,
}
impl Drop for RefreshGuard<'_> {
    fn drop(&mut self) {
        {
            let mut active = self.app.refreshing.lock().unwrap();
            if let Some(count) = active.get_mut(&self.id) {
                *count -= 1;
                if *count == 0 {
                    active.remove(&self.id);
                }
            }
        }
        self.app.publish();
    }
}
impl Application {
    pub fn open(paths: Paths) -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            let item = crate::vault::Item::new(Arc::new(crate::vault::KeychainVault));
            let secrets = crate::vault::ItemSecrets::new(item.clone());
            Self::open_inner(paths, Arc::new(secrets), Some(item))
        }
        #[cfg(not(target_os = "macos"))]
        Self::open_inner(paths, Arc::new(crate::credentials::SystemSecrets), None)
    }
    pub fn open_with_secrets(
        paths: Paths,
        secrets: Arc<dyn crate::credentials::SecretStore>,
    ) -> Result<Self> {
        Self::open_inner(paths, secrets, None)
    }
    fn open_inner(
        paths: Paths,
        secrets: Arc<dyn crate::credentials::SecretStore>,
        keychain: Option<Arc<crate::vault::Item>>,
    ) -> Result<Self> {
        // Upstream accounts without a proxy of their own use the system's.
        crate::utils::set_system_proxy(crate::system_proxy::for_url);
        fs::create_dir_all(&paths.data_dir).map_err(|e| e.to_string())?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(paths.data_dir.join("instance.lock"))
            .map_err(|e| e.to_string())?;
        file.try_lock()
            .map_err(|_| "This Copool data directory is already in use")?;
        // Recover malformed account JSON only after a private backup succeeds.
        store::load_store_from_path(&paths.store())?;
        let network = Arc::new(crate::account_network::Network {
            codex_config: paths.codex_auth.with_file_name("config.toml"),
        });
        let app = Self {
            paths,
            secrets,
            keychain,
            network,
            state: AppState::default(),
            operations: Mutex::new(()),
            proxy_operations: Mutex::new(()),
            _instance_lock: file,
            switch_effects: Arc::new(std::sync::RwLock::new(None)),
            refreshing: Default::default(),
            changes: Arc::new(std::sync::RwLock::new(None)),
            claude: Default::default(),
            claude_refresh_schedule: Default::default(),
            claude_unchanged_rounds: Default::default(),
            refresh_schedule: Default::default(),
            round_deferred: Default::default(),
            claude_round_deferred: Default::default(),
        };
        app.migrate_provider_credentials()?;
        app.preferences()?;
        Ok(app)
    }
    /// Lets an explicit action ask for Copool's Keychain item, which a
    /// refused request stopped background reads from doing; reads it unless
    /// read already.
    pub async fn authorize_keychain(&self) -> Result<()> {
        let Some(item) = self.keychain.clone() else {
            return Ok(());
        };
        tokio::task::spawn_blocking(move || item.authorize())
            .await
            .map_err(|e| e.to_string())?
    }
    pub fn refresh_activity(&self) -> Vec<String> {
        self.refreshing.lock().unwrap().keys().cloned().collect()
    }
    pub(crate) fn begin_refresh(&self, id: &str) -> RefreshGuard<'_> {
        *self
            .refreshing
            .lock()
            .unwrap()
            .entry(id.into())
            .or_default() += 1;
        self.publish();
        RefreshGuard {
            app: self,
            id: id.into(),
        }
    }
    pub fn on_change(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self.changes.write().unwrap() = Some(callback);
    }
    pub(crate) fn publish(&self) {
        if let Some(callback) = self.changes.read().unwrap().as_ref() {
            callback();
        }
    }
    /// Edits the account store under the lock its other writers, the proxy
    /// included, hold across their load and save.
    pub(crate) async fn mutate_accounts(
        &self,
        edit: impl FnOnce(&mut AccountsStore) -> Result<()>,
    ) -> Result<AccountsStore> {
        let _lock = self.state.store_lock.lock().await;
        crate::repository::mutate_accounts(&self.paths.store(), edit)
    }
    pub(crate) fn persist_accounts(&self, accounts: &AccountsStore) -> Result<()> {
        store::save_store_to_path(&self.paths.store(), accounts)?;
        self.publish();
        Ok(())
    }
    pub fn on_switch_effects(&self, effect: SwitchEffect) {
        *self.switch_effects.write().unwrap() = Some(effect);
    }
    pub fn enable_desktop_effects(&self) {
        self.on_switch_effects(Arc::new(|preferences, auth| {
            Box::pin(
                async move { crate::platform::after_switch(&preferences, auth.as_ref()).await },
            )
        }));
    }
    async fn after_switch(&self, auth: Option<&Value>) -> Result<SwitchExecution> {
        let effect = self.switch_effects.read().unwrap().clone();
        if let Some(effect) = effect {
            effect(self.preferences()?, auth.cloned()).await
        } else {
            Ok(SwitchExecution::default())
        }
    }
    pub(crate) fn storage(&self) -> proxy_service::ProxyStorageContext {
        let mut storage = proxy_service::new_proxy_storage_context(
            self.paths.data_dir.clone(),
            self.state.store_lock.clone(),
            false,
        );
        let paths = self.paths.clone();
        storage.account_proxy = Some(Arc::new(move || {
            let settings = crate::repository::load_settings(&paths.data_dir.join("settings.json"))?;
            Ok(Preferences::from_settings(settings)?.account_proxy_url)
        }));
        storage.prefer_local_selection = true;
        storage.active_auth_path = Some(self.paths.codex_auth.clone());
        // Always resolved here: the engine's own fallback reads ~/.codex, not
        // the Codex configuration Copool uses.
        storage.upstream_base_url = Some(crate::usage::chatgpt_base_origin(
            crate::codex_config::chatgpt_base_url(&self.codex_config_path()).as_deref(),
        ));
        let paths = self.paths.clone();
        storage.on_auth_refreshed = Some(Arc::new(move |id, from, auth| {
            crate::proxy_integration::sync_refreshed_auth(&paths, id, from, auth)
        }));
        let paths = self.paths.clone();
        storage.provider_candidates = Some(Arc::new(move || {
            crate::proxy_integration::candidates(&paths)
        }));
        let paths = self.paths.clone();
        let changes = self.changes.clone();
        let effects = self.switch_effects.clone();
        let store_lock = self.state.store_lock.clone();
        storage.on_selected = Some(Arc::new(move |id, auth| {
            let paths = paths.clone();
            let changes = changes.clone();
            let effects = effects.clone();
            let store_lock = store_lock.clone();
            Box::pin(async move {
                let selected = {
                    let _lock = store_lock.lock().await;
                    crate::proxy_integration::select(&paths, &id, &auth)?
                };
                if selected {
                    if let Some(callback) = changes.read().unwrap().as_ref() {
                        callback();
                    }
                    let effect = effects.read().unwrap().clone();
                    if let Some(effect) = effect {
                        let settings = crate::repository::load_settings(
                            &paths.data_dir.join("settings.json"),
                        )?;
                        let preferences = Preferences::from_settings(settings)?;
                        effect(
                            preferences,
                            if auth.get("copoolProvider").is_some() {
                                None
                            } else {
                                Some(auth)
                            },
                        )
                        .await?;
                    }
                }
                Ok(())
            })
        }));
        storage
    }
    pub fn preferences(&self) -> Result<Preferences> {
        let mut settings = self.configuration()?;
        // Import early Rust preferences once. settings.json is authoritative afterwards.
        if settings.extra.get("rustPreferencesMigrated").is_none() {
            let old = self.paths.settings();
            if old.exists() {
                // A preferences.json this build cannot read must not fail every
                // later call: keep a copy and fall back to the settings.
                let legacy = fs::read(&old)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| {
                        let mut value: Value =
                            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                        // The fixed refresh interval is no longer a preference.
                        if let Some(object) = value.as_object_mut() {
                            object.remove("refreshSeconds");
                        }
                        serde_json::from_value(value).map_err(|e| e.to_string())
                    })
                    .and_then(|legacy| validate_preferences(&legacy).map(|()| legacy));
                match legacy {
                    Ok(legacy) => Self::project_preferences(&mut settings, &legacy),
                    Err(error) => {
                        log::warn!("读取旧 preferences.json 失败, 已保留副本: {error}");
                        let backup = old.with_file_name(format!(
                            "preferences.corrupt-{}.json",
                            now_unix_seconds()
                        ));
                        let _ = fs::rename(&old, backup);
                    }
                }
            }
            settings
                .extra
                .insert("rustPreferencesMigrated".into(), true.into());
            self.persist_configuration(&settings)?;
        }
        Preferences::from_settings(settings)
    }

    fn project_preferences(s: &mut crate::settings::Settings, p: &Preferences) {
        s.extra.remove("importProxyUrl");
        s.extra
            .insert("accountProxyUrl".into(), p.account_proxy_url.trim().into());
        s.auto_smart_switch = p.auto_switch;
        s.auto_start_api_proxy = p.auto_start_proxy;
        s.locale = p.locale.clone();
        s.usage_progress_display_mode = p.usage_display.clone();
        s.proxy_configuration["preferredPortText"] = p.proxy_port.to_string().into();
        s.launch_at_startup = p.launch_at_startup;
        s.launch_codex_after_switch = p.launch_codex_after_switch;
        s.sync_opencode_openai_auth = p.sync_opencode_openai_auth;
        s.local_proxy_host_api_only = p.host_api_only;
        s.restart_editors_on_switch = p.restart_editors_on_switch;
        s.restart_editor_targets = p.restart_editor_targets.clone();
        s.extra
            .insert("requestNewAccounts".into(), p.request_new_accounts.into());
        // Pool-specific intervals replace the old shared preference.
        s.extra.remove("refreshSeconds");
        s.extra.insert(
            "accountOrder".into(),
            serde_json::to_value(&p.account_order).unwrap_or_default(),
        );
        s.extra
            .insert("rustPreferencesMigrated".into(), true.into());
    }
    pub async fn save_preferences(&self, value: Preferences) -> Result<()> {
        validate_preferences(&value)?;
        let _op = self.operations.lock().await;
        let mut settings = self.configuration()?;
        Self::project_preferences(&mut settings, &value);
        self.persist_configuration(&settings)
    }
    /// An explicit account proxy wins over the shared account default.
    pub(crate) fn account_proxy(&self, account_proxy: &str) -> Result<String> {
        if !account_proxy.trim().is_empty() {
            return Ok(account_proxy.trim().to_owned());
        }
        Ok(self.preferences()?.account_proxy_url.trim().to_owned())
    }
    fn current_auth(&self) -> Result<Option<Value>> {
        match fs::read(&self.paths.codex_auth) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| format!("Invalid Codex auth: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    pub async fn accounts(&self) -> Result<Vec<AccountSummary>> {
        let _lock = self.state.store_lock.lock().await;
        let mut store = store::load_store_from_path(&self.paths.store())?;
        let before = serde_json::to_value(&store).map_err(|e| e.to_string())?;
        for account in &mut store.accounts {
            if let Ok(extracted) = auth::extract_auth(&account.auth_json) {
                account.account_id = extracted.account_id;
                if extracted.email.is_some() {
                    account.email = extracted.email;
                }
                if extracted.principal_id.is_some() {
                    account.principal_id = extracted.principal_id;
                }
                if extracted.plan_type.is_some() {
                    account.plan_type = extracted.plan_type;
                }
                if let Some(name) = extracted.team_name {
                    account.extra.insert("teamName".into(), name.into());
                }
            }
        }
        // auth.json may be changed outside Copool; the login Codex actually uses wins over
        // the stored selection. A missing or unreadable login keeps the stored selection.
        let live = self.current_auth().ok().flatten();
        let current = live.as_ref().and_then(|v| identity(v).ok());
        if let Some(current) = &current {
            let matches = |a: &StoredAccount| identity(&a.auth_json).ok().as_ref() == Some(current);
            let stored_id = store.extra.get("currentAccountId").and_then(Value::as_str);
            if !store
                .accounts
                .iter()
                .any(|a| Some(a.id.as_str()) == stored_id && matches(a))
            {
                match store
                    .accounts
                    .iter()
                    .find(|a| matches(a))
                    .map(|a| a.id.clone())
                {
                    Some(id) => {
                        store.extra.insert("currentAccountId".into(), id.into());
                    }
                    None => {
                        store.extra.remove("currentAccountId");
                    }
                }
            }
        }
        if let Some(current_id) = store.extra.get("currentAccountId").and_then(Value::as_str) {
            if let Some(account) = store.accounts.iter().find(|a| a.id == current_id) {
                if let (Some(selection), Ok((workspace, principal))) =
                    (&mut store.current_selection, identity(&account.auth_json))
                {
                    selection.account_id = current_id.to_owned();
                    selection.account_key = Some(format!("{principal}|{workspace}"));
                }
            }
        }
        if serde_json::to_value(&store).map_err(|e| e.to_string())? != before {
            self.persist_accounts(&store)?;
        }
        let selected = store
            .extra
            .get("currentAccountId")
            .and_then(Value::as_str)
            .filter(|id| store.accounts.iter().any(|a| a.id == *id))
            .map(str::to_owned);
        let mut summaries: Vec<_> = store
            .accounts
            .iter()
            .map(|a| {
                let mut summary = a.to_summary(None);
                summary.is_current = selected.as_ref().is_some_and(|id| *id == a.id);
                summary.renewal = Some(a.renewal().into());
                // Codex renews the login it uses in auth.json; until a refresh
                // adopts it, that token's expiry is the one in effect.
                if let Some(live) = live.as_ref().filter(|l| newer_login(l, &a.auth_json)) {
                    summary.token_expires_at =
                        auth::access_token_expiry(live).and_then(crate::models::displayed_expiry);
                }
                summary
            })
            .collect();
        let provider = crate::codex_config::resolve_path(&self.codex_config_path());
        if !provider.is_official() {
            for a in &mut summaries {
                a.is_current = false;
            }
        }
        for p in self.configuration()?.sub2_api_provider.providers {
            for raw in p.cached_accounts {
                if let Ok(mut account) = serde_json::from_value::<crate::sub2api::Account>(raw) {
                    account.provider_id = Some(p.provider_id.clone());
                    let mut summary = account.summary();
                    summary.proxy_url = p.proxy_url.clone();
                    summary.is_current =
                        !provider.is_official() && provider.id.eq_ignore_ascii_case(&p.provider_id);
                    summaries.push(summary);
                }
            }
        }
        crate::ranking::sort_for_display(&mut summaries);
        if let Some(schedule) = self.refresh_schedule() {
            let every_round = crate::refresh_policy::round_card(&summaries).map(str::to_owned);
            for card in &mut summaries {
                // Sub2api cards refresh with their provider: every round while
                // it is in use, else on full rounds.
                card.next_refresh_at = if card.source_tag.as_deref() == Some("SUB2API") {
                    Some(schedule.next_refresh_at(card.is_current, 0))
                } else if card.awaiting_first_request {
                    None
                } else {
                    let every = every_round.as_deref() == Some(card.id.as_str());
                    Some(schedule.next_refresh_at(every, 0))
                };
            }
        }
        Ok(summaries)
    }
    pub async fn import_current(&self) -> Result<usize> {
        self.import_current_as(None).await
    }
    pub async fn import_current_as(&self, label: Option<&str>) -> Result<usize> {
        let value = self
            .current_auth()?
            .ok_or("Codex auth.json not found; log in with Codex first")?;
        let count = self
            .import_json(&serde_json::to_string(&value).map_err(|e| e.to_string())?)
            .await?;
        if let Some(label) = label {
            let identity = identity(&value)?;
            let _op = self.operations.lock().await;
            let _lock = self.state.store_lock.lock().await;
            let mut saved = store::load_store_from_path(&self.paths.store())?;
            if let Some(account) = saved.accounts.iter_mut().find(|a| {
                crate::auth::account_identity(&a.auth_json).ok().as_ref() == Some(&identity)
            }) {
                account.label = label.to_owned();
            }
            self.persist_accounts(&saved)?;
        }
        self.reconcile_workspace_names(true).await?;
        Ok(count)
    }
    /// Accept auth.json, arrays of auth documents, or a Swift/Rust accounts.json.
    /// Validate the complete batch before changing the destination; keep metadata.
    pub async fn import_json(&self, content: &str) -> Result<usize> {
        if content.len() > 16 * 1024 * 1024 {
            return Err("Import exceeds 16 MiB".into());
        }
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
        let now = now_unix_seconds();
        let mut incoming = Vec::new();
        for entry in entries {
            let document =
                auth::normalize_imported_auth_json(entry.get("authJson").unwrap_or(&entry).clone());
            let extracted = auth::extract_auth(&document)?;
            if extracted.access_token.trim().is_empty() || extracted.account_id.trim().is_empty() {
                return Err("Empty access token or account ID".into());
            }
            let mut account = if entry.get("authJson").is_some() {
                serde_json::from_value::<StoredAccount>(entry)
                    .map_err(|e| format!("Invalid stored account: {e}"))?
            } else {
                StoredAccount {
                    extra: Default::default(),
                    id: uuid::Uuid::new_v4().to_string(),
                    label: extracted
                        .email
                        .clone()
                        .unwrap_or_else(|| extracted.account_id.clone()),
                    email: extracted.email.clone(),
                    account_id: extracted.account_id.clone(),
                    plan_type: extracted.plan_type,
                    team_alias: None,
                    auth_json: document.clone(),
                    added_at: now,
                    updated_at: now,
                    usage: None,
                    usage_error: None,
                    principal_id: None,
                    proxy_url: String::new(),
                }
            };
            // Trust the auth document, not externally supplied account IDs.
            account.principal_id = extracted.principal_id;
            if let Some(name) = extracted.team_name {
                account.extra.insert("teamName".into(), name.into());
            }
            account.account_id = extracted.account_id;
            account.auth_json = document;
            account.updated_at = now;
            validate_proxy(&account.proxy_url)?;
            incoming.push(account);
        }
        let defer = !self.preferences()?.request_new_accounts;
        let _op = self.operations.lock().await;
        let _lock = self.state.store_lock.lock().await;
        let mut saved = store::load_store_from_path(&self.paths.store())?;
        let count = incoming.len();
        for mut account in incoming {
            let key = identity(&account.auth_json)?;
            if let Some(existing) = saved
                .accounts
                .iter_mut()
                .find(|a| identity(&a.auth_json).ok().as_ref() == Some(&key))
            {
                existing.auth_json = account.auth_json;
                existing.updated_at = now;
                if account.usage.is_some() || account.usage_error.is_some() {
                    existing.usage = account.usage;
                    existing.usage_error = account.usage_error;
                } else {
                    existing.usage_error = None;
                }
                for key in ["teamName", "usageStateUpdatedAt"] {
                    if let Some(value) = account.extra.get(key).filter(|v| !v.is_null()) {
                        existing.extra.insert(key.into(), value.clone());
                    }
                }
                existing
                    .extra
                    .insert("workspaceStatus".into(), "active".into());
                existing.extra.insert("displayStatus".into(), "list".into());
            } else {
                // External IDs may collide with another principal's local ID.
                account.id = uuid::Uuid::new_v4().to_string();
                if defer {
                    account
                        .extra
                        .insert(crate::models::FIRST_REQUEST_PENDING.into(), true.into());
                }
                saved.accounts.push(account);
            }
        }
        let authorized: Vec<_> = saved
            .accounts
            .iter()
            .filter(|a| a.is_available())
            .map(|a| a.account_id.trim().to_lowercase())
            .collect();
        if let Some(rows) = saved
            .extra
            .get_mut("workspaceDirectory")
            .and_then(Value::as_array_mut)
        {
            rows.retain(|v| {
                !authorized.contains(
                    &v["workspaceId"]
                        .as_str()
                        .unwrap_or_default()
                        .trim()
                        .to_lowercase(),
                )
            });
        }
        self.persist_accounts(&saved)?;
        Ok(count)
    }
    pub async fn update_account(&self, id: &str, label: &str, proxy_url: &str) -> Result<()> {
        if label.trim().is_empty() {
            return Err("Account label is required".into());
        }
        validate_proxy(proxy_url)?;
        let _op = self.operations.lock().await;
        let _lock = self.state.store_lock.lock().await;
        let mut saved = store::load_store_from_path(&self.paths.store())?;
        let account = saved
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or("Account not found")?;
        account.label = label.trim().to_string();
        account.proxy_url = proxy_url.trim().to_string();
        self.persist_accounts(&saved)
    }
    /// Sets whether Copool renews an account's tokens: "never", "once" (then
    /// never) or "always".
    pub async fn set_renewal(&self, id: &str, renewal: &str) -> Result<()> {
        if !crate::models::RENEWALS.contains(&renewal) {
            return Err(format!("Unknown token renewal: {renewal}"));
        }
        let _lock = self.state.store_lock.lock().await;
        let mut saved = store::load_store_from_path(&self.paths.store())?;
        let account = saved
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or("Account not found")?;
        account.extra.insert("renewal".into(), renewal.into());
        self.persist_accounts(&saved)
    }
    pub async fn delete_account(&self, id: &str) -> Result<()> {
        if let Some((provider, account)) = self.cached_provider_account(id)? {
            let _op = self.operations.lock().await;
            let mut settings = self.configuration()?;
            let p = settings
                .sub2_api_provider
                .providers
                .iter_mut()
                .find(|p| p.id == provider.id)
                .ok_or("Provider not found")?;
            p.imported_account_ids.retain(|v| *v != account.id);
            p.cached_accounts
                .retain(|v| v["id"].as_i64() != Some(account.id));
            return self.persist_configuration(&settings);
        }
        let _op = self.operations.lock().await;
        let _lock = self.state.store_lock.lock().await;
        let mut saved = store::load_store_from_path(&self.paths.store())?;
        if !saved.accounts.iter().any(|a| a.id == id) {
            return Err("Account not found".into());
        }
        if let Some(account) = saved.accounts.iter_mut().find(|a| {
            a.id == id
                && a.extra.get("displayStatus").and_then(Value::as_str) == Some("deactivated")
        }) {
            account
                .extra
                .insert("displayStatus".into(), "deleted".into());
            let workspace = account.account_id.clone();
            let entry = serde_json::json!({"workspaceId":workspace,"workspaceName":account.extra.get("teamName"),"email":account.email,"planType":account.plan_type,"kind":"workspace","source":"deactivated","status":"deactivated","visibility":"deleted","lastSeenAt":now_unix_seconds(),"lastStatusCheckedAt":now_unix_seconds()});
            let rows = saved
                .extra
                .entry("workspaceDirectory")
                .or_insert(serde_json::json!([]))
                .as_array_mut()
                .ok_or("Invalid workspace directory")?;
            if let Some(old) = rows.iter_mut().find(|v| v["workspaceId"] == workspace) {
                old["visibility"] = "deleted".into();
            } else {
                rows.push(entry);
            }
            return self.persist_accounts(&saved);
        }
        saved.accounts.retain(|a| a.id != id);
        self.persist_accounts(&saved)
    }
    pub async fn switch_account(&self, id: &str) -> Result<SwitchExecution> {
        if let Some((provider, account)) = self.cached_provider_account(id)? {
            if !account.status.eq_ignore_ascii_case("active") {
                return Err("Account is deactivated".into());
            }
            let _op = self.operations.lock().await;
            crate::codex_config::switch(&self.codex_config_path(), &provider.provider_id)?;
            self.publish();
            drop(_op);
            let mut execution = self.after_switch(None).await?;
            execution.provider = Some(provider.provider_id);
            return Ok(execution);
        }
        let _op = self.operations.lock().await;
        let _lock = self.state.store_lock.lock().await;
        let mut saved = store::load_store_from_path(&self.paths.store())?;
        let account = saved
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?
            .clone();
        if !account.is_available() {
            return Err("Account is deactivated or hidden".into());
        }
        let normalized = auth::normalized_auth_for_write(account.auth_json.clone())?;
        let previous = self.current_auth()?;
        if let Some(previous) = &previous {
            write_json(
                &self.paths.data_dir.join("auth-before-switch.json"),
                previous,
            )?;
        }
        if !crate::codex_config::resolve_path(&self.codex_config_path()).is_official() {
            crate::codex_config::switch(&self.codex_config_path(), "openai")?;
        }
        write_json(&self.paths.codex_auth, &normalized)?;
        saved.extra.insert("currentAccountId".into(), id.into());
        let (workspace, principal) = identity(&normalized)?;
        saved.current_selection = Some(CurrentAccountSelection {
            account_id: id.into(),
            selected_at: now_unix_seconds(),
            source_device_id: "rust-local".into(),
            account_key: Some(format!("{principal}|{workspace}")),
        });
        if let Err(error) = self.persist_accounts(&saved) {
            let rollback = if let Some(previous) = previous {
                write_json(&self.paths.codex_auth, &previous)
            } else {
                fs::remove_file(&self.paths.codex_auth).map_err(|e| e.to_string())
            };
            rollback.map_err(|rollback| format!("{error}; auth rollback failed: {rollback}"))?;
            return Err(error);
        }
        drop(_lock);
        drop(_op);
        self.after_switch(Some(&normalized)).await
    }
    /// As Swift's AccountsPageModel.smartSwitch: the account with the most
    /// remaining quota, and no switch when that is already the current one.
    pub async fn smart_switch(&self) -> Result<SmartSwitchOutcome> {
        let accounts = self.accounts().await?;
        // Swift's max(by:) keeps the first of equal scores, so a tie with the
        // current account (listed first) is no reason to switch.
        let best =
            accounts
                .iter()
                .filter(|a| a.available)
                .fold(None::<&AccountSummary>, |best, a| match best {
                    Some(b) if score(a) <= score(b) => Some(b),
                    _ => Some(a),
                });
        let Some(best) = best else {
            return Ok(SmartSwitchOutcome::NoTarget);
        };
        if best.is_current {
            return Ok(SmartSwitchOutcome::AlreadyBest);
        }
        let execution = self.switch_account(&best.id).await?;
        Ok(SmartSwitchOutcome::Switched {
            label: best.label.clone(),
            execution,
        })
    }
    pub async fn refresh_usage(&self, id: &str) -> Result<()> {
        self.refresh_usage_if_needed(id, true).await
    }
    pub async fn refresh_usage_if_needed(&self, id: &str, force: bool) -> Result<()> {
        let _activity = self.begin_refresh(id);
        if let Some((provider, account)) = self.cached_provider_account(id)? {
            return self.refresh_provider_card(&provider, account.id).await;
        }
        // Read a snapshot, perform IO without locks, then merge into the latest store.
        // Proxy edits, manual switches and account removal must remain responsive.
        let account = {
            let _lock = self.state.store_lock.lock().await;
            store::load_store_from_path(&self.paths.store())?
                .accounts
                .into_iter()
                .find(|a| a.id == id)
                .ok_or("Account not found")?
        };
        if account.awaiting_first_request() {
            return Ok(());
        }
        if !force
            && account
                .usage
                .as_ref()
                .is_some_and(|u| now_unix_seconds() - u.fetched_at < 25)
        {
            return Ok(());
        }
        // Codex rotates the refresh token of the login it uses in auth.json;
        // the stored copy of that account is spent once the live one is newer.
        let live = self
            .current_auth()
            .ok()
            .flatten()
            .filter(|live| newer_login(live, &account.auth_json));
        let auth_json = live.as_ref().unwrap_or(&account.auth_json);
        let result = crate::account_network::fetch(
            self.network.as_ref(),
            auth_json,
            &self.account_proxy(&account.proxy_url)?,
            now_unix_seconds(),
            account.renews(),
        )
        .await;
        let renewed = match &result {
            Ok((auth, _)) | Err(crate::account_network::FetchFailure { auth, .. }) => {
                auth != auth_json
            }
        };
        let _lock = self.state.store_lock.lock().await;
        let mut saved = store::load_store_from_path(&self.paths.store())?;
        let current = saved.extra.get("currentAccountId").and_then(Value::as_str) == Some(id);
        let mut live_write = None;
        if let Some(target) = saved.accounts.iter_mut().find(|a| a.id == id) {
            target.updated_at = now_unix_seconds();
            target
                .extra
                .insert("usageStateUpdatedAt".into(), target.updated_at.into());
            if renewed {
                target.renewed();
            }
            match &result {
                Ok((auth, value)) => {
                    target.usage = Some(value.clone());
                    target.usage_error = None;
                    // Tokens the proxy or another refresh rotated during the request
                    // are newer than the snapshot; only a newer refresh or adoption
                    // made here replaces them.
                    if auth != &account.auth_json
                        && (target.auth_json == account.auth_json
                            || newer_login(auth, &target.auth_json))
                    {
                        target.auth_json = auth.clone();
                    }
                    let extracted = crate::auth::extract_auth(auth)?;
                    if let Some(name) = extracted.team_name {
                        target.extra.insert("teamName".into(), name.into());
                    }
                    target.plan_type = extracted.plan_type.or(target.plan_type.clone());
                    target.email = extracted.email.or(target.email.clone());
                    target.principal_id = extracted.principal_id;
                    target
                        .extra
                        .insert("workspaceStatus".into(), "active".into());
                    if target.extra.get("displayStatus").and_then(Value::as_str) != Some("deleted")
                    {
                        target.extra.insert("displayStatus".into(), "list".into());
                    }
                    // Only refresh the login Codex is still using; never revert a switch
                    // made outside Copool to the stored selection.
                    if current
                        && auth != auth_json
                        && self
                            .current_auth()
                            .ok()
                            .flatten()
                            .as_ref()
                            .and_then(|v| identity(v).ok())
                            == identity(auth).ok()
                    {
                        live_write = Some(auth);
                    }
                }
                Err(crate::account_network::FetchFailure { auth, message }) => {
                    // A refresh that succeeded before the request failed has already
                    // spent the stored copy, so keep the rotated login even though the
                    // usage call did not answer.
                    if auth != &account.auth_json
                        && (target.auth_json == account.auth_json
                            || newer_login(auth, &target.auth_json))
                    {
                        target.auth_json = auth.clone();
                    }
                    if let Some(live) = live
                        .as_ref()
                        .filter(|live| newer_login(live, &target.auth_json))
                    {
                        target.auth_json = live.clone();
                    }
                    if current
                        && auth != auth_json
                        && self
                            .current_auth()
                            .ok()
                            .flatten()
                            .as_ref()
                            .and_then(|v| identity(v).ok())
                            == identity(auth).ok()
                    {
                        live_write = Some(auth);
                    }
                    target.usage_error = Some(if crate::account_network::expired_error(message) {
                        "error.accounts.sign_in_expired".into()
                    } else {
                        message.clone()
                    });
                    if crate::account_network::deactivated(message) {
                        target
                            .extra
                            .insert("workspaceStatus".into(), "deactivated".into());
                        if target.extra.get("displayStatus").and_then(Value::as_str)
                            != Some("deleted")
                        {
                            target
                                .extra
                                .insert("displayStatus".into(), "deactivated".into());
                        }
                    }
                }
            }
            self.persist_accounts(&saved)?;
        }
        // Written only once the store keeps the rotated tokens, and with the
        // store's check of their generation: only over the login they were
        // renewed from, or an older one. A request answered late must not
        // replace a login renewed since.
        let kept = |auth: &Value| {
            saved
                .accounts
                .iter()
                .any(|a| a.id == id && a.auth_json == *auth)
        };
        if let Some(auth) = live_write.filter(|auth| kept(auth)) {
            let renewable = self.current_auth().ok().flatten().is_some_and(|disk| {
                identity(&disk).ok() == identity(auth).ok()
                    && auth::renewal_replaces(auth, auth_json, &disk)
            });
            if renewable {
                write_json(
                    &self.paths.codex_auth,
                    &crate::auth::normalized_auth_for_write(auth.clone())?,
                )?;
            }
        }
        result.map(|_| ()).map_err(|failure| failure.message)
    }
    /// A manual refresh lets new accounts contact upstream: one account, or
    /// all of them without an id.
    pub async fn release_new_accounts(&self, id: Option<&str>) -> Result<()> {
        let _lock = self.state.store_lock.lock().await;
        let mut saved = store::load_store_from_path(&self.paths.store())?;
        let mut changed = false;
        for account in saved
            .accounts
            .iter_mut()
            .filter(|a| id.is_none_or(|id| a.id == id))
        {
            changed |= account
                .extra
                .remove(crate::models::FIRST_REQUEST_PENDING)
                .is_some();
        }
        if changed {
            self.persist_accounts(&saved)?;
        }
        Ok(())
    }
    pub async fn refresh_accounts(&self, ids: Option<&[String]>, force: bool) -> Result<()> {
        let accounts = self.accounts().await?;
        let mut errors = vec![];
        for a in accounts
            .into_iter()
            .filter(|a| ids.map_or(true, |ids| ids.contains(&a.id)))
        {
            if let Err(error) = self.refresh_usage_if_needed(&a.id, force).await {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join(" | "))
        }
    }
    pub async fn refresh_all(&self) -> Result<()> {
        // Accounts refresh concurrently: one slow or unreachable endpoint must
        // not hold back the rest of the pool.
        let accounts = self.accounts().await?;
        let refreshes = accounts
            .iter()
            .filter(|a| a.source_tag.as_deref() != Some("SUB2API"))
            .map(|account| async move {
                self.refresh_usage(&account.id)
                    .await
                    .map_err(|error| format!("{}: {error}", account.label))
            });
        let providers = self.configuration()?.sub2_api_provider.providers;
        let provider_refreshes = providers.iter().map(|p| async move {
            self.refresh_provider(&p.provider_id, false)
                .await
                .map(|_| ())
                .map_err(|error| format!("{}: {error}", p.provider_id))
        });
        let (accounts, providers) = tokio::join!(
            futures_util::future::join_all(refreshes),
            futures_util::future::join_all(provider_refreshes)
        );
        let errors: Vec<String> = accounts
            .into_iter()
            .chain(providers)
            .filter_map(Result::err)
            .collect();
        self.auto_switch_if_needed().await?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }
    pub async fn auto_switch_if_needed(&self) -> Result<Option<String>> {
        if !self.preferences()?.auto_switch {
            return Ok(None);
        }
        let accounts = self.accounts().await?;
        // A Sub2api provider's accounts, on its one card, are current together;
        // any fresh one will do, as in the Claude pool.
        if !accounts.iter().any(|a| {
            a.is_current
                && a.usage_error.is_none()
                && a.usage
                    .as_ref()
                    .is_some_and(|u| (0..=900).contains(&(now_unix_seconds() - u.fetched_at)))
        }) {
            return Ok(None);
        }
        let candidates: Vec<_> = accounts
            .into_iter()
            .filter(|a| a.is_current || eligible(a))
            .collect();
        if let Some(target) = crate::ranking::auto_switch_target(&candidates) {
            self.switch_account(&target.id).await?;
            return Ok(Some(target.id.clone()));
        }
        Ok(None)
    }
    pub async fn proxy_status(&self) -> Result<ApiProxyStatus> {
        proxy_service::get_api_proxy_status_with_runtime(&self.storage(), &self.state.api_proxy)
            .await
    }
    pub async fn start_proxy(&self) -> Result<ApiProxyStatus> {
        let _op = self.proxy_operations.lock().await;
        proxy_service::start_api_proxy_with_runtime(
            &self.storage(),
            &self.state.api_proxy,
            Some(self.preferences()?.proxy_port),
            "127.0.0.1",
        )
        .await
    }
    pub async fn stop_proxy(&self) -> Result<ApiProxyStatus> {
        let _op = self.proxy_operations.lock().await;
        proxy_service::stop_api_proxy_with_runtime(&self.storage(), &self.state.api_proxy).await
    }
    pub async fn rotate_proxy_key(&self) -> Result<ApiProxyStatus> {
        let _op = self.proxy_operations.lock().await;
        proxy_service::refresh_api_proxy_key_with_runtime(&self.storage(), &self.state.api_proxy)
            .await
    }
    pub async fn refresh_recurring(&self, tick: u64) -> Result<()> {
        let accounts = self.accounts().await?;
        let ids = crate::refresh_policy::recurring_ids(tick, &accounts);
        let refreshes = accounts
            .iter()
            .filter(|a| {
                a.source_tag.as_deref() != Some("SUB2API")
                    && ids.as_ref().is_none_or(|ids| ids.contains(&a.id))
            })
            .map(|account| self.refresh_usage(&account.id));
        // Sub2api providers: the one in use every round, all of them on a
        // round that refreshes the whole pool.
        let current = crate::codex_config::resolve_path(&self.codex_config_path());
        let providers: Vec<String> = self
            .configuration()?
            .sub2_api_provider
            .providers
            .into_iter()
            .map(|p| p.provider_id)
            .filter(|id| ids.is_none() || id.eq_ignore_ascii_case(&current.id))
            .collect();
        let provider_refreshes = providers.iter().map(|id| async move {
            self.refresh_provider(id, false)
                .await
                .map(|_| ())
                .map_err(|error| format!("{id}: {error}"))
        });
        let (accounts, providers) = tokio::join!(
            futures_util::future::join_all(refreshes),
            futures_util::future::join_all(provider_refreshes)
        );
        let errors: Vec<String> = accounts
            .into_iter()
            .chain(providers)
            .filter_map(Result::err)
            .collect();
        self.auto_switch_if_needed().await?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join(" | "))
        }
    }
    pub async fn background(self: Arc<Self>) {
        if self.preferences().is_ok_and(|p| p.auto_start_proxy) {
            let _ = self.start_proxy().await;
        }
        // Independent loops keep a slow provider from delaying the other pool.
        tokio::join!(self.background_pool(false), self.background_pool(true));
    }
    async fn background_pool(&self, claude: bool) {
        let mut tick = 0u64;
        loop {
            let mut wait = self.background_round(tick, claude).await;
            let deferred = if claude {
                &self.claude_round_deferred
            } else {
                &self.round_deferred
            };
            // Refreshing the whole pool starts the wait over.
            while tokio::select! {
                _ = tokio::time::sleep(wait) => false,
                _ = deferred.notified() => true,
            } {
                wait = self.round_interval(claude);
            }
            tick = tick.wrapping_add(1);
        }
    }
    fn round_interval(&self, claude: bool) -> std::time::Duration {
        if claude {
            self.claude_refresh_interval()
        } else {
            crate::refresh_policy::CODEX_REFRESH_INTERVAL
        }
    }
    /// Refreshing the whole pool stands in for a background round: its next
    /// round waits a whole interval from now. A round under way is left to
    /// schedule the next itself.
    pub fn defer_background_round(&self, claude: bool) {
        let (schedule, deferred) = if claude {
            (&self.claude_refresh_schedule, &self.claude_round_deferred)
        } else {
            (&self.refresh_schedule, &self.round_deferred)
        };
        let now = now_unix_seconds();
        if let Some(next) = schedule.lock().unwrap().as_mut().filter(|s| s.at > now) {
            next.at = now + next.interval;
            // Kept until the loop waits, should it not wait yet.
            deferred.notify_one();
        }
    }
    pub(crate) async fn background_round(&self, tick: u64, claude: bool) -> std::time::Duration {
        let interval = self.round_interval(claude);
        let schedule = if claude {
            &self.claude_refresh_schedule
        } else {
            &self.refresh_schedule
        };
        let set_schedule = |at, tick, interval: std::time::Duration| {
            *schedule.lock().unwrap() = Some(crate::refresh_policy::Schedule {
                at,
                tick,
                interval: interval.as_secs() as i64,
            });
        };
        set_schedule(now_unix_seconds(), tick, interval);
        let before = if claude {
            crate::refresh_policy::usage_values(&self.claude_usage_cards().unwrap_or_default())
        } else {
            Default::default()
        };
        if claude {
            if tick == 0 {
                let _ = self.claude_authorize_at_startup().await;
            }
            let _ = self.claude_refresh_recurring(tick).await;
        } else {
            let _ = self.refresh_recurring(tick).await;
        }
        let interval = if claude {
            let after =
                crate::refresh_policy::usage_values(&self.claude_usage_cards().unwrap_or_default());
            crate::refresh_policy::refresh_interval_for(
                interval,
                crate::refresh_policy::usage_changed(&before, &after),
                &mut self.claude_unchanged_rounds.lock().unwrap(),
            )
        } else {
            interval
        };
        set_schedule(
            now_unix_seconds() + interval.as_secs() as i64,
            tick.wrapping_add(1),
            interval,
        );
        self.publish();
        interval
    }
    pub(crate) fn claude_refresh_interval(&self) -> std::time::Duration {
        self.claude_refresh_schedule()
            .map(|s| std::time::Duration::from_secs(s.interval as u64))
            .unwrap_or(crate::refresh_policy::CLAUDE_REFRESH_INTERVAL)
    }
    pub(crate) fn claude_refresh_schedule(&self) -> Option<crate::refresh_policy::Schedule> {
        *self.claude_refresh_schedule.lock().unwrap()
    }
    pub fn claude_next_full_refresh_at(&self) -> Option<i64> {
        self.claude_refresh_schedule()
            .map(|s| s.next_refresh_at(true, 0))
    }
    #[cfg(test)]
    pub(crate) fn set_claude_refresh_schedule(&self, schedule: crate::refresh_policy::Schedule) {
        *self.claude_refresh_schedule.lock().unwrap() = Some(schedule);
    }
    pub(crate) fn refresh_schedule(&self) -> Option<crate::refresh_policy::Schedule> {
        *self.refresh_schedule.lock().unwrap()
    }
    /// When the next background round that refreshes the whole pool starts
    /// (Unix seconds), should the interval stay as it is; none before the
    /// first round.
    pub fn next_full_refresh_at(&self) -> Option<i64> {
        self.refresh_schedule()
            .map(|schedule| schedule.next_refresh_at(false, 0))
    }
    #[cfg(test)]
    pub(crate) fn set_refresh_schedule(&self, schedule: crate::refresh_policy::Schedule) {
        *self.refresh_schedule.lock().unwrap() = Some(schedule);
    }
}
fn validate_preferences(p: &Preferences) -> Result<()> {
    validate_proxy(&p.account_proxy_url).map_err(|_| "Invalid account proxy URL".to_string())?;
    if !["used", "remaining"].contains(&p.usage_display.as_str()) {
        return Err("Invalid usage display mode".into());
    }
    if ![
        "", "en", "zh-Hans", "zh-Hant", "ja", "ko", "fr", "de", "it", "es", "ru", "nl",
    ]
    .contains(&p.locale.as_str())
    {
        return Err("Unsupported locale".into());
    }
    if p.proxy_port == 0 {
        return Err("Proxy port must be 1–65535".into());
    }
    Ok(())
}
fn validate_proxy(value: &str) -> Result<()> {
    crate::utils::validate_account_proxy(value).map(|_| ())
}

fn identity(document: &Value) -> Result<(String, String)> {
    auth::account_identity(document)
}

fn score(a: &AccountSummary) -> f64 {
    crate::ranking::remaining_score(a)
}
fn exhausted(a: &AccountSummary) -> bool {
    crate::ranking::exhausted(a)
}
pub(crate) fn eligible(a: &AccountSummary) -> bool {
    a.available
        && a.usage_error.is_none()
        && !exhausted(a)
        && score(a) > 0.0
        && a.usage
            .as_ref()
            .is_some_and(|u| (0..=900).contains(&(now_unix_seconds() - u.fetched_at)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use serde_json::json;
    fn document(principal: &str) -> Value {
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(
                &json!({"sub": principal, "email": format!("{principal}@example.com")}),
            )
            .unwrap(),
        );
        json!({"tokens": {"account_id": "shared-team", "access_token": "test-only", "id_token": format!("x.{claims}.x")}})
    }
    /// An isolated pool that contacts upstream right after an import.
    fn app(dir: &tempfile::TempDir) -> Application {
        let app = Application::open(Paths {
            data_dir: dir.path().join("data"),
            codex_auth: dir.path().join("codex/auth.json"),
        })
        .unwrap();
        let mut settings = app.configuration().unwrap();
        settings
            .extra
            .insert("requestNewAccounts".into(), true.into());
        app.persist_configuration(&settings).unwrap();
        app
    }
    #[tokio::test]
    async fn account_proxy_migrates_the_previous_setting_name() {
        let (_dir, app, _) = crate::legacy_tests::network_fixture::fixture();
        let mut settings = app.configuration().unwrap();
        settings
            .extra
            .insert("importProxyUrl".into(), "http://127.0.0.1:7890".into());
        app.persist_configuration(&settings).unwrap();
        let mut preferences = app.preferences().unwrap();
        assert_eq!(preferences.account_proxy_url, "http://127.0.0.1:7890");
        preferences.account_proxy_url.clear();
        app.save_preferences(preferences).await.unwrap();
        assert!(app.preferences().unwrap().account_proxy_url.is_empty());
        assert!(!app
            .configuration()
            .unwrap()
            .extra
            .contains_key("importProxyUrl"));
    }

    #[tokio::test]
    async fn account_proxy_is_default_for_existing_accounts_but_not_imports() {
        let (_dir, app, network) = crate::legacy_tests::network_fixture::fixture();
        app.import_authorized(document("alice"), None, &[])
            .await
            .unwrap();
        let account = app.accounts().await.unwrap().remove(0);
        app.update_account(&account.id, &account.label, "socks5://127.0.0.1:7891")
            .await
            .unwrap();
        let mut preferences = app.preferences().unwrap();
        assert!(preferences.account_proxy_url.is_empty());
        preferences.account_proxy_url = "  http://127.0.0.1:7890  ".into();
        app.save_preferences(preferences.clone()).await.unwrap();
        assert_eq!(
            app.preferences().unwrap().account_proxy_url,
            "http://127.0.0.1:7890"
        );
        preferences.account_proxy_url = "ftp://127.0.0.1:7890".into();
        assert!(app.save_preferences(preferences).await.is_err());
        network.calls.lock().unwrap().clear();
        app.refresh_usage(&account.id).await.unwrap();
        assert_eq!(
            network.calls.lock().unwrap()[0].2,
            "socks5://127.0.0.1:7891"
        );
        network.calls.lock().unwrap().clear();
        app.import_authorized(document("alice"), None, &[])
            .await
            .unwrap();
        assert_eq!(
            network.calls.lock().unwrap()[0].2,
            "socks5://127.0.0.1:7891"
        );
        network.calls.lock().unwrap().clear();
        app.import_authorized(document("bob"), None, &[])
            .await
            .unwrap();
        assert_eq!(network.calls.lock().unwrap()[0].2, "");
        assert_eq!(
            app.accounts()
                .await
                .unwrap()
                .iter()
                .find(|a| a.id == account.id)
                .unwrap()
                .proxy_url,
            "socks5://127.0.0.1:7891"
        );
        let bob = app
            .accounts()
            .await
            .unwrap()
            .into_iter()
            .find(|a| a.id != account.id)
            .unwrap();
        network.calls.lock().unwrap().clear();
        app.refresh_usage(&bob.id).await.unwrap();
        assert_eq!(network.calls.lock().unwrap()[0].2, "http://127.0.0.1:7890");
        let storage = app.storage();
        assert_eq!(
            storage.account_proxy.as_ref().unwrap()().unwrap(),
            "http://127.0.0.1:7890"
        );
        let mut preferences = app.preferences().unwrap();
        preferences.account_proxy_url.clear();
        app.save_preferences(preferences).await.unwrap();
        assert_eq!(storage.account_proxy.as_ref().unwrap()().unwrap(), "");
        network.calls.lock().unwrap().clear();
        app.refresh_usage(&bob.id).await.unwrap();
        assert_eq!(network.calls.lock().unwrap()[0].2, "");
        network.calls.lock().unwrap().clear();
        app.refresh_usage(&account.id).await.unwrap();
        assert_eq!(
            network.calls.lock().unwrap()[0].2,
            "socks5://127.0.0.1:7891"
        );
    }

    #[tokio::test]
    async fn the_current_card_shows_the_expiry_of_the_login_codex_rotated_in() {
        let (_dir, app, _) = crate::legacy_tests::network_fixture::fixture();
        let token = |exp: i64| {
            let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"exp": exp})).unwrap());
            format!("x.{claims}.x")
        };
        let mut login = document("alice");
        login["tokens"]["access_token"] = token(1_800_000_000).into();
        login["last_refresh"] = "2026-10-02T10:29:58Z".into();
        app.import_json(&login.to_string()).await.unwrap();
        let id = app.accounts().await.unwrap()[0].id.clone();
        app.switch_account(&id).await.unwrap();
        let expiry = || async { app.accounts().await.unwrap()[0].token_expires_at };
        assert_eq!(expiry().await, Some(1_800_000_000));

        let mut live = login.clone();
        live["tokens"]["access_token"] = token(1_900_000_000).into();
        live["last_refresh"] = "2000-01-01T00:00:00Z".into();
        write_json(&app.paths.codex_auth, &live).unwrap();
        assert_eq!(
            expiry().await,
            Some(1_800_000_000),
            "an older login is ignored"
        );

        live["last_refresh"] = "2099-01-01T00:00:00Z".into();
        write_json(&app.paths.codex_auth, &live).unwrap();
        assert_eq!(expiry().await, Some(1_900_000_000));
    }

    #[tokio::test]
    async fn refresh_uses_the_newer_login_codex_rotated_into_auth_json() {
        let (_dir, app, network) = crate::legacy_tests::network_fixture::fixture();
        let mut login = document("alice");
        login["tokens"]["refresh_token"] = "stored-test-only".into();
        login["last_refresh"] = "2026-10-02T10:29:58.182Z".into();
        app.import_json(&login.to_string()).await.unwrap();
        let id = app.accounts().await.unwrap()[0].id.clone();
        app.switch_account(&id).await.unwrap();
        let stored_token = || {
            let store = store::load_store_from_path(&app.paths.store()).unwrap();
            store.accounts[0].auth_json["tokens"]["refresh_token"].clone()
        };
        let live = |token: &str, refreshed: &str| {
            let mut live = login.clone();
            live["tokens"]["refresh_token"] = token.into();
            live["last_refresh"] = refreshed.into();
            write_json(&app.paths.codex_auth, &live).unwrap();
        };
        // An older login in auth.json does not replace the stored one.
        live("older-test-only", "2000-01-01T00:00:00Z");
        app.refresh_usage(&id).await.unwrap();
        assert_eq!(stored_token(), "stored-test-only");
        // A rotation is adopted, and kept even when the request fails.
        live("rotated-test-only", "2099-01-01T00:00:00.5+00:00");
        *network.usage_error.lock().unwrap() = Some("offline".into());
        assert!(app.refresh_usage(&id).await.is_err());
        assert_eq!(stored_token(), "rotated-test-only");
        *network.usage_error.lock().unwrap() = None;
        live("next-test-only", "2099-01-02T00:00:00Z");
        app.refresh_usage(&id).await.unwrap();
        assert_eq!(stored_token(), "next-test-only");
        // Another account's login is never taken.
        let mut other = document("bob");
        other["tokens"]["refresh_token"] = "bob-test-only".into();
        other["last_refresh"] = "2099-01-03T00:00:00Z".into();
        write_json(&app.paths.codex_auth, &other).unwrap();
        app.refresh_usage(&id).await.unwrap();
        assert_eq!(stored_token(), "next-test-only");
    }

    #[tokio::test]
    async fn a_new_account_contacts_upstream_only_after_a_manual_refresh() {
        let (_dir, app, network) = crate::legacy_tests::network_fixture::waiting_fixture();
        assert!(!app.preferences().unwrap().request_new_accounts);
        app.import_authorized(document("alice"), None, &[])
            .await
            .unwrap();
        app.import_json(&document("bob").to_string()).await.unwrap();
        let accounts = app.accounts().await.unwrap();
        assert!(accounts.iter().all(|a| a.awaiting_first_request));
        // No import, background or workspace refresh, nor the proxy, uses them.
        app.refresh_recurring(0).await.unwrap();
        app.sync_workspace_directory().await.unwrap();
        assert_eq!(app.proxy_status().await.unwrap().available_accounts, 0);
        assert!(network.calls.lock().unwrap().is_empty());
        // A manual refresh releases one account.
        let alice = accounts
            .iter()
            .find(|a| a.email.as_deref() == Some("alice@example.com"))
            .unwrap();
        app.release_new_accounts(Some(&alice.id)).await.unwrap();
        app.refresh_usage(&alice.id).await.unwrap();
        assert_eq!(network.calls.lock().unwrap().len(), 1);
        assert_eq!(app.proxy_status().await.unwrap().available_accounts, 1);
        let accounts = app.accounts().await.unwrap();
        let alice = accounts.iter().find(|a| a.id == alice.id).unwrap();
        assert!(!alice.awaiting_first_request && alice.usage.is_some());
        // Signing in again to an account still waiting does not contact upstream.
        app.import_authorized(document("bob"), None, &[])
            .await
            .unwrap();
        assert_eq!(network.calls.lock().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn cards_show_when_the_background_refresh_asks_for_them_next() {
        let (_dir, app, _network) = crate::legacy_tests::network_fixture::waiting_fixture();
        app.import_json(
            &json!([document("alice"), document("bob"), document("carol")]).to_string(),
        )
        .await
        .unwrap();
        let accounts = app.accounts().await.unwrap();
        // Nothing is scheduled before the first background round.
        assert!(accounts.iter().all(|a| a.next_refresh_at.is_none()));
        assert_eq!(app.next_full_refresh_at(), None);
        let id = |name: &str| {
            let email = format!("{name}@example.com");
            accounts
                .iter()
                .find(|a| a.email.as_deref() == Some(email.as_str()))
                .unwrap()
                .id
                .clone()
        };
        for name in ["alice", "bob"] {
            app.release_new_accounts(Some(&id(name))).await.unwrap();
        }
        app.switch_account(&id("alice")).await.unwrap();
        app.set_refresh_schedule(crate::refresh_policy::Schedule {
            at: 1_000,
            tick: 2,
            interval: 300,
        });
        let accounts = app.accounts().await.unwrap();
        let next = |name: &str| {
            accounts
                .iter()
                .find(|a| a.id == id(name))
                .unwrap()
                .next_refresh_at
        };
        // The current card every round, the others on the next full round
        // (tick 6), and a new account only on a manual refresh.
        assert_eq!(next("alice"), Some(1_000));
        assert_eq!(next("bob"), Some(2_200));
        assert_eq!(next("carol"), None);
        // The refresh of the whole pool keeps its own schedule.
        assert_eq!(app.next_full_refresh_at(), Some(2_200));
    }
    #[tokio::test]
    async fn shared_workspace_keeps_principals_distinct_and_switch_is_exact() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        app.import_json(&json!([document("alice"), document("bob")]).to_string())
            .await
            .unwrap();
        app.import_json(&document("alice").to_string())
            .await
            .unwrap();
        let accounts = app.accounts().await.unwrap();
        assert_eq!(accounts.len(), 2);
        app.switch_account(&accounts[0].id).await.unwrap();
        let summaries = app.accounts().await.unwrap();
        assert_eq!(summaries.iter().filter(|a| a.is_current).count(), 1);
        let serialized = serde_json::to_string(&summaries).unwrap();
        assert!(!serialized.contains("test-only"));
        assert!(!serialized.contains("id_token"));
        app.switch_account(&accounts[1].id).await.unwrap();
        assert!(app.paths.data_dir.join("auth-before-switch.json").exists());
    }
    #[tokio::test]
    async fn smart_switch_reports_no_target_and_keeps_an_equally_good_current_account() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        assert_eq!(
            app.smart_switch().await.unwrap(),
            SmartSwitchOutcome::NoTarget
        );
        app.import_json(&json!([document("alice"), document("bob")]).to_string())
            .await
            .unwrap();
        let accounts = app.accounts().await.unwrap();
        app.switch_account(&accounts[1].id).await.unwrap();
        assert_eq!(
            app.smart_switch().await.unwrap(),
            SmartSwitchOutcome::AlreadyBest
        );
    }
    #[tokio::test]
    async fn proxy_status_counts_the_routable_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        assert_eq!(app.proxy_status().await.unwrap().available_accounts, 0);
        app.import_json(&json!([document("alice"), document("bob")]).to_string())
            .await
            .unwrap();
        assert_eq!(app.proxy_status().await.unwrap().available_accounts, 2);
    }
    #[tokio::test]
    async fn invalid_batch_is_atomic_and_corruption_has_a_private_backup() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        app.import_json(&document("alice").to_string())
            .await
            .unwrap();
        assert!(app
            .import_json(&json!([document("bob"), {}]).to_string())
            .await
            .is_err());
        assert_eq!(app.accounts().await.unwrap().len(), 1);
        fs::write(app.paths.store(), b"broken").unwrap();
        assert!(app.accounts().await.unwrap().is_empty());
        let backup = fs::read_dir(&app.paths.data_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("accounts.corrupt-")
            })
            .unwrap();
        assert_eq!(fs::read(backup.path()).unwrap(), b"broken");
    }
    #[tokio::test]
    async fn migration_preserves_swift_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        let entry = json!({"id":"old-id", "label":"Alice", "accountId":"shared-team", "authJson":document("alice"), "addedAt":1, "updatedAt":1, "workspaceName":"Research", "workspaceDirectory":[{"workspaceId":"x"}]});
        app.import_json(&json!({"accounts":[entry]}).to_string())
            .await
            .unwrap();
        let saved: Value = serde_json::from_slice(&fs::read(app.paths.store()).unwrap()).unwrap();
        assert_eq!(saved["accounts"][0]["workspaceName"], "Research");
    }
    #[tokio::test]
    async fn account_order_round_trips_and_tolerates_a_bad_value() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        let order = AccountOrder {
            codex: vec!["sub2api:team".into(), "a".into()],
            claude: vec!["claude-1".into()],
        };
        app.save_preferences(Preferences {
            account_order: order.clone(),
            ..app.preferences().unwrap()
        })
        .await
        .unwrap();
        assert_eq!(app.preferences().unwrap().account_order, order);
        let mut settings = app.configuration().unwrap();
        settings
            .extra
            .insert("accountOrder".into(), json!("broken"));
        app.persist_configuration(&settings).unwrap();
        assert_eq!(
            app.preferences().unwrap().account_order,
            AccountOrder::default()
        );
    }
    #[test]
    fn the_fixed_refresh_interval_is_dropped_from_old_preferences_and_settings() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();
        // Early Rust preferences still migrate with their interval present.
        fs::write(
            data.join("preferences.json"),
            r#"{"refreshSeconds": 10, "autoSwitch": true, "proxyPort": 9999}"#,
        )
        .unwrap();
        let app = Application::open(Paths {
            data_dir: data.clone(),
            codex_auth: dir.path().join("codex/auth.json"),
        })
        .unwrap();
        let preferences = app.preferences().unwrap();
        assert!(preferences.auto_switch);
        assert_eq!(preferences.proxy_port, 9999);
        assert!(!fs::read_dir(&data).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("preferences.corrupt-")));
        assert!(serde_json::to_value(&preferences)
            .unwrap()
            .get("refreshSeconds")
            .is_none());
        // Settings that still carry it lose it once saved.
        let mut settings = app.configuration().unwrap();
        settings.extra.insert("refreshSeconds".into(), 10.into());
        app.persist_configuration(&settings).unwrap();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(app.save_preferences(app.preferences().unwrap()))
            .unwrap();
        assert!(app
            .configuration()
            .unwrap()
            .extra
            .get("refreshSeconds")
            .is_none());
    }
    #[test]
    fn exclusive_instance_and_invalid_settings() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        assert!(Application::open(app.paths.clone()).is_err());
        assert!(validate_preferences(&Preferences {
            proxy_port: 0,
            ..Default::default()
        })
        .is_err());
    }
    #[test]
    fn smart_selection_excludes_exhausted_stale_failed_and_inactive_accounts() {
        let mut account: StoredAccount = serde_json::from_value(json!({"id":"a", "label":"A", "accountId":"team", "authJson":document("a"), "addedAt":1, "updatedAt":1})).unwrap();
        account.usage = Some(UsageSnapshot {
            remaining_reset_count: None,
            reset_expiries: Vec::new(),
            fetched_at: now_unix_seconds(),
            plan_type: None,
            five_hour: Some(UsageWindow {
                clock_offset_seconds: None,
                used_percent: 10.0,
                window_seconds: 18000,
                reset_at: None,
            }),
            one_week: Some(UsageWindow {
                clock_offset_seconds: None,
                used_percent: 20.0,
                window_seconds: 604800,
                reset_at: None,
            }),
            credits: None,
        });
        assert!(eligible(&account.to_summary(None)));
        account
            .usage
            .as_mut()
            .unwrap()
            .five_hour
            .as_mut()
            .unwrap()
            .used_percent = 100.0;
        assert!(!eligible(&account.to_summary(None)));
        account
            .usage
            .as_mut()
            .unwrap()
            .five_hour
            .as_mut()
            .unwrap()
            .used_percent = 10.0;
        account.usage.as_mut().unwrap().fetched_at = 1;
        assert!(!eligible(&account.to_summary(None)));
        account.usage.as_mut().unwrap().fetched_at = now_unix_seconds();
        account.usage_error = Some("failed".into());
        assert!(!eligible(&account.to_summary(None)));
        account.usage_error = None;
        account
            .extra
            .insert("displayStatus".into(), "deleted".into());
        assert!(!eligible(&account.to_summary(None)));
    }

    #[tokio::test]
    async fn proxy_authentication_rotation_and_stop() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        app.import_json(&document("alice").to_string())
            .await
            .unwrap();
        let status = proxy_service::start_api_proxy_with_runtime(
            &app.storage(),
            &app.state.api_proxy,
            Some(0),
            "127.0.0.1",
        )
        .await
        .unwrap();
        let url = format!("{}/models", status.base_url.unwrap());
        let client = reqwest::Client::new();
        assert_eq!(client.get(&url).send().await.unwrap().status(), 401);
        assert_eq!(
            client
                .get(&url)
                .bearer_auth(status.api_key.as_ref().unwrap())
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        let rotated = app.rotate_proxy_key().await.unwrap();
        assert_ne!(status.api_key, rotated.api_key);
        assert_eq!(
            client
                .get(&url)
                .bearer_auth(status.api_key.unwrap())
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert!(!app.stop_proxy().await.unwrap().running);
    }
}
