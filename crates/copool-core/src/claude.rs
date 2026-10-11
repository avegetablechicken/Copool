//! Claude Code account pool: the logins Copool keeps, the one Claude Code uses
//! (the macOS Keychain or `.credentials.json`, with `oauthAccount` in
//! `.claude.json`), OAuth usage and switching.
use crate::{
    account_network::Response,
    application::{Application, SmartSwitchOutcome, SwitchExecution},
    models::{AccountSummary, UsageSnapshot, UsageWindow},
    persistence::write_json,
    utils::now_unix_seconds,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, sync::Arc, time::Duration};

type Result<T> = std::result::Result<T, String>;

mod sub2api;
pub use sub2api::Provider as Sub2ApiProvider;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const USER_AGENT: &str = "claude-code/2.1.69";
const SIGN_IN_EXPIRED: &str = "error.accounts.sign_in_expired";

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

/// Keys of the user's `settings.json` that make Claude Code use an API key,
/// a gateway or a cloud provider instead of the account login (the
/// connection keys cc-switch manages there).
pub fn login_overrides(location: &Location) -> Vec<String> {
    let Some(settings) = fs::read_to_string(location.config_dir.join("settings.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    else {
        return vec![];
    };
    let set = |v: &Value| match v {
        Value::Null => false,
        Value::String(s) => !s.trim().is_empty(),
        Value::Bool(b) => *b,
        _ => true,
    };
    let mut keys: Vec<String> = ["apiKeyHelper"]
        .into_iter()
        .filter(|key| settings.get(key).is_some_and(set))
        .map(str::to_owned)
        .collect();
    if let Some(env) = settings["env"].as_object() {
        keys.extend(
            [
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_BASE_URL",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_USE_VERTEX",
                "CLAUDE_CODE_USE_FOUNDRY",
            ]
            .into_iter()
            .filter(|key| {
                env.get(*key).is_some_and(|v| {
                    set(v) && !matches!(v.as_str().map(str::trim), Some("0" | "false"))
                })
            })
            .map(str::to_owned),
        );
    }
    keys
}

pub(crate) trait Network: Send + Sync {
    /// The HTTP status and body of the OAuth usage endpoint.
    fn usage<'a>(&'a self, token: &'a str, proxy: &'a str) -> Response<'a, (u16, Value)>;
    /// A token response for a refresh token.
    fn refresh<'a>(&'a self, refresh_token: &'a str, proxy: &'a str) -> Response<'a, Value>;
    fn profile<'a>(&'a self, token: &'a str, proxy: &'a str) -> Response<'a, Value>;
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
    fn usage<'a>(&'a self, token: &'a str, proxy: &'a str) -> Response<'a, (u16, Value)> {
        Box::pin(async move {
            let response = client(proxy)?
                .get(USAGE_URL)
                .bearer_auth(token.trim())
                .header("Accept", "application/json")
                .header("anthropic-beta", "oauth-2025-04-20")
                .send()
                .await
                .map_err(|e| e.to_string())?;
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
                .map_err(|e| e.to_string())?;
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
                .map_err(|e| e.to_string())?;
            if !response.status().is_success() {
                return Err(format!(
                    "Claude profile request failed: HTTP {}",
                    response.status().as_u16()
                ));
            }
            response.json().await.map_err(|e| e.to_string())
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
        remaining_reset_count: None,
        fetched_at: now,
        plan_type: plan,
        five_hour: window("five_hour", 5 * 60 * 60),
        one_week: window("seven_day", 7 * 24 * 60 * 60),
        credits: None,
    })
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

