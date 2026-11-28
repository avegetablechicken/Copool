//! Sub2api providers, whose OAuth accounts join a pool as cards: one
//! client for every pool. What every kind of provider does with its cards
//! is `provider_accounts.rs`'s.
use crate::provider_accounts::Account;
use crate::{codex::config::Definition, pool::Pool, proxy_session, settings::Provider, usage};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};
/// How long a login without `expires_in` is reused.
const DEFAULT_TOKEN_LIFETIME: Duration = Duration::from_secs(15 * 60);
/// Logs in again this long before a token expires.
const TOKEN_EXPIRY_MARGIN: Duration = Duration::from_secs(60);
/// Administrator tokens by admin API root, login and route, so each refresh
/// round reuses one login instead of signing in again.
static ADMIN_TOKENS: LazyLock<Mutex<HashMap<TokenKey, CachedToken>>> =
    LazyLock::new(Default::default);
#[derive(Clone, PartialEq, Eq, Hash)]
struct TokenKey {
    admin_base: String,
    username: String,
    proxy_url: String,
    insecure: bool,
}
struct CachedToken {
    password: String,
    token: String,
    expires: Instant,
}
pub const PROVIDER_NOT_FOUND: &str =
    "The provider was not found in Codex config or Sub2api settings.";
/// Email fields in the order Sub2api's admin account list displays them.
fn listed_email(item: &Value) -> Option<String> {
    [
        &item["extra"]["email_address"],
        &item["extra"]["email"],
        &item["credentials"]["email"],
        &item["name"],
    ]
    .into_iter()
    .filter_map(Value::as_str)
    .map(str::trim)
    .find(|email| email.contains('@'))
    .map(str::to_owned)
}

fn subscription_tier(pool: Pool, field: &Value) -> Option<String> {
    let raw = field.as_str()?.trim();
    let lower = raw.to_ascii_lowercase();
    if matches!(lower.as_str(), "" | "unknown" | "abnormal") {
        return None;
    }
    match pool {
        Pool::Antigravity => {
            if let Some(plan) = crate::antigravity::ultra_by_id(&lower) {
                return Some(plan.into());
            }
            let id = lower.strip_suffix("-tier").unwrap_or(&lower);
            Some(id.strip_prefix("g1-").unwrap_or(id).replace('-', " "))
        }
        Pool::Grok => Some(raw.replace('_', " ")),
        Pool::Claude | Pool::Codex => None,
    }
}

/// The explicit subscription reported by Sub2api, independent of whether
/// its usage windows can be read. Codex uses its own quota response.
pub(crate) fn reported_plan(pool: Pool, usage: &Value) -> Option<String> {
    // Google's raw paid/current tier can distinguish variants that Sub2api
    // groups under PRO or ULTRA.
    if pool == Pool::Antigravity {
        return subscription_tier(pool, &usage["subscription_tier_raw"])
            .or_else(|| subscription_tier(pool, &usage["subscription_tier"]));
    }
    subscription_tier(pool, &usage["subscription_tier"])
        .or_else(|| subscription_tier(pool, &usage["subscription_tier_raw"]))
        .or_else(|| match pool {
            Pool::Grok => subscription_tier(pool, &usage["grok_billing"]["plan"]),
            _ => None,
        })
}

