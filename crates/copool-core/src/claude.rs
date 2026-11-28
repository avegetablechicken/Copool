//! Claude Code account pool: the logins Copool keeps, the one Claude Code uses
//! (the macOS Keychain or `.credentials.json`, with `oauthAccount` in
//! `.claude.json`), OAuth usage and switching.
use crate::login_store::{Account, Asked, LoginPool, Logins, Origin, RenewalLock};
use crate::{
    account_network::Response,
    application::{Application, SwitchExecution},
    claude_api::{
        number, refresh_at, request_error, reset_at, text, usage_snapshot, UsageResponse, API_ROOT,
        SIGN_IN_EXPIRED, TOKEN_REFRESH_RATE_LIMITED, USAGE_USER_AGENT, USER_AGENT,
    },
    models::{AccountSummary, CreditSnapshot, Renewal, UsageSnapshot},
    persistence::write_json,
    pool::{Hello, Pool, PoolAccount},
    pool_login::{
        self,
        claude::{
            apply_tokens, expires_at, exported_oauth, identity, label_of, parse_credentials, plan,
            profile_account, refresh_scope, same_login, same_tokens,
        },
    },
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
pub(crate) use sub2api::ProviderAdapter;

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
/// The provider Claude Code uses: under settings other than the official
/// ones, the provider last applied; else none, its login.
fn claude_provider<'a>(store: &'a Store, location: &Location) -> Option<&'a str> {
    store
        .settings_provider
        .as_deref()
        .filter(|_| !settings_are_official(location))
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
/// Each account's tokens, by account id.
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
        .any(|key| text(&account.document[*key]).is_some())
}
fn secrets_of(account: &Stored) -> Option<Value> {
    holds_tokens(account).then(|| {
        json!({
            "accessToken": account.document["accessToken"],
            "refreshToken": account.document["refreshToken"],
        })
    })
}
fn restore_secrets(account: &mut Stored, secrets: &Value) {
    for key in SECRET_KEYS {
        if !secrets[key].is_null() {
            account.document[key] = secrets[key].clone();
        }
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
    if let Some(oauth) = account.document.as_object_mut() {
        for key in SECRET_KEYS {
            oauth.remove(key);
        }
    }
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
    /// The store's locks.
    pub(crate) logins: crate::login_store::Locks,
    /// Claude Code's login as its profile named it: by access token, the
    /// account it names.
    named: std::sync::Mutex<std::collections::BTreeMap<String, Value>>,
    /// The access token whose profile could not be asked last, and when.
    unconfirmed: std::sync::Mutex<Option<(String, i64)>>,
    /// Reads of Claude Code's login answer this late (tests).
    #[cfg(test)]
    live_read_delay_ms: std::sync::atomic::AtomicU64,
    /// Where a provider given by its Sub2api URL finds the key its variable
    /// holds: with `now`, as a shell opened now exports it.
    environment: std::sync::RwLock<Environment>,
}
type Environment = Arc<dyn Fn(&str, bool) -> Option<String> + Send + Sync>;
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
            logins: Default::default(),
            named: Default::default(),
            unconfirmed: Default::default(),
            #[cfg(test)]
            live_read_delay_ms: Default::default(),
            #[cfg(not(test))]
            environment: std::sync::RwLock::new(Arc::new(|name: &str, now: bool| {
                use crate::proxy_integration::{environment_key, new_shell_keys, Shell};
                if now {
                    new_shell_keys(&[name], Shell::Interactive)
                        .into_iter()
                        .next()
                        .flatten()
                } else {
                    environment_key(name, Shell::Interactive)
                }
            })),
            // Tests never ask the user's shell.
            #[cfg(test)]
            environment: std::sync::RwLock::new(Arc::new(|_: &str, _: bool| None)),
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
/// What Claude adds to an account.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Own {
    /// Claude Code's `oauthAccount`, written to `.claude.json` on a switch.
    #[serde(default)]
    account: Value,
    /// A local backoff after the usage or token endpoint returns HTTP 429.
    #[serde(default)]
    usage_retry_at: Option<i64>,
    /// Consecutive HTTP 429 responses, each doubling the pause.
    #[serde(default, skip_serializing_if = "is_zero")]
    rate_limits: u32,
}
/// An account; its `document` is Claude Code's `claudeAiOauth` entry.
type Stored = Account<Own>;

