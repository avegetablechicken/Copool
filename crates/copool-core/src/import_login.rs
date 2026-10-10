use crate::{
    account_network, auth, models::StoredAccount, repository, utils::now_unix_seconds, Application,
};
use serde_json::{json, Value};
impl Application {
    /// Complete a single authenticated import without selecting it as current.
    pub async fn import_authorized(
        &self,
        document: Value,
        workspace: Option<&str>,
        consent: &[Value],
    ) -> Result<usize, String> {
        let mut document = auth::normalized_auth_for_write(document)?;
        let mut extracted = auth::extract_auth(&document)?;
        if workspace.is_some_and(|id| id != extracted.account_id) {
            return Err("error.oauth.workspace_mismatch".into());
        }
        let identity = auth::account_identity(&document)?;
        let store = repository::load_accounts(&self.paths.store())?;
        let proxy = store
            .accounts
            .iter()
            .find(|a| auth::account_identity(&a.auth_json).ok().as_ref() == Some(&identity))
            .map(|a| a.proxy_url.clone())
            .unwrap_or_default();
        let (usage, usage_error) = match account_network::fetch(
            self.network.as_ref(),
            &document,
            &proxy,
            now_unix_seconds(),
        )
        .await
        {
            Ok((refreshed, usage)) => {
                document = refreshed;
                extracted = auth::extract_auth(&document)?;
                (Some(usage), None)
            }
            Err(error) if account_network::deactivated(&error) => {
                self.mark_pending_deactivated(&extracted.account_id)?;
                return Err("error.accounts.workspace_deactivated".into());
            }
            Err(error) => (None, Some(error)),
        };
        let mut name = workspace
            .and_then(|id| {
                store
                    .extra
                    .get("workspaceDirectory")
                    .and_then(Value::as_array)
                    .and_then(|rows| rows.iter().find(|v| v["workspaceId"] == id))
                    .and_then(|v| v["workspaceName"].as_str())
                    .map(str::to_owned)
            })
            .or(extracted.team_name.clone());
        if workspace.is_none()
            && ["team", "business", "enterprise"].contains(
                &extracted
                    .plan_type
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .as_str(),
            )
        {
            if let Ok(rows) = self
                .network
                .workspaces(&extracted.access_token, &proxy)
                .await
            {
                if let Some(row) = rows.iter().find(|row| {
                    row.account_id.eq_ignore_ascii_case(&extracted.account_id)
                        && crate::workspace_directory::visible(row)
                }) {
                    name = row.workspace_name.clone();
                }
            }
        }
        let now = now_unix_seconds();
        let mut account:StoredAccount=serde_json::from_value(json!({"id":uuid::Uuid::new_v4().to_string(),"label":extracted.email.clone().unwrap_or(extracted.account_id.clone()),"email":extracted.email,"accountId":extracted.account_id,"planType":extracted.plan_type,"authJson":document,"addedAt":now,"updatedAt":now,"usage":usage,"usageError":usage_error,"teamName":name,"usageStateUpdatedAt":now,"workspaceStatus":"active","displayStatus":"list"})).map_err(|e|e.to_string())?;
        account.proxy_url = proxy;
        let count = self
            .import_json(&serde_json::to_string(&account).map_err(|e| e.to_string())?)
            .await?;
        if !consent.is_empty() {
            let _op = self.operations.lock().await;
            repository::mutate_accounts(&self.paths.store(), |store| {
                let authorized: Vec<_> = store
                    .accounts
                    .iter()
                    .filter(|a| a.is_available())
                    .map(|a| a.account_id.to_lowercase())
                    .collect();
                let rows = store
                    .extra
                    .entry("workspaceDirectory")
                    .or_insert(json!([]))
                    .as_array_mut()
                    .ok_or("Invalid workspace directory")?;
                for item in consent {
                    let Some(id) = item["workspaceId"]
                        .as_str()
                        .or_else(|| item["workspaceID"].as_str())
                    else {
                        continue;
                    };
                    if authorized.contains(&id.to_lowercase()) {
                        continue;
                    }
                    let mut entry = item.clone();
                    entry["workspaceId"] = id.into();
                    entry["source"] = "consent".into();
                    entry["status"] = "active".into();
                    entry["visibility"] = "visible".into();
                    entry["lastSeenAt"] = now.into();
                    if let Some(old) = rows.iter_mut().find(|v| v["workspaceId"] == id) {
                        if old["visibility"] == "deleted" {
                            entry["visibility"] = "deleted".into();
                        }
                        *old = entry;
                    } else {
                        rows.push(entry);
                    }
                }
                Ok(())
            })?;
        }
        self.publish();
        Ok(count)
    }
    fn mark_pending_deactivated(&self, id: &str) -> Result<(), String> {
        repository::mutate_accounts(&self.paths.store(), |store| {
            if let Some(rows) = store
                .extra
                .get_mut("workspaceDirectory")
                .and_then(Value::as_array_mut)
            {
                if let Some(row) = rows.iter_mut().find(|v| v["workspaceId"] == id) {
                    row["status"] = "deactivated".into();
                    row["lastStatusCheckedAt"] = now_unix_seconds().into();
                }
            }
            Ok(())
        })
        .map(|_| ())
    }
}
