//! The third-party agents a switch syncs its login to (OpenCode, pi, omp):
//! each one's row of `SyncAgent`, where its logins are written and how.
use super::{checked, executable};
use crate::{
    persistence::write_json,
    pool_login::{codex, OAuthCredential},
    settings::Settings,
    utils::now_unix_seconds,
    AgentSyncOutcome, Pool, Preferences,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

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
    // A login without an account id keeps no earlier login's, as the
    // database's rows do.
    if include_account_id {
        match &credential.account_id {
            Some(id) => entry.insert("accountId".into(), id.clone().into()),
            None => entry.remove("accountId"),
        };
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

fn pi_paths() -> Vec<PathBuf> {
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

fn omp_paths() -> Vec<PathBuf> {
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
    // Off the async threads: without a version, the database is opened to
    // look for OpenCode's credential table, which may wait for its lock.
    let json_paths = opencode_paths();
    let fallback = json_paths.clone();
    tokio::task::spawn_blocking(move || {
        resolve_opencode_target(
            installed_major,
            home.as_deref(),
            auth_path.as_deref(),
            db_env.as_deref(),
            xdg_data_home.as_deref(),
            json_paths,
        )
    })
    .await
    .unwrap_or(OpencodeTarget::Json(fallback))
}

fn opencode_paths() -> Vec<PathBuf> {
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

/// Syncs the login a switch in `pool` granted to every agent the
/// preferences send that pool's logins to, where each one keeps them.
pub(super) async fn sync(
    pool: Pool,
    p: &Preferences,
    credential: Result<OAuthCredential, String>,
) -> Vec<AgentSyncOutcome> {
    sync_to_agents(pool, p, credential, SyncAgent::locate).await
}

#[cfg(test)]
mod tests;
