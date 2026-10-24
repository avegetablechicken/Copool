use crate::{
    codex_config::{self, Definition},
    credentials::SecretStore,
    models::{AccountSummary, UsageSnapshot},
    proxy_session,
    settings::{Provider, Providers},
    usage,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
pub const PROVIDER_NOT_FOUND: &str =
    "The provider was not found in Codex config or Sub2api settings.";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: i64,
    pub name: String,
    pub email: Option<String>,
    #[serde(rename = "accountID", alias = "accountId")]
    pub account_id: Option<String>,
    pub account_type: String,
    pub status: String,
    pub plan_type: Option<String>,
    pub usage: Option<UsageSnapshot>,
    pub usage_error: Option<String>,
    #[serde(default, rename = "providerID", alias = "providerId")]
    pub provider_id: Option<String>,
}
impl Account {
    pub fn card_id(&self) -> String {
        format!(
            "sub2api-{}-{}",
            self.provider_id.as_deref().unwrap_or("unknown").trim(),
            self.id
        )
    }
    pub fn display_email(&self) -> &str {
        self.email
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("--")
    }
    pub fn summary(&self) -> AccountSummary {
        AccountSummary {
            display_status: "list".into(),
            workspace_status: if self.status.eq_ignore_ascii_case("active") {
                "active"
            } else {
                "deactivated"
            }
            .into(),
            id: self.card_id(),
            label: self.display_email().into(),
            email: self.email.clone(),
            account_id: self
                .account_id
                .clone()
                .unwrap_or_else(|| self.id.to_string()),
            plan_type: self.plan_type.clone(),
            team_name: None,
            source_tag: Some("SUB2API".into()),
            added_at: self.id,
            updated_at: self.usage.as_ref().map(|u| u.fetched_at).unwrap_or(0),
            usage: self.usage.clone().map(UsageSnapshot::with_matching_windows),
            usage_error: self.usage_error.clone(),
            is_current: false,
            awaiting_first_request: false,
            token_expires_at: None,
            renewal: None,
            next_refresh_at: None,
            available: self.status.eq_ignore_ascii_case("active"),
            proxy_url: String::new(),
        }
    }
}
/// The upstream platform whose OAuth accounts a pool shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Platform {
    OpenAi,
    Anthropic,
}
impl Platform {
    fn name(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
        }
    }
}
pub struct Connection {
    pub configuration: Provider,
    pub definition: Definition,
    pub admin_base: String,
}
pub struct Refresh {
    pub accounts: Vec<Account>,
    pub errors: Vec<String>,
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
pub fn connection(
    config_path: &Path,
    settings: &Providers,
    id: Option<&str>,
    secrets: &dyn SecretStore,
) -> Result<Connection, String> {
    let selected = codex_config::resolve_path(config_path);
    let id = id.unwrap_or(&selected.id);
    if id.eq_ignore_ascii_case("openai") {
        return Err(PROVIDER_NOT_FOUND.into());
    }
    let mut configuration = settings.provider(id).ok_or(PROVIDER_NOT_FOUND)?;
    let definition = codex_config::definitions(config_path)
        .into_iter()
        .find(|p| p.id.eq_ignore_ascii_case(id))
        .ok_or(PROVIDER_NOT_FOUND)?;
    if configuration.password.is_empty() {
        configuration.password = secrets.get(&configuration.id)?.unwrap_or_default();
    }
    if !configuration.is_enabled() {
        return Err("Sub2api administrator credentials are incomplete.".into());
    }
    let raw = if !configuration.admin_base_url.is_empty() {
        configuration.admin_base_url.as_str()
    } else {
        definition.base_url.as_deref().ok_or(PROVIDER_NOT_FOUND)?
    };
    let admin_base = admin_base(raw)?;
    Ok(Connection {
        configuration,
        definition,
        admin_base,
    })
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
    pub async fn fetch_accounts(
        &self,
        ids: Option<&[i64]>,
        now: i64,
    ) -> Result<Vec<Account>, String> {
        self.fetch_platform_accounts(Platform::OpenAi, ids, now)
            .await
    }
    /// Active OAuth accounts of a platform, with their usage.
    pub async fn fetch_platform_accounts(
        &self,
        platform: Platform,
        ids: Option<&[i64]>,
        now: i64,
    ) -> Result<Vec<Account>, String> {
        Ok(self
            .fetch_platform_accounts_via(platform, ids, now)
            .await?
            .into_iter()
            .filter(|a| ids.is_none_or(|ids| ids.contains(&a.id)))
            .collect())
    }
    /// Logs in and lists the provider's accounts once, with their usage.
    pub async fn refresh_platform_accounts(
        &self,
        platform: Platform,
        ids: Option<&[i64]>,
        now: i64,
    ) -> Result<Refresh, String> {
        let mut accounts = std::collections::BTreeMap::new();
        let mut errors = vec![];
        match self.fetch_platform_accounts_via(platform, ids, now).await {
            Ok(fetched) => {
                for account in fetched {
                    if ids.is_none_or(|ids| ids.contains(&account.id)) {
                        accounts.insert(account.id, account);
                    }
                }
            }
            Err(error) => {
                for raw in &self.configuration.cached_accounts {
                    if let Ok(mut account) = serde_json::from_value::<Account>(raw.clone()) {
                        if ids.is_none_or(|ids| ids.contains(&account.id)) {
                            account.usage_error = Some(error.clone());
                            accounts.insert(account.id, account);
                        }
                    }
                }
                errors.push(error);
            }
        }
        Ok(Refresh {
            accounts: accounts.into_values().collect(),
            errors,
        })
    }
    async fn admin_token(&self, client: &reqwest::Client) -> Result<String, String> {
        let login=payload(client.post(format!("{}/auth/login",self.admin_base)).json(&json!({"email":self.configuration.username,"password":self.configuration.password})).send().await.map_err(|e|usage::format_reqwest_error(&e))?).await?;
        login
            .get("access_token")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                "The Sub2api login response did not include an administrator token.".into()
            })
    }
    /// Sub2api's connection test of an account: a "hi" to `model` through
    /// the account, answered as server-sent events.
    pub async fn test_account(&self, id: i64, model: &str) -> Result<(), String> {
        let client = self.client()?;
        let token = self.admin_token(&client).await?;
        let response = client
            .post(format!("{}/admin/accounts/{id}/test", self.admin_base))
            .bearer_auth(token)
            .json(&json!({"model_id": model}))
            .send()
            .await
            .map_err(|e| usage::format_reqwest_error(&e))?;
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
    async fn fetch_platform_accounts_via(
        &self,
        platform: Platform,
        ids: Option<&[i64]>,
        now: i64,
    ) -> Result<Vec<Account>, String> {
        let client = self.client()?;
        let token = &self.admin_token(&client).await?;
        // A Claude card Sub2api stopped serving, such as one whose token
        // expired, says why; the Codex pool drops it.
        let inactive_shown = |id: i64| {
            platform == Platform::Anthropic
                && ids
                    .unwrap_or(&self.configuration.imported_account_ids)
                    .contains(&id)
        };
        let mut items = Vec::new();
        let mut page = 1usize;
        loop {
            let mut query = vec![
                ("page", page.to_string()),
                ("page_size", "200".into()),
                ("platform", platform.name().into()),
                ("type", "oauth".into()),
                ("lite", "1".into()),
                ("sort_by", "name".into()),
                ("sort_order", "asc".into()),
            ];
            if platform != Platform::Anthropic
                || (ids.is_none() && self.configuration.imported_account_ids.is_empty())
            {
                query.push(("status", "active".into()));
            }
            let data = payload(
                client
                    .get(format!("{}/admin/accounts", self.admin_base))
                    .bearer_auth(token)
                    .query(&query)
                    .send()
                    .await
                    .map_err(|e| usage::format_reqwest_error(&e))?,
            )
            .await?;
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
                    .zip([platform.name(), "oauth"])
                    .all(|(key, value)| {
                        item[key]
                            .as_str()
                            .is_some_and(|s| s.eq_ignore_ascii_case(value))
                    })
            {
                continue;
            }
            let status = item["status"].as_str().unwrap_or_default();
            if !status.eq_ignore_ascii_case("active") {
                // Shown as deactivated, with Sub2api's reason.
                if inactive_shown(id) {
                    accounts.push(Account {
                        id,
                        name: item["name"].as_str().unwrap_or_default().into(),
                        email: item["name"]
                            .as_str()
                            .filter(|s| s.contains('@'))
                            .map(str::to_owned),
                        account_id: None,
                        account_type: "oauth".into(),
                        status: if status.is_empty() { "unknown" } else { status }.into(),
                        plan_type: None,
                        usage: None,
                        usage_error: Some(
                            item["error_message"]
                                .as_str()
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .map(str::to_owned)
                                .unwrap_or_else(|| format!("The account is {status} in Sub2api")),
                        ),
                        provider_id: Some(self.configuration.provider_id.clone()),
                    });
                }
                continue;
            }
            let mut account = Account {
                id,
                name: item["name"].as_str().unwrap_or_default().into(),
                email: item["name"]
                    .as_str()
                    .filter(|s| s.contains('@'))
                    .map(str::to_owned),
                account_id: None,
                account_type: "oauth".into(),
                status: "active".into(),
                plan_type: None,
                usage: None,
                usage_error: item["error_message"].as_str().map(str::to_owned),
                provider_id: Some(self.configuration.provider_id.clone()),
            };
            if ids.is_some_and(|ids| !ids.contains(&id)) {
                accounts.push(account);
                continue;
            }
            let quota_url = match platform {
                Platform::OpenAi => format!("{}/admin/openai/accounts/{id}/quota", self.admin_base),
                Platform::Anthropic => format!("{}/admin/accounts/{id}/usage", self.admin_base),
            };
            let quota = async {
                let response = client
                    .get(quota_url)
                    .bearer_auth(token)
                    .send()
                    .await
                    .map_err(|e| usage::format_reqwest_error(&e))?;
                payload(response).await
            }
            .await;
            match quota {
                Ok(value) if platform == Platform::Anthropic => {
                    match crate::claude::sub2api_usage(&value, now) {
                        Ok(usage) => {
                            account.plan_type = usage.plan_type.clone();
                            account.usage = Some(usage);
                            account.usage_error = None;
                        }
                        Err(e) => account.usage_error = Some(e),
                    }
                }
                Ok(value) => {
                    account.email = value["email"].as_str().map(str::to_owned).or(account.email);
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
            // An account whose usage failed keeps its last known identity,
            // plan and usage, with the error, as an imported account does.
            if account.usage.is_none() {
                if let Some(last) = self.cached_account(id) {
                    account.email = account.email.or(last.email);
                    account.account_id = account.account_id.or(last.account_id);
                    account.plan_type = account.plan_type.or(last.plan_type);
                    account.usage = last.usage;
                }
            }
            accounts.push(account);
        }
        Ok(accounts)
    }
}
