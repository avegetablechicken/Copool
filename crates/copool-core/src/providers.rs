//! Sub2api providers in every pool: their settings forms, their cards and
//! how both are refreshed, implemented once. Each pool only keeps its
//! providers in its own place; the Codex pool's, here, in its account store,
//! each named after the `config.toml` provider whose gateway it serves.
use crate::{
    application::{Application, SAY_HI_REQUIRES_PLUS},
    codex::config,
    models::AccountSummary,
    pool::Pool,
    provider_registry::{upstream, Adapter, ProviderRecord, Route},
    repository,
    settings::Settings,
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
    /// Marks whether a named tunnel saved before the mark existed keeps its
    /// token.
    pub(crate) fn mark_kept_tunnel_token(&self) -> Result<()> {
        let mut settings = self.configuration()?;
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
    pub fn provider_definitions(&self) -> Vec<config::Definition> {
        config::definitions(&self.codex_config_path())
    }
    /// The `config.toml` provider a Codex provider is named after; never the
    /// official one.
    fn codex_definition(&self, name: &str) -> Result<config::Definition> {
        if name.eq_ignore_ascii_case("openai") {
            return Err(sub2api::PROVIDER_NOT_FOUND.into());
        }
        self.provider_definitions()
            .into_iter()
            .find(|d| d.id.eq_ignore_ascii_case(name))
            .ok_or_else(|| sub2api::PROVIDER_NOT_FOUND.into())
    }
    /// The id of the Codex provider Codex uses: the one named after the
    /// provider `config.toml` selects, unless that is the official one.
    pub(crate) fn codex_provider_in_use(&self) -> Result<Option<String>> {
        let selected = config::resolve_path(&self.codex_config_path());
        if selected.is_official() {
            return Ok(None);
        }
        Ok(codex_providers(&self.paths)?
            .into_iter()
            .find(|p| p.name.eq_ignore_ascii_case(&selected.id))
            .map(|p| p.id))
    }
    /// Copies the Codex providers the settings kept (`sub2APIProvider`, their
    /// accounts in the account store's `cachedAccounts`) to the account
    /// store's `providers`, keyed by id like every pool's, once: their
    /// passwords to the keyring names of their ids, and the cards' refresh
    /// switches and place in the order to the new card keys. The old data
    /// stays as it was, for an earlier version to read
    /// (docs/adr/0001-keep-codex-provider-data-after-migration.md). Providers
    /// already in the account store, from a run that stopped before setting
    /// the mark, stay as they are.
    pub(crate) fn migrate_codex_providers(&self) -> Result<()> {
        let mut settings = self.configuration()?;
        if settings.extra.contains_key(CODEX_PROVIDERS_MIGRATED) {
            return Ok(());
        }
        let legacy = settings.sub2_api_provider.providers.clone();
        if !legacy.is_empty() {
            let records: Vec<ProviderRecord> = legacy.iter().map(migrated_codex_provider).collect();
            let mut store = crate::codex::load(&self.paths)?;
            if store.providers.is_empty() {
                store.providers = records;
                crate::persistence::write_json(
                    &self.paths.data_dir.join(crate::codex::FILE),
                    &store,
                )?;
            }
        }
        let copied = legacy.iter().try_for_each(|p| {
            let key = sub2api::password_secret(Pool::Codex, &p.id);
            if self.secrets.get(&key)?.is_none() {
                if let Some(password) = self.secrets.get(&p.id)? {
                    self.secrets.set(&key, &password)?;
                }
            }
            Ok::<_, String>(())
        });
        if let Err(error) = copied {
            // Copool still opens; the next start copies them.
            log::warn!("The Codex provider passwords were not copied: {error}");
            return Ok(());
        }
        let renamed = |key: &str| {
            let provider = key.strip_prefix("sub2api:")?;
            legacy
                .iter()
                .find(|p| p.provider_id.eq_ignore_ascii_case(provider))
                .map(|p| format!("sub2api:{}", p.id))
        };
        if let Some(cards) = settings
            .extra
            .get_mut("refresh")
            .and_then(|v| v.get_mut("cards"))
            .and_then(|v| v.get_mut("codex"))
            .and_then(Value::as_object_mut)
        {
            *cards = std::mem::take(cards)
                .into_iter()
                .map(|(key, value)| (renamed(&key).unwrap_or(key), value))
                .collect();
        }
        if let Some(order) = settings
            .extra
            .get_mut("accountOrder")
            .and_then(|v| v.get_mut("codex"))
            .and_then(Value::as_array_mut)
        {
            for entry in order {
                if let Some(key) = entry.as_str().and_then(renamed) {
                    *entry = key.into();
                }
            }
        }
        settings
            .extra
            .insert(CODEX_PROVIDERS_MIGRATED.into(), true.into());
        self.persist_configuration(&settings)
    }
    /// A pool's Sub2api providers for the settings, without their secrets.
    pub fn providers(&self, pool: Pool) -> Result<Value> {
        self.registry(pool).providers()
    }
    /// Saves a provider form; a blank password keeps the saved one.
    pub async fn save_provider(&self, pool: Pool, draft: Value) -> Result<Value> {
        self.registry(pool).save(draft).await
    }
    pub async fn remove_provider(&self, pool: Pool, id: &str) -> Result<()> {
        self.registry(pool).remove(id).await
    }
    /// Keeps a form's password in the keyring, never in the settings; a
    /// blank one keeps the saved one, which is never read while editing.
    pub(crate) async fn store_provider_password(
        &self,
        pool: Pool,
        id: &str,
        password: String,
    ) -> Result<()> {
        if password.is_empty() {
            return Ok(());
        }
        let secrets = self.secrets.clone();
        let key = sub2api::password_secret(pool, id);
        tokio::task::spawn_blocking(move || secrets.set(&key, &password))
            .await
            .map_err(|e| e.to_string())?
    }
    fn members(&self, pool: Pool) -> Result<Vec<Member>> {
        self.registry(pool).members()
    }
    fn member(&self, pool: Pool, key: &str) -> Result<Member> {
        self.members(pool)?
            .into_iter()
            .find(|m| m.key.eq_ignore_ascii_case(key))
            .ok_or_else(|| crate::provider_registry::PROVIDER_NOT_FOUND.into())
    }
    /// The provider's connection as last saved: a save under way, which
    /// keeps its password after its record, is waited for.
    async fn member_connection(&self, pool: Pool, member: &Member) -> Result<sub2api::Connection> {
        let _saved = self.provider_saves.read().await;
        self.registry(pool).connection(&member.key)
    }
    /// Edits a provider's cards where its pool keeps them; none when it was
    /// removed.
    async fn edit_member<T: Send>(
        &self,
        pool: Pool,
        key: &str,
        edit: impl FnOnce(&mut Member) -> T + Send,
    ) -> Result<Option<T>> {
        let (mut edit, mut result) = (Some(edit), None);
        self.registry(pool)
            .edit_member(key, &mut |member| {
                if let Some(edit) = edit.take() {
                    result = Some(edit(member));
                }
            })
            .await?;
        Ok(result)
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
            // Sub2api's test would start only the third-party window; an
            // Antigravity card's hi starts the Gemini one too.
            Pool::Antigravity => return self.antigravity_unavailable(),
            Pool::Grok => return self.grok_unavailable(),
        };
        let (member, account) = self.card(pool, id)?;
        self.member_connection(pool, &member)
            .await?
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
            let connection = self.member_connection(pool, &member).await?;
            let now = now_unix_seconds();
            // A sync lists every account. A failure leaves the cards as they
            // are now, with its error.
            let asked = if sync { None } else { ids };
            connection.fetch_accounts(pool, asked, now).await
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
        self.apply_provider_refresh(pool, &member.key, fetched, ids, sync)
            .await
    }
    /// Stores a refresh's accounts in the provider as it is now: a removal
    /// or a proxy set while the request ran stands.
    pub(crate) async fn apply_provider_refresh(
        &self,
        pool: Pool,
        key: &str,
        fetched: Vec<Account>,
        ids: Option<&[i64]>,
        sync: bool,
    ) -> Result<usize> {
        Ok(self
            .edit_member(pool, key, |m| m.adopt(fetched, ids, sync))
            .await?
            .unwrap_or(0))
    }
}
/// Set in the settings once their Codex providers were copied to the account
/// store.
const CODEX_PROVIDERS_MIGRATED: &str = "codexProvidersMigrated";
/// The Codex pool's providers, as its account store keeps them.
pub(crate) fn codex_providers(paths: &crate::Paths) -> Result<Vec<ProviderRecord>> {
    Ok(crate::codex::load(paths)?.providers)
}
/// A Codex provider the settings kept, as the account store keeps it: named
/// after its `config.toml` provider, with its cached accounts.
fn migrated_codex_provider(p: &crate::settings::Provider) -> ProviderRecord {
    ProviderRecord {
        id: p.id.clone(),
        name: p.provider_id.clone(),
        admin_base_url: p.admin_base_url.clone(),
        username: p.username.clone(),
        proxy_url: p.proxy_url.clone(),
        allow_insecure_tls: p.allow_insecure_tls,
        imported_account_ids: p.imported_account_ids.clone(),
        cached_accounts: p
            .cached_accounts
            .iter()
            .filter_map(|raw| serde_json::from_value(raw.clone()).ok())
            .collect(),
        ..Default::default()
    }
}

