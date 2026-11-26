//! Adapts this pool's stored logins and Sub2api gateways to the local proxy.
use super::{sub2api, Application, Renewal, Result, Stored};
use crate::pool_login::antigravity::proxy_form;
use crate::{
    models::{Pool, StoredAccount},
    pool_login,
    proxy_service::{new_proxy_storage_context, ProxyStorageContext},
};
use serde_json::{json, Value};
use std::sync::Arc;

impl Stored {
    fn proxy_auth(&self) -> Option<Value> {
        Some(json!({
            "accessToken": pool_login::antigravity::access_token(&self.document)?,
            "project": self.own.project,
        }))
    }

    /// The engine's account for the login in its proxy form, `auth`: named
    /// as [`pool_login::antigravity::proxy_account_id`] says, else by its card.
    fn proxy_account(&self, auth: Value, proxy: &str, current: bool) -> Result<StoredAccount> {
        let account_id = pool_login::antigravity::proxy_account_id(
            &self.document,
            self.own.google_id.as_deref(),
        )
        .unwrap_or_else(|| self.id.clone());
        let mut stored: StoredAccount = serde_json::from_value(json!({
            "id": self.id, "label": self.label, "accountId": account_id,
            "authJson": auth, "usage": self.usage, "planType": self.plan,
            "addedAt": self.added_at, "updatedAt": self.updated_at, "proxyURL": proxy,
        }))
        .map_err(|error| error.to_string())?;
        if current {
            stored.extra.insert("copoolCurrent".into(), true.into());
        }
        if let Some(choice) = self.renewal {
            stored.set_renewal(choice);
        }
        Ok(stored)
    }

    /// Takes a node's renewal of this login, `row`'s proxy form, if it is
    /// one of the login uploaded (`generation`) and later than the one kept.
    fn adopt_node_renewal(&mut self, row: &Value, generation: &str) -> bool {
        let auth = &row["authJson"];
        let (Some(access), Some(expiry), Some(lifetime)) = (
            proxy_form::access_token(auth),
            proxy_form::expires_at(auth),
            proxy_form::lifetime(auth),
        ) else {
            return false;
        };
        if proxy_form::source_generation(auth) != Some(generation)
            || lifetime <= 0
            || pool_login::antigravity::access_token(&self.document).as_deref()
                == Some(access.as_str())
            || expiry < pool_login::antigravity::expiry(&self.document).unwrap_or(0)
        {
            return false;
        }
        pool_login::antigravity::apply_renewal(
            &mut self.document,
            &access,
            expiry,
            lifetime,
            proxy_form::refresh_token(auth).as_deref(),
        );
        self.usage_error = None;
        true
    }
}

impl Application {
    pub(crate) fn antigravity_proxy_storage(&self) -> ProxyStorageContext {
        let mut storage =
            new_proxy_storage_context(self.paths.data_dir.clone(), Default::default(), false);
        storage.pool = Pool::Antigravity;
        storage.key_file = "antigravity-api-proxy.key";
        storage.read_store = false;
        storage.refresh_usage = false;
        storage.prefer_local_selection = true;
        let Some(app) = self.shared() else {
            return storage;
        };
        let pool = Arc::downgrade(&app);
        storage.provider_candidates = Some(Arc::new(move || {
            pool.upgrade()
                .ok_or("Copool is closing")?
                .antigravity_proxy_candidates()
        }));
        let pool = Arc::downgrade(&app);
        storage.renew_candidate = Some(Arc::new(move |id, rejected| {
            let pool = pool.clone();
            Box::pin(async move {
                let app = pool.upgrade().ok_or("Copool is closing")?;
                app.antigravity_refresh_after_rejection(&id, &rejected)
                    .await?;
                app.antigravity_load()?
                    .accounts
                    .iter()
                    .find(|a| a.id == id)
                    .and_then(Stored::proxy_auth)
                    .ok_or_else(|| "Antigravity login unavailable".into())
            })
        }));
        storage
    }

