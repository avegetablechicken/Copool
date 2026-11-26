//! Explicit desktop integrations. Never run by core tests or during account reads.
use crate::{
    persistence::write_json,
    pool_login::{codex, OAuthCredential},
    process,
    settings::Settings,
    utils::now_unix_seconds,
    AgentSyncOutcome, Pool, Preferences, SwitchExecution,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
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
const OPENCODE_OPENAI_BROWSER_METHOD: &str = "chatgpt-browser";
const OPENCODE_OPENAI_HEADLESS_METHOD: &str = "chatgpt-headless";
const OPENCODE_XAI_DEVICE_METHOD: &str = "device";

#[derive(Clone, Debug, PartialEq, Eq)]
enum OpencodeTarget {
    Json(Vec<PathBuf>),
    Sqlite(PathBuf),
}

/// A file an agent's logins are written to: a JSON document or a SQLite
/// database, which only OpenCode's storage may be either of.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    Json(PathBuf),
    Sqlite(PathBuf),
}

impl Target {
    /// A file the user named, its storage told by its extension.
    #[cfg(test)]
    fn named(path: &Path) -> Self {
        if is_sqlite_path(path) {
            Self::Sqlite(path.into())
        } else {
            Self::Json(path.into())
        }
    }

    fn path(&self) -> &Path {
        match self {
            Self::Json(path) | Self::Sqlite(path) => path,
        }
    }
}

/// A third-party agent a switch hands its login to. Each one's row says
/// which pools it takes, where its switch is saved, where its logins are
/// written and how; nothing else names an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncAgent {
    Opencode,
    Pi,
    Omp,
}

impl SyncAgent {
    pub const ALL: [Self; 3] = [Self::Opencode, Self::Pi, Self::Omp];

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

    pub fn id(self) -> &'static str {
        match self {
            Self::Opencode => "opencode",
            Self::Pi => "pi",
            Self::Omp => "omp",
        }
    }

    /// The key its switch is saved under, in settings.json as in early Rust
    /// preferences.json: OpenCode's the Swift app's.
    pub(crate) fn switch_key(self) -> &'static str {
        match self {
            Self::Opencode => "syncOpencodeOpenaiAuth",
            Self::Pi => "syncPiAuth",
            Self::Omp => "syncOmpAuth",
        }
    }

    /// The key its switch had before, still read.
    fn earlier_switch_key(self) -> Option<&'static str> {
        match self {
            Self::Opencode => None,
            Self::Pi => Some("syncPiOpenaiAuth"),
            Self::Omp => Some("syncOmpOpenaiAuth"),
        }
    }

    /// Its switch as settings.json keeps it, where OpenCode's is a field of
    /// the Swift app's settings.
    pub(crate) fn stored_switch(self, settings: &Settings) -> bool {
        if self == Self::Opencode {
            return settings.sync_opencode_openai_auth;
        }
        settings
            .extra
            .get(self.switch_key())
            .or_else(|| {
                self.earlier_switch_key()
                    .and_then(|key| settings.extra.get(key))
            })
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    pub(crate) fn store_switch(self, settings: &mut Settings, on: bool) {
        if self == Self::Opencode {
            settings.sync_opencode_openai_auth = on;
            return;
        }
        if let Some(key) = self.earlier_switch_key() {
            settings.extra.remove(key);
        }
        settings.extra.insert(self.switch_key().into(), on.into());
    }

    /// Whether a switch in the pool syncs to this agent: its sync is on, it
    /// has a provider for the pool, and the pool is among those chosen.
    fn enabled(self, p: &Preferences, pool: Pool) -> bool {
        let sync = p.agent_sync.get(&self).copied().unwrap_or_default();
        sync.enabled && self.provider_key(pool).is_some() && sync.pools.includes(pool)
    }

    /// The files its logins are written to: OpenCode's storage follows its
    /// installed version, which its executable is asked for.
    async fn locate(self) -> Vec<Target> {
        match self {
            Self::Opencode => match opencode_target().await {
                OpencodeTarget::Sqlite(path) => vec![Target::Sqlite(path)],
                OpencodeTarget::Json(paths) => paths.into_iter().map(Target::Json).collect(),
            },
            Self::Pi => pi_paths().into_iter().map(Target::Json).collect(),
            Self::Omp => omp_paths().into_iter().map(Target::Sqlite).collect(),
        }
    }

    /// Writes the pool's credential to each of `targets`, succeeding when
    /// any write does. It may block on a database, so it runs off the async
    /// threads ([`sync_to_agents`]).
    fn write(
        self,
        pool: Pool,
        credential: &OAuthCredential,
        targets: &[Target],
    ) -> Result<(), String> {
        let provider_key = self
            .provider_key(pool)
            .ok_or_else(|| format!("Unsupported sync target: {self:?} for {pool:?}"))?;
        let include_account_id = pool == Pool::Codex;
        let missing = match self {
            Self::Opencode => "Unable to locate OpenCode auth file path",
            Self::Pi => "Unable to locate pi auth file path",
            Self::Omp => "Unable to locate omp agent database path",
        };
        sync_targets(targets, missing, |target| match (self, target) {
            (Self::Opencode, Target::Sqlite(path)) => {
                sync_opencode_db_one(pool, credential, provider_key, path)
            }
            (Self::Opencode, Target::Json(path)) => sync_oauth_json_one(
                credential,
                path,
                provider_key,
                "Invalid OpenCode auth document",
                false,
                include_account_id,
            ),
            (Self::Pi, target) => sync_oauth_json_one(
                credential,
                target.path(),
                provider_key,
                "Invalid pi auth document",
                true,
                include_account_id,
            ),
            (Self::Omp, target) => sync_omp_one(credential, provider_key, target.path()),
        })
    }

    /// Writes to files the test names, their storage told by extension.
    #[cfg(test)]
    fn sync_credential(
        self,
        pool: Pool,
        credential: &OAuthCredential,
        paths: &[PathBuf],
    ) -> Result<(), String> {
        let targets: Vec<_> = paths.iter().map(|path| Target::named(path)).collect();
        self.write(pool, credential, &targets)
    }
}

fn non_empty_trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

fn now_unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_else(|_| now_unix_seconds().saturating_mul(1000))
}

fn is_sqlite_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "db" | "sqlite" | "sqlite3"
            )
        })
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

fn sync_targets<T, F>(targets: &[T], empty_error: &str, mut apply: F) -> Result<(), String>
where
    F: FnMut(&T) -> Result<(), String>,
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
fn sync_oauth_json_one(
    credential: &OAuthCredential,
    path: &Path,
    provider_key: &str,
    invalid_doc_error: &str,
    replace_entry: bool,
    include_account_id: bool,
) -> Result<(), String> {
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
}

fn opencode_credential_id(now_ms: i64) -> String {
    const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let value = ((now_ms.max(0) as u64) << 12).saturating_add(1);
    let mut id = format!("cred_{:012x}", value & 0xffff_ffff_ffff);
    for byte in uuid::Uuid::new_v4().as_bytes().iter().take(14) {
        id.push(BASE62[(*byte as usize) % BASE62.len()] as char);
    }
    id
}

