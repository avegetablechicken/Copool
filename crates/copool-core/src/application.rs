use crate::{
    auth::{self, newer_login},
    models::*,
    persistence::write_json,
    pool::Pool,
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
pub(crate) const SAY_HI_REQUIRES_PLUS: &str = "Say hi requires a ChatGPT Plus account";

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
    /// The Claude pool's API proxy, independent of the Codex one.
    pub claude_auto_start_proxy: bool,
    pub claude_proxy_port: u16,
    pub antigravity_proxy_port: u16,
    pub antigravity_auto_start_proxy: bool,
    pub locale: String,
    pub usage_display: String,
    pub theme: String,
    pub launch_at_startup: bool,
    pub launch_codex_after_switch: bool,
    pub sync_opencode_openai_auth: bool,
    pub host_api_only: bool,
    pub restart_editors_on_switch: bool,
    pub restart_editor_targets: Vec<String>,
    /// Contact upstream as soon as a new account is imported; otherwise it
    /// waits for a manual refresh, so that its proxy can be set first.
    pub request_new_accounts: bool,
    /// Default proxy of the requests for accounts without their own proxy,
    /// those right after an import included; never kept as a new account's
    /// own, nor used for a browser sign-in's token exchange.
    #[serde(alias = "importProxyUrl")]
    pub account_proxy_url: String,
    /// The order the account page's cards were dragged into, per pool.
    pub account_order: AccountOrder,
    /// Each pool's background refresh, shared by all its cards.
    pub refresh: crate::refresh_policy::RefreshPreferences,
}
/// Card keys in display order: an account's id, or `sub2api:<provider>` for
/// all of a Sub2api provider's cards. Cards not listed follow, ranked.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountOrder {
    pub codex: Vec<String>,
    pub claude: Vec<String>,
    pub antigravity: Vec<String>,
    pub grok: Vec<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            auto_switch: false,
            auto_start_proxy: false,
            proxy_port: 8787,
            claude_auto_start_proxy: false,
            claude_proxy_port: 8788,
            antigravity_proxy_port: 8789,
            antigravity_auto_start_proxy: false,
            locale: String::new(),
            usage_display: "used".into(),
            theme: "system".into(),
            launch_at_startup: false,
            launch_codex_after_switch: true,
            sync_opencode_openai_auth: false,
            host_api_only: false,
            restart_editors_on_switch: false,
            restart_editor_targets: vec![],
            request_new_accounts: false,
            account_proxy_url: String::new(),
            account_order: AccountOrder::default(),
            refresh: Default::default(),
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
        let port = |name: &str, default| match settings.extra.get(name) {
            None => Ok(default),
            Some(port) => port
                .as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .ok_or("Invalid proxy port"),
        };
        let p = Preferences {
            auto_switch: settings.auto_smart_switch,
            auto_start_proxy: settings.auto_start_api_proxy,
            proxy_port: settings.proxy_configuration["preferredPortText"]
                .as_str()
                .unwrap_or("8787")
                .parse()
                .map_err(|_| "Invalid proxy port")?,
            claude_auto_start_proxy: settings
                .extra
                .get("claudeAutoStartProxy")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            claude_proxy_port: port("claudeProxyPort", 8788)?,
            antigravity_proxy_port: port("antigravityProxyPort", 8789)?,
            antigravity_auto_start_proxy: settings
                .extra
                .get("antigravityAutoStartProxy")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            locale: settings.locale,
            usage_display: settings.usage_progress_display_mode,
            // An unrecognized theme is only a display preference.
            theme: settings
                .extra
                .get("theme")
                .and_then(Value::as_str)
                .filter(|value| matches!(*value, "system" | "light" | "dark"))
                .unwrap_or("system")
                .into(),
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
            // Never keeps Copool from starting: see `from_saved`.
            refresh: settings
                .extra
                .get("refresh")
                .map(crate::refresh_policy::RefreshPreferences::from_saved)
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
    pub(crate) greetings_inflight: std::sync::Mutex<std::collections::BTreeSet<String>>,
    // Prevent two app processes from concurrently mutating the same account pool.
    _instance_lock: fs::File,
    switch_effects: Arc<std::sync::RwLock<Option<SwitchEffect>>>,
    refreshing: std::sync::Mutex<std::collections::BTreeMap<String, usize>>,
    pub(crate) changes: Arc<std::sync::RwLock<Option<Arc<dyn Fn() + Send + Sync>>>>,
    pub(crate) claude: crate::claude::State,
    pub(crate) antigravity: crate::antigravity::State,
    pub(crate) grok: crate::grok::State,
    /// Each pool's background refresh rounds, by `Pool`.
    pub(crate) rounds: [crate::pool::Rounds; Pool::ALL.len()],
    /// Itself, once shared (`into_shared`).
    this: std::sync::OnceLock<std::sync::Weak<Application>>,
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
            greetings_inflight: Default::default(),
            _instance_lock: file,
            switch_effects: Arc::new(std::sync::RwLock::new(None)),
            refreshing: Default::default(),
            changes: Arc::new(std::sync::RwLock::new(None)),
            claude: Default::default(),
            antigravity: Default::default(),
            grok: Default::default(),
            rounds: Default::default(),
            this: Default::default(),
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
    fn storage(&self) -> proxy_service::ProxyStorageContext {
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
                        log::warn!("Failed to read the old preferences.json; kept a copy: {error}");
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
        s.extra.insert("theme".into(), p.theme.clone().into());
        s.proxy_configuration["preferredPortText"] = p.proxy_port.to_string().into();
        s.extra.insert(
            "claudeAutoStartProxy".into(),
            p.claude_auto_start_proxy.into(),
        );
        s.extra
            .insert("claudeProxyPort".into(), p.claude_proxy_port.into());
        s.launch_at_startup = p.launch_at_startup;
        s.launch_codex_after_switch = p.launch_codex_after_switch;
        s.sync_opencode_openai_auth = p.sync_opencode_openai_auth;
        s.extra.insert(
            "antigravityAutoStartProxy".into(),
            p.antigravity_auto_start_proxy.into(),
        );
        s.extra.insert(
            "antigravityProxyPort".into(),
            p.antigravity_proxy_port.into(),
        );
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
        s.extra.insert(
            "refresh".into(),
            serde_json::to_value(p.refresh).unwrap_or_default(),
        );
        s.extra
            .insert("rustPreferencesMigrated".into(), true.into());
    }
    pub async fn save_preferences(&self, value: Preferences) -> Result<()> {
        validate_preferences(&value)?;
        let _op = self.operations.lock().await;
        let mut settings = self.configuration()?;
        let before = Preferences::from_settings(settings.clone())
            .map(|p| p.refresh)
            .ok();
        Self::project_preferences(&mut settings, &value);
        self.persist_configuration(&settings)?;
        // A pool's new refresh applies from now on, not after its next round.
        for pool in Pool::ALL {
            if before.is_none_or(|b| b.pool(pool) != value.refresh.pool(pool)) {
                self.reschedule(pool, value.refresh.pool(pool));
            }
        }
        Ok(())
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
    /// The Codex pool, following the login Codex uses in auth.json.
    pub(crate) async fn codex_accounts(&self) -> Result<Vec<AccountSummary>> {
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
                summary.renewal = Some(a.renewal().as_str().into());
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
        // Sub2api cards are current while Codex uses their provider.
        let current = (!provider.is_official()).then_some(provider.id.as_str());
        summaries.extend(self.provider_cards(Pool::Codex, current)?);
        crate::ranking::sort_for_display(&mut summaries);
        if let Some(schedule) = self.schedule(Pool::Codex) {
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
    /// Imports the login Codex uses.
    pub(crate) async fn codex_import_current(&self) -> Result<usize> {
        let value = self
            .current_auth()?
            .ok_or("Codex auth.json not found; log in with Codex first")?;
        let count = self
            .codex_import_json(&serde_json::to_string(&value).map_err(|e| e.to_string())?)
            .await?;
        self.reconcile_workspace_names(true).await?;
        Ok(count)
    }
    /// Accept auth.json, arrays of auth documents, or a Swift/Rust accounts.json.
    /// Validate the complete batch before changing the destination; keep metadata.
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
    /// Edits the stored accounts under the locks the store's writers hold.
    pub(crate) async fn codex_edit_accounts(
        &self,
        edit: impl FnOnce(&mut [&mut dyn crate::pool::PoolAccount]) -> Result<bool>,
    ) -> Result<()> {
        let _op = self.operations.lock().await;
        let _lock = self.state.store_lock.lock().await;
        let mut saved = store::load_store_from_path(&self.paths.store())?;
        let changed = {
            let mut accounts: Vec<&mut dyn crate::pool::PoolAccount> = saved
                .accounts
                .iter_mut()
                .map(|a| a as &mut dyn crate::pool::PoolAccount)
                .collect();
            edit(&mut accounts)?
        };
        if changed {
            self.persist_accounts(&saved)?;
        }
        Ok(())
    }
    /// Removes an account; a deactivated workspace's stays listed as deleted.
    pub(crate) async fn codex_delete_account(&self, id: &str) -> Result<()> {
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
    /// Gives Codex this account's login, or a Sub2api card's provider.
    pub(crate) async fn codex_switch_account(&self, id: &str) -> Result<SwitchExecution> {
        if crate::sub2api::is_card(Pool::Codex, id) {
            let (provider, account) = self.card(Pool::Codex, id)?;
            if !account.status.eq_ignore_ascii_case("active") {
                return Err("Account is deactivated".into());
            }
            let _op = self.operations.lock().await;
            crate::codex_config::switch(&self.codex_config_path(), &provider.key)?;
            self.publish();
            drop(_op);
            let mut execution = self.after_switch(None).await?;
            execution.provider = Some(provider.key);
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
    /// Whether the stored account failed: its usage refused, or its token
    /// expired.
    pub(crate) async fn codex_login_failed(&self, id: &str) -> Result<bool> {
        let _lock = self.state.store_lock.lock().await;
        let saved = store::load_store_from_path(&self.paths.store())?;
        let account = saved
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?;
        Ok(account.usage_error.is_some()
            || auth::access_token_expiry(&account.auth_json)
                .is_some_and(|at| at <= now_unix_seconds()))
    }
    /// One hi with this account's login, without switching to it.
    pub(crate) async fn codex_hello(&self, id: &str) -> Result<crate::pool::Hello> {
        let account = {
            let _lock = self.state.store_lock.lock().await;
            store::load_store_from_path(&self.paths.store())?
                .accounts
                .into_iter()
                .find(|a| a.id == id)
                .ok_or("Account not found")?
        };
        let plan = account
            .usage
            .as_ref()
            .and_then(|u| u.plan_type.as_deref())
            .filter(|p| !p.trim().is_empty())
            .or(account.plan_type.as_deref())
            .unwrap_or_default();
        if !plan.trim().eq_ignore_ascii_case("plus") {
            return Err(SAY_HI_REQUIRES_PLUS.into());
        }
        let live = self
            .current_auth()
            .ok()
            .flatten()
            .filter(|live| newer_login(live, &account.auth_json));
        let auth = live.as_ref().unwrap_or(&account.auth_json);
        let expired = auth::access_token_expiry(auth).is_some_and(|at| at <= now_unix_seconds());
        let result = if expired {
            Err("Provided authentication token is expired".into())
        } else {
            let base = crate::usage::chatgpt_base_origin(
                crate::codex_config::chatgpt_base_url(&self.codex_config_path()).as_deref(),
            );
            proxy_service::say_hi(&base, auth, &self.account_proxy(&account.proxy_url)?).await
        };
        // An explicit greeting never preflights or skips based on usage.
        match result {
            Ok(()) => Ok(crate::pool::Hello::Said),
            Err(error) if crate::account_network::expired_error(&error) => {
                Ok(crate::pool::Hello::Expired(error))
            }
            Err(error) => Err(error),
        }
    }
    /// With `renew_now`, only the tokens are renewed, even when unexpired or
    /// renewal is off; usage is not asked for, nor shown as refreshing, and
    /// whether the store kept the renewal is reported.
    pub(crate) async fn codex_refresh_account(
        &self,
        id: &str,
        force: bool,
        renew_now: Option<&mut bool>,
    ) -> Result<()> {
        let forced = renew_now.is_some();
        let _activity = (!forced).then(|| self.begin_refresh(id));
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
        let mut selected = self
            .current_auth()
            .ok()
            .flatten()
            .filter(|live| newer_login(live, &account.auth_json))
            .unwrap_or_else(|| account.auth_json.clone());
        let proxy = self.account_proxy(&account.proxy_url)?;
        let mut retried = false;
        let result = loop {
            let auth_json = &selected;
            let attempt = if forced {
                match self.network.refresh(auth_json, &proxy).await {
                    Ok(auth) => Ok((auth, None)),
                    Err(message) => Err(crate::account_network::FetchFailure {
                        auth: auth_json.clone(),
                        message,
                    }),
                }
            } else {
                crate::account_network::fetch(
                    self.network.as_ref(),
                    auth_json,
                    &proxy,
                    now_unix_seconds(),
                    account.renews(),
                )
                .await
                .map(|(auth, usage)| (auth, Some(usage)))
            };
            let Err(failure) = &attempt else {
                break attempt;
            };
            if retried
                || failure.auth != selected
                || !crate::account_network::expired_error(&failure.message)
            {
                break attempt;
            }
            // A different process can rotate the login while usage or renewal
            // is in flight. Re-read both copies before recording a sign-in error.
            let saved = {
                let _lock = self.state.store_lock.lock().await;
                store::load_store_from_path(&self.paths.store())?
            };
            let Some(target) = saved.accounts.iter().find(|a| a.id == id) else {
                break attempt;
            };
            let mut replacement = &failure.auth;
            if newer_login(&target.auth_json, replacement) {
                replacement = &target.auth_json;
            }
            let disk = self.current_auth().ok().flatten();
            if let Some(login) = disk.as_ref().filter(|v| newer_login(v, replacement)) {
                replacement = login;
            }
            if replacement["tokens"] == failure.auth["tokens"] {
                break attempt;
            }
            selected = replacement.clone();
            retried = true;
        };
        let auth_json = &selected;
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
            match &result {
                Ok((auth, value)) => {
                    // A renewal alone leaves the usage as it was.
                    if let Some(value) = value {
                        target.usage = Some(value.clone());
                        target.usage_error = None;
                        target
                            .extra
                            .insert("workspaceStatus".into(), "active".into());
                        if target.extra.get("displayStatus").and_then(Value::as_str)
                            != Some("deleted")
                        {
                            target.extra.insert("displayStatus".into(), "list".into());
                        }
                    }
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
        if let Some(reported) = renew_now {
            let (Ok((auth, _)) | Err(crate::account_network::FetchFailure { auth, .. })) = &result;
            *reported = renewed && kept(auth);
        }
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
    /// The pool's API proxy, as the engine runs it for that pool.
    pub(crate) fn proxy_storage(&self, pool: Pool) -> proxy_service::ProxyStorageContext {
        match pool {
            Pool::Codex => self.storage(),
            Pool::Claude => self.claude_proxy_storage(),
            Pool::Antigravity => self.antigravity_proxy_storage(),
            // Never started: Copool has no Grok proxy.
            Pool::Grok => {
                let mut storage = proxy_service::new_proxy_storage_context(
                    self.paths.data_dir.clone(),
                    Default::default(),
                    false,
                );
                storage.pool = Pool::Grok;
                storage.key_file = "grok-api-proxy.key";
                storage.read_store = false;
                storage.refresh_usage = false;
                storage
            }
        }
    }
    fn proxy_slot(&self, pool: Pool) -> &Mutex<Option<crate::state::ApiProxyRuntimeHandle>> {
        match pool {
            Pool::Codex => &self.state.api_proxy,
            Pool::Antigravity => &self.state.antigravity_api_proxy,
            Pool::Claude => &self.state.claude_api_proxy,
            Pool::Grok => &self.state.grok_api_proxy,
        }
    }
    pub(crate) fn proxy_port(&self, pool: Pool) -> Result<u16> {
        let preferences = self.preferences()?;
        Ok(match pool {
            Pool::Codex => preferences.proxy_port,
            Pool::Claude => preferences.claude_proxy_port,
            Pool::Antigravity => preferences.antigravity_proxy_port,
            Pool::Grok => return self.grok_unavailable(),
        })
    }
    pub async fn proxy_status(&self, pool: Pool) -> Result<ApiProxyStatus> {
        if pool == Pool::Grok {
            return Ok(ApiProxyStatus {
                running: false,
                port: None,
                api_key: None,
                base_url: None,
                active_account_id: None,
                active_account_label: None,
                last_error: None,
                available_accounts: 0,
            });
        }
        proxy_service::get_api_proxy_status_with_runtime(
            &self.proxy_storage(pool),
            self.proxy_slot(pool),
        )
        .await
    }
    pub async fn start_proxy(&self, pool: Pool) -> Result<ApiProxyStatus> {
        if pool == Pool::Grok {
            return self.grok_unavailable();
        }
        let _op = self.proxy_operations.lock().await;
        proxy_service::start_api_proxy_with_runtime(
            &self.proxy_storage(pool),
            self.proxy_slot(pool),
            Some(self.proxy_port(pool)?),
            "127.0.0.1",
        )
        .await
    }
    pub async fn stop_proxy(&self, pool: Pool) -> Result<ApiProxyStatus> {
        if pool == Pool::Grok {
            return self.proxy_status(pool).await;
        }
        let _op = self.proxy_operations.lock().await;
        proxy_service::stop_api_proxy_with_runtime(&self.proxy_storage(pool), self.proxy_slot(pool))
            .await
    }
    pub async fn rotate_proxy_key(&self, pool: Pool) -> Result<ApiProxyStatus> {
        if pool == Pool::Grok {
            return self.grok_unavailable();
        }
        let _op = self.proxy_operations.lock().await;
        proxy_service::refresh_api_proxy_key_with_runtime(
            &self.proxy_storage(pool),
            self.proxy_slot(pool),
        )
        .await
    }
    /// The application shared by its callers; the Claude proxy reaches the
    /// pool through it while it runs.
    pub fn into_shared(self) -> Arc<Self> {
        let shared = Arc::new(self);
        let _ = shared.this.set(Arc::downgrade(&shared));
        shared
    }
    pub(crate) fn shared(&self) -> Option<Arc<Self>> {
        self.this.get()?.upgrade()
    }
}
fn validate_preferences(p: &Preferences) -> Result<()> {
    validate_proxy(&p.account_proxy_url).map_err(|_| "Invalid account proxy URL".to_string())?;
    if !["used", "remaining"].contains(&p.usage_display.as_str()) {
        return Err("Invalid usage display mode".into());
    }
    if !["system", "light", "dark"].contains(&p.theme.as_str()) {
        return Err("Invalid theme".into());
    }
    if ![
        "", "en", "zh-Hans", "zh-Hant", "ja", "ko", "fr", "de", "it", "es", "ru", "nl",
    ]
    .contains(&p.locale.as_str())
    {
        return Err("Unsupported locale".into());
    }
    if p.proxy_port == 0 || p.claude_proxy_port == 0 || p.antigravity_proxy_port == 0 {
        return Err("Proxy port must be 1–65535".into());
    }
    p.refresh.validate()
}
fn validate_proxy(value: &str) -> Result<()> {
    crate::utils::validate_account_proxy(value).map(|_| ())
}

fn identity(document: &Value) -> Result<(String, String)> {
    auth::account_identity(document)
}

/// Whether automatic switching may choose this card: available, fresh and
/// with quota left.
pub(crate) fn eligible(a: &AccountSummary) -> bool {
    a.available
        && a.usage_error.is_none()
        && !crate::ranking::exhausted(a)
        && crate::ranking::remaining_score(a) > 0.0
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
    async fn account_proxy_is_default_for_accounts_and_their_imports() {
        let (_dir, app, network) = crate::legacy_tests::network_fixture::fixture();
        app.import_authorized(document("alice"), None, &[])
            .await
            .unwrap();
        let account = app.accounts(Pool::Codex).await.unwrap().remove(0);
        app.save_account_proxy(Pool::Codex, &account.id, "socks5://127.0.0.1:7891")
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
        app.refresh_usage(Pool::Codex, &account.id).await.unwrap();
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
        // A new account's import-time requests take the shared proxy, which
        // it does not keep as its own.
        app.import_authorized(document("bob"), None, &[])
            .await
            .unwrap();
        assert_eq!(network.calls.lock().unwrap()[0].2, "http://127.0.0.1:7890");
        assert_eq!(
            app.accounts(Pool::Codex)
                .await
                .unwrap()
                .iter()
                .find(|a| a.id == account.id)
                .unwrap()
                .proxy_url,
            "socks5://127.0.0.1:7891"
        );
        let bob = app
            .accounts(Pool::Codex)
            .await
            .unwrap()
            .into_iter()
            .find(|a| a.id != account.id)
            .unwrap();
        network.calls.lock().unwrap().clear();
        app.refresh_usage(Pool::Codex, &bob.id).await.unwrap();
        assert_eq!(network.calls.lock().unwrap()[0].2, "http://127.0.0.1:7890");
        let storage = app.proxy_storage(Pool::Codex);
        assert_eq!(
            storage.account_proxy.as_ref().unwrap()().unwrap(),
            "http://127.0.0.1:7890"
        );
        let mut preferences = app.preferences().unwrap();
        preferences.account_proxy_url.clear();
        app.save_preferences(preferences).await.unwrap();
        assert_eq!(storage.account_proxy.as_ref().unwrap()().unwrap(), "");
        network.calls.lock().unwrap().clear();
        app.refresh_usage(Pool::Codex, &bob.id).await.unwrap();
        assert_eq!(network.calls.lock().unwrap()[0].2, "");
        network.calls.lock().unwrap().clear();
        app.refresh_usage(Pool::Codex, &account.id).await.unwrap();
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
        app.import_json(Pool::Codex, &login.to_string())
            .await
            .unwrap();
        let id = app.accounts(Pool::Codex).await.unwrap()[0].id.clone();
        app.switch_account(Pool::Codex, &id).await.unwrap();
        let expiry = || async { app.accounts(Pool::Codex).await.unwrap()[0].token_expires_at };
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
        app.import_json(Pool::Codex, &login.to_string())
            .await
            .unwrap();
        let id = app.accounts(Pool::Codex).await.unwrap()[0].id.clone();
        app.switch_account(Pool::Codex, &id).await.unwrap();
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
        app.refresh_usage(Pool::Codex, &id).await.unwrap();
        assert_eq!(stored_token(), "stored-test-only");
        // A rotation is adopted, and kept even when the request fails.
        live("rotated-test-only", "2099-01-01T00:00:00.5+00:00");
        *network.usage_error.lock().unwrap() = Some("offline".into());
        assert!(app.refresh_usage(Pool::Codex, &id).await.is_err());
        assert_eq!(stored_token(), "rotated-test-only");
        *network.usage_error.lock().unwrap() = None;
        live("next-test-only", "2099-01-02T00:00:00Z");
        app.refresh_usage(Pool::Codex, &id).await.unwrap();
        assert_eq!(stored_token(), "next-test-only");
        // Another account's login is never taken.
        let mut other = document("bob");
        other["tokens"]["refresh_token"] = "bob-test-only".into();
        other["last_refresh"] = "2099-01-03T00:00:00Z".into();
        write_json(&app.paths.codex_auth, &other).unwrap();
        app.refresh_usage(Pool::Codex, &id).await.unwrap();
        assert_eq!(stored_token(), "next-test-only");
    }

    #[tokio::test]
    async fn refresh_recovers_when_codex_rotates_while_usage_is_in_flight() {
        let (_dir, app, network) = crate::legacy_tests::network_fixture::fixture();
        let mut login = document("alice");
        login["last_refresh"] = "2026-01-01T00:00:00Z".into();
        app.import_json(Pool::Codex, &login.to_string())
            .await
            .unwrap();
        let id = app.accounts(Pool::Codex).await.unwrap()[0].id.clone();
        write_json(&app.paths.codex_auth, &login).unwrap();
        *network.rejected_access_token.lock().unwrap() = Some("test-only".into());
        *network.refresh_error.lock().unwrap() = Some("refresh token has already been used".into());
        network
            .block
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let rotate = async {
            network.started.notified().await;
            login["last_refresh"] = "2026-01-02T00:00:00Z".into();
            login["tokens"]["access_token"] = "new-access".into();
            login["tokens"]["refresh_token"] = "new-refresh".into();
            write_json(&app.paths.codex_auth, &login).unwrap();
            network.resume.notify_one();
        };
        let (result, ()) = tokio::join!(app.refresh_usage(Pool::Codex, &id), rotate);
        result.unwrap();
        let saved = store::load_store_from_path(&app.paths.store()).unwrap();
        assert_eq!(saved.accounts[0].auth_json, login);
        assert!(saved.accounts[0].usage_error.is_none());
        assert!(saved.accounts[0].usage.is_some());
    }

    #[tokio::test]
    async fn a_new_account_contacts_upstream_only_after_a_manual_refresh() {
        let (_dir, app, network) = crate::legacy_tests::network_fixture::waiting_fixture();
        assert!(!app.preferences().unwrap().request_new_accounts);
        app.import_authorized(document("alice"), None, &[])
            .await
            .unwrap();
        app.import_json(Pool::Codex, &document("bob").to_string())
            .await
            .unwrap();
        let accounts = app.accounts(Pool::Codex).await.unwrap();
        assert!(accounts.iter().all(|a| a.awaiting_first_request));
        // No import, background or workspace refresh, nor the proxy, uses them.
        app.refresh_recurring(Pool::Codex, 0).await.unwrap();
        app.sync_workspace_directory().await.unwrap();
        assert_eq!(
            app.proxy_status(Pool::Codex)
                .await
                .unwrap()
                .available_accounts,
            0
        );
        assert!(network.calls.lock().unwrap().is_empty());
        // A manual refresh releases one account.
        let alice = accounts
            .iter()
            .find(|a| a.email.as_deref() == Some("alice@example.com"))
            .unwrap();
        app.release_new_accounts(Pool::Codex, Some(&alice.id))
            .await
            .unwrap();
        app.refresh_usage(Pool::Codex, &alice.id).await.unwrap();
        assert_eq!(network.calls.lock().unwrap().len(), 1);
        assert_eq!(
            app.proxy_status(Pool::Codex)
                .await
                .unwrap()
                .available_accounts,
            1
        );
        let accounts = app.accounts(Pool::Codex).await.unwrap();
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
            Pool::Codex,
            &json!([document("alice"), document("bob"), document("carol")]).to_string(),
        )
        .await
        .unwrap();
        let accounts = app.accounts(Pool::Codex).await.unwrap();
        // Nothing is scheduled before the first background round.
        assert!(accounts.iter().all(|a| a.next_refresh_at.is_none()));
        assert_eq!(app.next_full_refresh_at(Pool::Codex), None);
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
            app.release_new_accounts(Pool::Codex, Some(&id(name)))
                .await
                .unwrap();
        }
        app.switch_account(Pool::Codex, &id("alice")).await.unwrap();
        app.set_schedule(
            Pool::Codex,
            crate::refresh_policy::Schedule {
                at: 1_000,
                tick: 2,
                interval: 300,
                full_every: 6,
            },
        );
        let accounts = app.accounts(Pool::Codex).await.unwrap();
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
        assert_eq!(app.next_full_refresh_at(Pool::Codex), Some(2_200));
    }
    #[tokio::test]
    async fn shared_workspace_keeps_principals_distinct_and_switch_is_exact() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        app.import_json(
            Pool::Codex,
            &json!([document("alice"), document("bob")]).to_string(),
        )
        .await
        .unwrap();
        app.import_json(Pool::Codex, &document("alice").to_string())
            .await
            .unwrap();
        let accounts = app.accounts(Pool::Codex).await.unwrap();
        assert_eq!(accounts.len(), 2);
        app.switch_account(Pool::Codex, &accounts[0].id)
            .await
            .unwrap();
        let summaries = app.accounts(Pool::Codex).await.unwrap();
        assert_eq!(summaries.iter().filter(|a| a.is_current).count(), 1);
        let serialized = serde_json::to_string(&summaries).unwrap();
        assert!(!serialized.contains("test-only"));
        assert!(!serialized.contains("id_token"));
        app.switch_account(Pool::Codex, &accounts[1].id)
            .await
            .unwrap();
        assert!(app.paths.data_dir.join("auth-before-switch.json").exists());
    }
    #[tokio::test]
    async fn smart_switch_reports_no_target_and_keeps_an_equally_good_current_account() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        assert_eq!(
            app.smart_switch(Pool::Codex).await.unwrap(),
            SmartSwitchOutcome::NoTarget
        );
        app.import_json(
            Pool::Codex,
            &json!([document("alice"), document("bob")]).to_string(),
        )
        .await
        .unwrap();
        let accounts = app.accounts(Pool::Codex).await.unwrap();
        app.switch_account(Pool::Codex, &accounts[1].id)
            .await
            .unwrap();
        assert_eq!(
            app.smart_switch(Pool::Codex).await.unwrap(),
            SmartSwitchOutcome::AlreadyBest
        );
    }
    #[tokio::test]
    async fn proxy_status_counts_the_routable_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        assert_eq!(
            app.proxy_status(Pool::Codex)
                .await
                .unwrap()
                .available_accounts,
            0
        );
        app.import_json(
            Pool::Codex,
            &json!([document("alice"), document("bob")]).to_string(),
        )
        .await
        .unwrap();
        assert_eq!(
            app.proxy_status(Pool::Codex)
                .await
                .unwrap()
                .available_accounts,
            2
        );
    }
    #[tokio::test]
    async fn invalid_batch_is_atomic_and_corruption_has_a_private_backup() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        app.import_json(Pool::Codex, &document("alice").to_string())
            .await
            .unwrap();
        assert!(app
            .import_json(Pool::Codex, &json!([document("bob"), {}]).to_string())
            .await
            .is_err());
        assert_eq!(app.accounts(Pool::Codex).await.unwrap().len(), 1);
        fs::write(app.paths.store(), b"broken").unwrap();
        assert!(app.accounts(Pool::Codex).await.unwrap().is_empty());
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
        app.import_json(Pool::Codex, &json!({"accounts":[entry]}).to_string())
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
            antigravity: vec!["antigravity-1".into()],
            grok: vec!["grok-1".into()],
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
    #[tokio::test]
    async fn theme_round_trips_validates_and_tolerates_a_bad_saved_value() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        assert_eq!(app.preferences().unwrap().theme, "system");
        for theme in ["dark", "light", "system"] {
            app.save_preferences(Preferences {
                theme: theme.into(),
                ..app.preferences().unwrap()
            })
            .await
            .unwrap();
            assert_eq!(app.preferences().unwrap().theme, theme);
        }
        assert!(app
            .save_preferences(Preferences {
                theme: "sepia".into(),
                ..app.preferences().unwrap()
            })
            .await
            .is_err());
        let mut settings = app.configuration().unwrap();
        settings.extra.insert("theme".into(), json!("sepia"));
        app.persist_configuration(&settings).unwrap();
        assert_eq!(app.preferences().unwrap().theme, "system");
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
            resets_last_confirmed_at: None,
            reset_expiries: Vec::new(),
            reset_grants: Vec::new(),
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
            one_month: None,
            credits: None,
            third_party_five_hour: None,
            third_party_one_week: None,
            fable_one_week: None,
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
        app.import_json(Pool::Codex, &document("alice").to_string())
            .await
            .unwrap();
        // An upstream listing a model, so an authorized request succeeds.
        let upstream = axum::Router::new().fallback(|| async {
            axum::Json(serde_json::json!({"models": [{"slug": "gpt-6-luna"}]}))
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut storage = app.proxy_storage(Pool::Codex);
        storage.upstream_base_url = Some(format!("http://{}", listener.local_addr().unwrap()));
        tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let status = proxy_service::start_api_proxy_with_runtime(
            &storage,
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
        let rotated = app.rotate_proxy_key(Pool::Codex).await.unwrap();
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
        assert!(!app.stop_proxy(Pool::Codex).await.unwrap().running);
    }
}
