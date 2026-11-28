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
    cliproxyapi::shared::Shared,
    codex::config::Definition,
    pool::Pool,
    provider_accounts::{Account, Member},
    sub2api,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, pin::Pin};

type Result<T> = std::result::Result<T, String>;
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What an unofficial provider is: the server whose accounts its cards show
/// and whose gateway its pool's tool is switched to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// Sub2api: an administrator signs in to its admin API, which reads each
    /// account's usage.
    #[default]
    #[serde(rename = "sub2api")]
    Sub2api,
    /// CLIProxyAPI: its management key reaches its management API, which
    /// relays each credential's usage requests to the pool's service.
    #[serde(rename = "cliproxyapi")]
    CliProxyApi,
}
impl Kind {
    /// Kept unsaved: a provider saved before kinds existed is Sub2api's.
    fn is_sub2api(&self) -> bool {
        *self == Kind::Sub2api
    }
    /// The kind as the cards and settings name it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::Sub2api => "sub2api",
            Kind::CliProxyApi => "cliproxyapi",
        }
    }
    /// The workspace tag of its cards.
    pub(crate) fn tag(self) -> &'static str {
        match self {
            Kind::Sub2api => "SUB2API",
            Kind::CliProxyApi => "CLIPROXYAPI",
        }
    }
}

/// What every pool keeps of an unofficial provider; a pool's own provider
/// embeds it (`#[serde(flatten)]`) beside what only that pool needs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProviderRecord {
    pub id: String,
    /// What the provider is; a provider saved without one is Sub2api's.
    #[serde(skip_serializing_if = "Kind::is_sub2api")]
    pub kind: Kind,
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
    /// Accounts whose cards the user removed: a refresh discovering them
    /// adds none of them again, only a sync does.
    #[serde(rename = "removedAccountIDs", skip_serializing_if = "Vec::is_empty")]
    pub removed_account_ids: Vec<i64>,
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
            kind: self.kind,
            key: self.id.clone(),
            name: self.name.clone(),
            proxy_url: self.proxy_url.clone(),
            imported: self.imported_account_ids.clone(),
            removed: self.removed_account_ids.clone(),
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
    /// Removes a provider and its password; none to remove is no error. A
    /// CLIProxyAPI provider leaves every pool.
    fn remove<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<()>>;
    /// Whether the pool would take a form, as its save checks it, without
    /// saving it or reading what is saved.
    fn check(&self, draft: Value) -> Result<()>;
    /// The gateway the pool's tool is sent to for a CLIProxyAPI provider
    /// whose form leaves it blank: the one its management site gives.
    fn default_gateway(&self, shared: &Shared) -> Result<String>;
    /// Drops the pool's record of a provider alone, its secret kept.
    fn forget<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<()>>;
    /// Gives the pool's record of a CLIProxyAPI provider, if it keeps one,
    /// what every pool shares of it, for what reads its store as kept (its
    /// switch, its local API proxy).
    fn take_shared<'a>(&'a self, shared: &'a Shared) -> BoxFuture<'a, Result<()>>;
    /// Every provider's cards.
    fn members(&self) -> Result<Vec<Member>>;
    /// Edits the cards of the provider the key names, writing back their
    /// accounts and proxy; false when there is no such provider.
    fn edit_member<'a>(
        &'a self,
        key: &'a str,
        edit: &'a mut (dyn FnMut(&mut Member) + Send),
    ) -> BoxFuture<'a, Result<bool>>;
    /// Where Copool reaches the provider's server, with its secret.
    fn connection(&self, key: &str) -> Result<Connection>;
}

