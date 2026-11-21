//! Sub2api in the Antigravity pool: its Antigravity OAuth accounts as cards,
//! and agy pointed at its `/antigravity` gateway by `"modelProvider":
//! "gemini"` in agy's settings. agy then sends `GEMINI_API_KEY` to
//! `GOOGLE_GEMINI_BASE_URL`, both read from its environment only. A
//! provider names the variable holding its key, as a Codex provider's
//! `env_key` does, and Copool never holds the key itself; its gateway is
//! its Sub2api address. Copool assigns both, the key as that variable's
//! value, in a block of its own at the end of `~/.gemini/.env`, which
//! Gemini CLI reads and agy has from a shell that loads it, and switches
//! only once a new shell exports them so. An account's switch removes the
//! block. What the pools do alike with their providers is in
//! `providers.rs`; here is where the Antigravity pool keeps them and how it
//! uses one.
use super::{Result, Store};
use crate::{
    application::{Application, SwitchExecution},
    codex_config::Definition,
    pool::Pool,
    sub2api::{self, Account, Member},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// agy's switch from its Google login to the Gemini API.
const MODEL_PROVIDER: &str = "modelProvider";
const GEMINI: &str = "gemini";
/// What agy reads in Gemini API mode, from its environment only.
pub(super) const API_KEY: &str = "GEMINI_API_KEY";
pub(super) const BASE_URL: &str = "GOOGLE_GEMINI_BASE_URL";
pub(crate) const API_KEY_MISSING: &str = "error.antigravity.gemini_api_key_missing";
pub(crate) const API_KEY_VARIABLE_INVALID: &str = "error.antigravity.api_key_variable_invalid";
pub(crate) const ENV_NOT_LOADED: &str = "error.antigravity.gemini_env_not_loaded";
pub(crate) const SETTINGS_INVALID: &str = "error.antigravity.settings_invalid";
const CONFIGURATION_INCOMPLETE: &str = "error.sub2api.configuration_incomplete";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    /// The Sub2api address agy sends to, as its root or its `/antigravity`
    /// gateway: its switch sets `GOOGLE_GEMINI_BASE_URL` to the gateway.
    /// Through Coport it is Coport's, with Sub2api's in its path
    /// (`http://127.0.0.1:8787/antigravity/https://relay.example.com`), as
    /// `ANTHROPIC_BASE_URL` names a Claude gateway through Coport; the admin
    /// API is then derived from the Sub2api address in it, reached directly.
    #[serde(rename = "baseURL")]
    pub base_url: String,
    /// Optional admin API override, independent of agy's gateway. Never
    /// taken from `GOOGLE_GEMINI_BASE_URL`: the administrator's password
    /// goes only where the form says.
    #[serde(rename = "adminBaseURL")]
    pub admin_base_url: String,
    pub username: String,
    /// Only in a draft from the form; the system keyring keeps it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub password: String,
    /// The variable holding the Sub2api API key agy sends, as a new shell
    /// exports it: `GEMINI_API_KEY` itself unless named. Never the key.
    pub api_key_variable: String,
    /// The proxy of every request for the provider and its accounts: blank
    /// for the system settings, "direct://" for none.
    #[serde(rename = "proxyURL")]
    pub proxy_url: String,
    #[serde(rename = "allowInsecureTLS")]
    pub allow_insecure_tls: bool,
    #[serde(rename = "importedAccountIDs")]
    pub imported_account_ids: Vec<i64>,
    pub cached_accounts: Vec<Account>,
}
/// The Sub2api root of an address: without its `/antigravity` gateway.
fn root(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    url.strip_suffix("/antigravity")
        .unwrap_or(url)
        .trim_end_matches('/')
        .to_owned()
}
/// agy's gateway at a provider, its `GOOGLE_GEMINI_BASE_URL`.
pub(super) fn gateway(p: &Provider) -> Result<String> {
    if p.base_url.is_empty() {
        return Err(CONFIGURATION_INCOMPLETE.into());
    }
    Ok(format!("{}/antigravity", p.base_url))
}
/// The Sub2api address in an address: the upstream a proxy's path names
/// (`<proxy>/antigravity/https://relay`), else the address itself.
fn upstream(url: &str) -> &str {
    ["/https://", "/http://"]
        .iter()
        .filter_map(|scheme| url.find(scheme))
        .min()
        .map_or(url, |at| &url[at + 1..])
}
/// The variable holding a provider's key.
pub(super) fn key_variable(p: &Provider) -> &str {
    match p.api_key_variable.as_str() {
        "" => API_KEY,
        name => name,
    }
}
/// Whether a value can be assigned in single quotes, literally, to a shell
/// and to dotenv alike.
fn quotable(value: &str) -> bool {
    !value.is_empty()
        && !value
            .chars()
            .any(|c| c == '\'' || c == '"' || c.is_whitespace() || c.is_control())
}
/// An Antigravity provider's cards, named by its id.
fn member(p: &Provider) -> Member {
    Member {
        key: p.id.clone(),
        name: p.name.clone(),
        proxy_url: p.proxy_url.clone(),
        imported: p.imported_account_ids.clone(),
        cached: p.cached_accounts.clone(),
    }
}

