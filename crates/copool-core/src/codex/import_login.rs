use crate::{
    codex::{api, CodexLogins},
    login_store::LoginPool,
    pool_login::codex,
    utils::now_unix_seconds,
    Application,
};
use serde_json::Value;
impl Application {
    /// Stores a browser sign-in's login without switching to it, with the
    /// workspaces its consent named.
    pub(crate) async fn codex_import_login(
        &self,
        document: Value,
        workspace: Option<&str>,
    ) -> Result<usize, String> {
        let consent = document
            .get("consentWorkspaces")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        self.import_authorized(document, workspace, &consent).await
    }
    /// Complete a single authenticated import without selecting it as current.
    pub async fn import_authorized(
        &self,
        document: Value,
        workspace: Option<&str>,
        consent: &[Value],
    ) -> Result<usize, String> {
        let document = codex::normalized_auth_for_write(document)?;
        let extracted = codex::extract_auth(&document)?;
        if workspace.is_some_and(|id| id != extracted.account_id) {
            return Err("error.oauth.workspace_mismatch".into());
        }
        let same = |a: &crate::codex::Stored| CodexLogins::same_login(&a.document, &document);
        let store = self.codex_load()?;
        let existing = store.accounts.iter().find(|a| same(a)).cloned();
        // A new account waits for a manual refresh unless the user opted in.
        let contact = existing.as_ref().is_some_and(|a| !a.first_request_pending)
            || (existing.is_none() && self.preferences()?.request_new_accounts);
        self.codex_logins()
            .import(vec![document.clone()], crate::login_store::Origin::SignIn)
            .await?;
        let account = self
            .codex_load()?
            .accounts
            .into_iter()
            .find(|a| same(a))
            .ok_or("The login was not imported")?;
        if contact {
            if let Err(error) = self.codex_refresh(&account.id, None).await {
                if api::deactivated(&error) {
                    // A deactivated workspace joins only as seen.
                    if existing.is_none() {
                        self.codex_logins()
                            .change(|store| {
                                store.accounts.retain(|a| a.id != account.id);
                                if store.current_account_id.as_deref() == Some(account.id.as_str())
                                {
                                    store.current_account_id = None;
                                }
                                Ok(())
                            })
                            .await?;
                    }
                    self.mark_pending_deactivated(&extracted.account_id).await?;
                    return Err("error.accounts.workspace_deactivated".into());
                }
            }
        }
        let mut name = workspace
            .and_then(|id| {
                store
                    .workspace_directory
                    .iter()
                    .find(|v| v["workspaceId"] == id)
                    .and_then(|v| v["workspaceName"].as_str())
                    .map(str::to_owned)
            })
            .or(extracted.team_name.clone());
        if contact
            && workspace.is_none()
            && ["team", "business", "enterprise"].contains(
                &extracted
                    .plan_type
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .as_str(),
            )
        {
            let token = self
                .codex_load()?
                .accounts
                .into_iter()
                .find(|a| a.id == account.id)
                .and_then(|a| codex::extract_auth(&a.document).ok())
                .map_or(extracted.access_token.clone(), |e| e.access_token);
            let proxy = self.account_proxy(&account.proxy_url)?;
            if let Ok(rows) = self.network.workspaces(&token, &proxy).await {
                if let Some(row) = rows.iter().find(|row| {
                    row.account_id.eq_ignore_ascii_case(&extracted.account_id)
                        && crate::codex::workspace_directory::visible(row)
                }) {
                    name = row.workspace_name.clone();
                }
            }
        }
        let now = now_unix_seconds();
        self.codex_logins()
            .change(|store| {
                if let Some(name) = name {
                    if let Some(kept) = store.accounts.iter_mut().find(|a| a.id == account.id) {
                        kept.own.team_name = Some(name);
                    }
                }
                let authorized: Vec<_> = store
                    .accounts
                    .iter()
                    .filter(|a| a.standing.is_none())
                    .map(|a| a.own.workspace_id.to_lowercase())
                    .collect();
                let rows = &mut store.workspace_directory;
                // The authorized workspace is no longer only seen.
                rows.retain(|v| {
                    !authorized.contains(
                        &v["workspaceId"]
                            .as_str()
                            .unwrap_or_default()
                            .trim()
                            .to_lowercase(),
                    )
                });
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
            })
            .await?;
        Ok(1)
    }
    async fn mark_pending_deactivated(&self, id: &str) -> Result<(), String> {
        self.codex_logins()
            .change(|store| {
                if let Some(row) = store
                    .workspace_directory
                    .iter_mut()
                    .find(|v| v["workspaceId"] == id)
                {
                    row["status"] = "deactivated".into();
                    row["lastStatusCheckedAt"] = now_unix_seconds().into();
                }
                Ok(())
            })
            .await
    }
}
