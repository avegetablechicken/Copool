//! The Antigravity pool's CLIProxyAPI credentials: their quota, asked of
//! Cloud Code as the pool asks for its own accounts', through the same
//! requests and readings (`antigravity_cloud`), and their hi, said as the
//! pool's own accounts' is (`cloud_hello`), the credential's token put in by
//! CLIProxyAPI.
use crate::{
    antigravity_cloud::{self, Failure, Network, Reply, CLOUD_BASES},
    claude_api::SIGN_IN_EXPIRED,
    cliproxyapi::{Relay, TOKEN},
    models::UsageSnapshot,
    pool::Hello,
    provider_accounts::Account,
};
use serde_json::Value;

type Response<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send + 'a>>;

/// Cloud Code through a credential's relay. CLIProxyAPI renews its
/// credentials itself.
struct Relayed<'a>(&'a Relay<'a>);
impl Network for Relayed<'_> {
    fn cloud<'a>(
        &'a self,
        path: &'a str,
        token: &'a str,
        body: Value,
        agent: &'a str,
        _proxy: &'a str,
    ) -> Response<'a, Reply> {
        Box::pin(async move {
            let bearer = format!("Bearer {token}");
            let headers = [
                ("Authorization", bearer.as_str()),
                ("Accept", "application/json"),
                ("Content-Type", "application/json"),
                ("User-Agent", agent),
            ];
            // As the pool's own requests: the daily endpoint first.
            let mut last = Err("Antigravity request failed".to_owned());
            for base in CLOUD_BASES {
                match self
                    .0
                    .send("POST", &format!("{base}{path}"), &headers, Some(&body))
                    .await
                {
                    Ok(reply) if reply.status < 500 && reply.status != 404 => {
                        return Ok((reply.status, reply.body))
                    }
                    Ok(reply) => last = Ok((reply.status, reply.body)),
                    Err(error) => last = Err(error),
                }
            }
            last
        })
    }
    fn renew<'a>(&'a self, _refresh_token: &'a str, _proxy: &'a str) -> Response<'a, Reply> {
        Box::pin(async { Err("CLIProxyAPI renews its credentials".into()) })
    }
    fn identity<'a>(&'a self, _token: &'a str, _proxy: &'a str) -> Response<'a, Reply> {
        Box::pin(async { Err("CLIProxyAPI names its credentials".into()) })
    }
}

pub(crate) async fn usage(relay: &Relay<'_>, now: i64) -> Result<UsageSnapshot, String> {
    antigravity_cloud::fetch_usage(&Relayed(relay), TOKEN, "", now)
        .await
        .map_err(|failure| match failure {
            Failure::Expired => SIGN_IN_EXPIRED.into(),
            Failure::Denied(error) | Failure::Other(error) => error,
        })
}

/// Says hi with the credential through its relay, to the windows a hi
/// starts, as the pool's own accounts do.
pub(crate) async fn hello(
    relay: &Relay<'_>,
    account: &Account,
    only: Option<bool>,
) -> Result<(), String> {
    match super::cloud_hello(
        &Relayed(relay),
        TOKEN,
        "",
        None,
        account.usage.as_ref(),
        only,
    )
    .await?
    {
        Hello::Said => Ok(()),
        Hello::Expired(_) => Err(SIGN_IN_EXPIRED.into()),
    }
}