#[cfg(target_os = "macos")]
fn keychain_user() -> String {
    std::env::var("USER").unwrap_or_default()
}
#[cfg(target_os = "macos")]
async fn keychain_read(service: &str) -> Result<Option<Value>> {
    let user = keychain_user();
    // Claude Code files the entry under the login name; any account otherwise.
    for account in [Some(user.as_str()), None] {
        let mut args = vec!["find-generic-password", "-s", service, "-w"];
        if let Some(account) = account.filter(|a| !a.is_empty()) {
            args.extend(["-a", account]);
        }
        let output = crate::process::run("/usr/bin/security", &args, Duration::from_secs(15))
            .await
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            return parse_credentials(&output.stdout)
                .map(Some)
                .ok_or_else(|| "Invalid Claude Code Keychain credentials".into());
        }
    }
    Ok(None)
}
#[cfg(not(target_os = "macos"))]
async fn keychain_read(_: &str) -> Result<Option<Value>> {
    Ok(None)
}
/// As Claude Code: `security -i` on standard input, so the token never
/// appears in a process listing.
#[cfg(target_os = "macos")]
async fn keychain_write(service: &str, document: &Value) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let user = keychain_user();
    if [&user, service]
        .iter()
        .any(|v| v.contains(['"', '\\', '\n']))
    {
        return Err("Unsupported Keychain account name".into());
    }
    let payload = hex(&serde_json::to_vec(document).map_err(|e| e.to_string())?);
    let mut child = tokio::process::Command::new("/usr/bin/security")
        .arg("-i")
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("Keychain input unavailable")?;
    stdin
        .write_all(
            format!("add-generic-password -U -a \"{user}\" -s \"{service}\" -X \"{payload}\"\n")
                .as_bytes(),
        )
        .await
        .map_err(|e| e.to_string())?;
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .map_err(|_| "The Keychain did not respond")?
        .map_err(|e| e.to_string())?;
    // `security -i` reports a failed command on stderr, not in its status.
    let written = keychain_read(service).await?;
    if !output.status.success()
        || written.as_ref().map(|d| &d["claudeAiOauth"]) != Some(&document["claudeAiOauth"])
    {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Could not write the Claude Code Keychain entry {}",
            detail.trim()
        ));
    }
    Ok(())
}
#[cfg(not(target_os = "macos"))]
async fn keychain_write(_: &str, _: &Value) -> Result<()> {
    Err("The Keychain is only available on macOS".into())
}

