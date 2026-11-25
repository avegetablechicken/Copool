//! Explicit desktop integrations. Never run by core tests or during account reads.
use crate::{
    auth, persistence::write_json, process, utils::now_unix_seconds, Pool, Preferences,
    SwitchExecution,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};

pub fn executable(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    #[cfg(target_os = "macos")]
    dirs.extend(["/opt/homebrew/bin".into(), "/usr/local/bin".into()]);
    if let Some(home) = dirs::home_dir() {
        dirs.extend([home.join(".local/bin"), home.join(".cargo/bin")]);
    }
    dirs.into_iter()
        .map(|p| {
            p.join(if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.into()
            })
        })
        .find(|p| p.is_file())
}
pub async fn checked(program: &str, args: &[&str], timeout: u64) -> Result<String, String> {
    let output = process::run(program, args, Duration::from_secs(timeout))
        .await
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
pub fn open_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Unsupported URL".into());
    }
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("/usr/bin/open");
    #[cfg(target_os = "linux")]
    let mut cmd = std::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    cmd.arg(url).spawn().map(|_| ()).map_err(|e| e.to_string())
}
const OPENAI_CODEX_PROVIDER: &str = "openai-codex";
const ANTHROPIC_PROVIDER: &str = "anthropic";
const ANTIGRAVITY_CLI_PROVIDER: &str = "antigravity-cli";
const OMP_REPLACED_CAUSE: &str = "replaced by newer credential";

#[derive(Clone, Debug, PartialEq, Eq)]
struct OAuthCredential {
    access: String,
    refresh: String,
    expires_ms: i64,
    account_id: Option<String>,
    org_id: Option<String>,
    email: Option<String>,
    org_name: Option<String>,
    scope: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyncAgent {
    Opencode,
    Pi,
    Omp,
}

impl SyncAgent {
    fn provider_key(self, pool: Pool) -> Option<&'static str> {
        match (self, pool) {
            (Self::Opencode, Pool::Codex) => Some("openai"),
            (Self::Opencode, Pool::Grok) => Some("xai"),
            (Self::Pi, Pool::Codex) => Some(OPENAI_CODEX_PROVIDER),
            (Self::Pi, Pool::Grok) => Some("xai"),
            (Self::Pi, Pool::Claude) => Some(ANTHROPIC_PROVIDER),
            (Self::Omp, Pool::Codex) => Some(OPENAI_CODEX_PROVIDER),
            (Self::Omp, Pool::Grok) => Some("xai-oauth"),
            (Self::Omp, Pool::Claude) => Some(ANTHROPIC_PROVIDER),
            (Self::Omp, Pool::Antigravity) => Some(ANTIGRAVITY_CLI_PROVIDER),
            // Antigravity has no built-in provider in OpenCode or pi.
            _ => None,
        }
    }

    fn sync_credential(
        self,
        pool: Pool,
        credential: &OAuthCredential,
        targets: &[PathBuf],
    ) -> Result<(), String> {
        let provider_key = self
            .provider_key(pool)
            .ok_or_else(|| format!("Unsupported sync target: {self:?} for {pool:?}"))?;
        let include_account_id = pool == Pool::Codex;
        match self {
            Self::Opencode => sync_oauth_json(
                credential,
                targets,
                provider_key,
                "",
                "Invalid OpenCode auth document",
                false,
                include_account_id,
            ),
            Self::Pi => sync_oauth_json(
                credential,
                targets,
                provider_key,
                "Unable to locate pi auth file path",
                "Invalid pi auth document",
                true,
                include_account_id,
            ),
            Self::Omp => sync_omp_credential(credential, provider_key, targets),
        }
    }

    fn sync_raw(self, pool: Pool, auth_json: &Value, targets: &[PathBuf]) -> Result<(), String> {
        let credential = extract_oauth_credential(pool, auth_json)?;
        self.sync_credential(pool, &credential, targets)
    }
}

fn non_empty_trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

fn default_expires_ms() -> i64 {
    now_unix_seconds().saturating_mul(1000) + 55 * 60 * 1000
}

fn extract_oauth_credential(pool: Pool, auth_json: &Value) -> Result<OAuthCredential, String> {
    match pool {
        Pool::Codex => {
            let oauth = auth::extract_codex_oauth_tokens(auth_json)?;
            let expires_ms = oauth
                .expires_at_ms
                .map(|ms| ms.max(0))
                .unwrap_or_else(default_expires_ms);
            Ok(OAuthCredential {
                access: oauth.access_token,
                refresh: oauth.refresh_token,
                expires_ms,
                org_id: oauth.account_id.clone(),
                account_id: oauth.account_id,
                email: oauth.email,
                org_name: oauth.plan_type,
                scope: None,
            })
        }
        Pool::Grok => {
            let sign_in = crate::grok::login::sign_in_of(auth_json);
            let doc = sign_in.as_ref().unwrap_or(auth_json);
            let access = crate::grok::login::access_token(doc).ok_or("Missing access token")?;
            let refresh = crate::grok::login::refresh_token(doc).ok_or("Missing refresh token")?;
            let access_claims = crate::grok::login::claims(&access);
            let expires_ms = crate::grok::login::expiry(doc)
                .or_else(|| access_claims.get("exp").and_then(Value::as_i64))
                .map(|seconds| seconds.saturating_mul(1000).max(0))
                .unwrap_or_else(default_expires_ms);
            // The xAI subject, as omp's `xai-oauth` keeps it in `accountId`.
            let account_id = crate::grok::login::user_id(doc)
                .or_else(|| non_empty_trimmed(access_claims.get("sub").and_then(Value::as_str)));
            let email = crate::grok::login::email(doc)
                .or_else(|| {
                    non_empty_trimmed(access_claims.get("email").and_then(Value::as_str))
                        .filter(|email| email.contains('@'))
                })
                .map(|e| e.to_ascii_lowercase());
            Ok(OAuthCredential {
                access,
                refresh,
                expires_ms,
                account_id,
                org_id: None,
                email,
                org_name: None,
                scope: None,
            })
        }
        Pool::Claude => {
            // `{"claudeAiOauth": …, "oauthAccount": …}`, or the OAuth entry alone.
            let oauth = auth_json
                .get("claudeAiOauth")
                .filter(|v| v.is_object())
                .unwrap_or(auth_json);
            let account = auth_json.get("oauthAccount").unwrap_or(&Value::Null);
            let text = |v: &Value, key: &str| non_empty_trimmed(v.get(key).and_then(Value::as_str));
            let access = text(oauth, "accessToken").ok_or("Missing access token")?;
            let refresh = text(oauth, "refreshToken").ok_or("Missing refresh token")?;
            let expires_ms = crate::claude_api::expires_at_ms(oauth)
                .filter(|ms| *ms > 0)
                .unwrap_or_else(default_expires_ms);
            let scope = crate::claude_api::granted_scope(oauth).or_else(|| text(oauth, "scope"));
            Ok(OAuthCredential {
                access,
                refresh,
                expires_ms,
                account_id: text(account, "accountUuid"),
                org_id: text(account, "organizationUuid"),
                email: text(account, "emailAddress").map(|e| e.to_ascii_lowercase()),
                org_name: text(account, "organizationName"),
                scope,
            })
        }
        Pool::Antigravity => {
            let access =
                crate::antigravity::login::access_token(auth_json).ok_or("Missing access token")?;
            let refresh = crate::antigravity::login::refresh_token(auth_json)
                .ok_or("Missing refresh token")?;
            let expires_ms = crate::antigravity::login::expiry(auth_json)
                .map(|seconds| seconds.saturating_mul(1000).max(0))
                .unwrap_or_else(default_expires_ms);
            let email = crate::antigravity::login::email(auth_json).map(|e| e.to_ascii_lowercase());
            let account_id = non_empty_trimmed(
                auth_json
                    .get("account_id")
                    .or_else(|| auth_json.get("accountId"))
                    .or_else(|| auth_json.get("googleId"))
                    .and_then(Value::as_str),
            )
            .or_else(|| {
                let id_token = auth_json.get("id_token").and_then(Value::as_str)?;
                let claims = auth::decode_jwt_payload(id_token).ok()?;
                non_empty_trimmed(claims.get("sub").and_then(Value::as_str))
            });
            // omp's `antigravity-cli` requires email or subject to isolate its per-account dir.
            if email.is_none() && account_id.is_none() {
                return Err("Missing Antigravity account identity".into());
            }
            Ok(OAuthCredential {
                access,
                refresh,
                expires_ms,
                account_id,
                org_id: None,
                email,
                org_name: None,
                scope: None,
            })
        }
    }
}