/// What a refresh's usage request learned.
struct Answer {
    usage: UsageSnapshot,
    /// The account its profile named, when asked.
    profiled: Option<Value>,
    /// Its plan, for its login: `subscriptionType` and `rateLimitTier`.
    plan: Vec<(&'static str, Value)>,
}
/// Adds a profile's plan to what a login learns.
fn learn(
    plan: &mut Vec<(&'static str, Value)>,
    subscription: Option<String>,
    tier: Option<String>,
) {
    if let Some(subscription) = subscription {
        plan.push(("subscriptionType", subscription.into()));
    }
    if let Some(tier) = tier {
        plan.push(("rateLimitTier", tier.into()));
    }
}

/// Uses a later generation of the account's login seen elsewhere: its
/// access token lasts longer, or the account holds none (its vault unread).
/// Returns whether the account changed.
fn adopt(account: &mut Stored, oauth: &Value) -> bool {
    if same_login(&account.document, oauth) || !ClaudeLogins::newer(oauth, &account.document) {
        return false;
    }
    account.document = oauth.clone();
    account.usage_error = None;
    true
}
fn summary(account: &Stored, current: bool) -> AccountSummary {
    Logins::<ClaudeLogins>::summary(account, current)
}

/// The login Claude Code uses.
#[derive(Clone)]
struct Live {
    /// The complete credentials document; other entries (MCP logins) are kept.
    document: Value,
    account: Option<Value>,
    held: Held,
    /// `account` is the one its profile named, not Claude Code's.
    inferred: bool,
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

/// How long a login whose profile could not be asked waits to ask again,
/// in seconds.
const UNCONFIRMED_RETRY: i64 = 60;
/// How long its profile is waited for.
const CONFIRM_WAIT: std::time::Duration = if cfg!(test) {
    std::time::Duration::from_millis(300)
} else {
    std::time::Duration::from_secs(5)
};
const CLAUDE_CODE_UNCONFIRMED: &str =
    "Cannot confirm which account Claude Code's login is; retry once Claude can be reached";

/// The store's file, and the one it was first copied from.
const STORE_FILE: &str = "claude-pool.json";
const EARLIER_STORE_FILE: &str = "claude-accounts.json";
/// Copies the accounts an earlier version kept in `claude-accounts.json`
/// to the store's file once, its `oauth` as `document`. The earlier file
/// stays as it was, for that version to read; the tokens the vault keeps,
/// by account id, serve both. One that cannot be read is left behind, as
/// Codex's is: the pool starts empty, and Copool opens.
pub(crate) fn migrate(paths: &crate::Paths) -> Result<()> {
    let target = paths.data_dir.join(STORE_FILE);
    let source = paths.data_dir.join(EARLIER_STORE_FILE);
    if target.exists() || !source.exists() {
        return Ok(());
    }
    let bytes = fs::read(&source).map_err(|e| e.to_string())?;
    let Ok(mut store) = serde_json::from_slice::<Value>(&bytes) else {
        log::warn!("{EARLIER_STORE_FILE} cannot be read; the Claude pool starts empty");
        return Ok(());
    };
    for account in store["accounts"].as_array_mut().into_iter().flatten() {
        if let Some(account) = account.as_object_mut() {
            if let Some(oauth) = account.remove("oauth") {
                account.insert("document".into(), oauth);
            }
        }
    }
    // As the store reads it: what it no longer keeps stays behind.
    let store = match serde_json::from_value::<Store>(store) {
        Ok(store) => store,
        Err(error) => {
            log::warn!(
                "{EARLIER_STORE_FILE} cannot be read ({error}); the Claude pool starts empty"
            );
            return Ok(());
        }
    };
    write_json(&target, &store)
}

/// The Claude pool's facts, for its login store: Claude Code's login is its
/// credentials (the Keychain item, else `.credentials.json`) with its
/// `oauthAccount` (in `.claude.json`); the pool's tokens are kept in
/// Copool's vault where there is one; Claude Code renews under its lock.
struct ClaudeLogins<'a>(&'a Application);
impl LoginPool for ClaudeLogins<'_> {
    type Own = Own;
    type Store = Store;
    type Login = Live;
    /// The token response, and when it arrived, in milliseconds.
    type Renewed = (Value, i64);
    const FILE: &'static str = STORE_FILE;
    const NAME: &'static str = "Claude";
    const UNNAMED: &'static str = "Claude Code";
    const SIGN_IN_EXPIRED: &'static str = SIGN_IN_EXPIRED;
    const LIVE_UNREADABLE: &'static str = LIVE_UNREADABLE;

    fn app(&self) -> &Application {
        self.0
    }
    fn locks(&self) -> &crate::login_store::Locks {
        &self.0.claude.logins
    }
    fn accounts(store: &Store) -> &Vec<Stored> {
        &store.accounts
    }
    fn accounts_mut(store: &mut Store) -> &mut Vec<Stored> {
        &mut store.accounts
    }
    fn current(store: &mut Store) -> &mut Option<String> {
        &mut store.current_account_id
    }
    /// Claude Code's login, named by its profile when Claude Code keeps
    /// no `oauthAccount` and a refresh asked.
    /// One whose account cannot be confirmed yet is not read: it is
    /// followed, and the account it may be renewed, only once it is.
    async fn read_live(&self, access: KeychainAccess) -> Result<Option<Live>> {
        match self.0.claude_live(access).await? {
            Some(Ok(live)) => Ok(Some(live)),
            Some(Err(_)) => Err(CLAUDE_CODE_UNCONFIRMED.into()),
            None => Ok(None),
        }
    }
    /// Gives Claude Code the login where it read the one replaced, keeping
    /// the rest of its credentials, then its `oauthAccount`, unless the
    /// account is the one it names already; a failed write puts the
    /// credentials back, a failure to write the file beside the Keychain
    /// item too.
    async fn write_live(&self, login: &Live, access: KeychainAccess) -> Result<()> {
        let location = self.0.claude_location();
        let current = read_live_with_access(&location, access).await?;
        let previous = current.as_ref().map(|c| c.document.clone());
        let held = current.as_ref().map(|c| c.held);
        let mut document = previous.clone().unwrap_or_else(|| json!({}));
        document["claudeAiOauth"] = Self::document(login).clone();
        let rollback = |error: String| async {
            let restored = match &previous {
                Some(previous) => write_credentials(&location, previous, held, access).await,
                None => remove_credentials(&location, access).await,
            };
            restored
                .map_err(|rollback| format!("{error}; rollback failed: {rollback}"))
                .and(Err(error))
        };
        if let Err(error) = write_credentials(&location, &document, held, access).await {
            return rollback(error).await;
        }
        let previous_account = current.as_ref().and_then(|c| c.account.clone());
        if login.inferred || login.account == previous_account {
            return Ok(());
        }
        if let Err(error) = write_account(&location, login.account.as_ref().unwrap_or(&Value::Null))
        {
            return rollback(error).await;
        }
        Ok(())
    }
    fn document(login: &Live) -> &Value {
        &login.document["claudeAiOauth"]
    }
    fn document_mut(login: &mut Live) -> &mut Value {
        if !login.document.is_object() {
            login.document = json!({});
        }
        &mut login.document["claudeAiOauth"]
    }
    fn login_of(account: &Stored) -> Live {
        Live {
            document: json!({"claudeAiOauth": account.document}),
            account: identity(&account.own.account).map(|_| account.own.account.clone()),
            // Written where Claude Code's login is read.
            held: Held::File,
            inferred: false,
        }
    }
    /// Keeps the login's tokens, and its `oauthAccount` once it names one.
    fn keep(account: &mut Stored, login: Live) {
        account.document = Self::document(&login).clone();
        if let Some(named) = login.account.filter(|a| identity(a).is_some()) {
            account.own.account = named;
        }
    }
    fn access_token(document: &Value) -> Option<String> {
        text(&document["accessToken"])
    }
    fn refresh_token(document: &Value) -> Option<String> {
        text(&document["refreshToken"])
    }
    fn expiry(document: &Value) -> Option<i64> {
        pool_login::claude::expires_at_ms(document)
            .filter(|ms| *ms > 0)
            .map(|ms| ms / 1000)
    }
    /// The login names its account beside its tokens.
    fn email(_: &Value) -> Option<String> {
        None
    }
    fn login_email(login: &Live) -> Option<String> {
        login
            .account
            .as_ref()
            .and_then(|a| text(&a["emailAddress"]))
    }
    fn login_name(login: &Live) -> Option<String> {
        Some(label_of(login.account.as_ref().unwrap_or(&Value::Null)))
    }
    fn same_login(a: &Value, b: &Value) -> bool {
        same_login(a, b)
    }
    /// Its token lasts longer, or the other holds none (its vault unread).
    fn newer(a: &Value, b: &Value) -> bool {
        expires_at(a) > expires_at(b)
            || text(&b["accessToken"]).is_none() && text(&a["accessToken"]).is_some()
    }
    fn summarize(account: &Stored, summary: &mut AccountSummary) {
        summary.team_name = text(&account.own.account["organizationName"]);
        summary.email = text(&account.own.account["emailAddress"]);
        summary.plan_type = plan(&account.document);
    }
    /// By the account its login names; a login naming none, by its tokens.
    fn same_account(account: &Stored, login: &Live) -> bool {
        match login.account.as_ref().and_then(identity) {
            Some(named) => identity(&account.own.account).as_ref() == Some(&named),
            None => same_tokens(&account.document, Self::document(login)),
        }
    }
    fn account_id(account: &Stored) -> String {
        text(&account.own.account["accountUuid"]).unwrap_or_default()
    }
    async fn renew(&self, document: &mut Value, proxy: &str) -> Result<(Value, i64)> {
        let token = text(&document["refreshToken"]).ok_or(SIGN_IN_EXPIRED)?;
        let scope = refresh_scope(document);
        let tokens = self
            .0
            .claude_network()
            .refresh(&token, &scope, proxy)
            .await?;
        let at = now_unix_seconds() * 1000;
        apply_tokens(document, &tokens, at)?;
        Ok((tokens, at))
    }
    fn apply(renewed: &(Value, i64), document: &mut Value) {
        // Checked when the renewal arrived.
        let _ = apply_tokens(document, &renewed.0, renewed.1);
    }
    fn opened(&self, store: &mut Store) -> Result<()> {
        self.0.claude_restore(store);
        Ok(())
    }
    fn sealed(&self, store: &Store) -> Result<Value> {
        self.0.claude_sealed(store)
    }
    /// Where Claude Code keeps its login in the Keychain, the login may be
    /// kept without its directory, which Claude Code creates to lock it:
    /// it is created only while Claude Code keeps a login there, or may.
    /// Elsewhere the login is kept in that directory.
    async fn renewal_lock(&self) -> Result<Option<Box<dyn RenewalLock>>> {
        let location = self.0.claude_location();
        let login_kept = location.keychain.is_some()
            && (location.storage_dir.is_dir() || !matches!(read_live(&location).await, Ok(None)));
        Ok(claude_code_lock(&location, login_kept)
            .await?
            .map(|lock| Box::new(ClaudeCodeLock(lock)) as Box<dyn RenewalLock>))
    }
    /// Its credentials and `oauthAccount`, as they were.
    fn backed_up(login: &Live) -> Value {
        json!({"credentials": login.document, "oauthAccount": login.account})
    }
}
/// Claude Code's refresh lock, while held.
struct ClaudeCodeLock(refresh_lock::RefreshLock);
impl RenewalLock for ClaudeCodeLock {
    /// Taken over as a dead holder's: Claude Code may be renewing now.
    fn held(&self) -> Result<()> {
        if self.0.compromised() {
            return Err(CLAUDE_CODE_RENEWING.into());
        }
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn is_zero(count: &u32) -> bool {
    *count == 0
}
/// Folds an account just identified into the pool's card for the same
/// account: the newer tokens and this usage stay, as does the selection.
fn merge_identified(store: &mut Store, id: &str) {
    let Some(index) = store.accounts.iter().position(|a| a.id == id) else {
        return;
    };
    let key = identity(&store.accounts[index].own.account);
    let Some(other) = store
        .accounts
        .iter()
        .position(|a| a.id != id && key.is_some() && identity(&a.own.account) == key)
    else {
        return;
    };
    let merged = store.accounts.remove(index);
    let other = if other > index { other - 1 } else { other };
    let existing = &mut store.accounts[other];
    adopt(existing, &merged.document);
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
        inferred: false,
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
const CLAUDE_CODE_RENEWING_SWITCH: &str = "Claude Code is renewing its login; retry the switch";
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
            Err(error) => {
                log::warn!("Claude Code's refresh lock could not be taken: {error}");
                Err(CLAUDE_CODE_LOCK_UNAVAILABLE.to_owned())
            }
        }
    })
    .await
    .map_err(|_| CLAUDE_CODE_LOCK_UNAVAILABLE.to_owned())?
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

use crate::models::RENEWAL_OFF;
/// Renewing rotates the refresh token: the login Claude Code may be using is
/// not renewed unless the new tokens can be seen to reach it.
/// Nor while it is not yet confirmed whose it is.
const LIVE_UNREADABLE: &str = "Cannot read or confirm the Claude Code login; allow access, or retry once Claude can be reached, before renewing the account it uses";

impl Application {
    fn claude_logins(&self) -> Logins<ClaudeLogins<'_>> {
        Logins(ClaudeLogins(self))
    }
    fn claude_store_path(&self) -> PathBuf {
        self.paths.data_dir.join(STORE_FILE)
    }
    /// The store with each account's tokens: those the store itself holds
    /// (written while the vault could not be), else the vault's. An account
    /// whose tokens cannot be read is returned without them.
    fn claude_load(&self) -> Result<Store> {
        self.claude_logins().load()
    }
    /// Puts the vault's tokens back in the accounts without them.
    fn claude_restore(&self, store: &mut Store) {
        let Some(vault) = self.claude_vault() else {
            return;
        };
        let moved = moved_ids(store);
        if moved.is_empty() {
            return;
        }
        if let Some(secrets) = self.claude_secrets(&*vault, KeychainAccess::Background, &moved) {
            for account in store.accounts.iter_mut().filter(|a| !holds_tokens(a)) {
                if let Some(found) = secrets.get(&account.id) {
                    restore_secrets(account, found);
                }
            }
        }
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
    /// Saves the store, its tokens in the vault.
    fn claude_save(&self, store: &Store) -> Result<()> {
        self.claude_logins().save(store)
    }
    /// The store as its file keeps it: its tokens in the vault, which keeps
    /// an account's tokens that could not be read; when the vault cannot be
    /// written, the tokens stay in the file so that none is lost.
    fn claude_sealed(&self, store: &Store) -> Result<Value> {
        let vaulted = match self.claude_vault() {
            Some(vault) => self.claude_vault_save(&*vault, store),
            None => false,
        };
        let value = if vaulted {
            let mut file = store.clone();
            file.accounts.iter_mut().for_each(strip_secrets);
            serde_json::to_value(&file)
        } else {
            serde_json::to_value(store)
        };
        value.map_err(|e| e.to_string())
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
    /// Until when an account's requests pause after HTTP 429; turning the
    /// card's slow-down off lifts a pause under way.
    fn claude_pause(&self, account: &Stored) -> Option<i64> {
        account
            .own
            .usage_retry_at
            .filter(|_| self.card_refresh(Pool::Claude, &account.id).backoff)
    }
    /// The provider Claude Code uses now, if any.
    pub(crate) fn claude_provider_in_use(&self) -> Option<String> {
        let store = self.claude_load().ok()?;
        claude_provider(&store, &self.claude_location()).map(str::to_owned)
    }
    /// Each account's rate limit pause, by id, for the refresh plan.
    pub(crate) fn claude_pauses(&self) -> std::collections::BTreeMap<String, i64> {
        self.claude_load()
            .map(|store| {
                store
                    .accounts
                    .iter()
                    .filter_map(|a| Some((a.id.clone(), self.claude_pause(a)?)))
                    .collect()
            })
            .unwrap_or_default()
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
        let _lock = self.claude.logins.store.lock().await;
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
    fn set_claude_environment(&self, environment: Environment) {
        *self.claude.environment.write().unwrap() = environment;
    }
    /// A variable's value in Claude Code's environment, asked off the
    /// runtime: the shell may take seconds.
    fn claude_exported_now(
        &self,
        name: &str,
    ) -> impl std::future::Future<Output = Result<Option<String>>> {
        let environment = self.claude.environment.read().unwrap().clone();
        let name = name.to_owned();
        async move {
            tokio::task::spawn_blocking(move || environment(&name, true))
                .await
                .map_err(|e| e.to_string())
        }
    }
    /// A variable's value as the user's shell exports it, cached.
    fn claude_exported(&self, name: &str) -> Option<String> {
        let environment = self.claude.environment.read().unwrap().clone();
        environment(name, false)
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
        let _op = self.claude.logins.operations.lock().await;
        // A refusal leaves the tokens where they are; refreshes report it.
        if let Err(error) = self.claude_unlock().await {
            log::warn!("Copool's Keychain item could not be read: {error}");
        }
        let logins = self.claude_logins();
        let live = logins.0.read_live(KeychainAccess::Interactive).await?;
        let _lock = self.claude.logins.store.lock().await;
        let mut store = logins.load()?;
        if Logins::<ClaudeLogins>::follow_live(&mut store, live.as_ref()) {
            logins.save(&store)?;
        }
        Ok(())
    }
    /// The pool's cards as stored, without reading Claude Code's login.
    pub(crate) fn claude_usage_cards(&self) -> Result<Vec<AccountSummary>> {
        let store = self.claude_load()?;
        Ok(store
            .accounts
            .iter()
            .map(|a| summary(a, false))
            .chain(self.provider_cards(Pool::Claude, None)?)
            .collect())
    }
    /// The Claude pool, following the login Claude Code uses. Its account
    /// is current while the official settings are Claude Code's.
    pub(crate) async fn claude_accounts(&self) -> Result<Vec<AccountSummary>> {
        let store = self.claude_logins().followed().await?;
        let location = self.claude_location();
        let official = settings_are_official(&location);
        let provider = claude_provider(&store, &location);
        let mut summaries: Vec<_> = store
            .accounts
            .iter()
            .map(|a| {
                let current = store.current_account_id.as_deref() == Some(a.id.as_str());
                let mut card = summary(a, official && current);
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
        Ok(summaries)
    }
    /// Adds or updates an account, which waits for a manual refresh unless
    /// new accounts may be asked at once. Returns its id, label and whether
    /// it waits.
    async fn claude_upsert(
        &self,
        oauth: Value,
        account: Value,
        origin: Origin,
    ) -> Result<(String, String, bool)> {
        let label = label_of(&account);
        // A login without its account is matched by its tokens until a
        // refresh identifies it.
        let login = Live {
            document: json!({"claudeAiOauth": oauth}),
            account: identity(&account).map(|_| account),
            held: Held::File,
            inferred: false,
        };
        let (id, waiting) = self.claude_logins().upsert(login, origin).await?;
        Ok((id, label, waiting))
    }
    /// The pool's account for this login, by its identity or else its
    /// tokens; none for a new account.
    fn claude_known(&self, oauth: &Value, account: Option<&Value>) -> Result<Option<Stored>> {
        let account_identity = account.and_then(identity);
        let store = self.claude_load()?;
        let mut matches = store.accounts.iter().filter(|stored| {
            if let Some(expected) = &account_identity {
                return identity(&stored.own.account).as_ref() == Some(expected);
            }
            same_tokens(&stored.document, oauth)
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
        let _op = self.claude.logins.operations.lock().await;
        let _ = self.claude_unlock().await;
        let live = read_live_with_access(&self.claude_location(), KeychainAccess::Interactive)
            .await?
            .ok_or("error.claude.login_not_found")?;
        let (id, label, waiting) = self.claude_import_live(live, Origin::Live).await?;
        drop(_op);
        if !waiting {
            let _ = self.claude_refresh_usage_scoped(&id, None).await;
        }
        Ok(label)
    }
    /// Imports exported Claude Code credentials without selecting them: one
    /// export, or a list of them, all checked before any is kept.
    pub(crate) async fn claude_import_json(&self, content: &str) -> Result<usize> {
        let invalid = "error.claude.credentials_json_invalid";
        let document: Value =
            serde_json::from_str(content.trim_start_matches('\u{feff}')).map_err(|_| invalid)?;
        let exports = match &document {
            Value::Array(exports) if exports.is_empty() => return Err(invalid.into()),
            Value::Array(exports) => exports.clone(),
            _ => vec![document],
        };
        let mut logins = Vec::new();
        for export in &exports {
            let oauth = exported_oauth(export).ok_or(invalid)?;
            logins.push(Live {
                account: export
                    .get("oauthAccount")
                    .filter(|a| identity(a).is_some())
                    .cloned(),
                document: json!({"claudeAiOauth": oauth}),
                // An export goes nowhere until a switch writes it.
                held: Held::File,
                inferred: false,
            });
        }
        let _op = self.claude.logins.operations.lock().await;
        let _ = self.claude_unlock().await;
        // A nameless login matched by its tokens must match one account.
        for live in logins.iter().filter(|live| live.account.is_none()) {
            self.claude_known(live.oauth().ok_or(invalid)?, None)?;
        }
        let mut fresh = Vec::new();
        for live in logins {
            let (id, _, waiting) = self.claude_import_live(live, Origin::Export).await?;
            if !waiting {
                fresh.push(id);
            }
        }
        drop(_op);
        for id in fresh {
            let _ = self.claude_refresh_usage_scoped(&id, None).await;
        }
        Ok(exports.len())
    }
    async fn claude_import_live(
        &self,
        live: Live,
        origin: Origin,
    ) -> Result<(String, String, bool)> {
        let oauth = live.oauth().ok_or("error.claude.login_not_found")?.clone();
        let account = match live.account.clone() {
            Some(account) => account,
            // Only the profile names a login without oauthAccount. Until it
            // may be asked, or when it fails, the account joins unnamed and
            // its first refresh probes it.
            None => match self.claude_known(&oauth, None)? {
                Some(known) => known.own.account,
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
        self.claude_upsert(oauth, account, origin).await
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
        let _op = self.claude.logins.operations.lock().await;
        let _ = self.claude_unlock().await;
        let (id, label, waiting) = self.claude_upsert(oauth, account, Origin::SignIn).await?;
        drop(_op);
        if !waiting {
            let _ = self.claude_refresh_usage_scoped(&id, None).await;
        }
        Ok(label)
    }
    pub(crate) async fn claude_delete_account(&self, id: &str) -> Result<()> {
        self.claude_logins().delete(id, |_, _| {}).await
    }
    /// Edits the stored accounts; their tokens stay where they are kept.
    pub(crate) async fn claude_edit_accounts(
        &self,
        edit: impl FnOnce(&mut [&mut dyn PoolAccount]) -> Result<bool>,
    ) -> Result<()> {
        self.claude_logins().edit(edit).await
    }
    /// Gives Claude Code this account's login and identity, or a Sub2api
    /// card's provider. A previous login outside the pool is kept in
    /// `claude-before-switch.json`; a failed step is rolled back.
    pub(crate) async fn claude_switch_account(
        &self,
        id: &str,
        access: KeychainAccess,
    ) -> Result<SwitchExecution> {
        let _op = self.claude.logins.operations.lock().await;
        if crate::sub2api::is_card(Pool::Claude, id) {
            return self.claude_switch_sub2api(id).await;
        }
        if access == KeychainAccess::Interactive {
            let _ = self.claude_unlock().await;
        }
        let logins = self.claude_logins();
        // Keep tokens Claude Code refreshed for the account being left.
        logins.followed().await?;
        // Never hand Claude Code a login without its tokens.
        let held = logins.load()?.accounts.into_iter().find(|a| a.id == id);
        if held.as_ref().is_some_and(|a| !holds_tokens(a)) {
            return Err(self.claude_locked());
        }
        let location = self.claude_location();
        // A renewal Claude Code finishes after the switch would write the
        // login being left over it: switch only while it renews nothing, and
        // read the login being left under the same lock. Waited for before
        // the store's lock, which the cards are read under.
        let for_switch = |error: String| match error.as_str() {
            CLAUDE_CODE_RENEWING => CLAUDE_CODE_RENEWING_SWITCH.to_owned(),
            _ => error,
        };
        // The switch gives Claude Code a login, so locks it where it keeps
        // one in the Keychain even before its directory is there.
        let claude_code = claude_code_lock(&location, location.keychain.is_some())
            .await
            .map_err(for_switch)?
            .map(ClaudeCodeLock);
        // A login not yet confirmed is switched from as no account's: kept
        // in the backup, its tokens given to none.
        let live = self
            .claude_live(access)
            .await?
            .map(|live| live.unwrap_or_else(|unconfirmed| unconfirmed));
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
        if let Some(lock) = &claude_code {
            lock.held().map_err(for_switch)?;
        }
        let target = logins
            .switch(
                id,
                live,
                access,
                "claude-before-switch.json",
                |_| match &official {
                    Some(official) => put_settings(&location, official.as_deref()).map(|()| true),
                    None => Ok(false),
                },
                |changed| {
                    if changed {
                        let _ = put_settings(&location, settings.as_deref());
                    }
                },
                |store| store.settings_provider = None,
            )
            .await?;
        // Once restored, the record would only go stale as the official
        // settings change; leaving them again records them anew.
        if restoring_record {
            let _ = fs::remove_file(self.claude_official_path());
        }
        // Other agents take the login once Claude Code holds it, without
        // keeping Claude Code's renewals waiting.
        let credential = pool_login::claude::credential(
            &target.document,
            &pool_login::claude::account_identity(&target.own.account),
        );
        drop(claude_code);
        let mut execution = self.after_switch(Pool::Claude, Some(credential)).await?;
        execution.login_overrides = login_overrides(&location);
        Ok(execution)
    }
    /// Claude Code's login, with the account its profile named for its
    /// access token (`identify_live_rotation`, `claude_confirmed`) in place
    /// of the `oauthAccount` it keeps, if any.
    fn claude_named(&self, mut live: Live) -> Live {
        let token = live.oauth().and_then(|o| text(&o["accessToken"]));
        let named = token.and_then(|t| self.claude.named.lock().unwrap().get(&t).cloned());
        if let Some(named) = named {
            if live.account.as_ref().and_then(identity) != identity(&named) {
                live.account = Some(named);
                live.inferred = true;
            }
        }
        live
    }
    /// Claude Code keeps its tokens and its `oauthAccount` apart and writes
    /// them one after the other: a read in between pairs a new login's
    /// tokens with the account it replaced. Tokens the pool has not seen,
    /// later than those of the account the login names, are that
    /// account's once their profile says so, else the account it names;
    /// an account waiting for a manual refresh takes them as they come.
    /// Until it answers, they are no account's: none is followed or
    /// renewed with them, and Claude Code's `oauthAccount` is never
    /// written for them.
    /// `Err` holds the login as no account's while unconfirmed.
    async fn claude_confirmed(&self, mut live: Live) -> std::result::Result<Live, Live> {
        let Some(named) = live.account.as_ref().and_then(identity) else {
            return Ok(live);
        };
        let Some(oauth) = live.oauth().cloned() else {
            return Ok(live);
        };
        let (Some(token), false) = (text(&oauth["accessToken"]), live.inferred) else {
            return Ok(live);
        };
        // Its profile named it already (`claude_named`): not asked again,
        // which a renewal holding Claude Code's refresh lock would wait for.
        if self.claude.named.lock().unwrap().contains_key(&token) {
            return Ok(live);
        }
        let Ok(store) = self.claude_load() else {
            return Ok(live);
        };
        if store
            .accounts
            .iter()
            .any(|a| same_tokens(&a.document, &oauth))
        {
            return Ok(live);
        }
        let Some(account) = store
            .accounts
            .iter()
            .find(|a| identity(&a.own.account).as_ref() == Some(&named))
        else {
            return Ok(live);
        };
        if !ClaudeLogins::newer(&oauth, &account.document) {
            return Ok(live);
        }
        let unconfirmed = |mut live: Live| {
            live.account = None;
            live.inferred = true;
            Err(live)
        };
        // Nothing goes upstream for an account waiting for a manual
        // refresh: Claude Code's renewals are followed as they come.
        if account.first_request_pending {
            return Ok(live);
        }
        let now = now_unix_seconds();
        if let Some((failed, at)) = &*self.claude.unconfirmed.lock().unwrap() {
            if *failed == token && now - at < UNCONFIRMED_RETRY {
                return unconfirmed(live);
            }
        }
        // Not yet known to be the account's: never its proxy. Asked while
        // the cards wait, or Claude Code's refresh lock is held: briefly.
        let profile = match self.account_proxy("") {
            Ok(proxy) => tokio::time::timeout(
                CONFIRM_WAIT,
                self.claude_network()
                    .profile(&self.claude_api_root(), &token, &proxy),
            )
            .await
            .ok()
            .and_then(Result::ok),
            Err(_) => None,
        };
        let Some(profiled) = profile
            .map(|profile| profile_account(Some(&profile), &Value::Null).0)
            .filter(|profiled| identity(profiled).is_some())
        else {
            *self.claude.unconfirmed.lock().unwrap() = Some((token, now));
            return unconfirmed(live);
        };
        if identity(&profiled) != Some(named) {
            live.account = Some(profiled.clone());
            live.inferred = true;
        }
        // Only Claude Code's login as it is now needs its name.
        let mut known = self.claude.named.lock().unwrap();
        known.clear();
        known.insert(token, profiled);
        Ok(live)
    }
    /// Claude Code's login, named and confirmed (`claude_confirmed`).
    async fn claude_live(
        &self,
        access: KeychainAccess,
    ) -> Result<Option<std::result::Result<Live, Live>>> {
        let live = read_live_with_access(&self.claude_location(), access).await?;
        #[cfg(test)]
        {
            let delay = self
                .claude
                .live_read_delay_ms
                .load(std::sync::atomic::Ordering::Relaxed);
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        Ok(match live {
            Some(live) => Some(self.claude_confirmed(self.claude_named(live)).await),
            None => None,
        })
    }
    /// Names a nameless login Claude Code holds by its own access token:
    /// once its profile shows it is `account`'s, reads of Claude Code's
    /// login name it so. Until then, never a particular card's proxy.
    async fn identify_live_rotation(&self, account: &Stored) -> Result<()> {
        let Some(live) = read_live(&self.claude_location()).await.ok().flatten() else {
            return Ok(());
        };
        let Some(oauth) = live.oauth().filter(|_| live.account.is_none()) else {
            return Ok(());
        };
        if identity(&account.own.account).is_none()
            || account.first_request_pending
            || !holds_tokens(account)
            || same_login(oauth, &account.document)
        {
            return Ok(());
        }
        let Some(token) = text(&oauth["accessToken"]) else {
            return Ok(());
        };
        if self.claude.named.lock().unwrap().contains_key(&token)
            || self.claude_known(oauth, None)?.is_some()
        {
            return Ok(());
        }
        let proxy = self.account_proxy("")?;
        let Ok(profile) = self
            .claude_network()
            .profile(&self.claude_api_root(), &token, &proxy)
            .await
        else {
            return Ok(());
        };
        let named = profile_account(Some(&profile), &Value::Null).0;
        if identity(&named) == identity(&account.own.account) {
            // Only Claude Code's login as it is now needs its name.
            let mut known = self.claude.named.lock().unwrap();
            known.clear();
            known.insert(token, named);
        }
        Ok(())
    }
    /// Renews pool credentials and syncs an unchanged live login without
    /// showing authorization UI; external rotations take precedence over
    /// the response. Requests go through the account's proxy, else the
    /// shared account proxy. With `renew_now`, only the tokens are renewed, even when unexpired or
    /// renewal is off; usage is not asked for, nor shown as refreshing, and
    /// whether the renewal was kept is reported.
    pub(crate) async fn claude_refresh_usage_scoped(
        &self,
        id: &str,
        renew_now: Option<&mut bool>,
    ) -> Result<()> {
        self.claude_refresh_login(id, renew_now, None).await
    }
    /// As `claude_refresh_usage_scoped`; with `rejected`, the access token
    /// an upstream refused, renews only while the account still holds it
    /// and its renewal allows, so concurrent refusals renew it once.
    async fn claude_refresh_login(
        &self,
        id: &str,
        renew_now: Option<&mut bool>,
        rejected: Option<&str>,
    ) -> Result<()> {
        let forced = renew_now.is_some() || rejected.is_some();
        let _activity = (!forced).then(|| self.begin_refresh(id));
        // Serialize renewal with imports/switches and other pool refreshes.
        let _op = self.claude.logins.operations.lock().await;
        let logins = self.claude_logins();
        // A nameless login Claude Code renewed is named by its profile first.
        if let Some(target) = self.claude_load()?.accounts.iter().find(|a| {
            a.id == id
                && (forced
                    || self
                        .claude_pause(a)
                        .is_none_or(|until| until <= now_unix_seconds()))
        }) {
            self.identify_live_rotation(target).await?;
        }
        let begun = logins.begin(id).await?;
        let account = &begun.account;
        if account.first_request_pending {
            return Ok(());
        }
        if !holds_tokens(account) {
            return Err(self.claude_locked());
        }
        // Renewed since the token was refused, here or by Claude Code.
        if let Some(rejected) = rejected {
            if text(&account.document["accessToken"]).as_deref() != Some(rejected) {
                return Ok(());
            }
            if Renewal::of(account.renewal) == Renewal::Never {
                return Err(RENEWAL_OFF.into());
            }
        }
        // A usage pause does not hold back a renewal alone.
        if !forced
            && self
                .claude_pause(account)
                .is_some_and(|until| until > now_unix_seconds())
        {
            return Err(account.usage_error.clone().unwrap_or_else(|| {
                "Claude usage request failed: HTTP 429; retry paused for 5 minutes".into()
            }));
        }
        let network = self.claude_network();
        let api = self.claude_api_root();
        let proxy = self.account_proxy(&account.proxy_url)?;
        let now = now_unix_seconds();
        // What the last request said of rate limits.
        let limited: std::sync::Mutex<(bool, Option<u64>)> = Default::default();
        let ask = |document: Value| {
            let (network, api, proxy, limited) = (&network, api.as_str(), proxy.as_str(), &limited);
            async move {
                match self
                    .claude_ask(&**network, api, proxy, account, document, limited)
                    .await
                {
                    Ok(Some(answer)) => Asked::Answered(answer),
                    Ok(None) => Asked::Refused,
                    Err(error) => Asked::Failed(error),
                }
            }
        };
        let used = logins.use_token(&begun, forced, &proxy, now, ask).await;
        let (mut rate_limited, retry_after) = *limited.lock().unwrap();
        // Renewal can fail before usage is asked: the same pause applies.
        rate_limited |= used
            .outcome
            .as_ref()
            .err()
            .is_some_and(|error| error == TOKEN_REFRESH_RATE_LIMITED);
        // Each HTTP 429 in a row pauses longer; any other answer starts over.
        // Without the card's slow-down, none pauses or counts.
        let backoff = self.card_refresh(Pool::Claude, id).backoff;
        let rate_limited = rate_limited && backoff;
        let reported: std::sync::Mutex<Result<()>> = std::sync::Mutex::new(Ok(()));
        logins
            .record(&begun, &used, |store, index| {
                let target = &mut store.accounts[index];
                let rate_limits = match (rate_limited, &used.outcome) {
                    (false, _) if !backoff => 0,
                    (true, _) => target.own.rate_limits.saturating_add(1),
                    (false, Ok(Some(_))) => 0,
                    (false, _) => target.own.rate_limits,
                };
                let pause = rate_limited.then(|| rate_limit_pause(rate_limits, retry_after));
                target.updated_at = now_unix_seconds();
                // A renewal alone keeps the usage pause it found.
                if !forced || pause.is_some() {
                    target.own.usage_retry_at = pause.map(|pause| now_unix_seconds() + pause);
                }
                target.own.rate_limits = rate_limits;
                match &used.outcome {
                    Ok(Some(answer)) => {
                        let mut usage = answer.usage.clone();
                        crate::claude_api::retain_known_resets(&mut usage, target.usage.as_ref());
                        target.usage = Some(usage);
                        target.usage_error = None;
                        for (key, value) in &answer.plan {
                            target.document[key] = value.clone();
                        }
                        if let Some(named) = &answer.profiled {
                            if identity(&target.own.account).is_none() {
                                target.label = label_of(named);
                                target.own.account = named.clone();
                            } else if let Some(stored) = target.own.account.as_object_mut() {
                                for (key, value) in named.as_object().into_iter().flatten() {
                                    stored.insert(key.clone(), value.clone());
                                }
                            }
                        }
                    }
                    // A renewal alone leaves the usage as it was.
                    Ok(None) => {}
                    Err(error) => {
                        let error = match pause {
                            Some(pause) => {
                                format!("{error}; retry paused for {} minutes", (pause + 59) / 60)
                            }
                            None => error.clone(),
                        };
                        target.usage_error = Some(error.clone());
                        *reported.lock().unwrap() = Err(error);
                    }
                }
                merge_identified(store, id);
            })
            .await?;
        if let Some(reported) = renew_now {
            // A renewal Claude Code made meanwhile serves as one.
            *reported = used.renewed || used.adopted;
        }
        reported.into_inner().unwrap()
    }
    /// Asks Claude for the account's usage with this login: none when its
    /// token is refused. An account imported without its name is probed
    /// first, and one without its plan learns it; both through its proxy.
    /// `limited` hears whether Claude asked to slow down, and for how long.
    async fn claude_ask(
        &self,
        network: &dyn Network,
        api: &str,
        proxy: &str,
        account: &Stored,
        document: Value,
        limited: &std::sync::Mutex<(bool, Option<u64>)>,
    ) -> Result<Option<Answer>> {
        let token = text(&document["accessToken"]).unwrap_or_default();
        let mut profiled = None;
        let mut learned = Vec::new();
        if identity(&account.own.account).is_none() {
            let found = match network.profile(api, &token, proxy).await {
                Err(error) if error == SIGN_IN_EXPIRED => {
                    self.identify_live_rotation(account).await?;
                    return Ok(None);
                }
                found => found?,
            };
            let (named, subscription, tier) = profile_account(Some(&found), &Value::Null);
            if identity(&named).is_none() {
                return Err("Claude did not report the account".into());
            }
            learn(&mut learned, subscription, tier);
            profiled = Some(named);
        }
        let (status, body, after, clock_offset) =
            self.claude_usage(network, api, &token, proxy).await?;
        *limited.lock().unwrap() = (status == 429, after.filter(|_| status == 429));
        match status {
            200..=299 => {}
            // Claude Code may have renewed the login meanwhile: named, it is
            // tried before renewing.
            401 => {
                self.identify_live_rotation(account).await?;
                return Ok(None);
            }
            _ => {
                let message = body["error"]["message"].as_str().unwrap_or_default();
                return Err(
                    format!("Claude usage request failed: HTTP {status} {message}")
                        .trim()
                        .to_owned(),
                );
            }
        }
        if profiled.is_none() && text(&document["subscriptionType"]).is_none() {
            if let Ok(found) = network.profile(api, &token, proxy).await {
                let (named, subscription, tier) = profile_account(Some(&found), &Value::Null);
                if identity(&named) == identity(&account.own.account) {
                    learn(&mut learned, subscription, tier);
                    profiled = Some(named);
                }
            }
        }
        let mut planned = document.clone();
        for (key, value) in &learned {
            planned[key] = value.clone();
        }
        let mut usage = usage_snapshot(&body, plan(&planned), now_unix_seconds())?;
        for window in [
            &mut usage.five_hour,
            &mut usage.one_week,
            &mut usage.fable_one_week,
        ]
        .into_iter()
        .flatten()
        {
            window.clock_offset_seconds = clock_offset;
        }
        // Extra usage is paid from the organization's prepaid balance, asked
        // for through the same proxy.
        let org = text(&profiled.as_ref().unwrap_or(&account.own.account)["organizationUuid"]);
        if let (true, Some(org)) = (body["extra_usage"].is_object(), org) {
            if let Ok(prepaid) = network.prepaid(api, &token, &org, proxy).await {
                usage.credits = extra_usage_credits(&body["extra_usage"], &prepaid);
            }
        }
        Ok(Some(Answer {
            usage,
            profiled,
            plan: learned,
        }))
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
        let _lock = self.claude.logins.store.lock().await;
        let store = self.claude_load()?;
        let account = store
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?;
        let expiry = expires_at(&account.document);
        Ok(account.usage_error.is_some() || (expiry != 0 && expiry <= now_unix_seconds() * 1000))
    }
    /// One hi with an account, through its proxy, without switching to it.
    pub(crate) async fn claude_hello(&self, id: &str) -> Result<Hello> {
        let account = {
            let _lock = self.claude.logins.store.lock().await;
            self.claude_load()?
                .accounts
                .into_iter()
                .find(|a| a.id == id)
                .ok_or("Account not found")?
        };
        if !holds_tokens(&account) {
            return Err(self.claude_locked());
        }
        let expired = expires_at(&account.document) != 0
            && expires_at(&account.document) <= (now_unix_seconds() + 60) * 1000;
        let (status, body) = if expired {
            (401, Value::Null)
        } else {
            let token = text(&account.document["accessToken"]).unwrap_or_default();
            let proxy = self.account_proxy(&account.proxy_url)?;
            let uuid = text(&account.own.account["accountUuid"]).unwrap_or_default();
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
mod tests;
