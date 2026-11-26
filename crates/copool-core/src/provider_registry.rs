//! The Provider registry: the Sub2api providers Copool keeps for a pool,
//! with each one's saved form, password and accounts.
//!
//! What every pool does alike with its providers is here: a form's
//! normalization and checks, its password in the keyring, the accounts and
//! proxy a provider's cards own, its cards as a `Member`, and the
//! `Connection` Copool reaches Sub2api through. A pool's [`Adapter`] says
//! only where its providers are kept, how a form becomes one of its
//! providers, and where its tool's gateway is. What a switch writes into the
//! pool's tool stays with the pool.
use crate::{
    application::Application,
    codex_config::Definition,
    pool::Pool,
    sub2api::{self, Account, Member},
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, pin::Pin};

type Result<T> = std::result::Result<T, String>;
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What every pool keeps of a Sub2api provider; a pool's own provider
/// embeds it (`#[serde(flatten)]`) beside what only that pool needs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProviderRecord {
    pub id: String,
    pub name: String,
    /// The Sub2api address, as the pool's form normalizes it.
    #[serde(rename = "baseURL")]
    pub base_url: String,
    /// Optional admin API override, independent of the tool's gateway.
    #[serde(rename = "adminBaseURL")]
    pub admin_base_url: String,
    pub username: String,
    /// Only in a draft from the form; the system keyring keeps it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub password: String,
    /// The proxy of every request for the provider and its accounts: blank
    /// for the system settings, "direct://" for none.
    #[serde(rename = "proxyURL")]
    pub proxy_url: String,
    #[serde(rename = "allowInsecureTLS")]
    pub allow_insecure_tls: bool,
    #[serde(rename = "importedAccountIDs")]
    pub imported_account_ids: Vec<i64>,
    pub cached_accounts: Vec<Account>,
}
impl AsRef<ProviderRecord> for ProviderRecord {
    fn as_ref(&self) -> &ProviderRecord {
        self
    }
}
impl AsMut<ProviderRecord> for ProviderRecord {
    fn as_mut(&mut self) -> &mut ProviderRecord {
        self
    }
}
impl ProviderRecord {
    /// The provider's cards, named by its id.
    fn member(&self) -> Member {
        Member {
            key: self.id.clone(),
            name: self.name.clone(),
            proxy_url: self.proxy_url.clone(),
            imported: self.imported_account_ids.clone(),
            cached: self.cached_accounts.clone(),
        }
    }
}

/// Where a provider's tool sends its requests, and the Sub2api address in
/// it: the one a form's URL checks include, and where Copool reaches the
/// admin API unless the provider overrides it.
pub(crate) struct Route {
    pub gateway: String,
    pub sub2api: String,
}

/// A pool's providers, as the registry asks for them.
pub(crate) trait Adapter: Send + Sync {
    /// The pool's provider: a [`ProviderRecord`] and what only it needs.
    type Provider: Serialize
        + DeserializeOwned
        + Clone
        + Send
        + Sync
        + AsRef<ProviderRecord>
        + AsMut<ProviderRecord>;
    const POOL: Pool;
    /// The providers as stored.
    fn load_providers(&self) -> Result<Vec<Self::Provider>>;
    /// Edits the providers under the pool's lock, then saves and publishes
    /// them; nothing is saved when the edit fails.
    fn edit_providers<T: Send>(
        &self,
        edit: impl FnOnce(&mut Vec<Self::Provider>) -> Result<T> + Send,
    ) -> impl Future<Output = Result<T>> + Send;
    /// Makes a form, its shared fields trimmed, the provider it stores: its
    /// address normalized and present, the pool's own fields decided, and
    /// anything only a form carries cleared. Runs outside the lock; it may
    /// read the tool's files, and may rename the provider.
    fn prepare(&self, draft: &mut Self::Provider) -> Result<()>;
    /// Where the provider's tool sends its requests. A save checks the
    /// Sub2api address in it when there is one; a route the tool cannot give
    /// yet (a Codex provider its `config.toml` does not define, say) is no
    /// reason to refuse the form, and fails the provider's connections
    /// instead.
    fn route(&self, provider: &Self::Provider) -> Result<Route>;
    /// Fills what a listing shows of a stored provider beyond what it keeps.
    fn present(&self, _provider: &mut Self::Provider) {}
}

