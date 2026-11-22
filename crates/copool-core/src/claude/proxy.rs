//! The Claude pool behind its API proxy: which logins and providers the
//! engine routes to, how it renews a login, and Claude Code following the
//! account the proxy chose. Routing itself is the engine's, as for Codex.
use super::{
    holds_tokens, settings_are_official, settings_source, KeychainAccess, Result, API_ROOT,
};
use crate::{
    application::Application,
    claude_api::{plan, text},
    models::{Pool, StoredAccount},
    proxy_service::{new_proxy_storage_context, ProxyStorageContext},
};
use serde_json::{json, Value};
use std::sync::Arc;

/// A Sub2api provider's route in the proxy, beside its cards.
fn provider_route(id: &str) -> String {
    format!("claude-sub2api-provider:{id}")
}

impl Application {
    pub(crate) fn claude_proxy_storage(&self) -> ProxyStorageContext {
        let mut storage = new_proxy_storage_context(
            self.paths.data_dir.clone(),
            Arc::new(tokio::sync::Mutex::new(())),
            false,
        );
        storage.pool = Pool::Claude;
        storage.key_file = "claude-api-proxy.key";
        // The pool keeps its own logins, renews them and refreshes usage.
        storage.read_store = false;
        storage.refresh_usage = false;
        storage.prefer_local_selection = true;
        storage.upstream_base_url = Some(self.claude_proxy_upstream());
        let paths = self.paths.clone();
        storage.account_proxy = Some(Arc::new(move || {
            let settings = crate::repository::load_settings(&paths.data_dir.join("settings.json"))?;
            Ok(settings
                .extra
                .get("accountProxyUrl")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned())
        }));
        let Some(app) = self.shared() else {
            // Not shared: no pool to route to.
            storage.provider_candidates = Some(Arc::new(|| Ok(Vec::new())));
            return storage;
        };
        let weak = Arc::downgrade(&app);
        let pool = weak.clone();
        storage.provider_candidates = Some(Arc::new(move || {
            pool.upgrade()
                .ok_or("Copool is closing")?
                .claude_proxy_candidates()
        }));
        let renew = weak.clone();
        storage.renew_candidate = Some(Arc::new(move |id, rejected| {
            let renew = renew.clone();
            Box::pin(async move {
                renew
                    .upgrade()
                    .ok_or("Copool is closing")?
                    .claude_renew_login(&id, &rejected)
                    .await
            })
        }));
        storage.on_selected = Some(Arc::new(move |id, auth| {
            let weak = weak.clone();
            Box::pin(async move {
                match weak.upgrade() {
                    Some(app) => app.claude_follow_proxy(&id, &auth).await,
                    None => Ok(()),
                }
            })
        }));
        storage
    }
    /// Where the proxy sends Claude's requests: where Claude Code sends its
    /// own, unless that is this proxy, which would send them to itself.
    fn claude_proxy_upstream(&self) -> String {
        let base = self.claude_messages_base();
        let port = self.proxy_port(Pool::Claude).unwrap_or_default();
        let own = reqwest::Url::parse(&base).is_ok_and(|url| {
            matches!(
                url.host_str(),
                Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
            ) && url.port_or_known_default() == Some(port)
        });
        if own {
            API_ROOT.into()
        } else {
            base
        }
    }
    /// The pool's logins and providers as the engine's accounts. A login
    /// whose tokens cannot be read, or that waits for a manual refresh, is
    /// left out, as the Codex proxy leaves out such accounts.
    pub(crate) fn claude_proxy_candidates(&self) -> Result<Vec<StoredAccount>> {
        let store = self.claude_load()?;
        let location = self.claude_location();
        let official = settings_are_official(&location);
        let mut candidates = Vec::new();
        for account in store.accounts.iter().filter(|a| holds_tokens(a)) {
            let current = official && store.current_account_id.as_deref() == Some(&account.id);
            let mut stored: StoredAccount = serde_json::from_value(json!({
                "id": account.id,
                "label": account.label,
                "email": text(&account.account["emailAddress"]),
                "accountId": text(&account.account["accountUuid"]).unwrap_or_default(),
                "planType": plan(&account.oauth),
                "authJson": {"claudeAiOauth": account.oauth},
                "proxyUrl": account.proxy_url,
                "addedAt": account.added_at,
                "updatedAt": account.updated_at,
                "usage": account.usage,
                "usageError": account.usage_error,
            }))
            .map_err(|e| e.to_string())?;
            if let Some(renewal) = account.renewal {
                stored.set_renewal(renewal);
            }
            if account.first_request_pending {
                stored
                    .extra
                    .insert(crate::models::FIRST_REQUEST_PENDING.into(), true.into());
            }
            if current {
                stored.extra.insert("copoolCurrent".into(), true.into());
            }
            candidates.push(stored);
        }
        // Each provider is one route, as in the Codex proxy, with its gateway
        // and the key of its settings file.
        let applied = store.settings_provider.as_deref().filter(|_| !official);
        for provider in store
            .providers
            .iter()
            .filter(|p| !p.imported_account_ids.is_empty())
        {
            let Ok(source) =
                settings_source::load(&location, &provider.name, &provider.settings_path)
            else {
                continue;
            };
            let Some(key) = text(&source.document["env"][source.auth_key]) else {
                continue;
            };
            let representative = provider.cached_accounts.iter().max_by(|a, b| {
                crate::ranking::remaining_score(&a.summary(String::new()))
                    .total_cmp(&crate::ranking::remaining_score(&b.summary(String::new())))
            });
            let route = provider_route(&provider.id);
            candidates.push(
                serde_json::from_value(json!({
                    "id": route,
                    "label": provider.name,
                    "accountId": route,
                    "authJson": {"copoolProvider": {
                        "id": provider.id,
                        "baseUrl": super::sub2api::gateway(&source.base_url),
                        "apiKey": key,
                        "authHeader": if source.auth_key == "ANTHROPIC_API_KEY" { "x-api-key" } else { "authorization" },
                        "allowInsecureTLS": provider.allow_insecure_tls,
                        "preferred": applied == Some(provider.id.as_str()),
                    }},
                    "proxyUrl": provider.proxy_url.trim(),
                    "addedAt": representative.map(|a| a.id).unwrap_or_default(),
                    "updatedAt": 0,
                    "usage": representative.and_then(|a| a.usage.clone()),
                }))
                .map_err(|e| e.to_string())?,
            );
        }
        Ok(candidates)
    }
    /// Renews a login the upstream refused (its access token `rejected`),
    /// through the pool, which keeps Claude Code's login in step; returns
    /// the login now kept.
    pub(crate) async fn claude_renew_login(&self, id: &str, rejected: &str) -> Result<Value> {
        let mut renewed = false;
        let result = self
            .claude_refresh_login(id, Some(&mut renewed), Some(rejected))
            .await;
        let account = self
            .claude_load()?
            .accounts
            .into_iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?;
        // Claude Code's newer login, adopted meanwhile, serves as well.
        if !renewed {
            result?;
        }
        Ok(json!({"claudeAiOauth": account.oauth}))
    }
    /// As the Codex proxy: unless the local proxy only hosts the API, the
    /// account it chose becomes the one Claude Code uses.
    async fn claude_follow_proxy(&self, id: &str, auth: &Value) -> Result<()> {
        if self.preferences()?.host_api_only {
            return Ok(());
        }
        let store = self.claude_store_file()?;
        let official = settings_are_official(&self.claude_location());
        let target = match auth.pointer("/copoolProvider/id").and_then(Value::as_str) {
            Some(provider) => {
                if !official && store.settings_provider.as_deref() == Some(provider) {
                    return Ok(());
                }
                // A provider is switched to through any of its cards.
                let Some(card) = store
                    .providers
                    .iter()
                    .find(|p| p.id == provider)
                    .and_then(|p| {
                        p.cached_accounts
                            .iter()
                            .find(|a| a.status.eq_ignore_ascii_case("active"))
                            .map(|a| crate::sub2api::card_id(Pool::Claude, &p.id, a.id))
                    })
                else {
                    return Ok(());
                };
                card
            }
            None => {
                if official && store.current_account_id.as_deref() == Some(id) {
                    return Ok(());
                }
                id.to_owned()
            }
        };
        self.claude_switch_account(&target, KeychainAccess::Background)
            .await
            .map(|_| ())
    }
    /// The pool's official logins for a remote server's daemon, which
    /// renews them as allowed; Sub2api providers stay with the app, as the
    /// Codex servers' do.
    pub(crate) fn claude_remote_payload(&self) -> Result<Vec<u8>> {
        let mut accounts: Vec<StoredAccount> = self
            .claude_proxy_candidates()?
            .into_iter()
            .filter(|a| a.auth_json.get("copoolProvider").is_none())
            .collect();
        for account in &mut accounts {
            account.extra.remove("copoolCurrent");
        }
        if accounts.is_empty() {
            return Err("No usable accounts for remote deployment".into());
        }
        let store = crate::models::AccountsStore {
            accounts,
            ..Default::default()
        };
        serde_json::to_vec_pretty(&store).map_err(|e| e.to_string())
    }
    /// Takes the logins a server's daemon renewed since they were uploaded:
    /// a later generation of a login replaces the pool's, which stays to
    /// fall back on.
    pub(crate) async fn claude_adopt_remote_logins(&self, remote: &Value) -> Result<()> {
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        let mut changed = false;
        for found in remote["accounts"].as_array().into_iter().flatten() {
            let oauth = &found["authJson"]["claudeAiOauth"];
            if let Some(account) = store.accounts.iter_mut().find(|a| {
                found["id"].as_str() == Some(a.id.as_str())
                    && crate::claude_api::newer_login(oauth, &a.oauth)
            }) {
                changed |= account.adopt(oauth, false);
            }
        }
        if changed {
            self.claude_save(&store)?;
        }
        Ok(())
    }
}
