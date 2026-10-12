//! Provider editing, credentials, and account membership used by the desktop.
use crate::{
    application::Application,
    codex_config,
    credentials::SecretStore,
    repository,
    settings::{Provider, Settings},
    sub2api,
    utils::{now_unix_seconds, validate_account_proxy},
};
use serde_json::Value;

pub(crate) fn validate_and_save(
    mut draft: Provider,
    saved: &mut Settings,
    secrets: &dyn SecretStore,
) -> Result<Provider, String> {
    draft = draft.normalized();
    let existing = saved
        .sub2_api_provider
        .providers
        .iter()
        .find(|p| p.id == draft.id);
    if saved
        .sub2_api_provider
        .providers
        .iter()
        .any(|p| p.id != draft.id && p.provider_id.eq_ignore_ascii_case(&draft.provider_id))
    {
        return Err("error.sub2api.duplicate_provider".into());
    }
    if draft.provider_id.is_empty()
        || draft.username.is_empty()
        || (draft.password.is_empty() && existing.is_none())
    {
        return Err("error.sub2api.configuration_incomplete".into());
    }
    validate_account_proxy(&draft.proxy_url)?;
    if !draft.admin_base_url.is_empty() {
        let url = reqwest::Url::parse(&draft.admin_base_url)
            .map_err(|_| "error.sub2api.invalid_base_url")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("error.sub2api.invalid_base_url".into());
        }
    }
    if let Some(old) = existing {
        draft.imported_account_ids = old.imported_account_ids.clone();
        draft.cached_accounts = old.cached_accounts.clone();
        draft.account_proxy_urls = old.account_proxy_urls.clone();
    }
    // Never read credentials while editing. Empty means retain the stored secret.
    if !draft.password.is_empty() {
        secrets.set(&draft.id, &draft.password)?;
    }
    draft.password.clear();
    draft.legacy_admin_base_url.clear();
    saved.sub2_api_provider.upsert(draft.clone());
    Ok(draft)
}