/// A pool's Provider registry, as `providers.rs` calls it.
pub(crate) trait ProviderRegistry: Send + Sync {
    /// The providers for the settings, without their secrets.
    fn providers(&self) -> Result<Value>;
    /// Saves a form; a blank password keeps the saved one. Its addresses are
    /// checked, the one in its route only while the tool gives a route (see
    /// [`Adapter::route`]). Returns the provider as a listing shows it.
    fn save(&self, draft: Value) -> BoxFuture<'_, Result<Value>>;
    /// Removes a provider and its password; none to remove is no error.
    fn remove<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<()>>;
    /// Every provider's cards.
    fn members(&self) -> Result<Vec<Member>>;
    /// Edits the cards of the provider the key names, writing back their
    /// accounts and proxy; false when there is no such provider.
    fn edit_member<'a>(
        &'a self,
        key: &'a str,
        edit: &'a mut (dyn FnMut(&mut Member) + Send),
    ) -> BoxFuture<'a, Result<bool>>;
    /// Where Copool reaches the provider's Sub2api, with its password.
    fn connection(&self, key: &str) -> Result<sub2api::Connection>;
}

/// Not found among a pool's providers.
pub(crate) const PROVIDER_NOT_FOUND: &str = "Provider not found";

struct Registry<'a, A> {
    app: &'a Application,
    adapter: A,
}

/// The registry over a pool's adapter.
pub(crate) fn registry<'a, A: Adapter + 'a>(
    app: &'a Application,
    adapter: A,
) -> Box<dyn ProviderRegistry + 'a> {
    Box::new(Registry { app, adapter })
}

impl<A: Adapter> Registry<'_, A> {
    fn find(&self, key: &str) -> Result<A::Provider> {
        self.adapter
            .load_providers()?
            .into_iter()
            .find(|p| p.as_ref().id.eq_ignore_ascii_case(key))
            .ok_or_else(|| PROVIDER_NOT_FOUND.into())
    }
    async fn save_typed(&self, draft: Value) -> Result<Value> {
        let invalid = |e: serde_json::Error| e.to_string();
        let mut draft: A::Provider = serde_json::from_value(draft).map_err(invalid)?;
        {
            let r = draft.as_mut();
            r.name = r.name.trim().into();
            r.username = r.username.trim().into();
            r.admin_base_url = r.admin_base_url.trim().trim_end_matches('/').into();
            r.proxy_url = r.proxy_url.trim().into();
        }
        self.adapter.prepare(&mut draft)?;
        if draft.as_ref().id.trim().is_empty() {
            draft.as_mut().id = uuid::Uuid::new_v4().to_string();
        }
        // A provider the tool does not route yet is saved all the same.
        let routed = self
            .adapter
            .route(&draft)
            .map(|route| route.sub2api)
            .unwrap_or_default();
        let stored = self
            .adapter
            .edit_providers(|providers| {
                let r = draft.as_ref();
                let existing = providers.iter().position(|p| p.as_ref().id == r.id);
                sub2api::validate_form(
                    &r.name,
                    providers.iter().any(|p| {
                        p.as_ref().id != r.id && p.as_ref().name.eq_ignore_ascii_case(&r.name)
                    }),
                    &r.username,
                    &r.password,
                    existing.is_some(),
                    &r.proxy_url,
                    &[&r.base_url, &routed, &r.admin_base_url],
                )?;
                let mut stored = draft.clone();
                let s = stored.as_mut();
                if let Some(old) = existing.map(|i| providers[i].as_ref()) {
                    s.imported_account_ids = old.imported_account_ids.clone();
                    s.cached_accounts = old.cached_accounts.clone();
                    // The provider's card sets its proxy, not its settings.
                    s.proxy_url = old.proxy_url.clone();
                }
                self.app
                    .store_provider_password(A::POOL, &s.id, &s.password)?;
                s.password.clear();
                match existing {
                    Some(i) => providers[i] = stored.clone(),
                    None => providers.push(stored.clone()),
                }
                Ok(stored)
            })
            .await?;
        let mut shown = stored;
        self.adapter.present(&mut shown);
        serde_json::to_value(shown).map_err(invalid)
    }
}

