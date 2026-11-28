//! CLIProxyAPI providers, whose OAuth credentials join a pool as cards: one
//! client of its management API (v8) for every pool. It lists the
//! credentials, relays each one's usage requests to the pool's service
//! (`/requests/api-call`, which puts the credential's token where a header
//! says `$TOKEN$`), so a card reads its usage as the pool's own accounts do,
//! and says hi through one (`/credentials/verify`, an inference on that
//! credential alone). What every kind of provider does with its cards is
//! `provider_accounts.rs`'s; what a pool asks its service is in its
//! `<pool>/cliproxyapi.rs`.
use crate::{
    models::UsageSnapshot, pool::Pool, provider_accounts::Account, provider_registry::strip_suffix,
    proxy_session, usage,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(crate) mod shared;

type Result<T> = std::result::Result<T, String>;

/// Where a relayed request's header carries the credential's token.
pub(crate) const TOKEN: &str = "$TOKEN$";
/// The management API under a CLIProxyAPI address.
const MANAGEMENT_PATH: &str = "/v8/management";

/// The management API root (`…/v8/management`) of a CLIProxyAPI address: its
/// own, a management API's, or a gateway's under it.
pub(crate) fn management_base(raw: &str) -> Result<String> {
    let base = strip_suffix(raw, &["/v8/management", "/v0/management", "/v1beta", "/v1"]);
    let url = reqwest::Url::parse(&base).map_err(|_| "error.sub2api.invalid_base_url")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("error.sub2api.invalid_base_url".into());
    }
    Ok(format!("{base}{MANAGEMENT_PATH}"))
}
/// The CLIProxyAPI provider whose credentials a pool shows.
fn provider(pool: Pool) -> &'static str {
    match pool {
        Pool::Codex => "codex",
        Pool::Claude => "claude",
        Pool::Antigravity => "antigravity",
        Pool::Grok => "xai",
    }
}
/// The account a credential is, by its `auth_index`: the same one every
/// time, and a positive number as a Sub2api account's id is, so that its
/// card's id names it as every provider card's does.
pub(crate) fn account_id(credential: &str) -> i64 {
    let digest = Sha256::digest(credential.trim().as_bytes());
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    (u64::from_be_bytes(first) >> 1) as i64
}
/// A credential's text field, when it has one.
fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
/// The credential's account as listed, without its usage; none for a
/// credential of another provider, a disabled one, or one without an index.
fn listed(pool: Pool, item: &Value, provider_id: &str) -> Option<Account> {
    let kind = text(&item["provider"]).or_else(|| text(&item["type"]))?;
    if !kind.eq_ignore_ascii_case(provider(pool))
        || item["disabled"] == true
        || text(&item["status"]).is_some_and(|s| s.eq_ignore_ascii_case("disabled"))
    {
        return None;
    }
    let credential = text(&item["auth_index"])?;
    let name = text(&item["label"])
        .or_else(|| text(&item["name"]))
        .unwrap_or_default();
    let email = text(&item["email"])
        .or_else(|| text(&item["account"]).filter(|a| a.contains('@')))
        .or_else(|| {
            text(&item["name"])
                .map(|n| n.trim_end_matches(".json").to_owned())
                .filter(|n| n.contains('@'))
        });
    Some(Account {
        id: account_id(&credential),
        name,
        email,
        account_id: text(&item["id_token"]["chatgpt_account_id"]),
        account_type: "oauth".into(),
        status: "active".into(),
        plan_type: text(&item["id_token"]["plan_type"]),
        usage: None,
        // A credential cooling down or refused says why.
        usage_error: text(&item["status_message"])
            .filter(|_| item["unavailable"] == true || item["status"] == "error"),
        provider_id: Some(provider_id.to_owned()),
        credential: Some(credential),
        outdated_version: None,
    })
}
/// The error of a management API answer.
fn refusal(status: reqwest::StatusCode, body: &Value) -> String {
    if status == reqwest::StatusCode::NOT_FOUND && body.get("error").is_none() {
        // Its routes are not served without a management key, or in Home mode.
        return "error.cliproxyapi.management_unavailable".into();
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return "error.cliproxyapi.management_key_refused".into();
    }
    text(&body["error"])
        .or_else(|| text(&body["message"]))
        .unwrap_or_else(|| format!("CLIProxyAPI request failed: {status}"))
}

/// A relayed request's answer from the pool's service.
pub(crate) struct Relayed {
    pub(crate) status: u16,
    /// Its headers, each name's values as a list.
    pub(crate) headers: Value,
    /// Its JSON body; null when it has none or another.
    pub(crate) body: Value,
}
impl Relayed {
    /// The first value of a header, by its name in any case.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.as_object()?.iter().find_map(|(key, values)| {
            key.eq_ignore_ascii_case(name)
                .then(|| values.get(0).and_then(Value::as_str).or(values.as_str()))
                .flatten()
        })
    }
}
/// How many credentials' usage a refresh reads at once: an idle Codex
/// window's confirmation waits a second.
const RELAYS_AT_ONCE: usize = 4;
/// A credential's requests to the pool's service, through CLIProxyAPI.
pub(crate) struct Relay<'a> {
    connection: &'a Connection,
    client: &'a reqwest::Client,
    credential: &'a str,
}
impl Relay<'_> {
    /// Sends a request as the credential, its token where a header says
    /// [`TOKEN`]; through the credential's proxy, as CLIProxyAPI sends it.
    pub(crate) async fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Relayed> {
        let header: serde_json::Map<String, Value> = headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), Value::from(*value)))
            .collect();
        let mut request = json!({
            "auth_index": self.credential,
            "method": method,
            "url": url,
            "header": header,
        });
        if let Some(body) = body {
            request["data"] = body.to_string().into();
        }
        let answer = self
            .connection
            .request(
                self.client,
                reqwest::Method::POST,
                "/requests/api-call",
                Some(&request),
            )
            .await?;
        let status = answer["status_code"]
            .as_u64()
            .and_then(|s| u16::try_from(s).ok())
            .ok_or("Invalid CLIProxyAPI relay response")?;
        let body = answer["body"]
            .as_str()
            .and_then(|b| serde_json::from_str(b).ok())
            .unwrap_or(Value::Null);
        Ok(Relayed {
            status,
            headers: answer["header"].clone(),
            body,
        })
    }
}