/// Not found among a pool's providers.
pub(crate) const PROVIDER_NOT_FOUND: &str = "Provider not found";
const CONFIGURATION_INCOMPLETE: &str = "error.sub2api.configuration_incomplete";

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
    /// The pool's providers: those it keeps, a CLIProxyAPI one with what
    /// every pool shares of it, and one shared that the pool keeps nothing
    /// of yet as its switch's defaults make it (`fresh`).
    fn load(&self) -> Result<Vec<A::Provider>> {
        let shared = self.app.cliproxyapi_providers()?;
        let mut providers = self.adapter.load_providers()?;
        // A pool's record of a CLIProxyAPI provider removed went with it.
        providers.retain(|p| {
            let r = p.as_ref();
            r.kind != Kind::CliProxyApi || shared.iter().any(|s| s.id == r.id)
        });
        for p in &mut providers {
            if let Some(s) = shared_of(&shared, p.as_ref()) {
                s.apply(p.as_mut());
            }
        }
        for s in &shared {
            if !providers.iter().any(|p| p.as_ref().id == s.id) {
                providers.push(self.fresh(s)?);
            }
        }
        Ok(providers)
    }
    /// The pool's record of a CLIProxyAPI provider it keeps nothing of yet:
    /// its tool's gateway the server's root (CLIProxyAPI serves its API and
    /// management API alike), Claude Code's set by address, the rest as a
    /// saved form's are decided. None of its cards are imported.
    fn fresh(&self, shared: &Shared) -> Result<A::Provider> {
        let root = strip_suffix(
            &shared.admin_base_url,
            &["/v8/management", "/v0/management"],
        );
        let mut p: A::Provider =
            serde_json::from_value(serde_json::json!({"mode": "address", "baseURL": root}))
                .map_err(|e| e.to_string())?;
        p.as_mut().id = shared.id.clone();
        p.as_mut().kind = Kind::CliProxyApi;
        shared.apply(p.as_mut());
        // A pool that cannot decide them yet keeps them blank: its switch
        // says what is missing.
        if let Err(error) = self.adapter.prepare(&mut p) {
            log::debug!(
                "A CLIProxyAPI provider has no switch in {:?} yet: {error}",
                A::POOL
            );
        }
        shared.apply(p.as_mut());
        Ok(p)
    }
    fn find(&self, key: &str) -> Result<A::Provider> {
        self.load()?
            .into_iter()
            .find(|p| p.as_ref().id.eq_ignore_ascii_case(key))
            .ok_or_else(|| PROVIDER_NOT_FOUND.into())
    }
    /// A form as the pool keeps it: its fields trimmed, a CLIProxyAPI
    /// one's gateway the root of its management site unless given, and the
    /// pool's own fields decided (`Adapter::prepare`).
    fn prepared(&self, draft: Value) -> Result<A::Provider> {
        let mut draft: A::Provider = serde_json::from_value(draft).map_err(|e| e.to_string())?;
        {
            let r = draft.as_mut();
            r.name = r.name.trim().into();
            r.username = r.username.trim().into();
            r.admin_base_url = r.admin_base_url.trim().trim_end_matches('/').into();
            r.proxy_url = r.proxy_url.trim().into();
            // A CLIProxyAPI server's API is under its management site's root.
            if r.kind == Kind::CliProxyApi && r.base_url.trim().is_empty() {
                r.base_url = strip_suffix(&r.admin_base_url, &["/v8/management", "/v0/management"]);
            }
        }
        self.adapter.prepare(&mut draft)?;
        Ok(draft)
    }
    async fn save_typed(&self, draft: Value) -> Result<Value> {
        let invalid = |e: serde_json::Error| e.to_string();
        let mut draft = self.prepared(draft)?;
        if draft.as_ref().id.trim().is_empty() {
            draft.as_mut().id = uuid::Uuid::new_v4().to_string();
        }
        // Its connection is not read until both its record and password are.
        let _saving = self.app.provider_saves.write().await;
        // A provider the tool does not route yet is saved all the same.
        let routed = self
            .adapter
            .route(&draft)
            .map(|route| route.sub2api)
            .unwrap_or_default();
        let kind = draft.as_ref().kind;
        if kind == Kind::CliProxyApi {
            // Every pool reaches the server where the form says: its
            // management site, else the one its gateway is under.
            let r = draft.as_mut();
            if r.admin_base_url.is_empty() {
                r.admin_base_url = routed.trim_end_matches('/').to_owned();
            }
            if r.admin_base_url.is_empty() {
                return Err(CONFIGURATION_INCOMPLETE.into());
            }
        }
        // A CLIProxyAPI provider is every pool's: its name is unique among
        // every pool's providers.
        let mut elsewhere = Vec::new();
        if kind == Kind::CliProxyApi {
            for pool in Pool::ALL.into_iter().filter(|p| *p != A::POOL) {
                let listed = self.app.registry(pool).providers()?;
                elsewhere.extend(listed.as_array().into_iter().flatten().map(|p| {
                    let text = |key: &str| p[key].as_str().unwrap_or_default().to_owned();
                    (text("id"), text("name"))
                }));
            }
        }
        let shared = self.app.cliproxyapi_providers()?;
        let stored = self
            .adapter
            .edit_providers(|providers| {
                let r = draft.as_ref();
                let existing = providers.iter().position(|p| p.as_ref().id == r.id);
                let known = shared_of(&shared, r);
                let duplicate = providers.iter().any(|p| {
                    p.as_ref().id != r.id && p.as_ref().name.eq_ignore_ascii_case(&r.name)
                }) || shared
                    .iter()
                    .map(|s| (&s.id, &s.name))
                    .chain(elsewhere.iter().map(|(id, name)| (id, name)))
                    .any(|(id, name)| *id != r.id && name.eq_ignore_ascii_case(&r.name));
                crate::provider_accounts::validate_form(
                    r.kind,
                    &r.name,
                    duplicate,
                    &r.username,
                    &r.password,
                    existing.is_some() || known.is_some(),
                    &r.proxy_url,
                    &[&r.base_url, &routed, &r.admin_base_url],
                )?;
                let mut stored = draft.clone();
                let s = stored.as_mut();
                if let Some(old) = existing.map(|i| providers[i].as_ref()) {
                    s.imported_account_ids = old.imported_account_ids.clone();
                    s.removed_account_ids = old.removed_account_ids.clone();
                    s.cached_accounts = old.cached_accounts.clone();
                    // The provider's card sets its proxy, not its settings.
                    s.proxy_url = old.proxy_url.clone();
                }
                // Every pool's cards of a CLIProxyAPI provider share it.
                if let Some(known) = known {
                    s.proxy_url = known.proxy_url.clone();
                }
                // Kept apart from the store, once the store is saved.
                let password = std::mem::take(&mut s.password);
                let previous = match existing {
                    Some(i) => Some(std::mem::replace(&mut providers[i], stored.clone())),
                    None => {
                        providers.push(stored.clone());
                        None
                    }
                };
                Ok((stored, password, previous))
            })
            .await?;
        let (stored, password, previous) = stored;
        let id = stored.as_ref().id.clone();
        // What every pool shares of a CLIProxyAPI provider, the one before
        // kept should its key be refused.
        let shared_before = match kind {
            Kind::CliProxyApi => {
                let mine = Shared::of(stored.as_ref());
                self.app.edit_cliproxyapi_providers(|all| {
                    Ok(match all.iter_mut().find(|s| s.id == mine.id) {
                        // Its cards set its proxy, which they may have
                        // meanwhile.
                        Some(s) => Some(Some(std::mem::replace(
                            s,
                            Shared {
                                proxy_url: s.proxy_url.clone(),
                                ..mine
                            },
                        ))),
                        None => {
                            all.push(mine);
                            Some(None)
                        }
                    })
                })?
            }
            Kind::Sub2api => None,
        };
        // Off the store's lock and the async threads: the credential store
        // may wait for the user to allow it.
        let secret = crate::provider_accounts::provider_secret(A::POOL, kind, &id);
        if let Err(error) = self.app.store_provider_password(secret, password).await {
            // A save whose password is refused is not made: the form stays
            // to be saved again, an edited provider as it was, an added one
            // gone, and what its refreshes cached meanwhile kept.
            let undone = self
                .adapter
                .edit_providers(|providers| {
                    let Some(i) = providers.iter().position(|p| p.as_ref().id == id) else {
                        return Ok(());
                    };
                    match previous {
                        Some(mut previous) => {
                            let (now, was) = (providers[i].as_ref(), previous.as_mut());
                            was.imported_account_ids = now.imported_account_ids.clone();
                            was.removed_account_ids = now.removed_account_ids.clone();
                            was.cached_accounts = now.cached_accounts.clone();
                            was.proxy_url = now.proxy_url.clone();
                            providers[i] = previous;
                        }
                        None => {
                            providers.remove(i);
                        }
                    }
                    Ok(())
                })
                .await
                .and_then(|()| match shared_before {
                    Some(before) => self.app.edit_cliproxyapi_providers(|all| {
                        match before {
                            Some(before) => {
                                if let Some(s) = all.iter_mut().find(|s| s.id == id) {
                                    *s = Shared {
                                        proxy_url: s.proxy_url.clone(),
                                        ..before
                                    };
                                }
                            }
                            None => all.retain(|s| s.id != id),
                        }
                        Ok(())
                    }),
                    None => Ok(()),
                });
            if let Err(undo) = undone {
                log::warn!("The provider saved without its password stays saved: {undo}");
            }
            return Err(error);
        }
        if kind == Kind::CliProxyApi {
            self.app.spread_cliproxyapi(&id, A::POOL).await;
        }
        let mut shown = stored;
        self.adapter.present(&mut shown);
        serde_json::to_value(shown).map_err(invalid)
    }
}