impl<A: Adapter> ProviderRegistry for Registry<'_, A> {
    fn providers(&self) -> Result<Value> {
        let mut providers = self.adapter.load_providers()?;
        for p in &mut providers {
            self.adapter.present(p);
        }
        serde_json::to_value(providers).map_err(|e| e.to_string())
    }
    fn save(&self, draft: Value) -> BoxFuture<'_, Result<Value>> {
        Box::pin(self.save_typed(draft))
    }
    fn remove<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.adapter
                .edit_providers(|providers| {
                    providers.retain(|p| p.as_ref().id != id);
                    Ok(())
                })
                .await?;
            self.app
                .secrets
                .remove(&sub2api::password_secret(A::POOL, id))
        })
    }
    fn members(&self) -> Result<Vec<Member>> {
        Ok(self
            .adapter
            .load_providers()?
            .iter()
            .map(|p| p.as_ref().member())
            .collect())
    }
    fn edit_member<'a>(
        &'a self,
        key: &'a str,
        edit: &'a mut (dyn FnMut(&mut Member) + Send),
    ) -> BoxFuture<'a, Result<bool>> {
        Box::pin(self.adapter.edit_providers(move |providers| {
            let Some(p) = providers
                .iter_mut()
                .find(|p| p.as_ref().id.eq_ignore_ascii_case(key))
            else {
                return Ok(false);
            };
            let r = p.as_mut();
            let mut member = r.member();
            edit(&mut member);
            r.imported_account_ids = member.imported;
            r.cached_accounts = member.cached;
            r.proxy_url = member.proxy_url;
            Ok(true)
        }))
    }
    fn connection(&self, key: &str) -> Result<sub2api::Connection> {
        let p = self.find(key)?;
        let route = self.adapter.route(&p)?;
        let r = p.as_ref();
        let password = self
            .app
            .secrets
            .get(&sub2api::password_secret(A::POOL, &r.id))?
            .ok_or("error.sub2api.configuration_incomplete")?;
        // Sub2api itself, not a proxy in front of the tool's requests.
        let admin = if r.admin_base_url.is_empty() {
            &route.sub2api
        } else {
            &r.admin_base_url
        };
        Ok(sub2api::Connection {
            admin_base: sub2api::admin_base(admin)?,
            definition: Definition {
                id: r.name.clone(),
                base_url: Some(route.gateway),
                wire_api: None,
                env_key: None,
                requires_openai_auth: None,
            },
            configuration: crate::settings::Provider {
                id: r.id.clone(),
                provider_id: r.name.clone(),
                username: r.username.clone(),
                password,
                allow_insecure_tls: r.allow_insecure_tls,
                proxy_url: r.proxy_url.clone(),
                imported_account_ids: r.imported_account_ids.clone(),
                cached_accounts: r
                    .cached_accounts
                    .iter()
                    .map(serde_json::to_value)
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?,
                ..Default::default()
            },
        })
    }
}

/// The Sub2api address in an address: the upstream a proxy's path names
/// (`<proxy>/https://relay`, `<proxy>/antigravity/https://relay`), else the
/// address itself.
pub(crate) fn upstream(url: &str) -> &str {
    ["/https://", "/http://"]
        .iter()
        .filter_map(|scheme| url.find(scheme))
        .min()
        .map_or(url, |at| &url[at + 1..])
}
/// An address without surrounding space, trailing slashes and the first of
/// these suffixes it ends with.
pub(crate) fn strip_suffix(url: &str, suffixes: &[&str]) -> String {
    let url = url.trim().trim_end_matches('/');
    suffixes
        .iter()
        .find_map(|suffix| url.strip_suffix(suffix))
        .unwrap_or(url)
        .trim_end_matches('/')
        .to_owned()
}

/// Gives a pool's provider, a struct whose `record` field holds its
/// [`ProviderRecord`], the record's fields as its own.
macro_rules! embeds_record {
    ($provider:ty) => {
        impl std::ops::Deref for $provider {
            type Target = $crate::provider_registry::ProviderRecord;
            fn deref(&self) -> &Self::Target {
                &self.record
            }
        }
        impl std::ops::DerefMut for $provider {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.record
            }
        }
        impl AsRef<$crate::provider_registry::ProviderRecord> for $provider {
            fn as_ref(&self) -> &$crate::provider_registry::ProviderRecord {
                &self.record
            }
        }
        impl AsMut<$crate::provider_registry::ProviderRecord> for $provider {
            fn as_mut(&mut self) -> &mut $crate::provider_registry::ProviderRecord {
                &mut self.record
            }
        }
    };
}
pub(crate) use embeds_record;