fn expand_tilde(raw: &str, home: Option<&Path>) -> PathBuf {
    let trimmed = raw.trim();
    if trimmed == "~" {
        return home
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(trimmed));
    }
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        if let Some(home) = home {
            return home.join(rest);
        }
    }
    PathBuf::from(trimmed)
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn sync_targets<F>(targets: &[PathBuf], empty_error: &str, mut apply: F) -> Result<(), String>
where
    F: FnMut(&Path) -> Result<(), String>,
{
    if targets.is_empty() {
        return Err(empty_error.into());
    }
    let mut failures = vec![];
    for path in targets {
        if let Err(error) = apply(path) {
            failures.push(error);
        }
    }
    if failures.len() == targets.len() {
        Err(failures.join(" | "))
    } else {
        Ok(())
    }
}
fn sync_oauth_json(
    credential: &OAuthCredential,
    targets: &[PathBuf],
    provider_key: &str,
    empty_error: &str,
    invalid_doc_error: &str,
    replace_entry: bool,
    include_account_id: bool,
) -> Result<(), String> {
    sync_targets(targets, empty_error, |path| {
        let mut root: Value = if path.exists() {
            let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            let content = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
            serde_json::from_str(content).map_err(|e| e.to_string())?
        } else {
            json!({})
        };
        let Some(root_obj) = root.as_object_mut() else {
            return Err(invalid_doc_error.into());
        };
        let mut entry = if replace_entry {
            serde_json::Map::new()
        } else {
            root_obj
                .get(provider_key)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
        };
        entry.insert("type".into(), json!("oauth"));
        entry.remove("key");
        entry.insert("access".into(), credential.access.clone().into());
        entry.insert("refresh".into(), credential.refresh.clone().into());
        entry.insert("expires".into(), credential.expires_ms.into());
        if include_account_id {
            if let Some(id) = &credential.account_id {
                entry.insert("accountId".into(), id.clone().into());
            }
        }
        root_obj.insert(provider_key.into(), Value::Object(entry));
        write_json(path, &root)
    })
}

pub fn sync_opencode(auth_json: &Value, targets: &[PathBuf]) -> Result<(), String> {
    SyncAgent::Opencode.sync_raw(Pool::Codex, auth_json, targets)
}

pub fn sync_opencode_grok(document: &Value, targets: &[PathBuf]) -> Result<(), String> {
    SyncAgent::Opencode.sync_raw(Pool::Grok, document, targets)
}

pub fn sync_pi(auth_json: &Value, targets: &[PathBuf]) -> Result<(), String> {
    SyncAgent::Pi.sync_raw(Pool::Codex, auth_json, targets)
}

fn pi_paths_from(
    home: Option<&Path>,
    pi_auth_path: Option<&str>,
    pi_coding_agent_dir: Option<&str>,
) -> Vec<PathBuf> {
    if let Some(path) = non_empty_trimmed(pi_auth_path) {
        return vec![expand_tilde(&path, home)];
    }
    if let Some(dir) = non_empty_trimmed(pi_coding_agent_dir) {
        return vec![expand_tilde(&dir, home).join("auth.json")];
    }
    home.map(|h| vec![h.join(".pi/agent/auth.json")])
        .unwrap_or_default()
}

pub fn pi_paths() -> Vec<PathBuf> {
    pi_paths_from(
        dirs::home_dir().as_deref(),
        non_empty_env("PI_AUTH_PATH").as_deref(),
        non_empty_env("PI_CODING_AGENT_DIR").as_deref(),
    )
}

fn normalize_omp_profile(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() || trimmed == "default" {
        return None;
    }
    if trimmed == "." || trimmed == ".." || trimmed.ends_with('.') || trimmed.len() > 64 {
        return None;
    }
    let mut chars = trimmed.chars();
    let first = chars.next()?;
    if !matches!(first, 'a'..='z' | '0'..='9') {
        return None;
    }
    if !chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '.' | '_' | '-')) {
        return None;
    }
    let stem = trimmed.split('.').next().unwrap_or(trimmed);
    let upper = stem.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit())
    {
        return None;
    }
    Some(trimmed.to_string())
}

fn omp_paths_from(
    home: Option<&Path>,
    omp_agent_db_path: Option<&str>,
    omp_profile: Option<&str>,
    pi_profile: Option<&str>,
    pi_config_dir: Option<&str>,
    pi_coding_agent_dir: Option<&str>,
    xdg_data_home: Option<&str>,
    use_xdg: bool,
) -> Vec<PathBuf> {
    if let Some(path) = non_empty_trimmed(omp_agent_db_path) {
        return vec![expand_tilde(&path, home)];
    }
    // OMP_PROFILE takes precedence even when explicitly empty (selecting default).
    let active_profile = if omp_profile.is_some() {
        normalize_omp_profile(omp_profile)
    } else {
        normalize_omp_profile(pi_profile)
    };
    let config_dir_name = non_empty_trimmed(pi_config_dir).unwrap_or_else(|| ".omp".into());
    let Some(home) = home else {
        return non_empty_trimmed(pi_coding_agent_dir)
            .map(|dir| vec![PathBuf::from(dir).join("agent.db")])
            .unwrap_or_default();
    };
    let base_root = home.join(&config_dir_name);
    let config_root = match &active_profile {
        Some(profile) => base_root.join("profiles").join(profile),
        None => base_root.clone(),
    };
    let default_agent_dir = config_root.join("agent");
    let pi_profile_agent_dir = normalize_omp_profile(pi_profile)
        .map(|profile| base_root.join("profiles").join(profile).join("agent"));
    let agent_dir = if active_profile.is_some() {
        default_agent_dir.clone()
    } else if let Some(dir) = non_empty_trimmed(pi_coding_agent_dir) {
        let expanded = expand_tilde(&dir, Some(home));
        if pi_profile_agent_dir.as_ref() == Some(&expanded) {
            default_agent_dir.clone()
        } else {
            expanded
        }
    } else {
        default_agent_dir.clone()
    };
    let is_default = agent_dir == default_agent_dir;
    if use_xdg && is_default {
        if let Some(xdg) = non_empty_trimmed(xdg_data_home) {
            let app_root = expand_tilde(&xdg, Some(home)).join("omp");
            let candidate = match &active_profile {
                Some(profile) => app_root.join("profiles").join(profile),
                None => app_root,
            };
            if candidate.exists() {
                return vec![candidate.join("agent.db")];
            }
        }
    }
    vec![agent_dir.join("agent.db")]
}

pub fn omp_paths() -> Vec<PathBuf> {
    let omp_profile = std::env::var("OMP_PROFILE").ok();
    let pi_profile = std::env::var("PI_PROFILE").ok();
    omp_paths_from(
        dirs::home_dir().as_deref(),
        non_empty_env("OMP_AGENT_DB_PATH").as_deref(),
        omp_profile.as_deref(),
        pi_profile.as_deref(),
        non_empty_env("PI_CONFIG_DIR").as_deref(),
        non_empty_env("PI_CODING_AGENT_DIR").as_deref(),
        non_empty_env("XDG_DATA_HOME").as_deref(),
        cfg!(any(target_os = "linux", target_os = "macos")),
    )
}

/// Providers whose auth rule sets `org-scoped-identity` in oh-my-pi: one
/// email may hold several workspaces, each its own credential.
fn omp_org_scoped(provider: &str) -> bool {
    matches!(provider, OPENAI_CODEX_PROVIDER | ANTHROPIC_PROVIDER)
}

/// Mirrors `resolveProviderCredentialIdentityKey` in oh-my-pi's
/// `SqliteAuthCredentialStore`.
fn format_omp_identity_key(
    org_scoped: bool,
    email: Option<&str>,
    account_id: Option<&str>,
    org_id: Option<&str>,
) -> Option<String> {
    let email = email.map(|email| format!("email:{email}"));
    let account = account_id.map(|id| format!("account:{id}"));
    if !org_scoped {
        return account.or(email);
    }
    let base = email.or(account);
    let org = org_id.map(|id| format!("org:{id}"));
    match (base, org) {
        (Some(base), Some(org)) => Some(format!("{base}|{org}")),
        (Some(base), None) => Some(base),
        (None, Some(org)) => Some(org),
        (None, None) => None,
    }
}

fn omp_identity_key(credential: &OAuthCredential, org_scoped: bool) -> Option<String> {
    format_omp_identity_key(
        org_scoped,
        credential.email.as_deref(),
        credential.account_id.as_deref(),
        credential.org_id.as_deref(),
    )
}