fn opencode_default_method_id(pool: Pool, existing_method: Option<&str>) -> &'static str {
    match pool {
        Pool::Codex => match existing_method {
            Some(OPENCODE_OPENAI_HEADLESS_METHOD) => OPENCODE_OPENAI_HEADLESS_METHOD,
            _ => OPENCODE_OPENAI_BROWSER_METHOD,
        },
        Pool::Grok => OPENCODE_XAI_DEVICE_METHOD,
        Pool::Claude | Pool::Antigravity => "oauth",
    }
}

fn opencode_oauth_value(
    pool: Pool,
    credential: &OAuthCredential,
    existing: Option<&Value>,
) -> Value {
    let existing_method = existing
        .and_then(|v| v.get("methodID"))
        .and_then(Value::as_str);
    let method_id = opencode_default_method_id(pool, existing_method);
    let mut metadata = existing
        .and_then(|v| v.get("metadata"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if pool == Pool::Codex {
        if let Some(account_id) = &credential.account_id {
            metadata.insert("accountID".into(), account_id.clone().into());
        } else {
            metadata.remove("accountID");
        }
    }
    let mut value = serde_json::Map::new();
    value.insert("type".into(), json!("oauth"));
    value.insert("methodID".into(), method_id.into());
    value.insert("refresh".into(), credential.refresh.clone().into());
    value.insert("access".into(), credential.access.clone().into());
    value.insert("expires".into(), credential.expires_ms.max(0).into());
    if !metadata.is_empty() {
        value.insert("metadata".into(), Value::Object(metadata));
    }
    Value::Object(value)
}

/// An email as agents' rows are matched by: case does not tell accounts apart.
fn email_key(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

/// The email an OpenCode row is of, as [`email_key`] compares it.
fn opencode_row_email(pool: Pool, label: &str, value: &Value) -> Option<String> {
    let from_token = value
        .get("access")
        .and_then(Value::as_str)
        .and_then(|access| match pool {
            Pool::Codex => {
                let claims = codex::decode_jwt_payload(access).ok()?;
                codex::jwt_openai_email(&claims)
            }
            Pool::Grok => non_empty_trimmed(
                crate::pool_login::grok::claims(access)
                    .get("email")
                    .and_then(Value::as_str),
            )
            .filter(|email| email.contains('@')),
            Pool::Claude | Pool::Antigravity => None,
        });
    from_token
        .or_else(|| non_empty_trimmed(Some(label)).filter(|email| email.contains('@')))
        .map(|email| email_key(&email))
}

fn opencode_row_account_id(pool: Pool, value: &Value) -> Option<String> {
    non_empty_trimmed(
        value
            .get("metadata")
            .and_then(|m| m.get("accountID"))
            .and_then(Value::as_str),
    )
    .or_else(|| {
        let access = value.get("access").and_then(Value::as_str)?;
        match pool {
            Pool::Codex => {
                let claims = codex::decode_jwt_payload(access).ok()?;
                codex::jwt_chatgpt_account_id(&claims)
            }
            Pool::Grok => non_empty_trimmed(
                crate::pool_login::grok::claims(access)
                    .get("sub")
                    .and_then(Value::as_str),
            ),
            Pool::Claude | Pool::Antigravity => None,
        }
    })
}

fn opencode_db_matches_row(
    pool: Pool,
    label: &str,
    existing: &Value,
    incoming: &OAuthCredential,
) -> bool {
    if existing.get("type").and_then(Value::as_str) != Some("oauth") {
        return false;
    }
    if existing.get("refresh").and_then(Value::as_str) == Some(incoming.refresh.as_str())
        || existing.get("access").and_then(Value::as_str) == Some(incoming.access.as_str())
    {
        return true;
    }
    let existing_account = opencode_row_account_id(pool, existing);
    let existing_email = opencode_row_email(pool, label, existing);
    let incoming_email = incoming.email.as_deref().map(email_key);
    match pool {
        Pool::Codex => {
            if let (Some(incoming_id), Some(existing_id)) =
                (incoming.account_id.as_deref(), existing_account.as_deref())
            {
                if incoming_id != existing_id {
                    return false;
                }
                // Team workspace members share accountID; distinct emails stay separate rows.
                return match (incoming_email.as_deref(), existing_email.as_deref()) {
                    (Some(a), Some(b)) => a == b,
                    _ => true,
                };
            }
            incoming.account_id.is_none()
                && existing_account.is_none()
                && incoming_email.is_some()
                && incoming_email == existing_email
        }
        Pool::Grok => {
            if let (Some(incoming_id), Some(existing_id)) =
                (incoming.account_id.as_deref(), existing_account.as_deref())
            {
                return incoming_id == existing_id;
            }
            incoming_email.is_some() && incoming_email == existing_email
        }
        Pool::Claude | Pool::Antigravity => false,
    }
}

fn opencode_db_has_credential_table(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Ok(conn) = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return false;
    };
    let _ = conn.busy_timeout(Duration::from_millis(1000));
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'credential'",
        [],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

fn sync_opencode_db_one(
    pool: Pool,
    credential: &OAuthCredential,
    integration_id: &str,
    path: &Path,
) -> Result<(), String> {
    // OpenCode 2's initial credential migration uses CREATE TABLE without IF NOT EXISTS,
    // so never pre-create an unmigrated opencode.db file here.
    if !path.is_file() {
        return Err(
            "OpenCode database not found; launch OpenCode once to initialize opencode.db".into(),
        );
    }
    let mut conn = Connection::open(path).map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_millis(5000))
        .map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
        .map_err(|e| e.to_string())?;

    let table_exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'credential'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .is_some();
    if !table_exists {
        return Err(
            "OpenCode database is missing the credential table; launch OpenCode once first".into(),
        );
    }

    let columns: HashSet<String> = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(credential)")
            .map_err(|e| e.to_string())?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| e.to_string())?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(|e| e.to_string())?;
        names
    };
    if ![
        "id",
        "integration_id",
        "label",
        "value",
        "active",
        "time_created",
        "time_updated",
    ]
    .into_iter()
    .all(|col| columns.contains(col))
    {
        return Err("Invalid OpenCode credential table".into());
    }

    let now_ms = now_unix_millis();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;

    let existing_rows: Vec<(String, String, String, Option<i64>)> = {
        let mut stmt = tx
            .prepare(
                "SELECT id, label, value, active FROM credential WHERE integration_id = ?1 ORDER BY active DESC, time_created DESC, id DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![integration_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };

    let matched = existing_rows
        .iter()
        .find_map(|(id, label, raw_value, active)| {
            let parsed = serde_json::from_str::<Value>(raw_value).ok()?;
            opencode_db_matches_row(pool, label, &parsed, credential).then_some((
                id.as_str(),
                parsed,
                *active,
            ))
        });

    if let Some((target_id, existing_value, active)) = matched {
        let next_value = opencode_oauth_value(pool, credential, Some(&existing_value));
        let other_active = existing_rows
            .iter()
            .any(|(id, _, _, row_active)| id != target_id && row_active.unwrap_or(1) != 0);
        if active == Some(1) && !other_active && next_value == existing_value {
            return Ok(());
        }
        let next_json = serde_json::to_string(&next_value).map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE credential SET active = 0 WHERE integration_id = ?1 AND id != ?2 AND COALESCE(active, 1) != 0",
            params![integration_id, target_id],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE credential SET value = ?1, active = 1, time_updated = ?2 WHERE id = ?3",
            params![&next_json, now_ms, target_id],
        )
        .map_err(|e| e.to_string())?;
    } else {
        let next_value = opencode_oauth_value(pool, credential, None);
        let next_json = serde_json::to_string(&next_value).map_err(|e| e.to_string())?;
        let id = opencode_credential_id(now_ms);
        let label = credential.email.as_deref().unwrap_or("OAuth");
        tx.execute(
            "UPDATE credential SET active = 0 WHERE integration_id = ?1 AND COALESCE(active, 1) != 0",
            params![integration_id],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO credential (id, integration_id, label, value, active, time_created, time_updated) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)",
            params![&id, integration_id, label, &next_json, now_ms],
        )
        .map_err(|e| e.to_string())?;
    }

    tx.commit().map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) fn sync_opencode(
    credential: &OAuthCredential,
    targets: &[PathBuf],
) -> Result<(), String> {
    SyncAgent::Opencode.sync_credential(Pool::Codex, credential, targets)
}

