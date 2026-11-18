//! Sub2api providers in either pool: their settings forms, their cards and
//! how both are refreshed, implemented once. Each pool only keeps its
//! providers in its own place; the Codex pool's, here, in the settings
//! beside the Codex configuration naming their gateways.
use crate::{
    application::{Application, SAY_HI_REQUIRES_PLUS},
    codex_config,
    models::AccountSummary,
    pool::Pool,
    repository,
    settings::{Provider, Settings},
    sub2api::{self, Account, Member},
    utils::now_unix_seconds,
};
use serde_json::Value;

type Result<T> = std::result::Result<T, String>;

impl Application {
    /// A pool's public tunnel configuration, kept apart from the other's.
    pub async fn save_tunnel_configuration(
        &self,
        pool: Pool,
        config: &crate::tunnel::Configuration,
    ) -> Result<()> {
        let _op = self.operations.lock().await;
        let mut config = config.clone();
        if !config.api_token.is_empty() {
            self.secrets
                .set(cloudflare_token_secret(pool), &config.api_token)?;
        }
        config.has_api_token = !config.api_token.is_empty() || self.has_cloudflare_token(pool);
        let mut settings = self.configuration()?;
        settings.proxy_configuration[tunnel_configuration_key(pool)] =
            serde_json::to_value(&config).map_err(|e| e.to_string())?;
        self.persist_configuration(&settings)
    }
    /// The configuration with its saved API token, which the settings never
    /// hold, for starting a named tunnel.
    pub fn with_tunnel_token(
        &self,
        pool: Pool,
        mut config: crate::tunnel::Configuration,
    ) -> Result<crate::tunnel::Configuration> {
        if config.tunnel_mode == "named" && config.api_token.is_empty() {
            if let Some(token) = self.secrets.get(cloudflare_token_secret(pool))? {
                config.api_token = token;
            }
        }
        Ok(config)
    }
    fn has_cloudflare_token(&self, pool: Pool) -> bool {
        self.secrets
            .get(cloudflare_token_secret(pool))
            .ok()
            .flatten()
            .is_some_and(|token| !token.is_empty())
    }
    pub(crate) fn migrate_provider_credentials(&self) -> Result<()> {
        let mut settings = self.configuration()?;
        let mut changed = false;
        // A named tunnel saved before the token was marked as kept.
        if let Some(cloudflared) = settings
            .proxy_configuration
            .get_mut("cloudflared")
            .and_then(serde_json::Value::as_object_mut)
            .filter(|c| !c.contains_key("hasAPIToken"))
            .filter(|c| {
                c.get("accountId")
                    .and_then(|v| v.as_str())
                    .is_some_and(|v| !v.is_empty())
            })
        {
            cloudflared.insert(
                "hasAPIToken".into(),
                self.has_cloudflare_token(Pool::Codex).into(),
            );
            changed = true;
        }
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
    pub fn configuration(&self) -> Result<Settings> {
        repository::load_settings(&self.paths.data_dir.join("settings.json"))
    }
    pub(crate) fn persist_configuration(&self, value: &Settings) -> Result<()> {
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
    /// A pool's Sub2api providers for the settings, without their secrets.
    pub fn providers(&self, pool: Pool) -> Result<Value> {
        match pool {
            Pool::Codex => serde_json::to_value(self.configuration()?.sub2_api_provider.providers),
            Pool::Claude => serde_json::to_value(self.claude_providers()?),
        }
        .map_err(|e| e.to_string())
    }
    /// Saves a provider form; a blank password keeps the saved one.
    pub async fn save_provider(&self, pool: Pool, draft: Value) -> Result<Value> {
        let invalid = |e: serde_json::Error| e.to_string();
        match pool {
            Pool::Codex => serde_json::to_value(
                self.codex_save_provider(serde_json::from_value(draft).map_err(invalid)?)
                    .await?,
            ),
            Pool::Claude => serde_json::to_value(
                self.claude_save_provider(serde_json::from_value(draft).map_err(invalid)?)
                    .await?,
            ),
        }
        .map_err(invalid)
    }
    pub async fn remove_provider(&self, pool: Pool, id: &str) -> Result<()> {
        match pool {
            Pool::Codex => self.codex_remove_provider(id).await?,
            Pool::Claude => self.claude_remove_provider(id).await?,
        }
        self.secrets.remove(&sub2api::password_secret(pool, id))
    }
    /// Keeps a form's password in the keyring, never in the settings; a
    /// blank one keeps the saved one, which is never read while editing.
    pub(crate) fn store_provider_password(
        &self,
        pool: Pool,
        id: &str,
        password: &str,
    ) -> Result<()> {
        if !password.is_empty() {
            self.secrets
                .set(&sub2api::password_secret(pool, id), password)?;
        }
        Ok(())
    }
    fn members(&self, pool: Pool) -> Result<Vec<Member>> {
        match pool {
            Pool::Codex => Ok(self
                .configuration()?
                .sub2_api_provider
                .providers
                .iter()
                .map(codex_member)
                .collect()),
            Pool::Claude => self.claude_members(),
        }
    }
    fn member(&self, pool: Pool, key: &str) -> Result<Member> {
        self.members(pool)?
            .into_iter()
            .find(|m| m.key.eq_ignore_ascii_case(key))
            .ok_or_else(|| {
                match pool {
                    Pool::Codex => sub2api::PROVIDER_NOT_FOUND,
                    Pool::Claude => "Provider not found",
                }
                .into()
            })
    }
    fn member_connection(&self, pool: Pool, member: &Member) -> Result<sub2api::Connection> {
        match pool {
            Pool::Codex => sub2api::connection(
                &self.codex_config_path(),
                &self.configuration()?.sub2_api_provider,
                Some(&member.key),
                self.secrets.as_ref(),
            ),
            Pool::Claude => self.claude_member_connection(&member.key),
        }
    }
    /// Edits a provider's cards where its pool keeps them; none when it was
    /// removed.
    async fn edit_member<T: Send>(
        &self,
        pool: Pool,
        key: &str,
        edit: impl FnOnce(&mut Member) -> T + Send,
    ) -> Result<Option<T>> {
        match pool {
            Pool::Codex => self.codex_edit_member(key, edit).await,
            Pool::Claude => self.claude_edit_member(key, edit).await,
        }
    }
    /// Every provider's cards; those of the provider the pool uses
    /// (`current`) are current.
    pub(crate) fn provider_cards(
        &self,
        pool: Pool,
        current: Option<&str>,
    ) -> Result<Vec<AccountSummary>> {
        Ok(self
            .members(pool)?
            .iter()
            .flat_map(|m| {
                let current = current.is_some_and(|c| c.eq_ignore_ascii_case(&m.key));
                m.summaries(pool, current)
            })
            .collect())
    }
    /// A card's provider and account.
    pub(crate) fn card(&self, pool: Pool, id: &str) -> Result<(Member, Account)> {
        self.members(pool)?
            .into_iter()
            .find_map(|m| {
                let account = m
                    .cached
                    .iter()
                    .find(|a| m.card_id(pool, a.id) == id)?
                    .clone();
                Some((m, account))
            })
            .ok_or_else(|| "Account not found".into())
    }
    /// Removes a card; only an explicit sync of its provider restores it.
    pub(crate) async fn delete_card(&self, pool: Pool, id: &str) -> Result<()> {
        let (member, account) = self.card(pool, id)?;
        self.edit_member(pool, &member.key, |m| {
            m.imported.retain(|v| *v != account.id);
            m.cached.retain(|a| a.id != account.id);
        })
        .await?
        .ok_or_else(|| "Account not found".into())
    }
    /// A card's proxy is its provider's, for every request to it.
    pub(crate) async fn set_card_proxy(&self, pool: Pool, id: &str, value: String) -> Result<()> {
        let (member, _) = self.card(pool, id)?;
        self.edit_member(pool, &member.key, |m| m.proxy_url = value)
            .await?
            .ok_or_else(|| "Account not found".into())
    }
    pub(crate) async fn refresh_card(&self, pool: Pool, id: &str) -> Result<()> {
        let (member, account) = self.card(pool, id)?;
        self.refresh_provider_accounts(pool, &member.key, Some(&[account.id]), false)
            .await
            .map(|_| ())
    }
    /// Says hi through Sub2api's connection test of the card's account.
    pub(crate) async fn say_hi_through_provider(&self, pool: Pool, id: &str) -> Result<()> {
        let model = match pool {
            Pool::Codex => return Err(SAY_HI_REQUIRES_PLUS.into()),
            Pool::Claude => crate::claude::HELLO_MODEL,
        };
        let (member, account) = self.card(pool, id)?;
        self.member_connection(pool, &member)?
            .test_account(account.id, model)
            .await
    }
    /// Refreshes the providers by key, every one without; returns their errors.
    pub(crate) async fn refresh_providers(
        &self,
        pool: Pool,
        keys: Option<&[String]>,
    ) -> Vec<String> {
        let members = match self.members(pool) {
            Ok(members) => members,
            Err(error) => return vec![error],
        };
        let refreshes = members
            .iter()
            .filter(|m| keys.is_none_or(|keys| keys.iter().any(|k| k.eq_ignore_ascii_case(&m.key))))
            .map(|m| async move {
                self.refresh_provider_accounts(pool, &m.key, None, false)
                    .await
                    .map_err(|error| format!("{}: {error}", m.name))
            });
        futures_util::future::join_all(refreshes)
            .await
            .into_iter()
            .filter_map(Result::err)
            .collect()
    }
    /// Refreshes a provider's cards and usage, adopting the accounts it
    /// discovers; a sync takes its accounts as they are, restoring removed
    /// cards. Returns how many cards it has.
    pub async fn refresh_provider(&self, pool: Pool, key: &str, sync: bool) -> Result<usize> {
        self.refresh_provider_accounts(pool, key, None, sync).await
    }
    /// Its accounts by `ids`, every one without.
    async fn refresh_provider_accounts(
        &self,
        pool: Pool,
        key: &str,
        ids: Option<&[i64]>,
        sync: bool,
    ) -> Result<usize> {
        let member = self.member(pool, key)?;
        // Only an explicit sync restores a provider whose cards were removed.
        if !sync && member.imported.is_empty() {
            return Ok(0);
        }
        let wanted = |id: i64| ids.is_none_or(|ids| ids.contains(&id));
        // The cards show as refreshing until their accounts land.
        let cards: Vec<String> = member
            .cached
            .iter()
            .filter(|a| wanted(a.id))
            .map(|a| member.card_id(pool, a.id))
            .collect();
        let _activity: Vec<_> = cards.iter().map(|id| self.begin_refresh(id)).collect();
        let fetched = async {
            let connection = self.member_connection(pool, &member)?;
            let now = now_unix_seconds();
            if sync {
                Ok(sub2api::Refresh {
                    accounts: connection.fetch_accounts(pool, None, now).await?,
                    errors: vec![],
                })
            } else {
                connection.refresh_accounts(pool, ids, now).await
            }
        }
        .await;
        let fetched = match fetched {
            Ok(fetched) => fetched,
            Err(error) => {
                self.edit_member(pool, &member.key, |m| {
                    for account in m.cached.iter_mut().filter(|a| wanted(a.id)) {
                        account.usage_error = Some(error.clone());
                    }
                })
                .await?;
                return Err(error);
            }
        };
        let complete = fetched.errors.is_empty();
        let count = self
            .apply_provider_refresh(pool, &member.key, fetched.accounts, ids, complete, sync)
            .await?;
        if complete {
            Ok(count)
        } else {
            Err(fetched.errors.join("\n"))
        }
    }
    /// Stores a refresh's accounts in the provider as it is now: a removal
    /// or a proxy set while the request ran stands.
    pub(crate) async fn apply_provider_refresh(
        &self,
        pool: Pool,
        key: &str,
        fetched: Vec<Account>,
        ids: Option<&[i64]>,
        complete: bool,
        sync: bool,
    ) -> Result<usize> {
        Ok(self
            .edit_member(pool, key, |m| m.adopt(pool, fetched, ids, complete, sync))
            .await?
            .unwrap_or(0))
    }
    pub(crate) async fn codex_save_provider(&self, draft: Provider) -> Result<Provider> {
        let _op = self.operations.lock().await;
        let mut saved = self.configuration()?;
        let mut draft = draft.normalized();
        let providers = &saved.sub2_api_provider.providers;
        let existing = providers.iter().find(|p| p.id == draft.id);
        sub2api::validate_form(
            &draft.provider_id,
            providers.iter().any(|p| {
                p.id != draft.id && p.provider_id.eq_ignore_ascii_case(&draft.provider_id)
            }),
            &draft.username,
            &draft.password,
            existing.is_some(),
            &draft.proxy_url,
            &[&draft.admin_base_url],
        )?;
        if let Some(old) = existing {
            draft.imported_account_ids = old.imported_account_ids.clone();
            draft.cached_accounts = old.cached_accounts.clone();
            // The provider's card sets its proxy, not its settings.
            draft.proxy_url = old.proxy_url.clone();
        }
        self.store_provider_password(Pool::Codex, &draft.id, &draft.password)?;
        draft.password.clear();
        draft.legacy_admin_base_url.clear();
        saved.sub2_api_provider.upsert(draft.clone());
        self.persist_configuration(&saved)?;
        Ok(draft)
    }
    async fn codex_remove_provider(&self, id: &str) -> Result<()> {
        let _op = self.operations.lock().await;
        let mut saved = self.configuration()?;
        saved.sub2_api_provider.providers.retain(|p| p.id != id);
        self.persist_configuration(&saved)
    }
    async fn codex_edit_member<T>(
        &self,
        key: &str,
        edit: impl FnOnce(&mut Member) -> T,
    ) -> Result<Option<T>> {
        let _op = self.operations.lock().await;
        let mut settings = self.configuration()?;
        let Some(p) = settings
            .sub2_api_provider
            .providers
            .iter_mut()
            .find(|p| p.provider_id.eq_ignore_ascii_case(key))
        else {
            return Ok(None);
        };
        let mut member = codex_member(p);
        let result = edit(&mut member);
        p.imported_account_ids = member.imported;
        p.proxy_url = member.proxy_url;
        p.cached_accounts = member
            .cached
            .iter()
            .map(serde_json::to_value)
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let key = format!("providerSyncNeeded:{}", p.provider_id);
        settings.extra.remove(&key);
        self.persist_configuration(&settings)?;
        Ok(Some(result))
    }
}
/// Where a pool's tunnel configuration is kept in the proxy configuration.
pub fn tunnel_configuration_key(pool: Pool) -> &'static str {
    match pool {
        Pool::Codex => "cloudflared",
        Pool::Claude => "claudeCloudflared",
    }
}
fn cloudflare_token_secret(pool: Pool) -> &'static str {
    match pool {
        Pool::Codex => "cloudflare-api-token",
        Pool::Claude => "claude-cloudflare-api-token",
    }
}
/// A Codex provider's cards, named by the Codex provider it is.
fn codex_member(p: &Provider) -> Member {
    Member {
        key: p.provider_id.clone(),
        name: p.provider_id.clone(),
        proxy_url: p.proxy_url.clone(),
        imported: p.imported_account_ids.clone(),
        cached: p
            .cached_accounts
            .iter()
            .filter_map(|raw| serde_json::from_value(raw.clone()).ok())
            .collect(),
    }
}