/// `modelProvider` as agy's settings had it before a provider set it (none
/// when absent): an account's switch returns it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct SettingsRecord {
    model_provider: Option<Value>,
}

/// A file's text, agy's settings or `.env`; none when there is none.
fn read_text(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
/// The settings object; agy accepts a byte order mark, and so does Copool.
fn parse_settings(text: Option<&str>) -> Result<Map<String, Value>> {
    let text = text
        .map(|t| t.trim_start_matches('\u{feff}'))
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("{}");
    match serde_json::from_str(text) {
        Ok(Value::Object(settings)) => Ok(settings),
        _ => Err(SETTINGS_INVALID.into()),
    }
}
/// Settings in place of `previous`, in its newlines, byte order mark and
/// final newline. Keys keep their order; JSON strings hold no raw newline.
fn render_settings(settings: &Map<String, Value>, previous: Option<&str>) -> Result<String> {
    let mut text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    if previous.is_none_or(|t| t.trim_end_matches([' ', '\t']).ends_with('\n')) {
        text.push('\n');
    }
    if previous.is_some_and(|t| t.contains("\r\n")) {
        text = text.replace('\n', "\r\n");
    }
    if previous.is_some_and(|t| t.starts_with('\u{feff}')) {
        text.insert(0, '\u{feff}');
    }
    Ok(text)
}
/// Writes a file verbatim, or removes it for none.
fn put_text(path: &Path, text: Option<&str>) -> Result<()> {
    match text {
        Some(text) => crate::claude::write_text(path, text),
        None if path.exists() => std::fs::remove_file(path).map_err(|e| e.to_string()),
        None => Ok(()),
    }
}
fn uses_api_key(settings: &Map<String, Value>) -> bool {
    settings.get(MODEL_PROVIDER).and_then(Value::as_str) == Some(GEMINI)
}
/// Whether agy's settings put it on the Gemini API, off its login.
pub(super) fn api_key_mode(path: &Path) -> bool {
    read_text(path)
        .ok()
        .and_then(|text| parse_settings(text.as_deref()).ok())
        .is_some_and(|settings| uses_api_key(&settings))
}
/// What `leave_api_key` changed, as it was: agy's settings and its `.env`,
/// each none when unchanged.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Left {
    settings: Option<Option<String>>,
    env: Option<(PathBuf, Option<String>)>,
}
/// Puts agy back on its login: `modelProvider` as it was before a provider
/// set it, else none, and `.env` without Copool's block. Forgets the
/// provider and its record. Returns what it changed, to restore on a later
/// failure. Settings Copool cannot read refuse the switch only when it put
/// a provider in them: an account's switch never depends on agy's settings
/// otherwise; nor ever on its `.env`.
pub(super) fn leave_api_key(path: &Path, store: &mut Store) -> Result<Left> {
    let applied = store.settings_provider.is_some() || store.settings_record.is_some();
    let record = store.settings_record.take();
    store.settings_provider = None;
    let settings = leave_settings(path, record, applied)?;
    let env = env_path(path);
    let env = match read_text(&env) {
        Ok(Some(text)) if has_block(&text) => {
            let left = without_block(&text);
            put_text(&env, (!left.trim().is_empty()).then_some(&*left))
                .ok()
                .map(|()| (env, Some(text)))
        }
        _ => None,
    };
    Ok(Left { settings, env })
}
fn leave_settings(
    path: &Path,
    record: Option<SettingsRecord>,
    applied: bool,
) -> Result<Option<Option<String>>> {
    let read = read_text(path)
        .and_then(|text| parse_settings(text.as_deref()).map(|settings| (text, settings)));
    let (text, mut settings) = match read {
        Ok(read) => read,
        Err(error) if applied => return Err(error),
        Err(_) => return Ok(None),
    };
    if !uses_api_key(&settings) {
        return Ok(None);
    }
    // A record taken while agy already used the Gemini API keeps none.
    match record
        .and_then(|r| r.model_provider)
        .filter(|v| v.as_str() != Some(GEMINI))
    {
        Some(previous) => {
            settings.insert(MODEL_PROVIDER.into(), previous);
        }
        None => {
            settings.shift_remove(MODEL_PROVIDER);
        }
    }
    put_text(path, Some(&render_settings(&settings, text.as_deref())?))?;
    Ok(Some(text))
}
/// Restores what `leave_api_key` changed.
pub(super) fn restore_settings(path: &Path, left: Left) {
    if let Some(previous) = left.settings {
        let _ = put_text(path, previous.as_deref());
    }
    if let Some((env, previous)) = left.env {
        let _ = put_text(&env, previous.as_deref());
    }
}