impl Application {
    pub async fn save_tunnel_configuration(
        &self,
        config: &crate::tunnel::Configuration,
    ) -> Result<(), String> {
        let _op = self.operations.lock().await;
        if !config.api_token.is_empty() {
            self.secrets
                .set("cloudflare-api-token", &config.api_token)?;
        }
        let mut settings = self.configuration()?;
        settings.proxy_configuration["cloudflared"] =
            serde_json::to_value(config).map_err(|e| e.to_string())?;
        self.persist_configuration(&settings)
    }
    pub(crate) fn migrate_provider_credentials(&self) -> Result<(), String> {
        let mut settings = self.configuration()?;
        let mut changed = false;
        for p in &mut settings.sub2_api_provider.providers {
            if !p.legacy_admin_base_url.is_empty() {
                if let Some(id) = codex_config::matching_provider(
                    &p.legacy_admin_base_url,
                    &self.codex_config_path(),
                ) {
                    p.provider_id = id.clone();
                    for cached in &mut p.cached_accounts {
                        cached["providerID"] = id.clone().into();
                    }
                }
                p.legacy_admin_base_url.clear();
                changed = true;
            }
            if !p.password.is_empty() {
                self.secrets.set(&p.id, &p.password)?;
                p.password.clear();
                changed = true;
            }
        }
        if changed {
            self.persist_configuration(&settings)?;
        }
        Ok(())
    }
    pub fn configuration(&self) -> Result<Settings, String> {
        repository::load_settings(&self.paths.data_dir.join("settings.json"))
    }
    pub(crate) fn persist_configuration(&self, value: &Settings) -> Result<(), String> {
        repository::save_settings(&self.paths.data_dir.join("settings.json"), value)?;
        self.publish();
        Ok(())
    }
    pub fn codex_config_path(&self) -> std::path::PathBuf {
        self.paths.codex_auth.with_file_name("config.toml")
    }
    pub fn provider_definitions(&self) -> Vec<codex_config::Definition> {
        codex_config::definitions(&self.codex_config_path())
    }
    pub async fn save_provider(&self, draft: Provider) -> Result<Provider, String> {
        let _op = self.operations.lock().await;
        let mut saved = self.configuration()?;
        let provider = validate_and_save(draft, &mut saved, self.secrets.as_ref())?;
        self.persist_configuration(&saved)?;
        Ok(provider)
    }
    pub async fn remove_provider(&self, id: &str) -> Result<(), String> {
        let _op = self.operations.lock().await;
        let mut saved = self.configuration()?;
        saved.sub2_api_provider.providers.retain(|p| p.id != id);
        self.persist_configuration(&saved)?;
        self.secrets.remove(id)
    }
    pub(crate) fn cached_provider_account(
        &self,
        card_id: &str,
    ) -> Result<Option<(Provider, sub2api::Account)>, String> {
        for p in self.configuration()?.sub2_api_provider.providers {
            for raw in &p.cached_accounts {
                if let Ok(mut a) = serde_json::from_value::<sub2api::Account>(raw.clone()) {
                    a.provider_id = Some(p.provider_id.clone());
                    if a.card_id() == card_id {
                        return Ok(Some((p, a)));
                    }
                }
            }
        }
        Ok(None)
    }
    pub async fn save_account_proxy(&self, id: &str, value: &str) -> Result<(), String> {
        validate_account_proxy(value)?;
        let _op = self.operations.lock().await;
        if let Some((provider, account)) = self.cached_provider_account(id)? {
            let mut settings = self.configuration()?;
            let p = settings
                .sub2_api_provider
                .providers
                .iter_mut()
                .find(|p| p.id == provider.id)
                .ok_or("Account not found")?;
            if value.trim().is_empty() {
                p.account_proxy_urls.remove(&account.id.to_string());
            } else {
                p.account_proxy_urls
                    .insert(account.id.to_string(), value.trim().into());
            }
            self.persist_configuration(&settings)
        } else {
            let _lock = self.state.store_lock.lock().await;
            repository::mutate_accounts(&self.paths.store(), |store| {
                let a = store
                    .accounts
                    .iter_mut()
                    .find(|a| a.id == id)
                    .ok_or("Account not found")?;
                a.proxy_url = value.trim().into();
                Ok(())
            })
            .map(|_| ())
        }
    }
    /// Sync adopts membership. Refresh preserves imported IDs and only updates their intersection.
    pub async fn refresh_provider(
        &self,
        provider_id: &str,
        sync_membership: bool,
    ) -> Result<usize, String> {
        let snapshot = self.configuration()?;
        // The provider's cards show as refreshing until its accounts land.
        let _activity: Vec<_> = snapshot
            .sub2_api_provider
            .providers
            .iter()
            .filter(|p| p.provider_id.eq_ignore_ascii_case(provider_id))
            .flat_map(|p| {
                p.cached_accounts.iter().filter_map(|raw| {
                    let mut account =
                        serde_json::from_value::<sub2api::Account>(raw.clone()).ok()?;
                    account.provider_id = Some(p.provider_id.clone());
                    Some(account.card_id())
                })
            })
            .collect::<Vec<_>>()
            .iter()
            .map(|id| self.begin_refresh(id))
            .collect();
        let mut connection = sub2api::connection(
            &self.codex_config_path(),
            &snapshot.sub2_api_provider,
            Some(provider_id),
            self.secrets.as_ref(),
        )?;
        if !sync_membership {
            connection.account_request_proxy = self.preferences()?.account_proxy_url;
        }
        let fetched = connection.fetch_accounts(None, now_unix_seconds()).await?;
        self.apply_provider_refresh(provider_id, fetched, sync_membership)
            .await
    }
    pub(crate) async fn apply_provider_refresh(
        &self,
        provider_id: &str,
        fetched: Vec<sub2api::Account>,
        sync_membership: bool,
    ) -> Result<usize, String> {
        let _op = self.operations.lock().await;
        let mut latest = self.configuration()?;
        let p = latest
            .sub2_api_provider
            .providers
            .iter_mut()
            .find(|p| p.provider_id.eq_ignore_ascii_case(provider_id))
            .ok_or("Provider was removed during refresh")?;
        let ids: Vec<_> = fetched.iter().map(|a| a.id).collect();
        let changed = ids.iter().any(|id| !p.imported_account_ids.contains(id))
            || p.imported_account_ids.iter().any(|id| !ids.contains(id));
        if sync_membership {
            p.imported_account_ids = ids;
        }
        p.cached_accounts = fetched
            .into_iter()
            .filter(|a| p.imported_account_ids.contains(&a.id))
            .map(|a| serde_json::to_value(a).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let count = p.cached_accounts.len();
        let key = format!("providerSyncNeeded:{}", p.provider_id);
        latest
            .extra
            .insert(key, Value::Bool(changed && !sync_membership));
        self.persist_configuration(&latest)?;
        Ok(count)
    }
    pub(crate) async fn refresh_provider_card(
        &self,
        provider: &Provider,
        id: i64,
    ) -> Result<(), String> {
        let settings = self.configuration()?;
        let mut connection = sub2api::connection(
            &self.codex_config_path(),
            &settings.sub2_api_provider,
            Some(&provider.provider_id),
            self.secrets.as_ref(),
        )?;
        connection.account_request_proxy = self.preferences()?.account_proxy_url;
        let accounts = connection
            .fetch_accounts(Some(&[id]), now_unix_seconds())
            .await?;
        let _op = self.operations.lock().await;
        let mut latest = self.configuration()?;
        if let Some(p) = latest
            .sub2_api_provider
            .providers
            .iter_mut()
            .find(|p| p.id == provider.id)
        {
            for a in accounts {
                if let Some(cached) = p
                    .cached_accounts
                    .iter_mut()
                    .find(|v| v["id"].as_i64() == Some(a.id))
                {
                    *cached = serde_json::to_value(a).map_err(|e| e.to_string())?;
                }
            }
            self.persist_configuration(&latest)?;
        }
        Ok(())
    }
}
