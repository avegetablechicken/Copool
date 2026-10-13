use crate::{models::UsageSnapshot, workspaces::Metadata};
use serde_json::Value;
use std::{future::Future, pin::Pin};
pub(crate) type Response<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;
pub(crate) trait AccountNetwork: Send + Sync {
    fn usage<'a>(&'a self, auth: &'a Value, proxy: &'a str) -> Response<'a, UsageSnapshot>;
    fn refresh<'a>(&'a self, auth: &'a Value, proxy: &'a str) -> Response<'a, Value>;
    fn workspaces<'a>(&'a self, token: &'a str, proxy: &'a str) -> Response<'a, Vec<Metadata>>;
}
/// Requests follow `chatgpt_base_url` in Copool's Codex configuration, as
/// the proxy does.
pub(crate) struct Network {
    pub(crate) codex_config: std::path::PathBuf,
}
impl Network {
    pub(crate) fn origin(&self) -> String {
        crate::usage::chatgpt_base_origin(
            crate::codex_config::chatgpt_base_url(&self.codex_config).as_deref(),
        )
    }
}
impl AccountNetwork for Network {
    fn usage<'a>(&'a self, auth: &'a Value, proxy: &'a str) -> Response<'a, UsageSnapshot> {
        Box::pin(async move {
            let extracted = crate::auth::extract_auth(auth)?;
            crate::usage::fetch_usage_snapshot_from(
                &self.origin(),
                &extracted.access_token,
                &extracted.account_id,
                proxy,
            )
            .await
        })
    }
    fn refresh<'a>(&'a self, auth: &'a Value, proxy: &'a str) -> Response<'a, Value> {
        Box::pin(crate::auth::refresh_chatgpt_auth_tokens(auth, proxy))
    }
    fn workspaces<'a>(&'a self, token: &'a str, proxy: &'a str) -> Response<'a, Vec<Metadata>> {
        Box::pin(async move {
            let client = crate::proxy_session::client(proxy, Default::default())?;
            let base = self.origin();
            let mut endpoints = vec![
                format!(
                    "{}/accounts",
                    if base.ends_with("/backend-api") {
                        base
                    } else {
                        format!("{base}/backend-api")
                    }
                ),
                "https://chatgpt.com/backend-api/accounts".into(),
            ];
            endpoints.dedup();
            crate::workspaces::Client::new(client, endpoints)
                .fetch(token)
                .await
        })
    }
}
pub(crate) fn expired_error(error: &str) -> bool {
    let error = error.to_lowercase();
    [
        "provided authentication token is expired",
        "token_expired",
        "refresh token has already been used",
        "signing in again",
    ]
    .iter()
    .any(|v| error.contains(v))
}
pub(crate) fn deactivated(error: &str) -> bool {
    let error = error.to_lowercase();
    [
        "workspace_deactivated",
        "account_deactivated",
        "workspace is deactivated",
        "workspace has been deactivated",
        "organization has been disabled",
        "workspace has been disabled",
    ]
    .iter()
    .any(|v| error.contains(v))
}
/// A failed request still reports the login it used: a refresh that succeeded
/// before the failure already spent the caller's copy of the tokens.
pub(crate) struct FetchFailure {
    pub auth: Value,
    pub message: String,
}

pub(crate) async fn fetch(
    network: &dyn AccountNetwork,
    original: &Value,
    proxy: &str,
    now: i64,
) -> Result<(Value, UsageSnapshot), FetchFailure> {
    let mut auth = original.clone();
    let expired = crate::auth::access_token_expiry(&auth).is_some_and(|exp| exp <= now);
    if expired {
        auth = match network.refresh(&auth, proxy).await {
            Ok(refreshed) => refreshed,
            Err(message) => return Err(FetchFailure { auth, message }),
        };
    }
    match network.usage(&auth, proxy).await {
        Ok(usage) => Ok((auth, usage)),
        Err(error) if expired_error(&error) => {
            auth = match network.refresh(&auth, proxy).await {
                Ok(refreshed) => refreshed,
                Err(message) => return Err(FetchFailure { auth, message }),
            };
            match network.usage(&auth, proxy).await {
                Ok(usage) => Ok((auth, usage)),
                Err(message) => Err(FetchFailure { auth, message }),
            }
        }
        Err(message) => Err(FetchFailure { auth, message }),
    }
}