/// Gemini's `.env`, two levels above agy's settings: `~/.gemini/.env`.
pub(super) fn env_path(settings: &Path) -> PathBuf {
    settings
        .parent()
        .and_then(Path::parent)
        .unwrap_or(Path::new(""))
        .join(".env")
}
/// Copool's block in `.env`: the user's own lines above it stay, and the
/// last assignment wins, to a shell and to dotenv alike.
const BLOCK_START: &str = "# >>> Copool: agy's Sub2api provider (rewritten on each switch) >>>";
const BLOCK_END: &str = "# <<< Copool <<<";
fn has_block(text: &str) -> bool {
    text.lines().any(|line| line.trim_end() == BLOCK_START)
}
/// The text without Copool's block and the blank line `with_block` put
/// before it, its other lines as they were.
fn without_block(text: &str) -> String {
    let mut inside = false;
    let mut kept: Vec<&str> = Vec::new();
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if trimmed == BLOCK_START {
            inside = true;
            if kept.last().is_some_and(|l| l.trim().is_empty()) {
                kept.pop();
            }
        }
        if !inside {
            kept.push(line);
        }
        if trimmed == BLOCK_END {
            inside = false;
        }
    }
    kept.concat()
}
/// `.env` with Copool's block at its end, assigning agy's key from the
/// provider's variable and its gateway, in the file's newlines.
fn with_block(text: Option<&str>, variable: &str, gateway: &str) -> Result<String> {
    let mut lines = vec![BLOCK_START.to_owned()];
    if !crate::proxy_integration::is_variable(variable) {
        return Err(API_KEY_VARIABLE_INVALID.into());
    }
    // Expanded where it is loaded, after the user's own exports;
    // GEMINI_API_KEY itself is the user's to export.
    if variable != API_KEY {
        lines.push(format!("{API_KEY}=\"${{{variable}}}\""));
    }
    if !quotable(gateway) {
        return Err("Invalid Sub2api URL".into());
    }
    lines.push(format!("{BASE_URL}='{gateway}'"));
    lines.push(BLOCK_END.to_owned());
    let text = without_block(text.unwrap_or(""));
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    // A blank line sets the block off from the user's lines.
    let separator = if text.is_empty() {
        String::new()
    } else if text.ends_with('\n') {
        newline.to_owned()
    } else {
        newline.repeat(2)
    };
    Ok(format!("{text}{separator}{}{newline}", lines.join(newline)))
}

