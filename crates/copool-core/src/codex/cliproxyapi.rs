//! The Codex pool's CLIProxyAPI credentials: their usage, asked of ChatGPT
//! as the pool asks for its own accounts' and read the same way, its idle
//! Plus window confirmed and its reset credits read as theirs are.
use crate::{
    cliproxyapi::{Relay, TOKEN},
    models::UsageSnapshot,
    provider_accounts::Account,
    utils::now_unix_seconds,
};

pub(crate) async fn usage(relay: &Relay<'_>, account: &Account) -> Result<UsageSnapshot, String> {
    // CLIProxyAPI reaches ChatGPT itself, never through Copool's
    // `chatgpt_base_url`.
    let url = crate::usage::usage_urls(&crate::usage::chatgpt_base_origin(None))
        .into_iter()
        .next()
        .ok_or("No ChatGPT usage URL")?;
    let bearer = format!("Bearer {TOKEN}");
    let mut headers = vec![
        ("Authorization", bearer.as_str()),
        ("Accept", "application/json"),
    ];
    if let Some(id) = account.account_id.as_deref() {
        headers.push(("ChatGPT-Account-Id", id));
    }
    let with = |own: &[(&'static str, &'static str)]| {
        let mut all = headers.clone();
        all.extend_from_slice(own);
        all
    };
    let reading = with(&[("User-Agent", "codex-tools-swift/0.1")]);
    let reply = relay.send("GET", &url, &reading, None).await?;
    // As the other pools' credentials: a refused token is a sign-in to
    // renew at CLIProxyAPI.
    if reply.status == 401 {
        return Err(crate::codex::SIGN_IN_EXPIRED.into());
    }
    if !(200..=299).contains(&reply.status) {
        let error = &reply.body["error"];
        let message = error["message"]
            .as_str()
            .or(error.as_str())
            .or(reply.body["detail"].as_str())
            .unwrap_or_default();
        return Err(format!(
            "ChatGPT usage request failed: HTTP {} {message}",
            reply.status
        )
        .trim()
        .to_owned());
    }
    // Each reading is as of its answer, as the pool's own are.
    let first = crate::usage::parse_payload(reply.body, now_unix_seconds())?;
    let confirmation = with(&[
        ("User-Agent", "codex-tools-swift/0.1"),
        ("Cache-Control", "no-cache"),
    ]);
    let confirm = || async {
        let reply = relay.send("GET", &url, &confirmation, None).await.ok()?;
        (200..=299).contains(&reply.status).then_some(())?;
        crate::usage::parse_payload(reply.body, now_unix_seconds()).ok()
    };
    let mut usage = crate::usage::confirm_idle_window(first, confirm).await;
    if let Some(reset_url) = crate::usage::reset_credits_url(&url) {
        let credits = with(&[("OpenAI-Beta", "codex-1"), ("originator", "Codex Desktop")]);
        if let Ok(reply) = relay.send("GET", &reset_url, &credits, None).await {
            if (200..=299).contains(&reply.status) {
                crate::usage::apply_reset_credits(&mut usage, &reply.body);
            }
        }
    }
    Ok(usage)
}