fn omp_data_json(credential: &OAuthCredential) -> Result<String, String> {
    let mut map = serde_json::Map::new();
    map.insert("access".into(), credential.access.clone().into());
    map.insert("refresh".into(), credential.refresh.clone().into());
    map.insert("expires".into(), credential.expires_ms.into());
    if let Some(account_id) = &credential.account_id {
        map.insert("accountId".into(), account_id.clone().into());
    }
    if let Some(org_id) = &credential.org_id {
        map.insert("orgId".into(), org_id.clone().into());
    }
    if let Some(email) = &credential.email {
        map.insert("email".into(), email.clone().into());
    }
    if let Some(org_name) = &credential.org_name {
        map.insert("orgName".into(), org_name.clone().into());
    }
    if let Some(scope) = &credential.scope {
        map.insert("scope".into(), scope.clone().into());
    }
    serde_json::to_string(&Value::Object(map)).map_err(|e| e.to_string())
}

fn omp_extract_row_email(data: &Value) -> Option<String> {
    non_empty_trimmed(data.get("email").and_then(Value::as_str))
        .or_else(|| {
            let access = data.get("access").and_then(Value::as_str)?;
            let claims = auth::decode_jwt_payload(access).ok()?;
            auth::jwt_openai_email(&claims)
        })
        .map(|email| email.to_ascii_lowercase())
}

/// The stored `accountId`, else the access token's account claim in the
/// order oh-my-pi reads it (`account_id`, `accountId`, `user_id`, `sub`,
/// then ChatGPT's account id).
fn omp_extract_row_account_id(data: &Value) -> Option<String> {
    non_empty_trimmed(data.get("accountId").and_then(Value::as_str)).or_else(|| {
        let access = data.get("access").and_then(Value::as_str)?;
        let claims = auth::decode_jwt_payload(access).ok()?;
        ["account_id", "accountId", "user_id", "sub"]
            .into_iter()
            .find_map(|key| non_empty_trimmed(claims.get(key).and_then(Value::as_str)))
            .or_else(|| auth::jwt_chatgpt_account_id(&claims))
    })
}

fn omp_resolve_row_identity_key(
    org_scoped: bool,
    stored_identity_key: Option<&str>,
    data: Option<&Value>,
) -> Option<String> {
    if let Some(key) = non_empty_trimmed(stored_identity_key) {
        return Some(key);
    }
    let data = data?;
    let email = omp_extract_row_email(data);
    let account_id = omp_extract_row_account_id(data);
    let org_id = non_empty_trimmed(data.get("orgId").and_then(Value::as_str));
    format_omp_identity_key(
        org_scoped,
        email.as_deref(),
        account_id.as_deref(),
        org_id.as_deref(),
    )
}

/// Mirrors `matchesReplacementCredential` in oh-my-pi's
/// `SqliteAuthCredentialStore`, preserving workspace isolation between distinct
/// users who share a workspace `orgId`.
fn omp_matches_replacement(
    org_scoped: bool,
    existing_type: &str,
    existing_data_raw: &str,
    existing_identity_key: Option<&str>,
    incoming: &OAuthCredential,
    incoming_identity_key: Option<&str>,
) -> bool {
    if existing_type != "oauth" {
        return false;
    }
    let Some(incoming_key) = incoming_identity_key else {
        return false;
    };
    let existing_data = serde_json::from_str::<Value>(existing_data_raw).ok();
    let existing_key =
        omp_resolve_row_identity_key(org_scoped, existing_identity_key, existing_data.as_ref());
    let Some(existing_key) = existing_key else {
        return false;
    };
    if existing_key == incoming_key {
        return true;
    }
    // Only org-qualified keys upgrade a row keyed otherwise.
    if !org_scoped {
        return false;
    }
    let Some(org_id) = incoming.org_id.as_deref() else {
        return false;
    };
    let org_identifier = format!("org:{org_id}");
    if incoming_key != org_identifier && !incoming_key.ends_with(&format!("|{org_identifier}")) {
        return false;
    }
    if existing_key == org_identifier {
        return true;
    }
    // A base repeating the org's id is no per-user identity (openai-codex
    // stores the Team workspace id as both accountId and orgId).
    let bases = [
        incoming
            .email
            .as_deref()
            .map(|email| format!("email:{email}")),
        incoming
            .account_id
            .as_deref()
            .filter(|id| *id != org_id)
            .map(|id| format!("account:{id}")),
    ];
    // A same-org row's credential may carry identifiers its key does not use.
    let existing_identifiers: Vec<String> = match existing_data.as_ref() {
        Some(data) if existing_key.ends_with(&format!("|{org_identifier}")) => [
            omp_extract_row_email(data).map(|email| format!("email:{email}")),
            omp_extract_row_account_id(data).map(|id| format!("account:{id}")),
        ]
        .into_iter()
        .flatten()
        .collect(),
        _ => vec![],
    };
    bases.into_iter().flatten().any(|base| {
        existing_key == base
            || existing_key == format!("{base}|{org_identifier}")
            || existing_identifiers.contains(&base)
    })
}