#[cfg(test)]
pub(crate) fn sync_opencode_grok(
    credential: &OAuthCredential,
    targets: &[PathBuf],
) -> Result<(), String> {
    SyncAgent::Opencode.sync_credential(Pool::Grok, credential, targets)
}

#[cfg(test)]
pub(crate) fn sync_pi(credential: &OAuthCredential, targets: &[PathBuf]) -> Result<(), String> {
    SyncAgent::Pi.sync_credential(Pool::Codex, credential, targets)
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
/// `SqliteAuthCredentialStore`, with the email as [`email_key`] compares it.
fn format_omp_identity_key(
    org_scoped: bool,
    email: Option<&str>,
    account_id: Option<&str>,
    org_id: Option<&str>,
) -> Option<String> {
    let email = email.map(|email| format!("email:{}", email_key(email)));
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
            let claims = codex::decode_jwt_payload(access).ok()?;
            codex::jwt_openai_email(&claims)
        })
        .map(|email| email_key(&email))
}

/// The stored `accountId`, else the access token's account claim in the
/// order oh-my-pi reads it (`account_id`, `accountId`, `user_id`, `sub`,
/// then ChatGPT's account id).
fn omp_extract_row_account_id(data: &Value) -> Option<String> {
    non_empty_trimmed(data.get("accountId").and_then(Value::as_str)).or_else(|| {
        let access = data.get("access").and_then(Value::as_str)?;
        let claims = codex::decode_jwt_payload(access).ok()?;
        ["account_id", "accountId", "user_id", "sub"]
            .into_iter()
            .find_map(|key| non_empty_trimmed(claims.get(key).and_then(Value::as_str)))
            .or_else(|| codex::jwt_chatgpt_account_id(&claims))
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

/// A row's identity key with its email as [`email_key`] compares it; a key
/// oh-my-pi wrote itself may spell the email otherwise.
fn omp_comparable_key(key: &str) -> String {
    key.split('|')
        .map(|part| match part.strip_prefix("email:") {
            Some(email) => format!("email:{}", email_key(email)),
            None => part.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("|")
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
    let Some(existing_key) = existing_key.as_deref().map(omp_comparable_key) else {
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
            .map(|email| format!("email:{}", email_key(email))),
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

#[cfg(test)]
pub(crate) fn sync_omp(credential: &OAuthCredential, targets: &[PathBuf]) -> Result<(), String> {
    SyncAgent::Omp.sync_credential(Pool::Codex, credential, targets)
}
fn opencode_data_dir(home: Option<&Path>, xdg_data_home: Option<&str>) -> Option<PathBuf> {
    if let Some(xdg) = non_empty_trimmed(xdg_data_home) {
        return Some(expand_tilde(&xdg, home).join("opencode"));
    }
    home.map(|h| h.join(".local/share/opencode"))
}

fn opencode_db_path_from(
    home: Option<&Path>,
    opencode_db: Option<&str>,
    xdg_data_home: Option<&str>,
) -> Option<PathBuf> {
    let data_dir = opencode_data_dir(home, xdg_data_home);
    if let Some(raw) = non_empty_trimmed(opencode_db).filter(|v| v != ":memory:") {
        let path = expand_tilde(&raw, home);
        return if path.is_absolute() {
            Some(path)
        } else {
            data_dir.map(|dir| dir.join(path))
        };
    }
    data_dir.map(|dir| dir.join("opencode.db"))
}

fn parse_opencode_major_version(output: &str) -> Option<u32> {
    for token in output.split(|c: char| c.is_whitespace() || matches!(c, '/' | ',' | '(' | ')')) {
        let trimmed = token.trim().trim_start_matches(['v', 'V']);
        let Some((major, rest)) = trimmed.split_once('.') else {
            continue;
        };
        if major.is_empty() || !major.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let minor = rest.split(['.', '-', '+']).next().unwrap_or("");
        if minor.is_empty() || !minor.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if let Ok(parsed) = major.parse::<u32>() {
            return Some(parsed);
        }
    }
    None
}

fn opencode_executable() -> Option<PathBuf> {
    executable("opencode").or_else(|| {
        let name = if cfg!(windows) {
            "opencode.exe"
        } else {
            "opencode"
        };
        let candidate = dirs::home_dir()?.join(".opencode/bin").join(name);
        candidate.is_file().then_some(candidate)
    })
}

async fn detect_opencode_major_version() -> Option<u32> {
    let bin = opencode_executable()?;
    let output = checked(&bin.to_string_lossy(), &["--version"], 3)
        .await
        .ok()?;
    parse_opencode_major_version(&output)
}

fn resolve_opencode_target(
    installed_major: Option<u32>,
    home: Option<&Path>,
    opencode_auth_path: Option<&str>,
    opencode_db: Option<&str>,
    xdg_data_home: Option<&str>,
    json_paths: Vec<PathBuf>,
) -> OpencodeTarget {
    if let Some(raw) = non_empty_trimmed(opencode_auth_path) {
        let path = expand_tilde(&raw, home);
        return if is_sqlite_path(&path) {
            OpencodeTarget::Sqlite(path)
        } else {
            OpencodeTarget::Json(vec![path])
        };
    }
    if non_empty_trimmed(opencode_db)
        .filter(|v| v != ":memory:")
        .is_some()
    {
        if let Some(db_path) = opencode_db_path_from(home, opencode_db, xdg_data_home) {
            return OpencodeTarget::Sqlite(db_path);
        }
    }
    let db_path = opencode_db_path_from(home, None, xdg_data_home);
    match installed_major {
        Some(major) if major >= 2 => {
            if let Some(db_path) = db_path {
                if db_path.exists() {
                    return OpencodeTarget::Sqlite(db_path);
                }
                // Before OpenCode 2's first launch, write its legacy import source
                // (`<data>/auth.json`) rather than pre-creating an unmigrated DB.
                if let Some(data_dir) = db_path.parent() {
                    return OpencodeTarget::Json(vec![data_dir.join("auth.json")]);
                }
            }
            OpencodeTarget::Json(json_paths)
        }
        Some(_) => OpencodeTarget::Json(json_paths),
        None => {
            if let Some(db_path) = db_path.filter(|p| opencode_db_has_credential_table(p)) {
                OpencodeTarget::Sqlite(db_path)
            } else {
                OpencodeTarget::Json(json_paths)
            }
        }
    }
}

async fn opencode_target() -> OpencodeTarget {
    let home = dirs::home_dir();
    let auth_path = non_empty_env("OPENCODE_AUTH_PATH");
    let db_env = non_empty_env("OPENCODE_DB");
    let xdg_data_home = non_empty_env("XDG_DATA_HOME");
    let installed_major =
        if auth_path.is_some() || db_env.as_deref().is_some_and(|v| v != ":memory:") {
            None
        } else {
            detect_opencode_major_version().await
        };
    resolve_opencode_target(
        installed_major,
        home.as_deref(),
        auth_path.as_deref(),
        db_env.as_deref(),
        xdg_data_home.as_deref(),
        opencode_paths(),
    )
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
/// A third-party agent as the settings show it: the pools it can receive
/// and where their logins are written.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncAgentInfo {
    pub id: SyncAgent,
    pub pools: Vec<Pool>,
    /// The file written, `~`-abbreviated: OpenCode's storage follows its
    /// installed version. None when it cannot be located.
    pub target: Option<String>,
    /// Whether that file already exists.
    pub found: bool,
}

fn abbreviated(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => Path::new("~").join(rest).to_string_lossy().into_owned(),
        None => path.to_string_lossy().into_owned(),
    }
}

pub async fn sync_agents() -> Vec<SyncAgentInfo> {
    let home = dirs::home_dir();
    let mut agents = Vec::new();
    for agent in SyncAgent::ALL {
        let targets = agent.locate().await;
        let target = targets
            .iter()
            .map(Target::path)
            .find(|path| path.exists())
            .or_else(|| targets.first().map(Target::path));
        agents.push(SyncAgentInfo {
            id: agent,
            pools: Pool::ALL
                .into_iter()
                .filter(|&pool| agent.provider_key(pool).is_some())
                .collect(),
            target: target.map(|path| abbreviated(path, home.as_deref())),
            found: target.is_some_and(Path::exists),
        });
    }
    agents
}

/// Hands the login switched to in `pool` to each agent whose sync takes it,
/// in [`SyncAgent::ALL`]'s order, at the files `locate` finds for it: what
/// each reports, an error for a login that grants no credential. Each write
/// runs off the async threads, as a database's may block.
async fn sync_to_agents<F, Located>(
    pool: Pool,
    p: &Preferences,
    credential: Result<OAuthCredential, String>,
    locate: F,
) -> Vec<AgentSyncOutcome>
where
    F: Fn(SyncAgent) -> Located,
    Located: std::future::Future<Output = Vec<Target>>,
{
    let mut outcomes = vec![];
    for agent in SyncAgent::ALL
        .into_iter()
        .filter(|agent| agent.enabled(p, pool))
    {
        let outcome = match &credential {
            Err(error) => Err(error.clone()),
            Ok(credential) => {
                let targets = locate(agent).await;
                let credential = credential.clone();
                tokio::task::spawn_blocking(move || agent.write(pool, &credential, &targets))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|written| written)
            }
        };
        outcomes.push(AgentSyncOutcome {
            agent,
            error: outcome.err(),
        });
    }
    outcomes
}

/// Side effects of a switch, reported as Swift's SwitchAccountExecutionResult:
/// the switch itself has succeeded whatever fails here. `credential` is
/// what the login switched to grants, for the agents to sync; none for a
/// provider's card, and an error, which each of them reports, for a login
/// that grants none.
pub(crate) async fn after_switch(
    pool: Pool,
    p: &Preferences,
    credential: Option<Result<OAuthCredential, String>>,
) -> Result<SwitchExecution, String> {
    let mut result = SwitchExecution::default();
    if let Some(credential) = credential {
        result.agent_sync = sync_to_agents(pool, p, credential, SyncAgent::locate).await;
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

        let credential = OAuthCredential {
            access: "openai-access".into(),
            refresh: "openai-refresh".into(),
            expires_ms: 1_800_000_456_000,
            account_id: Some("acct-42".into()),
            org_id: Some("acct-42".into()),
            ..OAuthCredential::default()
        };
        sync_opencode(&credential, std::slice::from_ref(&path)).unwrap();

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

        let credential = OAuthCredential {
            access: "grok-access-1".into(),
            refresh: "grok-refresh-1".into(),
            expires_ms: grok_expires_ms(),
            ..OAuthCredential::default()
        };
        sync_opencode_grok(&credential, std::slice::from_ref(&path)).unwrap();

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

    /// Each of `agents` with its sync on, every pool chosen; the others off.
    fn syncing(agents: &[SyncAgent]) -> std::collections::BTreeMap<SyncAgent, crate::AgentSync> {
        SyncAgent::ALL
            .into_iter()
            .map(|agent| {
                let sync = crate::AgentSync {
                    enabled: agents.contains(&agent),
                    ..Default::default()
                };
                (agent, sync)
            })
            .collect()
    }

    /// Each agent's own file in `dir`, as `locate` would find it at home.
    fn located(dir: &Path, agent: SyncAgent) -> Vec<Target> {
        match agent {
            SyncAgent::Opencode => vec![Target::Json(dir.join("opencode/auth.json"))],
            SyncAgent::Pi => vec![Target::Json(dir.join("pi/auth.json"))],
            SyncAgent::Omp => vec![Target::Sqlite(dir.join("omp/agent.db"))],
        }
    }

    /// A switch writes an agent exactly when its sync is on, it takes the
    /// pool, and the pool is chosen for it; the writes land in the files
    /// `locate` names, never at home.
    #[tokio::test]
    async fn a_switch_syncs_each_agent_that_takes_its_pool_where_it_is_on_and_chosen() {
        let takes = |agent, pool| {
            matches!(
                (agent, pool),
                (SyncAgent::Opencode, Pool::Codex | Pool::Grok)
                    | (SyncAgent::Pi, Pool::Codex | Pool::Grok | Pool::Claude)
                    | (SyncAgent::Omp, _)
            )
        };
        let credential = |pool| match pool {
            Pool::Codex => codex_credential(
                "alice@example.com",
                "acct-1",
                "plus",
                1_780_000_000,
                "refresh-1",
            ),
            Pool::Claude => claude_credential("alice", "org-1", "rt-1"),
            Pool::Grok => grok_credential("alice", "grok-refresh"),
            Pool::Antigravity => OAuthCredential {
                access: "a".into(),
                refresh: "r".into(),
                expires_ms: 1_780_000_000_000,
                email: Some("alice@gmail.com".into()),
                ..OAuthCredential::default()
            },
        };
        for pool in Pool::ALL {
            for agent in SyncAgent::ALL {
                for (on, chosen) in [(true, true), (true, false), (false, true)] {
                    let dir = tempfile::tempdir().unwrap();
                    let on_agents: &[SyncAgent] = if on { &[agent] } else { &[] };
                    let mut p = Preferences {
                        agent_sync: syncing(on_agents),
                        ..Default::default()
                    };
                    let pools = &mut p.agent_sync.get_mut(&agent).unwrap().pools;
                    match pool {
                        Pool::Codex => pools.codex = chosen,
                        Pool::Claude => pools.claude = chosen,
                        Pool::Antigravity => pools.antigravity = chosen,
                        Pool::Grok => pools.grok = chosen,
                    }
                    let root = dir.path().to_path_buf();
                    let outcomes = sync_to_agents(pool, &p, Ok(credential(pool)), |agent| {
                        let targets = located(&root, agent);
                        async move { targets }
                    })
                    .await;
                    let expected = on && chosen && takes(agent, pool);
                    let case = format!("{agent:?} in {pool:?}, on {on}, chosen {chosen}");
                    let written = if expected {
                        vec![AgentSyncOutcome { agent, error: None }]
                    } else {
                        vec![]
                    };
                    assert_eq!(outcomes, written, "{case}");
                    for other in SyncAgent::ALL {
                        let path = located(dir.path(), other)[0].path().to_path_buf();
                        assert_eq!(
                            path.exists(),
                            expected && other == agent,
                            "{case}: {other:?}"
                        );
                    }
                }
            }
        }
    }

    /// Every agent on reports in the table's order, each written or why
    /// not, and a login granting no credential is each one's error.
    #[tokio::test]
    async fn a_switch_reports_each_agent_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let p = Preferences {
            agent_sync: syncing(&SyncAgent::ALL),
            ..Default::default()
        };
        // OpenCode's file is a directory: its write fails, the others' not.
        std::fs::create_dir_all(dir.path().join("opencode/auth.json")).unwrap();
        let root = dir.path().to_path_buf();
        let locate = |agent| {
            let targets = located(&root, agent);
            async move { targets }
        };
        let codex = codex_credential(
            "alice@example.com",
            "acct-1",
            "plus",
            1_780_000_000,
            "refresh-1",
        );
        let outcomes = sync_to_agents(Pool::Codex, &p, Ok(codex), locate).await;
        assert_eq!(
            outcomes.iter().map(|o| o.agent).collect::<Vec<_>>(),
            SyncAgent::ALL
        );
        assert!(outcomes[0].error.is_some());
        assert_eq!(outcomes[1].error, None);
        assert_eq!(outcomes[2].error, None);
        let none = "Missing refresh token".to_string();
        let outcomes = sync_to_agents(Pool::Codex, &p, Err(none.clone()), locate).await;
        assert_eq!(
            outcomes,
            SyncAgent::ALL
                .map(|agent| AgentSyncOutcome {
                    agent,
                    error: Some(none.clone()),
                })
                .to_vec()
        );
        // No agent's file is found: each says where it looked for none.
        let outcomes = sync_to_agents(
            Pool::Codex,
            &p,
            Ok(codex_credential(
                "alice@example.com",
                "acct-1",
                "plus",
                1_780_000_000,
                "refresh-1",
            )),
            |_| async { vec![] },
        )
        .await;
        assert_eq!(
            outcomes[0].error.as_deref(),
            Some("Unable to locate OpenCode auth file path")
        );
    }

    #[test]
    fn abbreviated_paths_start_from_home() {
        let home = Path::new("/Users/someone");
        assert_eq!(
            abbreviated(&home.join(".pi/agent/auth.json"), Some(home)),
            Path::new("~").join(".pi/agent/auth.json").to_string_lossy()
        );
        assert_eq!(abbreviated(Path::new("/tmp/a.db"), Some(home)), "/tmp/a.db");
    }

    #[tokio::test]
    async fn after_switch_outside_codex_skips_editors_codex_launch_and_unsupported_agents() {
        // Agents stay off for Grok: enabled, they would write the real home.
        let grok_preferences = Preferences {
            agent_sync: syncing(&[]),
            restart_editors_on_switch: true,
            restart_editor_targets: vec!["not-installed-editor".into()],
            launch_codex_after_switch: true,
            ..Default::default()
        };
        let execution = after_switch(
            Pool::Grok,
            &grok_preferences,
            Some(Ok(grok_credential("alice", "grok-refresh"))),
        )
        .await
        .unwrap();
        assert_eq!(execution, SwitchExecution::default());
        // OpenCode and pi do not take an Antigravity login, and OpenCode does
        // not take Claude; keep supported agents off so tests never touch home.
        let antigravity_preferences = Preferences {
            agent_sync: syncing(&[SyncAgent::Opencode, SyncAgent::Pi]),
            ..grok_preferences.clone()
        };
        let credential = OAuthCredential {
            access: "a".into(),
            refresh: "r".into(),
            expires_ms: 1_780_000_000_000,
            email: Some("alice@gmail.com".into()),
            ..OAuthCredential::default()
        };
        let execution = after_switch(
            Pool::Antigravity,
            &antigravity_preferences,
            Some(Ok(credential)),
        )
        .await
        .unwrap();
        assert_eq!(execution, SwitchExecution::default());

        let claude_preferences = Preferences {
            agent_sync: syncing(&[SyncAgent::Opencode]),
            ..grok_preferences
        };
        let execution = after_switch(
            Pool::Claude,
            &claude_preferences,
            Some(Ok(claude_credential("alice", "org-1", "rt-1"))),
        )
        .await
        .unwrap();
        assert_eq!(execution, SwitchExecution::default());
    }

    #[tokio::test]
    async fn after_switch_reports_a_login_granting_no_credential_to_each_agent() {
        // A login granting no Credential writes nothing, so no home is
        // touched; Grok's switch neither restarts editors nor launches Codex.
        let preferences = Preferences {
            agent_sync: syncing(&SyncAgent::ALL),
            restart_editors_on_switch: false,
            launch_codex_after_switch: false,
            ..Default::default()
        };
        let error = "Missing refresh token".to_string();
        let execution = after_switch(Pool::Grok, &preferences, Some(Err(error.clone())))
            .await
            .unwrap();
        assert_eq!(
            execution,
            SwitchExecution {
                agent_sync: SyncAgent::ALL
                    .map(|agent| AgentSyncOutcome {
                        agent,
                        error: Some(error.clone()),
                    })
                    .to_vec(),
                ..SwitchExecution::default()
            }
        );
        // A provider's card hands over none: no agent is synced.
        let execution = after_switch(Pool::Grok, &preferences, None).await.unwrap();
        assert_eq!(execution, SwitchExecution::default());
    }

    /// The Credential of a Claude login: its account and organization
    /// named after `name` and `org`, its email capitalized as Claude Code's
    /// account may spell it.
    fn claude_credential(name: &str, org: &str, refresh: &str) -> OAuthCredential {
        OAuthCredential {
            access: format!("sk-ant-oat-{name}"),
            refresh: refresh.into(),
            expires_ms: 1_780_000_000_000,
            account_id: Some(format!("acct-{name}")),
            org_id: Some(org.into()),
            email: Some(format!("{name}@Example.com")),
            org_name: Some(format!("{name}'s Organization")),
            scope: Some("user:inference".into()),
        }
    }

    fn grok_expires_ms() -> i64 {
        chrono::DateTime::parse_from_rfc3339("2026-11-26T12:00:00Z")
            .unwrap()
            .timestamp_millis()
    }

    /// The Credential of a Grok sign-in of `user`, its access token no JWT.
    fn grok_credential(user: &str, refresh: &str) -> OAuthCredential {
        OAuthCredential {
            access: format!("grok-access-{user}"),
            refresh: refresh.into(),
            expires_ms: grok_expires_ms(),
            account_id: Some(format!("uid-{user}")),
            email: Some(format!("{user}@x.example")),
            ..OAuthCredential::default()
        }
    }

    #[test]
    fn sync_pi_writes_anthropic_and_xai_entries_beside_openai_codex() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let codex = codex_credential("a@example.com", "acct-1", "plus", 1_780_000_000, "rt-1");
        sync_pi(&codex, std::slice::from_ref(&path)).unwrap();
        SyncAgent::Pi
            .sync_credential(
                Pool::Claude,
                &claude_credential("alice", "org-1", "claude-rt"),
                std::slice::from_ref(&path),
            )
            .unwrap();
        SyncAgent::Pi
            .sync_credential(
                Pool::Grok,
                &grok_credential("bob", "grok-rt"),
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
        let sync = |credential: &OAuthCredential| {
            SyncAgent::Omp
                .sync_credential(Pool::Claude, credential, std::slice::from_ref(&db_path))
                .unwrap()
        };
        sync(&claude_credential("alice", "org-team", "rt-1"));
        let rows = omp_rows(&db_path, "anthropic");
        assert_eq!(rows.len(), 1);
        let (alice_id, data, key, cause) = &rows[0];
        assert_eq!(key.as_deref(), Some("email:alice@example.com|org:org-team"));
        assert_eq!(cause, &None);
        assert_eq!(data["access"], "sk-ant-oat-alice");
        assert_eq!(data["refresh"], "rt-1");
        assert_eq!(data["accountId"], "acct-alice");
        assert_eq!(data["orgId"], "org-team");
        // Written as the login spells it; keyed as rows are compared.
        assert_eq!(data["email"], "alice@Example.com");
        assert_eq!(data["orgName"], "alice's Organization");
        assert_eq!(data["scope"], "user:inference");

        // A renewal of the same login updates its row in place.
        sync(&claude_credential("alice", "org-team", "rt-2"));
        let rows = omp_rows(&db_path, "anthropic");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, *alice_id);
        assert_eq!(rows[0].1["refresh"], "rt-2");

        // The same email in another organization is another subscription.
        sync(&claude_credential("alice", "org-personal", "rt-3"));
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
        let sync = |credential: &OAuthCredential| {
            SyncAgent::Omp
                .sync_credential(Pool::Grok, credential, std::slice::from_ref(&db_path))
                .unwrap()
        };
        sync(&grok_credential("alice", "rt-a1"));
        let rows = omp_rows(&db_path, "xai-oauth");
        assert_eq!(rows.len(), 1);
        let (alice_id, data, key, _) = &rows[0];
        assert_eq!(key.as_deref(), Some("account:uid-alice"));
        assert_eq!(data["access"], "grok-access-alice");
        assert_eq!(data["accountId"], "uid-alice");
        assert_eq!(data["email"], "alice@x.example");
        assert!(data.get("orgId").is_none());

        sync(&grok_credential("alice", "rt-a2"));
        let rows = omp_rows(&db_path, "xai-oauth");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, *alice_id);
        assert_eq!(rows[0].1["refresh"], "rt-a2");

        // A teammate is another account: inserted, Alice's row disabled.
        sync(&grok_credential("bob", "rt-b1"));
        let rows = omp_rows(&db_path, "xai-oauth");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].3.as_deref(), Some(OMP_REPLACED_CAUSE));
        assert_eq!(rows[1].2.as_deref(), Some("account:uid-bob"));
        assert_eq!(rows[1].3, None);
    }

    #[test]
    fn sync_omp_keys_antigravity_cli_by_account_else_email() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        let alice = OAuthCredential {
            access: "agy-access-alice".into(),
            refresh: "agy-rt-a1".into(),
            expires_ms: grok_expires_ms(),
            account_id: Some("google-sub-alice".into()),
            email: Some("Alice@Gmail.com".into()),
            ..OAuthCredential::default()
        };
        SyncAgent::Omp
            .sync_credential(Pool::Antigravity, &alice, std::slice::from_ref(&db_path))
            .unwrap();
        let rows = omp_rows(&db_path, "antigravity-cli");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].2.as_deref(), Some("account:google-sub-alice"));
        assert_eq!(rows[0].1["access"], "agy-access-alice");
        assert_eq!(rows[0].1["refresh"], "agy-rt-a1");
        assert_eq!(rows[0].1["email"], "Alice@Gmail.com");
        assert_eq!(rows[0].1["accountId"], "google-sub-alice");

        // Email-only grant (as `agy acp --login` often exports) keys by email.
        let bob = OAuthCredential {
            access: "agy-access-bob".into(),
            refresh: "agy-rt-b1".into(),
            expires_ms: grok_expires_ms() + 3_600_000,
            email: Some("Bob@Gmail.com".into()),
            ..OAuthCredential::default()
        };
        SyncAgent::Omp
            .sync_credential(Pool::Antigravity, &bob, std::slice::from_ref(&db_path))
            .unwrap();
        let rows = omp_rows(&db_path, "antigravity-cli");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].3.as_deref(), Some(OMP_REPLACED_CAUSE));
        assert_eq!(rows[1].2.as_deref(), Some("email:bob@gmail.com"));
        assert_eq!(rows[1].3, None);
    }

    /// The Credential of a ChatGPT login of `email` in workspace
    /// `account_id`, its access token carrying the claims agents' rows are
    /// told apart by.
    fn codex_credential(
        email: &str,
        account_id: &str,
        plan_type: &str,
        exp_sec: i64,
        refresh: &str,
    ) -> OAuthCredential {
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
        OAuthCredential {
            access,
            refresh: refresh.into(),
            expires_ms: exp_sec * 1000,
            account_id: Some(account_id.into()),
            org_id: Some(account_id.into()),
            email: Some(email.into()),
            org_name: Some(plan_type.to_ascii_lowercase()),
            scope: None,
        }
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

        let auth = codex_credential("Alice@Example.com", "acct-1", "Pro", 1_780_000_000, "rt-1");
        sync_opencode(&auth, std::slice::from_ref(&path)).unwrap();

        let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["anthropic"]["key"], "sk-ant");
        assert_eq!(written["openai"]["type"], "oauth");
        assert_eq!(written["openai"]["extra"], "keep");
        assert_eq!(written["openai"]["access"], auth.access);
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

        let auth = codex_credential("Alice@Example.com", "acct-1", "Pro", 1_780_000_000, "rt-1");
        sync_pi(&auth, std::slice::from_ref(&path)).unwrap();

        let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["anthropic"]["type"], "api_key");
        assert_eq!(written["anthropic"]["key"], "sk-ant");
        assert_eq!(written["openai-codex"]["type"], "oauth");
        assert_eq!(written["openai-codex"]["access"], auth.access);
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
    fn sync_pi_rejects_non_object_document_and_empty_targets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, "[]").unwrap();
        let auth = codex_credential("a@example.com", "acct-1", "plus", 1_780_000_000, "rt-1");
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
        let auth = codex_credential("Alice@Example.com", "ws-1", "Plus", 1_780_000_000, "rt-a");

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
        assert_eq!(data["access"], auth.access);
        assert_eq!(data["refresh"], "rt-a");
        assert_eq!(data["expires"], 1_780_000_000_000i64);
        assert_eq!(data["accountId"], "ws-1");
        assert_eq!(data["orgId"], "ws-1");
        assert_eq!(data["email"], "Alice@Example.com");
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
        let alice1 = codex_credential(
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
        let alice2 = codex_credential(
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
        let bob = codex_credential("bob@example.com", "ws-team", "team", 1_780_007_200, "rt-b1");
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

    fn init_opencode_v2_db(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE credential (
                id TEXT PRIMARY KEY,
                integration_id TEXT,
                label TEXT NOT NULL,
                value TEXT NOT NULL,
                connector_id TEXT,
                method_id TEXT,
                active INTEGER,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL
            );",
        )
        .unwrap();
    }

    fn opencode_db_rows(
        db_path: &Path,
        integration_id: &str,
    ) -> Vec<(String, String, Value, Option<i64>, i64)> {
        let conn = Connection::open(db_path).unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, label, value, active, time_updated FROM credential WHERE integration_id = ?1 ORDER BY time_created ASC, id ASC",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![integration_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    serde_json::from_str::<Value>(&r.get::<_, String>(2)?).unwrap(),
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    }

    #[test]
    fn parse_opencode_major_version_handles_cli_formats() {
        assert_eq!(parse_opencode_major_version("1.19.6\n"), Some(1));
        assert_eq!(parse_opencode_major_version("2.4.1\n"), Some(2));
        assert_eq!(parse_opencode_major_version("v2.0.0-beta.1"), Some(2));
        assert_eq!(
            parse_opencode_major_version("opencode 2.3.1 (abc)"),
            Some(2)
        );
        assert_eq!(parse_opencode_major_version("opencode/1.2.3"), Some(1));
        assert_eq!(parse_opencode_major_version(""), None);
        assert_eq!(parse_opencode_major_version("unknown"), None);
    }

    #[test]
    fn resolve_opencode_target_selects_storage_by_version_env_and_existing_db() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let xdg = dir.path().join("xdg");
        let fallback_json = vec![home.join(".config/opencode/auth.json")];

        // Explicit OPENCODE_AUTH_PATH picks SQLite or JSON by extension.
        assert_eq!(
            resolve_opencode_target(
                Some(1),
                Some(&home),
                Some("~/custom/opencode.db"),
                None,
                None,
                fallback_json.clone(),
            ),
            OpencodeTarget::Sqlite(home.join("custom/opencode.db"))
        );
        assert_eq!(
            resolve_opencode_target(
                Some(2),
                Some(&home),
                Some("~/custom/auth.json"),
                None,
                None,
                fallback_json.clone(),
            ),
            OpencodeTarget::Json(vec![home.join("custom/auth.json")])
        );

        // Explicit OPENCODE_DB resolves relative to data dir and ignores `:memory:`.
        assert_eq!(
            resolve_opencode_target(
                Some(1),
                Some(&home),
                None,
                Some("opencode-beta.db"),
                Some(&xdg.to_string_lossy()),
                fallback_json.clone(),
            ),
            OpencodeTarget::Sqlite(xdg.join("opencode/opencode-beta.db"))
        );
        assert_eq!(
            resolve_opencode_target(
                Some(1),
                Some(&home),
                None,
                Some(":memory:"),
                None,
                fallback_json.clone(),
            ),
            OpencodeTarget::Json(fallback_json.clone())
        );

        // OpenCode 1.x uses auth.json.
        assert_eq!(
            resolve_opencode_target(
                Some(1),
                Some(&home),
                None,
                None,
                None,
                fallback_json.clone(),
            ),
            OpencodeTarget::Json(fallback_json.clone())
        );

        // OpenCode 2.x before first launch writes `<data>/auth.json` for migration import.
        let default_db = home.join(".local/share/opencode/opencode.db");
        assert_eq!(
            resolve_opencode_target(
                Some(2),
                Some(&home),
                None,
                None,
                None,
                fallback_json.clone(),
            ),
            OpencodeTarget::Json(vec![home.join(".local/share/opencode/auth.json")])
        );

        // Once opencode.db exists, OpenCode 2.x targets SQLite.
        init_opencode_v2_db(&default_db);
        assert_eq!(
            resolve_opencode_target(
                Some(2),
                Some(&home),
                None,
                None,
                None,
                fallback_json.clone(),
            ),
            OpencodeTarget::Sqlite(default_db.clone())
        );

        // Unknown version falls back to SQLite when an initialized opencode.db exists.
        assert_eq!(
            resolve_opencode_target(None, Some(&home), None, None, None, fallback_json.clone()),
            OpencodeTarget::Sqlite(default_db)
        );
    }

    #[test]
    fn sync_opencode_db_manages_openai_credentials_preserves_api_key_and_isolates_team_members() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("opencode.db");
        init_opencode_v2_db(&db_path);
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO credential (id, integration_id, label, value, active, time_created, time_updated) VALUES (?1, ?2, ?3, ?4, 1, 1000, 1000)",
            params!["cred_openai_key", "openai", "API key", r#"{"type":"key","key":"sk-old"}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO credential (id, integration_id, label, value, active, time_created, time_updated) VALUES (?1, ?2, ?3, ?4, 1, 1000, 1000)",
            params!["cred_anthropic_key", "anthropic", "API key", r#"{"type":"key","key":"sk-ant"}"#],
        )
        .unwrap();
        drop(conn);

        // 1. Switch to Alice in Team workspace ws-team.
        let alice1 = codex_credential(
            "Alice@Example.com",
            "ws-team",
            "team",
            1_780_000_000,
            "rt-a1",
        );
        sync_opencode(&alice1, std::slice::from_ref(&db_path)).unwrap();

        let rows = opencode_db_rows(&db_path, "openai");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "cred_openai_key");
        assert_eq!(rows[0].3, Some(0));
        let (alice_id, alice_label, alice_val, alice_active, alice_updated) = rows[1].clone();
        assert!(alice_id.starts_with("cred_"));
        assert_eq!(alice_label, "Alice@Example.com");
        assert_eq!(alice_active, Some(1));
        assert_eq!(alice_val["type"], "oauth");
        assert_eq!(alice_val["methodID"], "chatgpt-browser");
        assert_eq!(alice_val["refresh"], "rt-a1");
        assert_eq!(alice_val["expires"], 1_780_000_000_000_i64);
        assert_eq!(alice_val["metadata"]["accountID"], "ws-team");

        // Unrelated integrations remain active.
        let anthropic_rows = opencode_db_rows(&db_path, "anthropic");
        assert_eq!(anthropic_rows[0].3, Some(1));

        // 2. Re-syncing identical credentials is a no-op.
        sync_opencode(&alice1, std::slice::from_ref(&db_path)).unwrap();
        let rows_after_noop = opencode_db_rows(&db_path, "openai");
        assert_eq!(rows_after_noop.len(), 2);
        assert_eq!(rows_after_noop[1].4, alice_updated);

        // 3. Refreshing Alice updates her row in place and preserves headless methodID + custom metadata.
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE credential SET label = 'Alice Work', value = ?1 WHERE id = ?2",
            params![
                json!({
                    "type": "oauth",
                    "methodID": "chatgpt-headless",
                    "refresh": "rt-a1",
                    "access": alice1.access,
                    "expires": 1_780_000_000_000_i64,
                    "metadata": { "accountID": "ws-team", "custom": "keep" }
                })
                .to_string(),
                &alice_id
            ],
        )
        .unwrap();
        drop(conn);

        let alice2 = codex_credential(
            "Alice@Example.com",
            "ws-team",
            "team",
            1_780_003_600,
            "rt-a2",
        );
        sync_opencode(&alice2, std::slice::from_ref(&db_path)).unwrap();
        let rows = opencode_db_rows(&db_path, "openai");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].0, alice_id);
        assert_eq!(rows[1].1, "Alice Work");
        assert_eq!(rows[1].2["methodID"], "chatgpt-headless");
        assert_eq!(rows[1].2["refresh"], "rt-a2");
        assert_eq!(rows[1].2["metadata"]["accountID"], "ws-team");
        assert_eq!(rows[1].2["metadata"]["custom"], "keep");

        // 4. Switching to Bob in the SAME Team workspace creates a separate row and deactivates Alice.
        let bob = codex_credential("bob@example.com", "ws-team", "team", 1_780_007_200, "rt-b1");
        sync_opencode(&bob, std::slice::from_ref(&db_path)).unwrap();
        let rows = opencode_db_rows(&db_path, "openai");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].0, alice_id);
        assert_eq!(rows[1].3, Some(0));
        let bob_id = rows[2].0.clone();
        assert_ne!(bob_id, alice_id);
        assert_eq!(rows[2].1, "bob@example.com");
        assert_eq!(rows[2].3, Some(1));
        assert_eq!(rows[2].2["metadata"]["accountID"], "ws-team");

        // 5. Switching back to Alice reactivates Alice's existing row without duplicating.
        sync_opencode(&alice2, std::slice::from_ref(&db_path)).unwrap();
        let rows = opencode_db_rows(&db_path, "openai");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].3, Some(1));
        assert_eq!(rows[2].3, Some(0));
    }

    #[test]
    fn sync_opencode_db_writes_and_updates_xai_device_oauth() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("opencode.db");
        init_opencode_v2_db(&db_path);

        let alice_access = fake_jwt(json!({
            "sub": "uid-alice",
            "email": "alice@x.example",
            "exp": 1_800_000_000_i64
        }));
        let alice1 = OAuthCredential {
            access: alice_access,
            refresh: "xai-rt-a1".into(),
            expires_ms: 1_800_000_000_000,
            account_id: Some("uid-alice".into()),
            email: Some("Alice@x.example".into()),
            ..OAuthCredential::default()
        };
        sync_opencode_grok(&alice1, std::slice::from_ref(&db_path)).unwrap();

        let rows = opencode_db_rows(&db_path, "xai");
        assert_eq!(rows.len(), 1);
        let alice_id = rows[0].0.clone();
        assert_eq!(rows[0].1, "Alice@x.example");
        assert_eq!(rows[0].2["type"], "oauth");
        assert_eq!(rows[0].2["methodID"], "device");
        assert_eq!(rows[0].2["refresh"], "xai-rt-a1");
        assert!(rows[0].2.get("metadata").is_none());
        assert_eq!(rows[0].3, Some(1));

        // Token renewal updates Alice in place by matching JWT `sub`.
        let alice_access2 = fake_jwt(json!({
            "sub": "uid-alice",
            "email": "alice@x.example",
            "exp": 1_800_003_600_i64
        }));
        let alice2 = OAuthCredential {
            access: alice_access2,
            refresh: "xai-rt-a2".into(),
            expires_ms: 1_800_003_600_000,
            account_id: Some("uid-alice".into()),
            email: Some("alice@x.example".into()),
            ..OAuthCredential::default()
        };
        sync_opencode_grok(&alice2, std::slice::from_ref(&db_path)).unwrap();
        let rows = opencode_db_rows(&db_path, "xai");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, alice_id);
        assert_eq!(rows[0].2["refresh"], "xai-rt-a2");

        // Switching to Bob adds a separate active xai row and deactivates Alice.
        let bob = grok_credential("bob", "xai-rt-b1");
        sync_opencode_grok(&bob, std::slice::from_ref(&db_path)).unwrap();
        let rows = opencode_db_rows(&db_path, "xai");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].3, Some(0));
        assert_eq!(rows[1].1, "bob@x.example");
        assert_eq!(rows[1].2["methodID"], "device");
        assert_eq!(rows[1].3, Some(1));
    }

    #[test]
    fn agents_match_an_existing_row_whose_email_differs_only_in_case() {
        let dir = tempfile::tempdir().unwrap();
        // An omp row keyed by oh-my-pi itself, with the email spelled otherwise.
        let db_path = dir.path().join("agent.db");
        sync_omp_one(
            &OAuthCredential {
                access: "old-access".into(),
                refresh: "old-refresh".into(),
                expires_ms: 1,
                org_id: Some("org-team".into()),
                ..OAuthCredential::default()
            },
            ANTHROPIC_PROVIDER,
            &db_path,
        )
        .unwrap();
        Connection::open(&db_path)
            .unwrap()
            .execute(
                "UPDATE auth_credentials SET identity_key = 'email:ALICE@Example.COM|org:org-team'",
                [],
            )
            .unwrap();
        let existing = omp_rows(&db_path, "anthropic")[0].0;
        SyncAgent::Omp
            .sync_credential(
                Pool::Claude,
                &claude_credential("alice", "org-team", "rt-new"),
                std::slice::from_ref(&db_path),
            )
            .unwrap();
        let rows = omp_rows(&db_path, "anthropic");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, existing);
        assert_eq!(rows[0].1["refresh"], "rt-new");
        assert_eq!(rows[0].1["email"], "alice@Example.com");

        // An OpenCode row of the same ChatGPT workspace member.
        let db_path = dir.path().join("opencode.db");
        init_opencode_v2_db(&db_path);
        let old_access = fake_jwt(json!({
            "https://api.openai.com/auth": {"chatgpt_account_id": "ws-team"},
            "https://api.openai.com/profile": {"email": "ALICE@EXAMPLE.COM"}
        }));
        Connection::open(&db_path)
            .unwrap()
            .execute(
                "INSERT INTO credential (id, integration_id, label, value, active, time_created, time_updated) VALUES ('cred_alice', 'openai', 'Alice', ?1, 1, 1000, 1000)",
                params![json!({"type": "oauth", "access": old_access, "refresh": "old-refresh", "metadata": {"accountID": "ws-team"}}).to_string()],
            )
            .unwrap();
        let alice = codex_credential(
            "Alice@Example.com",
            "ws-team",
            "team",
            1_780_000_000,
            "rt-a",
        );
        sync_opencode(&alice, std::slice::from_ref(&db_path)).unwrap();
        let rows = opencode_db_rows(&db_path, "openai");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "cred_alice");
        assert_eq!(rows[0].2["refresh"], "rt-a");
    }

    #[test]
    fn sync_opencode_db_rejects_uninitialized_database() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("opencode.db");
        let auth = codex_credential("a@example.com", "acct-1", "plus", 1_780_000_000, "rt-1");
        assert!(sync_opencode(&auth, std::slice::from_ref(&missing))
            .unwrap_err()
            .contains("not found"));
        assert!(!missing.exists());

        // An empty DB without the `credential` table is rejected without mutating schema.
        Connection::open(&missing).unwrap();
        assert!(sync_opencode(&auth, std::slice::from_ref(&missing))
            .unwrap_err()
            .contains("missing the credential table"));
    }
}
