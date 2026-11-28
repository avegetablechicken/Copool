use crate::{
    models::*, platform::SyncAgent, pool::Pool, pool_login::OAuthCredential, proxy_service,
    state::AppState, utils::now_unix_seconds,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc};
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
    /// Each third-party agent's sync, every one of them listed.
    pub agent_sync: BTreeMap<SyncAgent, AgentSync>,
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
/// A third-party agent's sync: whether a switch hands it the login
/// switched to, and in which pools.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentSync {
    pub enabled: bool,
    pub pools: PoolSelection,
}
/// Pools chosen for one agent; a pool missing from the saved value is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PoolSelection {
    pub codex: bool,
    pub claude: bool,
    pub antigravity: bool,
    pub grok: bool,
}
impl Default for PoolSelection {
    fn default() -> Self {
        Self {
            codex: true,
            claude: true,
            antigravity: true,
            grok: true,
        }
    }
}
impl PoolSelection {
    pub fn includes(&self, pool: Pool) -> bool {
        match pool {
            Pool::Codex => self.codex,
            Pool::Claude => self.claude,
            Pool::Antigravity => self.antigravity,
            Pool::Grok => self.grok,
        }
    }
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
            agent_sync: SyncAgent::ALL
                .into_iter()
                .map(|agent| (agent, AgentSync::default()))
                .collect(),
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

/// Early Rust preferences kept each agent's switch under its own key and
/// the agents' pools in `syncPools`; they are read as `agentSync`.
fn key_agent_sync(object: &mut serde_json::Map<String, Value>) {
    let pools = object.remove("syncPools");
    let mut agent_sync = serde_json::Map::new();
    for agent in SyncAgent::ALL {
        let mut sync = serde_json::Map::new();
        if let Some(on) = object.remove(agent.switch_key()) {
            sync.insert("enabled".into(), on);
        }
        if let Some(chosen) = pools.as_ref().and_then(|pools| pools.get(agent.id())) {
            sync.insert("pools".into(), chosen.clone());
        }
        if !sync.is_empty() {
            agent_sync.insert(agent.id().into(), sync.into());
        }
    }
    if !agent_sync.is_empty() {
        object
            .entry("agentSync")
            .or_insert_with(|| agent_sync.into());
    }
}

/// What followed a switch, as Swift's SwitchAccountExecutionResult.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchExecution {
    /// The Codex provider switched to, for an unofficial provider's card.
    pub provider: Option<String>,
    /// Each agent the login was handed to, in [`SyncAgent::ALL`]'s order.
    pub agent_sync: Vec<AgentSyncOutcome>,
    pub restarted_editors: Vec<String>,
    pub editor_restart_error: Option<String>,
    pub launch_error: Option<String>,
    /// Claude Code settings that bypass the switched-to account login.
    pub login_overrides: Vec<String>,
}
/// An agent's sync after a switch: written, or why not.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSyncOutcome {
    pub agent: SyncAgent,
    pub error: Option<String>,
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
            Pool,
            Preferences,
            Option<Result<OAuthCredential>>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<SwitchExecution>> + Send>>
        + Send
        + Sync,
