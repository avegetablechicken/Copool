//! Adapts this pool's stored logins and Sub2api gateways to the local proxy.
use super::{login, sub2api, Application, Result, Stored};
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
            let mut stored: StoredAccount = serde_json::from_value(json!({
                "id": account.id, "label": account.label,
                "accountId": account.google_id.as_ref().unwrap_or(&account.id),
                "authJson": auth, "usage": account.usage, "planType": account.plan,
                "addedAt": account.added_at, "updatedAt": account.updated_at,
                "proxyURL": if account.proxy_url.is_empty() { &shared_proxy } else { &account.proxy_url },
                "copoolCurrent": !api_key_mode && store.current_account_id.as_ref() == Some(&account.id)
            })).map_err(|e| e.to_string())?;
            if let Some(choice) = account.renewal {
                stored.set_renewal(choice);
            }
            candidates.push(stored);
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
