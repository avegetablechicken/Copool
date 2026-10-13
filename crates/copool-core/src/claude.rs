//! Claude Code account pool: the logins Copool keeps, the one Claude Code uses
//! (the macOS Keychain or `.credentials.json`, with `oauthAccount` in
//! `.claude.json`), OAuth usage and switching.
use crate::{
    account_network::Response,
    application::{Application, SmartSwitchOutcome, SwitchExecution},
    models::{AccountSummary, CreditSnapshot, UsageSnapshot, UsageWindow},
    persistence::write_json,
    utils::now_unix_seconds,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, sync::Arc};

type Result<T> = std::result::Result<T, String>;

mod provider_config;
mod settings_source;
mod sub2api;
pub use sub2api::Provider as Sub2ApiProvider;

// `cedar_ember=1` adds the banked usage-limit resets to the usage response.
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage?cedar_ember=1";
const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const ORGANIZATIONS_URL: &str = "https://api.anthropic.com/api/oauth/organizations";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const USER_AGENT: &str = "claude-code/2.1.69";
// The banked resets are returned only to Claude Code 2.1.280 or later, by its
// User-Agent; others read `eligible: false` with the rest of the usage intact.
// This is the earliest version that receives them.
const USAGE_USER_AGENT: &str = "claude-cli/2.1.280 (external, cli)";
const SIGN_IN_EXPIRED: &str = "error.accounts.sign_in_expired";
const TOKEN_REFRESH_RATE_LIMITED: &str = "Claude token refresh failed: HTTP 429";

/// Where Claude Code keeps its login.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub config_dir: PathBuf,
    pub global_config: PathBuf,
    /// The macOS Keychain service holding the credentials; none for the file.
    pub keychain: Option<String>,
}
impl Location {
    pub fn discover() -> Result<Self> {
        let env = |name| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        // An isolated directory for smoke tests, never the Keychain.
        if let Some(dir) = env("COPOOL_CLAUDE_CONFIG_DIR") {
            return Ok(Self::files(dir));
        }
        let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
        let (config_dir, global_config, service) = match env("CLAUDE_CONFIG_DIR") {
            // Claude Code scopes the Keychain entry of a custom directory by its hash.
            Some(dir) => {
                let digest = hex(&Sha256::digest(
                    dir.to_string_lossy().replace('\\', "/").as_bytes(),
                ));
                let service = format!("{KEYCHAIN_SERVICE}-{}", &digest[..8]);
                (dir.clone(), dir.join(".claude.json"), service)
            }
            None => (
                home.join(".claude"),
                home.join(".claude.json"),
                KEYCHAIN_SERVICE.into(),
            ),
        };
        Ok(Self {
            config_dir,
            global_config,
            keychain: cfg!(target_os = "macos").then_some(service),
        })
    }
    /// Credentials and account in files of one directory, without the Keychain.
    pub fn files(dir: PathBuf) -> Self {
        Self {
            global_config: dir.join(".claude.json"),
            config_dir: dir,
            keychain: None,
        }
    }
    fn credentials_file(&self) -> PathBuf {
        self.config_dir.join(".credentials.json")
    }
    /// The file holding `oauthAccount`: the global configuration, or a
    /// `.claude.json` inside the configuration directory that has one (as
    /// cc-account-switcher accepts), else the global configuration.
    fn account_file(&self) -> PathBuf {
        let inner = self.config_dir.join(".claude.json");
        let has_account = |path: &PathBuf| {
            fs::read_to_string(path)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .is_some_and(|v| v["oauthAccount"].is_object())
        };
        if !has_account(&self.global_config) && has_account(&inner) {
            inner
        } else {
            self.global_config.clone()
        }
    }
}

/// Authentication overrides in the user's `settings.json`. A custom API URL
/// only changes routing and can still use the current subscription login.
pub fn login_overrides(location: &Location) -> Vec<String> {
    read_settings(location)
        .ok()
        .and_then(|text| parse_settings(text.as_deref()).ok())
        .map(|settings| provider_config::overrides(&settings))
        .unwrap_or_default()
}

fn settings_file(location: &Location) -> PathBuf {
    location.config_dir.join("settings.json")
}
/// The main settings as they are; none when the file is missing.
fn read_settings(location: &Location) -> Result<Option<String>> {
    match fs::read_to_string(settings_file(location)) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("Cannot read main Claude settings".into()),
    }
}
fn parse_settings(text: Option<&str>) -> Result<Value> {
    let text = text.filter(|t| !t.trim().is_empty()).unwrap_or("{}");
    let settings: Value =
        serde_json::from_str(text).map_err(|_| "Invalid main Claude settings JSON")?;
    if !settings.is_object() || !settings.get("env").is_none_or(Value::is_object) {
        return Err("Invalid main Claude settings object".into());
    }
    Ok(settings)
}
/// Settings rendered in place of `previous`, keeping its final newline.
fn render_settings(settings: &Value, previous: Option<&str>) -> Result<String> {
    let mut text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    if previous.is_none_or(|t| t.ends_with('\n')) {
        text.push('\n');
    }
    Ok(text)
}
/// Writes the main settings verbatim, or removes them for none.
fn put_settings(location: &Location, text: Option<&str>) -> Result<()> {
    let path = settings_file(location);
    match text {
        Some(text) => write_text(&path, text),
        None if path.exists() => fs::remove_file(path).map_err(|e| e.to_string()),
        None => Ok(()),
    }
}
/// The official main settings recorded on leaving them (`None` when the file
/// was missing), and the provider settings then written in their place.
struct Official {
    settings: Option<String>,
    applied: Option<String>,
}
/// The official settings to return to: the record, with the changes made to
/// the main settings since they were written, except what replaces the login.
/// Unchanged settings return the recorded text verbatim.
fn official_base(
    official: &Official,
    current: &Value,
    text: Option<&str>,
) -> Result<Option<String>> {
    let Some(applied) = &official.applied else {
        return Ok(official.settings.clone());
    };
    let applied = parse_settings(Some(applied))?;
    if applied == *current {
        return Ok(official.settings.clone());
    }
    let base = parse_settings(official.settings.as_deref())?;
    let mut rebased = provider_config::rebase(&base, &applied, current);
    provider_config::remove_overrides(&mut rebased);
    // The login is sent to the address recorded with it, never to one set
    // while a provider's credential was in use.
    let recorded = base["env"].get(BASE_URL);
    match (
        recorded,
        rebased.get_mut("env").and_then(Value::as_object_mut),
    ) {
        (Some(url), Some(env)) => {
            env.insert(BASE_URL.into(), url.clone());
        }
        (None, Some(env)) => {
            env.shift_remove(BASE_URL);
        }
        (Some(url), None) => rebased["env"] = json!({BASE_URL: url}),
        (None, None) => {}
    }
    render_settings(&rebased, text).map(Some)
}
const BASE_URL: &str = "ANTHROPIC_BASE_URL";
/// Settings without what replaces the login. A base URL set beside such a
/// credential is its gateway, and goes with it; official settings keep theirs
/// (a relay that forwards the login, say), as they are left untouched.
fn without_login_replacement(mut settings: Value) -> Value {
    if provider_config::overrides(&settings).is_empty() {
        return settings;
    }
    provider_config::remove_overrides(&mut settings);
    if let Some(env) = settings.get_mut("env").and_then(Value::as_object_mut) {
        env.shift_remove(BASE_URL);
    }
    settings
}
/// Whether Claude Code uses its subscription login with the main settings.
fn settings_are_official(location: &Location) -> bool {
    read_settings(location)
        .and_then(|text| parse_settings(text.as_deref()))
        .is_ok_and(|settings| provider_config::overrides(&settings).is_empty())
}