/// The lines a shell's startup file loads Gemini's `.env` with, exporting
/// what it sets; zsh and bash both read them.
fn rc_lines(env: &Path) -> Result<String> {
    let env = env
        .to_str()
        .filter(|p| !p.contains(['\'', '\n']))
        .ok_or("Invalid .env path")?;
    Ok(format!(
        "# Copool: agy's Sub2api key and gateway, from Gemini's .env.\n\
         [ -f '{env}' ] && {{ set -a; . '{env}'; set +a; }}\n"
    ))
}
/// Has the startup file load `.env` last, after anything else it exports,
/// unless it already ends so. Returns its length before, none when it was
/// absent, for `unload_env_in`; none at all when nothing changed.
fn load_env_in(rc: &Path, env: &Path) -> Result<Option<Option<u64>>> {
    use std::io::Write;
    let lines = rc_lines(env)?;
    let text = read_text(rc)?;
    if text.as_deref().is_some_and(|t| t.ends_with(&lines)) {
        return Ok(None);
    }
    let before = text.as_ref().map(|t| t.len() as u64);
    let separator = match text.as_deref() {
        None | Some("") => "",
        Some(t) if t.ends_with('\n') => "\n",
        Some(_) => "\n\n",
    };
    // Appended in place: the file keeps its mode and any link to it.
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(rc)
        .and_then(|mut file| file.write_all(format!("{separator}{lines}").as_bytes()))
        .map_err(|e| e.to_string())?;
    Ok(Some(before))
}
/// Takes back what `load_env_in` appended.
fn unload_env_in(rc: &Path, before: Option<u64>) {
    let _ = match before {
        None => std::fs::remove_file(rc),
        Some(len) => std::fs::OpenOptions::new()
            .write(true)
            .open(rc)
            .and_then(|file| file.set_len(len)),
    };
}
/// What a provider's switch changed outside agy's settings, to take back
/// when it fails.
struct Changes {
    env: PathBuf,
    env_before: Option<String>,
    rc: Option<(PathBuf, Option<u64>)>,
}
impl Changes {
    fn undo(&self) {
        if let Some((rc, before)) = &self.rc {
            unload_env_in(rc, *before);
        }
        let _ = put_text(&self.env, self.env_before.as_deref());
    }
}