impl Application {
    /// A pool's Provider registry.
    pub(crate) fn registry(&self, pool: Pool) -> Box<dyn ProviderRegistry + '_> {
        match pool {
            Pool::Codex => registry(self, crate::providers::CodexAdapter(self)),
            Pool::Claude => registry(self, crate::claude::ProviderAdapter(self)),
            Pool::Antigravity => registry(self, crate::antigravity::ProviderAdapter(self)),
            Pool::Grok => registry(self, crate::grok::ProviderAdapter(self)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{collections::HashMap, sync::Arc, sync::Mutex};

    #[derive(Default)]
    struct Secrets(Mutex<HashMap<String, String>>);
    impl crate::credentials::SecretStore for Secrets {
        fn get(&self, id: &str) -> Result<Option<String>> {
            Ok(self.0.lock().unwrap().get(id).cloned())
        }
        fn set(&self, id: &str, value: &str) -> Result<()> {
            self.0.lock().unwrap().insert(id.into(), value.into());
            Ok(())
        }
        fn remove(&self, id: &str) -> Result<()> {
            self.0.lock().unwrap().remove(id);
            Ok(())
        }
    }

    /// One pool's provider form: what is typed, and what is stored and
    /// listed for it, beyond the fields every pool shares.
    struct Case {
        pool: Pool,
        typed_url: &'static str,
        own: Value,
        listed_own: Value,
        gateway: &'static str,
    }
    const ROOT: &str = "https://relay.example.com";
    const PROXIED: &str = "http://127.0.0.1:8787/x/https://relay.example.com";
    fn cases() -> Vec<Case> {
        vec![
            Case {
                pool: Pool::Claude,
                typed_url: "https://relay.example.com/v1/",
                own: json!({"mode": "address", "apiKeyVariable": "RELAY_KEY"}),
                listed_own: json!({"mode": "address", "apiKeyVariable": "RELAY_KEY",
                    "settingsPath": "", "hasApiKey": true}),
                gateway: ROOT,
            },
            Case {
                pool: Pool::Grok,
                typed_url: "https://relay.example.com/v1/",
                own: json!({"apiKeyVariable": "RELAY_KEY"}),
                listed_own: json!({"apiKeyVariable": "RELAY_KEY"}),
                gateway: "https://relay.example.com/v1",
            },
            Case {
                pool: Pool::Antigravity,
                typed_url: "https://relay.example.com/antigravity/",
                own: json!({"apiKeyVariable": ""}),
                listed_own: json!({"apiKeyVariable": "GEMINI_API_KEY"}),
                gateway: "https://relay.example.com/antigravity",
            },
            // Codex's gateway is the one `config.toml` names (`gateway_at`),
            // never a typed address.
            Case {
                pool: Pool::Codex,
                typed_url: "https://typed.example.com/v1/",
                own: json!({}),
                listed_own: json!({"baseURL": ""}),
                gateway: ROOT,
            },
        ]
    }
    /// Has Codex's `config.toml` define the provider Relay with this gateway.
    fn gateway_at(dir: &std::path::Path, url: &str) {
        std::fs::create_dir_all(dir.join("codex")).unwrap();
        std::fs::write(
            dir.join("codex/config.toml"),
            format!("[model_providers.Relay]\nbase_url = {url:?}\nwire_api = \"responses\"\n"),
        )
        .unwrap();
    }
    fn open(dir: &std::path::Path) -> (Application, Arc<Secrets>) {
        let secrets = Arc::new(Secrets::default());
        let app = Application::open_with_secrets(
            crate::Paths {
                data_dir: dir.join("data"),
                codex_auth: dir.join("codex/auth.json"),
            },
            secrets.clone(),
        )
        .unwrap();
        app.set_claude_location(crate::claude::Location::files(dir.join("claude")));
        gateway_at(dir, ROOT);
        (app, secrets)
    }
    fn form(case: &Case, url: &str, extra: Value) -> Value {
        let mut form = json!({"name": " Relay ", "baseURL": url,
            "username": " admin@example.com ", "password": "secret"});
        for (k, v) in case
            .own
            .as_object()
            .unwrap()
            .iter()
            .chain(extra.as_object().unwrap())
        {
            form[k] = v.clone();
        }
        form
    }
    fn account(id: i64) -> Account {
        serde_json::from_value(json!({"id": id, "name": format!("a{id}"), "email": null,
            "accountID": null, "accountType": "oauth", "status": "active", "planType": null,
            "usage": null, "usageError": null}))
        .unwrap()
    }

    #[tokio::test]
    async fn a_saved_form_is_listed_without_its_password_which_the_keyring_keeps() {
        for case in cases() {
            let dir = tempfile::tempdir().unwrap();
            let (app, secrets) = open(dir.path());
            let registry = app.registry(case.pool);
            let saved = registry
                .save(form(
                    &case,
                    case.typed_url,
                    json!({"adminBaseURL": " https://admin.example.com/ "}),
                ))
                .await
                .unwrap();
            let id = saved["id"].as_str().unwrap().to_owned();
            assert!(!id.is_empty());
            // Every pool keeps the same fields; the keys and values stay as
            // they were before the registry.
            let mut expected = json!({"id": id, "name": "Relay", "baseURL": ROOT,
                "adminBaseURL": "https://admin.example.com", "username": "admin@example.com",
                "proxyURL": "", "allowInsecureTLS": false, "importedAccountIDs": [],
                "cachedAccounts": []});
            for (k, v) in case.listed_own.as_object().unwrap() {
                expected[k] = v.clone();
            }
            assert_eq!(
                registry.providers().unwrap(),
                json!([expected]),
                "{:?}",
                case.pool
            );
            if case.pool == Pool::Claude {
                // The save's result shows the provider as listed.
                assert_eq!(saved, expected);
            }
            assert_eq!(
                secrets
                    .0
                    .lock()
                    .unwrap()
                    .get(&sub2api::password_secret(case.pool, &id)),
                Some(&"secret".to_owned())
            );
        }
    }

    #[tokio::test]
    async fn an_edit_keeps_the_password_accounts_and_proxy_its_cards_own() {
        for case in cases() {
            let dir = tempfile::tempdir().unwrap();
            let (app, secrets) = open(dir.path());
            let registry = app.registry(case.pool);
            let id = registry.save(form(&case, ROOT, json!({}))).await.unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let edited = registry
                .edit_member(&id.to_uppercase(), &mut |m: &mut Member| {
                    m.imported = vec![7];
                    m.cached = vec![account(7)];
                    m.proxy_url = "direct://".into();
                    m.key = "ignored".into();
                })
                .await
                .unwrap();
            assert!(edited);
            assert!(!registry.edit_member("missing", &mut |_| {}).await.unwrap());
            // A blank password keeps the saved one; the form cannot change
            // what the cards own.
            registry
                .save(form(
                    &case,
                    ROOT,
                    json!({"id": id, "name": "Renamed", "password": "",
                    "proxyURL": "http://form.invalid:1", "importedAccountIDs": [9]}),
                ))
                .await
                .unwrap();
            let members = registry.members().unwrap();
            assert_eq!(members.len(), 1);
            let m = &members[0];
            assert_eq!((m.key.as_str(), m.name.as_str()), (id.as_str(), "Renamed"));
            assert_eq!(
                (m.imported.clone(), m.proxy_url.as_str()),
                (vec![7], "direct://")
            );
            assert_eq!(m.cached.iter().map(|a| a.id).collect::<Vec<_>>(), vec![7]);
            assert_eq!(
                secrets
                    .0
                    .lock()
                    .unwrap()
                    .get(&sub2api::password_secret(case.pool, &id)),
                Some(&"secret".to_owned()),
                "{:?}",
                case.pool
            );
        }
    }

    #[tokio::test]
    async fn a_form_is_refused_whole() {
        for case in cases() {
            let dir = tempfile::tempdir().unwrap();
            let (app, _) = open(dir.path());
            let registry = app.registry(case.pool);
            registry.save(form(&case, ROOT, json!({}))).await.unwrap();
            for (extra, error) in [
                // Another provider of the same name, in any case.
                (json!({"name": "RELAY"}), "error.sub2api.duplicate_provider"),
                // A new provider needs its password.
                (
                    json!({"name": "Other", "password": ""}),
                    "error.sub2api.configuration_incomplete",
                ),
                (
                    json!({"name": "Other", "baseURL": ""}),
                    "error.sub2api.configuration_incomplete",
                ),
                (
                    json!({"name": "Other", "baseURL": "ftp://relay"}),
                    "error.sub2api.invalid_base_url",
                ),
                (
                    json!({"name": "Other", "adminBaseURL": "https://a?b"}),
                    "error.sub2api.invalid_base_url",
                ),
            ]
            .into_iter()
            // A Codex form has no address of its own.
            .filter(|(extra, _)| case.pool != Pool::Codex || extra.get("baseURL").is_none())
            {
                assert_eq!(
                    registry
                        .save(form(&case, ROOT, extra.clone()))
                        .await
                        .unwrap_err(),
                    error,
                    "{:?} {extra}",
                    case.pool
                );
            }
            assert_eq!(registry.members().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn a_connection_reaches_sub2api_itself_with_the_saved_password() {
        for case in cases() {
            let dir = tempfile::tempdir().unwrap();
            let (app, secrets) = open(dir.path());
            let registry = app.registry(case.pool);
            let id = registry.save(form(&case, ROOT, json!({}))).await.unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let c = registry.connection(&id).unwrap();
            assert_eq!(c.admin_base, "https://relay.example.com/api/v1");
            assert_eq!(c.definition.base_url.as_deref(), Some(case.gateway));
            assert_eq!(c.definition.id, "Relay");
            assert_eq!(c.definition.env_key, None);
            assert_eq!(
                (
                    c.configuration.provider_id.as_str(),
                    c.configuration.password.as_str()
                ),
                ("Relay", "secret")
            );
            // Through a proxy, the admin API is the Sub2api in its path; an
            // override wins.
            registry
                .save(form(&case, PROXIED, json!({"id": id, "password": ""})))
                .await
                .unwrap();
            gateway_at(dir.path(), PROXIED);
            let c = registry.connection(&id).unwrap();
            assert_eq!(
                c.admin_base, "https://relay.example.com/api/v1",
                "{:?}",
                case.pool
            );
            assert!(c.definition.base_url.unwrap().starts_with(PROXIED));
            registry
                .save(form(
                    &case,
                    PROXIED,
                    json!({"id": id, "password": "",
                    "adminBaseURL": "https://admin.example.com"}),
                ))
                .await
                .unwrap();
            assert_eq!(
                registry.connection(&id).unwrap().admin_base,
                "https://admin.example.com/api/v1"
            );
            assert_eq!(
                registry.connection("missing").err().unwrap(),
                PROVIDER_NOT_FOUND
            );
            secrets.0.lock().unwrap().clear();
            assert_eq!(
                registry.connection(&id).err().unwrap(),
                "error.sub2api.configuration_incomplete"
            );
        }
    }

    #[tokio::test]
    async fn removing_takes_the_provider_and_its_password() {
        for case in cases() {
            let dir = tempfile::tempdir().unwrap();
            let (app, secrets) = open(dir.path());
            let registry = app.registry(case.pool);
            let id = registry.save(form(&case, ROOT, json!({}))).await.unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            registry.remove(&id).await.unwrap();
            registry.remove("missing").await.unwrap();
            assert_eq!(registry.providers().unwrap(), json!([]));
            assert!(secrets.0.lock().unwrap().is_empty(), "{:?}", case.pool);
        }
    }

    #[tokio::test]
    async fn a_codex_provider_config_toml_does_not_define_yet_is_saved_but_not_reached() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _) = open(dir.path());
        let registry = app.registry(Pool::Codex);
        let case = cases().pop().unwrap();
        for name in ["Later", "openai"] {
            let saved = registry
                .save(form(&case, ROOT, json!({"name": name})))
                .await
                .unwrap();
            let id = saved["id"].as_str().unwrap();
            assert_eq!(
                registry.connection(id).err().unwrap(),
                sub2api::PROVIDER_NOT_FOUND,
                "{name}"
            );
        }
        // Defined since, it is reached through the gateway named for it.
        std::fs::write(
            dir.path().join("codex/config.toml"),
            "[model_providers.later]\nbase_url = \"https://later.example.com/v1\"\n",
        )
        .unwrap();
        let id = registry.members().unwrap()[0].key.clone();
        let c = registry.connection(&id).unwrap();
        assert_eq!(c.admin_base, "https://later.example.com/api/v1");
        assert_eq!(c.definition.id, "Later");
        // Kept in the Codex account store, keyed by id.
        let store = crate::repository::load_accounts(&app.paths.store()).unwrap();
        assert_eq!(store.extra["providers"][0]["id"], id.as_str());
        assert_eq!(store.extra["providers"][0]["name"], "Later");
    }

    #[test]
    fn addresses_lose_their_suffix_and_a_proxy_names_its_upstream() {
        assert_eq!(
            strip_suffix(" https://r/v1/ ", &["/api/v1", "/v1"]),
            "https://r"
        );
        assert_eq!(
            strip_suffix("https://r/api/v1", &["/api/v1", "/v1"]),
            "https://r"
        );
        assert_eq!(strip_suffix("https://r/", &["/v1"]), "https://r");
        assert_eq!(upstream(PROXIED), ROOT);
        assert_eq!(
            upstream("http://p/antigravity/http://relay"),
            "http://relay"
        );
        assert_eq!(upstream(ROOT), ROOT);
    }
}