fn listed_plan(pool: Pool, item: &Value) -> Option<String> {
    match pool {
        Pool::Antigravity => subscription_tier(pool, &item["credentials"]["plan_type"]),
        Pool::Grok => [
            &item["credentials"]["subscription_tier"],
            &item["extra"]["grok_usage_snapshot"]["subscription_tier"],
            &item["extra"]["grok_billing_snapshot"]["plan"],
        ]
        .into_iter()
        .find_map(|field| subscription_tier(pool, field)),
        Pool::Claude | Pool::Codex => None,
    }
}
/// The upstream platform whose OAuth accounts a pool shows. Sub2api keeps
/// Antigravity's apart from Gemini CLI's (`gemini`); its `/antigravity`
/// gateway serves only these.
fn platform(pool: Pool) -> &'static str {
    match pool {
        Pool::Codex => "openai",
        Pool::Claude => "anthropic",
        Pool::Antigravity => "antigravity",
        Pool::Grok => "grok",
    }
}
pub struct Connection {
    pub configuration: Provider,
    pub definition: Definition,
    pub admin_base: String,
}
/// The admin API root (`…/api/v1`) of a Sub2api base or admin URL.
pub fn admin_base(raw: &str) -> Result<String, String> {
    let raw = raw.trim().trim_end_matches('/');
    let base = raw
        .strip_suffix("/api/v1")
        .or_else(|| raw.strip_suffix("/v1"))
        .unwrap_or(raw);
    let url = reqwest::Url::parse(base).map_err(|_| {
        "The Sub2api admin API URL is invalid and could not be derived from the provider base URL."
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("Invalid Sub2api admin API URL".into());
    }
    Ok(format!("{base}/api/v1"))
}
async fn payload(response: reqwest::Response) -> Result<Value, String> {
    let status = response.status();
    let value: Value = response
        .json()
        .await
        .map_err(|e| usage::format_reqwest_error(&e))?;
    if !status.is_success()
        || value
            .get("code")
            .and_then(Value::as_i64)
            .is_some_and(|code| code != 0)
    {
        return Err(value
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Sub2api request failed: {status}")));
    }
    value
        .get("data")
        .cloned()
        .ok_or_else(|| "Sub2api response is missing data".into())
}
impl Connection {
    fn client(&self) -> Result<reqwest::Client, String> {
        proxy_session::client(
            &self.configuration.proxy_url,
            proxy_session::Options {
                insecure: self.configuration.allow_insecure_tls,
                ..Default::default()
            },
        )
    }
    /// The pool's OAuth accounts, with their usage.
    pub async fn fetch_accounts(
        &self,
        pool: Pool,
        ids: Option<&[i64]>,
        now: i64,
    ) -> Result<Vec<Account>, String> {
        Ok(self
            .fetch_accounts_via(pool, ids, now)
            .await?
            .into_iter()
            .filter(|a| ids.is_none_or(|ids| ids.contains(&a.id)))
            .collect())
    }
    fn token_key(&self) -> TokenKey {
        TokenKey {
            admin_base: self.admin_base.clone(),
            username: self.configuration.username.clone(),
            proxy_url: self.configuration.proxy_url.trim().to_owned(),
            insecure: self.configuration.allow_insecure_tls,
        }
    }
    /// A cached administrator token, or a new login when none is valid.
    async fn admin_token(&self, client: &reqwest::Client) -> Result<String, String> {
        let key = self.token_key();
        if let Ok(tokens) = ADMIN_TOKENS.lock() {
            if let Some(cached) = tokens.get(&key).filter(|cached| {
                cached.password == self.configuration.password && cached.expires > Instant::now()
            }) {
                return Ok(cached.token.clone());
            }
        }
        let login=payload(client.post(format!("{}/auth/login",self.admin_base)).json(&json!({"email":self.configuration.username,"password":self.configuration.password})).send().await.map_err(|e|usage::format_reqwest_error(&e))?).await?;
        let token = login
            .get("access_token")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                "The Sub2api login response did not include an administrator token.".to_owned()
            })?;
        let lifetime = login
            .get("expires_in")
            .and_then(Value::as_u64)
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_TOKEN_LIFETIME)
            .saturating_sub(TOKEN_EXPIRY_MARGIN);
        if let Ok(mut tokens) = ADMIN_TOKENS.lock() {
            tokens.insert(
                key,
                CachedToken {
                    password: self.configuration.password.clone(),
                    token: token.clone(),
                    expires: Instant::now() + lifetime,
                },
            );
        }
        Ok(token)
    }
    /// Drops a token the server rejected, so the next call logs in again.
    fn forget_token(&self, token: &str) {
        if let Ok(mut tokens) = ADMIN_TOKENS.lock() {
            let key = self.token_key();
            if tokens.get(&key).is_some_and(|cached| cached.token == token) {
                tokens.remove(&key);
            }
        }
    }
    /// Sub2api's connection test of an account: a "hi" to `model` through
    /// the account, answered as server-sent events.
    pub async fn test_account(&self, id: i64, model: &str) -> Result<(), String> {
        let client = self.client()?;
        let mut token = self.admin_token(&client).await?;
        let mut retried = false;
        let response = loop {
            let response = client
                .post(format!("{}/admin/accounts/{id}/test", self.admin_base))
                .bearer_auth(&token)
                .json(&json!({"model_id": model}))
                .send()
                .await
                .map_err(|e| usage::format_reqwest_error(&e))?;
            if response.status() != reqwest::StatusCode::UNAUTHORIZED || retried {
                break response;
            }
            self.forget_token(&token);
            token = self.admin_token(&client).await?;
            retried = true;
        };
        if !response.status().is_success() {
            return payload(response).await.map(|_| ());
        }
        let events = response
            .text()
            .await
            .map_err(|e| usage::format_reqwest_error(&e))?;
        for event in events
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
        {
            match event["type"].as_str() {
                Some("error") => {
                    return Err(event["error"]
                        .as_str()
                        .unwrap_or("Sub2api account test failed")
                        .to_owned())
                }
                Some("test_complete") if event["success"] == true => return Ok(()),
                _ => {}
            }
        }
        Err("Sub2api account test did not complete".into())
    }
    /// The account as the provider last cached it.
    fn cached_account(&self, id: i64) -> Option<Account> {
        self.configuration
            .cached_accounts
            .iter()
            .filter_map(|raw| serde_json::from_value::<Account>(raw.clone()).ok())
            .find(|a| a.id == id)
    }
    async fn fetch_accounts_via(
        &self,
        pool: Pool,
        ids: Option<&[i64]>,
        now: i64,
    ) -> Result<Vec<Account>, String> {
        let client = self.client()?;
        let mut token = self.admin_token(&client).await?;
        let mut retried = false;
        let mut items = Vec::new();
        let mut page = 1usize;
        loop {
            let query = vec![
                ("page", page.to_string()),
                ("page_size", "200".into()),
                ("platform", platform(pool).into()),
                ("type", "oauth".into()),
                ("status", "active".into()),
                ("lite", "1".into()),
                ("sort_by", "name".into()),
                ("sort_order", "asc".into()),
            ];
            let response = client
                .get(format!("{}/admin/accounts", self.admin_base))
                .bearer_auth(&token)
                .query(&query)
                .send()
                .await
                .map_err(|e| usage::format_reqwest_error(&e))?;
            // A cached token the server no longer accepts gets one fresh login.
            if response.status() == reqwest::StatusCode::UNAUTHORIZED && !retried {
                self.forget_token(&token);
                token = self.admin_token(&client).await?;
                retried = true;
                continue;
            }
            let data = payload(response).await?;
            items.extend(
                data.get("items")
                    .and_then(Value::as_array)
                    .ok_or("Invalid Sub2api account list")?
                    .iter()
                    .cloned(),
            );
            let pages = data.get("pages").and_then(Value::as_u64).unwrap_or(1);
            if page as u64 >= pages {
                break;
            }
            if page >= 1000 {
                return Err("Sub2api account pagination limit exceeded".into());
            }
            page += 1;
        }
        let mut accounts = Vec::new();
        for item in items {
            let Some(id) = item.get("id").and_then(Value::as_i64) else {
                continue;
            };
            if !item["parent_account_id"].is_null()
                || !["platform", "type"]
                    .into_iter()
                    .zip([platform(pool), "oauth"])
                    .all(|(key, value)| {
                        item[key]
                            .as_str()
                            .is_some_and(|s| s.eq_ignore_ascii_case(value))
                    })
            {
                continue;
            }
            let status = item["status"].as_str().unwrap_or_default();
            // Check locally too: a server may ignore the status filter.
            // Older servers may omit schedulable; an explicit false disables
            // an otherwise active account. Rate limits alone do not hide it.
            if !status.eq_ignore_ascii_case("active") || item["schedulable"] == false {
                continue;
            }
            let mut account = Account {
                id,
                name: item["name"].as_str().unwrap_or_default().into(),
                email: listed_email(&item),
                account_id: None,
                account_type: "oauth".into(),
                status: "active".into(),
                plan_type: listed_plan(pool, &item),
                usage: None,
                usage_error: item["error_message"].as_str().map(str::to_owned),
                provider_id: Some(self.configuration.provider_id.clone()),
                outdated_version: None,
            };
            if ids.is_some_and(|ids| !ids.contains(&id)) {
                accounts.push(account);
                continue;
            }
            let quota_url = match pool {
                Pool::Codex => format!("{}/admin/openai/accounts/{id}/quota", self.admin_base),
                Pool::Claude | Pool::Antigravity | Pool::Grok => {
                    format!("{}/admin/accounts/{id}/usage", self.admin_base)
                }
            };
            let quota = async {
                let response = client
                    .get(quota_url)
                    .bearer_auth(&token)
                    .send()
                    .await
                    .map_err(|e| usage::format_reqwest_error(&e))?;
                payload(response).await
            }
            .await;
            match quota {
                Ok(value) if pool != Pool::Codex => {
                    let usage = match pool {
                        Pool::Claude => crate::claude::sub2api_usage(&value, now),
                        Pool::Grok => crate::grok::sub2api_usage(&value, now),
                        _ => crate::antigravity::sub2api_usage(&value, now),
                    };
                    match usage {
                        Ok(mut usage) => {
                            usage.plan_type = usage.plan_type.or(account.plan_type);
                            account.plan_type = usage.plan_type.clone();
                            account.usage = Some(usage);
                            account.usage_error = None;
                        }
                        Err(e) => {
                            account.plan_type = reported_plan(pool, &value).or(account.plan_type);
                            account.usage_error = Some(e);
                        }
                    }
                }
                Ok(value) => {
                    if let Some(email) = value["email"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        account.email = Some(email.to_owned());
                    }
                    account.account_id = value["account_id"].as_str().map(str::to_owned);
                    match usage::parse_payload(value, now) {
                        Ok(usage) => {
                            account.plan_type = usage.plan_type.clone();
                            account.usage = Some(usage);
                            account.usage_error = None;
                        }
                        Err(e) => account.usage_error = Some(e),
                    }
                }
                Err(e) => account.usage_error = Some(e),
            }
            account.keep_last(self.cached_account(id));
            accounts.push(account);
        }
        // Without a version, each account keeps the one last seen.
        if let Some(since) = weekly_since(pool) {
            let version = self.version(&client, &token).await;
            for account in &mut accounts {
                account.outdated_version = match &version {
                    Some(v) => Some(v.clone()).filter(|v| predates(v, since)),
                    None => self
                        .cached_account(account.id)
                        .and_then(|last| last.outdated_version),
                };
            }
        }
        Ok(accounts)
    }
    /// Sub2api's version; none when it does not say.
    async fn version(&self, client: &reqwest::Client, token: &str) -> Option<String> {
        let response = client
            .get(format!("{}/admin/system/version", self.admin_base))
            .bearer_auth(token)
            .send()
            .await
            .ok()?;
        let data = payload(response).await.ok()?;
        let version = data.get("version")?.as_str()?.trim();
        (!version.is_empty()).then(|| version.to_owned())
    }
}
/// The first Sub2api release whose usage of the pool's accounts has weekly
/// windows; none for a pool whose usage always had them. Antigravity's came
/// with `antigravity_quota_groups` (0.2.20; before it, only each model's
/// five hours); Grok's with `grok_billing` (0.1.155; before it, only the
/// rate limits of xAI's response headers).
fn weekly_since(pool: Pool) -> Option<[u64; 3]> {
    match pool {
        Pool::Antigravity => Some([0, 2, 20]),
        Pool::Grok => Some([0, 1, 155]),
        Pool::Codex | Pool::Claude => None,
    }
}
/// Whether a Sub2api version predates `since`. A version it cannot read, or
/// a development build's 0.0.0, is taken as recent.
fn predates(version: &str, since: [u64; 3]) -> bool {
    let mut parts = version.trim().trim_start_matches(['v', 'V']).split('.');
    let mut number = [0u64; 3];
    for slot in &mut number {
        let digits: String = parts
            .next()
            .unwrap_or("0")
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        match digits.parse() {
            Ok(n) => *slot = n,
            Err(_) => return false,
        }
    }
    number != [0, 0, 0] && number < since
}

