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
    #[serde(default, rename = "proxyURL", alias = "proxyUrl")]
    pub proxy_url: Option<String>,
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
            available: self.status.eq_ignore_ascii_case("active"),
            proxy_url: self.proxy_url.clone().unwrap_or_default(),
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
    let value: Value = response.json().await.map_err(|e| e.to_string())?;
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
    fn client(&self, proxy: &str) -> Result<reqwest::Client, String> {
        proxy_session::client(
            proxy,
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
        let list_proxy = ids
            .filter(|v| v.len() == 1)
            .map(|v| self.configuration.proxy_for(v[0]))
            .unwrap_or_else(|| self.configuration.proxy_url.clone());
        let client = self.client(&list_proxy)?;
        let login=payload(client.post(format!("{}/auth/login",self.admin_base)).json(&json!({"email":self.configuration.username,"password":self.configuration.password})).send().await.map_err(|e|e.to_string())?).await?;
        let token = login
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or("The Sub2api login response did not include an administrator token.")?;
        let mut items = Vec::new();
        let mut page = 1usize;
        loop {
            let data = payload(
                client
                    .get(format!("{}/admin/accounts", self.admin_base))
                    .bearer_auth(token)
                    .query(&[
                        ("page", page.to_string()),
                        ("page_size", "200".into()),
                        ("platform", platform.name().into()),
                        ("type", "oauth".into()),
                        ("status", "active".into()),
                        ("lite", "1".into()),
                        ("sort_by", "name".into()),
                        ("sort_order", "asc".into()),
                    ])
                    .send()
                    .await
                    .map_err(|e| e.to_string())?,
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
                || !["platform", "type", "status"]
                    .into_iter()
                    .zip([platform.name(), "oauth", "active"])
                    .all(|(key, value)| {
                        item[key]
                            .as_str()
                            .is_some_and(|s| s.eq_ignore_ascii_case(value))
                    })
                || ids.is_some_and(|ids| !ids.contains(&id))
            {
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
                proxy_url: self
                    .configuration
                    .account_proxy_urls
                    .get(&id.to_string())
                    .cloned(),
            };
            let quota_client = self.client(&self.configuration.proxy_for(id))?;
            let quota_url = match platform {
                Platform::OpenAi => format!("{}/admin/openai/accounts/{id}/quota", self.admin_base),
                Platform::Anthropic => format!("{}/admin/accounts/{id}/usage", self.admin_base),
            };
            let quota = async {
                let response = quota_client
                    .get(quota_url)
                    .bearer_auth(token)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
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
            accounts.push(account);
        }
        Ok(accounts)
    }
}
