//! The Grok pool's CLIProxyAPI credentials: their usage, asked of Grok as
//! Grok Build asks (`fetch_usage`, the pool's own request and reading), the
//! credential's token put in by CLIProxyAPI.
use super::api::{Network, Reply, API_BASE, CLIENT_VERSION, SETTINGS_PATH};
use crate::{
    claude_api::SIGN_IN_EXPIRED,
    cliproxyapi::{Relay, TOKEN},
    login_store::Asked,
    models::UsageSnapshot,
};

type Response<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send + 'a>>;

/// Grok's API through a credential's relay. CLIProxyAPI renews its
/// credentials itself.
struct Relayed<'a>(&'a Relay<'a>);
impl Network for Relayed<'_> {
    fn api<'a>(&'a self, path: &'a str, token: &'a str, _proxy: &'a str) -> Response<'a, Reply> {
        Box::pin(async move {
            let bearer = format!("Bearer {token}");
            let agent = format!("grok-cli/{CLIENT_VERSION}");
            let headers = [
                ("Authorization", bearer.as_str()),
                ("Accept", "application/json"),
                ("x-grok-client-version", CLIENT_VERSION),
                ("User-Agent", agent.as_str()),
            ];
            let reply = self
                .0
                .send("GET", &format!("{API_BASE}{path}"), &headers, None)
                .await?;
            Ok((reply.status, reply.body))
        })
    }
    fn renew<'a>(&'a self, _refresh_token: &'a str, _proxy: &'a str) -> Response<'a, Reply> {
        Box::pin(async { Err("CLIProxyAPI renews its credentials".into()) })
    }
    fn gateway_models<'a>(
        &'a self,
        _url: &'a str,
        _key: &'a str,
        _proxy: &'a str,
        _insecure: bool,
    ) -> Response<'a, Reply> {
        Box::pin(async { Err("CLIProxyAPI lists its own models".into()) })
    }
}

/// The credential's usage, with the plan its settings name, as the pool's
/// own refresh reads them.
pub(crate) async fn usage(relay: &Relay<'_>, now: i64) -> Result<UsageSnapshot, String> {
    let network = Relayed(relay);
    let mut usage = match super::fetch_usage(&network, TOKEN, "", now).await {
        Asked::Answered(usage) => usage,
        Asked::Refused => return Err(SIGN_IN_EXPIRED.into()),
        Asked::Failed(error) => return Err(error),
    };
    if let Ok((200..=299, body)) = network.api(SETTINGS_PATH, TOKEN, "").await {
        usage.plan_type = super::api::plan(&body).or(usage.plan_type);
    }
    Ok(usage)
}