/// What every pool shares of a pool's CLIProxyAPI provider; none for
/// Sub2api's.
fn shared_of<'s>(shared: &'s [Shared], record: &ProviderRecord) -> Option<&'s Shared> {
    (record.kind == Kind::CliProxyApi)
        .then(|| shared.iter().find(|s| s.id == record.id))
        .flatten()
}

impl<A: Adapter> ProviderRegistry for Registry<'_, A> {
    fn providers(&self) -> Result<Value> {
        let mut providers = self.load()?;
        for p in &mut providers {
            self.adapter.present(p);
        }
        serde_json::to_value(providers).map_err(|e| e.to_string())
    }
    fn save(&self, draft: Value) -> BoxFuture<'_, Result<Value>> {
        Box::pin(self.save_typed(draft))
    }
    fn default_gateway(&self, shared: &Shared) -> Result<String> {
        Ok(self.fresh(shared)?.as_ref().base_url.clone())
    }
    fn check(&self, draft: Value) -> Result<()> {
        let draft = self.prepared(draft)?;
        let r = draft.as_ref();
        let routed = self
            .adapter
            .route(&draft)
            .map(|route| route.sub2api)
            .unwrap_or_default();
        // A form's name and secret are checked where it is saved.
        crate::provider_accounts::validate_form(
            r.kind,
            if r.name.is_empty() { "-" } else { &r.name },
            false,
            &r.username,
            &r.password,
            true,
            &r.proxy_url,
            &[&r.base_url, &routed, &r.admin_base_url],
        )
    }
    fn remove<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Not while it is being saved, whose password would be kept
            // after its record is gone.
            let _saving = self.app.provider_saves.write().await;
            let kind = self
                .load()?
                .iter()
                .find(|p| p.as_ref().id == id)
                .map_or(Kind::Sub2api, |p| p.as_ref().kind);
            self.forget(id).await?;
            // A CLIProxyAPI provider leaves every pool; their records of it
            // go as they are next read (`load`), or now (`forget`).
            if kind == Kind::CliProxyApi {
                self.app.edit_cliproxyapi_providers(|all| {
                    all.retain(|s| s.id != id);
                    Ok(())
                })?;
            }
            // Off the async threads: the credential store may wait for the
            // user to allow it.
            let secrets = self.app.secrets.clone();
            let key = crate::provider_accounts::provider_secret(A::POOL, kind, id);
            tokio::task::spawn_blocking(move || secrets.remove(&key))
                .await
                .map_err(|e| e.to_string())?
        })
    }
    fn forget<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.adapter.edit_providers(move |providers| {
            providers.retain(|p| p.as_ref().id != id);
            Ok(())
        }))
    }
    fn take_shared<'a>(&'a self, shared: &'a Shared) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.adapter.edit_providers(move |providers| {
            for p in providers.iter_mut() {
                if p.as_ref().kind == Kind::CliProxyApi && p.as_ref().id == shared.id {
                    shared.apply(p.as_mut());
                }
            }
            Ok(())
        }))
    }
    fn members(&self) -> Result<Vec<Member>> {
        Ok(self.load()?.iter().map(|p| p.as_ref().member()).collect())
    }
    fn edit_member<'a>(
        &'a self,
        key: &'a str,
        edit: &'a mut (dyn FnMut(&mut Member) + Send),
    ) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let Some(loaded) = self
                .load()?
                .into_iter()
                .find(|p| p.as_ref().id.eq_ignore_ascii_case(key))
            else {
                return Ok(false);
            };
            let shared =
                (loaded.as_ref().kind == Kind::CliProxyApi).then(|| Shared::of(loaded.as_ref()));
            let app = self.app;
            let edited = self
                .adapter
                .edit_providers(move |providers| {
                    let id = loaded.as_ref().id.clone();
                    let i = match providers.iter().position(|p| p.as_ref().id == id) {
                        Some(i) => i,
                        // A CLIProxyAPI provider the pool keeps nothing of
                        // yet is kept once its cards are, unless it was
                        // removed meanwhile; any other was removed.
                        None if shared.is_some()
                            && app.cliproxyapi_providers()?.iter().any(|s| s.id == id) =>
                        {
                            providers.push(loaded);
                            providers.len() - 1
                        }
                        None => return Ok(None),
                    };
                    let r = providers[i].as_mut();
                    if let Some(shared) = &shared {
                        shared.apply(r);
                    }
                    let mut member = r.member();
                    edit(&mut member);
                    r.imported_account_ids = member.imported;
                    r.removed_account_ids = member.removed;
                    r.cached_accounts = member.cached;
                    r.proxy_url = member.proxy_url;
                    Ok(Some(
                        shared
                            .filter(|s| s.proxy_url != r.proxy_url)
                            .map(|s| (s.id, r.proxy_url.clone())),
                    ))
                })
                .await?;
            let Some(proxy) = edited else {
                return Ok(false);
            };
            // A CLIProxyAPI card's proxy is every pool's cards'.
            if let Some((id, proxy)) = proxy {
                self.app.edit_cliproxyapi_providers(|all| {
                    if let Some(s) = all.iter_mut().find(|s| s.id == id) {
                        s.proxy_url = proxy;
                    }
                    Ok(())
                })?;
                self.app.spread_cliproxyapi(&id, A::POOL).await;
            }
            Ok(true)
        })
    }
    fn connection(&self, key: &str) -> Result<Connection> {
        let p = self.find(key)?;
        let r = p.as_ref();
        let password = || {
            self.app
                .secrets
                .get(&crate::provider_accounts::provider_secret(
                    A::POOL,
                    r.kind,
                    &r.id,
                ))?
                .ok_or_else(|| CONFIGURATION_INCOMPLETE.to_owned())
        };
        // Every pool reaches a CLIProxyAPI server at the management site it
        // shares, whether or not its tool can be switched to it yet.
        if r.kind == Kind::CliProxyApi {
            return Ok(Connection::CliProxyApi(crate::cliproxyapi::Connection {
                management: crate::cliproxyapi::management_base(&r.admin_base_url)?,
                key: password()?,
                proxy_url: r.proxy_url.clone(),
                insecure: r.allow_insecure_tls,
                provider_id: r.name.clone(),
                cached: r.cached_accounts.clone(),
            }));
        }
        // A Sub2api provider its tool does not route is not signed in to.
        let route = self.adapter.route(&p)?;
        let password = password()?;
        // The server itself, not a proxy in front of the tool's requests.
        let admin = if r.admin_base_url.is_empty() {
            &route.sub2api
        } else {
            &r.admin_base_url
        };
        Ok(Connection::Sub2api(sub2api::Connection {
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
        }))
    }
}