/// Where Copool reaches a CLIProxyAPI provider's management API.
pub(crate) struct Connection {
    /// The management API root, `…/v8/management`.
    pub(crate) management: String,
    pub(crate) key: String,
    /// The proxy of Copool's requests to CLIProxyAPI: blank for the system
    /// settings, "direct://" for none.
    pub(crate) proxy_url: String,
    pub(crate) insecure: bool,
    /// The provider as its accounts name it.
    pub(crate) provider_id: String,
    /// The accounts as the provider last cached them.
    pub(crate) cached: Vec<Account>,
}
impl Connection {
    fn client(&self) -> Result<reqwest::Client> {
        proxy_session::client(
            &self.proxy_url,
            proxy_session::Options {
                insecure: self.insecure,
                ..Default::default()
            },
        )
    }
    /// A management API request with the key; its JSON answer.
    async fn request(
        &self,
        client: &reqwest::Client,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value> {
        let mut request = client
            .request(method, format!("{}{path}", self.management))
            .bearer_auth(&self.key)
            .header("Accept", "application/json");
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|e| usage::format_reqwest_error(&e))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| usage::format_reqwest_error(&e))?;
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(refusal(status, &value));
        }
        if value.is_null() {
            return Err("Invalid CLIProxyAPI response".into());
        }
        Ok(value)
    }
    fn cached_account(&self, id: i64) -> Option<Account> {
        self.cached.iter().find(|a| a.id == id).cloned()
    }
    /// The pool's credentials by `ids` (every one without), with their
    /// usage, read a few at once. A credential CLIProxyAPI reports cooling
    /// down or refused shows why, though its usage is read.
    pub(crate) async fn fetch_accounts(
        &self,
        pool: Pool,
        ids: Option<&[i64]>,
        now: i64,
    ) -> Result<Vec<Account>> {
        use futures_util::StreamExt;
        let client = self.client()?;
        let listing = self
            .request(&client, reqwest::Method::GET, "/credentials", None)
            .await?;
        let files = listing["files"]
            .as_array()
            .ok_or("Invalid CLIProxyAPI credential list")?;
        let asked: Vec<Account> = files
            .iter()
            .filter_map(|item| listed(pool, item, &self.provider_id))
            .filter(|a| ids.is_none_or(|ids| ids.contains(&a.id)))
            .collect();
        let client = &client;
        Ok(futures_util::stream::iter(asked)
            .map(|mut account| async move {
                let relay = Relay {
                    connection: self,
                    client,
                    credential: account.credential.as_deref().unwrap_or_default(),
                };
                match relayed_usage(pool, &relay, &account, now).await {
                    Ok(mut usage) => {
                        usage.plan_type = usage.plan_type.or(account.plan_type);
                        account.plan_type = usage.plan_type.clone();
                        account.usage = Some(usage);
                    }
                    Err(error) => account.usage_error = Some(error),
                }
                account.keep_last(self.cached_account(account.id));
                account
            })
            .buffered(RELAYS_AT_ONCE)
            .collect()
            .await)
    }
    /// Says hi through one credential: CLIProxyAPI's inference check of it,
    /// with its own request for the credential's service; an Antigravity
    /// credential's as the pool's own accounts' is (`third_party` as
    /// `say_hi_to` takes it), through its relay, as the check sends one
    /// model alone.
    pub(crate) async fn say_hi(
        &self,
        pool: Pool,
        account: &Account,
        third_party: Option<bool>,
    ) -> Result<()> {
        let credential = account.credential.as_deref().ok_or("Account not found")?;
        let client = self.client()?;
        if pool == Pool::Antigravity {
            let relay = Relay {
                connection: self,
                client: &client,
                credential,
            };
            return crate::antigravity::cliproxyapi::hello(&relay, account, third_party).await;
        }
        let answer = self
            .request(
                &client,
                reqwest::Method::POST,
                "/credentials/verify",
                Some(&json!({"auth_index": credential})),
            )
            .await?;
        if answer["status"] == "ok" {
            return Ok(());
        }
        Err(text(&answer["error"]).unwrap_or_else(|| "CLIProxyAPI did not say hi".into()))
    }
}

/// The credential's usage, as the pool asks its service.
async fn relayed_usage(
    pool: Pool,
    relay: &Relay<'_>,
    account: &Account,
    now: i64,
) -> Result<UsageSnapshot> {
    match pool {
        Pool::Codex => crate::codex::cliproxyapi::usage(relay, account).await,
        Pool::Claude => crate::claude::cliproxyapi::usage(relay).await,
        Pool::Antigravity => crate::antigravity::cliproxyapi::usage(relay, now).await,
        Pool::Grok => crate::grok::cliproxyapi::usage(relay, now).await,
    }
}

#[cfg(test)]
mod tests;
