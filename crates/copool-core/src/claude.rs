//! Claude Code account pool: the logins Copool keeps, the one Claude Code uses
//! (the macOS Keychain or `.credentials.json`, with `oauthAccount` in
//! `.claude.json`), OAuth usage and switching.
use crate::{
    account_network::Response,
    application::{Application, SwitchExecution},
    claude_api::{
        apply_tokens, expires_at, number, plan, refresh_at, refresh_scope, request_error, reset_at,
        text, usage_snapshot, UsageResponse, API_ROOT, SIGN_IN_EXPIRED, TOKEN_REFRESH_RATE_LIMITED,
        USAGE_USER_AGENT, USER_AGENT,
    },
    models::{AccountSummary, CreditSnapshot, Renewal, UsageSnapshot},
    persistence::write_json,
    pool::{Hello, Pool, PoolAccount},
    utils::now_unix_seconds,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, sync::Arc};

type Result<T> = std::result::Result<T, String>;

mod provider_config;
mod proxy;
mod refresh_lock;
mod settings_source;
mod sub2api;

const PROFILE_PATH: &str = "/api/oauth/profile";
const ORGANIZATIONS_PATH: &str = "/api/oauth/organizations";
/// The cheapest model whose hi starts the five-hour window: a one-token
/// Haiku hi, as Claude Code's own quota check, does not.
pub(crate) const HELLO_MODEL: &str = "claude-sonnet-5-5";
/// Claude Code 2.1.300 in a terminal session. A headless or background
/// entrypoint (`sdk-cli`, `cli-bg`) is reportedly counted apart from the
/// five-hour window.
const INTERACTIVE_USER_AGENT: &str = "claude-cli/2.1.300 (external, cli)";
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
    /// Copool keeps the pool's tokens in its own Keychain item: on macOS,
    /// wherever Claude Code keeps its login; never for an isolated directory.
    pub vault: bool,
    /// `CLAUDE_CODE_CUSTOM_OAUTH_URL` in Copool's environment, which the user
    /// settings' `env` overrides; none for an isolated directory.
    #[serde(skip)]
    pub custom_oauth_env: Option<String>,
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
        let custom_oauth_env = env(CUSTOM_OAUTH);
        let custom_oauth = custom_oauth_url(&config_dir, custom_oauth_env.as_deref());
        let approved = custom_oauth
            .as_deref()
            .is_none_or(|url| APPROVED_CUSTOM_OAUTH.contains(&url));
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
            vault: keychain,
            custom_oauth_env,
        }
    }
    /// Credentials and account in files of one directory, without the Keychain.
    pub fn files(dir: PathBuf) -> Self {
        Self {
            global_config: dir.join(".claude.json"),
            storage_dir: dir.clone(),
            config_dir: dir,
            keychain: None,
            vault: false,
            custom_oauth_env: None,
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

const CUSTOM_OAUTH: &str = "CLAUDE_CODE_CUSTOM_OAUTH_URL";
/// `CLAUDE_CODE_CUSTOM_OAUTH_URL` as Claude Code (2.1.300) reads it: the user
/// settings' `env` sets it over the environment, an empty value included;
/// empty is unset; one trailing slash is dropped.
fn custom_oauth_url(config_dir: &std::path::Path, env: Option<&str>) -> Option<String> {
    let settings = fs::read_to_string(config_dir.join("settings.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|settings| settings["env"][CUSTOM_OAUTH].as_str().map(str::to_owned));
    let url = settings.or_else(|| env.map(str::to_owned))?;
    let url = url.strip_suffix('/').unwrap_or(&url);
    (!url.is_empty()).then(|| url.to_owned())
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

// The pool's tokens are kept apart from `claude-accounts.json`. On macOS, as
// Claude Code keeps its login, they are in Copool's own Keychain item (see
// `crate::vault`); the store then holds everything but the tokens.
use crate::vault::{KeychainAccess, Vault};
/// Each account's tokens and alternate generations, by account id.
type Secrets = serde_json::Map<String, Value>;
const SECRET_KEYS: [&str; 2] = ["accessToken", "refreshToken"];
const VAULT_LOCKED: &str =
    "Copool cannot read the account's tokens from its Keychain item; refresh, or reopen Copool, and allow access";
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
/// Whether the pool's tokens go to Copool's Keychain item: not whether Claude
/// Code's own login does.
fn uses_keychain_vault(location: &Location) -> bool {
    cfg!(target_os = "macos") && location.vault
}
/// The accounts whose tokens were moved to the vault.
fn moved_ids(store: &Store) -> Vec<String> {
    store
        .accounts
        .iter()
        .filter(|a| !holds_tokens(a))
        .map(|a| a.id.clone())
        .collect()
}
fn vault_mismatch(secrets: &Secrets, moved: &[String], missing: bool) -> Option<String> {
    let absent = moved.iter().filter(|id| !secrets.contains_key(*id)).count();
    (absent > 0).then(|| {
        if missing {
            "the Keychain reports the item missing".into()
        } else {
            format!("the item lacks the tokens of {absent} account(s)")
        }
    })
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
    /// Why the vault could not be used last, shown with the error.
    vault_problem: std::sync::Mutex<Option<String>>,
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
            vault_problem: Default::default(),
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
            next_refresh_at: None,
        }
    }
}
impl PoolAccount for Stored {
    fn id(&self) -> &str {
        &self.id
    }
    fn set_renewal(&mut self, renewal: Renewal) {
        self.renewal = Some(renewal);
    }
    fn set_proxy(&mut self, proxy: String) {
        self.proxy_url = proxy;
    }
    fn release(&mut self) -> bool {
        std::mem::take(&mut self.first_request_pending)
    }
}

/// The login Claude Code uses.
#[derive(Clone)]
struct Live {
    /// The complete credentials document; other entries (MCP logins) are kept.
    document: Value,
    account: Option<Value>,
    held: Held,
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
        expiries: prepaid_credit_expiries(prepaid),
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
/// Paid and promotional tranches have independent expiries. Never infer
/// these from the monthly spending limit or the subscription renewal date.
fn prepaid_credit_expiries(prepaid: &Value) -> Vec<i64> {
    let mut expiries: Vec<i64> = ["tranches", "promo_tranches"]
        .into_iter()
        .flat_map(|key| prepaid[key].as_array().into_iter().flatten())
        .filter(|tranche| {
            number(&tranche["remaining_amount_minor_units"]).is_none_or(|left| left > 0.0)
        })
        .filter_map(|tranche| reset_at(&tranche["expires_at"]))
        .chain(reset_at(&prepaid["next_expires_at"]))
        .filter_map(crate::models::displayed_expiry)
        .collect();
    expiries.sort_unstable();
    expiries.dedup();
    expiries
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

#[cfg(target_os = "macos")]
pub(crate) mod keychain;

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
/// How long Copool waits while Claude Code renews its login.
const CLAUDE_CODE_LOCK_WAIT: std::time::Duration = if cfg!(test) {
    std::time::Duration::from_millis(600)
} else {
    std::time::Duration::from_secs(10)
};
const CLAUDE_CODE_RENEWING: &str = "Claude Code is renewing its login; retry usage";
const CLAUDE_CODE_LOCK_UNAVAILABLE: &str = "Cannot take Claude Code's login refresh lock";

/// Claude Code's refresh lock over the login kept in `location`. Claude Code
/// renews its login only while holding it and rereads the login inside, so a
/// renewal or switch made under it never spends a refresh token Claude Code
/// is spending, nor is written over by Claude Code's renewal. Without the
/// directory and a login kept there, Claude Code holds no lock either: it
/// creates the directory before locking.
async fn claude_code_lock(
    location: &Location,
    login_kept: bool,
) -> Result<Option<refresh_lock::RefreshLock>> {
    let dir = location.storage_dir.clone();
    tokio::task::spawn_blocking(move || {
        if !dir.is_dir() {
            if !login_kept {
                return Ok(None);
            }
            fs::create_dir_all(&dir).map_err(|_| CLAUDE_CODE_LOCK_UNAVAILABLE.to_owned())?;
        }
        match refresh_lock::RefreshLock::acquire(
            &dir,
            refresh_lock::Wait::up_to(CLAUDE_CODE_LOCK_WAIT),
        ) {
            Ok(Some(lock)) => Ok(Some(lock)),
            Ok(None) => Err(CLAUDE_CODE_RENEWING.to_owned()),
            Err(_) => Err(CLAUDE_CODE_LOCK_UNAVAILABLE.to_owned()),
        }
    })
    .await
    .map_err(|_| CLAUDE_CODE_LOCK_UNAVAILABLE.to_owned())?
}

/// Hands Claude Code's login another generation of the same login, under
/// its lock and only while it still holds `seen`. Members Copool does not
/// keep (`clientId`, MCP logins) stay as Claude Code wrote them.
async fn sync_live(location: &Location, seen: &Live, oauth: &Value) -> Result<()> {
    let _lock = claude_code_lock(location, true).await?;
    let current = read_live(location).await?.ok_or(CREDENTIALS_CHANGED)?;
    let unchanged = match (seen.oauth(), current.oauth()) {
        (Some(seen), Some(now)) => same_oauth_generation(seen, now),
        _ => false,
    };
    if !unchanged || current.held != seen.held {
        return Err(CREDENTIALS_CHANGED.into());
    }
    let mut document = current.document.clone();
    document["claudeAiOauth"] = merged_oauth(current.oauth(), oauth);
    write_credentials(
        location,
        &document,
        Some(current.held),
        KeychainAccess::Background,
    )
    .await
}

/// `oauth` over the members a live login keeps beside it.
fn merged_oauth(live: Option<&Value>, oauth: &Value) -> Value {
    let mut merged = live
        .filter(|live| live.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let (Some(target), Some(source)) = (merged.as_object_mut(), oauth.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    merged
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
pub(crate) fn write_text(path: &std::path::Path, text: &str) -> Result<()> {
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

fn live_generation_changed(before: Option<&Live>, after: Option<&Live>) -> bool {
    after.and_then(Live::oauth).is_some_and(|now| {
        before
            .and_then(Live::oauth)
            .is_none_or(|old| !same_oauth_generation(old, now))
    })
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

pub(crate) const CREDENTIALS_CHANGED: &str =
    "Claude credentials changed during refresh; retry usage";
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
        let mut store = self.claude_store_file()?;
        if let Some(vault) = self.claude_vault() {
            let moved = moved_ids(&store);
            if !moved.is_empty() {
                if let Some(secrets) =
                    self.claude_secrets(&*vault, KeychainAccess::Background, &moved)
                {
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
    /// The store file as it is, without the vault's tokens.
    fn claude_store_file(&self) -> Result<Store> {
        match fs::read(self.claude_store_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("Invalid Claude account store: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
            Err(e) => Err(e.to_string()),
        }
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
        let moved = moved_ids(store);
        let Some(known) = self.claude_secrets(vault, KeychainAccess::Background, &moved) else {
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
    /// Tokens moved there must be in it: an item reported missing or without
    /// them is not what Copool wrote (one it cannot see as it is, say), so it
    /// is neither cached nor ever written over.
    fn claude_secrets(
        &self,
        vault: &dyn Vault,
        access: KeychainAccess,
        moved: &[String],
    ) -> Option<Secrets> {
        let mut cache = self.claude.secrets.lock().unwrap();
        if cache.is_none() {
            let read = match vault.read(access) {
                Ok(read) => read,
                Err(error) => {
                    *self.claude.vault_problem.lock().unwrap() = Some(error);
                    return None;
                }
            };
            let missing = read.is_none();
            let Some(secrets) = parse_secrets(read) else {
                *self.claude.vault_problem.lock().unwrap() =
                    Some("its contents are not the pool's tokens".into());
                return None;
            };
            if let Some(problem) = vault_mismatch(&secrets, moved, missing) {
                *self.claude.vault_problem.lock().unwrap() = Some(problem);
                return None;
            }
            *cache = Some(secrets);
            *self.claude.vault_problem.lock().unwrap() = None;
        }
        let secrets = cache.clone()?;
        if let Some(problem) = vault_mismatch(&secrets, moved, false) {
            *cache = None;
            *self.claude.vault_problem.lock().unwrap() = Some(problem);
            return None;
        }
        Some(secrets)
    }
    /// The error for an account whose tokens cannot be read, with the reason.
    fn claude_locked(&self) -> String {
        match self.claude.vault_problem.lock().unwrap().as_deref() {
            Some(problem) => format!("{VAULT_LOCKED} ({problem})"),
            None => VAULT_LOCKED.into(),
        }
    }
    /// Lets a user action ask for the vault's Keychain item when background
    /// work could not read it, as after a build changed Copool's signature.
    pub(crate) async fn claude_authorize(&self) -> Result<()> {
        self.claude_unlock().await?;
        // The item holds the providers' passwords as well; read once already
        // when the pool's tokens are there.
        self.authorize_keychain().await
    }
    /// Reads the vault once, allowing the Keychain to ask the user, and
    /// moves tokens still in the store into it. Startup and user actions do
    /// this; background work never asks.
    async fn claude_unlock(&self) -> Result<()> {
        let Some(vault) = self.claude_vault() else {
            return Ok(());
        };
        if self.claude.secrets.lock().unwrap().is_none() {
            let moved = moved_ids(&self.claude_store_file()?);
            let read = tokio::task::spawn_blocking(move || vault.read(KeychainAccess::Interactive))
                .await
                .map_err(|e| e.to_string())?;
            let read = read.inspect_err(|error| {
                *self.claude.vault_problem.lock().unwrap() = Some(error.clone());
            })?;
            let missing = read.is_none();
            let secrets = parse_secrets(read).ok_or_else(|| {
                "Copool's Keychain item does not hold the pool's tokens".to_string()
            })?;
            if let Some(problem) = vault_mismatch(&secrets, &moved, missing) {
                *self.claude.vault_problem.lock().unwrap() = Some(problem.clone());
                return Err(problem);
            }
            let mut cache = self.claude.secrets.lock().unwrap();
            if cache.is_none() {
                *cache = Some(secrets);
                *self.claude.vault_problem.lock().unwrap() = None;
            }
        }
        let _lock = self.claude.lock.lock().await;
        let store = self.claude_load()?;
        self.claude_save(&store)
    }
    /// The pool's tokens go to Copool's Keychain item on macOS, whether
    /// Claude Code keeps its own login in the Keychain or in its file;
    /// elsewhere and for an isolated directory they stay in the store.
    fn claude_vault(&self) -> Option<Arc<dyn Vault>> {
        if let Some(vault) = self.claude.vault.read().unwrap().clone() {
            return Some(vault);
        }
        if uses_keychain_vault(&self.claude_location()) {
            let item = self.keychain.clone()?;
            return Some(Arc::new(crate::vault::Section::claude(item)));
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
            .chain(self.provider_cards(Pool::Claude, None)?)
            .collect())
    }
    /// The Claude pool, following the login Claude Code uses.
    pub(crate) async fn claude_accounts(&self) -> Result<Vec<AccountSummary>> {
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
                let mut card = a.summary(official && current);
                // Tokens out of reach show as the card's error, as a failed
                // refresh does.
                if !holds_tokens(a) {
                    card.usage_error = Some(self.claude_locked());
                }
                card
            })
            .chain(self.provider_cards(Pool::Claude, provider)?)
            .collect();
        crate::ranking::sort_for_display(&mut summaries);
        if let Some(schedule) = self.schedule(Pool::Claude) {
            for card in &mut summaries {
                // Every round asks for all cards, subject to freshness and
                // rate limit pauses for official accounts.
                card.next_refresh_at = match store.accounts.iter().find(|a| a.id == card.id) {
                    None => Some(schedule.next_refresh_at(true, 0)),
                    Some(account) if account.first_request_pending => None,
                    Some(account) => {
                        let due = account.updated_at + schedule.interval;
                        let not_before = due.max(account.usage_retry_at.unwrap_or_default());
                        Some(schedule.next_refresh_at(true, not_before))
                    }
                };
            }
        }
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
    pub(crate) async fn claude_import_current(&self) -> Result<String> {
        let _op = self.claude.operations.lock().await;
        let _ = self.claude_unlock().await;
        let live = read_live_with_access(&self.claude_location(), KeychainAccess::Interactive)
            .await?
            .ok_or("error.claude.login_not_found")?;
        let (id, label, waiting) = self.claude_import_live(live, true).await?;
        drop(_op);
        if !waiting {
            let _ = self.claude_refresh_usage_scoped(&id, None).await;
        }
        Ok(label)
    }
    /// Imports an exported Claude Code credentials JSON without selecting it.
    pub(crate) async fn claude_import_json(&self, content: &str) -> Result<usize> {
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
            let _ = self.claude_refresh_usage_scoped(&id, None).await;
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
                    // A new account has no proxy of its own yet: the shared one.
                    let proxy = self.account_proxy("")?;
                    match self
                        .claude_network()
                        .profile(&self.claude_api_root(), &token, &proxy)
                        .await
                    {
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
    pub(crate) async fn claude_import_login(&self, tokens: Value) -> Result<String> {
        let mut oauth = json!({});
        apply_tokens(&mut oauth, &tokens, now_unix_seconds() * 1000)?;
        let token = text(&oauth["accessToken"]).unwrap_or_default();
        let account = profile_account(None, &tokens).0;
        let known = self.claude_known(&oauth, Some(&account))?;
        // The token response names the account; the profile adds its plan,
        // later on the first refresh for an account that waits.
        let profile = if self.claude_may_contact(known.as_ref())? {
            // Its own proxy, never another account's; else the shared one.
            let proxy =
                self.account_proxy(&known.map(|known| known.proxy_url).unwrap_or_default())?;
            self.claude_network()
                .profile(&self.claude_api_root(), &token, &proxy)
                .await
                .ok()
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
            let _ = self.claude_refresh_usage_scoped(&id, None).await;
        }
        Ok(label)
    }
    pub(crate) async fn claude_delete_account(&self, id: &str) -> Result<()> {
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
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
    /// Edits the stored accounts; their tokens stay where they are kept.
    pub(crate) async fn claude_edit_accounts(
        &self,
        edit: impl FnOnce(&mut [&mut dyn PoolAccount]) -> Result<bool>,
    ) -> Result<()> {
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        let changed = {
            let mut accounts: Vec<&mut dyn PoolAccount> = store
                .accounts
                .iter_mut()
                .map(|a| a as &mut dyn PoolAccount)
                .collect();
            edit(&mut accounts)?
        };
        if changed {
            self.claude_save(&store)?;
        }
        Ok(())
    }
    /// Gives Claude Code this account's login and identity, or a Sub2api
    /// card's provider. A previous login outside the pool is kept in
    /// `claude-before-switch.json`; a failed step is rolled back.
    pub(crate) async fn claude_switch_account(
        &self,
        id: &str,
        access: KeychainAccess,
    ) -> Result<SwitchExecution> {
        let _op = self.claude.operations.lock().await;
        if crate::sub2api::is_card(Pool::Claude, id) {
            return self.claude_switch_sub2api(id).await;
        }
        if access == KeychainAccess::Interactive {
            let _ = self.claude_unlock().await;
        }
        // Keep tokens Claude Code refreshed for the account being left.
        self.claude_accounts().await?;
        let location = self.claude_location();
        let _lock = self.claude.lock.lock().await;
        // A renewal Claude Code finishes after the switch would write the
        // login being left over it: switch only while it renews nothing, and
        // read the login being left under the same lock.
        let _claude_code = claude_code_lock(&location, true).await?;
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
            return Err(self.claude_locked());
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
    /// Resolve a nameless live rotation by its own access token. Until its profile
    /// confirms the identity, never use a particular card's proxy or its login.
    async fn identify_live_rotation(
        &self,
        location: &Location,
        live: &mut Option<Live>,
        account: &Stored,
    ) -> Result<()> {
        let Some(candidate) = live.as_ref().filter(|v| v.account.is_none()) else {
            return Ok(());
        };
        let Some(oauth) = candidate.oauth() else {
            return Ok(());
        };
        if identity(&account.account).is_none()
            || account.first_request_pending
            || !holds_tokens(account)
            || same_login(oauth, &account.oauth)
            || account.is_rejected(oauth)
        {
            return Ok(());
        }
        if self.claude_known(oauth, None)?.is_some() {
            return Ok(());
        }
        let Some(token) = text(&oauth["accessToken"]) else {
            return Ok(());
        };
        let proxy = self.account_proxy("")?;
        let Ok(profile) = self
            .claude_network()
            .profile(&self.claude_api_root(), &token, &proxy)
            .await
        else {
            return Ok(());
        };
        let named = profile_account(Some(&profile), &Value::Null).0;
        let mut latest = read_live(location).await.ok().flatten();
        if let Some(current) = latest.as_mut() {
            if current.account.is_none()
                && current
                    .oauth()
                    .is_some_and(|v| same_oauth_generation(oauth, v))
                && identity(&named) == identity(&account.account)
            {
                current.account = Some(named);
            }
        }
        *live = latest;
        Ok(())
    }
    /// Renews pool credentials and syncs an unchanged live login without
    /// showing authorization UI; external rotations take precedence over
    /// the response. A new import asks without the shared account proxy.
    /// With `renew_now`, only the tokens are renewed, even when unexpired or
    /// renewal is off; usage is not asked for, nor shown as refreshing, and
    /// whether the renewal was kept is reported.
    pub(crate) async fn claude_refresh_usage_scoped(
        &self,
        id: &str,
        mut renew_now: Option<&mut bool>,
    ) -> Result<()> {
        let forced = renew_now.is_some();
        let _activity = (!forced).then(|| self.begin_refresh(id));
        // Serialize renewal with imports/switches and other pool refreshes.
        let _op = self.claude.operations.lock().await;
        let location = self.claude_location();
        // Adopt newer external credentials when accessible, but the pool remains
        // usable even when the live login is locked, missing or unreadable.
        let mut live = read_live(&location).await.ok().flatten();
        if live.as_ref().is_some_and(|v| v.account.is_none()) {
            let saved = self.claude_load()?;
            if let Some(target) = saved.accounts.iter().find(|a| {
                a.id == id
                    && (forced
                        || a.usage_retry_at
                            .is_none_or(|until| until <= now_unix_seconds()))
            }) {
                self.identify_live_rotation(&location, &mut live, target)
                    .await?;
            }
        }
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
            return Err(self.claude_locked());
        }
        // Renewal off: an expired account is reported, never renewed, and
        // nothing is written back to Claude Code.
        let held_back =
            (!forced && Renewal::of(account.renewal) == Renewal::Never).then_some(RENEWAL_OFF);
        // A usage pause does not hold back a renewal alone.
        if !forced
            && account
                .usage_retry_at
                .is_some_and(|until| until > now_unix_seconds())
        {
            return Err(account.usage_error.clone().unwrap_or_else(|| {
                "Claude usage request failed: HTTP 429; retry paused for 5 minutes".into()
            }));
        }
        let network = self.claude_network();
        let api = self.claude_api_root();
        let api = api.as_str();
        let request_proxy = self.account_proxy(&account.proxy_url)?;
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
        // Claude Code's login as renewed by this round, and why it could not be.
        let written: std::sync::Mutex<Option<Live>> = Default::default();
        let unwritten: std::sync::Mutex<Option<String>> = Default::default();
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
                    let (written, unwritten) = (&written, &unwritten);
                    let before = live.as_ref();
                    async move {
                        // A renewal earlier in this round wrote Claude Code's login.
                        let ours = written.lock().unwrap().clone();
                        let before = ours.as_ref().or(before);
                        // Another app may have renewed while our usage request ran.
                        let mut latest = match read_live(location).await {
                            Ok(latest) => latest,
                            Err(_) if in_use => return Err(LIVE_UNREADABLE.into()),
                            Err(_) => None,
                        };
                        if live_generation_changed(before, latest.as_ref()) {
                            self.identify_live_rotation(location, &mut latest, account)
                                .await?;
                        }
                        if changed_live_oauth(account, rejected, before, latest.as_ref()).is_some()
                        {
                            return Err(CREDENTIALS_CHANGED.into());
                        }
                        if let Some(reason) = held_back {
                            return Err(reason.into());
                        }
                        let token = token.ok_or(REFRESH_REJECTED)?;
                        // Claude Code renews only under its lock, rereading its
                        // login there: hold the lock from our reread until the
                        // renewal reaches that login, so neither spends a refresh
                        // token the other is spending.
                        let lock = claude_code_lock(location, latest.is_some()).await?;
                        let locked = match read_live(location).await {
                            Ok(locked) => locked,
                            Err(_) if in_use => return Err(LIVE_UNREADABLE.into()),
                            Err(_) => None,
                        };
                        // A login named as another account, before and after
                        // the wait, never held this token: its renewal is no
                        // reason to stop.
                        let another = |live: Option<&Live>| {
                            let named = live.and_then(|l| l.account.as_ref()).and_then(identity);
                            identity(&account.account)
                                .is_some_and(|ours| named.is_some_and(|named| named != ours))
                        };
                        if live_generation_changed(latest.as_ref(), locked.as_ref())
                            && !(another(latest.as_ref()) && another(locked.as_ref()))
                        {
                            // Renewed or switched while we waited; the round
                            // adopts or leaves that login.
                            return Err(CREDENTIALS_CHANGED.into());
                        }
                        // Taken over as a dead holder's: another program may
                        // be renewing now.
                        if lock.as_ref().is_some_and(|lock| lock.compromised()) {
                            return Err(CLAUDE_CODE_RENEWING.into());
                        }
                        let tokens = match network.refresh(&token, &scope, proxy).await {
                            Err(error) if error == SIGN_IN_EXPIRED => {
                                return Err(REFRESH_REJECTED.into())
                            }
                            response => response?,
                        };
                        // Claude Code's login still holding the token just spent
                        // gets the renewal now, merged into what else it keeps;
                        // left until after the usage request, it would hold a
                        // spent token Claude Code could only fail to renew. A login
                        // changed meanwhile is someone else's and left alone.
                        // One time for both copies, so they stay one generation.
                        let at = now_unix_seconds() * 1000;
                        if let Ok(Some(current)) = read_live(location).await {
                            let spent = current
                                .oauth()
                                .filter(|o| text(&o["refreshToken"]).as_deref() == Some(&*token));
                            if let Some(mut renewed) = spent.cloned() {
                                apply_tokens(&mut renewed, &tokens, at)?;
                                let mut document = current.document.clone();
                                document["claudeAiOauth"] = renewed;
                                match write_credentials(
                                    location,
                                    &document,
                                    Some(current.held),
                                    KeychainAccess::Background,
                                )
                                .await
                                {
                                    Ok(()) => {
                                        *written.lock().unwrap() = Some(Live {
                                            document,
                                            ..current
                                        })
                                    }
                                    Err(error) => *unwritten.lock().unwrap() = Some(error),
                                }
                            }
                        }
                        Ok::<_, String>((tokens, at))
                    }
                };
                if (forced && renewals == 0)
                    || (expires_at(&oauth) != 0
                        && expires_at(&oauth) <= (now_unix_seconds() + 60) * 1000)
                {
                    let (tokens, at) = refresh(&oauth).await?;
                    apply_tokens(&mut oauth, &tokens, at)?;
                    renewed = true;
                    renewals += 1;
                }
                // An account imported without its name is probed first, through
                // its own proxy.
                if identity(&account.account).is_none() {
                    let token = text(&oauth["accessToken"]).unwrap_or_default();
                    let found = match network.profile(api, &token, proxy).await {
                        Err(error) if error == SIGN_IN_EXPIRED => {
                            let (tokens, at) = refresh(&oauth).await?;
                            apply_tokens(&mut oauth, &tokens, at)?;
                            renewed = true;
                            renewals += 1;
                            let token = text(&oauth["accessToken"]).unwrap_or_default();
                            network.profile(api, &token, proxy).await
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
                if forced {
                    return Ok(None);
                }
                let token = text(&oauth["accessToken"]).unwrap_or_default();
                let (mut status, mut body, mut after, mut clock_offset) =
                    self.claude_usage(&*network, api, &token, proxy).await?;
                if status == 401 {
                    let (tokens, at) = refresh(&oauth).await?;
                    apply_tokens(&mut oauth, &tokens, at)?;
                    renewed = true;
                    renewals += 1;
                    let token = text(&oauth["accessToken"]).unwrap_or_default();
                    (status, body, after, clock_offset) =
                        self.claude_usage(&*network, api, &token, proxy).await?;
                }
                retry_after = after.filter(|_| status == 429);
                // An account added without its profile learns its plan here,
                // through its own proxy.
                if (200..=299).contains(&status)
                    && profiled.is_none()
                    && text(&oauth["subscriptionType"]).is_none()
                {
                    let token = text(&oauth["accessToken"]).unwrap_or_default();
                    if let Ok(found) = network.profile(api, &token, proxy).await {
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
                        for window in [&mut usage.five_hour, &mut usage.one_week]
                            .into_iter()
                            .flatten()
                        {
                            window.clock_offset_seconds = clock_offset;
                        }
                        // Extra usage is paid from the organization's prepaid
                        // balance, asked for through the same proxy.
                        let org = text(
                            &profiled.as_ref().unwrap_or(&account.account)["organizationUuid"],
                        );
                        if let (true, Some(org)) = (body["extra_usage"].is_object(), org) {
                            let token = text(&oauth["accessToken"]).unwrap_or_default();
                            if let Ok(prepaid) = network.prepaid(api, &token, &org, proxy).await {
                                usage.credits = extra_usage_credits(&body["extra_usage"], &prepaid);
                            }
                        }
                        Ok(Some(usage))
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
            // What this round wrote is now the login read before it.
            if let Some(ours) = written.lock().unwrap().take() {
                live = Some(ours);
            }
            if let Some(error) = unwritten.lock().unwrap().take() {
                live_sync = Err(error);
            }
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
            let mut latest = read_live(&location).await.ok().flatten();
            if live_generation_changed(live.as_ref(), latest.as_ref()) {
                self.identify_live_rotation(&location, &mut latest, &account)
                    .await?;
            }
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
                        None if before.account.as_ref().and_then(identity)
                            == identity(&account.account)
                            && identity(&account.account).is_some()
                            && same_oauth_generation(old, current) =>
                        {
                            true
                        }
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
                        && !same_login(&oauth, current)
                        && (result.is_ok() || renewed)
                    {
                        live_sync = sync_live(&location, latest, &oauth).await;
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
        if let Some(reported) = renew_now.as_deref_mut() {
            *reported = renewed;
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
            (false, Ok(Some(_))) => 0,
            (false, _) => account.rate_limits,
        };
        let pause = rate_limited.then(|| rate_limit_pause(rate_limits, retry_after));
        let mut result = result.map_err(|error| match pause {
            Some(pause) => format!("{error}; retry paused for {} minutes", (pause + 59) / 60),
            None => error,
        });
        if let Some(target) = store.accounts.iter_mut().find(|a| a.id == id) {
            target.updated_at = now_unix_seconds();
            // A renewal alone keeps the usage pause it found.
            if !forced || pause.is_some() {
                target.usage_retry_at = pause.map(|pause| now_unix_seconds() + pause);
            }
            target.rate_limits = rate_limits;
            match &mut result {
                Ok(Some(usage)) => {
                    crate::claude_api::retain_known_resets(usage, target.usage.as_ref());
                    target.usage = Some(usage.clone());
                    target.usage_error = live_sync.as_ref().err().cloned();
                }
                // A renewal alone leaves the usage as it was.
                Ok(None) => {
                    if let Err(error) = &live_sync {
                        target.usage_error = Some(error.clone());
                    }
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
            if renewals > 0 {
                target.renewal = target.renewal.map(Renewal::after_renewal);
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
    /// A 429 retries with the alternate agent for this request only.
    async fn claude_usage(
        &self,
        network: &dyn Network,
        api: &str,
        token: &str,
        proxy: &str,
    ) -> Result<UsageResponse> {
        let response = network.usage(api, token, USAGE_USER_AGENT, proxy).await?;
        if response.0 == 429 {
            network.usage(api, token, USER_AGENT, proxy).await
        } else {
            Ok(response)
        }
    }
    /// Where Claude Code sends a login's OAuth API requests (usage, profile,
    /// prepaid credits): `CLAUDE_CODE_CUSTOM_OAUTH_URL`, its settings read
    /// again each time, else the API. Claude Code approves only its own
    /// endpoints there; any is accepted here, a relay say. Token renewal and
    /// sign-in stay with the API.
    fn claude_api_root(&self) -> String {
        let location = self.claude_location();
        custom_oauth_url(&location.config_dir, location.custom_oauth_env.as_deref())
            .unwrap_or_else(|| API_ROOT.into())
    }
    /// Where Claude Code sends a login's messages: the base URL of its
    /// official settings, a relay that forwards the login say, else the API.
    /// While a provider is applied, the official settings are those recorded
    /// on leaving them; a base URL beside a credential that replaces the
    /// login is that credential's gateway.
    fn claude_messages_url(&self) -> String {
        format!("{}/v1/messages", self.claude_messages_base())
    }
    /// The root of `claude_messages_url`.
    pub(crate) fn claude_messages_base(&self) -> String {
        let applied = self
            .claude_load()
            .is_ok_and(|store| store.settings_provider.is_some());
        let official = if applied {
            self.claude_official_settings()
                .ok()
                .flatten()
                .and_then(|official| official.settings)
        } else {
            read_settings(&self.claude_location()).ok().flatten()
        };
        let base = parse_settings(official.as_deref())
            .ok()
            .filter(|settings| provider_config::overrides(settings).is_empty())
            .and_then(|settings| text(&settings["env"][BASE_URL]))
            .unwrap_or_else(|| API_ROOT.into());
        base.trim_end_matches('/').to_owned()
    }
    /// Whether the stored account failed: its usage refused, or its token
    /// expired.
    pub(crate) async fn claude_login_failed(&self, id: &str) -> Result<bool> {
        let _lock = self.claude.lock.lock().await;
        let store = self.claude_load()?;
        let account = store
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?;
        let expiry = expires_at(&account.oauth);
        Ok(account.usage_error.is_some() || (expiry != 0 && expiry <= now_unix_seconds() * 1000))
    }
    /// One hi with an account, through its proxy, without switching to it.
    pub(crate) async fn claude_hello(&self, id: &str) -> Result<Hello> {
        let account = {
            let _lock = self.claude.lock.lock().await;
            self.claude_load()?
                .accounts
                .into_iter()
                .find(|a| a.id == id)
                .ok_or("Account not found")?
        };
        if !holds_tokens(&account) {
            return Err(self.claude_locked());
        }
        let expired = expires_at(&account.oauth) != 0
            && expires_at(&account.oauth) <= (now_unix_seconds() + 60) * 1000;
        let (status, body) = if expired {
            (401, Value::Null)
        } else {
            let token = text(&account.oauth["accessToken"]).unwrap_or_default();
            let proxy = self.account_proxy(&account.proxy_url)?;
            let uuid = text(&account.account["accountUuid"]).unwrap_or_default();
            self.claude_network()
                .hello(&self.claude_messages_url(), &token, &uuid, &proxy)
                .await?
        };
        match status {
            200..=299 => Ok(Hello::Said),
            401 => Ok(Hello::Expired(SIGN_IN_EXPIRED.into())),
            _ => {
                let message = body["error"]["message"].as_str().unwrap_or_default();
                Err(
                    format!("Claude message request failed: HTTP {status} {message}")
                        .trim()
                        .to_owned(),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::SmartSwitchOutcome;
    use crate::claude_api::*;
    use crate::models::ResetKind;
    use crate::Preferences;
    use std::sync::Mutex;

    #[test]
    fn live_reset_shape_uses_refund_flag_even_when_both_types_say_full() {
        // Read-only OAuth usage response, 2026-11-11. IDs and labels omitted.
        let body = json!({"cedar_ember": {"eligible": true, "grants": [
            {"type":"full", "resets_left":1, "ends_at":"2026-11-17T07:59:00+00:00",
             "gives_back_weekly":true, "clears":["five_hour","seven_day_overage_included"]},
            {"type":"full", "resets_left":1, "ends_at":"2026-12-10T22:00:00+00:00",
             "gives_back_weekly":false, "clears":["five_hour","seven_day","seven_day_overage_included"]}
        ]}});
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-11T00:00:00Z")
            .unwrap()
            .timestamp();
        let usage = usage_snapshot(&body, None, now).unwrap();
        assert_eq!(usage.remaining_reset_count, Some(2));
        assert_eq!(usage.reset_grants[0].kind, ResetKind::Fable);
        assert_eq!(usage.reset_grants[1].kind, ResetKind::Full);
    }

    #[test]
    fn retained_resets_expire_without_advancing_the_confirmation_time() {
        let body = json!({"cedar_ember":{"eligible":true,"grants":[
            {"resets_left":2,"ends_at":20}, {"resets_left":1,"ends_at":30}
        ]}});
        let first = usage_snapshot(&body, None, 10).unwrap();
        let unknown = |now| usage_snapshot(&json!({}), None, now).unwrap();
        for legacy in [false, true] {
            let mut previous = first.clone();
            if legacy {
                previous.reset_grants.clear();
            }
            for (now, count) in [(20, 1), (25, 1), (30, 0), (40, 0)] {
                let mut next = unknown(now);
                crate::claude_api::retain_known_resets(&mut next, Some(&previous));
                assert_eq!(next.remaining_reset_count, Some(count));
                assert_eq!(next.resets_last_confirmed_at, Some(10));
                assert!(next.reset_expiries.iter().all(|at| *at > now));
                previous = next;
            }
        }
        let mut next = unknown(30);
        crate::claude_api::retain_known_resets(&mut next, Some(&unknown(10)));
        assert_eq!(next.remaining_reset_count, None);
        assert_eq!(next.resets_last_confirmed_at, None);
    }

    #[test]
    fn reset_inventory_distinguishes_weekly_refunds_from_full_resets() {
        let grant = |left, ends, clears: Value, refund| {
            json!({
                "resets_left": left, "ends_at": ends, "clears": clears,
                "gives_back_weekly": refund
            })
        };
        let body = json!({"cedar_ember": {"eligible": true, "grants": [
            grant(1, 300, json!(["five_hour", "seven_day"]), false),
            grant(2, 400, json!(["five_hour", "seven_day_overage_included"]), true),
            grant(1, 500, json!(["five_hour"]), false),
            grant(1, 600, Value::Null, false),
            grant(3, 100, json!(["five_hour"]), true),
            grant(0, 700, json!(["five_hour"]), true)
        ]}});
        let usage = usage_snapshot(&body, None, 100).unwrap();
        assert_eq!(usage.remaining_reset_count, Some(5));
        assert_eq!(usage.reset_expiries, [300, 400, 400, 500, 600]);
        assert_eq!(
            usage
                .reset_grants
                .iter()
                .map(|g| g.kind)
                .collect::<Vec<_>>(),
            [
                ResetKind::Full,
                ResetKind::Fable,
                ResetKind::Partial,
                ResetKind::Unknown
            ]
        );
        assert_eq!(usage.reset_grants[1].count, 2);
        let serialized = serde_json::to_value(&usage).unwrap();
        assert_eq!(serialized["resetGrants"][1]["kind"], "fable");
        assert_eq!(serialized["resetGrants"][1]["expiresAt"], 400);
        let mut old = serialized;
        old.as_object_mut().unwrap().remove("resetGrants");
        assert!(serde_json::from_value::<UsageSnapshot>(old)
            .unwrap()
            .reset_grants
            .is_empty());
        // A future grant field must never override the weekly-refund flag.
        let refund = json!({"eligible":true,"grants":[
            grant(1, 300, json!(["five_hour", "seven_day"]), true)]});
        assert_eq!(
            banked_resets(&refund, 100).unwrap().2[0].kind,
            ResetKind::Fable
        );
    }

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
        let several = usage(eligible(several));
        assert_eq!(several.remaining_reset_count, Some(3));
        let ends = |at: &str| {
            chrono::DateTime::parse_from_rfc3339(at)
                .unwrap()
                .timestamp()
        };
        assert_eq!(
            several.reset_expiries,
            [
                ends("2026-10-22T16:00:00Z"),
                ends("2026-11-01T00:00:00Z"),
                ends("2026-11-01T00:00:00Z")
            ]
        );
        assert_eq!(live.reset_expiries, [ends("2026-10-22T16:00:00Z")]);
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
    fn credit_expiries_include_paid_and_promotional_tranches_without_spending_dates() {
        let a = "2026-12-01T00:00:00Z";
        let b = "2027-01-01T00:00:00Z";
        let prepaid = json!({"amount":500, "next_expires_at":a,
            "tranches":[{"remaining_amount_minor_units":200,"expires_at":b},
                        {"remaining_amount_minor_units":0,"expires_at":"2026-11-01T00:00:00Z"}],
            "promo_tranches":[{"remaining_amount_minor_units":300,"expires_at":a},
                              {"remaining_amount_minor_units":10,"expires_at":"invalid"}]});
        let extra = json!({"monthly_limit":10000,"used_credits":0});
        let credits = extra_usage_credits(&extra, &prepaid).unwrap();
        assert_eq!(
            credits.expiries,
            [reset_at(&json!(a)).unwrap(), reset_at(&json!(b)).unwrap()]
        );
        assert_eq!(credits.balance.as_deref(), Some("$5.00"));
        assert!(prepaid_credit_expiries(&json!({"expiry_policy_months":12})).is_empty());
        assert_eq!(
            prepaid_credit_expiries(&json!({"next_expires_at":b})),
            [reset_at(&json!(b)).unwrap()]
        );
        let old: CreditSnapshot =
            serde_json::from_value(json!({"hasCredits":true,"unlimited":false,"balance":"5"}))
                .unwrap();
        assert!(old.expiries.is_empty());
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

    #[test]
    fn server_clock_offsets_respect_time_zones() {
        let now = reset_at(&json!("2026-10-30T12:00:00Z")).unwrap();
        for date in [
            "Fri, 30 Oct 2026 18:00:00 GMT",
            "Sat, 31 Oct 2026 02:00:00 +0800",
        ] {
            assert_eq!(clock_offset_seconds(date, now), Some(6 * 3600));
        }
        assert_eq!(
            clock_offset_seconds("Fri, 30 Oct 2026 06:00:00 GMT", now),
            Some(-6 * 3600)
        );
        assert_eq!(clock_offset_seconds("invalid", now), None);
        assert_eq!(reset_at(&json!("2026-10-30T20:00:00+08:00")), Some(now));
    }

    #[tokio::test]
    async fn usage_clock_offset_survives_storage_without_changing_reset_dates() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.accounts(Pool::Claude).await.unwrap()[0].id.clone();
        for offset in [Some(6 * 3600), Some(-6 * 3600), None] {
            *fake.usage_clock_offset.lock().unwrap() = offset;
            app.refresh_usage(Pool::Claude, &id).await.unwrap();
            let stored = app.claude_load().unwrap();
            let usage = stored.accounts[0].usage.as_ref().unwrap();
            assert_eq!(
                usage.five_hour.as_ref().unwrap().clock_offset_seconds,
                offset
            );
            assert_eq!(
                usage.one_week.as_ref().unwrap().clock_offset_seconds,
                offset
            );
            assert_eq!(
                usage.five_hour.as_ref().unwrap().reset_at,
                reset_at(&json!("2099-01-01T00:00:00Z"))
            );
        }
    }

    #[tokio::test]
    async fn extra_usage_credits_ask_for_the_balance_through_the_account_proxy() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.accounts(Pool::Claude).await.unwrap()[0].id.clone();
        let usage = json!({"five_hour":{"utilization":5},"extra_usage":{"is_enabled":true,
            "monthly_limit":4000,"used_credits":1285,"currency":"USD","decimal_places":2}});
        *fake.prepaid.lock().unwrap() = Some(json!({"amount":-1,"balance":{"money":null,
            "credits":{"amount_minor":1500,"exponent":2}}}));
        fake.usage.lock().unwrap().push((200, usage.clone()));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        assert_eq!(credits(&app), None);
        fake.usage
            .lock()
            .unwrap()
            .push((200, json!({"five_hour":{"utilization":5}})));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        assert_eq!(fake.prepaid_requests.lock().unwrap().len(), 2);
    }

    const REFRESH_SECONDS: i64 = crate::refresh_policy::CLAUDE_REFRESH_INTERVAL.as_secs() as i64;
    fn queue_usage_rate_limit(fake: &Fake, body: Value) {
        fake.usage
            .lock()
            .unwrap()
            .extend([(429, Value::Null), (429, body)]);
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
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        let stored = || app.claude_load().unwrap().accounts[0].clone();
        let usage = stored().usage.unwrap();
        for (count, minutes) in [(1, 5), (2, 10), (3, 20), (4, 40), (5, 60), (6, 60)] {
            queue_usage_rate_limit(&fake, json!({"error":{"message":"Rate limited"}}));
            let before = now_unix_seconds();
            let error = app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
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
            assert_eq!(
                app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
                error
            );
            assert_eq!(usages(&fake).len(), requests);
            end_pause(&app);
        }
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        let account = stored();
        assert_eq!(account.rate_limits, 0);
        assert_eq!(account.usage_retry_at, None);
        assert_eq!(account.usage_error, None);
        // The count is gone from the store once it is back to zero.
        let saved = fs::read_to_string(app.claude_store_path()).unwrap();
        assert!(!saved.contains("rateLimits"));
        // A first rate limit after a success pauses briefly again.
        queue_usage_rate_limit(&fake, Value::Null);
        app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
        assert_eq!(stored().rate_limits, 1);
    }

    #[tokio::test]
    async fn other_failures_keep_the_rate_limit_count_without_pausing() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        for _ in 0..2 {
            queue_usage_rate_limit(&fake, Value::Null);
            app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
            end_pause(&app);
        }
        fake.usage.lock().unwrap().push((500, Value::Null));
        assert_eq!(
            app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
            "Claude usage request failed: HTTP 500"
        );
        let account = app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(account.rate_limits, 2);
        assert_eq!(account.usage_retry_at, None);
        queue_usage_rate_limit(&fake, Value::Null);
        assert!(app
            .refresh_usage(Pool::Claude, &id)
            .await
            .unwrap_err()
            .ends_with("retry paused for 20 minutes"));
    }

    #[tokio::test]
    async fn a_longer_retry_after_sets_the_pause() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        for (retry_after, minutes) in [(Some(5400), 90), (Some(0), 10), (None, 20)] {
            *fake.usage_retry_after.lock().unwrap() = retry_after;
            queue_usage_rate_limit(&fake, Value::Null);
            let before = now_unix_seconds();
            let error = app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
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
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        let account = app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(account.usage_retry_at, None);
        assert_eq!(account.rate_limits, 0);
    }

    #[tokio::test]
    async fn background_rounds_adapt_their_interval_to_usage_changes() {
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        assert_eq!(usages(&fake).len(), 1);
        let (five, one) = (Duration::from_secs(300), Duration::from_secs(60));
        let usage = |hour: u32| (200, json!({"five_hour": {"utilization": hour}}));
        // Codex runs independently and never requests Claude.
        assert_eq!(app.background_round(0, Pool::Codex).await.as_secs(), 10);
        let codex = app.schedule(Pool::Codex).unwrap();
        // Asked at import: the first round waits, and nothing changed.
        assert_eq!(app.background_round(0, Pool::Claude).await, five);
        assert_eq!(usages(&fake).len(), 1);
        fake.usage.lock().unwrap().push(usage(25));
        age(&app, 300);
        assert_eq!(app.background_round(1, Pool::Claude).await, one);
        assert_eq!(app.round_interval(Pool::Claude), one);
        assert_eq!(usages(&fake).len(), 2);
        // At one minute, an account asked under a minute ago waits...
        age(&app, 45);
        assert_eq!(app.background_round(2, Pool::Claude).await, one);
        assert_eq!(usages(&fake).len(), 2);
        // ...and the second unchanged round slows down again.
        fake.usage.lock().unwrap().push(usage(25));
        age(&app, 60);
        assert_eq!(app.background_round(3, Pool::Claude).await, five);
        assert_eq!(usages(&fake).len(), 3);
        assert_eq!(app.round_interval(Pool::Claude), five);
        // At five minutes, a minute-old request is not due.
        age(&app, 60);
        assert_eq!(app.background_round(4, Pool::Claude).await, five);
        assert_eq!(usages(&fake).len(), 3);
        // A change while slow speeds up at once.
        fake.usage.lock().unwrap().push(usage(30));
        age(&app, 300);
        assert_eq!(app.background_round(6, Pool::Claude).await, one);
        assert_eq!(usages(&fake).len(), 4);
        // A rate limit is no change in usage.
        queue_usage_rate_limit(&fake, Value::Null);
        age(&app, 60);
        assert_eq!(app.background_round(7, Pool::Claude).await, one);
        assert_eq!(app.schedule(Pool::Codex), Some(codex));
        assert_eq!(app.background_round(1, Pool::Codex).await.as_secs(), 10);
        assert_eq!(app.round_interval(Pool::Claude), one);
        assert_eq!(usages(&fake).len(), 6);
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
        // Every round skips accounts asked within the interval.
        for tick in 0..12 {
            app.refresh_recurring(Pool::Claude, tick).await.unwrap();
        }
        assert_eq!(usages(&fake).len(), 2);
        age(&app, REFRESH_SECONDS - 5);
        app.refresh_recurring(Pool::Claude, 0).await.unwrap();
        assert_eq!(usages(&fake).len(), 2);
        // Once due, every round asks both current and inactive accounts.
        age(&app, REFRESH_SECONDS);
        app.refresh_recurring(Pool::Claude, 1).await.unwrap();
        let mut refreshed = usages(&fake)[2..].to_vec();
        refreshed.sort();
        assert_eq!(refreshed, ["alice-access", "bob-access"]);
        app.refresh_recurring(Pool::Claude, 2).await.unwrap();
        assert_eq!(usages(&fake).len(), 4);
        app.refresh_recurring(Pool::Claude, 6).await.unwrap();
        assert_eq!(usages(&fake).len(), 4);
        // A manual refresh is not held back by the interval.
        app.refresh_all(Pool::Claude).await.unwrap();
        let mut manual = usages(&fake)[4..].to_vec();
        manual.sort();
        assert_eq!(manual, ["alice-access", "bob-access"]);
        app.refresh_usage(Pool::Claude, &app.claude_load().unwrap().accounts[0].id)
            .await
            .unwrap();
        assert_eq!(usages(&fake).len(), 7);
    }

    #[tokio::test]
    async fn refreshing_the_pool_puts_the_next_round_off_by_an_interval() {
        use crate::refresh_policy::Schedule;
        let dir = tempfile::tempdir().unwrap();
        let (app, location, _fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let next = |accounts: Vec<AccountSummary>| accounts[0].next_refresh_at.unwrap();
        // A round ended a minute ago; refreshed now, Alice would miss the
        // next one and wait for the one after.
        let now = now_unix_seconds();
        let round = |at| Schedule {
            at,
            tick: 2,
            interval: REFRESH_SECONDS,
        };
        app.set_schedule(Pool::Claude, round(now + REFRESH_SECONDS - 60));
        app.refresh_all(Pool::Claude).await.unwrap();
        let now = now_unix_seconds();
        assert!(next(app.accounts(Pool::Claude).await.unwrap()) > now + REFRESH_SECONDS);
        // Deferred, the next round asks for her an interval from now.
        app.defer_background_round(Pool::Claude);
        let schedule = app.schedule(Pool::Claude).unwrap();
        assert!((now..=now + 1).contains(&(schedule.at - REFRESH_SECONDS)));
        assert_eq!(schedule.tick, 2);
        assert_eq!(next(app.accounts(Pool::Claude).await.unwrap()), schedule.at);
        // A round under way schedules the next itself; the other pool stays.
        app.set_schedule(Pool::Claude, round(now - 1));
        app.defer_background_round(Pool::Claude);
        assert_eq!(app.schedule(Pool::Claude), Some(round(now - 1)));
        assert_eq!(app.schedule(Pool::Codex), None);
        let codex = Schedule {
            at: now + 5,
            tick: 3,
            interval: 10,
        };
        app.set_schedule(Pool::Codex, codex);
        app.defer_background_round(Pool::Codex);
        let deferred = app.schedule(Pool::Codex).unwrap();
        assert!((now + 10..=now + 11).contains(&deferred.at));
        assert_eq!((deferred.tick, deferred.interval), (3, 10));
        assert_eq!(app.schedule(Pool::Claude), Some(round(now - 1)));
    }

    #[tokio::test]
    async fn cards_show_when_the_background_refresh_asks_for_them_next() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, _fake) = app(&dir).await;
        for name in ["carol", "bob", "alice"] {
            sign_in(&location, name, LATER);
            app.claude_import_current().await.unwrap();
        }
        let id = |name: &str| {
            let store = app.claude_load().unwrap();
            let email = format!("{name}@example.com");
            let account = store
                .accounts
                .iter()
                .find(|a| text(&a.account["emailAddress"]).as_deref() == Some(email.as_str()));
            account.unwrap().id.clone()
        };
        let (alice, bob, carol) = (id("alice"), id("bob"), id("carol"));
        // Nothing is scheduled before the first background round, which
        // asks no one: every account was asked at import.
        let next = |accounts: &[AccountSummary], id: &str| {
            accounts
                .iter()
                .find(|a| a.id == id)
                .unwrap()
                .next_refresh_at
        };
        let accounts = app.accounts(Pool::Claude).await.unwrap();
        assert_eq!(next(&accounts, &alice), None);
        app.background_round(1, Pool::Claude).await;
        let schedule = app.schedule(Pool::Claude).unwrap();
        assert_eq!((schedule.tick, schedule.interval), (2, REFRESH_SECONDS));
        // Inactive accounts have the same next round as the current one.
        let accounts = app.accounts(Pool::Claude).await.unwrap();
        assert_eq!(next(&accounts, &bob), Some(schedule.at));
        assert_eq!(app.next_full_refresh_at(Pool::Claude), Some(schedule.at));
        // A rate limit pause holds Bob; Carol waits for a manual refresh.
        let mut store = app.claude_load().unwrap();
        for account in &mut store.accounts {
            if account.id == bob {
                account.usage_retry_at = Some(schedule.at + 5 * REFRESH_SECONDS + 10);
            } else if account.id == carol {
                account.first_request_pending = true;
            }
        }
        app.claude_save(&store).unwrap();
        let accounts = app.accounts(Pool::Claude).await.unwrap();
        // Paused accounts return on the first round after their pause.
        assert_eq!(next(&accounts, &alice), Some(schedule.at));
        assert_eq!(
            next(&accounts, &bob),
            Some(schedule.at + 6 * REFRESH_SECONDS)
        );
        assert_eq!(next(&accounts, &carol), None);
    }

    #[tokio::test]
    async fn usage_rate_limits_pause_manual_and_background_refresh_until_backoff_expires() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.accounts(Pool::Claude).await.unwrap()[0].id.clone();
        queue_usage_rate_limit(&fake, json!({"error":{"message":"Rate limited"}}));
        let error = app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
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
        assert_eq!(
            app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
            error
        );
        age(&app, REFRESH_SECONDS);
        assert!(app.refresh_recurring(Pool::Claude, 0).await.is_err());
        assert_eq!(fake.usage.lock().unwrap().len(), 1);
        let mut store = app.claude_load().unwrap();
        store.accounts[0].usage_retry_at = Some(now_unix_seconds() - 1);
        app.claude_save(&store).unwrap();
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
            app.release_new_accounts(Pool::Claude, Some(&id))
                .await
                .unwrap();
            let credentials = fs::read(location.credentials_file()).unwrap();
            if expires == LATER {
                fake.usage.lock().unwrap().push((401, Value::Null));
            }
            *fake.refresh_error.lock().unwrap() = Some(TOKEN_REFRESH_RATE_LIMITED.into());
            let before = now_unix_seconds();
            let paused = format!("{TOKEN_REFRESH_RATE_LIMITED}; retry paused for 5 minutes");
            assert_eq!(
                app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
                paused
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
                app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
                paused
            );
            assert!(app.refresh_all(Pool::Claude).await.is_err());
            age(&app, REFRESH_SECONDS);
            assert!(app.refresh_recurring(Pool::Claude, 0).await.is_err());
            assert_eq!(fake.refreshed.lock().unwrap().len(), 1);
            assert_eq!(fake.usage_tokens.lock().unwrap().len(), usage_calls);
            let mut store = app.claude_load().unwrap();
            store.accounts[0].usage_retry_at = Some(now_unix_seconds() - 1);
            app.claude_save(&store).unwrap();
            if expires == LATER {
                fake.usage.lock().unwrap().push((401, Value::Null));
            }
            app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
            app.release_new_accounts(Pool::Claude, Some(&id))
                .await
                .unwrap();
            *fake.refresh_error.lock().unwrap() = Some(error.into());
            assert_eq!(
                app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
                error
            );
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
            json("502 Bad Gateway", r#"{"error":{"message":"Claude account has no proxy route."}}"#),
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
        assert_eq!(
            refresh_at(&url, "rt-5", scope, "direct://")
                .await
                .unwrap_err(),
            "Claude token refresh failed: HTTP 502 Claude account has no proxy route."
        );
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 5);
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
                    "client_id": crate::claude_api::CLIENT,
                    "scope": scope,
                })
            );
        }
    }

    #[test]
    fn renewal_asks_again_for_the_scopes_the_login_holds() {
        let default = crate::claude_api::SCOPES;
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
        app.release_new_accounts(Pool::Claude, Some(&carol))
            .await
            .unwrap();
        app.refresh_usage(Pool::Claude, &carol).await.unwrap();
        assert_eq!(
            fake.refresh_scopes.lock().unwrap()[1],
            crate::claude_api::SCOPES
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
            crate::claude_api::TOKEN_URL,
            "sk-ant-ort01-copool-invalid-probe",
            crate::claude_api::SCOPES,
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
    async fn fallback_updates_windows_and_keeps_resets_until_confirmed_again() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        let grants =
            json!([{"resets_left":1,"ends_at":"2099-01-01T00:00:00Z","gives_back_weekly":true}]);
        fake.usage.lock().unwrap().push((
            200,
            json!({"cedar_ember":{"eligible":true,"grants":grants}}),
        ));
        app.claude_import_current().await.unwrap();
        let stored = || app.claude_load().unwrap().accounts[0].clone();
        let id = stored().id;
        let first = stored().usage.unwrap();
        for used in [41, 42] {
            fake.usage.lock().unwrap().extend([
                (429, Value::Null),
                (200, json!({"five_hour":{"utilization":used},"cedar_ember":{"eligible":false,"ineligible_reason":"surface"}}))
            ]);
            app.refresh_usage(Pool::Claude, &id).await.unwrap();
            let usage = stored().usage.unwrap();
            assert_eq!(usage.five_hour.unwrap().used_percent, f64::from(used));
            assert_eq!(usage.remaining_reset_count, Some(1));
            assert_eq!(usage.reset_grants[0].kind, ResetKind::Fable);
            assert_eq!(usage.reset_expiries, first.reset_expiries);
            assert_eq!(usage.resets_last_confirmed_at, Some(first.fetched_at));
        }
        fake.usage
            .lock()
            .unwrap()
            .push((200, json!({"cedar_ember":{"eligible":true,"grants":[]}})));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        let usage = stored().usage.unwrap();
        assert_eq!(usage.remaining_reset_count, Some(0));
        assert!(usage.reset_grants.is_empty());
        assert_eq!(usage.resets_last_confirmed_at, None);
        let (cli, code) = (USAGE_USER_AGENT, USER_AGENT);
        assert_eq!(agents(&fake), [cli, cli, code, cli, code, cli]);
    }

    #[tokio::test]
    async fn a_refusal_does_not_affect_another_account_on_the_same_proxy() {
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
        app.save_account_proxy(Pool::Claude, &id("alice"), "http://127.0.0.1:7001")
            .await
            .unwrap();
        app.save_account_proxy(Pool::Claude, &id("bob"), "http://127.0.0.1:7001")
            .await
            .unwrap();
        let (cli, code) = (USAGE_USER_AGENT, USER_AGENT);
        let start = agents(&fake).len();
        fake.usage.lock().unwrap().push((429, Value::Null));
        app.refresh_usage(Pool::Claude, &id("alice")).await.unwrap();
        app.refresh_usage(Pool::Claude, &id("bob")).await.unwrap();
        assert_eq!(agents(&fake)[start..], [cli, code, cli]);
        assert_eq!(
            fake.usage_proxies.lock().unwrap()[start..],
            [
                "http://127.0.0.1:7001",
                "http://127.0.0.1:7001",
                "http://127.0.0.1:7001"
            ]
        );
        // Refused both ways: a rate limit, counted and paused once.
        fake.usage.lock().unwrap().push((429, Value::Null));
        fake.usage
            .lock()
            .unwrap()
            .push((429, json!({"error": {"message": "Rate limited"}})));
        *fake.usage_retry_after.lock().unwrap() = Some(5400);
        let error = app
            .refresh_usage(Pool::Claude, &id("bob"))
            .await
            .unwrap_err();
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        fake.usage.lock().unwrap().push((401, Value::Null));
        fake.usage.lock().unwrap().push((429, Value::Null));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
        /// Reads report the item missing, as the Keychain can to a process
        /// it hides an item from.
        hidden: std::sync::atomic::AtomicBool,
        reads: Mutex<Vec<KeychainAccess>>,
        writes: Mutex<usize>,
    }
    impl Vault for MemoryVault {
        fn read(&self, access: KeychainAccess) -> Result<Option<String>> {
            self.reads.lock().unwrap().push(access);
            if self.unreadable.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("The user denied access".into());
            }
            if self.hidden.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(None);
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        app.accounts(Pool::Claude).await.unwrap();
        assert_eq!(vault.writes(), 2);
        assert_eq!(vault.reads().len(), 3);
        // Reopened, the tokens come back from the vault without asking.
        drop(app);
        let (app, _, fake) = vault_app(&dir, &vault);
        assert_eq!(app.accounts(Pool::Claude).await.unwrap().len(), 1);
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
        app.release_new_accounts(Pool::Claude, Some(&bob))
            .await
            .unwrap();
        let held = vault.secrets();
        drop(app);
        // Reopened without access to the vault (a new build not yet allowed).
        vault.set(&vault.unreadable, true);
        let (app, _, fake) = vault_app(&dir, &vault);
        let cards = app.accounts(Pool::Claude).await.unwrap();
        assert_eq!(cards.len(), 1);
        // The card shows its tokens are out of reach, as its error.
        let locked = |error: &Option<String>| {
            error
                .as_deref()
                .is_some_and(|e| e.starts_with(VAULT_LOCKED) && e.contains("denied"))
        };
        assert!(locked(&cards[0].usage_error));
        assert!(app
            .refresh_usage(Pool::Claude, &bob)
            .await
            .unwrap_err()
            .starts_with(VAULT_LOCKED));
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
        assert!(app
            .switch_account(Pool::Claude, &bob)
            .await
            .unwrap_err()
            .starts_with(VAULT_LOCKED));
        assert!(!location.credentials_file().exists());
        // The account whose tokens stayed in the store is not locked.
        let cards = app.accounts(Pool::Claude).await.unwrap();
        let card = |label: &str| cards.iter().find(|c| c.label == label).unwrap();
        let account_locked = |label: &str| locked(&card(label).usage_error);
        assert!(account_locked("bob@example.com"));
        assert!(!account_locked("carol@example.com"));
        // A user action, such as a manual refresh, asks again; once allowed,
        // every token is there and moves to the vault.
        vault.set(&vault.unreadable, false);
        let asked = vault.reads().len();
        app.claude_authorize().await.unwrap();
        assert_eq!(vault.reads()[asked..].first(), Some(&Interactive));
        assert!(app
            .accounts(Pool::Claude)
            .await
            .unwrap()
            .iter()
            .all(|card| !locked(&card.usage_error)));
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
        app.refresh_usage(Pool::Claude, &bob).await.unwrap();
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        assert!(vault.writes() >= 1);
        assert!(store_text(&app).contains("bob-refresh"));
        assert_eq!(vault.secrets(), json!({}));
        // The next save that sticks moves them.
        vault.set(&vault.lost_writes, false);
        app.save_account_proxy(Pool::Claude, &id, "http://127.0.0.1:7001")
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
        app.release_new_accounts(Pool::Claude, None).await.unwrap();
        // A renewal rotates the tokens in the vault, never in the store.
        app.refresh_usage(Pool::Claude, &bob).await.unwrap();
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
        app.refresh_usage(Pool::Claude, &bob).await.unwrap();
        assert_eq!(refreshes(&fake), ["bob-refresh", "fresh-refresh"]);
        assert!(store_text(&app).contains("next-refresh"));
        assert_eq!(vault.secrets()[&bob]["refreshToken"], "fresh-refresh");
        let rotated = app.claude_load().unwrap();
        let held = rotated.accounts.iter().find(|a| a.id == bob).unwrap();
        assert_eq!(held.oauth["refreshToken"], "next-refresh");
        vault.set(&vault.unwritable, false);
        // A switch hands Claude Code the tokens from the vault.
        app.switch_account(Pool::Claude, &bob).await.unwrap();
        assert_eq!(live_access(&location), "next-access");
        assert_no_tokens(&app, &["next-access", "next-refresh", "alice-access"]);
        assert_eq!(vault.secrets()[&bob]["refreshToken"], "next-refresh");
        assert_eq!(vault.secrets()[&alice]["refreshToken"], "alice-refresh");
        // A deleted account leaves the vault too.
        app.delete_account(Pool::Claude, &alice).await.unwrap();
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
        app.accounts(Pool::Claude).await.unwrap();
        let account = app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(account.oauth["accessToken"], "alice-renewed");
        // The tokens it could not read are no generation to fall back on.
        assert!(account.alternates.is_empty());
        app.release_new_accounts(Pool::Claude, Some(&alice))
            .await
            .unwrap();
        app.refresh_usage(Pool::Claude, &alice).await.unwrap();
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
        app.release_new_accounts(Pool::Claude, Some(&alice))
            .await
            .unwrap();
        // By default renewed, and the renewal goes back to the file it was
        // read from, never to a new Keychain item.
        app.refresh_usage(Pool::Claude, &alice).await.unwrap();
        assert_eq!(refreshes(&fake), ["alice-refresh"]);
        assert_eq!(live_access(&location), "fresh-access");
        let document: Value =
            serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
        assert_eq!(document["mcpOAuth"]["kept"], true);
        // Renewal turned off: the file is followed, not renewed nor written.
        app.set_renewal(Pool::Claude, &alice, "never")
            .await
            .unwrap();
        external_update(location.clone(), "alice", "alice-synced")();
        let synced = fs::read(location.credentials_file()).unwrap();
        fake.usage.lock().unwrap().push((401, Value::Null));
        assert_eq!(
            app.refresh_usage(Pool::Claude, &alice).await.unwrap_err(),
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
        app.accounts(Pool::Claude)
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
        app.release_new_accounts(Pool::Claude, None).await.unwrap();
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
        app.set_renewal(Pool::Claude, &bob, "never").await.unwrap();
        assert_eq!(
            app.refresh_usage(Pool::Claude, &bob).await.unwrap_err(),
            RENEWAL_OFF
        );
        assert!(refreshes(&fake).is_empty());
        // Once: renewed once, then never again.
        app.set_renewal(Pool::Claude, &bob, "once").await.unwrap();
        let card = |label: &str, cards: Vec<(String, Option<String>)>| {
            cards.into_iter().find(|c| c.0 == label).unwrap().1
        };
        assert_eq!(
            card("bob@example.com", renewals(&app).await).as_deref(),
            Some("once")
        );
        app.refresh_usage(Pool::Claude, &bob).await.unwrap();
        assert_eq!(refreshes(&fake), ["bob-refresh"]);
        assert_eq!(
            card("bob@example.com", renewals(&app).await).as_deref(),
            Some("never")
        );
        fake.usage.lock().unwrap().push((401, Value::Null));
        assert_eq!(
            app.refresh_usage(Pool::Claude, &bob).await.unwrap_err(),
            RENEWAL_OFF
        );
        assert_eq!(refreshes(&fake), ["bob-refresh"]);
        // Always, chosen or by default, renews the file login into its file.
        app.set_renewal(Pool::Claude, &alice, "always")
            .await
            .unwrap();
        app.refresh_usage(Pool::Claude, &alice).await.unwrap();
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
        assert!(app
            .set_renewal(Pool::Claude, &bob, "sometimes")
            .await
            .is_err());
        assert!(app
            .set_renewal(Pool::Claude, "missing", "always")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn renew_now_renews_without_asking_usage_unless_the_account_failed() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = file_login_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let (bob, account) = login("bob", 1);
        app.claude_import_json(&export(&bob, &account))
            .await
            .unwrap();
        app.release_new_accounts(Pool::Claude, None).await.unwrap();
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
        let renewal = |cards: Vec<(String, Option<String>)>, label: &str| {
            cards.into_iter().find(|c| c.0 == label).unwrap().1
        };
        // An unexpired login is not renewed by a refresh.
        app.refresh_usage(Pool::Claude, &alice).await.unwrap();
        assert!(refreshes(&fake).is_empty());
        let asked = usages(&fake).len();
        // Renew now renews it even with renewal off, writes it back to
        // Claude Code's login, asks no usage and leaves the choice as it was.
        app.set_renewal(Pool::Claude, &alice, "never")
            .await
            .unwrap();
        app.renew_now(Pool::Claude, &alice).await.unwrap();
        assert_eq!(refreshes(&fake), ["alice-refresh"]);
        assert_eq!(usages(&fake).len(), asked);
        assert_eq!(live_access(&location), "fresh-access");
        assert_eq!(
            renewal(renewals(&app).await, "alice@example.com").as_deref(),
            Some("never")
        );
        // An account that had failed refreshes its usage once renewed.
        app.set_renewal(Pool::Claude, &bob, "never").await.unwrap();
        assert_eq!(
            app.refresh_usage(Pool::Claude, &bob).await.unwrap_err(),
            RENEWAL_OFF
        );
        app.renew_now(Pool::Claude, &bob).await.unwrap();
        assert_eq!(refreshes(&fake), ["alice-refresh", "bob-refresh"]);
        assert_eq!(usages(&fake).len(), asked + 1);
        let card = app
            .claude_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| a.id == bob)
            .unwrap();
        assert_eq!(card.usage_error, None);
        // A refused renewal is reported.
        *fake.refresh_error.lock().unwrap() = Some(SIGN_IN_EXPIRED.into());
        assert!(app.renew_now(Pool::Claude, &alice).await.is_err());
        assert!(app.renew_now(Pool::Claude, "missing").await.is_err());
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
    async fn a_vault_without_the_tokens_moved_there_is_never_trusted_nor_written() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let (app, _, _) = vault_app(&dir, &vault);
        let (bob, account) = login("bob", LATER);
        app.claude_import_json(&export(&bob, &account))
            .await
            .unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        let held = vault.secrets();
        let writes = vault.writes();
        drop(app);
        // Reported missing although bob's tokens were moved there.
        vault.set(&vault.hidden, true);
        let (app, _, fake) = vault_app(&dir, &vault);
        let card = app.accounts(Pool::Claude).await.unwrap().remove(0);
        let error = card.usage_error.unwrap();
        assert!(error.starts_with(VAULT_LOCKED), "{error}");
        assert!(error.contains("reports the item missing"), "{error}");
        let refused = app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
        assert!(refused.contains("reports the item missing"), "{refused}");
        assert!(usages(&fake).is_empty());
        // Saving, importing or asking writes nothing over it.
        let (carol, carol_account) = login("carol", LATER);
        app.claude_import_json(&export(&carol, &carol_account))
            .await
            .unwrap();
        assert!(store_text(&app).contains("carol-refresh"));
        let asked = vault.reads().len();
        assert!(app.claude_authorize().await.is_err());
        assert!(vault.reads()[asked..].contains(&Interactive));
        assert_eq!(vault.writes(), writes);
        assert_eq!(vault.secrets(), held);
        // Contents without bob's tokens are not the pool's either.
        vault.set(&vault.hidden, false);
        let text = vault.text.lock().unwrap().clone();
        *vault.text.lock().unwrap() = Some(r#"{"someone-else":{}}"#.into());
        let error = app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
        assert!(
            error.contains("lacks the tokens of 1 account(s)"),
            "{error}"
        );
        assert_eq!(vault.writes(), writes);
        // Once it reads back as written, everything is there again.
        *vault.text.lock().unwrap() = text;
        app.claude_authorize().await.unwrap();
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        assert_eq!(usages(&fake), ["bob-access"]);
        assert_no_tokens(&app, &["bob-refresh", "carol-refresh"]);
    }

    #[tokio::test]
    async fn background_work_never_asks_for_keychain_access() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let (app, location, _) = vault_app(&dir, &vault);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        drop(app);
        let (app, _, _) = vault_app(&dir, &vault);
        let start = vault.reads().len();
        app.background_round(1, Pool::Claude).await;
        app.refresh_all(Pool::Claude).await.unwrap();
        app.accounts(Pool::Claude).await.unwrap();
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
            .refresh_all(Pool::Claude)
            .await
            .unwrap_err()
            .contains(VAULT_LOCKED));
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
        usage_clock_offset: Mutex<Option<i64>>,
        refreshed: Mutex<Vec<String>>,
        refresh_scopes: Mutex<Vec<String>>,
        profile_proxies: Mutex<Vec<String>>,
        usage_tokens: Mutex<Vec<String>>,
        usage_agents: Mutex<Vec<String>>,
        usage_proxies: Mutex<Vec<String>>,
        /// The OAuth API root of each usage, profile and prepaid request.
        api_roots: Mutex<Vec<String>>,
        usage_hooks: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
        refresh_hooks: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
        refresh_error: Mutex<Option<String>>,
        /// The next token response, instead of the usual one.
        refresh_response: Mutex<Option<Value>>,
        prepaid: Mutex<Option<Value>>,
        prepaid_requests: Mutex<Vec<(String, String)>>,
        hellos: Mutex<Vec<(u16, Value)>>,
        hello_requests: Mutex<Vec<(String, String)>>,
        hello_urls: Mutex<Vec<String>>,
        hello_accounts: Mutex<Vec<String>>,
    }
    impl Network for Fake {
        fn usage<'a>(
            &'a self,
            api: &'a str,
            token: &'a str,
            agent: &'a str,
            proxy: &'a str,
        ) -> Response<'a, UsageResponse> {
            Box::pin(async move {
                self.api_roots.lock().unwrap().push(api.into());
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
                Ok((
                    status,
                    body,
                    *self.usage_retry_after.lock().unwrap(),
                    *self.usage_clock_offset.lock().unwrap(),
                ))
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
        fn profile<'a>(&'a self, api: &'a str, _: &'a str, proxy: &'a str) -> Response<'a, Value> {
            Box::pin(async move {
                self.api_roots.lock().unwrap().push(api.into());
                self.profile_proxies.lock().unwrap().push(proxy.into());
                Ok(
                    json!({"account":{"uuid":"acct-new","email":"new@example.com"},"organization":{"uuid":"org-new","name":"New","organization_type":"claude_max","rate_limit_tier":"default_claude_max_5x"}}),
                )
            })
        }
        fn prepaid<'a>(
            &'a self,
            api: &'a str,
            _: &'a str,
            org: &'a str,
            proxy: &'a str,
        ) -> Response<'a, Value> {
            Box::pin(async move {
                self.api_roots.lock().unwrap().push(api.into());
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
        fn hello<'a>(
            &'a self,
            url: &'a str,
            token: &'a str,
            account: &'a str,
            proxy: &'a str,
        ) -> Response<'a, (u16, Value)> {
            Box::pin(async move {
                self.hello_urls.lock().unwrap().push(url.into());
                self.hello_accounts.lock().unwrap().push(account.into());
                self.hello_requests
                    .lock()
                    .unwrap()
                    .push((token.into(), proxy.into()));
                let mut queued = self.hellos.lock().unwrap();
                Ok(if queued.is_empty() {
                    (200, Value::Null)
                } else {
                    queued.remove(0)
                })
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
        let item = crate::vault::Item::new(Arc::new(MemoryVault::default()));
        let app = Application::open_with_secrets(
            crate::Paths {
                data_dir: dir.path().join("data"),
                codex_auth: dir.path().join("codex/auth.json"),
            },
            Arc::new(crate::vault::ItemSecrets::new(item)),
        )
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
        app.switch_account(Pool::Claude, &bob_id).await.unwrap();
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
        let accounts = app.accounts(Pool::Claude).await.unwrap();
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
        app.switch_account(Pool::Claude, &alice).await.unwrap();
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
        let current = app.accounts(Pool::Claude).await.unwrap();
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
        let current = app.accounts(Pool::Claude).await.unwrap();
        assert_eq!(current.iter().find(|a| a.is_current).unwrap().id, bob.id);
        let stored = app.claude_load().unwrap();
        let bob_stored = stored.accounts.iter().find(|a| a.id == bob.id).unwrap();
        assert_eq!(bob_stored.oauth["accessToken"], "bob-newer");

        // A login outside the pool is backed up, and stays so across switches.
        sign_in(&location, "carol", LATER);
        app.switch_account(Pool::Claude, &alice).await.unwrap();
        let read_backup = || serde_json::from_slice::<Value>(&fs::read(&backup).unwrap()).unwrap();
        let backed_up = read_backup();
        assert_eq!(
            backed_up["credentials"]["claudeAiOauth"]["accessToken"],
            "carol-access"
        );
        app.switch_account(Pool::Claude, &bob.id).await.unwrap();
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
        app.accounts(Pool::Claude).await.unwrap();
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
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
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        reject_next(&fake);
        assert_eq!(
            app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
            SIGN_IN_EXPIRED
        );
        fake.usage.lock().unwrap().push((401, Value::Null));
        assert_eq!(
            app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
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
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        assert_eq!(refreshes(&fake), ["bob-refresh"]);
    }

    #[tokio::test]
    async fn a_rejected_login_falls_back_to_the_pool_and_repairs_claude_code() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
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
        app.accounts(Pool::Claude).await.unwrap();
        assert_eq!(stored().oauth["refreshToken"], "alice-refresh");
        assert_eq!(stored().alternates, [renewed.clone()]);
        assert!(stored().in_live);

        reject_next(&fake);
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
        app.accounts(Pool::Claude).await.unwrap();
        assert_eq!(stored().oauth, renewed);
        assert!(!stored().in_live);
        assert_eq!(
            app.claude_load().unwrap().current_account_id,
            Some(id.clone())
        );
        // ...and the next refresh repairs it without sending the rejected token.
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
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
        app.switch_account(Pool::Claude, &account("alice").id)
            .await
            .unwrap();
        assert!(account("alice").in_live);
        assert!(!account("bob").in_live);
        // Claude Code renews alice: the generation it held is gone.
        external_update(location.clone(), "alice", "alice-renewed")();
        app.accounts(Pool::Claude).await.unwrap();
        assert_eq!(account("alice").oauth["accessToken"], "alice-renewed");
        assert!(account("alice").alternates.is_empty());
        // A sign-in to bob there keeps bob's pool tokens, which it did not hold.
        let bob = account("bob").oauth;
        external_update(location.clone(), "bob", "bob-login")();
        app.accounts(Pool::Claude).await.unwrap();
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        // Unreadable when the refresh starts, an older login once it renews.
        fs::create_dir_all(&location.config_dir).unwrap();
        fs::write(location.credentials_file(), "not json").unwrap();
        fake.refresh_hooks.lock().unwrap().push(external_update(
            location.clone(),
            "alice",
            "stale",
        ));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        assert_eq!(refreshes(&fake), ["alice-refresh"]);
        assert_eq!(usages(&fake), ["fresh-access", "stale"]);
        let stored = || app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(stored().oauth["accessToken"], "stale");
        assert_eq!(refresh_tokens(&stored().alternates), ["fresh-refresh"]);
        assert!(stored().in_live);
        // When Claude refuses that login, the renewal takes over and reaches
        // Claude Code.
        reject_next(&fake);
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        let (mut other, _) = login("bob", LATER);
        other["accessToken"] = "bob-other".into();
        other["refreshToken"] = "bob-other-refresh".into();
        let (path, adopted) = (app.claude_store_path(), other.clone());
        fake.usage_hooks.lock().unwrap().push(Box::new(move || {
            let mut store: Store = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert!(store.accounts[0].adopt(&adopted, false));
            write_json(&path, &store).unwrap();
        }));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        // Claude Code's login turns back to a generation Claude rejected.
        fake.usage_hooks
            .lock()
            .unwrap()
            .push(external_update(location.clone(), "alice", "stale"));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        let stored = app.claude_load().unwrap().accounts[0].clone();
        assert_eq!(stored.oauth, valid);
        assert!(stored.alternates.is_empty());
        assert!(!stored.in_live);
        assert_eq!(usages(&fake), ["alice-access"]);
        // Nothing is written over a login that changed during the request.
        assert_eq!(live_access(&location), "stale");
        // Later reads keep ignoring it.
        app.accounts(Pool::Claude).await.unwrap();
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        // Claude refuses the login; while it does, Claude Code rewrites the
        // same rejected generation with another expiry.
        reject_next(&fake);
        fake.refresh_hooks.lock().unwrap().push(Box::new({
            let location = location.clone();
            move || sign_in(&location, "alice", LATER + 5)
        }));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
            app.release_new_accounts(Pool::Claude, Some(&id))
                .await
                .unwrap();
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
        assert!(app.switch_account(Pool::Claude, &id).await.is_err());
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        let (a, b) = tokio::join!(
            app.refresh_usage(Pool::Claude, &id),
            app.refresh_usage(Pool::Claude, &id)
        );
        a.unwrap();
        b.unwrap();
        assert_eq!(*fake.refreshed.lock().unwrap(), vec!["alice-refresh"]);
        assert_eq!(
            app.claude_load().unwrap().accounts[0].oauth["accessToken"],
            "fresh-access"
        );
        // The active login and pool share the refreshed generation.
        app.accounts(Pool::Claude).await.unwrap();
        assert_eq!(
            app.claude_load().unwrap().accounts[0].oauth["refreshToken"],
            "fresh-refresh"
        );
        fake.usage.lock().unwrap().push((401, Value::Null));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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

    /// The live login, signed in with an expired token, imported and released.
    async fn expired_live_login(
        dir: &tempfile::TempDir,
    ) -> (Application, Location, Arc<Fake>, String) {
        let (app, location, fake) = waiting_app(dir);
        sign_in(&location, "alice", 1);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        (app, location, fake, id)
    }
    fn claude_code_locks(location: &Location) -> [PathBuf; 2] {
        refresh_lock::lock_paths(&location.storage_dir)
    }
    fn hold_claude_code_locks(location: &Location) -> [PathBuf; 2] {
        let locks = claude_code_locks(location);
        for lock in &locks {
            fs::create_dir(lock).unwrap();
        }
        locks
    }

    #[tokio::test]
    async fn renewal_waits_while_claude_code_renews_and_never_spends_its_token() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake, id) = expired_live_login(&dir).await;
        let live = fs::read(location.credentials_file()).unwrap();
        // Claude Code holds its lock throughout: nothing is renewed or written.
        let locks = hold_claude_code_locks(&location);
        let error = app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
        assert!(error.contains(CLAUDE_CODE_RENEWING), "{error}");
        assert!(refreshes(&fake).is_empty());
        assert!(usages(&fake).is_empty());
        assert_eq!(fs::read(location.credentials_file()).unwrap(), live);
        // Another holder's lock is never removed.
        assert!(locks.iter().all(|lock| lock.is_dir()));
        // Only the legacy lock, as an older Claude Code takes it, also counts.
        fs::remove_dir(&locks[0]).unwrap();
        assert!(app.refresh_usage(Pool::Claude, &id).await.is_err());
        assert!(refreshes(&fake).is_empty());
        assert!(!locks[0].exists() && locks[1].is_dir());

        // Claude Code finishes: it renewed the same login and released.
        fs::remove_dir(&locks[1]).unwrap();
        hold_claude_code_locks(&location);
        let finish = {
            let location = location.clone();
            let locks = locks.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(150));
                external_update(location, "alice", "code")();
                for lock in locks.iter().rev() {
                    fs::remove_dir(lock).unwrap();
                }
            })
        };
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        finish.join().unwrap();
        // Its renewal is adopted; the token it spent is not sent again.
        assert!(refreshes(&fake).is_empty());
        assert_eq!(usages(&fake), ["code"]);
        let stored = app.claude_load().unwrap();
        assert_eq!(stored.accounts[0].oauth["refreshToken"], "code-refresh");
        assert_eq!(live_access(&location), "code");
        assert!(locks.iter().all(|lock| !lock.exists()));
    }

    #[tokio::test]
    async fn renewal_reaches_claude_codes_login_under_its_lock_before_usage_is_asked() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake, id) = expired_live_login(&dir).await;
        // Members Copool does not keep: Claude Code's client and MCP logins.
        let mut document: Value =
            serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
        document["claudeAiOauth"]["clientId"] = "code-client".into();
        write_json(&location.credentials_file(), &document).unwrap();
        let locks = claude_code_locks(&location);
        let observed = Arc::new(Mutex::new(Vec::new()));
        fake.refresh_hooks.lock().unwrap().push(Box::new({
            let (locks, observed) = (locks.clone(), observed.clone());
            move || {
                let held = locks.iter().all(|lock| lock.is_dir());
                observed
                    .lock()
                    .unwrap()
                    .push(format!("refresh locked={held}"));
            }
        }));
        fake.usage_hooks.lock().unwrap().push(Box::new({
            let (locks, location, observed) = (locks.clone(), location.clone(), observed.clone());
            move || {
                let held = locks.iter().any(|lock| lock.exists());
                observed.lock().unwrap().push(format!(
                    "usage locked={held} live={}",
                    live_access(&location)
                ));
            }
        }));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        assert_eq!(
            *observed.lock().unwrap(),
            [
                "refresh locked=true",
                "usage locked=false live=fresh-access"
            ]
        );
        let live: Value =
            serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
        assert_eq!(live["claudeAiOauth"]["refreshToken"], "fresh-refresh");
        assert_eq!(live["claudeAiOauth"]["clientId"], "code-client");
        assert_eq!(live["mcpOAuth"], json!({"kept": true}));
        let stored = app.claude_load().unwrap();
        // The pool and Claude Code hold one generation.
        assert!(same_oauth_generation(
            &stored.accounts[0].oauth,
            &live["claudeAiOauth"]
        ));
        assert!(stored.accounts[0].in_live);
        assert_eq!(refreshes(&fake), ["alice-refresh"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn renewal_takes_over_from_a_claude_code_that_died_holding_its_lock() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake, id) = expired_live_login(&dir).await;
        // Exited mid-renewal: its locks are far from stale, but its owner
        // record proves it gone.
        let locks = refresh_lock::hold_as_dead_claude_code(&location.storage_dir);
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        assert_eq!(refreshes(&fake), ["alice-refresh"]);
        assert_eq!(live_access(&location), "fresh-access");
        assert!(locks.iter().all(|lock| !lock.exists()));
        assert!(!location.storage_dir.join(refresh_lock::OWNER_FILE).exists());
    }

    #[tokio::test]
    async fn switching_waits_until_claude_code_has_renewed_the_login_being_left() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, _) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        sign_in(&location, "bob", LATER);
        app.claude_import_current().await.unwrap();
        let alice = app
            .claude_load()
            .unwrap()
            .accounts
            .iter()
            .find(|a| a.label == "alice@example.com")
            .unwrap()
            .id
            .clone();
        let locks = hold_claude_code_locks(&location);
        let error = app.switch_account(Pool::Claude, &alice).await.unwrap_err();
        assert!(error.contains(CLAUDE_CODE_RENEWING), "{error}");
        assert_eq!(live_access(&location), "bob-access");
        for lock in locks.iter().rev() {
            fs::remove_dir(lock).unwrap();
        }
        app.switch_account(Pool::Claude, &alice).await.unwrap();
        assert_eq!(live_access(&location), "alice-access");
        assert!(locks.iter().all(|lock| !lock.exists()));
    }

    #[tokio::test]
    async fn pool_logins_renew_without_creating_a_claude_code_directory() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        let (oauth, account) = login("bob", 1);
        app.claude_import_json(
            &json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string(),
        )
        .await
        .unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        assert_eq!(refreshes(&fake), ["bob-refresh"]);
        assert!(!location.storage_dir.exists());
        let stored = app.claude_load().unwrap();
        assert_eq!(stored.accounts[0].oauth["refreshToken"], "fresh-refresh");
    }

    #[tokio::test]
    async fn a_pool_login_renews_after_waiting_for_claude_code_to_renew_another() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let (oauth, account) = login("bob", 1);
        app.claude_import_json(
            &json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string(),
        )
        .await
        .unwrap();
        let bob = app
            .claude_load()
            .unwrap()
            .accounts
            .iter()
            .find(|a| a.oauth["refreshToken"] == "bob-refresh")
            .unwrap()
            .id
            .clone();
        app.release_new_accounts(Pool::Claude, Some(&bob))
            .await
            .unwrap();
        // Claude Code renews its own login, alice's, while bob's waits.
        let locks = hold_claude_code_locks(&location);
        let finish = {
            let location = location.clone();
            let locks = locks.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(150));
                external_update(location, "alice", "code")();
                for lock in locks.iter().rev() {
                    fs::remove_dir(lock).unwrap();
                }
            })
        };
        app.refresh_usage(Pool::Claude, &bob).await.unwrap();
        finish.join().unwrap();
        assert_eq!(refreshes(&fake), ["bob-refresh"]);
        assert_eq!(live_access(&location), "code");
        let stored = app.claude_load().unwrap();
        let saved = stored.accounts.iter().find(|a| a.id == bob).unwrap();
        assert_eq!(saved.oauth["refreshToken"], "fresh-refresh");
        assert!(saved.usage_error.is_none());
    }

    #[tokio::test]
    async fn live_rotation_is_preserved_even_when_usage_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", 1);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        fake.usage.lock().unwrap().push((500, Value::Null));
        assert!(app
            .refresh_usage(Pool::Claude, &id)
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        fs::write(location.credentials_file(), "{").unwrap();
        assert_eq!(
            app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        let (a, b) = tokio::join!(
            app.refresh_usage(Pool::Claude, &id),
            app.refresh_usage(Pool::Claude, &id)
        );
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
            app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        // Only the account Claude Code uses waits for its login to be readable.
        sign_in(&location, "bob", LATER);
        app.claude_import_current().await.unwrap();
        fs::write(location.credentials_file(), "invalid").unwrap();
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        fake.usage.lock().unwrap().push((401, Value::Null));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
            app.release_new_accounts(Pool::Claude, Some(&id))
                .await
                .unwrap();
            fake.refresh_hooks.lock().unwrap().push(external_update(
                location.clone(),
                "alice",
                "external",
            ));
            if failed {
                *fake.refresh_error.lock().unwrap() = Some(SIGN_IN_EXPIRED.into());
            }
            app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        fake.usage_hooks.lock().unwrap().push(external_update(
            location.clone(),
            "alice",
            "external",
        ));
        fake.usage.lock().unwrap().push((401, Value::Null));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
    async fn nameless_rotation_during_a_401_uses_the_verified_token_without_renewal() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "new", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        let changed = location.clone();
        fake.usage_hooks.lock().unwrap().push(Box::new(move || {
            sign_in_unnamed(&changed, "rotated");
        }));
        fake.usage.lock().unwrap().push((401, Value::Null));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        assert!(fake.refreshed.lock().unwrap().is_empty());
        assert_eq!(usages(&fake), ["new-access", "rotated-access"]);
        assert_eq!(
            app.claude_load().unwrap().accounts[0].oauth["refreshToken"],
            "rotated-refresh"
        );
    }

    #[tokio::test]
    async fn repeated_external_changes_are_saved_but_retried_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        fake.usage_hooks.lock().unwrap().extend([
            external_update(location.clone(), "alice", "first"),
            external_update(location.clone(), "alice", "second"),
        ]);
        assert_eq!(
            app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
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
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        fake.refresh_hooks.lock().unwrap().push(external_update(
            location.clone(),
            "bob",
            "bob-new",
        ));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
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
        let accounts = app.accounts(Pool::Claude).await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert!(!accounts[0].is_current);
        assert_eq!(accounts[0].plan_type.as_deref(), Some("max 5x"));
        // The only stored account is not Claude Code's, so Smart Switch takes it.
        assert_eq!(
            app.smart_switch(Pool::Claude).await.unwrap(),
            SmartSwitchOutcome::Switched {
                label: "new@example.com".into(),
                execution: SwitchExecution::default(),
            }
        );
        assert_eq!(live_access(&location), "new-access");
    }

    #[tokio::test]
    async fn a_hi_is_said_as_claude_code_in_a_terminal_session() {
        use axum::{extract::Request, routing::any, Json, Router};
        let seen = Arc::new(Mutex::new(None));
        let recorder = seen.clone();
        let router = Router::new().fallback(any(move |request: Request| {
            let recorder = recorder.clone();
            async move {
                let headers = request.headers().clone();
                let body = axum::body::to_bytes(request.into_body(), usize::MAX)
                    .await
                    .unwrap();
                let body: Value = serde_json::from_slice(&body).unwrap();
                *recorder.lock().unwrap() = Some((headers, body));
                Json(json!({"type": "message"}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let (status, _) = Http.hello(&url, "token", "acct-alice", "").await.unwrap();
        assert_eq!(status, 200);
        let (headers, body) = seen.lock().unwrap().take().unwrap();
        let header = |name: &str| headers.get(name).unwrap().to_str().unwrap().to_owned();
        assert_eq!(header("user-agent"), INTERACTIVE_USER_AGENT);
        assert_eq!(header("x-app"), "cli");
        assert_eq!(header("authorization"), "Bearer token");
        let user: Value =
            serde_json::from_str(body["metadata"]["user_id"].as_str().unwrap()).unwrap();
        assert_eq!(user["account_uuid"], "acct-alice");
        assert_eq!(user["session_id"], header("x-claude-code-session-id"));
        assert_eq!(user["device_id"].as_str().unwrap().len(), 64);
        assert_eq!(
            (body["model"].as_str(), body["max_tokens"].as_i64()),
            (Some(HELLO_MODEL), Some(1))
        );
        // Sonnet 5.5 refuses thinking turned off.
        assert!(body["thinking"].is_null());
    }

    #[tokio::test]
    async fn oauth_api_requests_follow_the_custom_oauth_url_as_claude_code_reads_it() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts.remove(0).id;
        let mut store = app.claude_load().unwrap();
        store.accounts[0].account["organizationUuid"] = "org-alice".into();
        app.claude_save(&store).unwrap();
        *fake.prepaid.lock().unwrap() = Some(json!({"amount": 0}));
        fake.usage.lock().unwrap().extend((0..5).map(|_| {
            (
                200,
                json!({"five_hour": {"utilization": 1}, "extra_usage": {}}),
            )
        }));
        let roots = |env: Option<&str>, settings: Option<Value>| {
            app.set_claude_location(Location {
                custom_oauth_env: env.map(str::to_owned),
                ..location.clone()
            });
            let text = settings.map(|env| json!({ "env": env }).to_string());
            put_settings(&location, text.as_deref()).unwrap();
            let (app, fake, id) = (&app, &fake, &id);
            async move {
                fake.api_roots.lock().unwrap().clear();
                app.refresh_usage(Pool::Claude, id).await.unwrap();
                let roots = fake.api_roots.lock().unwrap().clone();
                assert!(roots.len() >= 2, "usage and prepaid: {roots:?}");
                roots.into_iter().collect::<std::collections::BTreeSet<_>>()
            }
        };
        let only = |root: &str| std::collections::BTreeSet::from([root.to_owned()]);
        assert_eq!(roots(None, None).await, only("https://api.anthropic.com"));
        // The settings' value wins over the environment, without one slash.
        assert_eq!(
            roots(
                Some("http://env.invalid"),
                Some(json!({CUSTOM_OAUTH: "http://127.0.0.1:8787/anthropic/"}))
            )
            .await,
            only("http://127.0.0.1:8787/anthropic")
        );
        // An empty value in the settings unsets the environment's.
        assert_eq!(
            roots(Some("http://env.invalid"), Some(json!({CUSTOM_OAUTH: ""}))).await,
            only("https://api.anthropic.com")
        );
        assert_eq!(
            roots(Some("http://env.invalid/"), Some(json!({}))).await,
            only("http://env.invalid")
        );
    }

    #[tokio::test]
    async fn saying_hi_goes_where_claude_code_sends_the_login() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts.remove(0).id;
        let settings =
            |env: Value| put_settings(&location, Some(&json!({ "env": env }).to_string())).unwrap();
        async fn said(app: &Application, fake: &Fake, id: &str) -> String {
            app.say_hi(Pool::Claude, id).await.unwrap();
            fake.hello_urls.lock().unwrap().pop().unwrap()
        }
        assert_eq!(
            said(&app, &fake, &id).await,
            "https://api.anthropic.com/v1/messages"
        );
        // A relay forwarding the login, with blank credentials beside it.
        settings(json!({BASE_URL: "http://127.0.0.1:8787/anthropic/",
            "ANTHROPIC_AUTH_TOKEN": "", "ANTHROPIC_API_KEY": ""}));
        assert_eq!(
            said(&app, &fake, &id).await,
            "http://127.0.0.1:8787/anthropic/v1/messages"
        );
        // A base URL beside a credential is that credential's gateway.
        settings(json!({BASE_URL: "https://gateway.invalid", "ANTHROPIC_AUTH_TOKEN": "sk"}));
        assert_eq!(
            said(&app, &fake, &id).await,
            "https://api.anthropic.com/v1/messages"
        );
        // While a provider is applied, the official settings recorded apply.
        app.claude_record_official(&Official {
            settings: Some(json!({"env": {BASE_URL: "http://relay.invalid"}}).to_string()),
            applied: None,
        })
        .unwrap();
        let mut store = app.claude_load().unwrap();
        store.settings_provider = Some("provider".into());
        app.claude_save(&store).unwrap();
        assert_eq!(
            said(&app, &fake, &id).await,
            "http://relay.invalid/v1/messages"
        );
    }

    #[tokio::test]
    async fn saying_hi_goes_through_the_account_proxy_and_then_shows_the_usage() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts.remove(0).id;
        app.save_account_proxy(Pool::Claude, &id, "socks5://127.0.0.1:7891")
            .await
            .unwrap();
        let usages = fake.usage_tokens.lock().unwrap().len();
        app.say_hi(Pool::Claude, &id).await.unwrap();
        assert_eq!(
            *fake.hello_requests.lock().unwrap(),
            vec![("alice-access".into(), "socks5://127.0.0.1:7891".into())]
        );
        // The usage, which shows the window only later, is left to the card.
        assert_eq!(fake.usage_tokens.lock().unwrap().len(), usages);
        assert_eq!(*fake.hello_accounts.lock().unwrap(), vec!["acct-alice"]);

        // A login Claude does not accept is renewed and says hi again.
        fake.hello_requests.lock().unwrap().clear();
        fake.hellos.lock().unwrap().push((401, Value::Null));
        fake.usage.lock().unwrap().push((401, Value::Null));
        app.say_hi(Pool::Claude, &id).await.unwrap();
        let tokens: Vec<String> = fake
            .hello_requests
            .lock()
            .unwrap()
            .iter()
            .map(|(token, _)| token.clone())
            .collect();
        assert_eq!(tokens, vec!["alice-access", "fresh-access"]);

        // Other failures are reported without retrying.
        fake.hello_requests.lock().unwrap().clear();
        fake.hellos
            .lock()
            .unwrap()
            .push((529, json!({"error": {"message": "Overloaded"}})));
        assert_eq!(
            app.say_hi(Pool::Claude, &id).await.unwrap_err(),
            "Claude message request failed: HTTP 529 Overloaded"
        );
        assert_eq!(fake.hello_requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn account_proxy_routes_imports_and_refreshes() {
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
        // The import's requests take the shared proxy, which the new
        // account does not keep as its own.
        assert_eq!(
            *fake.profile_proxies.lock().unwrap(),
            vec!["http://127.0.0.1:7890"]
        );
        assert_eq!(
            *fake.usage_proxies.lock().unwrap(),
            vec!["http://127.0.0.1:7890"]
        );
        let account = app.claude_load().unwrap().accounts.remove(0);
        assert!(account.proxy_url.is_empty());
        app.refresh_usage(Pool::Claude, &account.id).await.unwrap();
        assert_eq!(
            fake.usage_proxies.lock().unwrap().last().unwrap(),
            "http://127.0.0.1:7890"
        );
        app.save_account_proxy(Pool::Claude, &account.id, "socks5://127.0.0.1:7891")
            .await
            .unwrap();
        app.refresh_usage(Pool::Claude, &account.id).await.unwrap();
        assert_eq!(
            fake.usage_proxies.lock().unwrap().last().unwrap(),
            "socks5://127.0.0.1:7891"
        );
        let mut preferences = app.preferences().unwrap();
        preferences.account_proxy_url.clear();
        app.save_preferences(preferences).await.unwrap();
        app.refresh_usage(Pool::Claude, &account.id).await.unwrap();
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
        app.save_account_proxy(Pool::Claude, &alice, "http://127.0.0.1:8001")
            .await
            .unwrap();
        let (oauth, account) = login("new", LATER);
        let (id, _, _) = app
            .claude_upsert(oauth, account, false, false)
            .await
            .unwrap();
        app.save_account_proxy(Pool::Claude, &id, "socks5://127.0.0.1:8002")
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
            app.save_account_proxy(Pool::Claude, &id, "http://127.0.0.1:8002")
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
        app.save_account_proxy(Pool::Claude, &id, "http://127.0.0.1:8001")
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
    async fn a_nameless_external_rotation_only_updates_the_profile_matched_card() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        for user in ["new", "other"] {
            sign_in(&location, user, LATER);
            app.claude_import_current().await.unwrap();
        }
        let mut preferences = app.preferences().unwrap();
        preferences.account_proxy_url = "http://127.0.0.1:8787".into();
        app.save_preferences(preferences).await.unwrap();
        fake.profile_proxies.lock().unwrap().clear();
        sign_in_unnamed(&location, "rotated");
        let cards = app.accounts(Pool::Claude).await.unwrap();
        for user in ["other", "new"] {
            let card = cards
                .iter()
                .find(|a| a.label == format!("{user}@example.com"))
                .unwrap();
            app.refresh_usage(Pool::Claude, &card.id).await.unwrap();
            let saved = app
                .claude_load()
                .unwrap()
                .accounts
                .into_iter()
                .find(|a| a.id == card.id)
                .unwrap();
            let expected = if user == "new" {
                "rotated-access"
            } else {
                "other-access"
            };
            assert_eq!(saved.oauth["accessToken"], expected);
            assert_eq!(fake.usage_tokens.lock().unwrap().last().unwrap(), expected);
        }
        let checked = fake.profile_proxies.lock().unwrap();
        assert!(!checked.is_empty());
        assert!(checked.iter().all(|p| p == "http://127.0.0.1:8787"));
    }

    #[tokio::test]
    async fn cards_carry_the_access_token_expiry_in_seconds() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, _) = waiting_app(&dir);
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let accounts = app.accounts(Pool::Claude).await.unwrap();
        assert_eq!(accounts[0].token_expires_at, Some(LATER / 1000));
    }

    #[tokio::test]
    async fn an_unnamed_login_is_imported_and_probed_on_its_first_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in_unnamed(&location, "probe");
        assert_eq!(app.claude_import_current().await.unwrap(), "Claude Code");
        app.claude_import_current().await.unwrap();
        let accounts = app.accounts(Pool::Claude).await.unwrap();
        assert_eq!(accounts.len(), 1);
        let card = &accounts[0];
        assert!(card.is_current && card.awaiting_first_request && card.email.is_none());
        assert!(fake.profile_proxies.lock().unwrap().is_empty());

        app.save_account_proxy(Pool::Claude, &card.id, "http://127.0.0.1:8003")
            .await
            .unwrap();
        app.release_new_accounts(Pool::Claude, Some(&card.id))
            .await
            .unwrap();
        app.refresh_usage(Pool::Claude, &card.id).await.unwrap();
        assert_eq!(
            *fake.profile_proxies.lock().unwrap(),
            vec!["http://127.0.0.1:8003"]
        );
        let accounts = app.accounts(Pool::Claude).await.unwrap();
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
        assert_eq!(app.accounts(Pool::Claude).await.unwrap().len(), 2);
        let mut pool = app.claude_load().unwrap();
        let probe = pool
            .accounts
            .iter_mut()
            .find(|a| a.oauth["accessToken"] == "probe-access")
            .unwrap();
        probe.alternates = vec![json!({"accessToken": "old", "refreshToken": "probe-old"})];
        probe.rejected = vec![print("probe-rejected")];
        app.claude_save(&pool).unwrap();
        app.release_new_accounts(Pool::Claude, None).await.unwrap();
        app.refresh_all(Pool::Claude).await.unwrap();
        let accounts = app.accounts(Pool::Claude).await.unwrap();
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
        app.refresh_recurring(Pool::Claude, 0).await.unwrap();
        let accounts = app.accounts(Pool::Claude).await.unwrap();
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
        app.save_account_proxy(Pool::Claude, &new.id, "socks5://127.0.0.1:8002")
            .await
            .unwrap();
        app.release_new_accounts(Pool::Claude, Some(&new.id))
            .await
            .unwrap();
        app.refresh_usage(Pool::Claude, &new.id).await.unwrap();
        assert_eq!(
            *fake.profile_proxies.lock().unwrap(),
            vec!["socks5://127.0.0.1:8002"]
        );
        let accounts = app.accounts(Pool::Claude).await.unwrap();
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
        assert!(plain.vault);
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
        // Any other endpoint leaves Claude Code its file alone; the pool's
        // tokens still go to Copool's own Keychain item.
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
            assert!(file_only.vault);
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
    fn the_pool_keeps_its_keychain_item_whatever_claude_code_uses() {
        let in_settings = format!(r#"{{"env":{{"CLAUDE_CODE_CUSTOM_OAUTH_URL":"{CUSTOM}"}}}}"#);
        let (keychain, _) = resolved(&[], None);
        let (file_only, _) = resolved(&[], Some(&in_settings));
        assert!(keychain.keychain.is_some() && file_only.keychain.is_none());
        let on_macos = cfg!(target_os = "macos");
        assert_eq!(uses_keychain_vault(&keychain), on_macos);
        // Claude Code's login in its file does not take the pool's item away.
        assert_eq!(uses_keychain_vault(&file_only), on_macos);
        assert!(!uses_keychain_vault(&Location::files("/smoke".into())));
        let home = tempfile::tempdir().unwrap();
        let elsewhere = Location::resolve(&|_| None, home.path().into(), false);
        assert!(!uses_keychain_vault(&elsewhere));
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
        assert!(!linux.vault);
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
            vault: false,
            custom_oauth_env: None,
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

    /// An Anthropic API stand-in: it records each request (path and query,
    /// credential, betas, body) and answers the queued statuses, then 200.
    async fn messages_upstream(
        statuses: Arc<Mutex<Vec<u16>>>,
    ) -> (String, Arc<Mutex<Vec<[String; 4]>>>) {
        use axum::{body::to_bytes, extract::Request, routing::any, Router};
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let router = Router::new().fallback(any(move |request: Request| {
            let (log, statuses) = (log.clone(), statuses.clone());
            async move {
                let (parts, body) = request.into_parts();
                let header = |name: &str| {
                    parts
                        .headers
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned()
                };
                let credential = match header("x-api-key") {
                    key if !key.is_empty() => format!("x-api-key {key}"),
                    _ => header("authorization"),
                };
                let beta = header("anthropic-beta");
                let path = parts.uri.path_and_query().unwrap().to_string();
                let body = to_bytes(body, usize::MAX).await.unwrap();
                log.lock().unwrap().push([
                    path,
                    credential,
                    beta,
                    String::from_utf8_lossy(&body).into_owned(),
                ]);
                let status = {
                    let mut queued = statuses.lock().unwrap();
                    if queued.is_empty() { 200 } else { queued.remove(0) }
                };
                let body = if status == 200 {
                    json!({"type": "message", "content": [{"type": "text", "text": "hi"}]})
                } else {
                    json!({"type": "error", "error": {"type": "rate_limit_error", "message": "rate limit reached"}})
                };
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    [("content-type", "application/json")],
                    body.to_string(),
                )
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (base, seen)
    }
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }
    /// Two pool logins (Bob in use) and the Claude proxy started in front of
    /// the stand-in, which Claude Code's official settings name.
    async fn claude_proxy(
        dir: &tempfile::TempDir,
        statuses: Arc<Mutex<Vec<u16>>>,
    ) -> (
        Arc<Application>,
        Location,
        Arc<Fake>,
        Arc<Mutex<Vec<[String; 4]>>>,
        crate::ApiProxyStatus,
    ) {
        let (app, location, fake) = app(dir).await;
        let app = app.into_shared();
        let (base, seen) = messages_upstream(statuses).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        sign_in(&location, "bob", LATER);
        app.claude_import_current().await.unwrap();
        fs::write(
            settings_file(&location),
            json!({"env": {BASE_URL: base}}).to_string(),
        )
        .unwrap();
        let mut preferences = app.preferences().unwrap();
        preferences.claude_proxy_port = free_port();
        app.save_preferences(preferences).await.unwrap();
        let status = app.start_proxy(Pool::Claude).await.unwrap();
        (app, location, fake, seen, status)
    }
    async fn send(status: &crate::ApiProxyStatus, key: &str) -> (u16, Value) {
        let response = reqwest::Client::new()
            .post(format!(
                "{}/v1/messages?beta=true",
                status.base_url.as_deref().unwrap()
            ))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "claude-code-20250219")
            .body(r#"{"model":"claude-sonnet-5-5","max_tokens":1,"messages":[]}"#)
            .send()
            .await
            .unwrap();
        let code = response.status().as_u16();
        (code, response.json().await.unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn the_claude_proxy_passes_messages_through_with_the_login_in_use() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _, _, seen, status) = claude_proxy(&dir, Default::default()).await;
        let key = status.api_key.clone().unwrap();
        assert!(status.running);
        // Claude Code takes `/anthropic` as its base URL and adds the rest.
        assert!(status.base_url.as_deref().unwrap().ends_with("/anthropic"));
        assert_eq!(
            app.proxy_status(Pool::Claude)
                .await
                .unwrap()
                .available_accounts,
            2
        );
        // Independent of the Codex proxy, which stays stopped.
        assert!(!app.proxy_status(Pool::Codex).await.unwrap().running);
        assert_ne!(
            app.proxy_status(Pool::Codex).await.unwrap().api_key,
            Some(key.clone())
        );
        let (code, body) = send(&status, &key).await;
        assert_eq!(
            (code, body["content"][0]["text"].as_str()),
            (200, Some("hi"))
        );
        let [path, credential, beta, body] = seen.lock().unwrap()[0].clone();
        assert_eq!(path, "/v1/messages?beta=true");
        assert_eq!(credential, "Bearer bob-access");
        assert_eq!(beta, "claude-code-20250219,oauth-2025-04-20");
        assert!(body.contains("claude-sonnet-5-5"));
        assert_eq!(
            app.proxy_status(Pool::Claude)
                .await
                .unwrap()
                .active_account_id
                .as_deref(),
            Some("acct-bob")
        );
        // A wrong key reaches no account.
        assert_eq!(send(&status, "sk-wrong").await.0, 401);
        assert_eq!(seen.lock().unwrap().len(), 1);
        app.stop_proxy(Pool::Claude).await.unwrap();
    }

    #[tokio::test]
    async fn a_rate_limited_login_fails_over_and_claude_code_follows_unless_hosting_only() {
        let dir = tempfile::tempdir().unwrap();
        let statuses = Arc::new(Mutex::new(vec![429]));
        let (app, location, _, seen, status) = claude_proxy(&dir, statuses.clone()).await;
        let key = status.api_key.clone().unwrap();
        assert_eq!(send(&status, &key).await.0, 200);
        let credentials: Vec<String> = seen.lock().unwrap().iter().map(|r| r[1].clone()).collect();
        assert_eq!(credentials, ["Bearer bob-access", "Bearer alice-access"]);
        // As the Codex proxy's account, the one serving becomes Claude Code's.
        assert_eq!(live_access(&location), "alice-access");
        // Bob cools down; Alice keeps serving.
        assert_eq!(send(&status, &key).await.0, 200);
        assert_eq!(
            seen.lock().unwrap().last().unwrap()[1],
            "Bearer alice-access"
        );
        // Hosting the API only, the proxy leaves Claude Code's login alone.
        let mut preferences = app.preferences().unwrap();
        preferences.host_api_only = true;
        app.save_preferences(preferences).await.unwrap();
        sign_in(&location, "bob", LATER);
        app.accounts(Pool::Claude).await.unwrap();
        assert_eq!(send(&status, &key).await.0, 200);
        assert_eq!(live_access(&location), "bob-access");
        app.stop_proxy(Pool::Claude).await.unwrap();
    }

    #[tokio::test]
    async fn a_refused_login_is_renewed_through_the_pool_and_retried() {
        let dir = tempfile::tempdir().unwrap();
        let statuses = Arc::new(Mutex::new(vec![401]));
        let (app, location, fake, seen, status) = claude_proxy(&dir, statuses).await;
        let key = status.api_key.clone().unwrap();
        assert_eq!(send(&status, &key).await.0, 200);
        let credentials: Vec<String> = seen.lock().unwrap().iter().map(|r| r[1].clone()).collect();
        assert_eq!(credentials, ["Bearer bob-access", "Bearer fresh-access"]);
        assert_eq!(*fake.refreshed.lock().unwrap(), ["bob-refresh"]);
        // The pool keeps the renewal, and Claude Code's login with it.
        assert_eq!(live_access(&location), "fresh-access");
        let bob = app
            .claude_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| a.label.contains("bob"))
            .unwrap();
        assert_eq!(bob.oauth["accessToken"], "fresh-access");
        app.stop_proxy(Pool::Claude).await.unwrap();
    }

    #[tokio::test]
    async fn remote_servers_get_the_official_logins_and_their_renewals_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, _) = app(&dir).await;
        sign_in(&location, "alice", LATER);
        app.claude_import_current().await.unwrap();
        let payload: Value = serde_json::from_slice(&app.claude_remote_payload().unwrap()).unwrap();
        let accounts = payload["accounts"].as_array().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(
            accounts[0]["authJson"]["claudeAiOauth"]["accessToken"],
            "alice-access"
        );
        assert!(accounts[0].get("copoolCurrent").is_none());
        // The daemon renewed the login: the pool takes the later generation.
        let mut remote = payload.clone();
        remote["accounts"][0]["authJson"]["claudeAiOauth"] = json!({
            "accessToken": "remote-access", "refreshToken": "remote-refresh", "expiresAt": LATER + 1
        });
        app.claude_adopt_remote_logins(&remote).await.unwrap();
        assert_eq!(
            app.claude_load().unwrap().accounts[0].oauth["accessToken"],
            "remote-access"
        );
        // An older one is not.
        remote["accounts"][0]["authJson"]["claudeAiOauth"] = json!({
            "accessToken": "stale-access", "refreshToken": "stale-refresh", "expiresAt": 1
        });
        app.claude_adopt_remote_logins(&remote).await.unwrap();
        assert_eq!(
            app.claude_load().unwrap().accounts[0].oauth["accessToken"],
            "remote-access"
        );
    }
}