#[cfg(test)]
mod tests {
    use super::{predates, weekly_since};
    use crate::Pool;
    fn lacks_antigravity_weekly(version: &str) -> bool {
        predates(version, weekly_since(Pool::Antigravity).unwrap())
    }

    #[tokio::test]
    async fn every_pool_fetches_usage_only_for_active_enabled_accounts() {
        use axum::{extract::Request, routing::any, Json, Router};
        use serde_json::json;
        use std::sync::{Arc, Mutex};

        for pool in Pool::ALL {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen = requests.clone();
            let router = Router::new().fallback(any(move |request: Request| {
                let seen = seen.clone();
                async move {
                    let path = request.uri().path();
                    seen.lock().unwrap().push(request.uri().to_string());
                    let data = if path == "/api/v1/auth/login" {
                        json!({"access_token": "admin"})
                    } else if path == "/api/v1/admin/accounts" {
                        // Deliberately ignore the query's status filter to test
                        // local filtering, including accounts already imported.
                        let mut items = json!([
                            {"id": 1, "status": "active", "schedulable": true,
                             "rate_limit_reset_at": "2099-01-01T00:00:00Z"},
                            {"id": 2, "status": "active"},
                            {"id": 3, "status": "active", "schedulable": false},
                            {"id": 4, "status": "inactive"},
                            {"id": 5, "status": "disabled"},
                            {"id": 6, "status": "error"},
                            {"id": 7}
                        ]);
                        for item in items.as_array_mut().unwrap() {
                            item["platform"] = super::platform(pool).into();
                            item["type"] = "oauth".into();
                        }
                        json!({"items": items, "pages": 1})
                    } else {
                        json!({"seven_day": {"utilization": 34, "resets_at": null}})
                    };
                    Json(json!({"code": 0, "data": data}))
                }
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
            let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            let mut connection = super::Connection {
                configuration: crate::settings::Provider {
                    provider_id: "p".into(),
                    username: "admin@example.com".into(),
                    password: "secret".into(),
                    ..Default::default()
                },
                definition: crate::codex::config::Definition {
                    id: "p".into(),
                    base_url: None,
                    wire_api: None,
                    env_key: None,
                    requires_openai_auth: None,
                },
                admin_base: base,
            };
            // Initial sync, provider refresh and targeted card refresh agree.
            for targeted in [false, false, true] {
                let ids = [1, 2, 3, 4, 5, 6, 7];
                let fetched = connection
                    .fetch_accounts(pool, targeted.then_some(ids.as_slice()), 0)
                    .await
                    .unwrap();
                assert_eq!(
                    fetched.iter().map(|a| a.id).collect::<Vec<_>>(),
                    [1, 2],
                    "{pool:?}"
                );
                connection.configuration.imported_account_ids = ids.to_vec();
            }
            let requests = requests.lock().unwrap();
            let listings: Vec<_> = requests
                .iter()
                .filter(|uri| uri.starts_with("/api/v1/admin/accounts?"))
                .collect();
            assert_eq!(listings.len(), 3);
            assert!(listings.iter().all(|uri| uri.contains("status=active")));
            let quota: Vec<_> = requests
                .iter()
                .filter(|uri| uri.ends_with("/quota") || uri.ends_with("/usage"))
                .collect();
            assert_eq!(quota.len(), 6);
            assert!(quota
                .iter()
                .all(|uri| uri.contains("/accounts/1/") || uri.contains("/accounts/2/")));
            server.abort();
        }
    }

    #[tokio::test]
    async fn rounds_reuse_one_login_until_the_server_rejects_it_or_the_password_changes() {
        use axum::{extract::Request, http::StatusCode, response::IntoResponse, routing::any};
        use std::sync::{Arc, Mutex};
        // Each login issues the next token; the server accepts only the newest one.
        let issued = Arc::new(Mutex::new(0_u32));
        let logins = Arc::new(Mutex::new(0_u32));
        let (server_issued, server_logins) = (issued.clone(), logins.clone());
        let router = axum::Router::new().fallback(any(move |request: Request| {
            let (issued, logins) = (server_issued.clone(), server_logins.clone());
            async move {
                let uri = request.uri().path().to_owned();
                if uri == "/api/v1/auth/login" {
                    *logins.lock().unwrap() += 1;
                    let mut issued = issued.lock().unwrap();
                    *issued += 1;
                    let token = format!("t{}", *issued);
                    return axum::Json(serde_json::json!({"code":0,"data":{
                        "access_token": token, "expires_in": 3600}}))
                    .into_response();
                }
                let current = format!("Bearer t{}", *issued.lock().unwrap());
                if request
                    .headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    != Some(current.as_str())
                {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                axum::Json(serde_json::json!({"code":0,"data":{"items":[],"pages":1}}))
                    .into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let connection = |password: &str| super::Connection {
            configuration: crate::settings::Provider {
                provider_id: "p".into(),
                username: "admin@example.com".into(),
                password: password.into(),
                ..Default::default()
            },
            definition: crate::codex::config::Definition {
                id: "p".into(),
                base_url: None,
                wire_api: None,
                env_key: None,
                requires_openai_auth: None,
            },
            admin_base: base.clone(),
        };
        let rounds = |password: &'static str| {
            let connection = connection(password);
            async move {
                connection
                    .fetch_accounts(Pool::Codex, None, 0)
                    .await
                    .unwrap()
            }
        };
        rounds("one").await;
        rounds("one").await;
        assert_eq!(*logins.lock().unwrap(), 1);
        // Another login elsewhere invalidates the cached token: one retry with a new login.
        *issued.lock().unwrap() += 1;
        rounds("one").await;
        assert_eq!(*logins.lock().unwrap(), 2);
        rounds("one").await;
        assert_eq!(*logins.lock().unwrap(), 2);
        // A changed password never reuses the old login.
        rounds("two").await;
        assert_eq!(*logins.lock().unwrap(), 3);
    }

    #[test]
    fn grok_weekly_usage_needs_sub2api_0_1_155() {
        let since = weekly_since(Pool::Grok).unwrap();
        for (version, lacks) in [
            ("0.1.139", true),
            ("v0.1.154", true),
            ("0.1.155", false),
            ("0.1.177", false),
            ("0.2.24", false),
            ("0.0.0", false),
        ] {
            assert_eq!(predates(version, since), lacks, "{version}");
        }
        assert!(weekly_since(Pool::Claude).is_none());
    }

    #[test]
    fn antigravity_weekly_usage_needs_sub2api_0_2_20() {
        for (version, lacks) in [
            ("0.1.177", true),
            ("v0.1.185", true),
            ("0.2.19", true),
            ("0.2.20", false),
            ("0.2.20-beta.1", false),
            ("0.2.24", false),
            ("1.0", false),
            ("0.0.0-dev", false),
            ("unknown", false),
        ] {
            assert_eq!(lacks_antigravity_weekly(version), lacks, "{version}");
        }
    }
}