pub(crate) trait Network: Send + Sync {
    /// The HTTP status and body of the OAuth usage endpoint.
    fn usage<'a>(&'a self, token: &'a str, proxy: &'a str) -> Response<'a, (u16, Value)>;
    /// A token response for a refresh token.
    fn refresh<'a>(&'a self, refresh_token: &'a str, proxy: &'a str) -> Response<'a, Value>;
    fn profile<'a>(&'a self, token: &'a str, proxy: &'a str) -> Response<'a, Value>;
    /// The organization's prepaid balance, which pays for extra usage.
    fn prepaid<'a>(&'a self, token: &'a str, org: &'a str, proxy: &'a str) -> Response<'a, Value>;
}
struct Http;
fn client(proxy: &str) -> Result<reqwest::Client> {
    crate::proxy_session::client(
        proxy,
        crate::proxy_session::Options {
            user_agent: USER_AGENT.into(),
            ..Default::default()
        },
    )
}
// reqwest's Display omits the cause (TLS trust, proxy connection, DNS, etc.).
// Keep the chain without Debug-formatting requests or exposing auth headers.
fn request_error(error: reqwest::Error) -> String {
    use std::error::Error;
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(cause) = source {
        let message = cause.to_string();
        if !parts.contains(&message) {
            parts.push(message);
        }
        source = cause.source();
    }
    parts.join(" -> ")
}
impl Network for Http {
    fn usage<'a>(&'a self, token: &'a str, proxy: &'a str) -> Response<'a, (u16, Value)> {
        Box::pin(async move {
            let response = client(proxy)?
                .get(USAGE_URL)
                .header(reqwest::header::USER_AGENT, USAGE_USER_AGENT)
                .bearer_auth(token.trim())
                .header("Accept", "application/json")
                .header("anthropic-beta", "oauth-2025-04-20")
                .send()
                .await
                .map_err(request_error)?;
            let status = response.status().as_u16();
            Ok((status, response.json().await.unwrap_or(Value::Null)))
        })
    }
    fn refresh<'a>(&'a self, refresh_token: &'a str, proxy: &'a str) -> Response<'a, Value> {
        Box::pin(async move {
            let response = client(proxy)?
                .post(crate::login::CLAUDE_TOKEN_URL)
                .json(&json!({
                    "grant_type": "refresh_token",
                    "refresh_token": refresh_token,
                    "client_id": crate::login::CLAUDE_CLIENT,
                    "scope": crate::login::CLAUDE_SCOPES,
                }))
                .send()
                .await
                .map_err(request_error)?;
            let status = response.status();
            let body: Value = response.json().await.unwrap_or(Value::Null);
            if status.is_success() {
                return Ok(body);
            }
            let code = body["error"]
                .as_str()
                .or_else(|| body["error"]["type"].as_str());
            if matches!(status.as_u16(), 400 | 401) && code == Some("invalid_grant") {
                return Err(SIGN_IN_EXPIRED.into());
            }
            if status.as_u16() == 429 {
                return Err(TOKEN_REFRESH_RATE_LIMITED.into());
            }
            Err(format!(
                "Claude token refresh failed: HTTP {}",
                status.as_u16()
            ))
        })
    }
    fn profile<'a>(&'a self, token: &'a str, proxy: &'a str) -> Response<'a, Value> {
        Box::pin(async move {
            let response = client(proxy)?
                .get(PROFILE_URL)
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
    fn prepaid<'a>(&'a self, token: &'a str, org: &'a str, proxy: &'a str) -> Response<'a, Value> {
        Box::pin(async move {
            let response = client(proxy)?
                .get(format!("{ORGANIZATIONS_URL}/{org}/prepaid/credits"))
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
}

/// The Claude pool's place in the application.
pub(crate) struct State {
    location: std::sync::RwLock<Location>,
    network: std::sync::RwLock<Arc<dyn Network>>,
    lock: tokio::sync::Mutex<()>,
    operations: tokio::sync::Mutex<()>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            location: std::sync::RwLock::new(
                Location::discover().unwrap_or_else(|_| Location::files(".claude".into())),
            ),
            network: std::sync::RwLock::new(Arc::new(Http)),
            lock: Default::default(),
            operations: Default::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Store {
    accounts: Vec<Stored>,
    current_account_id: Option<String>,
    /// Sub2api providers whose Claude accounts join the pool.
    providers: Vec<sub2api::Provider>,
    /// The provider whose settings were last merged into Claude Code's; it
    /// is current while the main settings replace the login.
    settings_provider: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    id: String,
    label: String,
    /// Claude Code's `claudeAiOauth` entry.
    oauth: Value,
    /// Claude Code's `oauthAccount`, written to `.claude.json` on a switch.
    account: Value,
    #[serde(default)]
    proxy_url: String,
    added_at: i64,
    updated_at: i64,
    #[serde(default)]
    usage: Option<UsageSnapshot>,
    #[serde(default)]
    usage_error: Option<String>,
    /// A local backoff after the usage or token endpoint returns HTTP 429.
    #[serde(default)]
    usage_retry_at: Option<i64>,
    /// Newly imported; nothing goes upstream for it until a manual refresh.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    first_request_pending: bool,
}
impl Stored {
    fn summary(&self, current: bool) -> AccountSummary {
        let plan = plan(&self.oauth);
        AccountSummary {
            display_status: "list".into(),
            workspace_status: "active".into(),
            source_tag: None,
            team_name: text(&self.account["organizationName"]),
            available: true,
            proxy_url: self.proxy_url.clone(),
            id: self.id.clone(),
            label: self.label.clone(),
            email: text(&self.account["emailAddress"]),
            account_id: text(&self.account["accountUuid"]).unwrap_or_default(),
            plan_type: plan,
            added_at: self.added_at,
            updated_at: self.updated_at,
            usage: self.usage.clone(),
            usage_error: self.usage_error.clone(),
            is_current: current,
            awaiting_first_request: self.first_request_pending,
            token_expires_at: crate::models::displayed_expiry(expires_at(&self.oauth) / 1000),
        }
    }
}

/// The login Claude Code uses.
struct Live {
    /// The complete credentials document; other entries (MCP logins) are kept.
    document: Value,
    account: Option<Value>,
}
impl Live {
    fn oauth(&self) -> Option<&Value> {
        self.document
            .get("claudeAiOauth")
            .filter(|o| text(&o["accessToken"]).is_some())
    }
}

fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn identity(account: &Value) -> Option<String> {
    let account_id = text(&account["accountUuid"])?;
    Some(format!(
        "{account_id}|{}",
        text(&account["organizationUuid"]).unwrap_or_default()
    ))
}
fn same_tokens(a: &Value, b: &Value) -> bool {
    ["accessToken", "refreshToken"]
        .iter()
        .any(|key| text(&a[key]).is_some_and(|token| text(&b[key]) == Some(token)))
}
/// A card's name: the email, else the display name, else Claude Code for a
/// login not yet identified.
fn label_of(account: &Value) -> String {
    text(&account["emailAddress"])
        .or_else(|| text(&account["displayName"]))
        .unwrap_or_else(|| "Claude Code".into())
}
/// Folds an account just identified into the pool's card for the same
/// account: the newer tokens and this usage stay, as does the selection.
fn merge_identified(store: &mut Store, id: &str) {
    let Some(index) = store.accounts.iter().position(|a| a.id == id) else {
        return;
    };
    let key = identity(&store.accounts[index].account);
    let Some(other) = store
        .accounts
        .iter()
        .position(|a| a.id != id && key.is_some() && identity(&a.account) == key)
    else {
        return;
    };
    let merged = store.accounts.remove(index);
    let other = if other > index { other - 1 } else { other };
    let existing = &mut store.accounts[other];
    if expires_at(&merged.oauth) >= expires_at(&existing.oauth) {
        existing.oauth = merged.oauth;
    }
    if merged.usage.is_some() {
        existing.usage = merged.usage;
        existing.usage_error = merged.usage_error;
    }
    if existing.proxy_url.is_empty() {
        existing.proxy_url = merged.proxy_url;
    }
    existing.first_request_pending = false;
    if store.current_account_id.as_deref() == Some(id) {
        store.current_account_id = Some(existing.id.clone());
    }
}
fn expires_at(oauth: &Value) -> i64 {
    oauth["expiresAt"].as_f64().map_or(0, |v| v as i64)
}
/// "max 20x" from the subscription and its rate limit tier.
fn plan(oauth: &Value) -> Option<String> {
    let subscription = text(&oauth["subscriptionType"])?.to_lowercase();
    let multiplier = text(&oauth["rateLimitTier"]).and_then(|tier| {
        tier.split(|c: char| !c.is_ascii_alphanumeric())
            .find(|p| p.ends_with('x') && p[..p.len() - 1].parse::<u32>().is_ok())
            .map(str::to_owned)
    });
    Some(match multiplier {
        Some(m) => format!("{subscription} {m}"),
        None => subscription,
    })
}
/// The credentials document, as JSON or the hexadecimal Keychain encoding.
fn parse_credentials(bytes: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    serde_json::from_str::<Value>(text)
        .ok()
        .or_else(|| {
            if text.len() % 2 != 0 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let decoded = (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
                .collect::<Option<Vec<_>>>()?;
            serde_json::from_slice(&decoded).ok()
        })
        .filter(Value::is_object)
}
fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
}
fn reset_at(value: &Value) -> Option<i64> {
    if let Some(text) = value.as_str() {
        return chrono::DateTime::parse_from_rfc3339(text.trim())
            .map(|d| d.timestamp())
            .ok()
            .or_else(|| {
                chrono::NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%dT%H:%M:%S%.f")
                    .ok()
                    .map(|d| d.and_utc().timestamp())
            });
    }
    let raw = number(value)?;
    Some(if raw.abs() < 10_000_000_000.0 {
        raw as i64
    } else {
        (raw / 1000.0) as i64
    })
}
/// The five-hour and seven-day windows of the OAuth usage response.
fn usage_snapshot(body: &Value, plan: Option<String>, now: i64) -> Result<UsageSnapshot> {
    if !body.is_object() {
        return Err("Invalid Claude usage response".into());
    }
    let window = |key: &str, seconds| {
        body.get(key)
            .filter(|w| w.is_object())
            .map(|w| UsageWindow {
                used_percent: number(&w["utilization"]).unwrap_or(0.0).clamp(0.0, 100.0),
                window_seconds: seconds,
                reset_at: reset_at(&w["resets_at"]),
            })
    };
    Ok(UsageSnapshot {
        remaining_reset_count: banked_resets(&body["cedar_ember"], now),
        fetched_at: now,
        plan_type: plan,
        five_hour: window("five_hour", 5 * 60 * 60),
        one_week: window("seven_day", 7 * 24 * 60 * 60),
        credits: None,
    })
}
/// What extra usage has left, as Claude Code counts it: the smaller of the
/// monthly limit's remainder and the prepaid balance. A missing limit, or a
/// balance auto-reload tops up, is unbounded; both unbounded is unlimited.
/// Amounts are in minor units; without a balance nothing is known.
fn extra_usage_credits(extra: &Value, prepaid: &Value) -> Option<CreditSnapshot> {
    if !extra.is_object() || !prepaid.is_object() {
        return None;
    }
    let minor = |amount: &Value, exponent: &Value| {
        Some(number(amount)? / 10f64.powf(number(exponent).unwrap_or(2.0)))
    };
    let places = number(&extra["decimal_places"]).unwrap_or(2.0);
    let limit = number(&extra["monthly_limit"])
        .map(|limit| (limit - number(&extra["used_credits"]).unwrap_or(0.0)) / 10f64.powf(places));
    let balance = if prepaid["auto_reload_settings"]["enabled"].as_bool() == Some(true) {
        None
    } else {
        // `amount` is -1 when the balance is held as credits rather than money.
        let held = &prepaid["balance"];
        let balance = ["money", "credits"]
            .iter()
            .find_map(|kind| minor(&held[*kind]["amount_minor"], &held[*kind]["exponent"]))
            .or_else(|| {
                number(&prepaid["amount"])
                    .filter(|a| *a >= 0.0)
                    .map(|a| a / 10f64.powf(places))
            })?;
        Some(balance)
    };
    let left = match (limit, balance) {
        (Some(limit), Some(balance)) => Some(limit.min(balance)),
        (limit, balance) => limit.or(balance),
    }
    .map(|left| left.max(0.0));
    let currency = text(&extra["currency"])
        .or_else(|| text(&prepaid["currency"]))
        .unwrap_or_else(|| "USD".into());
    let places = places.clamp(0.0, 4.0) as usize;
    Some(CreditSnapshot {
        has_credits: left.is_none_or(|left| left > 0.0),
        unlimited: left.is_none(),
        balance: left.map(|amount| match currency.as_str() {
            "USD" => format!("${amount:.places$}"),
            "EUR" => format!("€{amount:.places$}"),
            "GBP" => format!("£{amount:.places$}"),
            code => format!("{code} {amount:.places$}"),
        }),
    })
}
/// The usage-limit resets the account has banked: what its unexpired grants
/// have left, as claude.ai shows them. An ineligible block says nothing about
/// the account, so it is unknown rather than zero. Grant ids, which spend a
/// reset, are never read.
fn banked_resets(block: &Value, now: i64) -> Option<i64> {
    if block["eligible"].as_bool() != Some(true) {
        return None;
    }
    let grants = block["grants"].as_array()?;
    Some(
        grants
            .iter()
            .filter(|g| reset_at(&g["ends_at"]).is_none_or(|end| end > now))
            .filter_map(|g| number(&g["resets_left"]))
            .map(|left| left.max(0.0) as i64)
            .sum(),
    )
}
/// Usage of a Claude account as Sub2api reports it. Sub2api reports an idle
/// five-hour window as zero without a reset, which is no window at all.
pub(crate) fn sub2api_usage(body: &Value, now: i64) -> Result<UsageSnapshot> {
    let mut body = body.clone();
    let idle = body
        .get("five_hour")
        .filter(|w| w.is_object())
        .is_some_and(|w| {
            number(&w["utilization"]).unwrap_or(0.0) <= 0.0
                && reset_at(&w["resets_at"]).is_none()
                && number(&w["remaining_seconds"]).unwrap_or(0.0) <= 0.0
        });
    if idle {
        if let Some(object) = body.as_object_mut() {
            object.remove("five_hour");
        }
    }
    let plan = text(&body["subscription_tier"]).map(|t| t.to_lowercase());
    usage_snapshot(&body, plan, now)
}
/// Applies a token response to a `claudeAiOauth` entry.
fn apply_tokens(oauth: &mut Value, tokens: &Value, now_ms: i64) -> Result<()> {
    let access = text(&tokens["access_token"]).ok_or("Claude returned no access token")?;
    oauth["accessToken"] = access.into();
    if let Some(refresh) = text(&tokens["refresh_token"]) {
        oauth["refreshToken"] = refresh.into();
    }
    if let Some(seconds) = number(&tokens["expires_in"]) {
        oauth["expiresAt"] = (now_ms + (seconds * 1000.0) as i64).into();
    }
    if let Some(scope) = text(&tokens["scope"]) {
        oauth["scopes"] = scope.split_whitespace().collect::<Vec<_>>().into();
    }
    Ok(())
}
/// `oauthAccount` and the subscription from the OAuth profile, else from the
/// account and organization of a token response.
fn profile_account(
    profile: Option<&Value>,
    tokens: &Value,
) -> (Value, Option<String>, Option<String>) {
    let mut account = serde_json::Map::new();
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(value) = value.filter(|v| !v.is_null()) {
            account.insert(key.into(), value);
        }
    };
    let (subscription, tier);
    if let Some(p) = profile {
        put("accountUuid", p["account"].get("uuid").cloned());
        put("emailAddress", p["account"].get("email").cloned());
        put("organizationUuid", p["organization"].get("uuid").cloned());
        put("organizationName", p["organization"].get("name").cloned());
        put("displayName", p["account"].get("display_name").cloned());
        put(
            "billingType",
            p["organization"].get("billing_type").cloned(),
        );
        put(
            "hasExtraUsageEnabled",
            p["organization"].get("has_extra_usage_enabled").cloned(),
        );
        subscription = text(&p["organization"]["organization_type"])
            .map(|t| t.strip_prefix("claude_").unwrap_or(&t).to_owned());
        tier = text(&p["organization"]["rate_limit_tier"]);
    } else {
        put("accountUuid", tokens["account"].get("uuid").cloned());
        put(
            "emailAddress",
            tokens["account"].get("email_address").cloned(),
        );
        put(
            "organizationUuid",
            tokens["organization"].get("uuid").cloned(),
        );
        put(
            "organizationName",
            tokens["organization"].get("name").cloned(),
        );
        subscription = None;
        tier = None;
    }
    (Value::Object(account), subscription, tier)
}

/// Replaces or adds a top-level member of a JSON object and keeps the rest of
/// the document byte for byte, so `.claude.json` is not reordered.
fn set_member(document: &str, key: &str, value: &Value) -> Result<String> {
    serde_json::from_str::<serde_json::Map<String, Value>>(document)
        .map_err(|e| format!("Invalid Claude configuration: {e}"))?;
    let rendered = serde_json::to_string_pretty(value)
        .map_err(|e| e.to_string())?
        .replace('\n', "\n  ");
    let bytes = document.as_bytes();
    let (mut depth, mut i) = (0usize, 0usize);
    let mut expect_key = false;
    let mut current_key: Option<String> = None;
    let mut value_start = None;
    let mut close = None;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let start = i;
                i += 1;
                while bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                if depth == 1 && expect_key {
                    current_key = serde_json::from_str(&document[start..=i]).ok();
                    expect_key = false;
                }
            }
            b':' if depth == 1 => {
                value_start = Some(i + 1);
            }
            b'{' | b'[' => {
                depth += 1;
                if depth == 1 {
                    expect_key = true;
                }
            }
            b',' | b'}' | b']' if depth == 1 => {
                if let (Some(name), Some(start)) = (&current_key, value_start) {
                    if name == key {
                        let span = &document[start..i];
                        let lead = span.len() - span.trim_start().len();
                        let trail = span.len() - span.trim_end().len();
                        return Ok(format!(
                            "{}{rendered}{}",
                            &document[..start + lead],
                            &document[i - trail..]
                        ));
                    }
                }
                current_key = None;
                value_start = None;
                if bytes[i] == b',' {
                    expect_key = true;
                } else {
                    close = Some(i);
                    break;
                }
            }
            b'}' | b']' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    let close = close.ok_or("Invalid Claude configuration")?;
    let head = document[..close].trim_end();
    let separator = if head.ends_with('{') { "" } else { "," };
    let member = serde_json::to_string(key).map_err(|e| e.to_string())?;
    Ok(format!(
        "{head}{separator}\n  {member}: {rendered}\n{}",
        &document[close..]
    ))
}

/// Recurring synchronization stays silent; startup and explicit actions may authorize.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeychainAccess {
    Background,
    Interactive,
}
#[cfg(target_os = "macos")]
mod keychain;

async fn read_live(location: &Location) -> Result<Option<Live>> {
    read_live_with_access(location, KeychainAccess::Background).await
}
async fn read_live_with_access(
    location: &Location,
    access: KeychainAccess,
) -> Result<Option<Live>> {
    #[cfg(not(target_os = "macos"))]
    let _ = access;
    let mut document = None;
    #[cfg(target_os = "macos")]
    if let Some(service) = &location.keychain {
        document = keychain::read(service, access).await?;
    }
    if document.is_none() {
        document = match fs::read(location.credentials_file()) {
            Ok(bytes) => Some(parse_credentials(&bytes).ok_or("Invalid Claude credentials")?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.to_string()),
        };
    }
    let Some(document) = document else {
        return Ok(None);
    };
    let account = match fs::read_to_string(location.account_file()) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.get("oauthAccount").cloned())
            .filter(|a| identity(a).is_some()),
        Err(_) => None,
    };
    Ok(Some(Live { document, account }))
}
async fn write_credentials(
    location: &Location,
    document: &Value,
    access: KeychainAccess,
) -> Result<()> {
    #[cfg(not(target_os = "macos"))]
    let _ = access;
    let file = location.credentials_file();
    if let Some(service) = &location.keychain {
        #[cfg(target_os = "macos")]
        {
            keychain::write(service, document, access).await?;
            // A file Claude Code falls back to must not keep the old login.
            if !file.exists() {
                return Ok(());
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = service;
            return Err("The Keychain is only available on macOS".into());
        }
    }
    write_json(&file, document)
}
/// Removes a login written where there was none, as `write_credentials`
/// would have placed it.
async fn remove_credentials(location: &Location, access: KeychainAccess) -> Result<()> {
    #[cfg(not(target_os = "macos"))]
    let _ = access;
    #[cfg(target_os = "macos")]
    if let Some(service) = &location.keychain {
        keychain::delete(service, access).await?;
    }
    match fs::remove_file(location.credentials_file()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}
fn write_account(location: &Location, account: &Value) -> Result<()> {
    let path = location.account_file();
    let document = match fs::read_to_string(&path) {
        Ok(text) if !text.trim().is_empty() => text,
        Ok(_) => "{}".into(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
        Err(e) => return Err(e.to_string()),
    };
    // An account not yet identified restores a login without oauthAccount;
    // Claude Code names it itself.
    let account = if identity(account).is_some() {
        account.clone()
    } else {
        Value::Null
    };
    let updated = set_member(&document, "oauthAccount", &account)?;
    write_text(&path, &updated)
}
fn write_text(path: &std::path::Path, text: &str) -> Result<()> {
    use std::io::Write;
    // Replace a symlink's target (a dotfiles checkout, say), not the link.
    let path = &fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let parent = path.parent().ok_or("Invalid file path")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Adopts what Claude Code changed: the current account follows its login,
/// and tokens it refreshed replace older stored ones. A missing login keeps
/// the stored selection. Returns whether the store changed.
fn follow_live(store: &mut Store, live: Option<&Live>) -> bool {
    let Some(live) = live else {
        return false;
    };
    let oauth = live.oauth();
    let token = |o: &Value, key: &str| text(&o[key]);
    let matched = store.accounts.iter().position(|a| match &live.account {
        Some(account) => identity(account).is_some() && identity(account) == identity(&a.account),
        None => oauth.is_some_and(|o| {
            token(o, "refreshToken").is_some()
                && token(o, "refreshToken") == token(&a.oauth, "refreshToken")
        }),
    });
    // A read taken mid-switch pairs the target's tokens with the account being
    // left; tokens that belong to another pool account are not adopted.
    if let (Some(i), Some(oauth)) = (matched, oauth) {
        if store
            .accounts
            .iter()
            .enumerate()
            .any(|(j, a)| j != i && same_tokens(&a.oauth, oauth))
        {
            return false;
        }
    }
    let mut changed = false;
    let current = matched.map(|i| store.accounts[i].id.clone());
    if store.current_account_id != current {
        store.current_account_id = current;
        changed = true;
    }
    if let (Some(i), Some(oauth)) = (matched, oauth) {
        let stored = &mut store.accounts[i];
        let rotated = ["accessToken", "refreshToken"]
            .iter()
            .any(|key| token(oauth, key) != token(&stored.oauth, key));
        if rotated && expires_at(oauth) >= expires_at(&stored.oauth) {
            stored.oauth = oauth.clone();
            stored.usage_error = None;
            changed = true;
        }
    }
    changed
}

// Compare only the credential generation, not unrelated profile/MCP metadata.
fn same_oauth_generation(a: &Value, b: &Value) -> bool {
    ["accessToken", "refreshToken", "expiresAt"]
        .iter()
        .all(|key| a[*key] == b[*key])
}

/// Only adopt an observed change for this account. A different live login must
/// never replace the account being refreshed, even if its expiry is later.
fn changed_live_oauth(
    account: &Stored,
    before: Option<&Live>,
    after: Option<&Live>,
) -> Option<Value> {
    let after = after?;
    let oauth = after.oauth()?;
    let named = after.account.as_ref().and_then(identity)?;
    let expected = identity(&account.account).or_else(|| {
        let before = before?;
        let previous = before.oauth()?;
        let shares_token = ["accessToken", "refreshToken"].iter().any(|key| {
            text(&previous[*key]).is_some() && text(&previous[*key]) == text(&account.oauth[*key])
        });
        shares_token
            .then(|| before.account.as_ref().and_then(identity))
            .flatten()
    })?;
    if named != expected {
        return None;
    }
    if let Some(previous) = before.and_then(Live::oauth) {
        if same_oauth_generation(previous, oauth) {
            return None;
        }
    } else if same_oauth_generation(&account.oauth, oauth)
        || expires_at(oauth) < expires_at(&account.oauth)
    {
        return None;
    }
    Some(oauth.clone())
}

const CREDENTIALS_CHANGED: &str = "Claude credentials changed during refresh; retry usage";
/// Renewing rotates the refresh token: the login Claude Code may be using is
/// not renewed unless the new tokens can be seen to reach it.
const LIVE_UNREADABLE: &str =
    "Cannot read the Claude Code login; allow access before renewing the account it uses";

impl Application {
    fn claude_store_path(&self) -> PathBuf {
        self.paths.data_dir.join("claude-accounts.json")
    }
    fn claude_load(&self) -> Result<Store> {
        match fs::read(self.claude_store_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("Invalid Claude account store: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
            Err(e) => Err(e.to_string()),
        }
    }
    fn claude_save(&self, store: &Store) -> Result<()> {
        write_json(&self.claude_store_path(), store)?;
        self.publish();
        Ok(())
    }
    /// Where the official main settings are recorded on leaving them.
    fn claude_official_path(&self) -> PathBuf {
        self.paths.data_dir.join("claude-official-settings.json")
    }
    /// The official main settings last recorded, with the main settings
    /// written over them.
    fn claude_official_settings(&self) -> Result<Option<Official>> {
        let invalid = || "Invalid recorded official Claude settings".to_owned();
        match fs::read(self.claude_official_path()) {
            Ok(bytes) => {
                let record: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
                let settings = match &record["settings"] {
                    Value::Null => None,
                    Value::String(text) => Some(text.clone()),
                    _ => return Err(invalid()),
                };
                let applied = match &record["applied"] {
                    Value::Null => None,
                    Value::String(text) => Some(text.clone()),
                    _ => return Err(invalid()),
                };
                Ok(Some(Official { settings, applied }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn claude_record_official(&self, official: &Official) -> Result<()> {
        write_json(
            &self.claude_official_path(),
            &json!({"settings": official.settings, "applied": official.applied}),
        )
    }
    pub fn claude_location(&self) -> Location {
        self.claude.location.read().unwrap().clone()
    }
    pub fn set_claude_location(&self, location: Location) {
        *self.claude.location.write().unwrap() = location;
    }
    #[cfg(test)]
    pub(crate) fn set_claude_network(&self, network: Arc<dyn Network>) {
        *self.claude.network.write().unwrap() = network;
    }
    fn claude_network(&self) -> Arc<dyn Network> {
        self.claude.network.read().unwrap().clone()
    }
    /// Request access once at startup, before recurring refreshes. A refusal
    /// leaves the saved pool intact; importing or switching can request again.
    pub(crate) async fn claude_authorize_at_startup(&self) -> Result<()> {
        if self.claude_load()?.accounts.is_empty() {
            return Ok(());
        }
        let _op = self.claude.operations.lock().await;
        let live =
            read_live_with_access(&self.claude_location(), KeychainAccess::Interactive).await?;
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        if follow_live(&mut store, live.as_ref()) {
            self.claude_save(&store)?;
        }
        Ok(())
    }
    /// The Claude pool, following the login Claude Code uses.
    pub async fn claude_accounts(&self) -> Result<Vec<AccountSummary>> {
        let location = self.claude_location();
        let saved = self.claude_load()?;
        // A pool without account logins never reads the Keychain.
        let live = if saved.accounts.is_empty() {
            None
        } else {
            read_live(&location).await.ok().flatten()
        };
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        if follow_live(&mut store, live.as_ref()) {
            self.claude_save(&store)?;
        }
        // Official settings use the login; others, the provider last applied.
        let official = settings_are_official(&location);
        let provider = store.settings_provider.as_deref().filter(|_| !official);
        let mut summaries: Vec<_> = store
            .accounts
            .iter()
            .map(|a| {
                a.summary(official && store.current_account_id.as_deref() == Some(a.id.as_str()))
            })
            .chain(sub2api::summaries(&store, provider))
            .collect();
        crate::ranking::sort_for_display(&mut summaries);
        Ok(summaries)
    }
    /// Adds or updates an account; a new one waits for a manual refresh when
    /// `defer`. Returns its id, label and whether it waits.
    async fn claude_upsert(
        &self,
        oauth: Value,
        account: Value,
        current: bool,
        defer: bool,
    ) -> Result<(String, String, bool)> {
        // A login without its account is matched by its tokens until a refresh
        // identifies it.
        let key = identity(&account);
        let now = now_unix_seconds();
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        let label = label_of(&account);
        let (id, awaiting) = match store.accounts.iter_mut().find(|a| match &key {
            Some(key) => identity(&a.account).as_ref() == Some(key),
            None => same_tokens(&a.oauth, &oauth),
        }) {
            Some(existing) => {
                // An older login (an earlier export, say) holds a refresh token
                // the pool has since rotated.
                if expires_at(&oauth) >= expires_at(&existing.oauth) {
                    existing.oauth = oauth;
                }
                if key.is_some() {
                    existing.account = account;
                }
                existing.updated_at = now;
                existing.usage_error = None;
                (existing.id.clone(), existing.first_request_pending)
            }
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                store.accounts.push(Stored {
                    id: id.clone(),
                    label: label.clone(),
                    oauth,
                    account,
                    proxy_url: String::new(),
                    added_at: now,
                    updated_at: now,
                    usage: None,
                    usage_error: None,
                    usage_retry_at: None,
                    first_request_pending: defer,
                });
                (id, defer)
            }
        };
        if current {
            store.current_account_id = Some(id.clone());
        }
        self.claude_save(&store)?;
        Ok((id, label, awaiting))
    }
    /// Resolve an existing account before requesting its profile. New accounts
    /// have no saved proxy yet; never borrow the currently selected account's.
    fn claude_profile_proxy(&self, oauth: &Value, account: Option<&Value>) -> Result<String> {
        Ok(self
            .claude_known(oauth, account)?
            .map(|known| known.proxy_url)
            .unwrap_or_default())
    }
    /// The pool's account for this login, by its identity or else its
    /// tokens; none for a new account.
    fn claude_known(&self, oauth: &Value, account: Option<&Value>) -> Result<Option<Stored>> {
        let account_identity = account.and_then(identity);
        let store = self.claude_load()?;
        let mut matches = store.accounts.iter().filter(|stored| {
            if let Some(expected) = &account_identity {
                return identity(&stored.account).as_ref() == Some(expected);
            }
            same_tokens(&stored.oauth, oauth)
        });
        let Some(first) = matches.next() else {
            return Ok(None);
        };
        if matches.any(|other| other.proxy_url != first.proxy_url) {
            return Err("Cannot determine the Claude account proxy unambiguously".into());
        }
        Ok(Some(first.clone()))
    }
    /// Whether this login may contact upstream now: a known account that no
    /// longer waits, or a new one when the user opted in.
    fn claude_may_contact(&self, known: Option<&Stored>) -> Result<bool> {
        Ok(match known {
            Some(known) => !known.first_request_pending,
            None => self.preferences()?.request_new_accounts,
        })
    }
    /// Imports the login Claude Code uses; returns the account label.
    pub async fn claude_import_current(&self) -> Result<String> {
        let _op = self.claude.operations.lock().await;
        let live = read_live_with_access(&self.claude_location(), KeychainAccess::Interactive)
            .await?
            .ok_or("error.claude.login_not_found")?;
        let (id, label, waiting) = self.claude_import_live(live, true).await?;
        drop(_op);
        if !waiting {
            let _ = self.claude_refresh_usage_scoped(&id, false).await;
        }
        Ok(label)
    }
    /// Imports an exported Claude Code credentials JSON without selecting it.
    pub async fn claude_import_json(&self, content: &str) -> Result<usize> {
        if content.len() > 16 * 1024 * 1024 {
            return Err("Import exceeds 16 MiB".into());
        }
        let invalid = "error.claude.credentials_json_invalid";
        let document: Value =
            serde_json::from_str(content.trim_start_matches('\u{feff}')).map_err(|_| invalid)?;
        let oauth = document.get("claudeAiOauth").unwrap_or(&document);
        if !oauth.is_object()
            || text(&oauth["accessToken"]).is_none()
            || oauth.get("refreshToken").is_some_and(|v| text(v).is_none())
            || oauth.get("expiresAt").is_some_and(|v| v.as_i64().is_none())
            || oauth.get("scopes").is_some_and(|v| {
                v.as_array()
                    .is_none_or(|items| items.iter().any(|v| !v.is_string()))
            })
        {
            return Err(invalid.into());
        }
        let live = Live {
            account: document
                .get("oauthAccount")
                .filter(|a| identity(a).is_some())
                .cloned(),
            document: json!({"claudeAiOauth": oauth}),
        };
        let _op = self.claude.operations.lock().await;
        let (id, _, waiting) = self.claude_import_live(live, false).await?;
        drop(_op);
        if !waiting {
            let _ = self.claude_refresh_usage_scoped(&id, false).await;
        }
        Ok(1)
    }
    async fn claude_import_live(
        &self,
        live: Live,
        current: bool,
    ) -> Result<(String, String, bool)> {
        let oauth = live.oauth().ok_or("error.claude.login_not_found")?.clone();
        let account = match live.account.clone() {
            Some(account) => account,
            // Only the profile names a login without oauthAccount. Until it
            // may be asked, or when it fails, the account joins unnamed and
            // its first refresh probes it.
            None => match self.claude_known(&oauth, None)? {
                Some(known) => known.account,
                None if self.preferences()?.request_new_accounts => {
                    let token = text(&oauth["accessToken"]).unwrap_or_default();
                    match self.claude_network().profile(&token, "").await {
                        Ok(profile) => profile_account(Some(&profile), &Value::Null).0,
                        Err(_) => json!({}),
                    }
                }
                None => json!({}),
            },
        };
        let defer = !self.preferences()?.request_new_accounts;
        self.claude_upsert(oauth, account, current, defer).await
    }
    /// Stores a browser sign-in without changing Claude Code's login.
    pub async fn claude_import_login(&self, tokens: Value) -> Result<String> {
        let mut oauth = json!({});
        apply_tokens(&mut oauth, &tokens, now_unix_seconds() * 1000)?;
        let token = text(&oauth["accessToken"]).unwrap_or_default();
        let account = profile_account(None, &tokens).0;
        let known = self.claude_known(&oauth, Some(&account))?;
        // The token response names the account; the profile adds its plan,
        // later on the first refresh for an account that waits.
        let profile = if self.claude_may_contact(known.as_ref())? {
            let proxy = known.map(|known| known.proxy_url).unwrap_or_default();
            self.claude_network().profile(&token, &proxy).await.ok()
        } else {
            None
        };
        let (account, subscription, tier) = profile_account(profile.as_ref(), &tokens);
        if let Some(subscription) = subscription {
            oauth["subscriptionType"] = subscription.into();
        }
        if let Some(tier) = tier {
            oauth["rateLimitTier"] = tier.into();
        }
        let defer = !self.preferences()?.request_new_accounts;
        let _op = self.claude.operations.lock().await;
        let (id, label, waiting) = self.claude_upsert(oauth, account, false, defer).await?;
        drop(_op);
        if !waiting {
            let _ = self.claude_refresh_usage_scoped(&id, false).await;
        }
        Ok(label)
    }
    pub async fn claude_delete_account(&self, id: &str) -> Result<()> {
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        if sub2api::is_card(id) {
            self.claude_remove_sub2api_card(&mut store, id)?;
            return self.claude_save(&store);
        }
        let before = store.accounts.len();
        store.accounts.retain(|a| a.id != id);
        if store.accounts.len() == before {
            return Err("Account not found".into());
        }
        if store.current_account_id.as_deref() == Some(id) {
            store.current_account_id = None;
        }
        self.claude_save(&store)
    }
    pub async fn claude_save_account_proxy(&self, id: &str, value: &str) -> Result<()> {
        crate::utils::validate_account_proxy(value)?;
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        if sub2api::is_card(id) {
            self.claude_sub2api_card_proxy(&mut store, id, value)?;
            return self.claude_save(&store);
        }
        store
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or("Account not found")?
            .proxy_url = value.trim().into();
        self.claude_save(&store)
    }
    /// Gives Claude Code this account's login and identity. A previous login
    /// outside the pool is kept in `claude-before-switch.json`; a failed step
    /// is rolled back.
    pub async fn claude_switch_account(&self, id: &str) -> Result<SwitchExecution> {
        self.claude_switch_with_access(id, KeychainAccess::Interactive)
            .await
    }
    async fn claude_switch_with_access(
        &self,
        id: &str,
        access: KeychainAccess,
    ) -> Result<SwitchExecution> {
        let _op = self.claude.operations.lock().await;
        if sub2api::is_card(id) {
            return self.claude_switch_sub2api(id).await;
        }
        // Keep tokens Claude Code refreshed for the account being left.
        self.claude_accounts().await?;
        let location = self.claude_location();
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        let live = read_live_with_access(&location, access).await?;
        if follow_live(&mut store, live.as_ref()) {
            self.claude_save(&store)?;
        }
        let target = store
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?
            .clone();
        // Settings that replace the login give way to the official ones
        // recorded when leaving them, or else lose only what replaces it.
        let settings = read_settings(&location)?;
        let current = parse_settings(settings.as_deref())?;
        // A record not yet restored comes first: a provider's credential
        // removed by hand still leaves its gateway.
        let mut restoring_record = false;
        let official = if let Some(recorded) = self.claude_official_settings()? {
            restoring_record = true;
            // Changes made while a provider was applied are kept.
            Some(official_base(&recorded, &current, settings.as_deref())?)
        } else if provider_config::overrides(&current).is_empty() {
            None
        } else {
            let stripped = without_login_replacement(current);
            Some(Some(render_settings(&stripped, settings.as_deref())?))
        };
        let previous_document = live.as_ref().map(|l| l.document.clone());
        let previous_account = live.as_ref().and_then(|l| l.account.clone());
        // A login the pool holds is not backed up: it would replace the backup
        // of one it does not hold, whose refresh token is kept nowhere else.
        let pooled = live.as_ref().is_some_and(|live| {
            store.accounts.iter().any(|a| {
                live.oauth()
                    .is_some_and(|oauth| same_tokens(&a.oauth, oauth))
                    || live
                        .account
                        .as_ref()
                        .and_then(identity)
                        .is_some_and(|named| identity(&a.account) == Some(named))
            })
        });
        if live.is_some() && !pooled {
            write_json(
                &self.paths.data_dir.join("claude-before-switch.json"),
                &json!({"credentials": previous_document, "oauthAccount": previous_account}),
            )?;
        }
        let mut document = previous_document.clone().unwrap_or_else(|| json!({}));
        document["claudeAiOauth"] = target.oauth.clone();
        let rollback = |error: String| async {
            // Without a previous login, none is left either.
            let restored = match &previous_document {
                Some(previous) => write_credentials(&location, previous, access).await,
                None => remove_credentials(&location, access).await,
            };
            restored
                .map_err(|rollback| format!("{error}; rollback failed: {rollback}"))
                .and(Err(error))
        };
        // The Keychain may already hold the target when the file write fails.
        if let Err(error) = write_credentials(&location, &document, access).await {
            return rollback(error).await;
        }
        if let Err(error) = write_account(&location, &target.account) {
            return rollback(error).await;
        }
        // Without a previous account, the target's must not stay either.
        let restore_account = || {
            let previous = previous_account.as_ref().unwrap_or(&Value::Null);
            let _ = write_account(&location, previous);
        };
        if let Some(official) = &official {
            if let Err(error) = put_settings(&location, official.as_deref()) {
                restore_account();
                return rollback(error).await;
            }
        }
        store.current_account_id = Some(id.into());
        store.settings_provider = None;
        if let Err(error) = self.claude_save(&store) {
            if official.is_some() {
                let _ = put_settings(&location, settings.as_deref());
            }
            restore_account();
            return rollback(error).await;
        }
        // Once restored, the record would only go stale as the official
        // settings change; leaving them again records them anew.
        if restoring_record {
            let _ = fs::remove_file(self.claude_official_path());
        }
        Ok(SwitchExecution {
            login_overrides: login_overrides(&location),
            ..Default::default()
        })
    }
    /// As the Codex pool's Smart Switch, over the Claude accounts.
    pub async fn claude_smart_switch(&self) -> Result<SmartSwitchOutcome> {
        let accounts = self.claude_accounts().await?;
        let score = crate::ranking::remaining_score;
        let best =
            accounts
                .iter()
                .filter(|a| a.available)
                .fold(None::<&AccountSummary>, |best, a| match best {
                    Some(b) if score(a) <= score(b) => Some(b),
                    _ => Some(a),
                });
        let Some(best) = best else {
            return Ok(SmartSwitchOutcome::NoTarget);
        };
        if best.is_current {
            return Ok(SmartSwitchOutcome::AlreadyBest);
        }
        let execution = self.claude_switch_account(&best.id).await?;
        Ok(SmartSwitchOutcome::Switched {
            label: best.label.clone(),
            execution,
        })
    }
    /// Renew pool credentials and sync an unchanged live login without showing
    /// authorization UI. External rotations take precedence over our response.
    pub async fn claude_refresh_usage(&self, id: &str) -> Result<()> {
        self.claude_refresh_usage_scoped(id, true).await
    }
    async fn claude_refresh_usage_scoped(&self, id: &str, use_account_proxy: bool) -> Result<()> {
        if sub2api::is_card(id) {
            return self.claude_refresh_sub2api_card(id).await;
        }
        let _activity = self.begin_refresh(id);
        // Serialize renewal with imports/switches and other pool refreshes.
        let _op = self.claude.operations.lock().await;
        let location = self.claude_location();
        // Adopt newer external credentials when accessible, but the pool remains
        // usable even when the live login is locked, missing or unreadable.
        let mut live = read_live(&location).await.ok().flatten();
        let (account, in_use) = {
            let _lock = self.claude.lock.lock().await;
            let mut store = self.claude_load()?;
            if follow_live(&mut store, live.as_ref()) {
                self.claude_save(&store)?;
            }
            // An unreadable login keeps the selection last seen.
            let in_use = store.current_account_id.as_deref() == Some(id);
            let account = store
                .accounts
                .into_iter()
                .find(|a| a.id == id)
                .ok_or("Account not found")?;
            (account, in_use)
        };
        if account.first_request_pending {
            return Ok(());
        }
        if account
            .usage_retry_at
            .is_some_and(|until| until > now_unix_seconds())
        {
            return Err(account.usage_error.clone().unwrap_or_else(|| {
                "Claude usage request failed: HTTP 429; retry paused for 5 minutes".into()
            }));
        }
        let network = self.claude_network();
        let request_proxy = if use_account_proxy {
            self.account_proxy(&account.proxy_url)?
        } else {
            account.proxy_url.clone()
        };
        let proxy = request_proxy.as_str();
        // A refresh rotates the refresh token, so its result is kept even
        // when the usage request then fails.
        let mut oauth = account.oauth.clone();
        let mut profiled = None;
        let mut rate_limited = false;
        let mut reloads = 0;
        let mut live_sync = Ok(());
        let result = loop {
            let result = async {
                let refresh = |oauth: &Value| {
                    let network = network.clone();
                    let token = text(&oauth["refreshToken"]);
                    let location = &location;
                    let account = &account;
                    let before = live.as_ref();
                    async move {
                        // Another app may have renewed while our usage request ran.
                        let latest = match read_live(location).await {
                            Ok(latest) => latest,
                            Err(_) if in_use => return Err(LIVE_UNREADABLE.into()),
                            Err(_) => None,
                        };
                        if changed_live_oauth(account, before, latest.as_ref()).is_some() {
                            return Err(CREDENTIALS_CHANGED.into());
                        }
                        let token = token.ok_or(SIGN_IN_EXPIRED)?;
                        network.refresh(&token, proxy).await
                    }
                };
                if expires_at(&oauth) != 0 && expires_at(&oauth) <= (now_unix_seconds() + 60) * 1000
                {
                    let tokens = refresh(&oauth).await?;
                    apply_tokens(&mut oauth, &tokens, now_unix_seconds() * 1000)?;
                }
                // An account imported without its name is probed first, through
                // its own proxy.
                if identity(&account.account).is_none() {
                    let token = text(&oauth["accessToken"]).unwrap_or_default();
                    let found = match network.profile(&token, proxy).await {
                        Err(error) if error == SIGN_IN_EXPIRED => {
                            let tokens = refresh(&oauth).await?;
                            apply_tokens(&mut oauth, &tokens, now_unix_seconds() * 1000)?;
                            let token = text(&oauth["accessToken"]).unwrap_or_default();
                            network.profile(&token, proxy).await
                        }
                        found => found,
                    }?;
                    let (named, subscription, tier) = profile_account(Some(&found), &Value::Null);
                    if identity(&named).is_none() {
                        return Err("Claude did not report the account".into());
                    }
                    if let Some(subscription) = subscription {
                        oauth["subscriptionType"] = subscription.into();
                    }
                    if let Some(tier) = tier {
                        oauth["rateLimitTier"] = tier.into();
                    }
                    profiled = Some(named);
                }
                let token = text(&oauth["accessToken"]).unwrap_or_default();
                let (mut status, mut body) = network.usage(&token, proxy).await?;
                if status == 401 {
                    let tokens = refresh(&oauth).await?;
                    apply_tokens(&mut oauth, &tokens, now_unix_seconds() * 1000)?;
                    let token = text(&oauth["accessToken"]).unwrap_or_default();
                    (status, body) = network.usage(&token, proxy).await?;
                }
                // An account added without its profile learns its plan here,
                // through its own proxy.
                if (200..=299).contains(&status)
                    && profiled.is_none()
                    && text(&oauth["subscriptionType"]).is_none()
                {
                    let token = text(&oauth["accessToken"]).unwrap_or_default();
                    if let Ok(found) = network.profile(&token, proxy).await {
                        let (named, subscription, tier) =
                            profile_account(Some(&found), &Value::Null);
                        if identity(&named) == identity(&account.account) {
                            if let Some(subscription) = subscription {
                                oauth["subscriptionType"] = subscription.into();
                            }
                            if let Some(tier) = tier {
                                oauth["rateLimitTier"] = tier.into();
                            }
                            profiled = Some(named);
                        }
                    }
                }
                rate_limited = status == 429;
                match status {
                    200..=299 => {
                        let mut usage = usage_snapshot(&body, plan(&oauth), now_unix_seconds())?;
                        // Extra usage is paid from the organization's prepaid
                        // balance, asked for through the same proxy.
                        let org = text(
                            &profiled.as_ref().unwrap_or(&account.account)["organizationUuid"],
                        );
                        if let (true, Some(org)) = (body["extra_usage"].is_object(), org) {
                            let token = text(&oauth["accessToken"]).unwrap_or_default();
                            if let Ok(prepaid) = network.prepaid(&token, &org, proxy).await {
                                usage.credits = extra_usage_credits(&body["extra_usage"], &prepaid);
                            }
                        }
                        Ok(usage)
                    }
                    401 => Err(SIGN_IN_EXPIRED.to_string()),
                    _ => {
                        let message = body["error"]["message"].as_str().unwrap_or_default();
                        Err(
                            format!("Claude usage request failed: HTTP {status} {message}")
                                .trim()
                                .to_owned(),
                        )
                    }
                }
            }
            .await;
            // Renewal can fail before usage is requested, including after a
            // 401 retry. Apply the same persisted cooldown to all renewal paths.
            rate_limited |= result
                .as_ref()
                .err()
                .is_some_and(|error| error == TOKEN_REFRESH_RATE_LIMITED);
            // Check on success AND failure: another app may win a rotation
            // while our refresh is in flight. Never overwrite that generation
            // with our response. Retry the read with the adopted credentials once.
            let latest = read_live(&location).await.ok().flatten();
            if let Some(updated) = changed_live_oauth(&account, live.as_ref(), latest.as_ref()) {
                oauth = updated;
                live = latest;
                profiled = None;
                rate_limited = false;
                if reloads == 0 {
                    reloads += 1;
                    continue;
                }
                break Err(CREDENTIALS_CHANGED.to_owned());
            }
            // Only write back when the same live login is still at the
            // generation we read before the request. Never replace a switch or
            // an external rotation. Keep all unrelated credential members.
            if let (Some(before), Some(latest)) = (live.as_ref(), latest.as_ref()) {
                if let (Some(old), Some(current)) = (before.oauth(), latest.oauth()) {
                    let same_account = match latest.account.as_ref().and_then(identity) {
                        Some(named) => Some(named) == identity(&account.account),
                        None => ["accessToken", "refreshToken"].iter().any(|key| {
                            text(&current[*key]).is_some()
                                && text(&current[*key]) == text(&account.oauth[*key])
                        }),
                    };
                    if same_account
                        && same_oauth_generation(old, current)
                        && !same_oauth_generation(&oauth, &account.oauth)
                        && !same_oauth_generation(&oauth, current)
                    {
                        let mut document = latest.document.clone();
                        document["claudeAiOauth"] = oauth.clone();
                        live_sync =
                            write_credentials(&location, &document, KeychainAccess::Background)
                                .await;
                    }
                }
            }
            break result;
        };
        let refreshed = Some(oauth).filter(|oauth| *oauth != account.oauth);
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        if let Some(target) = store.accounts.iter_mut().find(|a| a.id == id) {
            target.updated_at = now_unix_seconds();
            target.usage_retry_at = rate_limited.then(|| now_unix_seconds() + 300);
            match &result {
                Ok(usage) => {
                    target.usage = Some(usage.clone());
                    target.usage_error = live_sync.as_ref().err().cloned();
                }
                Err(error) => target.usage_error = Some(error.clone()),
            }
            if let Some(oauth) = refreshed {
                target.oauth = oauth;
            }
            if let Some(named) = profiled {
                if identity(&target.account).is_none() {
                    target.label = label_of(&named);
                    target.account = named;
                } else if let Some(stored) = target.account.as_object_mut() {
                    for (key, value) in named.as_object().into_iter().flatten() {
                        stored.insert(key.clone(), value.clone());
                    }
                }
            }
            merge_identified(&mut store, id);
            self.claude_save(&store)?;
        }
        result.map(|_| ()).and(live_sync)
    }
    /// A manual refresh lets new accounts contact upstream: one account, or
    /// all of them without an id.
    pub async fn claude_release_new_accounts(&self, id: Option<&str>) -> Result<()> {
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        let mut changed = false;
        for account in store
            .accounts
            .iter_mut()
            .filter(|a| id.is_none_or(|id| a.id == id))
        {
            changed |= std::mem::take(&mut account.first_request_pending);
        }
        if changed {
            self.claude_save(&store)?;
        }
        Ok(())
    }
    /// Every account and every provider, or only the ids between full refreshes.
    pub async fn claude_refresh(&self, ids: Option<Vec<String>>) -> Result<()> {
        self.claude_refresh_scoped(ids.clone(), ids.as_deref())
            .await
    }
    /// Accounts by `ids`, provider cards by their own set: `None` refreshes
    /// every provider, an empty slice none at all.
    async fn claude_refresh_scoped(
        &self,
        ids: Option<Vec<String>>,
        cards: Option<&[String]>,
    ) -> Result<()> {
        let accounts = self.claude_accounts().await?;
        let refreshes = accounts
            .iter()
            .filter(|a| !sub2api::is_card(&a.id))
            .filter(|a| ids.as_ref().is_none_or(|ids| ids.contains(&a.id)))
            .map(|a| async move {
                self.claude_refresh_usage(&a.id)
                    .await
                    .map_err(|error| format!("{}: {error}", a.label))
            });
        let (accounts, providers) = tokio::join!(
            futures_util::future::join_all(refreshes),
            self.claude_refresh_sub2api_cards(cards)
        );
        let errors: Vec<String> = accounts
            .into_iter()
            .filter_map(Result::err)
            .chain(providers)
            .collect();
        self.claude_auto_switch_if_needed().await?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }
    /// Like the Codex pool, an automatic tick touches only the provider in use:
    /// its cards refresh on every tick, and other providers wait for an
    /// explicit refresh.
    pub async fn claude_refresh_recurring(&self, tick: u64) -> Result<()> {
        let accounts = self.claude_accounts().await?;
        if accounts.is_empty() {
            return Ok(());
        }
        let ids = crate::refresh_policy::recurring_ids(tick, &accounts);
        let cards: Vec<String> = accounts
            .iter()
            .filter(|a| a.is_current && sub2api::is_card(&a.id))
            .map(|a| a.id.clone())
            .collect();
        self.claude_refresh_scoped(ids, Some(&cards)).await
    }
    /// The Codex pool's automatic switch rules, over the Claude accounts.
    async fn claude_auto_switch_if_needed(&self) -> Result<Option<String>> {
        if !self.preferences()?.auto_switch {
            return Ok(None);
        }
        let accounts = self.claude_accounts().await?;
        let fresh = |a: &AccountSummary| {
            a.usage_error.is_none()
                && a.usage
                    .as_ref()
                    .is_some_and(|u| (0..=900).contains(&(now_unix_seconds() - u.fetched_at)))
        };
        if !accounts.iter().any(|a| a.is_current && fresh(a)) {
            return Ok(None);
        }
        let candidates: Vec<_> = accounts
            .into_iter()
            .filter(|a| a.is_current || crate::application::eligible(a))
            .collect();
        if let Some(target) = crate::ranking::auto_switch_target(&candidates) {
            self.claude_switch_with_access(&target.id, KeychainAccess::Background)
                .await?;
            return Ok(Some(target.id.clone()));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Preferences;
    use std::sync::Mutex;

    #[test]
    fn banked_resets_count_what_unexpired_grants_have_left() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-12T14:30:00Z")
            .unwrap()
            .timestamp();
        // A grant as /api/oauth/usage?cedar_ember=1 returned it to a Max account.
        let grant = |left: i64, ends: &str| {
            json!({"id":"grant-id","label":"Claude Opus 5.5 launch: one usage-limit reset for Pro and Max",
                "resets_total":1,"resets_left":left,"starts_at":"2026-09-22T16:00:00+00:00","ends_at":ends,
                "clears":["five_hour","seven_day","seven_day_overage_included"],"paused":false,"usable_now":left > 0})
        };
        let usage = |block: Value| {
            usage_snapshot(
                &json!({"five_hour":{"utilization":5.0,"resets_at":"2026-10-12T17:39:59.634467+00:00"},
                    "seven_day":{"utilization":24.0,"resets_at":"2026-10-17T06:00:00.634492+00:00"},
                    "cedar_ember":block,"extra_usage":{"is_enabled":false,"monthly_limit":4000,"used_credits":1285.0,
                        "utilization":32.125,"currency":"USD","decimal_places":2,"disabled_reason":"out_of_credits"}}),
                None,
                now,
            )
            .unwrap()
        };
        let eligible = |grants: Vec<Value>| json!({"eligible":true,"ineligible_reason":null,"grants":grants,"next_grant_id":"next"});
        let live = usage(eligible(vec![grant(1, "2026-10-22T16:00:00+00:00")]));
        assert_eq!(live.remaining_reset_count, Some(1));
        assert_eq!(live.five_hour.as_ref().unwrap().used_percent, 5.0);
        assert_eq!(live.one_week.as_ref().unwrap().used_percent, 24.0);
        // Spent, expired and absent grants are a real zero.
        for grants in [
            vec![grant(0, "2026-10-22T16:00:00+00:00")],
            vec![grant(1, "2026-10-01T00:00:00+00:00")],
            vec![],
        ] {
            assert_eq!(usage(eligible(grants)).remaining_reset_count, Some(0));
        }
        let several = vec![
            grant(1, "2026-10-22T16:00:00+00:00"),
            grant(2, "2026-11-01T00:00:00+00:00"),
            grant(-3, "2026-11-01T00:00:00+00:00"),
        ];
        assert_eq!(usage(eligible(several)).remaining_reset_count, Some(3));
        // An ineligible caller, or a response without the block, is unknown.
        for block in [
            json!({"eligible":false,"ineligible_reason":"surface"}),
            json!({"eligible":false,"ineligible_reason":"cli_version","grants":[]}),
            json!({"eligible":true}),
            Value::Null,
        ] {
            assert_eq!(usage(block).remaining_reset_count, None);
        }
    }

    #[test]
    fn extra_usage_credits_are_the_smaller_of_the_limit_remainder_and_the_balance() {
        // As /api/oauth/usage and /prepaid/credits returned them for a Max account.
        let extra = json!({"is_enabled":false,"monthly_limit":4000,"used_credits":1285.0,
            "currency":"USD","decimal_places":2,"disabled_reason":"out_of_credits"});
        let credits = |extra: &Value, prepaid: Value| {
            extra_usage_credits(extra, &prepaid).map(|c| (c.balance, c.unlimited, c.has_credits))
        };
        let held = |minor: i64| {
            json!({"amount":-1,"currency":"USD","balance":{"money":null,
                "credits":{"amount_minor":minor,"exponent":2}},"auto_reload_settings":null})
        };
        assert_eq!(
            credits(&extra, held(0)),
            Some((Some("$0.00".into()), false, false))
        );
        assert_eq!(
            credits(&extra, held(9000)),
            Some((Some("$27.15".into()), false, true))
        );
        assert_eq!(
            credits(&extra, held(1000)),
            Some((Some("$10.00".into()), false, true))
        );
        let money = json!({"amount":9000,"currency":"USD","balance":{"money":
            {"amount_minor":9000,"currency":"USD","exponent":2},"credits":null}});
        let uncapped = json!({"is_enabled":true,"used_credits":1285,"currency":"EUR"});
        assert_eq!(
            credits(&uncapped, money),
            Some((Some("€90.00".into()), false, true))
        );
        // Auto-reload leaves only the limit, and with neither it is unlimited.
        let reloaded = json!({"amount":200,"auto_reload_settings":{"enabled":true}});
        assert_eq!(
            credits(&extra, reloaded.clone()),
            Some((Some("$27.15".into()), false, true))
        );
        assert_eq!(credits(&uncapped, reloaded), Some((None, true, true)));
        // A bare amount is minor units; without a balance nothing is known.
        assert_eq!(
            credits(&extra, json!({"amount":500})),
            Some((Some("$5.00".into()), false, true))
        );
        assert_eq!(credits(&extra, json!({"amount":-1})), None);
        assert_eq!(credits(&extra, Value::Null), None);
        assert_eq!(credits(&Value::Null, held(9000)), None);
    }

    #[tokio::test]
    async fn extra_usage_credits_ask_for_the_balance_through_the_account_proxy() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_accounts().await.unwrap()[0].id.clone();
        let usage = json!({"five_hour":{"utilization":5},"extra_usage":{"is_enabled":true,
            "monthly_limit":4000,"used_credits":1285,"currency":"USD","decimal_places":2}});
        *fake.prepaid.lock().unwrap() = Some(json!({"amount":-1,"balance":{"money":null,
            "credits":{"amount_minor":1500,"exponent":2}}}));
        fake.usage.lock().unwrap().push((200, usage.clone()));
        app.claude_refresh_usage(&id).await.unwrap();
        let credits = |app: &Application| {
            app.claude_load().unwrap().accounts[0]
                .usage
                .as_ref()
                .unwrap()
                .credits
                .as_ref()
                .and_then(|c| c.balance.clone())
        };
        assert_eq!(credits(&app), Some("$15.00".into()));
        let requests = fake.prepaid_requests.lock().unwrap().clone();
        let proxy = fake.usage_proxies.lock().unwrap().last().cloned().unwrap();
        assert_eq!(requests, vec![("org-alice".to_string(), proxy)]);
        // A failed balance is unknown, and without extra usage it is not asked for.
        *fake.prepaid.lock().unwrap() = None;
        fake.usage.lock().unwrap().push((200, usage));
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(credits(&app), None);
        fake.usage
            .lock()
            .unwrap()
            .push((200, json!({"five_hour":{"utilization":5}})));
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(fake.prepaid_requests.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn usage_rate_limits_pause_manual_and_background_refresh_until_backoff_expires() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_accounts().await.unwrap()[0].id.clone();
        fake.usage
            .lock()
            .unwrap()
            .push((429, json!({"error":{"message":"Rate limited"}})));
        let error = app.claude_refresh_usage(&id).await.unwrap_err();
        assert!(error.contains("HTTP 429"));
        let until = app.claude_load().unwrap().accounts[0]
            .usage_retry_at
            .unwrap();
        assert!(until > now_unix_seconds());
        fake.usage
            .lock()
            .unwrap()
            .push((200, json!({"five_hour":{"utilization":42}})));
        assert_eq!(app.claude_refresh_usage(&id).await.unwrap_err(), error);
        assert!(app.claude_refresh_recurring(0).await.is_err());
        assert_eq!(fake.usage.lock().unwrap().len(), 1);
        let mut store = app.claude_load().unwrap();
        store.accounts[0].usage_retry_at = Some(now_unix_seconds() - 1);
        app.claude_save(&store).unwrap();
        app.claude_refresh_usage(&id).await.unwrap();
        let stored = app.claude_load().unwrap();
        assert!(stored.accounts[0].usage_retry_at.is_none());
        assert!(stored.accounts[0].usage_error.is_none());
        assert!(fake.usage.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn token_rate_limits_pause_all_refreshes_and_recover_after_backoff() {
        for expires in [1, LATER] {
            let dir = tempfile::tempdir().unwrap();
            let (app, location, fake) = waiting_app(&dir);
            sign_in(&location, "alice", expires);
            app.claude_import_current().await.unwrap();
            let id = app.claude_load().unwrap().accounts[0].id.clone();
            app.claude_release_new_accounts(Some(&id)).await.unwrap();
            let credentials = fs::read(location.credentials_file()).unwrap();
            if expires == LATER {
                fake.usage.lock().unwrap().push((401, Value::Null));
            }
            *fake.refresh_error.lock().unwrap() = Some(TOKEN_REFRESH_RATE_LIMITED.into());
            let before = now_unix_seconds();
            assert_eq!(
                app.claude_refresh_usage(&id).await.unwrap_err(),
                TOKEN_REFRESH_RATE_LIMITED
            );
            let stored = app.claude_load().unwrap();
            let retry_at = stored.accounts[0].usage_retry_at.unwrap();
            assert!((before + 300..=now_unix_seconds() + 300).contains(&retry_at));
            assert_eq!(stored.accounts[0].oauth["refreshToken"], "alice-refresh");
            assert_eq!(fs::read(location.credentials_file()).unwrap(), credentials);
            let usage_calls = fake.usage_tokens.lock().unwrap().len();
            assert_eq!(fake.refreshed.lock().unwrap().len(), 1);
            // A persisted cooldown survives reopening the application.
            drop(app);
            let (app, _, _) = waiting_app(&dir);
            app.set_claude_network(fake.clone());
            assert_eq!(
                app.claude_refresh_usage(&id).await.unwrap_err(),
                TOKEN_REFRESH_RATE_LIMITED
            );
            assert!(app.claude_refresh(None).await.is_err());
            assert!(app.claude_refresh_recurring(0).await.is_err());
            assert_eq!(fake.refreshed.lock().unwrap().len(), 1);
            assert_eq!(fake.usage_tokens.lock().unwrap().len(), usage_calls);
            let mut store = app.claude_load().unwrap();
            store.accounts[0].usage_retry_at = Some(now_unix_seconds() - 1);
            app.claude_save(&store).unwrap();
            if expires == LATER {
                fake.usage.lock().unwrap().push((401, Value::Null));
            }
            app.claude_refresh_usage(&id).await.unwrap();
            assert_eq!(fake.refreshed.lock().unwrap().len(), 2);
            let stored = app.claude_load().unwrap();
            assert!(stored.accounts[0].usage_retry_at.is_none());
            assert!(stored.accounts[0].usage_error.is_none());
            assert_eq!(stored.accounts[0].oauth["refreshToken"], "fresh-refresh");
        }
    }

    #[tokio::test]
    async fn other_token_failures_are_not_classified_as_rate_limits() {
        for error in [SIGN_IN_EXPIRED, "Claude token refresh failed: HTTP 500"] {
            let dir = tempfile::tempdir().unwrap();
            let (app, location, fake) = waiting_app(&dir);
            sign_in(&location, "alice", 1);
            app.claude_import_current().await.unwrap();
            let id = app.claude_load().unwrap().accounts[0].id.clone();
            app.claude_release_new_accounts(Some(&id)).await.unwrap();
            *fake.refresh_error.lock().unwrap() = Some(error.into());
            assert_eq!(app.claude_refresh_usage(&id).await.unwrap_err(), error);
            assert!(app.claude_load().unwrap().accounts[0]
                .usage_retry_at
                .is_none());
        }
    }

    #[tokio::test]
    async fn request_errors_preserve_the_cause_without_auth_headers() {
        // A truncated response produces a real transport error deterministically.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")
                .await
                .unwrap();
        });
        let error = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{address}/"))
            .bearer_auth("synthetic-secret")
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap_err();
        let top_level = error.to_string();
        let message = request_error(error);
        assert!(message.starts_with(&top_level));
        assert!(message.contains(" -> "), "{message}");
        assert!(!message.contains("synthetic-secret"));
        server.await.unwrap();
    }

    #[derive(Default)]
    pub(super) struct Fake {
        usage: Mutex<Vec<(u16, Value)>>,
        refreshed: Mutex<Vec<String>>,
        profile_proxies: Mutex<Vec<String>>,
        usage_tokens: Mutex<Vec<String>>,
        usage_proxies: Mutex<Vec<String>>,
        usage_hooks: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
        refresh_hooks: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
        refresh_error: Mutex<Option<String>>,
        prepaid: Mutex<Option<Value>>,
        prepaid_requests: Mutex<Vec<(String, String)>>,
    }
    impl Network for Fake {
        fn usage<'a>(&'a self, token: &'a str, proxy: &'a str) -> Response<'a, (u16, Value)> {
            Box::pin(async move {
                self.usage_tokens.lock().unwrap().push(token.into());
                self.usage_proxies.lock().unwrap().push(proxy.into());
                let mut hooks = self.usage_hooks.lock().unwrap();
                if !hooks.is_empty() {
                    hooks.remove(0)();
                }
                drop(hooks);
                let mut queued = self.usage.lock().unwrap();
                Ok(if queued.is_empty() {
                    (
                        200,
                        json!({"five_hour":{"utilization":25,"resets_at":"2099-01-01T00:00:00Z"},"seven_day":{"utilization":"40.5","resets_at":null}}),
                    )
                } else {
                    queued.remove(0)
                })
            })
        }
        fn refresh<'a>(&'a self, token: &'a str, _: &'a str) -> Response<'a, Value> {
            Box::pin(async move {
                self.refreshed.lock().unwrap().push(token.into());
                let mut hooks = self.refresh_hooks.lock().unwrap();
                if !hooks.is_empty() {
                    hooks.remove(0)();
                }
                drop(hooks);
                if let Some(error) = self.refresh_error.lock().unwrap().take() {
                    return Err(error);
                }
                Ok(
                    json!({"access_token":"fresh-access","refresh_token":"fresh-refresh","expires_in":3600}),
                )
            })
        }
        fn profile<'a>(&'a self, _: &'a str, proxy: &'a str) -> Response<'a, Value> {
            Box::pin(async move {
                self.profile_proxies.lock().unwrap().push(proxy.into());
                Ok(
                    json!({"account":{"uuid":"acct-new","email":"new@example.com"},"organization":{"uuid":"org-new","name":"New","organization_type":"claude_max","rate_limit_tier":"default_claude_max_5x"}}),
                )
            })
        }
        fn prepaid<'a>(&'a self, _: &'a str, org: &'a str, proxy: &'a str) -> Response<'a, Value> {
            Box::pin(async move {
                self.prepaid_requests
                    .lock()
                    .unwrap()
                    .push((org.into(), proxy.into()));
                self.prepaid
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or_else(|| "HTTP 403".to_string())
            })
        }
    }
    fn login(user: &str, expires: i64) -> (Value, Value) {
        (
            json!({"accessToken": format!("{user}-access"), "refreshToken": format!("{user}-refresh"), "expiresAt": expires, "subscriptionType": "max", "rateLimitTier": "default_claude_max_20x"}),
            json!({"accountUuid": format!("acct-{user}"), "organizationUuid": format!("org-{user}"), "emailAddress": format!("{user}@example.com")}),
        )
    }
    #[tokio::test]
    async fn startup_authorization_follows_the_live_account_without_refreshing() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        sign_in(&location, "bob", LATER);
        app.claude_import_current().await.unwrap();
        sign_in(&location, "alice", LATER);

        app.claude_authorize_at_startup().await.unwrap();

        let store = app.claude_load().unwrap();
        let current = store
            .accounts
            .iter()
            .find(|a| Some(&a.id) == store.current_account_id.as_ref())
            .unwrap();
        assert_eq!(current.label, "alice@example.com");
        assert!(store.accounts.iter().all(|a| a.usage.is_none()));
        assert!(fake.usage.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn startup_authorization_skips_empty_pools_and_preserves_them_on_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, _) = waiting_app(&dir);
        fs::create_dir_all(&location.config_dir).unwrap();
        fs::write(location.credentials_file(), "invalid credentials").unwrap();
        // An empty pool must not inspect even an existing credentials file.
        app.claude_authorize_at_startup().await.unwrap();

        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let before = fs::read(app.claude_store_path()).unwrap();
        fs::write(location.credentials_file(), "invalid credentials").unwrap();
        assert!(app.claude_authorize_at_startup().await.is_err());
        assert_eq!(fs::read(app.claude_store_path()).unwrap(), before);
    }
    pub(super) fn sign_in(location: &Location, user: &str, expires: i64) {
        let (oauth, account) = login(user, expires);
        fs::create_dir_all(&location.config_dir).unwrap();
        write_json(
            &location.credentials_file(),
            &json!({"claudeAiOauth": oauth, "mcpOAuth": {"kept": true}}),
        )
        .unwrap();
        write_account(location, &account).unwrap();
    }
    /// An isolated pool that contacts upstream right after an import.
    async fn app(dir: &tempfile::TempDir) -> (Application, Location, Arc<Fake>) {
        let (app, location, fake) = waiting_app(dir);
        let preferences = Preferences {
            request_new_accounts: true,
            ..app.preferences().unwrap()
        };
        app.save_preferences(preferences).await.unwrap();
        (app, location, fake)
    }
    /// An isolated pool with the default: new accounts wait for a refresh.
    fn waiting_app(dir: &tempfile::TempDir) -> (Application, Location, Arc<Fake>) {
        let app = Application::open(crate::Paths {
            data_dir: dir.path().join("data"),
            codex_auth: dir.path().join("codex/auth.json"),
        })
        .unwrap();
        let location = Location::files(dir.path().join("claude"));
        app.set_claude_location(location.clone());
        let fake = Arc::new(Fake::default());
        app.set_claude_network(fake.clone());
        (app, location, fake)
    }
    fn live_access(location: &Location) -> String {
        let document: Value =
            serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
        document["claudeAiOauth"]["accessToken"]
            .as_str()
            .unwrap()
            .into()
    }
    const LATER: i64 = 4_000_000_000_000;

    #[tokio::test]
    async fn json_import_is_offline_deduplicated_and_only_switch_writes_live_login() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let credentials = fs::read(location.credentials_file()).unwrap();
        let config = fs::read(location.account_file()).unwrap();
        let (oauth, _) = login("bob", LATER);
        let document = json!({"claudeAiOauth": oauth, "mcpOAuth": {"foreign": true}});
        assert_eq!(
            app.claude_import_json(&document.to_string()).await.unwrap(),
            1
        );
        let imported = app.claude_load().unwrap();
        let bob = imported.accounts.iter().find(|a| a.oauth == oauth).unwrap();
        let bob_id = bob.id.clone();
        assert!(bob.first_request_pending);
        assert_ne!(
            imported.current_account_id.as_deref(),
            Some(bob_id.as_str())
        );
        assert!(fake.profile_proxies.lock().unwrap().is_empty());
        assert!(fake.refreshed.lock().unwrap().is_empty());
        // A bare token object and a BOM-prefixed export update the same entry.
        app.claude_import_json(&oauth.to_string()).await.unwrap();
        app.claude_import_json(&format!("\u{feff}{document}"))
            .await
            .unwrap();
        let repeated = app.claude_load().unwrap();
        assert_eq!(repeated.accounts.len(), 2);
        assert!(repeated.accounts.iter().any(|a| a.id == bob_id));
        assert_eq!(fs::read(location.credentials_file()).unwrap(), credentials);
        assert_eq!(fs::read(location.account_file()).unwrap(), config);
        app.claude_switch_account(&bob_id).await.unwrap();
        assert_eq!(live_access(&location), "bob-access");
        let live = read_live(&location).await.unwrap().unwrap();
        assert_eq!(live.document["mcpOAuth"], json!({"kept": true}));
        assert!(live.account.is_none());
    }

    #[tokio::test]
    async fn json_import_rejects_invalid_documents_without_changing_the_pool() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, _) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let saved = fs::read(app.claude_store_path()).unwrap();
        let live = fs::read(location.credentials_file()).unwrap();
        for invalid in [
            "not JSON",
            "null",
            "[]",
            "{}",
            "42",
            r#"{"claudeAiOauth":null}"#,
            r#"{"claudeAiOauth":{"accessToken":" "}}"#,
            r#"{"accessToken":123}"#,
            r#"{"accessToken":"test","refreshToken":42}"#,
            r#"{"accessToken":"test","expiresAt":"tomorrow"}"#,
            r#"{"accessToken":"test","scopes":[42]}"#,
            r#"{"tokens":{"access_token":"codex-token"}}"#,
        ] {
            assert_eq!(
                app.claude_import_json(invalid).await.unwrap_err(),
                "error.claude.credentials_json_invalid"
            );
            assert_eq!(fs::read(app.claude_store_path()).unwrap(), saved);
            assert_eq!(fs::read(location.credentials_file()).unwrap(), live);
        }
        assert!(app
            .claude_import_json(&" ".repeat(16 * 1024 * 1024 + 1))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn json_import_keeps_supplied_identity_or_resolves_it_when_requests_are_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        let (oauth, account) = login("bob", LATER);
        app.claude_import_json(
            &json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string(),
        )
        .await
        .unwrap();
        let store = app.claude_load().unwrap();
        assert_eq!(store.accounts[0].account, account);
        assert!(store.current_account_id.is_none());
        assert!(!location.credentials_file().exists());
        assert!(!location.account_file().exists());
        let (other, _) = login("carol", LATER);
        app.claude_import_json(&json!({"claudeAiOauth": other}).to_string())
            .await
            .unwrap();
        let store = app.claude_load().unwrap();
        assert_eq!(store.accounts.len(), 2);
        assert!(store
            .accounts
            .iter()
            .any(|a| a.account["emailAddress"] == "new@example.com"));
        assert!(!fake.profile_proxies.lock().unwrap().is_empty());
        assert!(!location.credentials_file().exists());
    }

    #[tokio::test]
    async fn imports_switches_and_follows_the_claude_code_login() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, _) = app(&dir).await;
        fs::create_dir_all(&location.config_dir).unwrap();
        fs::write(
            &location.global_config,
            "{\n  \"zeta\": 1,\n  \"oauthAccount\": null,\n  \"alpha\": [1, {\"a\": \"}\"}]\n}\n",
        )
        .unwrap();
        sign_in(&location, "alice", LATER);
        assert_eq!(
            app.claude_import_current().await.unwrap(),
            "alice@example.com"
        );
        sign_in(&location, "bob", LATER);
        app.claude_import_current().await.unwrap();
        let accounts = app.claude_accounts().await.unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts.iter().filter(|a| a.is_current).count(), 1);
        let bob = accounts.iter().find(|a| a.is_current).unwrap();
        assert_eq!(bob.email.as_deref(), Some("bob@example.com"));
        assert_eq!(bob.plan_type.as_deref(), Some("max 20x"));
        let usage = bob.usage.as_ref().unwrap();
        assert_eq!(usage.five_hour.as_ref().unwrap().used_percent, 25.0);
        assert_eq!(usage.one_week.as_ref().unwrap().used_percent, 40.5);
        assert!(!serde_json::to_string(&accounts).unwrap().contains("access"));

        let alice = accounts.iter().find(|a| !a.is_current).unwrap().id.clone();
        app.claude_switch_account(&alice).await.unwrap();
        assert_eq!(live_access(&location), "alice-access");
        let document: Value =
            serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
        assert_eq!(document["mcpOAuth"]["kept"], true);
        let config = fs::read_to_string(&location.global_config).unwrap();
        assert!(config.starts_with("{\n  \"zeta\": 1,\n  \"oauthAccount\": {"));
        assert!(config.contains("\"alpha\": [1, {\"a\": \"}\"}]"));
        let config: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(config["oauthAccount"]["accountUuid"], "acct-alice");
        // Bob is in the pool: only a login outside it is backed up.
        let backup = app.paths.data_dir.join("claude-before-switch.json");
        assert!(!backup.exists());
        let current = app.claude_accounts().await.unwrap();
        assert_eq!(current.iter().find(|a| a.is_current).unwrap().id, alice);

        // A login made in Claude Code is followed, with the tokens it refreshed.
        let (mut oauth, account) = login("bob", LATER + 1);
        oauth["accessToken"] = "bob-newer".into();
        write_json(
            &location.credentials_file(),
            &json!({"claudeAiOauth": oauth}),
        )
        .unwrap();
        write_account(&location, &account).unwrap();
        let current = app.claude_accounts().await.unwrap();
        assert_eq!(current.iter().find(|a| a.is_current).unwrap().id, bob.id);
        let stored = app.claude_load().unwrap();
        let bob_stored = stored.accounts.iter().find(|a| a.id == bob.id).unwrap();
        assert_eq!(bob_stored.oauth["accessToken"], "bob-newer");

        // A login outside the pool is backed up, and stays so across switches.
        sign_in(&location, "carol", LATER);
        app.claude_switch_account(&alice).await.unwrap();
        let read_backup = || serde_json::from_slice::<Value>(&fs::read(&backup).unwrap()).unwrap();
        let backed_up = read_backup();
        assert_eq!(
            backed_up["credentials"]["claudeAiOauth"]["accessToken"],
            "carol-access"
        );
        app.claude_switch_account(&bob.id).await.unwrap();
        assert_eq!(read_backup(), backed_up);
    }

    #[tokio::test]
    async fn a_failed_switch_leaves_no_login_where_there_was_none() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, _) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        fs::remove_file(location.credentials_file()).unwrap();
        // The identity cannot be written after the credentials are.
        fs::remove_file(location.account_file()).unwrap();
        fs::create_dir(location.account_file()).unwrap();
        assert!(app.claude_switch_account(&id).await.is_err());
        assert!(!location.credentials_file().exists());
    }

    #[tokio::test]
    async fn a_live_login_renews_and_syncs_without_changing_other_fields() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", 1);
        let before: Value =
            serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
        let config_before = fs::read(location.account_file()).unwrap();
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        let (a, b) = tokio::join!(app.claude_refresh_usage(&id), app.claude_refresh_usage(&id));
        a.unwrap();
        b.unwrap();
        assert_eq!(*fake.refreshed.lock().unwrap(), vec!["alice-refresh"]);
        assert_eq!(
            app.claude_load().unwrap().accounts[0].oauth["accessToken"],
            "fresh-access"
        );
        // The active login and pool share the refreshed generation.
        app.claude_accounts().await.unwrap();
        assert_eq!(
            app.claude_load().unwrap().accounts[0].oauth["refreshToken"],
            "fresh-refresh"
        );
        fake.usage.lock().unwrap().push((401, Value::Null));
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(
            *fake.refreshed.lock().unwrap(),
            vec!["alice-refresh", "fresh-refresh"]
        );
        let after: Value =
            serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
        assert_eq!(
            after["claudeAiOauth"],
            app.claude_load().unwrap().accounts[0].oauth
        );
        assert_eq!(after["mcpOAuth"], before["mcpOAuth"]);
        assert_eq!(fs::read(location.account_file()).unwrap(), config_before);
    }

    #[tokio::test]
    async fn live_rotation_is_preserved_even_when_usage_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", 1);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        fake.usage.lock().unwrap().push((500, Value::Null));
        assert!(app
            .claude_refresh_usage(&id)
            .await
            .unwrap_err()
            .contains("HTTP 500"));
        assert_eq!(live_access(&location), "fresh-access");
        assert_eq!(
            app.claude_load().unwrap().accounts[0].oauth["refreshToken"],
            "fresh-refresh"
        );
    }

    #[tokio::test]
    async fn an_unreadable_login_keeps_the_account_in_use_from_renewing() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", 1);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        fs::write(location.credentials_file(), "{").unwrap();
        assert_eq!(
            app.claude_refresh_usage(&id).await.unwrap_err(),
            LIVE_UNREADABLE
        );
        assert!(fake.refreshed.lock().unwrap().is_empty());
        assert_eq!(
            app.claude_load().unwrap().accounts[0].oauth["refreshToken"],
            "alice-refresh"
        );
    }

    #[tokio::test]
    async fn an_inactive_login_renews_once_without_writing_live_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", 1);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        sign_in(&location, "bob", LATER);
        let before = fs::read(location.credentials_file()).unwrap();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        let (a, b) = tokio::join!(app.claude_refresh_usage(&id), app.claude_refresh_usage(&id));
        a.unwrap();
        b.unwrap();
        assert_eq!(*fake.refreshed.lock().unwrap(), vec!["alice-refresh"]);
        assert_eq!(fs::read(location.credentials_file()).unwrap(), before);
        assert_eq!(
            app.claude_load().unwrap().accounts[0].oauth["refreshToken"],
            "fresh-refresh"
        );
        fake.usage
            .lock()
            .unwrap()
            .extend([(401, Value::Null), (401, Value::Null)]);
        assert_eq!(
            app.claude_refresh_usage(&id).await.unwrap_err(),
            SIGN_IN_EXPIRED
        );
    }

    #[tokio::test]
    async fn unreadable_live_credentials_do_not_block_pool_renewal() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        // Only the account Claude Code uses waits for its login to be readable.
        sign_in(&location, "bob", LATER);
        app.claude_import_current().await.unwrap();
        fs::write(location.credentials_file(), "invalid").unwrap();
        app.claude_refresh_usage(&id).await.unwrap();
        fake.usage.lock().unwrap().push((401, Value::Null));
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(*fake.refreshed.lock().unwrap(), vec!["alice-refresh"]);
        assert_eq!(
            app.claude_load().unwrap().accounts[0].oauth["refreshToken"],
            "fresh-refresh"
        );
        assert_eq!(
            fs::read_to_string(location.credentials_file()).unwrap(),
            "invalid"
        );
    }

    #[tokio::test]
    async fn a_card_refresh_follows_rotated_tokens_without_renewing_them() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let (mut oauth, _) = login("alice", LATER + 1);
        oauth["refreshToken"] = "rotated-refresh".into();
        write_json(
            &location.credentials_file(),
            &json!({"claudeAiOauth": oauth}),
        )
        .unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(app.claude_load().unwrap().accounts[0].oauth, oauth);
        assert!(fake.refreshed.lock().unwrap().is_empty());
    }

    fn external_update(location: Location, user: &str, access: &str) -> Box<dyn FnOnce() + Send> {
        let user = user.to_owned();
        let access = access.to_owned();
        Box::new(move || {
            let (mut oauth, account) = login(&user, LATER);
            oauth["accessToken"] = access.clone().into();
            oauth["refreshToken"] = format!("{access}-refresh").into();
            write_json(
                &location.credentials_file(),
                &json!({"claudeAiOauth": oauth}),
            )
            .unwrap();
            write_account(&location, &account).unwrap();
        })
    }

    #[tokio::test]
    async fn external_rotation_wins_over_refresh_success_or_failure() {
        for failed in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let (app, location, fake) = waiting_app(&dir);
            sign_in(&location, "alice", 1);
            app.claude_import_current().await.unwrap();
            let id = app.claude_load().unwrap().accounts[0].id.clone();
            app.claude_release_new_accounts(Some(&id)).await.unwrap();
            fake.refresh_hooks.lock().unwrap().push(external_update(
                location.clone(),
                "alice",
                "external",
            ));
            if failed {
                *fake.refresh_error.lock().unwrap() = Some(SIGN_IN_EXPIRED.into());
            }
            app.claude_refresh_usage(&id).await.unwrap();
            let stored = app.claude_load().unwrap();
            assert_eq!(stored.accounts[0].oauth["accessToken"], "external");
            assert_eq!(stored.accounts[0].oauth["refreshToken"], "external-refresh");
            assert!(stored.accounts[0].usage_error.is_none());
            assert_eq!(*fake.refreshed.lock().unwrap(), vec!["alice-refresh"]);
            assert_eq!(
                fake.usage_tokens.lock().unwrap().last().unwrap(),
                "external"
            );
            assert_eq!(live_access(&location), "external");
        }
    }

    #[tokio::test]
    async fn external_rotation_during_401_avoids_a_redundant_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        fake.usage_hooks.lock().unwrap().push(external_update(
            location.clone(),
            "alice",
            "external",
        ));
        fake.usage.lock().unwrap().push((401, Value::Null));
        app.claude_refresh_usage(&id).await.unwrap();
        assert!(fake.refreshed.lock().unwrap().is_empty());
        assert_eq!(
            *fake.usage_tokens.lock().unwrap(),
            vec!["alice-access", "external"]
        );
        assert_eq!(
            app.claude_load().unwrap().accounts[0].oauth["accessToken"],
            "external"
        );
    }

    #[tokio::test]
    async fn repeated_external_changes_are_saved_but_retried_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        fake.usage_hooks.lock().unwrap().extend([
            external_update(location.clone(), "alice", "first"),
            external_update(location.clone(), "alice", "second"),
        ]);
        assert_eq!(
            app.claude_refresh_usage(&id).await.unwrap_err(),
            CREDENTIALS_CHANGED
        );
        assert_eq!(
            *fake.usage_tokens.lock().unwrap(),
            vec!["alice-access", "first"]
        );
        assert_eq!(
            app.claude_load().unwrap().accounts[0].oauth["accessToken"],
            "second"
        );
        assert_eq!(live_access(&location), "second");
    }

    #[tokio::test]
    async fn external_account_switch_does_not_replace_the_refreshed_account() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", 1);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        fake.refresh_hooks.lock().unwrap().push(external_update(
            location.clone(),
            "bob",
            "bob-new",
        ));
        app.claude_refresh_usage(&id).await.unwrap();
        let stored = app.claude_load().unwrap();
        assert_eq!(stored.accounts[0].oauth["accessToken"], "fresh-access");
        assert_eq!(stored.accounts[0].account["accountUuid"], "acct-alice");
        assert_eq!(live_access(&location), "bob-new");
    }

    #[test]
    fn generation_comparison_includes_expiry_but_ignores_metadata() {
        let (oauth, _) = login("alice", LATER);
        let mut changed = oauth.clone();
        changed["subscriptionType"] = "pro".into();
        assert!(same_oauth_generation(&oauth, &changed));
        changed["expiresAt"] = (LATER + 1).into();
        assert!(!same_oauth_generation(&oauth, &changed));
    }

    #[tokio::test]
    async fn a_browser_sign_in_is_stored_without_changing_claude_code() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, _) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        let label = app
            .claude_import_login(json!({"access_token":"new-access","refresh_token":"new-refresh","expires_in":28800,"scope":"user:inference user:profile"}))
            .await
            .unwrap();
        assert_eq!(label, "new@example.com");
        assert_eq!(live_access(&location), "alice-access");
        let accounts = app.claude_accounts().await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert!(!accounts[0].is_current);
        assert_eq!(accounts[0].plan_type.as_deref(), Some("max 5x"));
        // The only stored account is not Claude Code's, so Smart Switch takes it.
        assert_eq!(
            app.claude_smart_switch().await.unwrap(),
            SmartSwitchOutcome::Switched {
                label: "new@example.com".into(),
                execution: SwitchExecution::default(),
            }
        );
        assert_eq!(live_access(&location), "new-access");
    }

    #[tokio::test]
    async fn account_proxy_routes_refresh_but_not_import() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _location, fake) = app(&dir).await;
        let mut preferences = app.preferences().unwrap();
        preferences.account_proxy_url = "http://127.0.0.1:7890".into();
        app.save_preferences(preferences).await.unwrap();
        app.claude_import_login(json!({
            "access_token": "new-access", "refresh_token": "new-refresh",
            "expires_in": 3600,
            "account": {"uuid": "acct-new"}, "organization": {"uuid": "org-new"}
        }))
        .await
        .unwrap();
        assert_eq!(*fake.profile_proxies.lock().unwrap(), vec![""]);
        assert_eq!(*fake.usage_proxies.lock().unwrap(), vec![""]);
        let account = app.claude_load().unwrap().accounts.remove(0);
        assert!(account.proxy_url.is_empty());
        app.claude_refresh_usage(&account.id).await.unwrap();
        assert_eq!(
            fake.usage_proxies.lock().unwrap().last().unwrap(),
            "http://127.0.0.1:7890"
        );
        app.claude_save_account_proxy(&account.id, "socks5://127.0.0.1:7891")
            .await
            .unwrap();
        app.claude_refresh_usage(&account.id).await.unwrap();
        assert_eq!(
            fake.usage_proxies.lock().unwrap().last().unwrap(),
            "socks5://127.0.0.1:7891"
        );
        let mut preferences = app.preferences().unwrap();
        preferences.account_proxy_url.clear();
        app.save_preferences(preferences).await.unwrap();
        app.claude_refresh_usage(&account.id).await.unwrap();
        assert_eq!(
            fake.usage_proxies.lock().unwrap().last().unwrap(),
            "socks5://127.0.0.1:7891"
        );
    }

    #[tokio::test]
    async fn browser_profile_uses_the_existing_account_proxy_with_rotated_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        // The currently selected account must not supply the new login's proxy.
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let alice = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_save_account_proxy(&alice, "http://127.0.0.1:8001")
            .await
            .unwrap();
        let (oauth, account) = login("new", LATER);
        let (id, _, _) = app
            .claude_upsert(oauth, account, false, false)
            .await
            .unwrap();
        app.claude_save_account_proxy(&id, "socks5://127.0.0.1:8002")
            .await
            .unwrap();
        app.claude_import_login(json!({
            "access_token": "rotated-access", "refresh_token": "rotated-refresh",
            "expires_in": 3600,
            "account": {"uuid": "acct-new"}, "organization": {"uuid": "org-new"}
        }))
        .await
        .unwrap();
        assert_eq!(
            *fake.profile_proxies.lock().unwrap(),
            vec!["socks5://127.0.0.1:8002"]
        );
        assert_eq!(app.claude_load().unwrap().accounts.len(), 2);
    }

    #[tokio::test]
    /// A login without oauthAccount whose tokens match a pool account is
    /// that account: no profile request is needed to name it.
    async fn importing_without_local_identity_reuses_the_account_its_tokens_match() {
        for retained in ["accessToken", "refreshToken"] {
            let dir = tempfile::tempdir().unwrap();
            let (app, location, fake) = app(&dir).await;
            sign_in(&location, "new", LATER);
            app.claude_import_current().await.unwrap();
            let id = app.claude_load().unwrap().accounts[0].id.clone();
            app.claude_save_account_proxy(&id, "http://127.0.0.1:8002")
                .await
                .unwrap();
            let (mut oauth, _) = login("new", LATER);
            let changed = if retained == "accessToken" {
                "refreshToken"
            } else {
                "accessToken"
            };
            oauth[changed] = "rotated".into();
            write_json(
                &location.credentials_file(),
                &json!({"claudeAiOauth": oauth}),
            )
            .unwrap();
            write_json(&location.global_config, &json!({})).unwrap();
            app.claude_import_current().await.unwrap();
            assert!(fake.profile_proxies.lock().unwrap().is_empty());
            let accounts = app.claude_load().unwrap().accounts;
            assert_eq!(accounts.len(), 1);
            assert_eq!(accounts[0].account["accountUuid"], "acct-new");
            assert_eq!(accounts[0].proxy_url, "http://127.0.0.1:8002");
        }
    }

    #[tokio::test]
    async fn an_unknown_profile_does_not_use_another_accounts_proxy() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_save_account_proxy(&id, "http://127.0.0.1:8001")
            .await
            .unwrap();
        app.claude_import_login(json!({
            "access_token": "new-access", "refresh_token": "new-refresh", "expires_in": 3600
        }))
        .await
        .unwrap();
        assert_eq!(*fake.profile_proxies.lock().unwrap(), vec![""]);
    }

    /// A Claude Code login whose configuration does not name its account.
    fn sign_in_unnamed(location: &Location, user: &str) {
        let (oauth, _) = login(user, LATER);
        fs::create_dir_all(&location.config_dir).unwrap();
        write_json(
            &location.credentials_file(),
            &json!({"claudeAiOauth": oauth}),
        )
        .unwrap();
        let _ = fs::remove_file(&location.global_config);
    }

    #[tokio::test]
    async fn cards_carry_the_access_token_expiry_in_seconds() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, _) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let accounts = app.claude_accounts().await.unwrap();
        assert_eq!(accounts[0].token_expires_at, Some(LATER / 1000));
    }

    #[tokio::test]
    async fn an_unnamed_login_is_imported_and_probed_on_its_first_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in_unnamed(&location, "probe");
        assert_eq!(app.claude_import_current().await.unwrap(), "Claude Code");
        app.claude_import_current().await.unwrap();
        let accounts = app.claude_accounts().await.unwrap();
        assert_eq!(accounts.len(), 1);
        let card = &accounts[0];
        assert!(card.is_current && card.awaiting_first_request && card.email.is_none());
        assert!(fake.profile_proxies.lock().unwrap().is_empty());

        app.claude_save_account_proxy(&card.id, "http://127.0.0.1:8003")
            .await
            .unwrap();
        app.claude_release_new_accounts(Some(&card.id))
            .await
            .unwrap();
        app.claude_refresh_usage(&card.id).await.unwrap();
        assert_eq!(
            *fake.profile_proxies.lock().unwrap(),
            vec!["http://127.0.0.1:8003"]
        );
        let accounts = app.claude_accounts().await.unwrap();
        assert_eq!(accounts.len(), 1);
        let card = &accounts[0];
        assert_eq!(card.label, "new@example.com");
        assert_eq!(card.plan_type.as_deref(), Some("max 5x"));
        assert!(card.is_current && card.usage.is_some());
    }

    #[tokio::test]
    async fn a_probed_login_folds_into_the_pool_card_of_its_account() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        app.claude_import_login(json!({
            "access_token": "browser-access", "refresh_token": "browser-refresh",
            "expires_in": 3600,
            "account": {"uuid": "acct-new", "email_address": "new@example.com"},
            "organization": {"uuid": "org-new"}
        }))
        .await
        .unwrap();
        sign_in_unnamed(&location, "probe");
        app.claude_import_current().await.unwrap();
        assert_eq!(app.claude_accounts().await.unwrap().len(), 2);
        app.claude_release_new_accounts(None).await.unwrap();
        app.claude_refresh(None).await.unwrap();
        let accounts = app.claude_accounts().await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].label, "new@example.com");
        assert!(accounts[0].is_current);
        // Switching to it later names the account in Claude Code again.
        assert_eq!(
            app.claude_load().unwrap().accounts[0].oauth["accessToken"],
            "probe-access"
        );
        let _ = fake;
    }

    #[tokio::test]
    async fn a_new_account_contacts_upstream_only_after_a_manual_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", 1);
        app.claude_import_current().await.unwrap();
        app.claude_import_login(json!({
            "access_token": "new-access", "refresh_token": "new-refresh", "expires_in": 3600,
            "account": {"uuid": "acct-new", "email_address": "new@example.com"},
            "organization": {"uuid": "org-new"}
        }))
        .await
        .unwrap();
        // Neither import, nor the background refresh, reached upstream.
        app.claude_refresh_recurring(0).await.unwrap();
        let accounts = app.claude_accounts().await.unwrap();
        assert_eq!(accounts.len(), 2);
        assert!(accounts
            .iter()
            .all(|a| a.awaiting_first_request && a.usage.is_none()));
        assert!(fake.refreshed.lock().unwrap().is_empty());
        assert!(fake.profile_proxies.lock().unwrap().is_empty());

        // A manual refresh releases one account, through its own proxy, and
        // learns the plan its profile was not asked for at sign-in.
        let new = accounts
            .iter()
            .find(|a| a.label == "new@example.com")
            .unwrap();
        app.claude_save_account_proxy(&new.id, "socks5://127.0.0.1:8002")
            .await
            .unwrap();
        app.claude_release_new_accounts(Some(&new.id))
            .await
            .unwrap();
        app.claude_refresh_usage(&new.id).await.unwrap();
        assert_eq!(
            *fake.profile_proxies.lock().unwrap(),
            vec!["socks5://127.0.0.1:8002"]
        );
        let accounts = app.claude_accounts().await.unwrap();
        let new = accounts.iter().find(|a| a.id == new.id).unwrap();
        assert!(!new.awaiting_first_request);
        assert!(new.usage.is_some());
        assert_eq!(new.plan_type.as_deref(), Some("max 5x"));
        assert!(accounts
            .iter()
            .any(|a| a.id != new.id && a.awaiting_first_request));
        assert!(fake.refreshed.lock().unwrap().is_empty());
    }

    #[test]
    fn set_member_adds_to_empty_and_filled_objects() {
        let value = json!({"a": 1});
        assert_eq!(
            set_member("{}", "k", &value).unwrap(),
            "{\n  \"k\": {\n    \"a\": 1\n  }\n}"
        );
        let added = set_member("{\"x\": \"\\\"}\"}\n", "k", &json!(2)).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&added).unwrap(),
            json!({"x":"\"}","k":2})
        );
        assert!(set_member("[]", "k", &value).is_err());
    }

    #[test]
    fn settings_that_bypass_the_account_login_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let location = Location::files(dir.path().to_path_buf());
        assert!(login_overrides(&location).is_empty());
        fs::write(
            dir.path().join("settings.json"),
            r#"{"apiKeyHelper":"","env":{"ANTHROPIC_BASE_URL":"https://gateway.invalid","ANTHROPIC_AUTH_TOKEN":"","CLAUDE_CODE_USE_BEDROCK":"0","DISABLE_TELEMETRY":"1"}}"#,
        )
        .unwrap();
        assert!(login_overrides(&location).is_empty());
    }

    #[test]
    fn the_account_follows_a_claude_json_inside_the_configuration_directory() {
        let dir = tempfile::tempdir().unwrap();
        let location = Location {
            config_dir: dir.path().join(".claude"),
            global_config: dir.path().join(".claude.json"),
            keychain: None,
        };
        assert_eq!(location.account_file(), location.global_config);
        fs::create_dir_all(&location.config_dir).unwrap();
        let inner = location.config_dir.join(".claude.json");
        fs::write(&inner, r#"{"oauthAccount":{"accountUuid":"a"}}"#).unwrap();
        assert_eq!(location.account_file(), inner);
        fs::write(
            &location.global_config,
            r#"{"oauthAccount":{"accountUuid":"b"}}"#,
        )
        .unwrap();
        assert_eq!(location.account_file(), location.global_config);
    }

    #[test]
    fn credentials_accept_the_hexadecimal_keychain_form() {
        let document = json!({"claudeAiOauth": {"accessToken": "t"}});
        let encoded = hex(&serde_json::to_vec(&document).unwrap());
        assert_eq!(parse_credentials(encoded.as_bytes()), Some(document));
    }
}