    pub(super) fn antigravity_proxy_candidates(&self) -> Result<Vec<StoredAccount>> {
        let store = self.antigravity_load()?;
        let shared_proxy = self.account_proxy("")?;
        let api_key_mode = sub2api::api_key_mode(&self.antigravity_settings_path());
        let mut candidates = Vec::new();
        for account in store.accounts.iter().filter(|a| !a.first_request_pending) {
            let Some(auth) = account.proxy_auth() else {
                continue;
            };
            let current = !api_key_mode && store.current_account_id.as_ref() == Some(&account.id);
            let proxy = if account.proxy_url.is_empty() {
                &shared_proxy
            } else {
                &account.proxy_url
            };
            candidates.push(account.proxy_account(auth, proxy, current)?);
        }
        let environment = self.antigravity.environment.read().unwrap().clone();
        for provider in &store.providers {
            let Some(account) = provider
                .cached_accounts
                .iter()
                .filter(|a| {
                    provider.imported_account_ids.contains(&a.id)
                        && a.status.eq_ignore_ascii_case("active")
                })
                .max_by(|a, b| {
                    crate::ranking::remaining_score(&a.summary(String::new()))
                        .total_cmp(&crate::ranking::remaining_score(&b.summary(String::new())))
                })
            else {
                continue;
            };
            let Some(key) = environment(&[sub2api::key_variable(provider)], false)
                .into_iter()
                .next()
                .flatten()
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            // A provider not fully set up is no route; the others still are.
            let Ok(gateway) = sub2api::gateway(provider) else {
                continue;
            };
            let id = format!("antigravity-sub2api-provider:{}", provider.id);
            candidates.push(
                serde_json::from_value(json!({
                    "id": id, "accountId": id, "label": provider.name, "addedAt": account.id,
                    "updatedAt": 0, "proxyURL": provider.proxy_url, "usage": account.usage,
                    "authJson": {"copoolProvider": {
                        "apiKey": key, "baseUrl": gateway,
                        "allowInsecureTLS": provider.allow_insecure_tls,
                        "preferred": api_key_mode && store.settings_provider.as_ref() == Some(&provider.id)
                    }}
                }))
                .map_err(|e| e.to_string())?,
            );
        }
        Ok(candidates)
    }
}

/// Identifies the login originally uploaded, even if Google rotated its refresh
/// token on the node. A later local sign-in must survive the next sync.
fn refresh_generation(refresh: &str, access: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(refresh.as_bytes());
    digest.update([0]);
    digest.update(access.as_bytes());
    format!("{:x}", digest.finalize())
}

impl Application {
    pub(crate) async fn antigravity_remote_payload(
        &self,
        remote: Option<&Value>,
    ) -> Result<Vec<u8>> {
        let _operations = self.antigravity.logins.operations.lock().await;
        let _lock = self.antigravity.logins.store.lock().await;
        let mut store = self.antigravity_load()?;
        let mut changed = false;
        let mut accounts = Vec::new();
        for account in store
            .accounts
            .iter_mut()
            .filter(|a| !a.first_request_pending)
        {
            let Some(refresh) = pool_login::antigravity::refresh_token(&account.document) else {
                continue;
            };
            let generation = refresh_generation(
                &refresh,
                pool_login::antigravity::access_token(&account.document)
                    .as_deref()
                    .unwrap_or_default(),
            );
            let found = remote
                .and_then(|store| store["accounts"].as_array())
                .and_then(|rows| {
                    rows.iter()
                        .find(|row| row["id"].as_str() == Some(&account.id))
                });
            if let Some(row) = found {
                changed |= account.adopt_node_renewal(row, &generation);
            }
            let Some(mut auth) = account.proxy_auth() else {
                continue;
            };
            let choice = Renewal::of(account.renewal);
            if choice != Renewal::Never {
                let refresh = pool_login::antigravity::refresh_token(&account.document)
                    .expect("checked login");
                let access = proxy_form::access_token(&auth).unwrap_or_default();
                auth["sourceGeneration"] = refresh_generation(&refresh, &access).into();
                auth["refreshToken"] = refresh.into();
            }
            auth["expiresAt"] = pool_login::antigravity::expiry(&account.document).into();
            accounts.push(account.proxy_account(auth, &account.proxy_url, false)?);
        }
        if changed {
            self.antigravity_save(&store)?;
        }
        if accounts.is_empty() {
            return Err("No usable accounts for remote deployment".into());
        }
        serde_json::to_vec_pretty(&crate::models::AccountsStore {
            accounts,
            ..Default::default()
        })
        .map_err(|e| e.to_string())
    }
}