impl Application {
    /// The Antigravity providers for the settings, without their secrets.
    pub(crate) fn antigravity_providers(&self) -> Result<Vec<Provider>> {
        Ok(self.antigravity_load()?.providers)
    }
    pub(crate) async fn antigravity_save_provider(&self, mut draft: Provider) -> Result<Provider> {
        draft.name = draft.name.trim().into();
        draft.username = draft.username.trim().into();
        draft.base_url = root(&draft.base_url);
        draft.admin_base_url = draft.admin_base_url.trim().trim_end_matches('/').into();
        draft.proxy_url = draft.proxy_url.trim().into();
        // agy's gateway is the Sub2api URL; the admin API only overrides.
        if draft.base_url.is_empty() {
            return Err(CONFIGURATION_INCOMPLETE.into());
        }
        draft.api_key_variable = match draft.api_key_variable.trim() {
            "" => API_KEY.into(),
            name if crate::proxy_integration::is_variable(name) => name.into(),
            // A key typed here would be kept in plain text: never.
            _ => return Err(API_KEY_VARIABLE_INVALID.into()),
        };
        if draft.id.trim().is_empty() {
            draft.id = uuid::Uuid::new_v4().to_string();
        }
        self.antigravity_edit_providers(|providers| {
            let existing = providers.iter().position(|p| p.id == draft.id);
            sub2api::validate_form(
                &draft.name,
                providers
                    .iter()
                    .any(|p| p.id != draft.id && p.name.eq_ignore_ascii_case(&draft.name)),
                &draft.username,
                &draft.password,
                existing.is_some(),
                &draft.proxy_url,
                &[
                    &draft.base_url,
                    upstream(&draft.base_url),
                    &draft.admin_base_url,
                ],
            )?;
            if let Some(old) = existing.map(|i| &providers[i]) {
                draft.imported_account_ids = old.imported_account_ids.clone();
                draft.cached_accounts = old.cached_accounts.clone();
                // The provider's card sets its proxy, not its settings.
                draft.proxy_url = old.proxy_url.clone();
            }
            self.store_provider_password(Pool::Antigravity, &draft.id, &draft.password)?;
            draft.password.clear();
            match existing {
                Some(i) => providers[i] = draft.clone(),
                None => providers.push(draft.clone()),
            }
            Ok(())
        })
        .await?;
        Ok(draft)
    }
    pub(crate) async fn antigravity_remove_provider(&self, id: &str) -> Result<()> {
        self.antigravity_edit_providers(|providers| {
            providers.retain(|p| p.id != id);
            Ok(())
        })
        .await
    }
    /// Edits the providers alone; the logins stay as stored.
    async fn antigravity_edit_providers<T>(
        &self,
        edit: impl FnOnce(&mut Vec<Provider>) -> Result<T>,
    ) -> Result<T> {
        let _lock = self.antigravity.lock.lock().await;
        let mut store = self.antigravity_load()?;
        let result = edit(&mut store.providers)?;
        self.antigravity_save(&store)?;
        Ok(result)
    }
    pub(crate) fn antigravity_members(&self) -> Result<Vec<Member>> {
        Ok(self
            .antigravity_load()?
            .providers
            .iter()
            .map(member)
            .collect())
    }
    pub(crate) async fn antigravity_edit_member<T>(
        &self,
        key: &str,
        edit: impl FnOnce(&mut Member) -> T,
    ) -> Result<Option<T>> {
        self.antigravity_edit_providers(|providers| {
            let Some(p) = providers
                .iter_mut()
                .find(|p| p.id.eq_ignore_ascii_case(key))
            else {
                return Ok(None);
            };
            let mut member = member(p);
            let result = edit(&mut member);
            p.imported_account_ids = member.imported;
            p.cached_accounts = member.cached;
            p.proxy_url = member.proxy_url;
            Ok(Some(result))
        })
        .await
    }
    pub(crate) fn antigravity_member_connection(&self, key: &str) -> Result<sub2api::Connection> {
        let p = self
            .antigravity_load()?
            .providers
            .into_iter()
            .find(|p| p.id.eq_ignore_ascii_case(key))
            .ok_or("Provider not found")?;
        let password = self
            .secrets
            .get(&sub2api::password_secret(Pool::Antigravity, &p.id))?
            .ok_or("error.sub2api.configuration_incomplete")?;
        // Sub2api itself, not a proxy in front of agy's requests.
        let admin = if p.admin_base_url.is_empty() {
            upstream(&p.base_url)
        } else {
            &p.admin_base_url
        };
        Ok(sub2api::Connection {
            admin_base: sub2api::admin_base(admin)?,
            definition: Definition {
                id: p.name.clone(),
                base_url: Some(format!("{}/antigravity", p.base_url)),
                wire_api: None,
                env_key: Some(API_KEY.into()),
                requires_openai_auth: None,
            },
            configuration: crate::settings::Provider {
                id: p.id,
                provider_id: p.name,
                username: p.username,
                password,
                allow_insecure_tls: p.allow_insecure_tls,
                proxy_url: p.proxy_url,
                imported_account_ids: p.imported_account_ids,
                cached_accounts: p
                    .cached_accounts
                    .iter()
                    .map(serde_json::to_value)
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?,
                ..Default::default()
            },
        })
    }
    /// Variables in agy's environment, as the user's shell exports them;
    /// with `now`, as one shell opened now does. Asked off the runtime, for
    /// the shell may take seconds.
    async fn antigravity_exported(&self, names: &[&str], now: bool) -> Result<Vec<Option<String>>> {
        let environment = self.antigravity.environment.read().unwrap().clone();
        let names: Vec<String> = names.iter().map(|&name| name.to_owned()).collect();
        tokio::task::spawn_blocking(move || {
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let mut values = environment(&names, now);
            values.resize(names.len(), None);
            values
        })
        .await
        .map_err(|e| e.to_string())
    }
    /// Whether agy started from a terminal opened now has the gateway, and
    /// the variable's value as its key.
    async fn antigravity_env_loaded(&self, variable: &str, gateway: &str) -> Result<bool> {
        let values = self
            .antigravity_exported(&[BASE_URL, API_KEY, variable], true)
            .await?;
        let url = values[0]
            .as_deref()
            .map(|url| url.trim().trim_end_matches('/'));
        Ok(url == Some(gateway) && values[1].is_some() && values[1] == values[2])
    }
    /// Assigns agy's key and gateway in Copool's block of Gemini's `.env`,
    /// and has the shell's startup file load it when a new shell does not
    /// export them so. Takes all back when even then it does not.
    async fn antigravity_put_env(
        &self,
        settings: &Path,
        variable: &str,
        gateway: &str,
    ) -> Result<Changes> {
        let env = env_path(settings);
        let env_before = read_text(&env)?;
        put_text(
            &env,
            Some(&with_block(env_before.as_deref(), variable, gateway)?),
        )?;
        let mut changes = Changes {
            env,
            env_before,
            rc: None,
        };
        let mut loaded = self.antigravity_env_loaded(variable, gateway).await;
        if let (Ok(false), Some(rc)) = (&loaded, self.antigravity_shell_rc()) {
            loaded = match load_env_in(&rc, &changes.env) {
                Ok(Some(before)) => {
                    changes.rc = Some((rc, before));
                    self.antigravity_env_loaded(variable, gateway).await
                }
                // It loads the file last already: something else differs.
                Ok(None) => Ok(false),
                Err(error) => Err(error),
            };
        }
        match loaded {
            Ok(true) => Ok(changes),
            other => {
                changes.undo();
                Err(other.err().unwrap_or_else(|| ENV_NOT_LOADED.into()))
            }
        }
    }
    /// Puts agy on the provider's gateway: its key and gateway in Gemini's
    /// `.env`, and `"modelProvider": "gemini"` in agy's settings, recording
    /// what that replaced; the login in the Keychain stays, for an
    /// account's switch to return to. Called under the pool's operations
    /// lock.
    pub(super) async fn antigravity_switch_sub2api(&self, id: &str) -> Result<SwitchExecution> {
        let (member, account) = self.card(Pool::Antigravity, id)?;
        if !account.status.eq_ignore_ascii_case("active") {
            return Err("Account is deactivated".into());
        }
        let provider = self
            .antigravity_load()?
            .providers
            .into_iter()
            .find(|p| p.id == member.key)
            .ok_or("Account not found")?;
        let variable = key_variable(&provider);
        // A variable nothing exports would leave agy without a key.
        if self.antigravity_exported(&[variable], false).await?[0].is_none() {
            return Err(API_KEY_MISSING.into());
        }
        let path = self.antigravity_settings_path();
        // Outside the store's lock: the shell may take seconds.
        let changes = self
            .antigravity_put_env(&path, variable, &gateway(&provider)?)
            .await?;
        let applied = self.antigravity_apply_gemini(&path, member.key).await;
        if applied.is_err() {
            changes.undo();
        }
        applied?;
        Ok(SwitchExecution {
            provider: Some(provider.name),
            ..Default::default()
        })
    }
    /// `"modelProvider": "gemini"` in agy's settings, for the provider.
    async fn antigravity_apply_gemini(&self, path: &Path, provider: String) -> Result<()> {
        let _lock = self.antigravity.lock.lock().await;
        let mut store = self.antigravity_load()?;
        let text = read_text(path)?;
        let mut settings = parse_settings(text.as_deref())?;
        // Between providers, the record of the login's settings stays.
        if store.settings_record.is_none() {
            store.settings_record = Some(SettingsRecord {
                model_provider: settings.get(MODEL_PROVIDER).cloned(),
            });
        }
        settings.insert(MODEL_PROVIDER.into(), GEMINI.into());
        put_text(path, Some(&render_settings(&settings, text.as_deref())?))?;
        store.settings_provider = Some(provider);
        if let Err(error) = self.antigravity_save(&store) {
            let _ = put_text(path, text.as_deref());
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(model_provider: Option<Value>) -> Store {
        Store {
            settings_record: Some(SettingsRecord { model_provider }),
            settings_provider: Some("provider".into()),
            ..Default::default()
        }
    }

    #[test]
    fn leaving_the_api_returns_modelprovider_and_keeps_the_rest_in_its_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cli/settings.json");
        for newline in ["\n", "\r\n"] {
            for bom in ["", "\u{feff}"] {
                let applied = format!(
                    "{bom}{{{newline}  \"trustedWorkspaces\": [\"/w\"],{newline}  \"modelProvider\": \"gemini\",{newline}  \"agentMode\": \"auto\"{newline}}}{newline}"
                );
                fs_write(&path, &applied);
                let mut store = record(None);
                let previous = leave_api_key(&path, &mut store).unwrap();
                assert_eq!(previous.settings, Some(Some(applied.clone())));
                assert_eq!(previous.env, None);
                assert!(store.settings_record.is_none() && store.settings_provider.is_none());
                let left = std::fs::read_to_string(&path).unwrap();
                // The format stays: its newlines, mark and key order.
                assert_eq!(left.starts_with('\u{feff}'), !bom.is_empty());
                let crlf = left.matches("\r\n").count();
                if newline == "\r\n" {
                    assert_eq!(crlf, left.matches('\n').count());
                } else {
                    assert_eq!(crlf, 0);
                }
                assert!(left.ends_with(newline));
                let value = parse_settings(Some(&left)).unwrap();
                assert_eq!(
                    value.keys().collect::<Vec<_>>(),
                    ["trustedWorkspaces", "agentMode"]
                );
                assert_eq!(value["trustedWorkspaces"], serde_json::json!(["/w"]));
                assert!(!api_key_mode(&path));
                // A failed switch puts them back.
                restore_settings(&path, previous);
                assert_eq!(std::fs::read_to_string(&path).unwrap(), applied);
            }
        }
    }

    #[test]
    fn leaving_returns_an_earlier_value_but_never_gemini() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cli/settings.json");
        for (recorded, expected) in [
            (Some(Value::from("vertex")), Some(Value::from("vertex"))),
            (Some(Value::from(GEMINI)), None),
            (None, None),
        ] {
            fs_write(&path, r#"{"modelProvider":"gemini"}"#);
            let mut store = record(recorded);
            leave_api_key(&path, &mut store).unwrap();
            let left = parse_settings(Some(&std::fs::read_to_string(&path).unwrap())).unwrap();
            assert_eq!(left.get(MODEL_PROVIDER).cloned(), expected);
        }
        // Settings agy already uses its login with are not rewritten.
        let unchanged = "{\"modelProvider\":\"vertex\"}";
        fs_write(&path, unchanged);
        assert_eq!(
            leave_api_key(&path, &mut record(None)).unwrap(),
            Left::default()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), unchanged);
        // No settings at all stay none.
        let missing = dir.path().join("missing/cli/settings.json");
        assert_eq!(
            leave_api_key(&missing, &mut record(None)).unwrap(),
            Left::default()
        );
        assert!(!missing.exists());
        // Settings that are not a JSON object are refused, not replaced,
        // once Copool put a provider in them; otherwise left alone.
        for invalid in ["[]", "{not json"] {
            fs_write(&path, invalid);
            assert_eq!(
                leave_api_key(&path, &mut record(None)).unwrap_err(),
                SETTINGS_INVALID
            );
            assert_eq!(
                leave_api_key(&path, &mut Store::default()).unwrap(),
                Left::default()
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
        }
    }

    #[test]
    fn agys_gateway_is_the_address_and_admin_is_the_sub2api_in_it() {
        let direct = "https://182.92.106.196:6060";
        let coport = format!("http://127.0.0.1:8787/antigravity/{direct}");
        for (raw, base) in [
            (format!("{direct}/antigravity/"), direct.to_owned()),
            (format!("{coport}/antigravity"), coport.clone()),
            (coport.clone(), coport.clone()),
        ] {
            let p = Provider {
                base_url: root(&raw),
                ..Default::default()
            };
            assert_eq!(p.base_url, base, "{raw}");
            assert_eq!(gateway(&p).unwrap(), format!("{base}/antigravity"));
            // The admin password goes to Sub2api, not through Coport.
            assert_eq!(upstream(&p.base_url), direct, "{raw}");
        }
        assert_eq!(
            upstream("http://relay.example.com"),
            "http://relay.example.com"
        );
        assert_eq!(
            upstream("http://127.0.0.1:8787/antigravity/http://relay.example.com"),
            "http://relay.example.com"
        );
        assert_eq!(
            gateway(&Provider::default()).unwrap_err(),
            CONFIGURATION_INCOMPLETE
        );
    }

    #[test]
    fn a_provider_is_its_sub2api_root() {
        for raw in [
            "https://relay.example.com",
            "https://relay.example.com/",
            "https://relay.example.com/antigravity",
            " https://relay.example.com/antigravity/ ",
        ] {
            assert_eq!(root(raw), "https://relay.example.com", "{raw}");
        }
    }

    #[test]
    fn copools_block_ends_env_and_leaves_the_users_lines() {
        let url = "https://relay.example.com/antigravity";
        let block = |text: Option<&str>, variable: &str| with_block(text, variable, url).unwrap();
        let own = "# mine\nGEMINI_API_KEY=k\nGOOGLE_GEMINI_BASE_URL=https://old\n";
        let applied = block(Some(own), "RELAY_KEY");
        assert_eq!(
            applied,
            format!("{own}\n{BLOCK_START}\nGEMINI_API_KEY=\"${{RELAY_KEY}}\"\nGOOGLE_GEMINI_BASE_URL='{url}'\n{BLOCK_END}\n")
        );
        // Rewritten in place on the next switch, the user's lines kept.
        // GEMINI_API_KEY itself is the user's to export.
        assert_eq!(
            block(Some(&applied), API_KEY),
            format!("{own}\n{BLOCK_START}\nGOOGLE_GEMINI_BASE_URL='{url}'\n{BLOCK_END}\n")
        );
        // Removed with its blank line; a blank line of the user's stays.
        assert_eq!(without_block(&applied), own);
        let spaced = format!("{own}\n");
        assert_eq!(without_block(&block(Some(&spaced), "RELAY_KEY")), spaced);
        // A `.env` of Copool's block alone starts with it.
        assert!(block(None, "RELAY_KEY").starts_with(BLOCK_START));
        // The file's newlines, after a last line without one.
        assert_eq!(
            block(Some("A=1\r\nB=2"), "RELAY_KEY"),
            format!("A=1\r\nB=2\r\n\r\n{BLOCK_START}\r\nGEMINI_API_KEY=\"${{RELAY_KEY}}\"\r\nGOOGLE_GEMINI_BASE_URL='{url}'\r\n{BLOCK_END}\r\n")
        );
        // Only a variable's name, never a key, and no address to escape.
        for invalid in ["sk-1", "1KEY", "A B", "${A}", ""] {
            assert_eq!(
                with_block(None, invalid, url).unwrap_err(),
                API_KEY_VARIABLE_INVALID,
                "{invalid:?}"
            );
        }
        assert!(with_block(None, API_KEY, "https://a/'b").is_err());
    }

    #[test]
    fn leaving_removes_copools_block_and_restores_it_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cli/settings.json");
        fs_write(&path, r#"{"modelProvider":"gemini"}"#);
        let env = env_path(&path);
        let own = "GEMINI_API_KEY=k\n";
        let applied = with_block(Some(own), "RELAY_KEY", "https://r").unwrap();
        fs_write(&env, &applied);
        let left = leave_api_key(&path, &mut record(None)).unwrap();
        assert_eq!(std::fs::read_to_string(&env).unwrap(), own);
        restore_settings(&path, left);
        assert_eq!(std::fs::read_to_string(&env).unwrap(), applied);
        // A `.env` of Copool's block alone goes with it.
        let only = with_block(None, "RELAY_KEY", "https://r").unwrap();
        fs_write(&env, &only);
        leave_api_key(&path, &mut record(None)).unwrap();
        assert!(!env.exists());
    }

    #[test]
    fn a_startup_file_loads_env_last_once_and_is_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let (rc, env) = (dir.path().join("zshrc"), dir.path().join(".env"));
        let lines = rc_lines(&env).unwrap();
        // Absent: created, then removed.
        let before = load_env_in(&rc, &env).unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(&rc).unwrap(), lines);
        assert_eq!(load_env_in(&rc, &env).unwrap(), None);
        unload_env_in(&rc, before);
        assert!(!rc.exists());
        for (text, separator) in [("", ""), ("a=1\n", "\n"), ("a=1", "\n\n")] {
            fs_write(&rc, text);
            let before = load_env_in(&rc, &env).unwrap().unwrap();
            assert_eq!(
                std::fs::read_to_string(&rc).unwrap(),
                format!("{text}{separator}{lines}")
            );
            unload_env_in(&rc, before);
            assert_eq!(std::fs::read_to_string(&rc).unwrap(), text);
        }
        // Loaded earlier but exported over since: loaded last again.
        fs_write(&rc, &format!("{lines}export GOOGLE_GEMINI_BASE_URL=x\n"));
        assert!(load_env_in(&rc, &env).unwrap().is_some());
        assert!(std::fs::read_to_string(&rc).unwrap().ends_with(&lines));
        assert_eq!(
            env_path(Path::new("/h/.gemini/antigravity-cli/settings.json")),
            Path::new("/h/.gemini/.env")
        );
    }

    fn fs_write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}
