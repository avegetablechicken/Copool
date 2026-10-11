use crate::{
    auth, models::*, persistence::write_json, proxy_service, state::AppState, store,
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
    pub refresh_seconds: u64,
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
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            refresh_seconds: 10,
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
        }
    }
}

type SwitchEffect = Arc<
    dyn Fn(
            Preferences,
            Option<Value>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
>;
impl Preferences {
    fn from_settings(settings: crate::settings::Settings) -> Result<Self> {
        let p = Preferences {
            refresh_seconds: settings
                .extra
                .get("refreshSeconds")
                .and_then(Value::as_u64)
                .unwrap_or(10),
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
        };
        validate_preferences(&p)?;
        Ok(p)
    }
}
pub struct Application {
    pub paths: Paths,
    pub(crate) secrets: Arc<dyn crate::credentials::SecretStore>,
    pub(crate) network: Arc<dyn crate::account_network::AccountNetwork>,
    pub(crate) state: AppState,
    pub(crate) operations: Mutex<()>,
    proxy_operations: Mutex<()>,
    // Prevent two app processes from concurrently mutating the same account pool.
    _instance_lock: fs::File,
    switch_effects: Arc<std::sync::RwLock<Option<SwitchEffect>>>,
    refreshing: std::sync::Mutex<std::collections::BTreeMap<String, usize>>,
    pub(crate) changes: Arc<std::sync::RwLock<Option<Arc<dyn Fn() + Send + Sync>>>>,
}
struct RefreshGuard<'a> {
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
        Self::open_with_secrets(paths, Arc::new(crate::credentials::SystemSecrets))
    }
    pub fn open_with_secrets(
        paths: Paths,
        secrets: Arc<dyn crate::credentials::SecretStore>,
    ) -> Result<Self> {
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
        let app = Self {
            paths,
            secrets,
            network: Arc::new(crate::account_network::Network),
            state: AppState::default(),
            operations: Mutex::new(()),
            proxy_operations: Mutex::new(()),
            _instance_lock: file,
            switch_effects: Arc::new(std::sync::RwLock::new(None)),
            refreshing: Default::default(),
            changes: Arc::new(std::sync::RwLock::new(None)),
        };
        app.migrate_provider_credentials()?;
        app.preferences()?;
        Ok(app)
    }
    pub fn refresh_activity(&self) -> Vec<String> {
        self.refreshing.lock().unwrap().keys().cloned().collect()
    }
    fn begin_refresh(&self, id: &str) -> RefreshGuard<'_> {
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
    fn persist_accounts(&self, accounts: &AccountsStore) -> Result<()> {
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
    async fn after_switch(&self, auth: Option<&Value>) -> Result<()> {
        let effect = self.switch_effects.read().unwrap().clone();
        if let Some(effect) = effect {
            effect(self.preferences()?, auth.cloned()).await
        } else {
            Ok(())
        }
    }
    pub(crate) fn storage(&self) -> proxy_service::ProxyStorageContext {
        let mut storage = proxy_service::new_proxy_storage_context(
            self.paths.data_dir.clone(),
            self.state.store_lock.clone(),
            false,
        );
        storage.prefer_local_selection = true;
        storage.active_auth_path = Some(self.paths.codex_auth.clone());
        storage.upstream_base_url = fs::read_to_string(self.codex_config_path())
            .ok()
            .and_then(|s| s.parse::<toml_edit::DocumentMut>().ok())
            .and_then(|d| {
                d.get("chatgpt_base_url")
                    .and_then(toml_edit::Item::as_str)
                    .map(str::to_owned)
            });
        let paths = self.paths.clone();
        storage.on_auth_refreshed = Some(Arc::new(move |id, auth| {
            crate::proxy_integration::sync_refreshed_auth(&paths, id, auth)
        }));
        let paths = self.paths.clone();
        storage.provider_candidates = Some(Arc::new(move || {
            crate::proxy_integration::candidates(&paths)
        }));
        let paths = self.paths.clone();
        let changes = self.changes.clone();
        let effects = self.switch_effects.clone();
        storage.on_selected = Some(Arc::new(move |id, auth| {
            let paths = paths.clone();
            let changes = changes.clone();
            let effects = effects.clone();
            Box::pin(async move {
                if crate::proxy_integration::select(&paths, &id, &auth)? {
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
                let legacy: Preferences =
                    serde_json::from_slice(&fs::read(old).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                validate_preferences(&legacy)?;
                Self::project_preferences(&mut settings, &legacy);
            }
            settings
                .extra
                .insert("rustPreferencesMigrated".into(), true.into());
            self.persist_configuration(&settings)?;
        }
        Preferences::from_settings(settings)
    }

    fn project_preferences(s: &mut crate::settings::Settings, p: &Preferences) {
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
            .insert("refreshSeconds".into(), p.refresh_seconds.into());
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
        let current = self.current_auth()?.as_ref().and_then(|v| identity(v).ok());
        let selected = store
            .extra
            .get("currentAccountId")
            .and_then(Value::as_str)
            .filter(|id| store.accounts.iter().any(|a| a.id == *id))
            .map(str::to_owned)
            .or_else(|| {
                store
                    .accounts
                    .iter()
                    .find(|a| {
                        current
                            .as_ref()
                            .is_some_and(|id| identity(&a.auth_json).ok().as_ref() == Some(id))
                    })
                    .map(|a| a.id.clone())
            });
        let mut summaries: Vec<_> = store
            .accounts
            .iter()
            .map(|a| {
                let mut summary = a.to_summary(None);
                summary.is_current = selected.as_ref().is_some_and(|id| *id == a.id);
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
                    summary.proxy_url = p
                        .account_proxy_urls
                        .get(&account.id.to_string())
                        .cloned()
                        .unwrap_or_default();
                    summary.is_current =
                        !provider.is_official() && provider.id.eq_ignore_ascii_case(&p.provider_id);
                    summaries.push(summary);
                }
            }
        }
        crate::ranking::sort_for_display(&mut summaries);
        Ok(summaries)
    }
    /// Import a refresh token without modifying the current Codex login.
    pub async fn import_refresh_token(&self, token: &str) -> Result<usize> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?;
        let document = auth::exchange_refresh_token_at(
            &client,
            "https://auth.openai.com/oauth/token",
            token,
            None,
        )
        .await?;
        self.import_json(&document.to_string()).await
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
            p.account_proxy_urls.remove(&account.id.to_string());
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
    pub async fn switch_account(&self, id: &str) -> Result<()> {
        if let Some((provider, account)) = self.cached_provider_account(id)? {
            if !account.status.eq_ignore_ascii_case("active") {
                return Err("Account is deactivated".into());
            }
            let _op = self.operations.lock().await;
            crate::codex_config::switch(&self.codex_config_path(), &provider.provider_id)?;
            self.publish();
            drop(_op);
            return self.after_switch(None).await;
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
    pub async fn smart_switch(&self) -> Result<String> {
        let accounts = self.accounts().await?;
        let best = accounts
            .iter()
            .filter(|a| eligible(a))
            .max_by(|a, b| score(a).total_cmp(&score(b)))
            .ok_or("No account with fresh, available quota; refresh usage first")?;
        self.switch_account(&best.id).await?;
        Ok(best.label.clone())
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
        if !force
            && account
                .usage
                .as_ref()
                .is_some_and(|u| now_unix_seconds() - u.fetched_at < 25)
        {
            return Ok(());
        }
        let result = crate::account_network::fetch(
            self.network.as_ref(),
            &account.auth_json,
            &account.proxy_url,
            now_unix_seconds(),
        )
        .await;
        let _lock = self.state.store_lock.lock().await;
        let mut saved = store::load_store_from_path(&self.paths.store())?;
        let current = saved.extra.get("currentAccountId").and_then(Value::as_str) == Some(id);
        if let Some(target) = saved.accounts.iter_mut().find(|a| a.id == id) {
            target.updated_at = now_unix_seconds();
            target
                .extra
                .insert("usageStateUpdatedAt".into(), target.updated_at.into());
            match &result {
                Ok((auth, value)) => {
                    target.usage = Some(value.clone());
                    target.usage_error = None;
                    target.auth_json = auth.clone();
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
                    if current && auth != &account.auth_json {
                        write_json(
                            &self.paths.codex_auth,
                            &crate::auth::normalized_auth_for_write(auth.clone())?,
                        )?;
                    }
                }
                Err(error) => {
                    target.usage_error = Some(if crate::account_network::expired_error(error) {
                        "error.accounts.sign_in_expired".into()
                    } else {
                        error.clone()
                    });
                    if crate::account_network::deactivated(error) {
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
        result.map(|_| ())
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
        let mut errors: Vec<String> = futures_util::future::join_all(refreshes)
            .await
            .into_iter()
            .filter_map(Result::err)
            .collect();
        for p in self.configuration()?.sub2_api_provider.providers {
            if let Err(error) = self.refresh_provider(&p.provider_id, false).await {
                errors.push(format!("{}: {error}", p.provider_id));
            }
        }
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
        let current = accounts.iter().find(|a| a.is_current);
        if !current.is_some_and(|a| {
            a.usage_error.is_none()
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
        let mut errors: Vec<String> = futures_util::future::join_all(refreshes)
            .await
            .into_iter()
            .filter_map(Result::err)
            .collect();
        let current = crate::codex_config::resolve_path(&self.codex_config_path());
        if self
            .configuration()?
            .sub2_api_provider
            .provider(&current.id)
            .is_some()
        {
            if let Err(error) = self.refresh_provider(&current.id, false).await {
                errors.push(error);
            }
        }
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
        // Refresh the whole pool at launch; the UI shows the stored values
        // meanwhile and updates each card as its refresh lands.
        let mut tick = 0u64;
        let _ = self.refresh_recurring(tick).await;
        loop {
            let seconds = self.preferences().map(|p| p.refresh_seconds).unwrap_or(10);
            tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
            tick = tick.wrapping_add(1);
            let _ = self.refresh_recurring(tick).await;
        }
    }
}
fn validate_preferences(p: &Preferences) -> Result<()> {
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
    if !(10..=86400).contains(&p.refresh_seconds) {
        return Err("Refresh interval must be 10–86400 seconds".into());
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
fn eligible(a: &AccountSummary) -> bool {
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
    fn app(dir: &tempfile::TempDir) -> Application {
        Application::open(Paths {
            data_dir: dir.path().join("data"),
            codex_auth: dir.path().join("codex/auth.json"),
        })
        .unwrap()
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
    #[test]
    fn exclusive_instance_and_invalid_settings() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        assert!(Application::open(app.paths.clone()).is_err());
        assert!(validate_preferences(&Preferences {
            refresh_seconds: 0,
            ..Default::default()
        })
        .is_err());
    }
    #[test]
    fn smart_selection_excludes_exhausted_stale_failed_and_inactive_accounts() {
        let mut account: StoredAccount = serde_json::from_value(json!({"id":"a", "label":"A", "accountId":"team", "authJson":document("a"), "addedAt":1, "updatedAt":1})).unwrap();
        account.usage = Some(UsageSnapshot {
            remaining_reset_count: None,
            fetched_at: now_unix_seconds(),
            plan_type: None,
            five_hour: Some(UsageWindow {
                used_percent: 10.0,
                window_seconds: 18000,
                reset_at: None,
            }),
            one_week: Some(UsageWindow {
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
