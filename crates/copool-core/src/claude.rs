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
const USER_AGENT: &str = "claude-code/2.1.69";
// The banked resets are returned only to Claude Code 2.1.280 or later, by its
// User-Agent; others read `eligible: false` with the rest of the usage intact.
// This is the earliest version that receives them.
const USAGE_USER_AGENT: &str = "claude-cli/2.1.280 (external, cli)";
/// How long usage is asked as `claude-code/…` through a proxy after the
/// usage endpoint refused `claude-cli/…` there.
const CLI_REFUSAL_SECONDS: i64 = 3600;
const SIGN_IN_EXPIRED: &str = "error.accounts.sign_in_expired";
const TOKEN_REFRESH_RATE_LIMITED: &str = "Claude token refresh failed: HTTP 429";
/// The pause after a first HTTP 429, doubled for each following one up to
/// the longest; a longer `Retry-After` is honored up to six hours.
const RATE_LIMIT_PAUSE: i64 = 300;
const RATE_LIMIT_LONGEST_PAUSE: i64 = 3600;
const RETRY_AFTER_LONGEST: i64 = 6 * 3600;
fn rate_limit_pause(count: u32, retry_after: Option<u64>) -> i64 {
    let doubled = RATE_LIMIT_PAUSE << count.saturating_sub(1).min(8);
    let requested = retry_after.map_or(0, |s| s.min(RETRY_AFTER_LONGEST as u64) as i64);
    doubled.min(RATE_LIMIT_LONGEST_PAUSE).max(requested)
}

/// The only custom OAuth endpoints Claude Code accepts; with another, it keeps
/// its login in the credentials file alone.
const APPROVED_CUSTOM_OAUTH: [&str; 3] = [
    "https://beacon.claude-ai.staging.ant.dev",
    "https://claude.fedstart.com",
    "https://claude-staging.fedstart.com",
];
/// Where Claude Code keeps its login.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub config_dir: PathBuf,
    pub global_config: PathBuf,
    /// The macOS Keychain service holding the credentials; none for the file.
    pub keychain: Option<String>,
    /// The directory of the credentials file Claude Code reads without the
    /// Keychain item.
    pub storage_dir: PathBuf,
}
impl Location {
    pub fn discover() -> Result<Self> {
        let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
        Ok(Self::resolve(
            &|name| std::env::var(name).ok(),
            home,
            cfg!(target_os = "macos"),
        ))
    }
    /// Claude Code's own rules (2.1.285), as observed in its sessions: the
    /// Keychain service `Claude Code{suffix}-credentials{-hash}`, else the
    /// credentials file. `CLAUDE_CODE_CUSTOM_OAUTH_URL`, from the user
    /// settings' `env` or else the environment, adds `-custom-oauth` for an
    /// endpoint Claude Code approves; for any other, Claude Code cannot name
    /// its Keychain item and keeps its login in the file alone. The hash and
    /// the file's directory follow `CLAUDE_SECURESTORAGE_CONFIG_DIR` when it is
    /// set at all, else `CLAUDE_CONFIG_DIR`. The global configuration stays
    /// `.claude.json`, which Claude Code resolves before applying the settings.
    fn resolve(env: &dyn Fn(&str) -> Option<String>, home: PathBuf, keychain: bool) -> Self {
        let set = |name: &str| env(name).filter(|v| !v.is_empty());
        // An isolated directory for smoke tests, never the Keychain.
        if let Some(dir) = set("COPOOL_CLAUDE_CONFIG_DIR") {
            return Self::files(dir.into());
        }
        let custom_dir = set("CLAUDE_CONFIG_DIR").map(PathBuf::from);
        let config_dir = custom_dir.clone().unwrap_or_else(|| home.join(".claude"));
        let global_config = match &custom_dir {
            Some(dir) => dir.join(".claude.json"),
            None => home.join(".claude.json"),
        };
        // The settings' env overrides the environment, an empty value included.
        let custom_oauth = fs::read_to_string(config_dir.join("settings.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|settings| {
                settings["env"]["CLAUDE_CODE_CUSTOM_OAUTH_URL"]
                    .as_str()
                    .map(str::to_owned)
            })
            .or_else(|| env("CLAUDE_CODE_CUSTOM_OAUTH_URL"))
            .filter(|url| !url.is_empty());
        let approved = custom_oauth
            .as_deref()
            .is_none_or(|url| APPROVED_CUSTOM_OAUTH.contains(&url.trim_end_matches('/')));
        let digest =
            |dir: &str| hex(&Sha256::digest(dir.replace('\\', "/").as_bytes()))[..8].to_owned();
        let (hash, storage_dir) = match env("CLAUDE_SECURESTORAGE_CONFIG_DIR") {
            Some(dir) if dir.is_empty() => (None, home.join(".claude")),
            Some(dir) => (Some(digest(&dir)), PathBuf::from(dir)),
            None => (
                custom_dir
                    .as_ref()
                    .map(|dir| digest(&dir.to_string_lossy())),
                config_dir.clone(),
            ),
        };
        let service = format!(
            "Claude Code{}-credentials{}",
            if custom_oauth.is_some() {
                "-custom-oauth"
            } else {
                ""
            },
            hash.map(|h| format!("-{h}")).unwrap_or_default()
        );
        Self {
            config_dir,
            global_config,
            keychain: (keychain && approved).then_some(service),
            storage_dir,
        }
    }
    /// Credentials and account in files of one directory, without the Keychain.
    pub fn files(dir: PathBuf) -> Self {
        Self {
            global_config: dir.join(".claude.json"),
            storage_dir: dir.clone(),
            config_dir: dir,
            keychain: None,
        }
    }
    fn credentials_file(&self) -> PathBuf {
        self.storage_dir.join(".credentials.json")
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

type UsageResponse = (u16, Value, Option<u64>);
pub(crate) trait Network: Send + Sync {
    /// The HTTP status and body of the OAuth usage endpoint, with the seconds
    /// of a `Retry-After` header.
    fn usage<'a>(
        &'a self,
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
    fn usage<'a>(
        &'a self,
        token: &'a str,
        agent: &'a str,
        proxy: &'a str,
    ) -> Response<'a, UsageResponse> {
        Box::pin(async move {
            let response = client(proxy)?
                .get(USAGE_URL)
                .header(reqwest::header::USER_AGENT, agent)
                .bearer_auth(token.trim())
                .header("Accept", "application/json")
                .header("anthropic-beta", "oauth-2025-04-20")
                .send()
                .await
                .map_err(request_error)?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse().ok());
            let body = response.json().await.unwrap_or(Value::Null);
            Ok((status, body, retry_after))
        })
    }
    fn refresh<'a>(
        &'a self,
        refresh_token: &'a str,
        scope: &'a str,
        proxy: &'a str,
    ) -> Response<'a, Value> {
        Box::pin(refresh_at(
            crate::login::CLAUDE_TOKEN_URL,
            refresh_token,
            scope,
            proxy,
        ))
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

/// Renews a login with the request Claude Code sends: a JSON body asking
/// again for the login's scopes, and no User-Agent of its own. The token
/// endpoint answers HTTP 429 to some agents (`claude-code/…`, `curl/…`)
/// whatever the token, while Claude Code's request passes.
async fn refresh_at(url: &str, refresh_token: &str, scope: &str, proxy: &str) -> Result<Value> {
    let response = crate::utils::with_account_proxy(reqwest::Client::builder(), proxy)?
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?
        .post(url)
        .json(&json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": crate::login::CLAUDE_CLIENT,
            "scope": scope,
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
}
/// The scopes a login was granted, as Claude Code asks for them on renewal;
/// the default ones for a login that records none.
fn refresh_scope(oauth: &Value) -> String {
    let scopes: Vec<&str> = oauth["scopes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .collect();
    if scopes.is_empty() {
        crate::login::CLAUDE_SCOPES.into()
    } else {
        scopes.join(" ")
    }
}

/// Keeps the pool's tokens apart from `claude-accounts.json`. On macOS, as
/// Claude Code keeps its login, they are in a Keychain item of Copool's own;
/// the store then holds everything but the tokens.
trait Vault: Send + Sync {
    /// The stored text; none before anything was stored.
    fn read(&self, access: KeychainAccess) -> Result<Option<String>>;
    fn write(&self, text: &str, access: KeychainAccess) -> Result<()>;
}
/// Each account's tokens and alternate generations, by account id.
type Secrets = serde_json::Map<String, Value>;
const SECRET_KEYS: [&str; 2] = ["accessToken", "refreshToken"];
#[cfg(target_os = "macos")]
struct KeychainVault;
#[cfg(target_os = "macos")]
impl Vault for KeychainVault {
    fn read(&self, access: KeychainAccess) -> Result<Option<String>> {
        keychain::read_private(VAULT_SERVICE, VAULT_ACCOUNT, access)?
            .map(|bytes| {
                String::from_utf8(bytes).map_err(|_| "Invalid Copool Keychain item".into())
            })
            .transpose()
    }
    fn write(&self, text: &str, access: KeychainAccess) -> Result<()> {
        keychain::write_private(
            VAULT_SERVICE,
            VAULT_ACCOUNT,
            VAULT_LABEL,
            text.as_bytes(),
            access,
        )
    }
}
#[cfg(target_os = "macos")]
const VAULT_SERVICE: &str = "io.github.copool.rust.claude";
#[cfg(target_os = "macos")]
const VAULT_ACCOUNT: &str = "accounts";
#[cfg(target_os = "macos")]
const VAULT_LABEL: &str = "Copool Claude accounts";
const VAULT_LOCKED: &str =
    "Copool cannot read the account's tokens from its Keychain item; reopen Copool and allow access";
/// The vault's text as a map; none when it is not one, so that it is never
/// replaced by what little is known.
fn parse_secrets(text: Option<String>) -> Option<Secrets> {
    match text {
        None => Some(Secrets::new()),
        Some(text) => serde_json::from_str::<Value>(&text)
            .ok()?
            .as_object()
            .cloned(),
    }
}
/// The account holds its tokens in memory, read from either place.
fn holds_tokens(account: &Stored) -> bool {
    SECRET_KEYS
        .iter()
        .any(|key| text(&account.oauth[*key]).is_some())
}
fn secrets_of(account: &Stored) -> Option<Value> {
    holds_tokens(account).then(|| {
        json!({
            "accessToken": account.oauth["accessToken"],
            "refreshToken": account.oauth["refreshToken"],
            "alternates": account.alternates,
        })
    })
}
fn restore_secrets(account: &mut Stored, secrets: &Value) {
    for key in SECRET_KEYS {
        if !secrets[key].is_null() {
            account.oauth[key] = secrets[key].clone();
        }
    }
    if account.alternates.is_empty() {
        account.alternates =
            serde_json::from_value(secrets["alternates"].clone()).unwrap_or_default();
    }
}
fn strip_secrets(account: &mut Stored) {
    if let Some(oauth) = account.oauth.as_object_mut() {
        for key in SECRET_KEYS {
            oauth.remove(key);
        }
    }
    account.alternates.clear();
}

/// The Claude pool's place in the application.
pub(crate) struct State {
    location: std::sync::RwLock<Location>,
    network: std::sync::RwLock<Arc<dyn Network>>,
    /// Where the pool's tokens are kept instead of the store, when not the
    /// default for the location (tests).
    vault: std::sync::RwLock<Option<Arc<dyn Vault>>>,
    /// The vault's contents as last read or written: tokens by account id.
    /// None until it has been read.
    secrets: std::sync::Mutex<Option<Secrets>>,
    /// Until when each proxy asks for usage as `claude-code/…`.
    cli_refused: std::sync::Mutex<std::collections::HashMap<String, i64>>,
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
            vault: Default::default(),
            secrets: Default::default(),
            cli_refused: Default::default(),
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
    /// Consecutive HTTP 429 responses, each doubling the pause.
    #[serde(default, skip_serializing_if = "is_zero")]
    rate_limits: u32,
    /// Newly imported; nothing goes upstream for it until a manual refresh.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    first_request_pending: bool,
    /// Other generations of this login not known to be invalid, newest
    /// first: tried in turn when Claude rejects the one in use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    alternates: Vec<Value>,
    /// Fingerprints of refresh tokens Claude rejected, never adopted again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rejected: Vec<String>,
    /// `oauth` is what Claude Code's login held when last read or written,
    /// so Claude Code renewing that login rotates it away.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    in_live: bool,
    /// The user's choice of whether to renew the tokens; none for the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    renewal: Option<Renewal>,
}
impl Stored {
    fn is_rejected(&self, oauth: &Value) -> bool {
        fingerprint(oauth).is_some_and(|print| self.rejected.contains(&print))
    }
    fn reject(&mut self, print: String) {
        if !self.rejected.contains(&print) {
            self.rejected.insert(0, print);
            self.rejected.truncate(MAX_REJECTED);
        }
        let rejected = &self.rejected;
        self.alternates
            .retain(|a| fingerprint(a).is_none_or(|print| !rejected.contains(&print)));
    }
    /// Keeps a generation to fall back on, unless Claude rejected it or it
    /// is the one in use.
    fn keep_alternate(&mut self, oauth: Value) {
        if !self.is_rejected(&oauth) && fingerprint(&oauth) != fingerprint(&self.oauth) {
            push_alternate(&mut self.alternates, oauth);
        }
    }
    /// Uses a generation of this login seen elsewhere, unless Claude rejected
    /// it. Validity, not expiry, decides between generations, so the previous
    /// one stays to fall back on unless Claude Code renewed it away. Returns
    /// whether the account changed.
    fn adopt(&mut self, oauth: &Value, from_live: bool) -> bool {
        if self.is_rejected(oauth) {
            // Claude Code no longer holds the generation in use.
            let changed = from_live && self.in_live;
            self.in_live &= !from_live;
            return changed;
        }
        if same_login(&self.oauth, oauth) {
            let changed = from_live && !self.in_live;
            self.in_live |= from_live;
            return changed;
        }
        let previous = std::mem::replace(&mut self.oauth, oauth.clone());
        let print = fingerprint(oauth);
        self.alternates.retain(|a| fingerprint(a) != print);
        if !(from_live && self.in_live) {
            self.keep_alternate(previous);
        }
        self.in_live = from_live;
        self.usage_error = None;
        true
    }
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
            renewal: Some(Renewal::of(self.renewal).as_str().into()),
        }
    }
}

