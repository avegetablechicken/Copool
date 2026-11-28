//! The Claude pool's CLIProxyAPI credentials: their usage, asked of
//! Anthropic as Claude Code asks, and read as the pool reads its own.
use crate::{
    claude_api::{self, OAUTH_BETA, USAGE_USER_AGENT, USER_AGENT},
    cliproxyapi::{Relay, TOKEN},
    models::UsageSnapshot,
    utils::now_unix_seconds,
};

pub(crate) async fn usage(relay: &Relay<'_>) -> Result<UsageSnapshot, String> {
    let url = format!("{}{}", claude_api::API_ROOT, claude_api::USAGE_PATH);
    let bearer = format!("Bearer {TOKEN}");
    let ask = |agent: &'static str| {
        let headers = [
            ("Authorization", bearer.as_str()),
            ("Accept", "application/json"),
            ("anthropic-beta", OAUTH_BETA),
            ("User-Agent", agent),
        ];
        let url = url.clone();
        async move { relay.send("GET", &url, &headers, None).await }
    };
    // As `claude-cli/…`, else as `claude-code/…` when that agent is refused.
    let mut reply = ask(USAGE_USER_AGENT).await?;
    if reply.status == 429 {
        reply = ask(USER_AGENT).await?;
    }
    let clock_offset = reply
        .header("Date")
        .and_then(|date| claude_api::clock_offset_seconds(date, now_unix_seconds()));
    claude_api::usage_reply(reply.status, &reply.body, None, clock_offset)
}
