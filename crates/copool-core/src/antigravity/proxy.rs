//! Adapts this pool's stored logins and Sub2api gateways to the local proxy.
use super::{login, sub2api, Application, Renewal, Result, Stored};
use crate::{
    models::{Pool, StoredAccount},
    proxy_service::{new_proxy_storage_context, ProxyStorageContext},
};
use serde_json::{json, Value};
use std::sync::Arc;

impl Stored {
    fn proxy_auth(&self) -> Option<Value> {
        Some(json!({
            "accessToken": login::access_token(&self.document)?,
            "project": self.project,
        }))
    }

    fn proxy_account(&self, auth: Value, proxy: &str, current: bool) -> Result<StoredAccount> {
        let mut stored: StoredAccount = serde_json::from_value(json!({
            "id": self.id, "label": self.label, "accountId": self.google_id.as_ref().unwrap_or(&self.id),
            "authJson": auth, "usage": self.usage, "planType": self.plan,
            "addedAt": self.added_at, "updatedAt": self.updated_at, "proxyURL": proxy,
        })).map_err(|error| error.to_string())?;
        if current {
            stored.extra.insert("copoolCurrent".into(), true.into());
        }
        if let Some(choice) = self.renewal {
            stored.set_renewal(choice);
        }
        Ok(stored)
    }

    fn adopt_node_renewal(&mut self, row: &Value, generation: &str) -> bool {
        let auth = &row["authJson"];
        let (Some(access), Some(expiry), Some(lifetime)) = (
            auth["accessToken"].as_str(),
            auth["expiresAt"].as_i64(),
            auth["expiresIn"].as_i64(),
        ) else {
            return false;
        };
        if auth["sourceGeneration"].as_str() != Some(generation)
            || access.is_empty()
            || lifetime <= 0
            || login::access_token(&self.document).as_deref() == Some(access)
            || expiry < login::expiry(&self.document).unwrap_or(0)
        {
            return false;
        }
        login::apply_renewal(
            &mut self.document,
            access,
            expiry,
            lifetime,
            auth["refreshToken"].as_str(),
        );
        if self.renewal == Some(Renewal::Once) && row["renewal"] == "never" {
            self.renewal = Some(Renewal::Never);
        }
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

    fn antigravity_proxy_candidates(&self) -> Result<Vec<StoredAccount>> {
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
            let id = format!("antigravity-sub2api-provider:{}", provider.id);
            candidates.push(
                serde_json::from_value(json!({
                    "id": id, "accountId": id, "label": provider.name, "addedAt": account.id,
                    "updatedAt": 0, "proxyURL": provider.proxy_url, "usage": account.usage,
                    "authJson": {"copoolProvider": {
                        "apiKey": key, "baseUrl": sub2api::gateway(provider)?,
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
        let _operations = self.antigravity.operations.lock().await;
        let _lock = self.antigravity.lock.lock().await;
        let mut store = self.antigravity_load()?;
        let mut changed = false;
        let mut accounts = Vec::new();
        for account in store
            .accounts
            .iter_mut()
            .filter(|a| !a.first_request_pending)
        {
            let Some(refresh) = login::refresh_token(&account.document) else {
                continue;
            };
            let generation = refresh_generation(
                &refresh,
                login::access_token(&account.document)
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
                let refresh = login::refresh_token(&account.document).expect("checked login");
                auth["sourceGeneration"] =
                    refresh_generation(&refresh, auth["accessToken"].as_str().unwrap_or_default())
                        .into();
                auth["refreshToken"] = refresh.into();
            }
            auth["expiresAt"] = login::expiry(&account.document).into();
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