fn sync_omp_one(credential: &OAuthCredential, provider: &str, path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("Invalid omp database path")?;
    let parent_existed = parent.exists();
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    if !parent_existed {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = parent_existed;

    let existed = path.exists();
    let mut conn = Connection::open(path).map_err(|e| e.to_string())?;
    if !existed {
        crate::utils::set_private_permissions(path);
    }

    conn.busy_timeout(Duration::from_millis(5000))
        .map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
        .map_err(|e| e.to_string())?;

    let table_exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'auth_credentials'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .is_some();

    if !table_exists {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS auth_credentials (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider TEXT NOT NULL,
                credential_type TEXT NOT NULL,
                data TEXT NOT NULL,
                disabled_cause TEXT DEFAULT NULL,
                identity_key TEXT DEFAULT NULL,
                created_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER)),
                updated_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER))
            );
            CREATE INDEX IF NOT EXISTS idx_auth_provider ON auth_credentials(provider);
            CREATE INDEX IF NOT EXISTS idx_auth_provider_identity ON auth_credentials(provider, identity_key) WHERE identity_key IS NOT NULL;",
        )
        .map_err(|e| e.to_string())?;
    }

    let columns: HashSet<String> = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(auth_credentials)")
            .map_err(|e| e.to_string())?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| e.to_string())?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(|e| e.to_string())?;
        names
    };
    if !["id", "provider", "credential_type", "data"]
        .into_iter()
        .all(|col| columns.contains(col))
    {
        return Err("Invalid omp auth_credentials table".into());
    }
    let has_disabled_cause = columns.contains("disabled_cause");
    let has_disabled = columns.contains("disabled");
    let has_identity_key = columns.contains("identity_key");
    let has_created_at = columns.contains("created_at");
    let has_updated_at = columns.contains("updated_at");

    let data_json = omp_data_json(credential)?;
    let org_scoped = omp_org_scoped(provider);
    let identity_key = omp_identity_key(credential, org_scoped);

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;

    let select_active_sql = match (has_identity_key, has_disabled_cause, has_disabled) {
        (true, true, _) => "SELECT id, credential_type, data, identity_key FROM auth_credentials WHERE provider = ?1 AND disabled_cause IS NULL ORDER BY id ASC",
        (false, true, _) => "SELECT id, credential_type, data, NULL FROM auth_credentials WHERE provider = ?1 AND disabled_cause IS NULL ORDER BY id ASC",
        (true, false, true) => "SELECT id, credential_type, data, identity_key FROM auth_credentials WHERE provider = ?1 AND COALESCE(disabled, 0) = 0 ORDER BY id ASC",
        (false, false, true) => "SELECT id, credential_type, data, NULL FROM auth_credentials WHERE provider = ?1 AND COALESCE(disabled, 0) = 0 ORDER BY id ASC",
        (true, false, false) => "SELECT id, credential_type, data, identity_key FROM auth_credentials WHERE provider = ?1 ORDER BY id ASC",
        (false, false, false) => "SELECT id, credential_type, data, NULL FROM auth_credentials WHERE provider = ?1 ORDER BY id ASC",
    };

    let active_rows: Vec<(i64, String, String, Option<String>)> = {
        let mut stmt = tx.prepare(select_active_sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![provider], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };

    let matched = active_rows.iter().find(|(_, ctype, data, id_key)| {
        omp_matches_replacement(
            org_scoped,
            ctype,
            data,
            id_key.as_deref(),
            credential,
            identity_key.as_deref(),
        )
    });

    let target_id = if let Some((id, ctype, existing_data, existing_id_key)) = matched {
        let unchanged = ctype == "oauth"
            && existing_data == &data_json
            && (!has_identity_key || existing_id_key == &identity_key);
        if !unchanged {
            match (has_identity_key, has_updated_at) {
                (true, true) => {
                    tx.execute(
                        "UPDATE auth_credentials SET credential_type = 'oauth', data = ?1, identity_key = ?2, updated_at = CAST(strftime('%s','now') AS INTEGER) WHERE id = ?3",
                        params![&data_json, &identity_key, id],
                    )
                    .map_err(|e| e.to_string())?;
                }
                (true, false) => {
                    tx.execute(
                        "UPDATE auth_credentials SET credential_type = 'oauth', data = ?1, identity_key = ?2 WHERE id = ?3",
                        params![&data_json, &identity_key, id],
                    )
                    .map_err(|e| e.to_string())?;
                }
                (false, true) => {
                    tx.execute(
                        "UPDATE auth_credentials SET credential_type = 'oauth', data = ?1, updated_at = CAST(strftime('%s','now') AS INTEGER) WHERE id = ?2",
                        params![&data_json, id],
                    )
                    .map_err(|e| e.to_string())?;
                }
                (false, false) => {
                    tx.execute(
                        "UPDATE auth_credentials SET credential_type = 'oauth', data = ?1 WHERE id = ?2",
                        params![&data_json, id],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }
        *id
    } else {
        match (has_identity_key, has_created_at && has_updated_at) {
            (true, true) => {
                tx.execute(
                    "INSERT INTO auth_credentials (provider, credential_type, data, identity_key, created_at, updated_at) VALUES (?1, 'oauth', ?2, ?3, CAST(strftime('%s','now') AS INTEGER), CAST(strftime('%s','now') AS INTEGER))",
                    params![provider, &data_json, &identity_key],
                )
                .map_err(|e| e.to_string())?;
            }
            (true, false) => {
                tx.execute(
                    "INSERT INTO auth_credentials (provider, credential_type, data, identity_key) VALUES (?1, 'oauth', ?2, ?3)",
                    params![provider, &data_json, &identity_key],
                )
                .map_err(|e| e.to_string())?;
            }
            (false, true) => {
                tx.execute(
                    "INSERT INTO auth_credentials (provider, credential_type, data, created_at, updated_at) VALUES (?1, 'oauth', ?2, CAST(strftime('%s','now') AS INTEGER), CAST(strftime('%s','now') AS INTEGER))",
                    params![provider, &data_json],
                )
                .map_err(|e| e.to_string())?;
            }
            (false, false) => {
                tx.execute(
                    "INSERT INTO auth_credentials (provider, credential_type, data) VALUES (?1, 'oauth', ?2)",
                    params![provider, &data_json],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        tx.last_insert_rowid()
    };

    for (id, _, _, _) in &active_rows {
        if *id == target_id {
            continue;
        }
        if has_disabled_cause {
            if has_updated_at {
                tx.execute(
                    "UPDATE auth_credentials SET disabled_cause = ?1, updated_at = CAST(strftime('%s','now') AS INTEGER) WHERE id = ?2 AND disabled_cause IS NULL",
                    params![OMP_REPLACED_CAUSE, id],
                )
                .map_err(|e| e.to_string())?;
            } else {
                tx.execute(
                    "UPDATE auth_credentials SET disabled_cause = ?1 WHERE id = ?2 AND disabled_cause IS NULL",
                    params![OMP_REPLACED_CAUSE, id],
                )
                .map_err(|e| e.to_string())?;
            }
        } else if has_disabled {
            if has_updated_at {
                tx.execute(
                    "UPDATE auth_credentials SET disabled = 1, updated_at = CAST(strftime('%s','now') AS INTEGER) WHERE id = ?1",
                    params![id],
                )
                .map_err(|e| e.to_string())?;
            } else {
                tx.execute(
                    "UPDATE auth_credentials SET disabled = 1 WHERE id = ?1",
                    params![id],
                )
                .map_err(|e| e.to_string())?;
            }
        } else {
            tx.execute("DELETE FROM auth_credentials WHERE id = ?1", params![id])
                .map_err(|e| e.to_string())?;
        }
    }

    if has_disabled_cause && identity_key.is_some() {
        let select_disabled_sql = if has_identity_key {
            "SELECT id, credential_type, data, identity_key FROM auth_credentials WHERE provider = ?1 AND disabled_cause IS NOT NULL ORDER BY id ASC"
        } else {
            "SELECT id, credential_type, data, NULL FROM auth_credentials WHERE provider = ?1 AND disabled_cause IS NOT NULL ORDER BY id ASC"
        };
        let disabled_rows: Vec<(i64, String, String, Option<String>)> = {
            let mut stmt = tx.prepare(select_disabled_sql).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![provider], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            rows
        };
        for (id, ctype, data, id_key) in disabled_rows {
            if omp_matches_replacement(
                org_scoped,
                &ctype,
                &data,
                id_key.as_deref(),
                credential,
                identity_key.as_deref(),
            ) {
                tx.execute("DELETE FROM auth_credentials WHERE id = ?1", params![id])
                    .map_err(|e| e.to_string())?;
            }
        }
    }

    tx.commit().map_err(|e| e.to_string())
}

fn sync_omp_credential(
    credential: &OAuthCredential,
    provider: &str,
    targets: &[PathBuf],
) -> Result<(), String> {
    sync_targets(
        targets,
        "Unable to locate omp agent database path",
        |path| sync_omp_one(credential, provider, path),
    )
}

pub fn sync_omp(auth_json: &Value, targets: &[PathBuf]) -> Result<(), String> {
    SyncAgent::Omp.sync_raw(Pool::Codex, auth_json, targets)
}
pub fn opencode_paths() -> Vec<PathBuf> {
    if let Some(p) = std::env::var_os("OPENCODE_AUTH_PATH").filter(|p| !p.is_empty()) {
        return vec![p.into()];
    }
    let mut paths = vec![];
    for (env, suffix) in [
        ("OPENCODE_CONFIG_HOME", "auth.json"),
        ("XDG_CONFIG_HOME", "opencode/auth.json"),
    ] {
        if let Some(p) = std::env::var_os(env).filter(|p| !p.is_empty()) {
            paths.push(PathBuf::from(p).join(suffix));
        }
    }
    if let Some(home) = dirs::home_dir() {
        paths.extend([
            home.join(".config/opencode/auth.json"),
            home.join("Library/Application Support/opencode/auth.json"),
        ]);
    }
    if let Some(p) = std::env::var_os("XDG_DATA_HOME").filter(|p| !p.is_empty()) {
        paths.push(PathBuf::from(p).join("opencode/auth.json"));
    }
    if let Some(home) = dirs::home_dir() {
        paths.extend([
            home.join(".local/share/opencode/auth.json"),
            home.join(".opencode/auth.json"),
        ]);
    }
    let existing: Vec<_> = paths.iter().filter(|p| p.exists()).cloned().collect();
    if existing.is_empty() {
        paths.into_iter().take(1).collect()
    } else {
        existing
    }
}
#[derive(Clone, Serialize)]
pub struct Editor {
    pub id: String,
    pub label: String,
    pub path: PathBuf,
    pub process: String,
}
pub fn editors() -> Vec<Editor> {
    [
        ("vscode", "VS Code", "Visual Studio Code", "code", "Code"),
        (
            "vscodeInsiders",
            "VS Code Insiders",
            "Visual Studio Code - Insiders",
            "code-insiders",
            "Code - Insiders",
        ),
        ("cursor", "Cursor", "Cursor", "cursor", "Cursor"),
        (
            "antigravity",
            "Antigravity",
            "Antigravity",
            "antigravity",
            "Antigravity",
        ),
        ("kiro", "Kiro", "Kiro", "kiro", "Kiro"),
        ("trae", "Trae", "Trae", "trae", "Trae"),
        ("qoder", "Qoder", "Qoder", "qoder", "Qoder"),
    ]
    .into_iter()
    .filter_map(|(id, label, bundle, bin, process)| {
        let path = if cfg!(target_os = "macos") {
            [
                PathBuf::from("/Applications"),
                dirs::home_dir()?.join("Applications"),
            ]
            .into_iter()
            .map(|p| p.join(format!("{bundle}.app")))
            .find(|p| p.exists())
        } else {
            executable(bin)
        }?;
        Some(Editor {
            id: id.into(),
            label: label.into(),
            path,
            process: if cfg!(target_os = "linux") {
                bin
            } else {
                process
            }
            .into(),
        })
    })
    .collect()
}
fn record_sync(outcome: Result<(), String>, synced: &mut bool, sync_error: &mut Option<String>) {
    match outcome {
        Ok(()) => *synced = true,
        Err(error) => *sync_error = Some(error),
    }
}

/// Side effects of a switch, reported as Swift's SwitchAccountExecutionResult:
/// the switch itself has succeeded whatever fails here.
pub async fn after_switch(
    pool: Pool,
    p: &Preferences,
    auth: Option<&Value>,
) -> Result<SwitchExecution, String> {
    let mut result = SwitchExecution::default();
    if let Some(auth) = auth {
        let sync_opencode_enabled =
            p.sync_opencode_openai_auth && SyncAgent::Opencode.provider_key(pool).is_some();
        let sync_pi_enabled = p.sync_pi_auth && SyncAgent::Pi.provider_key(pool).is_some();
        let sync_omp_enabled = p.sync_omp_auth && SyncAgent::Omp.provider_key(pool).is_some();
        if sync_opencode_enabled || sync_pi_enabled || sync_omp_enabled {
            let extracted = extract_oauth_credential(pool, auth);
            if sync_opencode_enabled {
                let outcome = extracted.as_ref().map_err(Clone::clone).and_then(|cred| {
                    SyncAgent::Opencode.sync_credential(pool, cred, &opencode_paths())
                });
                record_sync(
                    outcome,
                    &mut result.opencode_synced,
                    &mut result.opencode_sync_error,
                );
            }
            if sync_pi_enabled {
                let outcome = extracted
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|cred| SyncAgent::Pi.sync_credential(pool, cred, &pi_paths()));
                record_sync(outcome, &mut result.pi_synced, &mut result.pi_sync_error);
            }
            if sync_omp_enabled {
                let outcome = match extracted {
                    Ok(cred) => {
                        let targets = omp_paths();
                        tokio::task::spawn_blocking(move || {
                            SyncAgent::Omp.sync_credential(pool, &cred, &targets)
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|res| res)
                    }
                    Err(error) => Err(error),
                };
                record_sync(outcome, &mut result.omp_synced, &mut result.omp_sync_error);
            }
        }
    }
    if pool == Pool::Codex && p.restart_editors_on_switch {
        let installed = editors();
        let mut errors = vec![];
        for id in &p.restart_editor_targets {
            let Some(editor) = installed.iter().find(|e| &e.id == id) else {
                errors.push(format!("Editor not installed: {id}"));
                continue;
            };
            #[cfg(unix)]
            {
                let _ = checked("pkill", &["-TERM", "-x", &editor.process], 5).await;
            }
            #[cfg(windows)]
            {
                let _ = checked("taskkill", &["/IM", &format!("{}.exe", editor.process)], 5).await;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            #[cfg(target_os = "macos")]
            let restarted = checked(
                "/usr/bin/open",
                &["-na", &editor.path.to_string_lossy()],
                15,
            )
            .await
            .map(|_| ());
            #[cfg(not(target_os = "macos"))]
            let restarted = std::process::Command::new(&editor.path)
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string());
            match restarted {
                Ok(()) => result.restarted_editors.push(editor.label.clone()),
                Err(e) => errors.push(e),
            }
        }
        if !errors.is_empty() {
            result.editor_restart_error = Some(errors.join(" | "));
        }
    }
    if pool == Pool::Codex && p.launch_codex_after_switch {
        result.launch_error = launch_codex().await.err();
    }
    Ok(result)
}
async fn launch_codex() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join("Applications"));
        }
        let app = roots
            .into_iter()
            .flat_map(|p| [p.join("Codex.app"), p.join("Codex Desktop.app")])
            .find(|p| p.is_dir());
        if let Some(app) = app {
            return checked("/usr/bin/open", &["-a", &app.to_string_lossy()], 15)
                .await
                .map(|_| ());
        }
    }
    #[cfg(target_os = "windows")]
    if let Some(root) = dirs::data_local_dir() {
        for candidate in [
            root.join("Programs/Codex/Codex.exe"),
            root.join("Programs/Codex Desktop/Codex.exe"),
        ] {
            if candidate.is_file() {
                return std::process::Command::new(candidate)
                    .spawn()
                    .map(|_| ())
                    .map_err(|e| e.to_string());
            }
        }
    }
    let cli = executable("codex").ok_or("Codex is not installed or could not be found")?;
    checked(&cli.to_string_lossy(), &["app"], 30)
        .await
        .map(|_| ())
}
pub fn startup(enabled: bool, executable: &Path) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Home directory unavailable")?;
    #[cfg(target_os = "macos")]
    {
        let path = home.join("Library/LaunchAgents/io.github.copool.rust.plist");
        if !enabled {
            if path.exists() {
                std::fs::remove_file(path).map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        let escaped = executable
            .to_string_lossy()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let content=format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>io.github.copool.rust</string><key>ProgramArguments</key><array><string>{escaped}</string></array><key>RunAtLoad</key><true/></dict></plist>");
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(path, content).map_err(|e| e.to_string())
    }
    #[cfg(target_os = "linux")]
    {
        let path = dirs::config_dir()
            .unwrap_or(home.join(".config"))
            .join("autostart/copool.desktop");
        if !enabled {
            if path.exists() {
                std::fs::remove_file(path).map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        let escaped = executable
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$");
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(path,format!("[Desktop Entry]\nType=Application\nName=Copool\nExec=\"{escaped}\"\nTerminal=false\n")).map_err(|e|e.to_string())
    }
    #[cfg(target_os = "windows")]
    {
        let _ = home;
        let mut command = std::process::Command::new("reg");
        let key = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run";
        if enabled {
            command.args([
                "add",
                key,
                "/v",
                "Copool",
                "/t",
                "REG_SZ",
                "/d",
                &format!("\"{}\"", executable.display()),
                "/f",
            ]);
        } else {
            command.args(["delete", key, "/v", "Copool", "/f"]);
        }
        let output = command.output().map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).into_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    fn fake_jwt(payload: Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        format!("{header}.{body}.sig")
    }

    #[test]
    fn sync_opencode_writes_openai_oauth_and_replaces_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        write_json(
            &path,
            &json!({
                "openai": { "type": "api", "key": "sk-old" },
                "xai": { "type": "oauth", "access": "xai-access", "refresh": "xai-refresh", "expires": 1_700_000_000_000_i64 }
            }),
        )
        .unwrap();

        let id_token = fake_jwt(json!({"exp": 1_800_000_456_i64}));
        let auth = json!({
            "tokens": {
                "access_token": "openai-access",
                "refresh_token": "openai-refresh",
                "id_token": id_token,
                "account_id": "acct-42"
            }
        });
        sync_opencode(&auth, std::slice::from_ref(&path)).unwrap();

        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["openai"]["type"], "oauth");
        assert_eq!(saved["openai"]["access"], "openai-access");
        assert_eq!(saved["openai"]["refresh"], "openai-refresh");
        assert_eq!(saved["openai"]["expires"], 1_800_000_456_000_i64);
        assert_eq!(saved["openai"]["accountId"], "acct-42");
        assert!(saved["openai"].get("key").is_none());
        assert_eq!(saved["xai"]["access"], "xai-access");
    }

    #[test]
    fn sync_opencode_grok_writes_xai_oauth_and_keeps_other_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        write_json(
            &path,
            &json!({
                "openai": {
                    "type": "oauth",
                    "access": "openai-access",
                    "refresh": "openai-refresh",
                    "expires": 1_700_000_000_000_i64,
                    "accountId": "acct-1"
                },
                "xai": {
                    "type": "api",
                    "key": "xai-old-api-key"
                }
            }),
        )
        .unwrap();

        let doc = json!({
            "key": "grok-access-1",
            "refresh_token": "grok-refresh-1",
            "expires_at": "2026-11-26T12:00:00.000000Z"
        });
        sync_opencode_grok(&doc, std::slice::from_ref(&path)).unwrap();

        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["openai"]["access"], "openai-access");
        assert_eq!(saved["openai"]["accountId"], "acct-1");
        assert_eq!(saved["xai"]["type"], "oauth");
        assert_eq!(saved["xai"]["access"], "grok-access-1");
        assert_eq!(saved["xai"]["refresh"], "grok-refresh-1");
        assert_eq!(
            saved["xai"]["expires"],
            chrono::DateTime::parse_from_rfc3339("2026-11-26T12:00:00Z")
                .unwrap()
                .timestamp()
                * 1000
        );
        assert!(saved["xai"].get("key").is_none());
        assert!(saved["xai"].get("accountId").is_none());
    }

    #[test]
    fn sync_opencode_grok_reads_wrapped_auth_and_jwt_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let access = fake_jwt(json!({"exp": 1_800_000_123_i64}));
        let wrapped = json!({
            "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {
                "key": access,
                "refresh_token": "grok-refresh-wrapped"
            }
        });

        sync_opencode_grok(&wrapped, std::slice::from_ref(&path)).unwrap();

        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["xai"]["type"], "oauth");
        assert_eq!(saved["xai"]["access"], access);
        assert_eq!(saved["xai"]["refresh"], "grok-refresh-wrapped");
        assert_eq!(saved["xai"]["expires"], 1_800_000_123_000_i64);
    }

    #[test]
    fn sync_opencode_grok_errors_when_tokens_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        assert_eq!(
            sync_opencode_grok(
                &json!({"refresh_token": "only-refresh"}),
                std::slice::from_ref(&path)
            )
            .unwrap_err(),
            "Missing access token"
        );
        assert_eq!(
            sync_opencode_grok(&json!({"key": "only-access"}), std::slice::from_ref(&path))
                .unwrap_err(),
            "Missing refresh token"
        );
    }

    #[tokio::test]
    async fn after_switch_outside_codex_skips_editors_codex_launch_and_unsupported_agents() {
        // Agents stay off for Grok: enabled, they would write the real home.
        let grok_preferences = Preferences {
            sync_opencode_openai_auth: false,
            sync_pi_auth: false,
            sync_omp_auth: false,
            restart_editors_on_switch: true,
            restart_editor_targets: vec!["not-installed-editor".into()],
            launch_codex_after_switch: true,
            ..Default::default()
        };
        let doc = json!({
            "key": "grok-access",
            "refresh_token": "grok-refresh",
            "expires_at": "2026-11-26T12:00:00Z"
        });
        let execution = after_switch(Pool::Grok, &grok_preferences, Some(&doc))
            .await
            .unwrap();
        assert_eq!(execution, SwitchExecution::default());
        // OpenCode and pi do not take an Antigravity login, and OpenCode does
        // not take Claude; keep supported agents off so tests never touch home.
        let antigravity_preferences = Preferences {
            sync_opencode_openai_auth: true,
            sync_pi_auth: true,
            sync_omp_auth: false,
            ..grok_preferences.clone()
        };
        let doc = json!({
            "token": {"access_token": "a", "refresh_token": "r"},
            "id_token_email": "alice@gmail.com"
        });
        let execution = after_switch(Pool::Antigravity, &antigravity_preferences, Some(&doc))
            .await
            .unwrap();
        assert_eq!(execution, SwitchExecution::default());

        let claude_preferences = Preferences {
            sync_opencode_openai_auth: true,
            sync_pi_auth: false,
            sync_omp_auth: false,
            ..grok_preferences
        };
        let execution = after_switch(
            Pool::Claude,
            &claude_preferences,
            Some(&sample_claude_doc("alice", "org-1", "rt-1")),
        )
        .await
        .unwrap();
        assert_eq!(execution, SwitchExecution::default());
    }

    fn sample_claude_doc(name: &str, org: &str, refresh: &str) -> Value {
        json!({
            "claudeAiOauth": {
                "accessToken": format!("sk-ant-oat-{name}"),
                "refreshToken": refresh,
                "expiresAt": 1_780_000_000_000_i64,
                "scopes": ["user:inference"],
                "subscriptionType": "max"
            },
            "oauthAccount": {
                "accountUuid": format!("acct-{name}"),
                "emailAddress": format!("{}@Example.com", name),
                "organizationUuid": org,
                "organizationName": format!("{name}'s Organization")
            }
        })
    }

    fn sample_grok_doc(user: &str, refresh: &str) -> Value {
        json!({
            "key": format!("grok-access-{user}"),
            "refresh_token": refresh,
            "expires_at": "2026-11-26T12:00:00Z",
            "user_id": format!("uid-{user}"),
            "email": format!("{user}@x.example"),
            "team_id": "team-shared"
        })
    }

    #[test]
    fn sync_pi_writes_anthropic_and_xai_entries_beside_openai_codex() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let codex = sample_codex_auth("a@example.com", "acct-1", "plus", 1_780_000_000, "rt-1");
        sync_pi(&codex, std::slice::from_ref(&path)).unwrap();
        SyncAgent::Pi
            .sync_raw(
                Pool::Claude,
                &sample_claude_doc("alice", "org-1", "claude-rt"),
                std::slice::from_ref(&path),
            )
            .unwrap();
        SyncAgent::Pi
            .sync_raw(
                Pool::Grok,
                &sample_grok_doc("bob", "grok-rt"),
                std::slice::from_ref(&path),
            )
            .unwrap();

        let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["openai-codex"]["refresh"], "rt-1");
        assert_eq!(written["anthropic"]["type"], "oauth");
        assert_eq!(written["anthropic"]["access"], "sk-ant-oat-alice");
        assert_eq!(written["anthropic"]["refresh"], "claude-rt");
        assert_eq!(written["anthropic"]["expires"], 1_780_000_000_000_i64);
        assert!(written["anthropic"].get("accountId").is_none());
        assert_eq!(written["xai"]["type"], "oauth");
        assert_eq!(written["xai"]["access"], "grok-access-bob");
        assert_eq!(written["xai"]["refresh"], "grok-rt");
        assert!(written["xai"].get("accountId").is_none());
        assert_eq!(
            written["xai"]["expires"],
            chrono::DateTime::parse_from_rfc3339("2026-11-26T12:00:00Z")
                .unwrap()
                .timestamp()
                * 1000
        );
    }

    fn omp_rows(
        db_path: &Path,
        provider: &str,
    ) -> Vec<(i64, Value, Option<String>, Option<String>)> {
        let conn = Connection::open(db_path).unwrap();
        let mut stmt = conn
            .prepare("SELECT id, data, identity_key, disabled_cause FROM auth_credentials WHERE provider = ?1 ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map(params![provider], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    serde_json::from_str::<Value>(&r.get::<_, String>(1)?).unwrap(),
                    r.get(2)?,
                    r.get(3)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    }

    #[test]
    fn sync_omp_keys_anthropic_by_email_and_organization() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        let sync = |doc: &Value| {
            SyncAgent::Omp
                .sync_raw(Pool::Claude, doc, std::slice::from_ref(&db_path))
                .unwrap()
        };
        sync(&sample_claude_doc("alice", "org-team", "rt-1"));
        let rows = omp_rows(&db_path, "anthropic");
        assert_eq!(rows.len(), 1);
        let (alice_id, data, key, cause) = &rows[0];
        assert_eq!(key.as_deref(), Some("email:alice@example.com|org:org-team"));
        assert_eq!(cause, &None);
        assert_eq!(data["access"], "sk-ant-oat-alice");
        assert_eq!(data["refresh"], "rt-1");
        assert_eq!(data["accountId"], "acct-alice");
        assert_eq!(data["orgId"], "org-team");
        assert_eq!(data["email"], "alice@example.com");
        assert_eq!(data["orgName"], "alice's Organization");
        assert_eq!(data["scope"], "user:inference");

        // A renewal of the same login updates its row in place.
        sync(&sample_claude_doc("alice", "org-team", "rt-2"));
        let rows = omp_rows(&db_path, "anthropic");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, *alice_id);
        assert_eq!(rows[0].1["refresh"], "rt-2");

        // The same email in another organization is another subscription.
        sync(&sample_claude_doc("alice", "org-personal", "rt-3"));
        let rows = omp_rows(&db_path, "anthropic");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].3.as_deref(), Some(OMP_REPLACED_CAUSE));
        assert_eq!(
            rows[1].2.as_deref(),
            Some("email:alice@example.com|org:org-personal")
        );
        assert_eq!(rows[1].3, None);
    }

    #[test]
    fn sync_omp_keys_xai_oauth_by_account_without_organization() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        let sync = |doc: &Value| {
            SyncAgent::Omp
                .sync_raw(Pool::Grok, doc, std::slice::from_ref(&db_path))
                .unwrap()
        };
        sync(&sample_grok_doc("alice", "rt-a1"));
        let rows = omp_rows(&db_path, "xai-oauth");
        assert_eq!(rows.len(), 1);
        let (alice_id, data, key, _) = &rows[0];
        assert_eq!(key.as_deref(), Some("account:uid-alice"));
        assert_eq!(data["access"], "grok-access-alice");
        assert_eq!(data["accountId"], "uid-alice");
        assert_eq!(data["email"], "alice@x.example");
        assert!(data.get("orgId").is_none());

        sync(&sample_grok_doc("alice", "rt-a2"));
        let rows = omp_rows(&db_path, "xai-oauth");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, *alice_id);
        assert_eq!(rows[0].1["refresh"], "rt-a2");

        // A teammate is another account: inserted, Alice's row disabled.
        sync(&sample_grok_doc("bob", "rt-b1"));
        let rows = omp_rows(&db_path, "xai-oauth");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].3.as_deref(), Some(OMP_REPLACED_CAUSE));
        assert_eq!(rows[1].2.as_deref(), Some("account:uid-bob"));
        assert_eq!(rows[1].3, None);
    }

    #[test]
    fn sync_omp_writes_antigravity_cli_with_identity_and_rejects_nameless_grant() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        let nameless = json!({
            "token": {
                "access_token": "agy-access",
                "refresh_token": "agy-refresh",
                "expiry": "2026-11-26T12:00:00Z"
            },
            "auth_method": "consumer"
        });
        assert_eq!(
            SyncAgent::Omp
                .sync_raw(Pool::Antigravity, &nameless, std::slice::from_ref(&db_path))
                .unwrap_err(),
            "Missing Antigravity account identity"
        );

        let alice = json!({
            "token": {
                "access_token": "agy-access-alice",
                "refresh_token": "agy-rt-a1",
                "expiry": "2026-11-26T12:00:00Z"
            },
            "auth_method": "consumer",
            "id_token_email": "Alice@Gmail.com",
            "account_id": "google-sub-alice"
        });
        SyncAgent::Omp
            .sync_raw(Pool::Antigravity, &alice, std::slice::from_ref(&db_path))
            .unwrap();
        let rows = omp_rows(&db_path, "antigravity-cli");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].2.as_deref(), Some("account:google-sub-alice"));
        assert_eq!(rows[0].1["access"], "agy-access-alice");
        assert_eq!(rows[0].1["refresh"], "agy-rt-a1");
        assert_eq!(rows[0].1["email"], "alice@gmail.com");
        assert_eq!(rows[0].1["accountId"], "google-sub-alice");
        assert_eq!(
            rows[0].1["expires"],
            chrono::DateTime::parse_from_rfc3339("2026-11-26T12:00:00Z")
                .unwrap()
                .timestamp()
                * 1000
        );

        // Email-only grant (as `agy acp --login` often exports) keys by email.
        let bob = json!({
            "token": {
                "access_token": "agy-access-bob",
                "refresh_token": "agy-rt-b1",
                "expiry": "2026-11-26T13:00:00Z"
            },
            "auth_method": "consumer",
            "id_token_email": "Bob@Gmail.com"
        });
        SyncAgent::Omp
            .sync_raw(Pool::Antigravity, &bob, std::slice::from_ref(&db_path))
            .unwrap();
        let rows = omp_rows(&db_path, "antigravity-cli");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].3.as_deref(), Some(OMP_REPLACED_CAUSE));
        assert_eq!(rows[1].2.as_deref(), Some("email:bob@gmail.com"));
        assert_eq!(rows[1].3, None);
    }

    fn sample_codex_auth(
        email: &str,
        account_id: &str,
        plan_type: &str,
        exp_sec: i64,
        refresh: &str,
    ) -> Value {
        let access = fake_jwt(json!({
            "exp": exp_sec,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "chatgpt_plan_type": plan_type,
            },
            "https://api.openai.com/profile": {
                "email": email,
            }
        }));
        let id = fake_jwt(json!({
            "exp": exp_sec - 100,
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "chatgpt_plan_type": plan_type,
            }
        }));
        json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": access,
                "refresh_token": refresh,
                "id_token": id,
                "account_id": account_id,
            }
        })
    }

    #[test]
    fn pi_paths_respect_explicit_agent_dir_and_default() {
        let home = PathBuf::from("/Users/tester");
        assert_eq!(
            pi_paths_from(Some(&home), Some("~/custom/pi-auth.json"), Some("/ignored")),
            vec![home.join("custom/pi-auth.json")]
        );
        assert_eq!(
            pi_paths_from(Some(&home), None, Some("~/my-pi")),
            vec![home.join("my-pi/auth.json")]
        );
        assert_eq!(
            pi_paths_from(Some(&home), None, None),
            vec![home.join(".pi/agent/auth.json")]
        );
    }

    #[test]
    fn sync_opencode_updates_openai_entry_and_preserves_existing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode/auth.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"anthropic":{"type":"api_key","key":"sk-ant"},"openai":{"type":"oauth","extra":"keep"}}"#,
        )
        .unwrap();

        let auth = sample_codex_auth("Alice@Example.com", "acct-1", "Pro", 1_780_000_000, "rt-1");
        sync_opencode(&auth, std::slice::from_ref(&path)).unwrap();

        let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["anthropic"]["key"], "sk-ant");
        assert_eq!(written["openai"]["type"], "oauth");
        assert_eq!(written["openai"]["extra"], "keep");
        assert_eq!(written["openai"]["access"], auth["tokens"]["access_token"]);
        assert_eq!(written["openai"]["refresh"], "rt-1");
        assert_eq!(written["openai"]["expires"], 1_780_000_000_000i64);
        assert_eq!(written["openai"]["accountId"], "acct-1");
    }

    #[test]
    fn sync_pi_writes_openai_codex_oauth_and_preserves_other_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent/auth.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "\u{feff}{\"anthropic\":{\"type\":\"api_key\",\"key\":\"sk-ant\"},\"openai-codex\":{\"type\":\"api_key\",\"key\":\"stale\"}}",
        )
        .unwrap();

        let auth = sample_codex_auth("Alice@Example.com", "acct-1", "Pro", 1_780_000_000, "rt-1");
        sync_pi(&auth, std::slice::from_ref(&path)).unwrap();

        let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["anthropic"]["type"], "api_key");
        assert_eq!(written["anthropic"]["key"], "sk-ant");
        assert_eq!(written["openai-codex"]["type"], "oauth");
        assert_eq!(
            written["openai-codex"]["access"],
            auth["tokens"]["access_token"]
        );
        assert_eq!(written["openai-codex"]["refresh"], "rt-1");
        assert_eq!(written["openai-codex"]["expires"], 1_780_000_000_000i64);
        assert_eq!(written["openai-codex"]["accountId"], "acct-1");
        assert!(written["openai-codex"].get("key").is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn sync_pi_accepts_fractional_jwt_exp_without_falling_back_to_id_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let access = fake_jwt(json!({
            "exp": 1_780_000_123.75,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct-frac",
                "chatgpt_plan_type": "plus",
            }
        }));
        let id = fake_jwt(json!({
            "exp": 1_700_000_000,
            "email": "frac@example.com",
        }));
        let auth = json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": access,
                "refresh_token": "rt-frac",
                "id_token": id,
                "account_id": "acct-frac",
            }
        });

        sync_pi(&auth, std::slice::from_ref(&path)).unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["openai-codex"]["expires"], 1_780_000_123_000i64);
    }

    #[test]
    fn sync_pi_rejects_non_object_document_and_empty_targets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, "[]").unwrap();
        let auth = sample_codex_auth("a@example.com", "acct-1", "plus", 1_780_000_000, "rt-1");
        assert!(sync_pi(&auth, std::slice::from_ref(&path)).is_err());
        assert!(sync_pi(&auth, &[]).is_err());
    }

    #[test]
    fn omp_paths_follow_profiles_env_overrides_and_xdg() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let xdg = dir.path().join("xdg-data");
        std::fs::create_dir_all(&home).unwrap();

        assert_eq!(
            omp_paths_from(
                Some(&home),
                Some("~/custom/agent.db"),
                Some("work"),
                None,
                None,
                None,
                None,
                true,
            ),
            vec![home.join("custom/agent.db")]
        );
        assert_eq!(
            omp_paths_from(Some(&home), None, None, None, None, None, None, true),
            vec![home.join(".omp/agent/agent.db")]
        );
        assert_eq!(
            omp_paths_from(
                Some(&home),
                None,
                Some("work"),
                None,
                None,
                Some("/ignored/agent"),
                None,
                true,
            ),
            vec![home.join(".omp/profiles/work/agent/agent.db")]
        );
        // Explicit empty OMP_PROFILE selects default and ignores a PI_PROFILE-derived agent dir.
        let derived = home.join(".omp/profiles/work/agent");
        assert_eq!(
            omp_paths_from(
                Some(&home),
                None,
                Some(""),
                Some("work"),
                None,
                Some(&derived.to_string_lossy()),
                None,
                true,
            ),
            vec![home.join(".omp/agent/agent.db")]
        );
        assert_eq!(
            omp_paths_from(
                Some(&home),
                None,
                None,
                None,
                Some(".custom-omp"),
                Some("~/custom-agent"),
                None,
                true,
            ),
            vec![home.join("custom-agent/agent.db")]
        );
        // XDG applies only once $XDG_DATA_HOME/omp exists, flattening agent/.
        assert_eq!(
            omp_paths_from(
                Some(&home),
                None,
                None,
                None,
                None,
                None,
                Some(&xdg.to_string_lossy()),
                true,
            ),
            vec![home.join(".omp/agent/agent.db")]
        );
        std::fs::create_dir_all(xdg.join("omp")).unwrap();
        assert_eq!(
            omp_paths_from(
                Some(&home),
                None,
                None,
                None,
                None,
                None,
                Some(&xdg.to_string_lossy()),
                true,
            ),
            vec![xdg.join("omp/agent.db")]
        );
        // Named profiles require the profile-specific XDG dir to exist.
        assert_eq!(
            omp_paths_from(
                Some(&home),
                None,
                Some("work"),
                None,
                None,
                None,
                Some(&xdg.to_string_lossy()),
                true,
            ),
            vec![home.join(".omp/profiles/work/agent/agent.db")]
        );
        std::fs::create_dir_all(xdg.join("omp/profiles/work")).unwrap();
        assert_eq!(
            omp_paths_from(
                Some(&home),
                None,
                Some("work"),
                None,
                None,
                None,
                Some(&xdg.to_string_lossy()),
                true,
            ),
            vec![xdg.join("omp/profiles/work/agent.db")]
        );
    }

    #[test]
    fn sync_omp_creates_fresh_db_with_expected_schema_and_payload() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("agent/agent.db");
        let auth = sample_codex_auth("Alice@Example.com", "ws-1", "Plus", 1_780_000_000, "rt-a");

        sync_omp(&auth, std::slice::from_ref(&db_path)).unwrap();

        let conn = Connection::open(&db_path).unwrap();
        let (provider, ctype, data_raw, disabled_cause, identity_key): (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT provider, credential_type, data, disabled_cause, identity_key FROM auth_credentials",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(provider, "openai-codex");
        assert_eq!(ctype, "oauth");
        assert_eq!(disabled_cause, None);
        assert_eq!(
            identity_key.as_deref(),
            Some("email:alice@example.com|org:ws-1")
        );

        let data: Value = serde_json::from_str(&data_raw).unwrap();
        assert!(data.get("type").is_none());
        assert_eq!(data["access"], auth["tokens"]["access_token"]);
        assert_eq!(data["refresh"], "rt-a");
        assert_eq!(data["expires"], 1_780_000_000_000i64);
        assert_eq!(data["accountId"], "ws-1");
        assert_eq!(data["orgId"], "ws-1");
        assert_eq!(data["email"], "alice@example.com");
        assert_eq!(data["orgName"], "plus");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&db_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn sync_omp_replaces_active_codex_fires_revision_triggers_and_isolates_team_members() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE auth_credentials (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider TEXT NOT NULL,
                credential_type TEXT NOT NULL,
                data TEXT NOT NULL,
                disabled_cause TEXT DEFAULT NULL,
                identity_key TEXT DEFAULT NULL,
                created_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER)),
                updated_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER))
            );
            CREATE TABLE auth_change_revision (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                revision INTEGER NOT NULL
            );
            INSERT INTO auth_change_revision (id, revision) VALUES (1, 0);
            CREATE TRIGGER auth_change_revision_auth_credentials_insert
            AFTER INSERT ON auth_credentials
            BEGIN
                UPDATE auth_change_revision SET revision = revision + 1 WHERE id = 1;
            END;
            CREATE TRIGGER auth_change_revision_auth_credentials_update
            AFTER UPDATE ON auth_credentials
            BEGIN
                UPDATE auth_change_revision SET revision = revision + 1 WHERE id = 1;
            END;
            CREATE TRIGGER auth_change_revision_auth_credentials_delete
            AFTER DELETE ON auth_credentials
            BEGIN
                UPDATE auth_change_revision SET revision = revision + 1 WHERE id = 1;
            END;
            INSERT INTO auth_credentials (provider, credential_type, data, identity_key)
            VALUES ('anthropic', 'api_key', '{\"key\":\"sk-ant\"}', NULL);
            INSERT INTO auth_credentials (provider, credential_type, data, identity_key)
            VALUES ('openai-codex', 'api_key', '{\"key\":\"sk-old\"}', NULL);",
        )
        .unwrap();
        drop(conn);

        let revision = || -> i64 {
            Connection::open(&db_path)
                .unwrap()
                .query_row(
                    "SELECT revision FROM auth_change_revision WHERE id = 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };
        let rev0 = revision();

        // 1. First switch to Alice in Team workspace ws-team: inserts Alice, disables old api_key.
        let alice1 = sample_codex_auth(
            "alice@example.com",
            "ws-team",
            "team",
            1_780_000_000,
            "rt-a1",
        );
        sync_omp(&alice1, std::slice::from_ref(&db_path)).unwrap();
        let rev1 = revision();
        assert!(rev1 > rev0);

        // 2. Re-syncing identical credentials is a no-op (does not churn revision).
        sync_omp(&alice1, std::slice::from_ref(&db_path)).unwrap();
        assert_eq!(revision(), rev1);

        let conn = Connection::open(&db_path).unwrap();
        let alice_id: i64 = conn
            .query_row(
                "SELECT id FROM auth_credentials WHERE provider = 'openai-codex' AND disabled_cause IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        drop(conn);

        // 3. Refreshing Alice's token updates her row in-place, preserving row id.
        let alice2 = sample_codex_auth(
            "alice@example.com",
            "ws-team",
            "team",
            1_780_003_600,
            "rt-a2",
        );
        sync_omp(&alice2, std::slice::from_ref(&db_path)).unwrap();
        let conn = Connection::open(&db_path).unwrap();
        let (alice_id_after, alice_data): (i64, String) = conn
            .query_row(
                "SELECT id, data FROM auth_credentials WHERE provider = 'openai-codex' AND disabled_cause IS NULL",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(alice_id_after, alice_id);
        assert_eq!(
            serde_json::from_str::<Value>(&alice_data).unwrap()["refresh"],
            "rt-a2"
        );
        drop(conn);

        // 4. Switching to Bob in the SAME Team workspace inserts a distinct row and disables Alice.
        let bob = sample_codex_auth("bob@example.com", "ws-team", "team", 1_780_007_200, "rt-b1");
        sync_omp(&bob, std::slice::from_ref(&db_path)).unwrap();
        let conn = Connection::open(&db_path).unwrap();
        let (bob_id, bob_key): (i64, Option<String>) = conn
            .query_row(
                "SELECT id, identity_key FROM auth_credentials WHERE provider = 'openai-codex' AND disabled_cause IS NULL",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_ne!(bob_id, alice_id);
        assert_eq!(
            bob_key.as_deref(),
            Some("email:bob@example.com|org:ws-team")
        );
        let alice_disabled_cause: Option<String> = conn
            .query_row(
                "SELECT disabled_cause FROM auth_credentials WHERE id = ?1",
                params![alice_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(alice_disabled_cause.as_deref(), Some(OMP_REPLACED_CAUSE));
        // Unrelated provider rows remain untouched.
        let anthropic_active: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM auth_credentials WHERE provider = 'anthropic' AND disabled_cause IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(anthropic_active, 1);
        drop(conn);

        // 5. Switching back to Alice activates Alice, disables Bob, and purges Alice's old tombstone.
        sync_omp(&alice2, std::slice::from_ref(&db_path)).unwrap();
        let conn = Connection::open(&db_path).unwrap();
        let alice_disabled_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM auth_credentials WHERE provider = 'openai-codex' AND identity_key = 'email:alice@example.com|org:ws-team' AND disabled_cause IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(alice_disabled_count, 0);
        let active_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM auth_credentials WHERE provider = 'openai-codex' AND disabled_cause IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active_count, 1);
    }

    #[test]
    fn sync_omp_prefers_id_profile_email_over_top_level_access_email() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        let access = fake_jwt(json!({
            "exp": 1_780_000_000,
            "email": "access-top@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "ws-priority",
                "chatgpt_plan_type": "team"
            }
        }));
        let id = fake_jwt(json!({
            "exp": 1_780_000_000,
            "email": "id-top@example.com",
            "https://api.openai.com/profile": {
                "email": "Id-Profile@Example.com"
            }
        }));
        let auth = json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": access,
                "refresh_token": "rt-priority",
                "id_token": id,
                "account_id": "ws-priority"
            }
        });

        let extracted = auth::extract_codex_oauth_tokens(&auth).unwrap();
        assert_eq!(extracted.email.as_deref(), Some("id-profile@example.com"));

        sync_omp(&auth, std::slice::from_ref(&db_path)).unwrap();
        let conn = Connection::open(&db_path).unwrap();
        let (data_raw, identity_key): (String, Option<String>) = conn
            .query_row(
                "SELECT data, identity_key FROM auth_credentials WHERE provider = 'openai-codex'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let data: Value = serde_json::from_str(&data_raw).unwrap();
        assert_eq!(data["email"], "id-profile@example.com");
        assert_eq!(
            identity_key.as_deref(),
            Some("email:id-profile@example.com|org:ws-priority")
        );
    }
}