async fn read_live(location: &Location) -> Result<Option<Live>> {
    let mut document = None;
    if let Some(service) = &location.keychain {
        document = keychain_read(service).await?;
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
async fn write_credentials(location: &Location, document: &Value) -> Result<()> {
    let file = location.credentials_file();
    if let Some(service) = &location.keychain {
        keychain_write(service, document).await?;
        // A file Claude Code falls back to must not keep the old login.
        if !file.exists() {
            return Ok(());
        }
    }
    write_json(&file, document)
}
fn write_account(location: &Location, account: &Value) -> Result<()> {
    let path = location.account_file();
    let document = match fs::read_to_string(&path) {
        Ok(text) if !text.trim().is_empty() => text,
        Ok(_) => "{}".into(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
        Err(e) => return Err(e.to_string()),
    };
    let updated = set_member(&document, "oauthAccount", account)?;
    write_text(&path, &updated)
}
fn write_text(path: &std::path::Path, text: &str) -> Result<()> {
    use std::io::Write;
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
        // A Sub2api gateway in settings.json takes Claude Code off its login.
        let gateway = sub2api::active(&store, &location);
        let mut summaries: Vec<_> = store
            .accounts
            .iter()
            .map(|a| {
                a.summary(
                    gateway.is_none() && store.current_account_id.as_deref() == Some(a.id.as_str()),
                )
            })
            .chain(sub2api::summaries(&store, gateway))
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
        let key = identity(&account).ok_or("Claude did not report the account")?;
        let now = now_unix_seconds();
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        let label = text(&account["emailAddress"])
            .or_else(|| text(&account["displayName"]))
            .unwrap_or_else(|| key.clone());
        let (id, awaiting) = match store
            .accounts
            .iter_mut()
            .find(|a| identity(&a.account).as_ref() == Some(&key))
        {
            Some(existing) => {
                existing.oauth = oauth;
                existing.account = account;
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
            .map(|(proxy, _)| proxy)
            .unwrap_or_default())
    }
    /// The proxy of the pool's account for this login, and whether it still
    /// waits for its first manual refresh; none for a new account.
    fn claude_known(
        &self,
        oauth: &Value,
        account: Option<&Value>,
    ) -> Result<Option<(String, bool)>> {
        let account_identity = account.and_then(identity);
        let store = self.claude_load()?;
        let mut matches = store.accounts.iter().filter(|stored| {
            if let Some(expected) = &account_identity {
                return identity(&stored.account).as_ref() == Some(expected);
            }
            ["accessToken", "refreshToken"].iter().any(|key| {
                text(&oauth[key]).is_some_and(|token| text(&stored.oauth[key]) == Some(token))
            })
        });
        let Some(first) = matches.next() else {
            return Ok(None);
        };
        if matches.any(|other| other.proxy_url != first.proxy_url) {
            return Err("Cannot determine the Claude account proxy unambiguously".into());
        }
        Ok(Some((first.proxy_url.clone(), first.first_request_pending)))
    }
    /// Whether this login may contact upstream now: a known account that no
    /// longer waits, or a new one when the user opted in.
    fn claude_may_contact(&self, known: Option<&(String, bool)>) -> Result<bool> {
        Ok(match known {
            Some((_, waiting)) => !waiting,
            None => self.preferences()?.request_new_accounts,
        })
    }
    /// Imports the login Claude Code uses; returns the account label.
    pub async fn claude_import_current(&self) -> Result<String> {
        let _op = self.claude.operations.lock().await;
        let live = read_live(&self.claude_location())
            .await?
            .ok_or("error.claude.login_not_found")?;
        let oauth = live.oauth().ok_or("error.claude.login_not_found")?.clone();
        let account = match live.account.clone() {
            Some(account) => account,
            // Only the profile names a login without oauthAccount.
            None => {
                let known = self.claude_known(&oauth, None)?;
                if !self.claude_may_contact(known.as_ref())? {
                    return Err("error.claude.account_unnamed".into());
                }
                let token = text(&oauth["accessToken"]).unwrap_or_default();
                let proxy = known.map(|(proxy, _)| proxy).unwrap_or_default();
                let profile = self.claude_network().profile(&token, &proxy).await?;
                profile_account(Some(&profile), &Value::Null).0
            }
        };
        let defer = !self.preferences()?.request_new_accounts;
        let (id, label, waiting) = self.claude_upsert(oauth, account, true, defer).await?;
        drop(_op);
        if !waiting {
            let _ = self.claude_refresh_usage(&id).await;
        }
        Ok(label)
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
            let proxy = known.map(|(proxy, _)| proxy).unwrap_or_default();
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
            let _ = self.claude_refresh_usage(&id).await;
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
    /// Gives Claude Code this account's login and identity. Its previous login
    /// is kept in `claude-before-switch.json`; a failed step is rolled back.
    pub async fn claude_switch_account(&self, id: &str) -> Result<SwitchExecution> {
        let _op = self.claude.operations.lock().await;
        if sub2api::is_card(id) {
            return self.claude_switch_sub2api(id).await;
        }
        // Keep tokens Claude Code refreshed for the account being left.
        self.claude_accounts().await?;
        let location = self.claude_location();
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        let target = store
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?
            .clone();
        let live = read_live(&location).await?;
        let previous_document = live.as_ref().map(|l| l.document.clone());
        let previous_account = live.as_ref().and_then(|l| l.account.clone());
        if live.is_some() {
            write_json(
                &self.paths.data_dir.join("claude-before-switch.json"),
                &json!({"credentials": previous_document, "oauthAccount": previous_account}),
            )?;
        }
        let mut document = previous_document.clone().unwrap_or_else(|| json!({}));
        document["claudeAiOauth"] = target.oauth.clone();
        write_credentials(&location, &document).await?;
        let rollback = |error: String| async {
            let restored = match &previous_document {
                Some(previous) => write_credentials(&location, previous).await,
                None => Ok(()),
            };
            restored
                .map_err(|rollback| format!("{error}; rollback failed: {rollback}"))
                .and(Err(error))
        };
        if let Err(error) = write_account(&location, &target.account) {
            return rollback(error).await;
        }
        let settings = match sub2api::leave(&store, &location) {
            Ok(previous) => previous,
            Err(error) => {
                if let Some(previous) = &previous_account {
                    let _ = write_account(&location, previous);
                }
                return rollback(error).await;
            }
        };
        store.current_account_id = Some(id.into());
        if let Err(error) = self.claude_save(&store) {
            let _ = sub2api::restore_settings(&location, settings);
            if let Some(previous) = &previous_account {
                let _ = write_account(&location, previous);
            }
            return rollback(error).await;
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
    /// Usage of one account, refreshing its token when it expired or was
    /// rejected. The current account's new token is handed to Claude Code.
    pub async fn claude_refresh_usage(&self, id: &str) -> Result<()> {
        if sub2api::is_card(id) {
            return self.claude_refresh_sub2api_card(id).await;
        }
        let _activity = self.begin_refresh(id);
        let location = self.claude_location();
        // Tokens Claude Code rotated must be used, never its stale refresh token.
        let live = read_live(&location).await.ok().flatten();
        let account = {
            let _lock = self.claude.lock.lock().await;
            let mut store = self.claude_load()?;
            if follow_live(&mut store, live.as_ref()) {
                self.claude_save(&store)?;
            }
            store
                .accounts
                .into_iter()
                .find(|a| a.id == id)
                .ok_or("Account not found")?
        };
        if account.first_request_pending {
            return Ok(());
        }
        let network = self.claude_network();
        let proxy = account.proxy_url.as_str();
        // A refresh rotates the refresh token, so its result is kept even
        // when the usage request then fails.
        let mut oauth = account.oauth.clone();
        let mut profiled = None;
        let result = async {
            let refresh = |oauth: &Value| {
                let network = network.clone();
                let token = text(&oauth["refreshToken"]);
                async move {
                    let token = token.ok_or(SIGN_IN_EXPIRED)?;
                    network.refresh(&token, proxy).await
                }
            };
            if expires_at(&oauth) != 0 && expires_at(&oauth) <= (now_unix_seconds() + 60) * 1000 {
                let tokens = refresh(&oauth).await?;
                apply_tokens(&mut oauth, &tokens, now_unix_seconds() * 1000)?;
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
            if (200..=299).contains(&status) && text(&oauth["subscriptionType"]).is_none() {
                if let Ok(found) = network.profile(&token, proxy).await {
                    let (named, subscription, tier) = profile_account(Some(&found), &Value::Null);
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
            match status {
                200..=299 => usage_snapshot(&body, plan(&oauth), now_unix_seconds()),
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
        let refreshed = Some(oauth).filter(|oauth| *oauth != account.oauth);
        // Only the login Claude Code still uses for this account is replaced.
        if let Some(oauth) = &refreshed {
            if let Ok(Some(live)) = read_live(&location).await {
                let same = live.account.as_ref().and_then(identity) == identity(&account.account)
                    || live.oauth().and_then(|o| text(&o["refreshToken"]))
                        == text(&account.oauth["refreshToken"]);
                if same {
                    let mut document = live.document;
                    document["claudeAiOauth"] = oauth.clone();
                    write_credentials(&location, &document).await?;
                }
            }
        }
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        if let Some(target) = store.accounts.iter_mut().find(|a| a.id == id) {
            target.updated_at = now_unix_seconds();
            match &result {
                Ok(usage) => {
                    target.usage = Some(usage.clone());
                    target.usage_error = None;
                }
                Err(error) => target.usage_error = Some(error.clone()),
            }
            if let Some(oauth) = refreshed {
                target.oauth = oauth;
            }
            if let (Some(named), Some(stored)) = (profiled, target.account.as_object_mut()) {
                for (key, value) in named.as_object().into_iter().flatten() {
                    stored.insert(key.clone(), value.clone());
                }
            }
            self.claude_save(&store)?;
        }
        result.map(|_| ())
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
    /// Every account, or only the current one between full refreshes.
    pub async fn claude_refresh(&self, ids: Option<Vec<String>>) -> Result<()> {
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
            self.claude_refresh_sub2api_cards(ids.as_deref())
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
    pub async fn claude_refresh_recurring(&self, tick: u64) -> Result<()> {
        let accounts = self.claude_accounts().await?;
        if accounts.is_empty() {
            return Ok(());
        }
        self.claude_refresh(crate::refresh_policy::recurring_ids(tick, &accounts))
            .await
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
            self.claude_switch_account(&target.id).await?;
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

    #[derive(Default)]
    pub(super) struct Fake {
        usage: Mutex<Vec<(u16, Value)>>,
        refreshed: Mutex<Vec<String>>,
        profile_proxies: Mutex<Vec<String>>,
    }
    impl Network for Fake {
        fn usage<'a>(&'a self, _: &'a str, _: &'a str) -> Response<'a, (u16, Value)> {
            Box::pin(async move {
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
    }
    fn login(user: &str, expires: i64) -> (Value, Value) {
        (
            json!({"accessToken": format!("{user}-access"), "refreshToken": format!("{user}-refresh"), "expiresAt": expires, "subscriptionType": "max", "rateLimitTier": "default_claude_max_20x"}),
            json!({"accountUuid": format!("acct-{user}"), "organizationUuid": format!("org-{user}"), "emailAddress": format!("{user}@example.com")}),
        )
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
        assert!(app
            .paths
            .data_dir
            .join("claude-before-switch.json")
            .exists());
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
    }

    #[tokio::test]
    async fn an_expired_token_is_refreshed_and_handed_to_claude_code() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "alice", 1);
        app.claude_import_current().await.unwrap();
        assert_eq!(*fake.refreshed.lock().unwrap(), vec!["alice-refresh"]);
        assert_eq!(live_access(&location), "fresh-access");
        let stored = app.claude_load().unwrap();
        assert_eq!(stored.accounts[0].oauth["refreshToken"], "fresh-refresh");
        assert_eq!(stored.accounts[0].oauth["subscriptionType"], "max");

        // A rejected token is refreshed once; a second rejection asks to sign in.
        fake.usage
            .lock()
            .unwrap()
            .extend([(401, Value::Null), (401, Value::Null)]);
        let id = stored.accounts[0].id.clone();
        assert_eq!(
            app.claude_refresh_usage(&id).await.unwrap_err(),
            SIGN_IN_EXPIRED
        );
        let accounts = app.claude_accounts().await.unwrap();
        assert_eq!(accounts[0].usage_error.as_deref(), Some(SIGN_IN_EXPIRED));
    }

    #[tokio::test]
    async fn a_card_refresh_uses_the_tokens_claude_code_rotated() {
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
        fake.usage.lock().unwrap().push((401, Value::Null));
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.claude_refresh_usage(&id).await.unwrap();
        assert_eq!(*fake.refreshed.lock().unwrap(), vec!["rotated-refresh"]);
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
    async fn importing_without_local_identity_resolves_profile_proxy_from_credentials() {
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
            assert_eq!(
                *fake.profile_proxies.lock().unwrap(),
                vec!["http://127.0.0.1:8002"]
            );
            assert_eq!(app.claude_load().unwrap().accounts.len(), 1);
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
        assert_eq!(login_overrides(&location), vec!["ANTHROPIC_BASE_URL"]);
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
