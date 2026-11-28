//! The Codex pool's Sub2api providers: kept in its account store, each
//! named after the `config.toml` provider whose gateway it serves, which a
//! switch to its cards selects; and their move there from the settings.
use super::config;
use crate::{
    application::Application,
    pool::Pool,
    provider_registry::{upstream, Adapter, ProviderRecord, Route},
    sub2api,
};
use serde_json::Value;

type Result<T> = std::result::Result<T, String>;

impl Application {
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
            let key = crate::provider_accounts::password_secret(Pool::Codex, &p.id);
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
}
/// Set in the settings once their Codex providers were copied to the account
/// store.
pub(crate) const CODEX_PROVIDERS_MIGRATED: &str = "codexProvidersMigrated";
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
pub(crate) struct ProviderAdapter<'a>(pub(crate) &'a Application);
impl Adapter for ProviderAdapter<'_> {
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