/// The Codex pool's providers, for its Provider registry: kept in its
/// account store, each named after the `config.toml` provider whose gateway
/// it serves, which a switch to its cards selects.
pub(crate) struct CodexAdapter<'a>(pub(crate) &'a Application);
impl Adapter for CodexAdapter<'_> {
    type Provider = ProviderRecord;
    const POOL: Pool = Pool::Codex;
    fn load_providers(&self) -> Result<Vec<ProviderRecord>> {
        codex_providers(&self.0.paths)
    }
    async fn edit_providers<T: Send>(
        &self,
        edit: impl FnOnce(&mut Vec<ProviderRecord>) -> Result<T> + Send,
    ) -> Result<T> {
        self.0
            .codex_logins()
            .change(|store| edit(&mut store.providers))
            .await
    }
    fn prepare(&self, draft: &mut ProviderRecord) -> Result<()> {
        // The gateway is the one `config.toml` names, read on every use.
        draft.base_url.clear();
        Ok(())
    }
    fn route(&self, p: &ProviderRecord) -> Result<Route> {
        // Without a gateway, only an admin API override reaches Sub2api.
        let gateway = match self.0.codex_definition(&p.name)?.base_url {
            Some(gateway) => gateway,
            None if !p.admin_base_url.is_empty() => String::new(),
            None => return Err(sub2api::PROVIDER_NOT_FOUND.into()),
        };
        Ok(Route {
            sub2api: upstream(&gateway).to_owned(),
            gateway,
        })
    }
}

