//! The Codex pool behind its API proxy: which logins and providers the
//! engine routes to, how it renews a login, Codex following the account the
//! proxy chose, and the logins a remote node receives.
use super::{config, load, CodexLogins, Result, Stored};
use crate::{
    application::{Application, Paths},
    login_store::{LoginPool, Standing},
    models::StoredAccount,
    pool::Pool,
    pool_login::{codex, Identity},
    proxy_service::{new_proxy_storage_context, ProxyStorageContext},
    Preferences,
};
use serde_json::{json, Value};
use std::sync::Arc;

/// The account as the proxy engine and remote nodes keep one.
pub(crate) fn engine_account(account: &Stored) -> Result<StoredAccount> {
    let mut engine: StoredAccount = serde_json::from_value(json!({
        "id": account.id, "label": account.label, "email": account.email,
        "accountId": account.own.workspace_id, "planType": account.plan,
        "authJson": account.document, "addedAt": account.added_at,
        "updatedAt": account.updated_at, "usage": account.usage,
        "usageError": account.usage_error, "principalId": account.own.principal_id,
        "proxyURL": account.proxy_url,
    }))
    .map_err(|e| e.to_string())?;
    if let Some(name) = &account.own.team_name {
        engine.extra.insert("teamName".into(), name.clone().into());
    }
    let (display, workspace) = match account.standing {
        None => ("list", "active"),
        Some(Standing::Deactivated) => ("deactivated", "deactivated"),
        Some(Standing::Deleted) => ("deleted", "deactivated"),
    };
    engine.extra.insert("displayStatus".into(), display.into());
    engine
        .extra
        .insert("workspaceStatus".into(), workspace.into());
    if account.first_request_pending {
        engine
            .extra
            .insert(crate::models::FIRST_REQUEST_PENDING.into(), true.into());
    }
    if let Some(choice) = account.renewal {
        engine.set_renewal(choice);
    }
    Ok(engine)
}
/// The pool's logins as the proxy's candidates: those in use, the one
/// Codex uses marked current, unless Codex is on a provider or the proxy
/// only hosts the API.
pub(crate) fn proxy_candidates(paths: &Paths) -> Result<Vec<StoredAccount>> {
    let store = load(paths)?;
    let settings = crate::repository::load_settings(&paths.data_dir.join("settings.json"))?;
    let official =
        config::resolve_path(&paths.codex_auth.with_file_name("config.toml")).is_official();
    let current = store
        .current_account_id
        .as_deref()
        .filter(|_| official && !settings.local_proxy_host_api_only);
    store
        .accounts
        .iter()
        .filter(|a| a.standing.is_none() && !a.first_request_pending)
        .map(|account| {
            let mut engine = engine_account(account)?;
            if current == Some(account.id.as_str()) {
                engine.extra.insert("copoolCurrent".into(), true.into());
            }
            Ok(engine)
        })
        .collect()
}
impl Application {
    /// The pool's API proxy, as the engine runs it: the pool keeps its
    /// logins, renews them and refreshes their usage.
    pub(crate) fn codex_proxy_storage(&self) -> ProxyStorageContext {
        let mut storage = new_proxy_storage_context(
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
            config::chatgpt_base_url(&self.codex_config_path()).as_deref(),
        ));
        // The pool keeps its logins, renews them and refreshes their usage.
        storage.read_store = false;
        storage.refresh_usage = false;
        let paths = self.paths.clone();
        storage.provider_candidates = Some(Arc::new(move || {
            let mut candidates = proxy_candidates(&paths)?;
            candidates.extend(crate::proxy_integration::candidates(&paths)?);
            Ok(candidates)
        }));
        if let Some(app) = self.shared() {
            let pool = Arc::downgrade(&app);
            storage.renew_candidate = Some(Arc::new(move |id, rejected| {
                let pool = pool.clone();
                Box::pin(async move {
                    pool.upgrade()
                        .ok_or("Copool is closing")?
                        .codex_renew_login(&id, &rejected)
                        .await
                })
            }));
            // A failover the proxy made switches Codex to the account that
            // answered.
            let pool = Arc::downgrade(&app);
            storage.on_selected = Some(Arc::new(move |id, auth| {
                let pool = pool.clone();
                Box::pin(async move {
                    let Some(app) = pool.upgrade() else {
                        return Ok(());
                    };
                    app.codex_follow_proxy(&id, &auth).await.map(|_| ())
                })
            }));
        }
        storage
    }
    /// The account the proxy chose, which it reached with `auth`: unless the
    /// proxy only hosts the API, Codex is switched to it, as any switch is,
    /// under the operations lock: a refresh renewing the account meanwhile
    /// has kept its renewal before the switch reads it. The proxy asks on
    /// every answered request, so what Codex already uses is told without
    /// the lock, which a refresh holds across its requests. The agents a
    /// switch syncs are given the login written to Codex, not `auth`, which
    /// a renewal may have replaced meanwhile. Returns whether it was
    /// switched.
    pub(crate) async fn codex_follow_proxy(&self, id: &str, auth: &Value) -> Result<bool> {
        if self.preferences()?.host_api_only || !self.codex_proxy_switches(id, auth)? {
            return Ok(false);
        }
        let _op = self.codex.operations.lock().await;
        // A switch, or the choice to only host the API, may have been made
        // while the lock was awaited.
        if self.preferences()?.host_api_only || !self.codex_proxy_switches(id, auth)? {
            return Ok(false);
        }
        let credential = if let Some(provider) = auth.get("copoolProvider") {
            let provider = provider["id"].as_str().ok_or("Invalid provider")?;
            config::switch(&self.codex_config_path(), provider)?;
            self.publish();
            None
        } else {
            let target = self
                .codex_switch_login(id, crate::vault::KeychainAccess::Interactive)
                .await?;
            let login = codex::normalized_auth_for_write(target.document)?;
            Some(codex::credential(&login, &Identity::default()))
        };
        drop(_op);
        self.after_switch(Pool::Codex, credential).await?;
        Ok(true)
    }
    /// Whether following the proxy to the account would switch Codex: to
    /// another provider, or to a known account it does not use.
    fn codex_proxy_switches(&self, id: &str, auth: &Value) -> Result<bool> {
        let config = config::resolve_path(&self.codex_config_path());
        if let Some(provider) = auth.get("copoolProvider") {
            let provider = provider["id"].as_str().ok_or("Invalid provider")?;
            return Ok(!config.id.eq_ignore_ascii_case(provider));
        }
        let store = self.codex_load()?;
        let Some(account) = store.accounts.iter().find(|a| a.id == id) else {
            return Ok(false);
        };
        let uses = config.is_official()
            && store.current_account_id.as_deref() == Some(id)
            && self
                .codex_read_live()
                .ok()
                .flatten()
                .is_some_and(|live| CodexLogins::same_login(&live, &account.document));
        Ok(!uses)
    }
    /// Renews a login the proxy's upstream refused (`rejected`, its access
    /// token), and returns the account's login in the proxy's form; a login
    /// renewed since is returned as it is.
    pub(crate) async fn codex_renew_login(&self, id: &str, rejected: &str) -> Result<Value> {
        let mut renewed = false;
        let result = self
            .codex_refresh_login(id, true, Some(&mut renewed), Some(rejected))
            .await;
        if !renewed {
            result?;
        }
        Ok(self
            .codex_load()?
            .accounts
            .into_iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?
            .document)
    }
    /// Merges a node's renewals, then the accounts to upload, in the
    /// engine's form: one per account, its newest login kept.
    pub(crate) async fn codex_remote_payload(&self, remote: Option<&Value>) -> Result<Vec<u8>> {
        let renewed: Vec<Value> = remote
            .and_then(|rows| rows["accounts"].as_array())
            .into_iter()
            .flatten()
            .map(|row| row["authJson"].clone())
            .collect();
        let newer = |account: &Stored| {
            renewed
                .iter()
                .find(|auth| codex::newer_login(auth, &account.document))
                .cloned()
        };
        let mut store = self.codex_load()?;
        if store.accounts.iter().any(|a| newer(a).is_some()) {
            store = self
                .codex_logins()
                .change(|store| {
                    for account in &mut store.accounts {
                        if let Some(login) = newer(account) {
                            account.document = login;
                        }
                    }
                    Ok(store.clone())
                })
                .await?;
        }
        let mut merged: Vec<&Stored> = Vec::new();
        for incoming in &store.accounts {
            let key = codex::account_identity(&incoming.document).ok();
            if let Some(old) = merged
                .iter_mut()
                .find(|a| key.is_some() && codex::account_identity(&a.document).ok() == key)
            {
                if incoming.updated_at > old.updated_at {
                    *old = incoming;
                }
            } else {
                merged.push(incoming);
            }
        }
        if !merged
            .iter()
            .any(|a| codex::extract_auth(&a.document).is_ok())
        {
            return Err("No usable accounts for remote deployment".into());
        }
        let accounts = merged
            .into_iter()
            .map(engine_account)
            .collect::<Result<Vec<_>>>()?;
        serde_json::to_vec_pretty(
            &json!({"version": 1, "accounts": accounts, "currentSelection": null}),
        )
        .map_err(|e| e.to_string())
    }
}