/// The login Claude Code uses.
struct Live {
    /// The complete credentials document; other entries (MCP logins) are kept.
    document: Value,
    account: Option<Value>,
    held: Held,
}
/// Whether Copool renews an account's tokens; without a choice, always.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Renewal {
    Never,
    /// Once, then never.
    Once,
    Always,
}
impl Renewal {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "never" => Ok(Self::Never),
            "once" => Ok(Self::Once),
            "always" => Ok(Self::Always),
            _ => Err(format!("Unknown token renewal: {value}")),
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Once => "once",
            Self::Always => "always",
        }
    }
    fn of(choice: Option<Self>) -> Self {
        choice.unwrap_or(Self::Always)
    }
}
/// Where a login was read: the Keychain item, or the file Claude Code reads
/// while the item is missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Held {
    Keychain,
    File,
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
fn is_zero(count: &u32) -> bool {
    *count == 0
}
const MAX_ALTERNATES: usize = 3;
const MAX_REJECTED: usize = 16;
/// Identifies a generation by its refresh token, else its access token,
/// without keeping the token itself.
fn fingerprint(oauth: &Value) -> Option<String> {
    let token = text(&oauth["refreshToken"]).or_else(|| text(&oauth["accessToken"]))?;
    Some(hex(&Sha256::digest(token.as_bytes()))[..16].to_owned())
}
/// Both tokens are the same.
fn same_login(a: &Value, b: &Value) -> bool {
    ["accessToken", "refreshToken"]
        .iter()
        .all(|key| text(&a[key]) == text(&b[key]))
}
fn push_alternate(alternates: &mut Vec<Value>, oauth: Value) {
    // A generation without tokens (unread from the vault) is no alternate.
    let Some(print) = fingerprint(&oauth).map(Some) else {
        return;
    };
    alternates.retain(|a| fingerprint(a) != print);
    alternates.insert(0, oauth);
    alternates.truncate(MAX_ALTERNATES);
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
    for print in merged.rejected {
        existing.reject(print);
    }
    for alternate in merged.alternates {
        existing.keep_alternate(alternate);
    }
    // The card just refreshed holds the generation Claude last saw.
    existing.adopt(&merged.oauth, merged.in_live);
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
    // Claude Code records when the refresh token itself expires.
    if let Some(seconds) = number(&tokens["refresh_token_expires_in"]) {
        oauth["refreshTokenExpiresAt"] = (now_ms + (seconds * 1000.0) as i64).into();
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    let held = if document.is_some() {
        Held::Keychain
    } else {
        Held::File
    };
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
    Ok(Some(Live {
        document,
        account,
        held,
    }))
}
/// Writes a login where the one it replaces was read; with none, where
/// Claude Code reads first. A login read from the file goes back to the file:
/// creating the Keychain item would turn Claude Code away from the file.
async fn write_credentials(
    location: &Location,
    document: &Value,
    held: Option<Held>,
    access: KeychainAccess,
) -> Result<()> {
    #[cfg(not(target_os = "macos"))]
    let _ = access;
    let file = location.credentials_file();
    if let Some(service) = written_keychain(location, held) {
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
fn written_keychain(location: &Location, held: Option<Held>) -> Option<&String> {
    location
        .keychain
        .as_ref()
        .filter(|_| held != Some(Held::File))
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
/// whose tokens are used unless Claude rejected them. A missing login keeps
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
    for (j, account) in store.accounts.iter_mut().enumerate() {
        if Some(j) != matched && account.in_live {
            account.in_live = false;
            changed = true;
        }
    }
    if let (Some(i), Some(oauth)) = (matched, oauth) {
        changed |= store.accounts[i].adopt(oauth, true);
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
/// never replace the account being refreshed, even if its expiry is later,
/// nor a login Claude rejected, as stored or seen in this refresh.
fn changed_live_oauth(
    account: &Stored,
    rejected: &[String],
    before: Option<&Live>,
    after: Option<&Live>,
) -> Option<Value> {
    let after = after?;
    let oauth = after.oauth()?;
    if account.is_rejected(oauth)
        || fingerprint(oauth).is_some_and(|print| rejected.contains(&print))
    {
        return None;
    }
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
    } else if same_oauth_generation(&account.oauth, oauth) {
        return None;
    }
    Some(oauth.clone())
}

const CREDENTIALS_CHANGED: &str = "Claude credentials changed during refresh; retry usage";
use crate::models::RENEWAL_OFF;
/// Claude refused a refresh token; shown as an expired sign-in.
const REFRESH_REJECTED: &str = "Claude rejected the refresh token";
/// Renewing rotates the refresh token: the login Claude Code may be using is
/// not renewed unless the new tokens can be seen to reach it.
const LIVE_UNREADABLE: &str =
    "Cannot read the Claude Code login; allow access before renewing the account it uses";

impl Application {
    fn claude_store_path(&self) -> PathBuf {
        self.paths.data_dir.join("claude-accounts.json")
    }
    /// The store with each account's tokens: those the store itself holds
    /// (written while the vault could not be), else the vault's. An account
    /// whose tokens cannot be read is returned without them.
    fn claude_load(&self) -> Result<Store> {
        let mut store: Store = match fs::read(self.claude_store_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("Invalid Claude account store: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Store::default(),
            Err(e) => return Err(e.to_string()),
        };
        if let Some(vault) = self.claude_vault() {
            if store.accounts.iter().any(|a| !holds_tokens(a)) {
                if let Some(secrets) = self.claude_secrets(&*vault, KeychainAccess::Background) {
                    for account in store.accounts.iter_mut().filter(|a| !holds_tokens(a)) {
                        if let Some(found) = secrets.get(&account.id) {
                            restore_secrets(account, found);
                        }
                    }
                }
            }
        }
        Ok(store)
    }
    /// Saves the store, its tokens in the vault. The vault keeps an
    /// account's tokens that could not be read; when it cannot be written,
    /// the tokens stay in the store so that none is lost.
    fn claude_save(&self, store: &Store) -> Result<()> {
        let path = self.claude_store_path();
        let vaulted = match self.claude_vault() {
            Some(vault) => self.claude_vault_save(&*vault, store),
            None => false,
        };
        if vaulted {
            let mut file = store.clone();
            file.accounts.iter_mut().for_each(strip_secrets);
            write_json(&path, &file)?;
        } else {
            write_json(&path, store)?;
        }
        self.publish();
        Ok(())
    }
    /// Whether the vault now holds every account's tokens.
    fn claude_vault_save(&self, vault: &dyn Vault, store: &Store) -> bool {
        let Some(known) = self.claude_secrets(vault, KeychainAccess::Background) else {
            return false;
        };
        let next: Secrets = store
            .accounts
            .iter()
            .filter_map(|a| {
                Some((
                    a.id.clone(),
                    secrets_of(a).or_else(|| known.get(&a.id).cloned())?,
                ))
            })
            .collect();
        if next == known {
            return true;
        }
        let text = Value::Object(next.clone()).to_string();
        let stored = vault
            .write(&text, KeychainAccess::Background)
            .and_then(|()| vault.read(KeychainAccess::Background));
        let mut cache = self.claude.secrets.lock().unwrap();
        match stored {
            Ok(Some(read)) if read == text => {
                *cache = Some(next);
                true
            }
            outcome => {
                log::warn!(
                    "Copool's Keychain item was not updated; tokens stay in the store: {}",
                    outcome.err().unwrap_or_else(|| "read back differs".into())
                );
                // What the item holds now is not known.
                *cache = None;
                false
            }
        }
    }
    /// The vault's contents, read once; none when it cannot be read.
    fn claude_secrets(&self, vault: &dyn Vault, access: KeychainAccess) -> Option<Secrets> {
        let mut cache = self.claude.secrets.lock().unwrap();
        if cache.is_none() {
            *cache = parse_secrets(vault.read(access).ok()?);
        }
        cache.clone()
    }
    /// Reads the vault once, allowing the Keychain to ask the user, and
    /// moves tokens still in the store into it. Startup and user actions do
    /// this; background work never asks.
    async fn claude_unlock(&self) -> Result<()> {
        let Some(vault) = self.claude_vault() else {
            return Ok(());
        };
        if self.claude.secrets.lock().unwrap().is_none() {
            let read = tokio::task::spawn_blocking(move || vault.read(KeychainAccess::Interactive))
                .await
                .map_err(|e| e.to_string())??;
            let mut cache = self.claude.secrets.lock().unwrap();
            if cache.is_none() {
                *cache = parse_secrets(read);
            }
        }
        let _lock = self.claude.lock.lock().await;
        let store = self.claude_load()?;
        self.claude_save(&store)
    }
    /// The pool's tokens go to Copool's Keychain item where Claude Code keeps
    /// its own login in the Keychain; elsewhere they stay in the store.
    fn claude_vault(&self) -> Option<Arc<dyn Vault>> {
        if let Some(vault) = self.claude.vault.read().unwrap().clone() {
            return Some(vault);
        }
        #[cfg(target_os = "macos")]
        if self.claude_location().keychain.is_some() {
            return Some(Arc::new(KeychainVault));
        }
        None
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
    fn set_claude_vault(&self, vault: Arc<dyn Vault>) {
        *self.claude.vault.write().unwrap() = Some(vault);
        *self.claude.secrets.lock().unwrap() = None;
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
        // A refusal leaves the tokens where they are; refreshes report it.
        if let Err(error) = self.claude_unlock().await {
            log::warn!("Copool's Keychain item could not be read: {error}");
        }
        let live =
            read_live_with_access(&self.claude_location(), KeychainAccess::Interactive).await?;
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        if follow_live(&mut store, live.as_ref()) {
            self.claude_save(&store)?;
        }
        Ok(())
    }
    /// The pool's cards as stored, without reading Claude Code's login.
    pub(crate) fn claude_usage_cards(&self) -> Result<Vec<AccountSummary>> {
        let store = self.claude_load()?;
        Ok(store
            .accounts
            .iter()
            .map(|a| a.summary(false))
            .chain(sub2api::summaries(&store, None))
            .collect())
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
                let current = store.current_account_id.as_deref() == Some(a.id.as_str());
                a.summary(official && current)
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
                // Claude Code's login supersedes what it renewed; another
                // generation stays to fall back on until Claude settles it.
                existing.adopt(&oauth, current);
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
                    rate_limits: 0,
                    first_request_pending: defer,
                    alternates: Vec::new(),
                    rejected: Vec::new(),
                    in_live: current,
                    renewal: None,
                });
                (id, defer)
            }
        };
        if current {
            store.current_account_id = Some(id.clone());
            for other in store.accounts.iter_mut().filter(|a| a.id != id) {
                other.in_live = false;
            }
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
        let _ = self.claude_unlock().await;
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
            // An export goes nowhere until a switch writes it.
            held: Held::File,
        };
        let _op = self.claude.operations.lock().await;
        let _ = self.claude_unlock().await;
        let (id, _, waiting) = self.claude_import_live(live, false).await?;
        drop(_op);
        // Should Claude reject the export, the pool's tokens take over again.
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
        let _ = self.claude_unlock().await;
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
        if access == KeychainAccess::Interactive {
            let _ = self.claude_unlock().await;
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
        // Never hand Claude Code a login without its tokens.
        if !holds_tokens(&target) {
            return Err(VAULT_LOCKED.into());
        }
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
        // The target replaces the login where Claude Code read it.
        let held = live.as_ref().map(|live| live.held);
        let rollback = |error: String| async {
            // Without a previous login, none is left either.
            let restored = match &previous_document {
                Some(previous) => write_credentials(&location, previous, held, access).await,
                None => remove_credentials(&location, access).await,
            };
            restored
                .map_err(|rollback| format!("{error}; rollback failed: {rollback}"))
                .and(Err(error))
        };
        // The Keychain may already hold the target when the file write fails.
        if let Err(error) = write_credentials(&location, &document, held, access).await {
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
        for account in store.accounts.iter_mut() {
            account.in_live = account.id == id;
        }
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
        if !holds_tokens(&account) {
            return Err(VAULT_LOCKED.into());
        }
        // Renewal off: an expired account is reported, never renewed, and
        // nothing is written back to Claude Code.
        let held_back = (Renewal::of(account.renewal) == Renewal::Never).then_some(RENEWAL_OFF);
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
        // Generations to fall back on when Claude rejects the one in use.
        let mut alternates = account.alternates.clone();
        let mut rejected: Vec<String> = Vec::new();
        // `oauth` came from a token response, which proves it.
        let mut renewed = false;
        let mut renewals = 0;
        // `oauth` is what Claude Code's login holds.
        let mut in_live;
        let mut profiled = None;
        let mut rate_limited = false;
        let mut retry_after = None;
        let mut reloads = 0;
        let mut live_sync = Ok(());
        let result = loop {
            let mut result = async {
                let refresh = |oauth: &Value| {
                    let network = network.clone();
                    // A refresh token Claude rejected is not sent again.
                    let token =
                        text(&oauth["refreshToken"]).filter(|_| !account.is_rejected(oauth));
                    let scope = refresh_scope(oauth);
                    let location = &location;
                    let account = &account;
                    let rejected = &rejected;
                    let before = live.as_ref();
                    async move {
                        // Another app may have renewed while our usage request ran.
                        let latest = match read_live(location).await {
                            Ok(latest) => latest,
                            Err(_) if in_use => return Err(LIVE_UNREADABLE.into()),
                            Err(_) => None,
                        };
                        if changed_live_oauth(account, rejected, before, latest.as_ref()).is_some()
                        {
                            return Err(CREDENTIALS_CHANGED.into());
                        }
                        if let Some(reason) = held_back {
                            return Err(reason.into());
                        }
                        let token = token.ok_or(REFRESH_REJECTED)?;
                        match network.refresh(&token, &scope, proxy).await {
                            Err(error) if error == SIGN_IN_EXPIRED => Err(REFRESH_REJECTED.into()),
                            response => response,
                        }
                    }
                };
                if expires_at(&oauth) != 0 && expires_at(&oauth) <= (now_unix_seconds() + 60) * 1000
                {
                    let tokens = refresh(&oauth).await?;
                    apply_tokens(&mut oauth, &tokens, now_unix_seconds() * 1000)?;
                    renewed = true;
                    renewals += 1;
                }
                // An account imported without its name is probed first, through
                // its own proxy.
                if identity(&account.account).is_none() {
                    let token = text(&oauth["accessToken"]).unwrap_or_default();
                    let found = match network.profile(&token, proxy).await {
                        Err(error) if error == SIGN_IN_EXPIRED => {
                            let tokens = refresh(&oauth).await?;
                            apply_tokens(&mut oauth, &tokens, now_unix_seconds() * 1000)?;
                            renewed = true;
                            renewals += 1;
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
                let (mut status, mut body, mut after) =
                    self.claude_usage(&*network, &token, proxy).await?;
                if status == 401 {
                    let tokens = refresh(&oauth).await?;
                    apply_tokens(&mut oauth, &tokens, now_unix_seconds() * 1000)?;
                    renewed = true;
                    renewals += 1;
                    let token = text(&oauth["accessToken"]).unwrap_or_default();
                    (status, body, after) = self.claude_usage(&*network, &token, proxy).await?;
                }
                retry_after = after.filter(|_| status == 429);
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
            // Claude rejected this generation's refresh token: it is never
            // used again, and the next generation, if any, is tried.
            let refused = result
                .as_ref()
                .is_err_and(|error| error == REFRESH_REJECTED);
            if refused {
                rejected.extend(fingerprint(&oauth));
                alternates
                    .retain(|a| fingerprint(a).is_none_or(|print| !rejected.contains(&print)));
                result = Err(SIGN_IN_EXPIRED.to_owned());
            }
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
            if let Some(updated) =
                changed_live_oauth(&account, &rejected, live.as_ref(), latest.as_ref())
            {
                // What we renewed, or a generation Claude Code's login did not
                // hold, stays to fall back on.
                let held = live
                    .as_ref()
                    .and_then(Live::oauth)
                    .is_some_and(|held| same_login(held, &oauth));
                let previous = std::mem::replace(&mut oauth, updated);
                if !refused && (renewed || !held) {
                    push_alternate(&mut alternates, previous);
                }
                renewed = false;
                in_live = true;
                live = latest;
                profiled = None;
                rate_limited = false;
                if reloads == 0 {
                    reloads += 1;
                    continue;
                }
                break Err(CREDENTIALS_CHANGED.to_owned());
            }
            if refused && !alternates.is_empty() {
                oauth = alternates.remove(0);
                renewed = false;
                continue;
            }
            in_live = latest
                .as_ref()
                .and_then(Live::oauth)
                .is_some_and(|current| same_login(current, &oauth));
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
                    // A login Claude rejected is replaced by the generation
                    // it accepted, renewed or not.
                    let dead = rejected
                        .iter()
                        .chain(&account.rejected)
                        .any(|print| fingerprint(current).as_ref() == Some(print));
                    if same_account
                        && held_back.is_none()
                        && same_oauth_generation(old, current)
                        && (dead || !same_oauth_generation(&oauth, &account.oauth))
                        && !same_oauth_generation(&oauth, current)
                        && (result.is_ok() || renewed)
                    {
                        let mut document = latest.document.clone();
                        document["claudeAiOauth"] = oauth.clone();
                        live_sync = write_credentials(
                            &location,
                            &document,
                            Some(latest.held),
                            KeychainAccess::Background,
                        )
                        .await;
                        in_live = live_sync.is_ok();
                    }
                }
            }
            break result;
        };
        // Once renewed, the generation in use is proven.
        if renewed {
            alternates.clear();
        }
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        // Each HTTP 429 in a row pauses longer; any other answer starts over.
        let rate_limits = match (rate_limited, &result) {
            (true, _) => store
                .accounts
                .iter()
                .find(|a| a.id == id)
                .map_or(account.rate_limits, |a| a.rate_limits)
                .saturating_add(1),
            (false, Ok(_)) => 0,
            (false, Err(_)) => account.rate_limits,
        };
        let pause = rate_limited.then(|| rate_limit_pause(rate_limits, retry_after));
        let result = result.map_err(|error| match pause {
            Some(pause) => format!("{error}; retry paused for {} minutes", (pause + 59) / 60),
            None => error,
        });
        if let Some(target) = store.accounts.iter_mut().find(|a| a.id == id) {
            target.updated_at = now_unix_seconds();
            target.usage_retry_at = pause.map(|pause| now_unix_seconds() + pause);
            target.rate_limits = rate_limits;
            match &result {
                Ok(usage) => {
                    target.usage = Some(usage.clone());
                    target.usage_error = live_sync.as_ref().err().cloned();
                }
                Err(error) => target.usage_error = Some(error.clone()),
            }
            // Generations offered while this refresh ran stay to fall back on;
            // a primary this refresh did not change is left as it is now.
            let known = |o: &Value| account.alternates.iter().any(|a| same_login(a, o));
            let mut arrived: Vec<Value> = target
                .alternates
                .iter()
                .filter(|o| !known(o))
                .cloned()
                .collect();
            target.alternates = alternates;
            // A single renewal allowed has been used.
            if renewals > 0 && target.renewal == Some(Renewal::Once) {
                target.renewal = Some(Renewal::Never);
            }
            if oauth != account.oauth {
                let previous = std::mem::replace(&mut target.oauth, oauth);
                if !same_login(&previous, &account.oauth) {
                    arrived.insert(0, previous);
                }
                target.in_live = in_live;
            } else if same_login(&target.oauth, &oauth) {
                target.in_live = in_live;
            }
            for print in rejected.into_iter().rev() {
                target.reject(print);
            }
            for generation in arrived.into_iter().rev() {
                target.keep_alternate(generation);
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
    /// Asks for usage as Claude Code 2.1.280 (`claude-cli/…`), which the
    /// banked resets need. The endpoint at times refuses that agent outright,
    /// with HTTP 429 before reading the token; usage is then asked as
    /// `claude-code/…` at once, and through that proxy for the next hour.
    async fn claude_usage(
        &self,
        network: &dyn Network,
        token: &str,
        proxy: &str,
    ) -> Result<UsageResponse> {
        let now = now_unix_seconds();
        let refused = self
            .claude
            .cli_refused
            .lock()
            .unwrap()
            .get(proxy)
            .is_some_and(|until| *until > now);
        if !refused {
            let response = network.usage(token, USAGE_USER_AGENT, proxy).await?;
            if response.0 != 429 {
                return Ok(response);
            }
            self.claude
                .cli_refused
                .lock()
                .unwrap()
                .insert(proxy.to_owned(), now + CLI_REFUSAL_SECONDS);
        }
        network.usage(token, USER_AGENT, proxy).await
    }
    /// Sets whether Copool renews an account's tokens: "never", "once" (then
    /// never) or "always".
    pub async fn claude_set_renewal(&self, id: &str, renewal: &str) -> Result<()> {
        let renewal = Renewal::parse(renewal)?;
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        let account = store
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or("Account not found")?;
        account.renewal = Some(renewal);
        self.claude_save(&store)
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
        // As OpenQuota, an account asked within the current interval (by a
        // manual refresh, say) waits for the next round.
        let now = now_unix_seconds();
        let interval = self.refresh_interval().as_secs() as i64;
        let due = |a: &&AccountSummary| !sub2api::is_card(&a.id) && now - a.updated_at >= interval;
        let ids: Vec<String> = match crate::refresh_policy::recurring_ids(tick, &accounts) {
            None => accounts.iter().filter(due).map(|a| a.id.clone()).collect(),
            Some(ids) => accounts
                .iter()
                .filter(due)
                .filter(|a| ids.contains(&a.id))
                .map(|a| a.id.clone())
                .collect(),
        };
        let cards: Vec<String> = accounts
            .iter()
            .filter(|a| a.is_current && sub2api::is_card(&a.id))
            .map(|a| a.id.clone())
            .collect();
        self.claude_refresh_scoped(Some(ids), Some(&cards)).await
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

    const REFRESH_SECONDS: i64 = crate::refresh_policy::REFRESH_INTERVAL.as_secs() as i64;
    /// Usage is asked as `claude-code/…` only, as after the endpoint
    /// refused `claude-cli/…`: each HTTP 429 is then a real rate limit.
    fn refuse_cli(app: &Application) {
        app.claude
            .cli_refused
            .lock()
            .unwrap()
            .insert(String::new(), i64::MAX);
    }
    /// Moves every account's last request this many seconds back.
    fn age(app: &Application, seconds: i64) {
        let mut store = app.claude_load().unwrap();
        for account in &mut store.accounts {
            account.updated_at = now_unix_seconds() - seconds;
        }
        app.claude_save(&store).unwrap();
    }
    /// Ends a rate limit pause without forgetting how many came before.
    fn end_pause(app: &Application) {
        let mut store = app.claude_load().unwrap();
        store.accounts[0].usage_retry_at = Some(now_unix_seconds() - 1);
        app.claude_save(&store).unwrap();
    }

    #[test]
    fn rate_limit_pauses_double_up_to_an_hour_and_honor_a_longer_retry_after() {
        let pauses: Vec<i64> = (1..=7).map(|n| rate_limit_pause(n, None)).collect();
        assert_eq!(pauses, [300, 600, 1200, 2400, 3600, 3600, 3600]);
        assert_eq!(rate_limit_pause(u32::MAX, None), 3600);
        assert_eq!(rate_limit_pause(0, None), 300);
        // Retry-After: 0 or shorter than the backoff does not shorten it.
        assert_eq!(rate_limit_pause(1, Some(0)), 300);
        assert_eq!(rate_limit_pause(3, Some(60)), 1200);
        assert_eq!(rate_limit_pause(1, Some(5400)), 5400);
        assert_eq!(rate_limit_pause(1, Some(u64::MAX)), 6 * 3600);
    }

    #[tokio::test]
    async fn consecutive_rate_limits_pause_longer_until_usage_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        refuse_cli(&app);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        let stored = || app.claude_load().unwrap().accounts[0].clone();
        let usage = stored().usage.unwrap();
        for (count, minutes) in [(1, 5), (2, 10), (3, 20), (4, 40), (5, 60), (6, 60)] {
            fake.usage
                .lock()
                .unwrap()
                .push((429, json!({"error":{"message":"Rate limited"}})));
            let before = now_unix_seconds();
            let error = app.claude_refresh_usage(&id).await.unwrap_err();
            assert_eq!(
                error,
                format!("Claude usage request failed: HTTP 429 Rate limited; retry paused for {minutes} minutes")
            );
            let account = stored();
            assert_eq!(account.rate_limits, count);
            let until = account.usage_retry_at.unwrap();
            assert!((before + minutes * 60..=now_unix_seconds() + minutes * 60).contains(&until));
            // The last usage stays on the card with the error beside it.
            assert_eq!(
                serde_json::to_value(&account.usage).unwrap(),
                serde_json::to_value(Some(&usage)).unwrap()
            );
            assert_eq!(account.usage_error.as_deref(), Some(error.as_str()));
            // While paused, nothing is requested.
            let requests = usages(&fake).len();
            assert_eq!(app.claude_refresh_usage(&id).await.unwrap_err(), error);
            assert_eq!(usages(&fake).len(), requests);
            end_pause(&app);
        }
        app.claude_refresh_usage(&id).await.unwrap();
        let account = stored();
        assert_eq!(account.rate_limits, 0);
        assert_eq!(account.usage_retry_at, None);
        assert_eq!(account.usage_error, None);
        // The count is gone from the store once it is back to zero.
        let saved = fs::read_to_string(app.claude_store_path()).unwrap();
        assert!(!saved.contains("rateLimits"));
        // A first rate limit after a success pauses briefly again.
        fake.usage.lock().unwrap().push((429, Value::Null));
        app.claude_refresh_usage(&id).await.unwrap_err();
        assert_eq!(stored().rate_limits, 1);
    }

    #[tokio::test]
    async fn other_failures_keep_the_rate_limit_count_without_pausing() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        refuse_cli(&app);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        for _ in 0..2 {
            fake.usage.lock().unwrap().push((429, Value::Null));
            app.claude_refresh_usage(&id).await.unwrap_err();
            end_pause(&app);
        }
        fake.usage.lock().unwrap().push((500, Value::Null));
        assert_eq!(
            app.claude_refresh_usage(&id).await.unwrap_err(),
            "Claude usage request failed: HTTP 500"
        );
        let account = app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(account.rate_limits, 2);
        assert_eq!(account.usage_retry_at, None);
        fake.usage.lock().unwrap().push((429, Value::Null));
        assert!(app
            .claude_refresh_usage(&id)
            .await
            .unwrap_err()
            .ends_with("retry paused for 20 minutes"));
    }

    #[tokio::test]
    async fn a_longer_retry_after_sets_the_pause() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        refuse_cli(&app);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        for (retry_after, minutes) in [(Some(5400), 90), (Some(0), 10), (None, 20)] {
            *fake.usage_retry_after.lock().unwrap() = retry_after;
            fake.usage.lock().unwrap().push((429, Value::Null));
            let before = now_unix_seconds();
            let error = app.claude_refresh_usage(&id).await.unwrap_err();
            assert!(
                error.ends_with(&format!("retry paused for {minutes} minutes")),
                "{error}"
            );
            let until = app.claude_load().unwrap().accounts[0]
                .usage_retry_at
                .unwrap();
            assert!((before + minutes * 60..=now_unix_seconds() + minutes * 60).contains(&until));
            end_pause(&app);
        }
        // A Retry-After on a successful response sets no pause.
        *fake.usage_retry_after.lock().unwrap() = Some(9999);
        app.claude_refresh_usage(&id).await.unwrap();
        let account = app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(account.usage_retry_at, None);
        assert_eq!(account.rate_limits, 0);
    }

    #[tokio::test]
    async fn background_rounds_adapt_their_interval_to_usage_changes() {
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        refuse_cli(&app);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        assert_eq!(usages(&fake).len(), 1);
        let (five, one) = (Duration::from_secs(300), Duration::from_secs(60));
        let usage = |hour: u32| (200, json!({"five_hour": {"utilization": hour}}));
        let mut unchanged = 0;
        // Asked at import: the first round waits, and nothing changed.
        assert_eq!(app.background_round(0, &mut unchanged).await, five);
        assert_eq!(usages(&fake).len(), 1);
        fake.usage.lock().unwrap().push(usage(25));
        age(&app, 300);
        assert_eq!(app.background_round(1, &mut unchanged).await, one);
        assert_eq!(app.refresh_interval(), one);
        assert_eq!(usages(&fake).len(), 2);
        // At one minute, an account asked under a minute ago waits...
        age(&app, 45);
        assert_eq!(app.background_round(2, &mut unchanged).await, one);
        assert_eq!(usages(&fake).len(), 2);
        // ...and the second unchanged round slows down again.
        fake.usage.lock().unwrap().push(usage(25));
        age(&app, 60);
        assert_eq!(app.background_round(3, &mut unchanged).await, five);
        assert_eq!(usages(&fake).len(), 3);
        assert_eq!(app.refresh_interval(), five);
        // At five minutes, a minute-old request is not due.
        age(&app, 60);
        assert_eq!(app.background_round(4, &mut unchanged).await, five);
        assert_eq!(usages(&fake).len(), 3);
        // A change while slow speeds up at once.
        fake.usage.lock().unwrap().push(usage(30));
        age(&app, 300);
        assert_eq!(app.background_round(6, &mut unchanged).await, one);
        assert_eq!(usages(&fake).len(), 4);
        // A rate limit is no change in usage.
        fake.usage.lock().unwrap().push((429, Value::Null));
        age(&app, 60);
        assert_eq!(app.background_round(7, &mut unchanged).await, one);
        assert_eq!(unchanged, 1);
        assert_eq!(usages(&fake).len(), 5);
    }

    #[tokio::test]
    async fn background_refreshes_skip_accounts_asked_within_the_interval() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "bob", LATER);
        app.claude_import_current().await.unwrap();
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        assert_eq!(usages(&fake), ["bob-access", "alice-access"]);
        // Every tick, full or not, skips accounts asked within the interval.
        for tick in 0..12 {
            app.claude_refresh_recurring(tick).await.unwrap();
        }
        assert_eq!(usages(&fake).len(), 2);
        age(&app, REFRESH_SECONDS - 5);
        app.claude_refresh_recurring(0).await.unwrap();
        assert_eq!(usages(&fake).len(), 2);
        // Once due, a tick between full ones asks only the current account...
        age(&app, REFRESH_SECONDS);
        app.claude_refresh_recurring(1).await.unwrap();
        assert_eq!(usages(&fake)[2..], ["alice-access"]);
        app.claude_refresh_recurring(2).await.unwrap();
        assert_eq!(usages(&fake).len(), 3);
        // ...and a full tick only those still due.
        app.claude_refresh_recurring(6).await.unwrap();
        assert_eq!(usages(&fake)[3..], ["bob-access"]);
        // A manual refresh is not held back by the interval.
        app.claude_refresh(None).await.unwrap();
        let mut manual = usages(&fake)[4..].to_vec();
        manual.sort();
        assert_eq!(manual, ["alice-access", "bob-access"]);
        app.claude_refresh_usage(&app.claude_load().unwrap().accounts[0].id)
            .await
            .unwrap();
        assert_eq!(usages(&fake).len(), 7);
    }

    #[tokio::test]
    async fn usage_rate_limits_pause_manual_and_background_refresh_until_backoff_expires() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        refuse_cli(&app);
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
        assert_eq!(
            error,
            "Claude usage request failed: HTTP 429 Rate limited; retry paused for 5 minutes"
        );
        assert_eq!(app.claude_refresh_usage(&id).await.unwrap_err(), error);
        age(&app, REFRESH_SECONDS);
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
            let paused = format!("{TOKEN_REFRESH_RATE_LIMITED}; retry paused for 5 minutes");
            assert_eq!(app.claude_refresh_usage(&id).await.unwrap_err(), paused);
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
            assert_eq!(app.claude_refresh_usage(&id).await.unwrap_err(), paused);
            assert!(app.claude_refresh(None).await.is_err());
            age(&app, REFRESH_SECONDS);
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

    /// Serves one canned response per connection and returns each request's
    /// head (lowercased) and body.
    async fn token_server(
        responses: Vec<&'static str>,
    ) -> (String, tokio::task::JoinHandle<Vec<(String, Value)>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/oauth/token", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut requests = vec![];
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut raw = vec![];
                let mut buffer = [0; 4096];
                let (head, body) = loop {
                    let read = stream.read(&mut buffer).await.unwrap();
                    raw.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&raw).into_owned();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let head = text[..end].to_lowercase();
                        let length: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .map_or(0, |v| v.trim().parse().unwrap());
                        if raw.len() >= end + 4 + length {
                            break (head, text[end + 4..end + 4 + length].to_owned());
                        }
                    }
                };
                requests.push((head, serde_json::from_str(&body).unwrap()));
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (url, server)
    }

    #[tokio::test]
    async fn refresh_requests_match_claude_code_without_a_user_agent() {
        let json = |status: &str, body: &str| -> &'static str {
            Box::leak(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .into_boxed_str(),
            )
        };
        let (url, server) = token_server(vec![
            json("200 OK", r#"{"access_token":"new","refresh_token":"next","expires_in":28800}"#),
            json("400 Bad Request", r#"{"error":"invalid_grant","error_description":"Refresh token not found or invalid"}"#),
            json("429 Too Many Requests", r#"{"error":{"type":"rate_limit_error","message":"Rate limited. Please try again later."}}"#),
            json("500 Internal Server Error", "{}"),
        ])
        .await;
        let scope = "user:inference user:profile";
        let tokens = refresh_at(&url, "rt-1", scope, "direct://").await.unwrap();
        assert_eq!(tokens["access_token"], "new");
        assert_eq!(
            refresh_at(&url, "rt-2", scope, "direct://")
                .await
                .unwrap_err(),
            SIGN_IN_EXPIRED
        );
        assert_eq!(
            refresh_at(&url, "rt-3", scope, "direct://")
                .await
                .unwrap_err(),
            TOKEN_REFRESH_RATE_LIMITED
        );
        assert_eq!(
            refresh_at(&url, "rt-4", scope, "direct://")
                .await
                .unwrap_err(),
            "Claude token refresh failed: HTTP 500"
        );
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 4);
        for (n, (head, body)) in requests.iter().enumerate() {
            assert!(head.starts_with("post /v1/oauth/token http/1.1"), "{head}");
            // The token endpoint refuses `claude-code/…` and `curl/…` agents.
            assert!(!head.contains("\r\nuser-agent:"), "{head}");
            assert!(
                head.contains("\r\ncontent-type: application/json"),
                "{head}"
            );
            assert!(!head.contains("authorization:"), "{head}");
            assert!(!head.contains("anthropic-beta:"), "{head}");
            assert_eq!(
                *body,
                json!({
                    "grant_type": "refresh_token",
                    "refresh_token": format!("rt-{}", n + 1),
                    "client_id": crate::login::CLAUDE_CLIENT,
                    "scope": scope,
                })
            );
        }
    }

    #[test]
    fn renewal_asks_again_for_the_scopes_the_login_holds() {
        let default = crate::login::CLAUDE_SCOPES;
        assert_eq!(
            refresh_scope(&json!({"scopes": ["user:inference", "user:profile"]})),
            "user:inference user:profile"
        );
        assert_eq!(
            refresh_scope(&json!({"scopes": [" user:inference ", "", 7]})),
            "user:inference"
        );
        for oauth in [
            json!({}),
            json!({"scopes": []}),
            json!({"scopes": "user:inference"}),
            json!({"scopes": [""]}),
        ] {
            assert_eq!(refresh_scope(&oauth), default, "{oauth}");
        }
    }

    #[tokio::test]
    async fn a_refresh_sends_the_login_scopes_and_records_both_expiries() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _, fake) = waiting_app(&dir);
        let (mut oauth, account) = login("bob", 1);
        oauth["scopes"] = json!(["user:inference", "user:profile"]);
        app.claude_import_json(
            &json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string(),
        )
        .await
        .unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(
            *fake.refresh_scopes.lock().unwrap(),
            ["user:inference user:profile"]
        );
        // A login without scopes asks for the default ones.
        let (expiring, other) = login("carol", 1);
        app.claude_import_json(
            &json!({"claudeAiOauth": expiring, "oauthAccount": other}).to_string(),
        )
        .await
        .unwrap();
        let carol = app
            .claude_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| a.label == "carol@example.com")
            .unwrap()
            .id;
        app.claude_release_new_accounts(Some(&carol)).await.unwrap();
        app.claude_refresh_usage(&carol).await.unwrap();
        assert_eq!(
            fake.refresh_scopes.lock().unwrap()[1],
            crate::login::CLAUDE_SCOPES
        );
        let mut renewed = json!({});
        let now = 1_000_000;
        apply_tokens(
            &mut renewed,
            &json!({"access_token": "a", "refresh_token": "r", "expires_in": 28800, "refresh_token_expires_in": 2_592_000, "scope": "user:inference user:profile"}),
            now,
        )
        .unwrap();
        assert_eq!(renewed["expiresAt"], now + 28_800_000);
        assert_eq!(renewed["refreshTokenExpiresAt"], now + 2_592_000_000);
        assert_eq!(renewed["scopes"], json!(["user:inference", "user:profile"]));
        // A response without it leaves the recorded expiry.
        renewed["refreshTokenExpiresAt"] = 5.into();
        apply_tokens(&mut renewed, &json!({"access_token": "b"}), now).unwrap();
        assert_eq!(renewed["refreshTokenExpiresAt"], 5);
        assert_eq!(renewed["refreshToken"], "r");
    }

    /// Claude's real token endpoint, through `COPOOL_LIVE_CLAUDE_PROXY` (or
    /// the system proxy), with a refresh token that cannot exist: a request
    /// it accepts is refused as `invalid_grant`, one it filters gets HTTP 429.
    #[tokio::test]
    #[ignore = "contacts platform.claude.com"]
    async fn the_live_token_endpoint_accepts_the_refresh_request() {
        let proxy = std::env::var("COPOOL_LIVE_CLAUDE_PROXY").unwrap_or_default();
        let error = refresh_at(
            crate::login::CLAUDE_TOKEN_URL,
            "sk-ant-ort01-copool-invalid-probe",
            crate::login::CLAUDE_SCOPES,
            &proxy,
        )
        .await
        .unwrap_err();
        assert_eq!(error, SIGN_IN_EXPIRED);
    }

    fn agents(fake: &Fake) -> Vec<String> {
        fake.usage_agents.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn usage_is_asked_as_claude_code_while_claude_cli_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        let (cli, code) = (USAGE_USER_AGENT, USER_AGENT);
        assert_eq!(agents(&fake), [cli]);
        // Refused as claude-cli, the same refresh asks as claude-code.
        fake.usage
            .lock()
            .unwrap()
            .push((429, json!({"error": {"type": "rate_limit_error"}})));
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(agents(&fake)[1..], [cli, code]);
        let account = app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(account.usage_error, None);
        assert_eq!(account.rate_limits, 0);
        assert_eq!(account.usage_retry_at, None);
        // Through that proxy, only claude-code is asked for the next hour...
        let until = app.claude.cli_refused.lock().unwrap()[""];
        assert!((now_unix_seconds() + 3590..=now_unix_seconds() + 3600).contains(&until));
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(agents(&fake)[3..], [code]);
        // ...and claude-cli again after it.
        app.claude
            .cli_refused
            .lock()
            .unwrap()
            .insert(String::new(), now_unix_seconds() - 1);
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(agents(&fake)[4..], [cli]);
        // Any other answer to claude-cli is final.
        fake.usage.lock().unwrap().push((500, Value::Null));
        assert!(app.claude_refresh_usage(&id).await.is_err());
        assert_eq!(agents(&fake)[5..], [cli]);
    }

    #[tokio::test]
    async fn a_refusal_holds_only_for_its_proxy_and_both_refusals_are_a_rate_limit() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        sign_in(&location, "bob", LATER);
        app.claude_import_current().await.unwrap();
        let id = |user: &str| {
            app.claude_load()
                .unwrap()
                .accounts
                .into_iter()
                .find(|a| a.label == format!("{user}@example.com"))
                .unwrap()
                .id
        };
        app.claude_save_account_proxy(&id("alice"), "http://127.0.0.1:7001")
            .await
            .unwrap();
        app.claude_save_account_proxy(&id("bob"), "http://127.0.0.1:7002")
            .await
            .unwrap();
        let (cli, code) = (USAGE_USER_AGENT, USER_AGENT);
        let start = agents(&fake).len();
        fake.usage.lock().unwrap().push((429, Value::Null));
        app.claude_refresh_usage(&id("alice")).await.unwrap();
        app.claude_refresh_usage(&id("bob")).await.unwrap();
        assert_eq!(agents(&fake)[start..], [cli, code, cli]);
        assert_eq!(
            fake.usage_proxies.lock().unwrap()[start..],
            [
                "http://127.0.0.1:7001",
                "http://127.0.0.1:7001",
                "http://127.0.0.1:7002"
            ]
        );
        // Refused both ways: a rate limit, counted and paused once.
        fake.usage.lock().unwrap().push((429, Value::Null));
        fake.usage
            .lock()
            .unwrap()
            .push((429, json!({"error": {"message": "Rate limited"}})));
        *fake.usage_retry_after.lock().unwrap() = Some(5400);
        let error = app.claude_refresh_usage(&id("bob")).await.unwrap_err();
        assert_eq!(agents(&fake)[start + 3..], [cli, code]);
        assert_eq!(
            error,
            "Claude usage request failed: HTTP 429 Rate limited; retry paused for 90 minutes"
        );
        let bob = app
            .claude_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| a.id == id("bob"))
            .unwrap();
        assert_eq!(bob.rate_limits, 1);
    }

    #[tokio::test]
    async fn the_usage_retry_after_a_renewal_falls_back_as_well() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        fake.usage.lock().unwrap().push((401, Value::Null));
        fake.usage.lock().unwrap().push((429, Value::Null));
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(
            agents(&fake),
            [USAGE_USER_AGENT, USAGE_USER_AGENT, USER_AGENT]
        );
        assert_eq!(
            usages(&fake),
            ["alice-access", "fresh-access", "fresh-access"]
        );
    }

    /// Copool's Keychain item, in memory.
    #[derive(Default)]
    struct MemoryVault {
        text: Mutex<Option<String>>,
        unreadable: std::sync::atomic::AtomicBool,
        unwritable: std::sync::atomic::AtomicBool,
        /// Writes report success without storing anything.
        lost_writes: std::sync::atomic::AtomicBool,
        reads: Mutex<Vec<KeychainAccess>>,
        writes: Mutex<usize>,
    }
    impl Vault for MemoryVault {
        fn read(&self, access: KeychainAccess) -> Result<Option<String>> {
            self.reads.lock().unwrap().push(access);
            if self.unreadable.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("The user denied access".into());
            }
            Ok(self.text.lock().unwrap().clone())
        }
        fn write(&self, text: &str, _: KeychainAccess) -> Result<()> {
            if self.unwritable.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("The user denied access".into());
            }
            *self.writes.lock().unwrap() += 1;
            if !self.lost_writes.load(std::sync::atomic::Ordering::SeqCst) {
                *self.text.lock().unwrap() = Some(text.into());
            }
            Ok(())
        }
    }
    impl MemoryVault {
        fn secrets(&self) -> Value {
            serde_json::from_str(self.text.lock().unwrap().as_deref().unwrap_or("{}")).unwrap()
        }
        fn writes(&self) -> usize {
            *self.writes.lock().unwrap()
        }
        fn reads(&self) -> Vec<KeychainAccess> {
            self.reads.lock().unwrap().clone()
        }
        fn set(&self, flag: &std::sync::atomic::AtomicBool, value: bool) {
            flag.store(value, std::sync::atomic::Ordering::SeqCst);
        }
    }
    fn vault_app(
        dir: &tempfile::TempDir,
        vault: &Arc<MemoryVault>,
    ) -> (Application, Location, Arc<Fake>) {
        let (app, location, fake) = waiting_app(dir);
        app.set_claude_vault(vault.clone());
        (app, location, fake)
    }
    fn store_text(app: &Application) -> String {
        fs::read_to_string(app.claude_store_path()).unwrap()
    }
    fn assert_no_tokens(app: &Application, tokens: &[&str]) {
        let text = store_text(app);
        for token in tokens {
            assert!(!text.contains(token), "{token} in the store");
        }
    }
    fn export(oauth: &Value, account: &Value) -> String {
        json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string()
    }
    use KeychainAccess::{Background, Interactive};

    #[tokio::test]
    async fn the_pool_tokens_live_in_the_vault_and_the_store_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let (app, _, _) = vault_app(&dir, &vault);
        let (oauth, account) = login("bob", LATER);
        app.claude_import_json(&export(&oauth, &account))
            .await
            .unwrap();
        let (mut newer, _) = login("bob", LATER + 1);
        newer["accessToken"] = "bob-newer".into();
        newer["refreshToken"] = "bob-newer-refresh".into();
        app.claude_import_json(&export(&newer, &account))
            .await
            .unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        assert_no_tokens(&app, &["bob-access", "bob-refresh", "bob-newer"]);
        let file: Value = serde_json::from_str(&store_text(&app)).unwrap();
        let saved = &file["accounts"][0];
        assert_eq!(
            saved["oauth"],
            json!({"expiresAt": LATER + 1, "subscriptionType": "max", "rateLimitTier": "default_claude_max_20x"})
        );
        assert!(saved.get("alternates").is_none());
        assert_eq!(saved["label"], "bob@example.com");
        assert_eq!(saved["firstRequestPending"], true);
        assert_eq!(
            vault.secrets(),
            json!({id.clone(): {"accessToken": "bob-newer", "refreshToken": "bob-newer-refresh", "alternates": [oauth]}})
        );
        let loaded = app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(loaded.oauth, newer);
        assert_eq!(loaded.alternates, [oauth.clone()]);
        // Read once, asking the user; written, and read back, as tokens change.
        assert_eq!(vault.reads(), [Interactive, Background, Background]);
        assert_eq!(vault.writes(), 2);
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        app.claude_accounts().await.unwrap();
        assert_eq!(vault.writes(), 2);
        assert_eq!(vault.reads().len(), 3);
        // Reopened, the tokens come back from the vault without asking.
        drop(app);
        let (app, _, fake) = vault_app(&dir, &vault);
        assert_eq!(app.claude_accounts().await.unwrap().len(), 1);
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(usages(&fake), ["bob-newer"]);
        assert_eq!(vault.reads()[3..], [Background]);
    }

    #[tokio::test]
    async fn tokens_left_in_the_store_move_to_the_vault_at_startup() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, _) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        assert!(store_text(&app).contains("alice-refresh"));
        let before = app.claude_load().unwrap();
        drop(app);
        let vault = Arc::new(MemoryVault::default());
        let (app, _, _) = vault_app(&dir, &vault);
        app.claude_authorize_at_startup().await.unwrap();
        assert_eq!(vault.reads(), [Interactive, Background]);
        assert_no_tokens(&app, &["alice-access", "alice-refresh"]);
        let id = &before.accounts[0].id;
        assert_eq!(vault.secrets()[id]["refreshToken"], "alice-refresh");
        let after = app.claude_load().unwrap();
        assert_eq!(after.accounts[0].oauth, before.accounts[0].oauth);
        assert_eq!(after.current_account_id, before.current_account_id);
        assert!(after.accounts[0].first_request_pending);
    }

    #[tokio::test]
    async fn an_unreadable_vault_loses_no_tokens_and_contacts_no_one() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let (app, location, _) = vault_app(&dir, &vault);
        let (oauth, account) = login("bob", LATER);
        app.claude_import_json(&export(&oauth, &account))
            .await
            .unwrap();
        let bob = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&bob)).await.unwrap();
        let held = vault.secrets();
        drop(app);
        // Reopened without access to the vault (a new build not yet allowed).
        vault.set(&vault.unreadable, true);
        let (app, _, fake) = vault_app(&dir, &vault);
        assert_eq!(app.claude_accounts().await.unwrap().len(), 1);
        assert_eq!(
            app.claude_refresh_usage(&bob).await.unwrap_err(),
            VAULT_LOCKED
        );
        assert!(usages(&fake).is_empty());
        assert!(refreshes(&fake).is_empty());
        // Another import keeps its tokens in the store; the vault is untouched.
        let (carol, carol_account) = login("carol", LATER);
        app.claude_import_json(&export(&carol, &carol_account))
            .await
            .unwrap();
        assert!(store_text(&app).contains("carol-refresh"));
        assert_no_tokens(&app, &["bob-access", "bob-refresh"]);
        assert_eq!(vault.secrets(), held);
        assert_eq!(vault.writes(), 1);
        // Claude Code is never handed a login without its tokens.
        assert_eq!(
            app.claude_switch_account(&bob).await.unwrap_err(),
            VAULT_LOCKED
        );
        assert!(!location.credentials_file().exists());
        // Once allowed, every token is there and moves to the vault.
        vault.set(&vault.unreadable, false);
        let store = app.claude_load().unwrap();
        let tokens: Vec<_> = store
            .accounts
            .iter()
            .map(|a| a.oauth["refreshToken"].clone())
            .collect();
        assert_eq!(tokens, [json!("bob-refresh"), json!("carol-refresh")]);
        app.claude_authorize_at_startup().await.unwrap();
        assert_no_tokens(&app, &["bob-refresh", "carol-refresh"]);
        assert_eq!(vault.secrets().as_object().unwrap().len(), 2);
        app.claude_refresh_usage(&bob).await.unwrap();
        assert_eq!(usages(&fake), ["bob-access"]);
    }

    #[tokio::test]
    async fn a_vault_that_does_not_take_a_write_leaves_the_tokens_in_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let (app, _, _) = vault_app(&dir, &vault);
        // Refused: the store keeps them.
        vault.set(&vault.unwritable, true);
        let (oauth, account) = login("bob", LATER);
        app.claude_import_json(&export(&oauth, &account))
            .await
            .unwrap();
        assert!(store_text(&app).contains("bob-refresh"));
        assert_eq!(vault.secrets(), json!({}));
        assert_eq!(app.claude_load().unwrap().accounts[0].oauth, oauth);
        // Reported done but not stored: the read back tells, the store keeps them.
        vault.set(&vault.unwritable, false);
        vault.set(&vault.lost_writes, true);
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        assert!(vault.writes() >= 1);
        assert!(store_text(&app).contains("bob-refresh"));
        assert_eq!(vault.secrets(), json!({}));
        // The next save that sticks moves them.
        vault.set(&vault.lost_writes, false);
        app.claude_save_account_proxy(&id, "http://127.0.0.1:7001")
            .await
            .unwrap();
        assert_no_tokens(&app, &["bob-access", "bob-refresh"]);
        assert_eq!(vault.secrets()[&id]["refreshToken"], "bob-refresh");
    }

    #[tokio::test]
    async fn vault_contents_that_cannot_be_read_as_tokens_are_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        *vault.text.lock().unwrap() = Some("not the pool".into());
        let (app, _, _) = vault_app(&dir, &vault);
        let (oauth, account) = login("bob", LATER);
        app.claude_import_json(&export(&oauth, &account))
            .await
            .unwrap();
        assert!(store_text(&app).contains("bob-refresh"));
        assert_eq!(vault.text.lock().unwrap().as_deref(), Some("not the pool"));
        assert_eq!(vault.writes(), 0);
    }

    #[tokio::test]
    async fn renewals_switches_and_deletions_go_through_the_vault() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let (app, location, fake) = vault_app(&dir, &vault);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let (oauth, account) = login("bob", 1);
        app.claude_import_json(&export(&oauth, &account))
            .await
            .unwrap();
        let ids = |user: &str| {
            app.claude_load()
                .unwrap()
                .accounts
                .into_iter()
                .find(|a| a.label == format!("{user}@example.com"))
                .unwrap()
                .id
        };
        let (alice, bob) = (ids("alice"), ids("bob"));
        app.claude_release_new_accounts(None).await.unwrap();
        // A renewal rotates the tokens in the vault, never in the store.
        app.claude_refresh_usage(&bob).await.unwrap();
        assert_eq!(refreshes(&fake), ["bob-refresh"]);
        assert_eq!(vault.secrets()[&bob]["refreshToken"], "fresh-refresh");
        assert_no_tokens(&app, &["fresh-access", "fresh-refresh", "alice-refresh"]);
        // Renewed while the vault refuses writes: the store's newer tokens
        // win over the vault's until they can be moved.
        vault.set(&vault.unwritable, true);
        fake.usage.lock().unwrap().push((401, Value::Null));
        *fake.refresh_response.lock().unwrap() = Some(
            json!({"access_token": "next-access", "refresh_token": "next-refresh", "expires_in": 3600}),
        );
        app.claude_refresh_usage(&bob).await.unwrap();
        assert_eq!(refreshes(&fake), ["bob-refresh", "fresh-refresh"]);
        assert!(store_text(&app).contains("next-refresh"));
        assert_eq!(vault.secrets()[&bob]["refreshToken"], "fresh-refresh");
        let rotated = app.claude_load().unwrap();
        let held = rotated.accounts.iter().find(|a| a.id == bob).unwrap();
        assert_eq!(held.oauth["refreshToken"], "next-refresh");
        vault.set(&vault.unwritable, false);
        // A switch hands Claude Code the tokens from the vault.
        app.claude_switch_account(&bob).await.unwrap();
        assert_eq!(live_access(&location), "next-access");
        assert_no_tokens(&app, &["next-access", "next-refresh", "alice-access"]);
        assert_eq!(vault.secrets()[&bob]["refreshToken"], "next-refresh");
        assert_eq!(vault.secrets()[&alice]["refreshToken"], "alice-refresh");
        // A deleted account leaves the vault too.
        app.claude_delete_account(&alice).await.unwrap();
        assert_eq!(
            vault
                .secrets()
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            [&bob]
        );
    }

    #[tokio::test]
    async fn claude_codes_login_serves_its_account_while_the_vault_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let (app, location, _) = vault_app(&dir, &vault);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let (bob, bob_account) = login("bob", LATER);
        app.claude_import_json(&export(&bob, &bob_account))
            .await
            .unwrap();
        // The pool's alice tokens are not the ones Claude Code holds.
        let mut store = app.claude_load().unwrap();
        store.accounts[0].in_live = false;
        app.claude_save(&store).unwrap();
        let alice = store.accounts[0].id.clone();
        drop(app);
        vault.set(&vault.unreadable, true);
        let (app, _, fake) = vault_app(&dir, &vault);
        // Claude Code renewed its login meanwhile.
        external_update(location.clone(), "alice", "alice-renewed")();
        app.claude_accounts().await.unwrap();
        let account = app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(account.oauth["accessToken"], "alice-renewed");
        // The tokens it could not read are no generation to fall back on.
        assert!(account.alternates.is_empty());
        app.claude_release_new_accounts(Some(&alice)).await.unwrap();
        app.claude_refresh_usage(&alice).await.unwrap();
        assert_eq!(usages(&fake), ["alice-renewed"]);
        // Readable again: the store's newer alice tokens win over the vault's
        // older ones, and bob's come from the vault.
        vault.set(&vault.unreadable, false);
        assert_eq!(vault.secrets()[&alice]["accessToken"], "alice-access");
        let store = app.claude_load().unwrap();
        assert_eq!(store.accounts[0].oauth["accessToken"], "alice-renewed");
        assert_eq!(store.accounts[1].oauth["accessToken"], "bob-access");
        app.claude_authorize_at_startup().await.unwrap();
        assert_eq!(vault.secrets()[&alice]["accessToken"], "alice-renewed");
        assert_no_tokens(&app, &["alice-renewed", "bob-access"]);
    }

    #[tokio::test]
    async fn a_store_read_before_the_vault_opened_keeps_its_tokens_there() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let (app, _, _) = vault_app(&dir, &vault);
        let (oauth, account) = login("bob", LATER);
        app.claude_import_json(&export(&oauth, &account))
            .await
            .unwrap();
        let held = vault.secrets();
        drop(app);
        vault.set(&vault.unreadable, true);
        let (app, _, _) = vault_app(&dir, &vault);
        // Read while the vault was closed, saved after it opened.
        let stale = app.claude_load().unwrap();
        assert!(!holds_tokens(&stale.accounts[0]));
        vault.set(&vault.unreadable, false);
        app.claude_load().unwrap();
        app.claude_save(&stale).unwrap();
        assert_eq!(vault.secrets(), held);
        assert_eq!(app.claude_load().unwrap().accounts[0].oauth, oauth);
    }

    #[tokio::test]
    async fn a_login_claude_code_reads_from_its_file_is_renewed_back_into_it() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = file_login_app(&dir);
        sign_in(&location, "alice", 1);
        app.claude_import_current().await.unwrap();
        let alice = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&alice)).await.unwrap();
        // By default renewed, and the renewal goes back to the file it was
        // read from, never to a new Keychain item.
        app.claude_refresh_usage(&alice).await.unwrap();
        assert_eq!(refreshes(&fake), ["alice-refresh"]);
        assert_eq!(live_access(&location), "fresh-access");
        let document: Value =
            serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
        assert_eq!(document["mcpOAuth"]["kept"], true);
        // Renewal turned off: the file is followed, not renewed nor written.
        app.claude_set_renewal(&alice, "never").await.unwrap();
        external_update(location.clone(), "alice", "alice-synced")();
        let synced = fs::read(location.credentials_file()).unwrap();
        fake.usage.lock().unwrap().push((401, Value::Null));
        assert_eq!(
            app.claude_refresh_usage(&alice).await.unwrap_err(),
            RENEWAL_OFF
        );
        assert_eq!(refreshes(&fake), ["alice-refresh"]);
        assert_eq!(fs::read(location.credentials_file()).unwrap(), synced);
        assert_eq!(
            app.claude_load().unwrap().accounts[0].oauth["accessToken"],
            "alice-synced"
        );
    }

    /// A pool whose current login Claude Code reads from its file, with
    /// Copool's tokens in memory.
    fn file_login_app(dir: &tempfile::TempDir) -> (Application, Location, Arc<Fake>) {
        let (app, files, fake) = waiting_app(dir);
        let location = Location {
            keychain: Some(format!(
                "io.github.copool.rust.test-absent-{}",
                std::process::id()
            )),
            ..files
        };
        app.set_claude_location(location.clone());
        app.set_claude_vault(Arc::new(MemoryVault::default()));
        (app, location, fake)
    }
    async fn renewals(app: &Application) -> Vec<(String, Option<String>)> {
        app.claude_accounts()
            .await
            .unwrap()
            .into_iter()
            .map(|a| (a.label, a.renewal))
            .collect()
    }

    #[tokio::test]
    async fn the_renewal_choice_holds_back_overrides_or_allows_once() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = file_login_app(&dir);
        sign_in(&location, "alice", 1);
        app.claude_import_current().await.unwrap();
        let (bob, account) = login("bob", 1);
        app.claude_import_json(&export(&bob, &account))
            .await
            .unwrap();
        app.claude_release_new_accounts(None).await.unwrap();
        let id = |user: &str| {
            app.claude_load()
                .unwrap()
                .accounts
                .into_iter()
                .find(|a| a.label == format!("{user}@example.com"))
                .unwrap()
                .id
        };
        let (alice, bob) = (id("alice"), id("bob"));
        // By default every account, the file login included, is renewed.
        let mut cards = renewals(&app).await;
        cards.sort();
        assert_eq!(
            cards,
            [
                ("alice@example.com".to_string(), Some("always".to_string())),
                ("bob@example.com".to_string(), Some("always".to_string())),
            ]
        );
        // Never: an expired account is reported, not renewed.
        app.claude_set_renewal(&bob, "never").await.unwrap();
        assert_eq!(
            app.claude_refresh_usage(&bob).await.unwrap_err(),
            RENEWAL_OFF
        );
        assert!(refreshes(&fake).is_empty());
        // Once: renewed once, then never again.
        app.claude_set_renewal(&bob, "once").await.unwrap();
        let card = |label: &str, cards: Vec<(String, Option<String>)>| {
            cards.into_iter().find(|c| c.0 == label).unwrap().1
        };
        assert_eq!(
            card("bob@example.com", renewals(&app).await).as_deref(),
            Some("once")
        );
        app.claude_refresh_usage(&bob).await.unwrap();
        assert_eq!(refreshes(&fake), ["bob-refresh"]);
        assert_eq!(
            card("bob@example.com", renewals(&app).await).as_deref(),
            Some("never")
        );
        fake.usage.lock().unwrap().push((401, Value::Null));
        assert_eq!(
            app.claude_refresh_usage(&bob).await.unwrap_err(),
            RENEWAL_OFF
        );
        assert_eq!(refreshes(&fake), ["bob-refresh"]);
        // Always, chosen or by default, renews the file login into its file.
        app.claude_set_renewal(&alice, "always").await.unwrap();
        app.claude_refresh_usage(&alice).await.unwrap();
        assert_eq!(refreshes(&fake), ["bob-refresh", "alice-refresh"]);
        assert_eq!(live_access(&location), "fresh-access");
        assert_eq!(
            card("alice@example.com", renewals(&app).await).as_deref(),
            Some("always")
        );
        // The choice is stored; an unknown one or account is refused.
        let saved: Value = serde_json::from_str(&store_text(&app)).unwrap();
        let choices: Vec<_> = saved["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["renewal"].clone())
            .collect();
        assert!(choices.contains(&json!("always")) && choices.contains(&json!("never")));
        assert!(app.claude_set_renewal(&bob, "sometimes").await.is_err());
        assert!(app.claude_set_renewal("missing", "always").await.is_err());
    }

    #[test]
    fn renewal_choices_read_and_default_as_shown() {
        for (text, choice) in [
            ("never", Renewal::Never),
            ("once", Renewal::Once),
            ("always", Renewal::Always),
        ] {
            assert_eq!(Renewal::parse(text), Ok(choice));
            assert_eq!(choice.as_str(), text);
            assert_eq!(serde_json::to_value(choice).unwrap(), json!(text));
            assert_eq!(Renewal::of(Some(choice)), choice);
        }
        assert_eq!(Renewal::of(None), Renewal::Always);
        assert!(Renewal::parse("Never").is_err());
        let legacy = json!({"id": "a", "label": "a", "oauth": {}, "account": {}, "addedAt": 1, "updatedAt": 1});
        let stored: Stored = serde_json::from_value(legacy).unwrap();
        assert_eq!(stored.renewal, None);
        assert!(serde_json::to_value(&stored)
            .unwrap()
            .get("renewal")
            .is_none());
    }

    #[tokio::test]
    async fn background_work_never_asks_for_keychain_access() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let (app, location, _) = vault_app(&dir, &vault);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        drop(app);
        let (app, _, _) = vault_app(&dir, &vault);
        let start = vault.reads().len();
        let mut unchanged = 0;
        app.background_round(1, &mut unchanged).await;
        app.claude_refresh(None).await.unwrap();
        app.claude_accounts().await.unwrap();
        assert!(vault.reads()[start..].iter().all(|a| *a == Background));
        assert!(!vault.reads()[start..].is_empty());
        // A refusal is not asked again in the background, nor cached. (With
        // Claude Code's login present, its tokens would serve the account.)
        drop(app);
        fs::remove_file(location.credentials_file()).unwrap();
        vault.set(&vault.unreadable, true);
        let (app, _, _) = vault_app(&dir, &vault);
        let start = vault.reads().len();
        assert!(app
            .claude_refresh(None)
            .await
            .unwrap_err()
            .ends_with(VAULT_LOCKED));
        assert!(vault.reads()[start..].iter().all(|a| *a == Background));
        assert!(vault.reads().len() - start >= 2);
        // The startup authorization asks, once.
        let start = vault.reads().len();
        app.claude_authorize_at_startup().await.unwrap();
        let asked = vault.reads()[start..]
            .iter()
            .filter(|a| **a == Interactive)
            .count();
        assert_eq!(asked, 1);
    }

    #[derive(Default)]
    pub(super) struct Fake {
        usage: Mutex<Vec<(u16, Value)>>,
        usage_retry_after: Mutex<Option<u64>>,
        refreshed: Mutex<Vec<String>>,
        refresh_scopes: Mutex<Vec<String>>,
        profile_proxies: Mutex<Vec<String>>,
        usage_tokens: Mutex<Vec<String>>,
        usage_agents: Mutex<Vec<String>>,
        usage_proxies: Mutex<Vec<String>>,
        usage_hooks: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
        refresh_hooks: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
        refresh_error: Mutex<Option<String>>,
        /// The next token response, instead of the usual one.
        refresh_response: Mutex<Option<Value>>,
        prepaid: Mutex<Option<Value>>,
        prepaid_requests: Mutex<Vec<(String, String)>>,
    }
    impl Network for Fake {
        fn usage<'a>(
            &'a self,
            token: &'a str,
            agent: &'a str,
            proxy: &'a str,
        ) -> Response<'a, UsageResponse> {
            Box::pin(async move {
                self.usage_tokens.lock().unwrap().push(token.into());
                self.usage_agents.lock().unwrap().push(agent.into());
                self.usage_proxies.lock().unwrap().push(proxy.into());
                let mut hooks = self.usage_hooks.lock().unwrap();
                if !hooks.is_empty() {
                    hooks.remove(0)();
                }
                drop(hooks);
                let mut queued = self.usage.lock().unwrap();
                let (status, body) = if queued.is_empty() {
                    (
                        200,
                        json!({"five_hour":{"utilization":25,"resets_at":"2099-01-01T00:00:00Z"},"seven_day":{"utilization":"40.5","resets_at":null}}),
                    )
                } else {
                    queued.remove(0)
                };
                Ok((status, body, *self.usage_retry_after.lock().unwrap()))
            })
        }
        fn refresh<'a>(
            &'a self,
            token: &'a str,
            scope: &'a str,
            _: &'a str,
        ) -> Response<'a, Value> {
            Box::pin(async move {
                self.refreshed.lock().unwrap().push(token.into());
                self.refresh_scopes.lock().unwrap().push(scope.into());
                let mut hooks = self.refresh_hooks.lock().unwrap();
                if !hooks.is_empty() {
                    hooks.remove(0)();
                }
                drop(hooks);
                if let Some(error) = self.refresh_error.lock().unwrap().take() {
                    return Err(error);
                }
                if let Some(response) = self.refresh_response.lock().unwrap().take() {
                    return Ok(response);
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

    fn print(refresh: &str) -> String {
        fingerprint(&json!({ "refreshToken": refresh })).unwrap()
    }
    fn refreshes(fake: &Fake) -> Vec<String> {
        fake.refreshed.lock().unwrap().clone()
    }
    fn usages(fake: &Fake) -> Vec<String> {
        fake.usage_tokens.lock().unwrap().clone()
    }
    /// Claude answers the next usage request with 401 and refuses the next
    /// refresh token.
    fn reject_next(fake: &Fake) {
        fake.usage.lock().unwrap().push((401, Value::Null));
        *fake.refresh_error.lock().unwrap() = Some(SIGN_IN_EXPIRED.into());
    }
    fn refresh_tokens(oauths: &[Value]) -> Vec<String> {
        oauths
            .iter()
            .map(|o| text(&o["refreshToken"]).unwrap())
            .collect()
    }

    #[test]
    fn fingerprints_identify_a_generation_by_its_refresh_token() {
        let both = fingerprint(&json!({"accessToken": "a", "refreshToken": "r"})).unwrap();
        assert_eq!(both.len(), 16);
        assert!(both.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(
            Some(both.clone()),
            fingerprint(&json!({"accessToken": "b", "refreshToken": "r"}))
        );
        assert_ne!(Some(both), fingerprint(&json!({"accessToken": "a"})));
        assert_eq!(fingerprint(&json!({"refreshToken": " "})), None);
        assert_eq!(fingerprint(&json!({})), None);
    }

    #[test]
    fn generations_are_optional_in_the_store_and_omitted_when_empty() {
        let legacy = json!({"id": "a", "label": "a", "oauth": {}, "account": {}, "addedAt": 1, "updatedAt": 1});
        let stored: Stored = serde_json::from_value(legacy).unwrap();
        assert!(stored.alternates.is_empty());
        assert!(stored.rejected.is_empty());
        assert!(!stored.in_live);
        let saved = serde_json::to_value(&stored).unwrap();
        for key in ["alternates", "rejected", "inLive"] {
            assert!(saved.get(key).is_none(), "{key}");
        }
    }

    #[test]
    fn adopting_keeps_what_claude_code_did_not_renew_away() {
        let (oauth, account) = login("alice", LATER);
        let mut stored = Stored {
            id: "a".into(),
            label: "alice".into(),
            oauth: oauth.clone(),
            account,
            proxy_url: String::new(),
            added_at: 0,
            updated_at: 0,
            usage: None,
            usage_error: Some(SIGN_IN_EXPIRED.into()),
            usage_retry_at: None,
            rate_limits: 0,
            first_request_pending: false,
            alternates: Vec::new(),
            rejected: Vec::new(),
            in_live: false,
            renewal: None,
        };
        let generation = |refresh: &str, expires: i64| {
            let (mut oauth, _) = login("alice", expires);
            oauth["accessToken"] = format!("{refresh}-access").into();
            oauth["refreshToken"] = refresh.into();
            oauth
        };
        // The same tokens only record that Claude Code holds them.
        assert!(stored.adopt(&oauth, true));
        assert!(!stored.adopt(&oauth, true));
        assert!(stored.alternates.is_empty());
        // Claude Code renewed what it held: nothing to fall back on.
        assert!(stored.adopt(&generation("b", LATER - 9), true));
        assert!(stored.alternates.is_empty());
        assert_eq!(stored.usage_error, None);
        // Another source keeps the previous generation, whatever its expiry.
        assert!(stored.adopt(&generation("c", LATER - 99), false));
        assert_eq!(refresh_tokens(&stored.alternates), ["b"]);
        assert!(!stored.in_live);
        // Claude Code's login, newer or not, keeps a generation it never held.
        assert!(stored.adopt(&generation("d", 1), true));
        assert_eq!(refresh_tokens(&stored.alternates), ["c", "b"]);
        // The newest generations are kept, without duplicates.
        stored.in_live = false;
        for refresh in ["e", "c", "f"] {
            stored.adopt(&generation(refresh, LATER), false);
        }
        assert_eq!(stored.oauth["refreshToken"], "f");
        assert_eq!(refresh_tokens(&stored.alternates), ["c", "e", "d"]);
        // A rejected generation is dropped and never adopted again.
        stored.reject(print("e"));
        assert_eq!(refresh_tokens(&stored.alternates), ["c", "d"]);
        stored.keep_alternate(generation("e", LATER));
        assert!(!stored.adopt(&generation("e", LATER), false));
        assert_eq!(stored.oauth["refreshToken"], "f");
        assert_eq!(refresh_tokens(&stored.alternates), ["c", "d"]);
        // A rejected login in Claude Code means it no longer holds ours.
        stored.in_live = true;
        assert!(stored.adopt(&generation("e", LATER), true));
        assert!(!stored.in_live);
        assert!(!stored.adopt(&generation("e", LATER), true));
        // The rejected list stays bounded, newest first.
        for n in 0..MAX_REJECTED + 4 {
            stored.reject(print(&n.to_string()));
        }
        assert_eq!(stored.rejected.len(), MAX_REJECTED);
        assert_eq!(stored.rejected[0], print(&(MAX_REJECTED + 3).to_string()));
    }

    #[tokio::test]
    async fn the_live_login_replaces_rotated_pool_tokens_with_a_later_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", LATER + 1);
        app.claude_import_current().await.unwrap();
        let stored = || app.claude_load().unwrap().accounts[0].clone();
        assert!(stored().in_live);
        // Claude Code's login expires earlier than the pool's revoked copy.
        let (mut oauth, account) = login("alice", LATER);
        oauth["refreshToken"] = "alice-live".into();
        write_json(
            &location.credentials_file(),
            &json!({"claudeAiOauth": oauth}),
        )
        .unwrap();
        write_account(&location, &account).unwrap();
        app.claude_accounts().await.unwrap();
        assert_eq!(stored().oauth, oauth);
        // Claude Code renewed the generation it held: nothing is kept.
        assert!(stored().alternates.is_empty());
        assert!(stored().in_live);

        // Importing the current login replaces a later-expiring copy too.
        let mut pool = app.claude_load().unwrap();
        pool.accounts[0].oauth["refreshToken"] = "alice-revoked".into();
        pool.accounts[0].oauth["expiresAt"] = (LATER + 1).into();
        pool.accounts[0].in_live = false;
        app.claude_save(&pool).unwrap();
        app.claude_import_current().await.unwrap();
        assert_eq!(stored().oauth, oauth);
        assert_eq!(refresh_tokens(&stored().alternates), ["alice-revoked"]);
        assert!(refreshes(&fake).is_empty());
        assert!(usages(&fake).is_empty());
    }

    #[tokio::test]
    async fn an_imported_export_is_kept_only_while_claude_accepts_it() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _, fake) = waiting_app(&dir);
        let (oauth, account) = login("bob", LATER);
        let export =
            |oauth: &Value| json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string();
        app.claude_import_json(&export(&oauth)).await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        let stored = || app.claude_load().unwrap().accounts[0].clone();
        // A later expiry does not make a rotated refresh token valid.
        let (mut rotated, _) = login("bob", LATER + 1);
        rotated["accessToken"] = "bob-revoked".into();
        rotated["refreshToken"] = "bob-export".into();
        reject_next(&fake);
        app.claude_import_json(&export(&rotated)).await.unwrap();
        assert_eq!(refreshes(&fake), ["bob-export"]);
        assert_eq!(usages(&fake), ["bob-revoked", "bob-access"]);
        assert_eq!(stored().oauth, oauth);
        assert!(stored().alternates.is_empty());
        assert_eq!(stored().rejected, [print("bob-export")]);
        assert_eq!(stored().usage_error, None);
        assert!(stored().usage.is_some());
        // The rejected export is never taken again.
        app.claude_import_json(&export(&rotated)).await.unwrap();
        assert_eq!(stored().oauth, oauth);
        assert_eq!(refreshes(&fake), ["bob-export"]);
        // An earlier expiry Claude accepts is used; the pool's tokens remain
        // until its refresh token is proven.
        let (mut valid, _) = login("bob", LATER - 1);
        valid["accessToken"] = "bob-valid".into();
        valid["refreshToken"] = "bob-valid-refresh".into();
        app.claude_import_json(&export(&valid)).await.unwrap();
        assert_eq!(stored().oauth, valid);
        assert_eq!(stored().alternates, [oauth.clone()]);
        // Its access token worked, but Claude later refuses its refresh token.
        reject_next(&fake);
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(refreshes(&fake), ["bob-export", "bob-valid-refresh"]);
        assert_eq!(
            usages(&fake),
            [
                "bob-revoked",
                "bob-access",
                "bob-access",
                "bob-valid",
                "bob-valid",
                "bob-access"
            ]
        );
        assert_eq!(stored().oauth, oauth);
        assert!(stored().alternates.is_empty());
        assert_eq!(
            stored().rejected,
            [print("bob-valid-refresh"), print("bob-export")]
        );
        assert_eq!(stored().usage_error, None);
        // Only fingerprints of rejected tokens are written down.
        let saved = fs::read_to_string(app.claude_store_path()).unwrap();
        assert!(!saved.contains("bob-export"));
        assert!(!saved.contains("bob-valid"));
    }

    #[tokio::test]
    async fn a_rejected_refresh_token_is_never_sent_again() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _, fake) = waiting_app(&dir);
        let (oauth, account) = login("bob", LATER);
        app.claude_import_json(
            &json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string(),
        )
        .await
        .unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        reject_next(&fake);
        assert_eq!(
            app.claude_refresh_usage(&id).await.unwrap_err(),
            SIGN_IN_EXPIRED
        );
        fake.usage.lock().unwrap().push((401, Value::Null));
        assert_eq!(
            app.claude_refresh_usage(&id).await.unwrap_err(),
            SIGN_IN_EXPIRED
        );
        assert_eq!(refreshes(&fake), ["bob-refresh"]);
        assert_eq!(usages(&fake), ["bob-access", "bob-access"]);
        let stored = app.claude_load().unwrap().accounts[0].clone();
        // With nothing to fall back on, the login stays for display.
        assert_eq!(stored.oauth, oauth);
        assert_eq!(stored.rejected, [print("bob-refresh")]);
        assert_eq!(stored.usage_error.as_deref(), Some(SIGN_IN_EXPIRED));
        // A usable access token still reports usage until it expires.
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(refreshes(&fake), ["bob-refresh"]);
    }

    #[tokio::test]
    async fn a_rejected_login_falls_back_to_the_pool_and_repairs_claude_code() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        // Copool renewed alice, but the renewal never reached Claude Code.
        let mut pool = app.claude_load().unwrap();
        let renewed = &mut pool.accounts[0].oauth;
        renewed["accessToken"] = "pool-access".into();
        renewed["refreshToken"] = "pool-refresh".into();
        renewed["expiresAt"] = (LATER + 1).into();
        let renewed = renewed.clone();
        pool.accounts[0].in_live = false;
        app.claude_save(&pool).unwrap();
        let stored = || app.claude_load().unwrap().accounts[0].clone();
        app.claude_accounts().await.unwrap();
        assert_eq!(stored().oauth["refreshToken"], "alice-refresh");
        assert_eq!(stored().alternates, [renewed.clone()]);
        assert!(stored().in_live);

        reject_next(&fake);
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(refreshes(&fake), ["alice-refresh"]);
        assert_eq!(usages(&fake), ["alice-access", "pool-access"]);
        let after = stored();
        assert_eq!(after.oauth, renewed);
        assert!(after.alternates.is_empty());
        assert_eq!(after.rejected, [print("alice-refresh")]);
        assert_eq!(after.usage_error, None);
        assert!(after.in_live);
        // Claude Code receives the accepted generation, other members intact.
        let document: Value =
            serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
        assert_eq!(document["claudeAiOauth"], renewed);
        assert_eq!(document["mcpOAuth"]["kept"], true);

        // Claude Code holding the rejected login again is not followed...
        sign_in(&location, "alice", LATER);
        app.claude_accounts().await.unwrap();
        assert_eq!(stored().oauth, renewed);
        assert!(!stored().in_live);
        assert_eq!(
            app.claude_load().unwrap().current_account_id,
            Some(id.clone())
        );
        // ...and the next refresh repairs it without sending the rejected token.
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(refreshes(&fake), ["alice-refresh"]);
        assert_eq!(live_access(&location), "pool-access");
        assert!(stored().in_live);
    }

    #[tokio::test]
    async fn a_renewal_proves_the_login_and_drops_the_alternates() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _, fake) = waiting_app(&dir);
        let (oauth, account) = login("bob", LATER);
        let export =
            |oauth: &Value| json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string();
        app.claude_import_json(&export(&oauth)).await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        let (mut expiring, _) = login("bob", 1);
        expiring["accessToken"] = "bob-new".into();
        expiring["refreshToken"] = "bob-new-refresh".into();
        app.claude_import_json(&export(&expiring)).await.unwrap();
        assert_eq!(refreshes(&fake), ["bob-new-refresh"]);
        assert_eq!(usages(&fake), ["fresh-access"]);
        let stored = app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(stored.oauth["refreshToken"], "fresh-refresh");
        assert!(stored.alternates.is_empty());
        assert!(stored.rejected.is_empty());
    }

    #[tokio::test]
    async fn a_waiting_account_keeps_its_newest_generations_without_requests() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _, fake) = waiting_app(&dir);
        let (oauth, account) = login("bob", LATER);
        let export =
            |oauth: &Value| json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string();
        app.claude_import_json(&export(&oauth)).await.unwrap();
        for n in 1..=5 {
            let (mut generation, _) = login("bob", LATER - n);
            generation["refreshToken"] = format!("bob-{n}").into();
            app.claude_import_json(&export(&generation)).await.unwrap();
        }
        let stored = || app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(app.claude_load().unwrap().accounts.len(), 1);
        assert_eq!(stored().oauth["refreshToken"], "bob-5");
        assert_eq!(
            refresh_tokens(&stored().alternates),
            ["bob-4", "bob-3", "bob-2"]
        );
        // Importing a held generation again uses it, without a duplicate.
        let (mut again, _) = login("bob", LATER);
        again["refreshToken"] = "bob-3".into();
        app.claude_import_json(&export(&again)).await.unwrap();
        assert_eq!(stored().oauth["refreshToken"], "bob-3");
        assert_eq!(
            refresh_tokens(&stored().alternates),
            ["bob-5", "bob-4", "bob-2"]
        );
        assert!(stored().first_request_pending);
        assert!(refreshes(&fake).is_empty());
        assert!(usages(&fake).is_empty());
    }

    #[tokio::test]
    async fn claude_code_renewing_its_login_supersedes_only_what_it_held() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        sign_in(&location, "bob", LATER);
        app.claude_import_current().await.unwrap();
        let account = |user: &str| {
            app.claude_load()
                .unwrap()
                .accounts
                .into_iter()
                .find(|a| a.label == format!("{user}@example.com"))
                .unwrap()
        };
        assert!(account("bob").in_live);
        assert!(!account("alice").in_live);
        app.claude_switch_account(&account("alice").id)
            .await
            .unwrap();
        assert!(account("alice").in_live);
        assert!(!account("bob").in_live);
        // Claude Code renews alice: the generation it held is gone.
        external_update(location.clone(), "alice", "alice-renewed")();
        app.claude_accounts().await.unwrap();
        assert_eq!(account("alice").oauth["accessToken"], "alice-renewed");
        assert!(account("alice").alternates.is_empty());
        // A sign-in to bob there keeps bob's pool tokens, which it did not hold.
        let bob = account("bob").oauth;
        external_update(location.clone(), "bob", "bob-login")();
        app.claude_accounts().await.unwrap();
        assert_eq!(account("bob").oauth["accessToken"], "bob-login");
        assert_eq!(account("bob").alternates, [bob]);
        assert!(account("bob").in_live);
        assert!(!account("alice").in_live);
        assert_eq!(
            app.claude_load().unwrap().current_account_id,
            Some(account("bob").id)
        );
        assert!(refreshes(&fake).is_empty());
        assert!(usages(&fake).is_empty());
    }

    #[tokio::test]
    async fn a_renewal_survives_a_login_that_became_readable_during_the_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        let (oauth, account) = login("alice", 1);
        app.claude_import_json(
            &json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string(),
        )
        .await
        .unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        // Unreadable when the refresh starts, an older login once it renews.
        fs::create_dir_all(&location.config_dir).unwrap();
        fs::write(location.credentials_file(), "not json").unwrap();
        fake.refresh_hooks.lock().unwrap().push(external_update(
            location.clone(),
            "alice",
            "stale",
        ));
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(refreshes(&fake), ["alice-refresh"]);
        assert_eq!(usages(&fake), ["fresh-access", "stale"]);
        let stored = || app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(stored().oauth["accessToken"], "stale");
        assert_eq!(refresh_tokens(&stored().alternates), ["fresh-refresh"]);
        assert!(stored().in_live);
        // When Claude refuses that login, the renewal takes over and reaches
        // Claude Code.
        reject_next(&fake);
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(refreshes(&fake), ["alice-refresh", "stale-refresh"]);
        assert_eq!(live_access(&location), "fresh-access");
        assert_eq!(stored().oauth["refreshToken"], "fresh-refresh");
        assert!(stored().alternates.is_empty());
        assert_eq!(stored().rejected, [print("stale-refresh")]);
        assert!(stored().in_live);
    }

    #[tokio::test]
    async fn a_refresh_keeps_a_generation_adopted_while_it_ran() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _, fake) = waiting_app(&dir);
        let (oauth, account) = login("bob", LATER);
        app.claude_import_json(
            &json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string(),
        )
        .await
        .unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        let (mut other, _) = login("bob", LATER);
        other["accessToken"] = "bob-other".into();
        other["refreshToken"] = "bob-other-refresh".into();
        let (path, adopted) = (app.claude_store_path(), other.clone());
        fake.usage_hooks.lock().unwrap().push(Box::new(move || {
            let mut store: Store = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert!(store.accounts[0].adopt(&adopted, false));
            write_json(&path, &store).unwrap();
        }));
        app.claude_refresh_usage(&id).await.unwrap();
        let stored = app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(stored.oauth, other);
        assert_eq!(stored.alternates, [oauth]);
        assert!(stored.usage.is_some());
        assert_eq!(usages(&fake), ["bob-access"]);
    }

    #[tokio::test]
    async fn a_rejected_login_read_during_a_refresh_is_not_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let mut pool = app.claude_load().unwrap();
        let id = pool.accounts[0].id.clone();
        let valid = pool.accounts[0].oauth.clone();
        pool.accounts[0].reject(print("stale-refresh"));
        app.claude_save(&pool).unwrap();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        // Claude Code's login turns back to a generation Claude rejected.
        fake.usage_hooks
            .lock()
            .unwrap()
            .push(external_update(location.clone(), "alice", "stale"));
        app.claude_refresh_usage(&id).await.unwrap();
        let stored = app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(stored.oauth, valid);
        assert!(stored.alternates.is_empty());
        assert!(!stored.in_live);
        assert_eq!(usages(&fake), ["alice-access"]);
        // Nothing is written over a login that changed during the request.
        assert_eq!(live_access(&location), "stale");
        // Later reads keep ignoring it.
        app.claude_accounts().await.unwrap();
        assert_eq!(app.claude_load().unwrap().accounts[0].oauth, valid);
    }

    #[tokio::test]
    async fn a_login_rejected_in_this_refresh_is_not_adopted_when_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let mut pool = app.claude_load().unwrap();
        let id = pool.accounts[0].id.clone();
        let mut renewed = pool.accounts[0].oauth.clone();
        renewed["accessToken"] = "pool-access".into();
        renewed["refreshToken"] = "pool-refresh".into();
        pool.accounts[0].alternates = vec![renewed.clone()];
        app.claude_save(&pool).unwrap();
        app.claude_release_new_accounts(Some(&id)).await.unwrap();
        // Claude refuses the login; while it does, Claude Code rewrites the
        // same rejected generation with another expiry.
        reject_next(&fake);
        fake.refresh_hooks.lock().unwrap().push(Box::new({
            let location = location.clone();
            move || sign_in(&location, "alice", LATER + 5)
        }));
        app.claude_refresh_usage(&id).await.unwrap();
        let stored = app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(stored.oauth, renewed);
        assert_eq!(stored.rejected, [print("alice-refresh")]);
        assert_eq!(refreshes(&fake), ["alice-refresh"]);
        assert_eq!(usages(&fake), ["alice-access", "pool-access"]);
    }

    #[tokio::test]
    async fn unrelated_failures_neither_reject_nor_fall_back() {
        for (status, error) in [
            (None, "Claude token refresh failed: HTTP 500"),
            (None, TOKEN_REFRESH_RATE_LIMITED),
            (Some(500), ""),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (app, _, fake) = waiting_app(&dir);
            let (oauth, account) = login("bob", LATER);
            let export = |oauth: &Value| {
                json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string()
            };
            app.claude_import_json(&export(&oauth)).await.unwrap();
            let id = app.claude_load().unwrap().accounts[0].id.clone();
            app.claude_release_new_accounts(Some(&id)).await.unwrap();
            let (mut other, _) = login("bob", LATER);
            other["accessToken"] = "bob-other".into();
            other["refreshToken"] = "bob-other-refresh".into();
            fake.usage
                .lock()
                .unwrap()
                .push((status.unwrap_or(401), Value::Null));
            if status.is_none() {
                *fake.refresh_error.lock().unwrap() = Some(error.into());
            }
            app.claude_import_json(&export(&other)).await.unwrap();
            let stored = app.claude_load().unwrap().accounts[0].clone();
            assert_eq!(stored.oauth, other, "{error}");
            assert_eq!(stored.alternates, [oauth.clone()], "{error}");
            assert!(stored.rejected.is_empty(), "{error}");
            assert!(stored.usage_error.is_some(), "{error}");
            assert_eq!(usages(&fake), ["bob-other"], "{error}");
        }
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
            // Our renewal stays to fall back on; a rejected token does not.
            let account = &stored.accounts[0];
            if failed {
                assert!(account.alternates.is_empty());
                assert_eq!(account.rejected, [print("alice-refresh")]);
            } else {
                assert_eq!(refresh_tokens(&account.alternates), ["fresh-refresh"]);
                assert!(account.rejected.is_empty());
            }
            assert!(account.in_live);
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
        let mut pool = app.claude_load().unwrap();
        let probe = pool
            .accounts
            .iter_mut()
            .find(|a| a.oauth["accessToken"] == "probe-access")
            .unwrap();
        probe.alternates = vec![json!({"accessToken": "old", "refreshToken": "probe-old"})];
        probe.rejected = vec![print("probe-rejected")];
        app.claude_save(&pool).unwrap();
        app.claude_release_new_accounts(None).await.unwrap();
        app.claude_refresh(None).await.unwrap();
        let accounts = app.claude_accounts().await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].label, "new@example.com");
        assert!(accounts[0].is_current);
        // Switching to it later names the account in Claude Code again.
        let stored = app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(stored.oauth["accessToken"], "probe-access");
        // The sign-in Claude Code never held stays to fall back on, with what
        // the folded card knew.
        assert_eq!(
            refresh_tokens(&stored.alternates),
            ["browser-refresh", "probe-old"]
        );
        assert_eq!(stored.rejected, [print("probe-rejected")]);
        assert!(stored.in_live);
        assert!(refreshes(&fake).is_empty());
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

    /// Resolves the location for `vars` under a home holding `settings` as
    /// the user settings.
    fn resolved(vars: &[(&str, &str)], settings: Option<&str>) -> (Location, PathBuf) {
        let home = tempfile::tempdir().unwrap().keep();
        if let Some(settings) = settings {
            fs::create_dir_all(home.join(".claude")).unwrap();
            fs::write(home.join(".claude/settings.json"), settings).unwrap();
        }
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let env = move |name: &str| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        (Location::resolve(&env, home.clone(), true), home)
    }
    const CUSTOM: &str = "http://127.0.0.1:8787/anthropic";

    #[test]
    fn the_login_is_where_claude_code_keeps_it() {
        let (plain, home) = resolved(&[], None);
        assert_eq!(plain.keychain.as_deref(), Some("Claude Code-credentials"));
        assert_eq!(plain.config_dir, home.join(".claude"));
        assert_eq!(plain.storage_dir, home.join(".claude"));
        assert_eq!(plain.global_config, home.join(".claude.json"));
        assert_eq!(
            plain.credentials_file(),
            home.join(".claude/.credentials.json")
        );
        // An approved custom OAuth endpoint, from the settings or the
        // environment, names its own item; the global configuration stays.
        let approved = "https://claude.fedstart.com";
        let in_settings =
            |url: &str| format!(r#"{{"env":{{"CLAUDE_CODE_CUSTOM_OAUTH_URL":"{url}"}}}}"#);
        for (vars, settings) in [
            (vec![("CLAUDE_CODE_CUSTOM_OAUTH_URL", approved)], None),
            (vec![], Some(in_settings(approved))),
            (
                vec![],
                Some(in_settings("https://claude-staging.fedstart.com/")),
            ),
        ] {
            let (custom, home) = resolved(&vars, settings.as_deref());
            assert_eq!(
                custom.keychain.as_deref(),
                Some("Claude Code-custom-oauth-credentials")
            );
            assert_eq!(custom.global_config, home.join(".claude.json"));
            assert_eq!(
                custom.credentials_file(),
                home.join(".claude/.credentials.json")
            );
        }
        // Any other endpoint leaves Claude Code its file alone.
        for (vars, settings) in [
            (vec![], Some(in_settings(CUSTOM))),
            (vec![("CLAUDE_CODE_CUSTOM_OAUTH_URL", CUSTOM)], None),
            (
                vec![("CLAUDE_CODE_CUSTOM_OAUTH_URL", approved)],
                Some(in_settings(CUSTOM)),
            ),
        ] {
            let (file_only, home) = resolved(&vars, settings.as_deref());
            assert_eq!(file_only.keychain, None);
            assert_eq!(
                file_only.credentials_file(),
                home.join(".claude/.credentials.json")
            );
        }
        // The settings override the environment, an empty value included;
        // other settings or unreadable ones change nothing.
        for (vars, settings) in [
            (
                vec![("CLAUDE_CODE_CUSTOM_OAUTH_URL", CUSTOM)],
                in_settings(""),
            ),
            (
                vec![],
                r#"{"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:8787/anthropic"}}"#.to_owned(),
            ),
            (
                vec![],
                r#"{"env":{"CLAUDE_CODE_CUSTOM_OAUTH_URL":7}}"#.to_owned(),
            ),
            (vec![], "not json".to_owned()),
        ] {
            let (location, _) = resolved(&vars, Some(&settings));
            assert_eq!(
                location.keychain.as_deref(),
                Some("Claude Code-credentials"),
                "{settings}"
            );
        }
        let (empty, _) = resolved(&[("CLAUDE_CODE_CUSTOM_OAUTH_URL", "")], None);
        assert_eq!(empty.keychain.as_deref(), Some("Claude Code-credentials"));
    }

    #[test]
    fn the_keychain_hash_and_file_follow_the_configuration_and_storage_directories() {
        let dir = "/work/claude-config";
        let (custom_dir, _) = resolved(&[("CLAUDE_CONFIG_DIR", dir)], None);
        assert_eq!(
            custom_dir.keychain.as_deref(),
            Some("Claude Code-credentials-619df0a7")
        );
        assert_eq!(custom_dir.config_dir, PathBuf::from(dir));
        assert_eq!(custom_dir.storage_dir, PathBuf::from(dir));
        assert_eq!(
            custom_dir.global_config,
            PathBuf::from(dir).join(".claude.json")
        );
        let (both, _) = resolved(
            &[
                ("CLAUDE_CONFIG_DIR", dir),
                (
                    "CLAUDE_CODE_CUSTOM_OAUTH_URL",
                    "https://claude.fedstart.com",
                ),
            ],
            None,
        );
        assert_eq!(
            both.keychain.as_deref(),
            Some("Claude Code-custom-oauth-credentials-619df0a7")
        );
        // The secure storage directory, when set at all, replaces both.
        let (storage, _) = resolved(
            &[
                ("CLAUDE_CONFIG_DIR", dir),
                ("CLAUDE_SECURESTORAGE_CONFIG_DIR", "/vault/store"),
            ],
            None,
        );
        assert_eq!(
            storage.keychain.as_deref(),
            Some("Claude Code-credentials-efec3556")
        );
        assert_eq!(storage.storage_dir, PathBuf::from("/vault/store"));
        assert_eq!(storage.config_dir, PathBuf::from(dir));
        let (unscoped, home) = resolved(
            &[
                ("CLAUDE_CONFIG_DIR", dir),
                ("CLAUDE_SECURESTORAGE_CONFIG_DIR", ""),
            ],
            None,
        );
        assert_eq!(
            unscoped.keychain.as_deref(),
            Some("Claude Code-credentials")
        );
        assert_eq!(unscoped.storage_dir, home.join(".claude"));
        // Off macOS there is only the file; smoke tests stay in one directory.
        let home = tempfile::tempdir().unwrap();
        let env = |name: &str| {
            (name == "CLAUDE_SECURESTORAGE_CONFIG_DIR").then(|| "/vault/store".to_owned())
        };
        let linux = Location::resolve(&env, home.path().into(), false);
        assert_eq!(linux.keychain, None);
        assert_eq!(
            linux.credentials_file(),
            PathBuf::from("/vault/store/.credentials.json")
        );
        let (smoke, _) = resolved(&[("COPOOL_CLAUDE_CONFIG_DIR", "/smoke")], None);
        assert_eq!(smoke, Location::files("/smoke".into()));
    }

    #[tokio::test]
    async fn a_login_read_from_the_file_is_written_back_to_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path().join("storage");
        let env = {
            let storage = storage.to_string_lossy().into_owned();
            move |name: &str| (name == "CLAUDE_SECURESTORAGE_CONFIG_DIR").then(|| storage.clone())
        };
        // Without the Keychain, as off macOS, the login is the storage file.
        let location = Location::resolve(&env, dir.path().into(), false);
        let (oauth, _) = login("alice", LATER);
        fs::create_dir_all(&storage).unwrap();
        write_json(
            &storage.join(".credentials.json"),
            &json!({"claudeAiOauth": oauth}),
        )
        .unwrap();
        let live = read_live(&location).await.unwrap().unwrap();
        assert_eq!(live.held, Held::File);
        assert_eq!(live.oauth(), Some(&oauth));
        // Where the Keychain item is missing, the file keeps receiving the
        // login; only a login read from the item, or none, goes to the item.
        let keychain = Location {
            keychain: Some("Claude Code-custom-oauth-credentials".into()),
            ..location
        };
        assert_eq!(written_keychain(&keychain, Some(Held::File)), None);
        for held in [Some(Held::Keychain), None] {
            assert_eq!(
                written_keychain(&keychain, held).map(String::as_str),
                Some("Claude Code-custom-oauth-credentials")
            );
        }
        assert_eq!(written_keychain(&Location::files(storage), None), None);
    }

    #[test]
    fn the_account_follows_a_claude_json_inside_the_configuration_directory() {
        let dir = tempfile::tempdir().unwrap();
        let location = Location {
            config_dir: dir.path().join(".claude"),
            global_config: dir.path().join(".claude.json"),
            keychain: None,
            storage_dir: dir.path().join(".claude"),
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
