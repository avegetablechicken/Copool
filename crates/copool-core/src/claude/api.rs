//! Anthropic's API as Claude Code reads it: a login's OAuth usage, its
//! profile and organizations, its renewal, and the hi that starts its
//! five-hour window.
use super::{hex, Result, HELLO_MODEL};
use crate::{
    claude_api::{refresh_at, request_error, UsageResponse, SIGN_IN_EXPIRED, USER_AGENT},
    pool::Response,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const PROFILE_PATH: &str = "/api/oauth/profile";
const ORGANIZATIONS_PATH: &str = "/api/oauth/organizations";
/// Claude Code 2.1.300 in a terminal session. A headless or background
/// entrypoint (`sdk-cli`, `cli-bg`) is reportedly counted apart from the
/// five-hour window.
pub(super) const INTERACTIVE_USER_AGENT: &str = "claude-cli/2.1.300 (external, cli)";
pub(crate) trait Network: Send + Sync {
    /// The HTTP status and body of the OAuth usage endpoint, with the seconds
    /// of a `Retry-After` header.
    fn usage<'a>(
        &'a self,
        api: &'a str,
        token: &'a str,
        agent: &'a str,
        proxy: &'a str,
    ) -> Response<'a, UsageResponse>;
    /// A token response for a refresh token, asking again for `scope`.
    fn refresh<'a>(
        &'a self,
        refresh_token: &'a str,
        scope: &'a str,
        proxy: &'a str,
    ) -> Response<'a, Value>;
    fn profile<'a>(&'a self, api: &'a str, token: &'a str, proxy: &'a str) -> Response<'a, Value>;
    /// The organization's prepaid balance, which pays for extra usage.
    fn prepaid<'a>(
        &'a self,
        api: &'a str,
        token: &'a str,
        org: &'a str,
        proxy: &'a str,
    ) -> Response<'a, Value>;
    /// The HTTP status and body of a one-token "hi" to the messages `url`,
    /// as Claude Code says it in a terminal session of the `account` (its
    /// UUID).
    fn hello<'a>(
        &'a self,
        url: &'a str,
        token: &'a str,
        account: &'a str,
        proxy: &'a str,
    ) -> Response<'a, (u16, Value)>;
}
pub(super) struct Http;
fn client(proxy: &str) -> Result<reqwest::Client> {
    crate::proxy_session::client(
        proxy,
        crate::proxy_session::Options {
            user_agent: USER_AGENT.into(),
            ..Default::default()
        },
    )
}
impl Network for Http {
    fn usage<'a>(
        &'a self,
        api: &'a str,
        token: &'a str,
        agent: &'a str,
        proxy: &'a str,
    ) -> Response<'a, UsageResponse> {
        Box::pin(crate::claude_api::request_usage(api, token, agent, proxy))
    }
    fn refresh<'a>(
        &'a self,
        refresh_token: &'a str,
        scope: &'a str,
        proxy: &'a str,
    ) -> Response<'a, Value> {
        Box::pin(refresh_at(
            crate::claude_api::TOKEN_URL,
            refresh_token,
            scope,
            proxy,
        ))
    }
    fn profile<'a>(&'a self, api: &'a str, token: &'a str, proxy: &'a str) -> Response<'a, Value> {
        Box::pin(async move {
            let response = client(proxy)?
                .get(format!("{api}{PROFILE_PATH}"))
                .bearer_auth(token.trim())
                .header("Accept", "application/json")
                .header("anthropic-beta", "oauth-2025-04-20")
                .send()
                .await
                .map_err(request_error)?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                return Err(SIGN_IN_EXPIRED.into());
            }
            if !response.status().is_success() {
                return Err(format!(
                    "Claude profile request failed: HTTP {}",
                    response.status().as_u16()
                ));
            }
            response.json().await.map_err(request_error)
        })
    }
    fn prepaid<'a>(
        &'a self,
        api: &'a str,
        token: &'a str,
        org: &'a str,
        proxy: &'a str,
    ) -> Response<'a, Value> {
        Box::pin(async move {
            let response = client(proxy)?
                .get(format!("{api}{ORGANIZATIONS_PATH}/{org}/prepaid/credits"))
                .bearer_auth(token.trim())
                .header("Accept", "application/json")
                .header("x-organization-uuid", org)
                .header("anthropic-beta", "oauth-2025-04-20")
                .header("anthropic-version", "2023-06-01")
                .send()
                .await
                .map_err(request_error)?;
            if !response.status().is_success() {
                return Err(format!(
                    "Claude prepaid credits request failed: HTTP {}",
                    response.status().as_u16()
                ));
            }
            response.json().await.map_err(request_error)
        })
    }
    fn hello<'a>(
        &'a self,
        url: &'a str,
        token: &'a str,
        account: &'a str,
        proxy: &'a str,
    ) -> Response<'a, (u16, Value)> {
        Box::pin(async move {
            // A subscription login is served only Claude Code's requests; its
            // terminal sessions name the session and the device, which stands
            // here for the account.
            let session = uuid::Uuid::new_v4().to_string();
            let device = hex(&Sha256::digest(format!("copool:{account}")));
            let user = json!({"device_id": device, "account_uuid": account, "session_id": session});
            let response = client(proxy)?
                .post(url)
                .bearer_auth(token.trim())
                .header(reqwest::header::USER_AGENT, INTERACTIVE_USER_AGENT)
                .header("Accept", "application/json")
                .header("anthropic-beta", "claude-code-20250219,oauth-2025-04-20")
                .header("anthropic-version", "2023-06-01")
                .header("x-app", "cli")
                .header("X-Claude-Code-Session-Id", &session)
                .json(&json!({
                    "model": HELLO_MODEL,
                    "max_tokens": 1,
                    // Sonnet 5.5 refuses thinking turned off; the least effort
                    // costs least.
                    "output_config": {"effort": "low"},
                    "system": "You are Claude Code, Anthropic's official CLI for Claude.",
                    "metadata": {"user_id": user.to_string()},
                    "messages": [{"role": "user", "content": "hi"}],
                }))
                .send()
                .await
                .map_err(request_error)?;
            let status = response.status().as_u16();
            Ok((status, response.json().await.unwrap_or(Value::Null)))
        })
    }
}