>;
impl Preferences {
    pub(crate) fn from_settings(settings: crate::settings::Settings) -> Result<Self> {
        let agent_sync = SyncAgent::ALL
            .into_iter()
            .map(|agent| {
                let sync = AgentSync {
                    enabled: agent.stored_switch(&settings),
                    // A selection this build cannot read keeps every
                    // pool chosen.
                    pools: settings
                        .extra
                        .get("syncPools")
                        .and_then(|pools| pools.get(agent.id()))
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default(),
                };
                (agent, sync)
            })
            .collect();
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
            agent_sync,
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
struct InstanceLock {
    file: fs::File,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // A forked child can keep a descriptor open after the app closes its own.
        let _ = self.file.unlock();
    }
}

pub struct Application {
    pub paths: Paths,
    pub(crate) secrets: Arc<dyn crate::credentials::SecretStore>,
    /// Copool's own Keychain item, holding `secrets` and the Claude pool's
    /// tokens; none off macOS and for injected secrets.
    pub(crate) keychain: Option<Arc<crate::vault::Item>>,
    pub(crate) network: Arc<dyn crate::codex::api::AccountNetwork>,
    pub(crate) state: AppState,
    pub(crate) operations: Mutex<()>,
    proxy_operations: Mutex<()>,
    /// Held to save a provider, whose password is kept apart from its
    /// record, and to read its connection: never one half saved.
    pub(crate) provider_saves: tokio::sync::RwLock<()>,
    /// Held to edit the CLIProxyAPI providers every pool shares.
    pub(crate) cliproxyapi_providers: std::sync::Mutex<()>,
    pub(crate) codex: crate::login_store::Locks,
    /// Reads of Codex's `auth.json` answer this late with what was there
    /// when asked.
    #[cfg(test)]
    pub(crate) codex_live_read_delay_ms: std::sync::atomic::AtomicU64,
    pub(crate) greetings_inflight: std::sync::Mutex<std::collections::BTreeSet<String>>,
    switch_effects: Arc<std::sync::RwLock<Option<SwitchEffect>>>,
    refreshing: std::sync::Mutex<std::collections::BTreeMap<String, usize>>,
    pub(crate) changes: Arc<std::sync::RwLock<Option<Arc<dyn Fn() + Send + Sync>>>>,
    pub(crate) claude: crate::claude::State,
    pub(crate) antigravity: crate::antigravity::State,
    pub(crate) grok: crate::grok::State,
    /// Each pool's background refresh rounds, by `Pool`.
    pub(crate) rounds: [crate::pool::Rounds; Pool::ALL.len()],
    /// Each pool's switches' turns at their effects, by `Pool`.
    switch_turns: [tokio::sync::Mutex<()>; Pool::ALL.len()],
    /// Itself, once shared (`into_shared`).
    this: std::sync::OnceLock<std::sync::Weak<Application>>,
    // Release the directory after the rest of the application has been dropped.
    _instance_lock: InstanceLock,
}
/// A switch's turn at its effects ([`Application::switch_turn`]).
pub(crate) struct SwitchTurn<'a>(#[allow(dead_code)] tokio::sync::MutexGuard<'a, ()>);
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
        let instance_lock = InstanceLock { file };
        let network = Arc::new(crate::codex::api::Network {
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
            provider_saves: Default::default(),
            cliproxyapi_providers: Default::default(),
            codex: Default::default(),
            #[cfg(test)]
            codex_live_read_delay_ms: Default::default(),
            greetings_inflight: Default::default(),
            switch_effects: Arc::new(std::sync::RwLock::new(None)),
            refreshing: Default::default(),
            changes: Arc::new(std::sync::RwLock::new(None)),
            claude: Default::default(),
            antigravity: Default::default(),
            grok: Default::default(),
            rounds: Default::default(),
            switch_turns: Default::default(),
            this: Default::default(),
            _instance_lock: instance_lock,
        };
        app.mark_kept_tunnel_token()?;
        crate::codex::migrate(&app.paths)?;
        crate::claude::migrate(&app.paths)?;
        app.migrate_codex_providers()?;
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
    pub(crate) fn on_switch_effects(&self, effect: SwitchEffect) {
        *self.switch_effects.write().unwrap() = Some(effect);
    }
    pub fn enable_desktop_effects(&self) {
        self.on_switch_effects(Arc::new(|pool, preferences, credential| {
            Box::pin(
                async move { crate::platform::after_switch(pool, &preferences, credential).await },
            )
        }));
    }
    /// A switch's turn at its effects, taken while the switch still holds
    /// its pool's operations lock: the effects of the pool's switches run in
    /// the order the switches were made, so the agents synced are left with
    /// the last switch's login, not an earlier one whose sync ended last.
    pub(crate) async fn switch_turn(&self, pool: Pool) -> SwitchTurn<'_> {
        SwitchTurn(self.switch_turns[pool as usize].lock().await)
    }
    /// The switch's effects, in its turn, given the Credential of the login
    /// switched to: none for a provider's card, an error for a login that
    /// grants none, which each agent to sync reports.
    pub(crate) async fn after_switch(
        &self,
        _turn: SwitchTurn<'_>,
        pool: Pool,
        credential: Option<Result<OAuthCredential>>,
    ) -> Result<SwitchExecution> {
        let effect = self.switch_effects.read().unwrap().clone();
        if let Some(effect) = effect {
            effect(pool, self.preferences()?, credential).await
        } else {
            Ok(SwitchExecution::default())
        }
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
                            key_agent_sync(object);
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
        let mut pools = serde_json::Map::new();
        for agent in SyncAgent::ALL {
            let sync = p.agent_sync.get(&agent).copied().unwrap_or_default();
            agent.store_switch(s, sync.enabled);
            pools.insert(
                agent.id().into(),
                serde_json::to_value(sync.pools).unwrap_or_default(),
            );
        }
        s.extra.insert("syncPools".into(), pools.into());
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
            serde_json::to_value(&p.refresh).unwrap_or_default(),
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
            if before
                .as_ref()
                .is_none_or(|b| b.pool(pool) != value.refresh.pool(pool))
            {
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
    /// The pool's API proxy, as the engine runs it for that pool.
    pub(crate) fn proxy_storage(&self, pool: Pool) -> proxy_service::ProxyStorageContext {
        match pool {
            Pool::Codex => self.codex_proxy_storage(),
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
    use crate::persistence::write_json;
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
    #[cfg(unix)]
    #[test]
    fn instance_lock_releases_even_with_an_inherited_descriptor() {
        let dir = tempfile::tempdir().unwrap();
        let first = app(&dir);
        let paths = first.paths.clone();
        // A forked child shares the open file description until exec or exit.
        let inherited = first._instance_lock.file.try_clone().unwrap();
        assert_eq!(
            Application::open(paths.clone()).err().as_deref(),
            Some("This Copool data directory is already in use")
        );
        drop(first);
        let _reopened = Application::open(paths).unwrap();
        drop(inherited);
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
            let store = app.codex_load().unwrap();
            store.accounts[0].document["tokens"]["refresh_token"].clone()
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

    /// The proxy takes the pool's logins, the one Codex uses marked current
    /// while Codex is on its official provider, and has the pool renew a
    /// login its upstream refused; the pool's store and Codex's login keep
    /// the renewal.
    #[tokio::test]
    async fn the_codex_proxy_takes_its_logins_and_renewals_from_the_pool() {
        let (_dir, app, network) = crate::legacy_tests::network_fixture::fixture();
        let app = app.into_shared();
        let mut login = document("alice");
        login["tokens"]["refresh_token"] = "refresh-test-only".into();
        app.import_json(Pool::Codex, &login.to_string())
            .await
            .unwrap();
        let id = app.accounts(Pool::Codex).await.unwrap()[0].id.clone();
        app.switch_account(Pool::Codex, &id).await.unwrap();
        let storage = app.proxy_storage(Pool::Codex);
        assert!(!storage.read_store && !storage.refresh_usage);
        let candidates = storage.provider_candidates.as_ref().unwrap()().unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].extra["copoolCurrent"], true);
        let renew = storage.renew_candidate.as_ref().unwrap();
        // A token renewed since is handed back as it is.
        assert_eq!(
            renew(id.clone(), "not-the-stored-token".into())
                .await
                .unwrap()["tokens"]["access_token"],
            "test-only"
        );
        let renewed = renew(id.clone(), "test-only".into()).await.unwrap();
        assert_eq!(renewed["tokens"]["access_token"], "refreshed-test-only");
        let stored = app.codex_load().unwrap();
        assert_eq!(stored.accounts[0].document, renewed);
        let live: Value =
            serde_json::from_slice(&fs::read(&app.paths.codex_auth).unwrap()).unwrap();
        assert_eq!(live["tokens"]["access_token"], "refreshed-test-only");
        assert_eq!(
            network
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(call, ..)| call == "refresh")
                .count(),
            1
        );
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
        let saved = app.codex_load().unwrap();
        assert_eq!(saved.accounts[0].document, login);
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
        // Rounds every 300 seconds, starting once the accounts were last
        // refreshed an interval ago.
        let refreshed = accounts.iter().map(|a| a.updated_at).max().unwrap();
        let at = refreshed + 1_000;
        let schedule = |at| crate::refresh_policy::Schedule {
            at,
            tick: 2,
            interval: 300,
            full_every: 6,
        };
        app.set_schedule(Pool::Codex, schedule(at));
        let accounts = app.accounts(Pool::Codex).await.unwrap();
        let next = |accounts: &[AccountSummary], name: &str| {
            accounts
                .iter()
                .find(|a| a.id == id(name))
                .unwrap()
                .next_refresh_at
        };
        // The current card every round, the others on the next full round
        // (tick 6), and a new account only on a manual refresh.
        assert_eq!(next(&accounts, "alice"), Some(at));
        assert_eq!(next(&accounts, "bob"), Some(at + 1_200));
        assert_eq!(next(&accounts, "carol"), None);
        // The refresh of the whole pool keeps its own schedule.
        assert_eq!(app.next_full_refresh_at(Pool::Codex), Some(at + 1_200));
        // A round within the interval since a card was refreshed passes it
        // by, as in every pool.
        let alice = accounts
            .iter()
            .find(|a| a.id == id("alice"))
            .unwrap()
            .updated_at;
        app.set_schedule(Pool::Codex, schedule(alice + 100));
        let accounts = app.accounts(Pool::Codex).await.unwrap();
        assert_eq!(next(&accounts, "alice"), Some(alice + 400));
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
        // The login left is the pool's own: nothing to keep aside.
        assert!(!app.paths.data_dir.join("auth-before-switch.json").exists());
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
    async fn an_invalid_batch_imports_nothing() {
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
            r#"{"refreshSeconds": 10, "autoSwitch": true, "proxyPort": 9999,
                "syncOpencodeOpenaiAuth": true, "syncPools": {"opencode": {"grok": false}}}"#,
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
        // As do its agents' syncs, kept before they were keyed by agent.
        let opencode = preferences.agent_sync[&SyncAgent::Opencode];
        assert!(opencode.enabled && !opencode.pools.grok && opencode.pools.codex);
        assert!(!preferences.agent_sync[&SyncAgent::Pi].enabled);
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

    /// Each agent's switch and pools round-trip through settings.json, its
    /// switch under the key it always had; a new install has all off and
    /// every pool chosen.
    #[tokio::test]
    async fn agent_sync_preferences_round_trip_under_their_keys() {
        let dir = tempfile::tempdir().unwrap();
        let app = Application::open(Paths {
            data_dir: dir.path().join("data"),
            codex_auth: dir.path().join("codex/auth.json"),
        })
        .unwrap();
        let initial = app.preferences().unwrap();
        assert_eq!(
            initial.agent_sync.keys().copied().collect::<Vec<_>>(),
            SyncAgent::ALL
        );
        for (agent, sync) in &initial.agent_sync {
            assert!(!sync.enabled, "{agent:?}");
            for pool in Pool::ALL {
                assert!(sync.pools.includes(pool), "{agent:?} {pool:?}");
            }
        }

        let mut updated = initial.clone();
        for sync in updated.agent_sync.values_mut() {
            sync.enabled = true;
        }
        let omp = updated.agent_sync.get_mut(&SyncAgent::Omp).unwrap();
        omp.pools.claude = false;
        updated
            .agent_sync
            .get_mut(&SyncAgent::Opencode)
            .unwrap()
            .pools
            .grok = false;
        app.save_preferences(updated.clone()).await.unwrap();
        assert_eq!(app.preferences().unwrap().agent_sync, updated.agent_sync);

        let saved: Value =
            serde_json::from_slice(&fs::read(dir.path().join("data/settings.json")).unwrap())
                .unwrap();
        for key in ["syncOpencodeOpenaiAuth", "syncPiAuth", "syncOmpAuth"] {
            assert_eq!(saved[key], true, "{key}");
        }
        assert_eq!(saved["syncPools"]["omp"]["claude"], false);
        assert_eq!(saved["syncPools"]["opencode"]["grok"], false);
        assert_eq!(saved["syncPools"]["pi"]["grok"], true);
    }

    /// Switches saved under the names Pi's and omp's had are still read,
    /// and a pool selection this build cannot read keeps every pool chosen.
    #[test]
    fn agent_sync_reads_earlier_names_and_tolerates_unreadable_pools() {
        let mut settings: crate::settings::Settings = Default::default();
        settings
            .extra
            .insert("syncPiOpenaiAuth".into(), true.into());
        settings
            .extra
            .insert("syncOmpOpenaiAuth".into(), true.into());
        settings.extra.insert(
            "syncPools".into(),
            json!({"pi": {"codex": false}, "omp": "not a selection"}),
        );
        let p = Preferences::from_settings(settings).unwrap();
        let pi = p.agent_sync[&SyncAgent::Pi];
        assert!(pi.enabled && !pi.pools.codex && pi.pools.grok);
        let omp = p.agent_sync[&SyncAgent::Omp];
        assert!(omp.enabled);
        assert_eq!(omp.pools, PoolSelection::default());
        assert!(!p.agent_sync[&SyncAgent::Opencode].enabled);

        let mut settings: crate::settings::Settings = Default::default();
        settings
            .extra
            .insert("syncPools".into(), json!("not a selection"));
        let p = Preferences::from_settings(settings).unwrap();
        assert!(p
            .agent_sync
            .values()
            .all(|sync| sync.pools == PoolSelection::default()));
    }
}
