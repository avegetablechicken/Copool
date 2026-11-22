//! Sub2api providers, whose OAuth accounts join a pool as cards: one
//! client for both pools, and what both do with their providers' cards.
use crate::{
    codex_config::{self, Definition},
    credentials::SecretStore,
    models::{AccountSummary, UsageSnapshot},
    pool::Pool,
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
    /// The provider's Sub2api version, for an Antigravity account, when it
    /// predates weekly usage (`ANTIGRAVITY_WEEKLY_SINCE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outdated_version: Option<String>,
}
impl Account {
    pub fn display_email(&self) -> &str {
        self.email
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("--")
    }
    /// The card's email: Sub2api's name for the account stands in until its
    /// usage reports one.
    fn shown_email(&self) -> Option<String> {
        [self.email.as_deref(), Some(self.name.as_str())]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|s| !s.is_empty())
            .map(str::to_owned)
    }
    /// The account's card, by its id in the pool.
    pub fn summary(&self, id: String) -> AccountSummary {
        AccountSummary {
            display_status: "list".into(),
            workspace_status: if self.status.eq_ignore_ascii_case("active") {
                "active"
            } else {
                "deactivated"
            }
            .into(),
            id,
            label: self.shown_email().unwrap_or_else(|| "--".into()),
            email: self.shown_email(),
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
            outdated_sub2api_version: self.outdated_version.clone(),
        }
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
    }
}
/// A card's id: its pool, provider and account.
pub(crate) fn card_id(pool: Pool, provider: &str, account: i64) -> String {
    format!("{}{}-{account}", card_prefix(pool), provider.trim())
}
fn card_prefix(pool: Pool) -> &'static str {
    match pool {
        Pool::Codex => "sub2api-",
        Pool::Claude => "claude-sub2api-",
        Pool::Antigravity => "antigravity-sub2api-",
    }
}
/// Whether a card of the pool is a Sub2api account's.
pub(crate) fn is_card(pool: Pool, id: &str) -> bool {
    id.starts_with(card_prefix(pool))
}
/// The provider of a Sub2api card of the pool, as its id names it.
pub(crate) fn card_provider(pool: Pool, id: &str) -> Option<&str> {
    let (provider, account) = id.strip_prefix(card_prefix(pool))?.rsplit_once('-')?;
    account.parse::<i64>().ok().map(|_| provider)
}
/// Where the keyring keeps a provider's administrator password.
pub(crate) fn password_secret(pool: Pool, id: &str) -> String {
    match pool {
        Pool::Codex => id.to_owned(),
        Pool::Claude => format!("claude-sub2api-password:{id}"),
        Pool::Antigravity => format!("antigravity-sub2api-password:{id}"),
    }
}
/// A provider's accounts in a pool: those imported, as last fetched.
#[derive(Clone, Debug)]
pub(crate) struct Member {
    /// The provider as its cards' ids name it.
    pub(crate) key: String,
    /// The provider as shown, and in its errors.
    pub(crate) name: String,
    /// The proxy of every request for the provider and its accounts.
    pub(crate) proxy_url: String,
    pub(crate) imported: Vec<i64>,
    pub(crate) cached: Vec<Account>,
}
impl Member {
    pub(crate) fn card_id(&self, pool: Pool, account: i64) -> String {
        card_id(pool, &self.key, account)
    }
    /// The provider's cards; current while its pool uses the provider.
    pub(crate) fn summaries(&self, pool: Pool, current: bool) -> Vec<AccountSummary> {
        self.cached
            .iter()
            .map(|a| {
                let mut summary = a.summary(self.card_id(pool, a.id));
                summary.is_current = current;
                summary.proxy_url = self.proxy_url.clone();
                summary
            })
            .collect()
    }
    /// Takes a refresh's accounts: a sync's are the provider's accounts from
    /// then on. A refresh adds those it discovers; when it answered for all
    /// it asked about (`ids`, every one without), an account it no longer
    /// lists leaves the Codex pool, and in the Claude pool says so instead
    /// of its last error.
    pub(crate) fn adopt(
        &mut self,
        pool: Pool,
        fetched: Vec<Account>,
        ids: Option<&[i64]>,
        complete: bool,
        sync: bool,
    ) -> usize {
        if sync {
            self.imported = fetched.iter().map(|a| a.id).collect();
            self.cached = fetched;
            return self.cached.len();
        }
        // Respect a removal made while the request ran.
        if self.imported.is_empty() {
            return 0;
        }
        let missing = |a: &Account| {
            complete
                && ids.is_none_or(|ids| ids.contains(&a.id))
                && !fetched.iter().any(|f| f.id == a.id)
        };
        match pool {
            Pool::Codex => {
                let gone: Vec<i64> = self
                    .cached
                    .iter()
                    .filter(|a| missing(a))
                    .map(|a| a.id)
                    .collect();
                self.cached.retain(|a| !gone.contains(&a.id));
                self.imported.retain(|id| !gone.contains(id));
            }
            Pool::Claude | Pool::Antigravity => {
                for cached in self.cached.iter_mut().filter(|a| missing(a)) {
                    cached.status = "missing".into();
                    cached.usage = None;
                    cached.usage_error = Some("The account is no longer in Sub2api".into());
                }
            }
        }
        for account in fetched {
            if !self.imported.contains(&account.id) {
                self.imported.push(account.id);
            }
            match self.cached.iter_mut().find(|a| a.id == account.id) {
                Some(cached) => *cached = account,
                None => self.cached.push(account),
            }
        }
        self.cached.len()
    }
}
/// Checks a provider form: its name unique in the pool, its sign-in
/// complete (a saved provider keeps its password), and its proxy and URLs.
pub(crate) fn validate_form(
    name: &str,
    duplicate: bool,
    username: &str,
    password: &str,
    saved: bool,
    proxy: &str,
    urls: &[&str],
) -> Result<(), String> {
    if duplicate {
        return Err("error.sub2api.duplicate_provider".into());
    }
    if name.is_empty() || username.is_empty() || (password.is_empty() && !saved) {
        return Err("error.sub2api.configuration_incomplete".into());
    }
    crate::utils::validate_account_proxy(proxy)?;
    for url in urls.iter().filter(|url| !url.is_empty()) {
        let url = reqwest::Url::parse(url).map_err(|_| "error.sub2api.invalid_base_url")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("error.sub2api.invalid_base_url".into());
        }
    }
    Ok(())
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
    /// Logs in and lists the provider's accounts once, with their usage.
    pub async fn refresh_accounts(
        &self,
        pool: Pool,
        ids: Option<&[i64]>,
        now: i64,
    ) -> Result<Refresh, String> {
        let mut accounts = std::collections::BTreeMap::new();
        let mut errors = vec![];
        match self.fetch_accounts_via(pool, ids, now).await {
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
    async fn fetch_accounts_via(
        &self,
        pool: Pool,
        ids: Option<&[i64]>,
        now: i64,
    ) -> Result<Vec<Account>, String> {
        let client = self.client()?;
        let token = &self.admin_token(&client).await?;
        // A Claude or Antigravity card Sub2api stopped serving, such as one
        // whose token expired, says why; the Codex pool drops it.
        let inactive_shown = |id: i64| {
            pool != Pool::Codex
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
                ("platform", platform(pool).into()),
                ("type", "oauth".into()),
                ("lite", "1".into()),
                ("sort_by", "name".into()),
                ("sort_order", "asc".into()),
            ];
            if pool == Pool::Codex
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
                        outdated_version: None,
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
                outdated_version: None,
            };
            if ids.is_some_and(|ids| !ids.contains(&id)) {
                accounts.push(account);
                continue;
            }
            let quota_url = match pool {
                Pool::Codex => format!("{}/admin/openai/accounts/{id}/quota", self.admin_base),
                Pool::Claude | Pool::Antigravity => {
                    format!("{}/admin/accounts/{id}/usage", self.admin_base)
                }
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
                Ok(value) if pool != Pool::Codex => {
                    let usage = match pool {
                        Pool::Claude => crate::claude::sub2api_usage(&value, now),
                        _ => crate::antigravity::sub2api_usage(&value, now),
                    };
                    match usage {
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
        // Without a version, each account keeps the one last seen.
        if pool == Pool::Antigravity {
            let version = self.version(&client, token).await;
            for account in &mut accounts {
                account.outdated_version = match &version {
                    Some(v) => Some(v.clone()).filter(|v| lacks_antigravity_weekly(v)),
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
/// The first Sub2api release whose Antigravity usage has weekly windows
/// (`antigravity_quota_groups`); before it, only each model's five hours.
const ANTIGRAVITY_WEEKLY_SINCE: [u64; 3] = [0, 2, 20];
/// Whether a Sub2api version predates `ANTIGRAVITY_WEEKLY_SINCE`. A version
/// it cannot read, or a development build's 0.0.0, is taken as recent.
fn lacks_antigravity_weekly(version: &str) -> bool {
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
    number != [0, 0, 0] && number < ANTIGRAVITY_WEEKLY_SINCE
}

#[cfg(test)]
mod tests {
    use super::lacks_antigravity_weekly;

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