/// Where Copool reaches a provider's server, by its kind.
pub(crate) enum Connection {
    Sub2api(sub2api::Connection),
    CliProxyApi(crate::cliproxyapi::Connection),
}
impl Connection {
    /// The pool's accounts at the server, those by `ids` (every one
    /// without) with their usage.
    pub(crate) async fn fetch_accounts(
        &self,
        pool: Pool,
        ids: Option<&[i64]>,
        now: i64,
    ) -> Result<Vec<Account>> {
        match self {
            Connection::Sub2api(c) => c.fetch_accounts(pool, ids, now).await,
            Connection::CliProxyApi(c) => c.fetch_accounts(pool, ids, now).await,
        }
    }
    /// The Sub2api connection, for a test of a Sub2api provider.
    #[cfg(test)]
    pub(crate) fn sub2api(self) -> sub2api::Connection {
        match self {
            Connection::Sub2api(c) => c,
            Connection::CliProxyApi(_) => panic!("a CLIProxyAPI provider"),
        }
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
            Pool::Codex => registry(self, crate::codex::ProviderAdapter(self)),
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
                    .get(&crate::provider_accounts::password_secret(case.pool, &id)),
                Some(&"secret".to_owned())
            );
        }
    }

    #[tokio::test]
    async fn a_cliproxyapi_provider_signs_in_with_its_key_alone_and_its_cards_say_so() {
        for case in cases() {
            let dir = tempfile::tempdir().unwrap();
            let (app, _) = open(dir.path());
            let registry = app.registry(case.pool);
            // Only Sub2api's administrator signs in with a name.
            let unnamed = form(&case, case.typed_url, json!({"username": ""}));
            assert_eq!(
                registry.save(unnamed).await.unwrap_err(),
                "error.sub2api.configuration_incomplete",
                "{:?}",
                case.pool
            );
            let saved = registry
                .save(form(
                    &case,
                    case.typed_url,
                    json!({"kind": "cliproxyapi", "username": "", "password": "management-key"}),
                ))
                .await
                .unwrap();
            assert_eq!(saved["kind"], "cliproxyapi", "{:?}", case.pool);
            let id = saved["id"].as_str().unwrap().to_owned();
            // Its management API is the server's, whatever gateway the tool
            // is sent to.
            match registry.connection(&id).unwrap() {
                Connection::CliProxyApi(c) => {
                    assert_eq!(c.management, format!("{ROOT}/v8/management"));
                    assert_eq!(c.key, "management-key");
                    assert_eq!(c.provider_id, "Relay");
                }
                Connection::Sub2api(_) => panic!("{:?}", case.pool),
            }
            registry
                .edit_member(&id, &mut |m: &mut Member| {
                    m.imported = vec![7];
                    m.cached = vec![account(7)];
                })
                .await
                .unwrap();
            let cards = app.provider_cards(case.pool, None).unwrap();
            assert_eq!(cards.len(), 1);
            assert!(crate::provider_accounts::is_card(case.pool, &cards[0].id));
            assert_eq!(cards[0].source_tag.as_deref(), Some("CLIPROXYAPI"));
            assert_eq!(cards[0].provider_kind.as_deref(), Some("cliproxyapi"));
        }
    }

    // One CLIProxyAPI server lists every pool's credentials: a provider
    // saved in one pool is every pool's, its key kept once, each pool's
    // switch and cards its own, and it leaves every pool together.
    #[tokio::test]
    async fn a_cliproxyapi_provider_is_every_pools() {
        use crate::credentials::SecretStore;
        let dir = tempfile::tempdir().unwrap();
        let (app, secrets) = open(dir.path());
        let saved = app
            .save_provider(
                Pool::Claude,
                json!({"kind": "cliproxyapi", "mode": "address", "name": " Relay ",
                    "adminBaseURL": "http://cpa.example.com:8317/v8/management/",
                    "password": "management-key"}),
            )
            .await
            .unwrap();
        let id = saved["id"].as_str().unwrap().to_owned();
        // Claude Code's gateway defaults to the server's root.
        assert_eq!(saved["baseURL"], "http://cpa.example.com:8317");
        assert_eq!(
            secrets
                .get(&crate::cliproxyapi::shared::key_secret(&id))
                .unwrap()
                .as_deref(),
            Some("management-key")
        );
        assert!(secrets
            .get(&crate::provider_accounts::password_secret(
                Pool::Claude,
                &id
            ))
            .unwrap()
            .is_none());
        for pool in Pool::ALL {
            let listed = app.providers(pool).unwrap();
            let listed = listed.as_array().unwrap();
            assert_eq!(listed.len(), 1, "{pool:?}");
            assert_eq!(listed[0]["id"], id.as_str());
            assert_eq!(listed[0]["name"], "Relay");
            assert_eq!(
                listed[0]["adminBaseURL"],
                "http://cpa.example.com:8317/v8/management"
            );
            // Nothing imported in a pool before its own import.
            assert!(app.provider_cards(pool, None).unwrap().is_empty());
            match app.registry(pool).connection(&id).unwrap() {
                Connection::CliProxyApi(c) => {
                    assert_eq!(c.management, "http://cpa.example.com:8317/v8/management");
                    assert_eq!(c.key, "management-key");
                }
                Connection::Sub2api(_) => panic!("{pool:?}"),
            }
        }
        // A pool keeping nothing of it yet switches to the server's root,
        // with its own key variable.
        let agy = &app.providers(Pool::Antigravity).unwrap()[0];
        assert_eq!(agy["baseURL"], "http://cpa.example.com:8317");
        assert_eq!(agy["apiKeyVariable"], "GEMINI_API_KEY");

        // A pool's cards are its own; their proxy is every pool's.
        app.registry(Pool::Grok)
            .edit_member(&id, &mut |m: &mut Member| {
                m.imported = vec![7];
                m.cached = vec![account(7)];
                m.proxy_url = "http://proxy.example.com:8080".into();
            })
            .await
            .unwrap();
        assert_eq!(app.provider_cards(Pool::Grok, None).unwrap().len(), 1);
        assert!(app.provider_cards(Pool::Codex, None).unwrap().is_empty());
        for pool in Pool::ALL {
            assert_eq!(
                app.providers(pool).unwrap()[0]["proxyURL"],
                "http://proxy.example.com:8080",
                "{pool:?}"
            );
        }

        // An edit in another pool, its key blank, keeps the key and renames
        // it in every pool; the pool's own switch is its own.
        let mut edit = app.providers(Pool::Antigravity).unwrap()[0].clone();
        edit["name"] = json!("Renamed");
        edit["apiKeyVariable"] = json!("CPA_KEY");
        app.save_provider(Pool::Antigravity, edit).await.unwrap();
        for pool in Pool::ALL {
            assert_eq!(
                app.providers(pool).unwrap()[0]["name"],
                "Renamed",
                "{pool:?}"
            );
        }
        assert_eq!(
            app.providers(Pool::Antigravity).unwrap()[0]["apiKeyVariable"],
            "CPA_KEY"
        );
        // Grok's names none of its own: its switch asks for one.
        assert_eq!(app.providers(Pool::Grok).unwrap()[0]["apiKeyVariable"], "");
        assert_eq!(
            secrets
                .get(&crate::cliproxyapi::shared::key_secret(&id))
                .unwrap()
                .as_deref(),
            Some("management-key")
        );
        assert_eq!(app.provider_cards(Pool::Grok, None).unwrap().len(), 1);

        // A Sub2api provider of the same name is refused in any pool.
        assert_eq!(
            app.save_provider(
                Pool::Codex,
                json!({"name": "renamed", "username": "admin", "password": "p",
                    "adminBaseURL": "https://relay.example.com"}),
            )
            .await
            .unwrap_err(),
            "error.sub2api.duplicate_provider"
        );

        // Removed in one pool, it leaves every pool, with its key.
        app.remove_provider(Pool::Codex, &id).await.unwrap();
        for pool in Pool::ALL {
            assert!(
                app.providers(pool).unwrap().as_array().unwrap().is_empty(),
                "{pool:?}"
            );
        }
        assert!(secrets
            .get(&crate::cliproxyapi::shared::key_secret(&id))
            .unwrap()
            .is_none());
    }

    // A CLIProxyAPI provider is saved for every pool from one form, with
    // each pool's switch; another of the same site and key is refused, one
    // with another key is not.
    #[tokio::test]
    async fn a_cliproxyapi_form_is_saved_for_every_pool_and_once() {
        use crate::credentials::SecretStore;
        let dir = tempfile::tempdir().unwrap();
        let (app, secrets) = open(dir.path());
        let form = |name: &str, key: &str| {
            json!({"name": name, "adminBaseURL": "http://cpa.example.com:8317", "password": key,
                "pools": {"grok": {"baseURL": "", "apiKeyVariable": ""},
                    "antigravity": {"baseURL": "http://gateway.example.com:8317", "apiKeyVariable": "CPA_KEY"}}})
        };
        let saved = app
            .save_cliproxyapi_provider(form("Relay", "one"))
            .await
            .unwrap();
        let id = saved["id"].as_str().unwrap().to_owned();
        assert_eq!(saved["kind"], "cliproxyapi");
        assert_eq!(saved["pools"]["antigravity"]["apiKeyVariable"], "CPA_KEY");
        assert_eq!(
            saved["pools"]["antigravity"]["baseURL"],
            "http://gateway.example.com:8317"
        );
        // Grok Build names no variable until it is switched to it.
        assert_eq!(saved["pools"]["grok"]["apiKeyVariable"], "");
        // Claude Code's follows the management site, as left blank.
        assert_eq!(saved["pools"]["claude"]["baseURL"], "");
        assert_eq!(
            app.registry(Pool::Claude).providers().unwrap()[0]["baseURL"],
            "http://cpa.example.com:8317"
        );
        assert_eq!(
            secrets
                .get(&crate::cliproxyapi::shared::key_secret(&id))
                .unwrap()
                .as_deref(),
            Some("one")
        );
        // Every pool keeps its record of it now.
        for pool in Pool::ALL {
            assert_eq!(
                app.registry(pool).providers().unwrap()[0]["id"],
                id.as_str(),
                "{pool:?}"
            );
        }
        // The same site and key again, by whatever name, is the same one.
        assert_eq!(
            app.save_cliproxyapi_provider(form("Other", "one"))
                .await
                .unwrap_err(),
            crate::cliproxyapi::shared::DUPLICATE
        );
        let mut other_site = form("Other", "one");
        other_site["adminBaseURL"] = json!("http://cpa.example.com:8317/v8/management/");
        assert_eq!(
            app.save_cliproxyapi_provider(other_site).await.unwrap_err(),
            crate::cliproxyapi::shared::DUPLICATE
        );
        let second = app
            .save_cliproxyapi_provider(form("Other", "two"))
            .await
            .unwrap();
        assert_eq!(
            app.cliproxyapi_provider_list()
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
        // An edit of one, its key blank, keeps its key and is not its own
        // duplicate.
        let mut edit = second.clone();
        edit["password"] = json!("");
        edit["pools"]["grok"]["apiKeyVariable"] = json!("GROK_CPA");
        let edited = app.save_cliproxyapi_provider(edit).await.unwrap();
        assert_eq!(edited["pools"]["grok"]["apiKeyVariable"], "GROK_CPA");
        // Its key changed to the first's is refused.
        let mut same = second.clone();
        same["password"] = json!("one");
        assert_eq!(
            app.save_cliproxyapi_provider(same).await.unwrap_err(),
            crate::cliproxyapi::shared::DUPLICATE
        );
        // A form a pool refuses saves nothing in any.
        let mut invalid = form("Third", "three");
        invalid["pools"]["antigravity"]["apiKeyVariable"] = json!("not a variable");
        assert!(app.save_cliproxyapi_provider(invalid).await.is_err());
        assert_eq!(
            app.cliproxyapi_provider_list()
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
        for pool in Pool::ALL {
            assert_eq!(
                app.registry(pool)
                    .providers()
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
        }
        app.remove_cliproxyapi_provider(&id).await.unwrap();
        assert_eq!(
            app.cliproxyapi_provider_list()
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    // A new one a pool fails to save after another saved it is removed
    // whole, its key too.
    #[tokio::test]
    async fn a_new_cliproxyapi_provider_is_never_left_half_saved() {
        let dir = tempfile::tempdir().unwrap();
        let (app, secrets) = open(dir.path());
        // Antigravity's store cannot be read: its save fails after Codex's
        // and Claude's.
        std::fs::create_dir_all(app.paths.data_dir.join("antigravity-accounts.json")).unwrap();
        assert!(app
            .save_cliproxyapi_provider(json!({"name": "Relay",
                "adminBaseURL": "http://cpa.example.com:8317", "password": "key"}))
            .await
            .is_err());
        assert!(app.cliproxyapi_providers().unwrap().is_empty());
        assert!(crate::codex::load(&app.paths).unwrap().providers.is_empty());
        assert!(app
            .registry(Pool::Claude)
            .providers()
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        assert!(
            secrets.0.lock().unwrap().is_empty(),
            "{:?}",
            secrets.0.lock().unwrap()
        );
    }

    // A gateway its form leaves blank follows the management site: listed
    // blank, it moves with the site; one typed stays.
    #[tokio::test]
    async fn a_blank_gateway_follows_the_management_site() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _) = open(dir.path());
        let saved = app
            .save_cliproxyapi_provider(json!({"name": "Relay", "adminBaseURL": "http://old.example.com:8317",
                "password": "key", "pools": {"antigravity": {"baseURL": "http://typed.example.com"}}}))
            .await
            .unwrap();
        assert_eq!(saved["pools"]["claude"]["baseURL"], "");
        assert_eq!(saved["pools"]["grok"]["baseURL"], "");
        assert_eq!(
            saved["pools"]["antigravity"]["baseURL"],
            "http://typed.example.com"
        );
        let mut edit = saved.clone();
        edit["adminBaseURL"] = json!("http://new.example.com:8317");
        app.save_cliproxyapi_provider(edit).await.unwrap();
        let gateway = |pool| app.registry(pool).providers().unwrap()[0]["baseURL"].clone();
        assert_eq!(gateway(Pool::Claude), "http://new.example.com:8317");
        assert_eq!(gateway(Pool::Grok), "http://new.example.com:8317");
        assert_eq!(gateway(Pool::Antigravity), "http://typed.example.com");
    }

    // What reads a pool's store as kept (its switch, its local API proxy)
    // sees a CLIProxyAPI provider as edited in another pool; a Codex one
    // `config.toml` names nothing of is reached all the same; its name is
    // unique among every pool's providers, Sub2api's too; one saved by its
    // gateway alone is reached there by every pool; and a refresh ending
    // after its provider was removed keeps nothing of it.
    #[tokio::test]
    async fn a_cliproxyapi_provider_reads_alike_in_every_pool() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _) = open(dir.path());
        app.save_provider(
            Pool::Codex,
            json!({"name": "Taken", "username": "admin", "password": "p"}),
        )
        .await
        .unwrap();
        let refused = app
            .save_provider(
                Pool::Grok,
                json!({"kind": "cliproxyapi", "name": "taken", "apiKeyVariable": "K",
                    "adminBaseURL": "http://cpa.example.com:8317", "password": "key"}),
            )
            .await
            .unwrap_err();
        assert_eq!(refused, "error.sub2api.duplicate_provider");
        // Its gateway alone: its management site is the server the gateway
        // is on.
        let saved = app
            .save_provider(
                Pool::Grok,
                json!({"kind": "cliproxyapi", "name": "Unknown", "apiKeyVariable": "K",
                    "baseURL": "http://cpa.example.com:8317/v1", "password": "key"}),
            )
            .await
            .unwrap();
        let id = saved["id"].as_str().unwrap().to_owned();
        let shared = app.cliproxyapi_providers().unwrap();
        assert_eq!(shared[0].admin_base_url, "http://cpa.example.com:8317");
        // Codex's config.toml defines no provider "Unknown": its cards are
        // read all the same.
        assert!(matches!(
            app.registry(Pool::Codex).connection(&id).unwrap(),
            Connection::CliProxyApi(_)
        ));
        // Each pool keeps a record once it has cards; an edit elsewhere is
        // in it as kept.
        for pool in Pool::ALL {
            app.registry(pool)
                .edit_member(&id, &mut |m: &mut Member| m.imported = vec![7])
                .await
                .unwrap();
        }
        app.registry(Pool::Claude)
            .edit_member(&id, &mut |m: &mut Member| m.proxy_url = "direct://".into())
            .await
            .unwrap();
        let mut edit = app.providers(Pool::Grok).unwrap()[0].clone();
        edit["name"] = json!("Renamed");
        edit["allowInsecureTLS"] = json!(true);
        app.save_provider(Pool::Grok, edit).await.unwrap();
        let kept = crate::codex::load(&app.paths).unwrap().providers;
        assert_eq!(kept[1].name, "Renamed");
        assert_eq!(kept[1].proxy_url, "direct://");
        assert!(kept[1].allow_insecure_tls);
        let agy: Value = serde_json::from_slice(
            &std::fs::read(app.paths.data_dir.join("antigravity-accounts.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(agy["providers"][0]["name"], "Renamed");
        assert_eq!(agy["providers"][0]["proxyURL"], "direct://");

        // A refresh of a provider removed meanwhile keeps nothing of it.
        let registry = app.registry(Pool::Codex);
        let sub2api = app.providers(Pool::Codex).unwrap()[0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        app.remove_provider(Pool::Codex, &sub2api).await.unwrap();
        assert!(!registry
            .edit_member(&sub2api, &mut |m: &mut Member| m.imported = vec![1])
            .await
            .unwrap());
        app.remove_provider(Pool::Antigravity, &id).await.unwrap();
        assert!(!registry
            .edit_member(&id, &mut |m: &mut Member| m.imported = vec![1])
            .await
            .unwrap());
        assert!(crate::codex::load(&app.paths).unwrap().providers.is_empty());
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
                    .get(&crate::provider_accounts::password_secret(case.pool, &id)),
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
            let c = registry.connection(&id).unwrap().sub2api();
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
            let c = registry.connection(&id).unwrap().sub2api();
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
                registry.connection(&id).unwrap().sub2api().admin_base,
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
        let c = registry.connection(&id).unwrap().sub2api();
        assert_eq!(c.admin_base, "https://later.example.com/api/v1");
        assert_eq!(c.definition.id, "Later");
        // Kept in the Codex account store, keyed by id.
        let store = app.codex_load().unwrap();
        assert_eq!(store.providers[0].id, id);
        assert_eq!(store.providers[0].name, "Later");
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
