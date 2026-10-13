use crate::{application::Application, auth, repository, utils::now_unix_seconds, workspaces};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
fn normalized(id: &str) -> String {
    id.trim().to_lowercase()
}
pub(crate) fn visible(metadata: &workspaces::Metadata) -> bool {
    let searchable = format!(
        "{} {}",
        metadata.workspace_name.as_deref().unwrap_or_default(),
        metadata.structure.as_deref().unwrap_or_default()
    )
    .to_lowercase();
    metadata
        .workspace_name
        .as_ref()
        .is_some_and(|n| !n.trim().is_empty())
        && ![
            "personal",
            "deactivat",
            "disabl",
            "archiv",
            "suspend",
            "inactive",
            "deleted",
        ]
        .iter()
        .any(|s| searchable.contains(s))
}
impl Application {
    pub async fn accounts_with_metadata(&self) -> Result<Vec<crate::AccountSummary>, String> {
        self.accounts().await?;
        self.reconcile_workspace_names(false).await?;
        self.accounts().await
    }
    pub(crate) async fn reconcile_workspace_names(&self, force: bool) -> Result<(), String> {
        let snapshot = repository::load_accounts(&self.paths.store())?;
        for account in snapshot.accounts {
            if !account.is_available() || account.awaiting_first_request() {
                continue;
            }
            if !force
                && account
                    .extra
                    .get("teamName")
                    .and_then(Value::as_str)
                    .is_some_and(|n| !n.trim().is_empty())
            {
                continue;
            }
            let Ok(extracted) = auth::extract_auth(&account.auth_json) else {
                continue;
            };
            if !["team", "business", "enterprise"].contains(
                &extracted
                    .plan_type
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .as_str(),
            ) {
                continue;
            }
            let Ok(metadata) = self
                .network
                .workspaces(
                    &extracted.access_token,
                    &self.account_proxy(&account.proxy_url)?,
                )
                .await
            else {
                continue;
            };
            let Some(name) = metadata
                .iter()
                .find(|m| {
                    normalized(&m.account_id) == normalized(&account.account_id) && visible(m)
                })
                .and_then(|m| m.workspace_name.as_ref())
            else {
                continue;
            };
            self.mutate_accounts(|store| {
                if let Some(current) = store.accounts.iter_mut().find(|a| a.id == account.id) {
                    current.extra.insert("teamName".into(), name.trim().into());
                }
                Ok(())
            })
            .await?;
            self.publish();
        }
        Ok(())
    }
    pub async fn workspace_directory(&self) -> Result<Vec<Value>, String> {
        let _lock = self.state.store_lock.lock().await;
        Ok(repository::load_accounts(&self.paths.store())?
            .extra
            .get("workspaceDirectory")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }
    pub async fn dismiss_workspace(&self, id: &str) -> Result<(), String> {
        // A blank id matches every row that carries no workspace key.
        if normalized(id).is_empty() {
            return Ok(());
        }
        let _op = self.operations.lock().await;
        self.mutate_accounts(|store| {
            if let Some(rows) = store
                .extra
                .get_mut("workspaceDirectory")
                .and_then(Value::as_array_mut)
            {
                for row in rows {
                    if normalized(row["workspaceId"].as_str().unwrap_or_default()) == normalized(id)
                    {
                        row["visibility"] = "deleted".into();
                    }
                }
            }
            Ok(())
        })
        .await
        .map(|_| ())
    }
    pub async fn sync_workspace_directory(&self) -> Result<Vec<Value>, String> {
        let snapshot = repository::load_accounts(&self.paths.store())?;
        let mut discoveries = BTreeMap::new();
        let mut did_fetch = false;
        for account in &snapshot.accounts {
            if !account.is_available() || account.awaiting_first_request() {
                continue;
            }
            let Ok(extracted) = auth::extract_auth(&account.auth_json) else {
                continue;
            };
            if !["team", "business", "enterprise"].contains(
                &extracted
                    .plan_type
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .as_str(),
            ) {
                continue;
            }
            let metadata = self
                .network
                .workspaces(
                    &extracted.access_token,
                    &self.account_proxy(&account.proxy_url)?,
                )
                .await?;
            did_fetch = true;
            for row in metadata {
                discoveries.entry(normalized(&row.account_id)).or_insert((
                    row,
                    account.email.clone(),
                    account.plan_type.clone(),
                ));
            }
        }
        if !did_fetch {
            return self.workspace_directory().await;
        }
        let _op = self.operations.lock().await;
        let store = self.mutate_accounts(|store| {
            let authorized: BTreeSet<_> = store
                .accounts
                .iter()
                .filter(|a| a.is_available())
                .map(|a| normalized(&a.account_id))
                .collect();
            let mut entries: BTreeMap<String, Value> = store
                .extra
                .get("workspaceDirectory")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|v| (normalized(v["workspaceId"].as_str().unwrap_or_default()), v))
                .collect();
            for (id, (metadata, email, plan)) in &discoveries {
                if authorized.contains(id) || !visible(metadata) {
                    entries.remove(id);
                    continue;
                }
                let old = entries.get(id);
                entries.insert(id.clone(),json!({"workspaceId":metadata.account_id,"workspaceName":metadata.workspace_name,"email":email,"planType":plan,"kind":"workspace","source":"legacyMetadata","status":if old.is_some_and(|v|v["status"]=="deactivated"){"deactivated"}else{"active"},"visibility":old.and_then(|v|v["visibility"].as_str()).unwrap_or("visible"),"lastSeenAt":now_unix_seconds(),"lastStatusCheckedAt":old.and_then(|v|v.get("lastStatusCheckedAt")).cloned()}));
            }
            entries.retain(|id, v| {
                v["status"] == "deactivated"
                    || v["visibility"] == "deleted"
                    || v["source"] == "consent"
                    || discoveries.contains_key(id)
            });
            let mut rows: Vec<_> = entries.into_values().collect();
            rows.sort_by_key(|v| {
                v["workspaceName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_lowercase()
            });
            store.extra.insert("workspaceDirectory".into(), rows.into());
            Ok(())
        })
.await?;
        Ok(store.extra["workspaceDirectory"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }
}