/// Where a pool's tunnel configuration is kept in the proxy configuration.
pub fn tunnel_configuration_key(pool: Pool) -> &'static str {
    match pool {
        Pool::Codex => "cloudflared",
        Pool::Claude => "claudeCloudflared",
        Pool::Antigravity => "antigravityCloudflared",
        Pool::Grok => "grokCloudflared",
    }
}
fn cloudflare_token_secret(pool: Pool) -> &'static str {
    match pool {
        Pool::Codex => "cloudflare-api-token",
        Pool::Claude => "claude-cloudflare-api-token",
        Pool::Antigravity => "antigravity-cloudflare-api-token",
        Pool::Grok => "grok-cloudflare-api-token",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::SecretStore;
    use serde_json::json;
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    #[derive(Default)]
    struct Secrets(
        Mutex<HashMap<String, String>>,
        /// Writes take this long, once started.
        std::sync::atomic::AtomicU64,
        /// A write has started.
        std::sync::atomic::AtomicBool,
    );
    impl SecretStore for Secrets {
        fn get(&self, id: &str) -> Result<Option<String>> {
            Ok(self.0.lock().unwrap().get(id).cloned())
        }
        fn set(&self, id: &str, value: &str) -> Result<()> {
            use std::sync::atomic::Ordering::SeqCst;
            self.2.store(true, SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(self.1.load(SeqCst)));
            self.0.lock().unwrap().insert(id.into(), value.into());
            Ok(())
        }
        fn remove(&self, id: &str) -> Result<()> {
            self.0.lock().unwrap().remove(id);
            Ok(())
        }
    }

    /// An earlier version's data directory, and the Codex configuration
    /// defining `my`.
    struct Earlier {
        dir: tempfile::TempDir,
        secrets: Arc<Secrets>,
    }
    impl Earlier {
        fn new(settings: Value, accounts: Value) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let data = dir.path().join("data");
            std::fs::create_dir_all(&data).unwrap();
            let mut full = serde_json::to_value(Settings::default()).unwrap();
            for (k, v) in settings.as_object().unwrap() {
                full[k] = v.clone();
            }
            std::fs::write(data.join("settings.json"), full.to_string()).unwrap();
            std::fs::write(data.join("accounts.json"), accounts.to_string()).unwrap();
            std::fs::create_dir_all(dir.path().join("codex")).unwrap();
            std::fs::write(
                dir.path().join("codex/config.toml"),
                "model_provider = \"my\"\n[model_providers.my]\nbase_url = \"https://sub2.test/v1\"\n",
            )
            .unwrap();
            Self {
                dir,
                secrets: Default::default(),
            }
        }
        fn paths(&self) -> crate::Paths {
            crate::Paths {
                data_dir: self.dir.path().join("data"),
                codex_auth: self.dir.path().join("codex/auth.json"),
            }
        }
        fn open(&self) -> Application {
            Application::open_with_secrets(self.paths(), self.secrets.clone()).unwrap()
        }
        fn file(&self, name: &str) -> Value {
            serde_json::from_slice(&std::fs::read(self.paths().data_dir.join(name)).unwrap())
                .unwrap()
        }
        fn secret(&self, id: &str) -> Option<String> {
            self.secrets.0.lock().unwrap().get(id).cloned()
        }
        /// What an earlier version reads of its providers: the settings'
        /// providers, with the account store's caches.
        fn earlier_view(&self) -> (crate::settings::Providers, Value) {
            let settings =
                repository::load_settings(&self.paths().data_dir.join("settings.json")).unwrap();
            (
                settings.sub2_api_provider,
                self.file("accounts.json")["cachedAccounts"].clone(),
            )
        }
    }
    fn cached(id: i64) -> Value {
        json!({"id": id, "name": format!("a{id}"), "email": format!("a{id}@example.test"),
            "accountType": "oauth", "status": "active", "planType": "pro", "providerID": "my",
            "usage": null, "usageError": null})
    }
    const ID: &str = "5A4B3C2D-UPPER-CASE-ID";

    #[tokio::test]
    async fn codex_providers_move_to_the_account_store_keyed_by_id() {
        let earlier = Earlier::new(
            json!({
                "sub2APIProvider": {"confirmedProviderIDs": ["my"], "providers": [{
                    "id": ID, "providerID": "my", "username": "admin@example.test",
                    "adminBaseURL": "https://admin.test/api/v1", "proxyURL": "direct://",
                    "allowInsecureTLS": true, "importedAccountIDs": [42, 43]}]},
                "refresh": {"cards": {"codex": {"sub2api:my": {"adaptive": true}, "local": {"adaptive": false}},
                    "claude": {"sub2api:my": {"adaptive": true}}}},
                "accountOrder": {"codex": ["local", "sub2api:MY"], "claude": ["sub2api:my"]},
            }),
            // Account 43's cache cannot be read: it is left behind.
            json!({"accounts": [], "cachedAccounts": {ID: [cached(42), {"id": 43, "name": 5}]}}),
        );
        earlier.secrets.set(ID, "secret").unwrap();
        let before = earlier.earlier_view();
        let app = earlier.open();
        let providers = earlier.file(crate::codex::FILE)["providers"].clone();
        assert_eq!(
            providers,
            json!([{"id": ID, "name": "my", "baseURL": "", "adminBaseURL": "https://admin.test/api/v1",
                "username": "admin@example.test", "proxyURL": "direct://", "allowInsecureTLS": true,
                "importedAccountIDs": [42, 43],
                "cachedAccounts": [serde_json::to_value(
                    serde_json::from_value::<Account>(cached(42)).unwrap()).unwrap()]}])
        );
        // The password under the id's new name; the old one stays.
        assert_eq!(
            earlier
                .secret(&sub2api::password_secret(Pool::Codex, ID))
                .as_deref(),
            Some("secret")
        );
        assert_eq!(earlier.secret(ID).as_deref(), Some("secret"));
        // The cards' settings follow them to their new key, in their pool only.
        let settings = earlier.file("settings.json");
        assert_eq!(
            settings["refresh"]["cards"]["codex"],
            json!({format!("sub2api:{ID}"): {"adaptive": true}, "local": {"adaptive": false}})
        );
        assert_eq!(
            settings["refresh"]["cards"]["claude"],
            json!({"sub2api:my": {"adaptive": true}})
        );
        assert_eq!(
            settings["accountOrder"]["codex"],
            json!(["local", format!("sub2api:{ID}")])
        );
        assert_eq!(settings["accountOrder"]["claude"], json!(["sub2api:my"]));
        assert_eq!(settings["codexProvidersMigrated"], true);
        // An earlier version still finds its providers as they were.
        assert_eq!(earlier.earlier_view(), before);
        assert_eq!(
            settings["sub2APIProvider"]["confirmedProviderIDs"],
            json!(["my"])
        );
        // The cards are the provider's, current while Codex uses it.
        let cards = app.accounts(Pool::Codex).await.unwrap();
        assert_eq!(
            cards
                .iter()
                .map(|c| (c.id.as_str(), c.is_current))
                .collect::<Vec<_>>(),
            [(format!("sub2api-{ID}-42").as_str(), true)]
        );
        assert_eq!(
            app.registry(Pool::Codex)
                .connection(ID)
                .unwrap()
                .configuration
                .password,
            "secret"
        );
    }

    #[tokio::test]
    async fn a_connection_is_never_read_half_saved() {
        use std::sync::atomic::Ordering::SeqCst;
        let earlier = Earlier::new(json!({}), json!({"accounts": []}));
        let app = earlier.open();
        let form = |admin: &str, password: &str| {
            json!({"id": ID, "name": "my", "username": "admin@example.test",
                "adminBaseURL": admin, "password": password})
        };
        app.save_provider(Pool::Codex, form("https://old.test/api/v1", "old"))
            .await
            .unwrap();
        earlier.secrets.1.store(300, SeqCst);
        earlier.secrets.2.store(false, SeqCst);
        let member = app.member(Pool::Codex, ID).unwrap();
        let (saved, read) = tokio::join!(
            app.save_provider(Pool::Codex, form("https://new.test/api/v1", "new")),
            async {
                // Read while the new password is being kept.
                while !earlier.secrets.2.load(SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                app.member_connection(Pool::Codex, &member).await.unwrap()
            }
        );
        saved.unwrap();
        assert!(read.admin_base.contains("new.test"), "{}", read.admin_base);
        assert_eq!(read.configuration.password, "new");
    }

    #[tokio::test]
    async fn the_migration_runs_once_and_keeps_what_an_earlier_run_stored() {
        let earlier = Earlier::new(
            json!({"sub2APIProvider": {"providers": [{"id": ID, "providerID": "my",
                "username": "admin@example.test", "importedAccountIDs": [42]}]},
                "accountOrder": {"codex": ["sub2api:my"]}}),
            json!({"accounts": [], "cachedAccounts": {ID: [cached(42)]}}),
        );
        earlier.secrets.set(ID, "secret").unwrap();
        let app = earlier.open();
        // Edited since, in this version.
        app.save_provider(
            Pool::Codex,
            json!({"id": ID, "name": "renamed", "username": "admin@example.test",
                "password": "new"}),
        )
        .await
        .unwrap();
        drop(app);
        let stored = earlier.file(crate::codex::FILE)["providers"].clone();
        let settings = earlier.file("settings.json");
        drop(earlier.open());
        assert_eq!(earlier.file(crate::codex::FILE)["providers"], stored);
        assert_eq!(earlier.file("settings.json"), settings);
        assert_eq!(
            earlier
                .secret(&sub2api::password_secret(Pool::Codex, ID))
                .as_deref(),
            Some("new")
        );
        // A run stopped before its mark: the providers stored stay, and a
        // password already under its new name is kept.
        let mut unmarked = settings.clone();
        unmarked
            .as_object_mut()
            .unwrap()
            .remove(CODEX_PROVIDERS_MIGRATED);
        std::fs::write(
            earlier.paths().data_dir.join("settings.json"),
            unmarked.to_string(),
        )
        .unwrap();
        drop(earlier.open());
        assert_eq!(earlier.file(crate::codex::FILE)["providers"], stored);
        assert_eq!(
            earlier.file("settings.json")["codexProvidersMigrated"],
            true
        );
        assert_eq!(
            earlier.file("settings.json")["accountOrder"]["codex"],
            json!([format!("sub2api:{ID}")])
        );
        assert_eq!(
            earlier
                .secret(&sub2api::password_secret(Pool::Codex, ID))
                .as_deref(),
            Some("new")
        );
    }

    #[test]
    fn a_new_data_directory_is_only_marked() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::Paths {
            data_dir: dir.path().join("data"),
            codex_auth: dir.path().join("codex/auth.json"),
        };
        let _app =
            Application::open_with_secrets(paths.clone(), Arc::new(Secrets::default())).unwrap();
        let settings: Value =
            serde_json::from_slice(&std::fs::read(paths.data_dir.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(settings["codexProvidersMigrated"], true);
        assert!(codex_providers(&paths).unwrap().is_empty());
    }
}
