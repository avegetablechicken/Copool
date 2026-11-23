//! Sub2api in the Grok pool: its Grok OAuth accounts as cards, and Grok
//! Build pointed at its OpenAI-compatible gateway (`<Sub2api>/v1`).
//!
//! A switch sets Grok Build's session endpoints (`cli_chat_proxy_base_url` and
//! `xai_api_base_url`) and `[auth] preferred_method = "api_key"` in
//! `config.toml`, recording just the TOML fields it changed or removed so the
//! next provider or the official account can restore them, including configs
//! written by older Copool. When the provider uses a variable other than
//! `XAI_API_KEY`, Copool bridges `XAI_API_KEY="${<variable>}"` in `~/.grok/.env`
//! (loaded from `~/.zshrc` or `~/.bashrc`), and removes that block on leaving.
mod config;
use super::{api, Result, Store};
use crate::{
    application::{Application, SwitchExecution},
    codex_config::Definition,
    pool::Pool,
    sub2api::{self, Account, Member},
};
use config::{Layout, XAI_KEY};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table};

pub(crate) const API_KEY_MISSING: &str = "error.grok.sub2api_api_key_missing";
pub(crate) const API_KEY_VARIABLE_INVALID: &str = "error.grok.api_key_variable_invalid";
pub(crate) const ENV_NOT_LOADED: &str = "error.grok.xai_env_not_loaded";
pub(crate) const CONFIG_INVALID: &str = "error.grok.config_invalid";
const CONFIGURATION_INCOMPLETE: &str = "error.sub2api.configuration_incomplete";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    /// The Sub2api address, its root or its `/v1` gateway, which Grok Build
    /// sends to: a switch names `<root>/v1`. Through Coport it is Coport's,
    /// with Sub2api's in its path; the admin API is then derived from the
    /// Sub2api address in it, reached directly.
    #[serde(rename = "baseURL")]
    pub base_url: String,
    /// Optional admin API override, independent of Grok Build's gateway.
    #[serde(rename = "adminBaseURL")]
    pub admin_base_url: String,
    pub username: String,
    /// Only in a draft from the form; the system keyring keeps it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub password: String,
    /// The variable holding the Sub2api API key Grok Build sends, as a new
    /// shell exports it. The user supplies the variable name.
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
/// The Sub2api root of an address: without its `/v1` gateway.
fn root(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    url.strip_suffix("/v1")
        .unwrap_or(url)
        .trim_end_matches('/')
        .to_owned()
}
/// Grok Build's gateway at a provider.
pub(super) fn gateway(p: &Provider) -> Result<String> {
    if p.base_url.is_empty() {
        return Err(CONFIGURATION_INCOMPLETE.into());
    }
    Ok(format!("{}/v1", p.base_url))
}
/// The Sub2api address in an address: the upstream a proxy's path names
/// (`<proxy>/https://relay`), else the address itself.
fn upstream(url: &str) -> &str {
    ["/https://", "/http://"]
        .iter()
        .filter_map(|scheme| url.find(scheme))
        .min()
        .map_or(url, |at| &url[at + 1..])
}
/// The variable holding a provider's key.
fn key_variable(p: &Provider) -> Result<&str> {
    let name = p.api_key_variable.trim();
    if crate::proxy_integration::is_variable(name) {
        Ok(name)
    } else {
        Err(API_KEY_VARIABLE_INVALID.into())
    }
}
/// A Grok provider's cards, named by its id.
fn member(p: &Provider) -> Member {
    Member {
        key: p.id.clone(),
        name: p.name.clone(),
        proxy_url: p.proxy_url.clone(),
        imported: p.imported_account_ids.clone(),
        cached: p.cached_accounts.clone(),
    }
}

/// What a provider's switch replaced in Grok Build's `config.toml`, until an
/// account's switch puts it back.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct ConfigRecord {
    /// The gateway the session was last pointed at.
    gateway: String,
    /// Whether there was no `config.toml` before.
    created: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    models: Vec<ModelRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    layout: Option<Layout>,
}
/// Legacy per-model record from earlier Copool versions, kept so switching
/// away cleanly restores older configs.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
struct ModelRecord {
    id: String,
    created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    env_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backend: Option<BackendOverride>,
}

/// Old switches did not manage the backend. Once managed, remember
/// whether an explicit backend existed before switching.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct BackendOverride {
    previous: Option<String>,
}

fn read_text(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
/// Writes the file, keeping a symlink and the file's permissions; none
/// removes it.
fn put_text(path: &Path, text: Option<&str>) -> Result<()> {
    match text {
        Some(text) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(path, text).map_err(|e| e.to_string())
        }
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        },
    }
}
fn parse(text: Option<&str>) -> Result<DocumentMut> {
    text.unwrap_or_default()
        .parse::<DocumentMut>()
        .map_err(|_| CONFIG_INVALID.into())
}
/// A TOML value as text, to be put back as it was.
fn value_text(item: Option<&Item>) -> Option<String> {
    item.and_then(Item::as_value)
        .map(|value| value.clone().decorated("", "").to_string())
}
fn put_value(table: &mut Table, key: &str, text: Option<&str>) {
    match text.and_then(|text| text.parse::<toml_edit::Value>().ok()) {
        Some(value) => {
            table.insert(key, Item::Value(value));
        }
        None => {
            table.remove(key);
        }
    }
}

/// Grok's `.env`, beside `config.toml` (`~/.grok/.env`).
pub(super) fn env_path(config: &Path) -> PathBuf {
    config.with_file_name(".env")
}
/// Copool's block in `~/.grok/.env`: bridges a provider's custom variable to
/// `XAI_API_KEY` for Grok Build's session-wide API-key mode.
const BLOCK_START: &str =
    "# >>> Copool: Grok Build's Sub2api provider (rewritten on each switch) >>>";
const BLOCK_END: &str = "# <<< Copool <<<";
fn has_block(text: &str) -> bool {
    text.lines().any(|line| line.trim_end() == BLOCK_START)
}
pub(super) fn env_has_block(text: Option<&str>) -> bool {
    text.is_some_and(has_block)
}
pub(super) fn env_bridges_variable(text: Option<&str>, variable: &str) -> bool {
    let Some(text) = text else {
        return false;
    };
    let expected = format!("{XAI_KEY}=\"${{{variable}}}\"");
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed == BLOCK_START {
            inside = true;
            continue;
        }
        if trimmed == BLOCK_END {
            inside = false;
            continue;
        }
        if inside && trimmed == expected {
            return true;
        }
    }
    false
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
/// `.env` with Copool's block at its end, assigning `XAI_API_KEY` from the
/// provider's variable, in the file's newlines.
fn with_block(text: Option<&str>, variable: &str) -> Result<String> {
    if !crate::proxy_integration::is_variable(variable) {
        return Err(API_KEY_VARIABLE_INVALID.into());
    }
    let lines = [
        BLOCK_START.to_owned(),
        format!("{XAI_KEY}=\"${{{variable}}}\""),
        BLOCK_END.to_owned(),
    ];
    let text = without_block(text.unwrap_or(""));
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let separator = if text.is_empty() {
        String::new()
    } else if text.ends_with('\n') {
        newline.to_owned()
    } else {
        newline.repeat(2)
    };
    Ok(format!("{text}{separator}{}{newline}", lines.join(newline)))
}
/// The lines a shell's startup file loads Grok's `.env` with, exporting
/// `XAI_API_KEY` from the provider's variable; skipped under `OWN_EXPORTS_ONLY`.
fn rc_lines(env: &Path) -> Result<String> {
    let env = env
        .to_str()
        .filter(|p| env.is_absolute() && !p.contains('\'') && !p.contains(char::is_control))
        .ok_or("Invalid .env path")?;
    let own = crate::proxy_integration::OWN_EXPORTS_ONLY;
    Ok(format!(
        "# Copool: Grok Build's Sub2api key, from Grok's .env.\n\
         [ -z \"${{{own}:-}}\" ] && [ -f '{env}' ] && {{ set -a; . '{env}'; set +a; }}\n"
    ))
}
#[derive(Debug, PartialEq)]
struct Appended {
    before: Option<u64>,
    text: String,
}
fn load_env_in(rc: &Path, env: &Path) -> Result<Option<Appended>> {
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
    let text = format!("{separator}{lines}");
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(rc)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|e| e.to_string())?;
    Ok(Some(Appended { before, text }))
}
fn unload_env_in(rc: &Path, appended: &Appended) {
    let Ok(Some(now)) = read_text(rc) else {
        return;
    };
    let before = appended.before.unwrap_or(0);
    if now.len() as u64 != before + appended.text.len() as u64 || !now.ends_with(&appended.text) {
        return;
    }
    let _ = match appended.before {
        None => std::fs::remove_file(rc),
        Some(len) => std::fs::OpenOptions::new()
            .write(true)
            .open(rc)
            .and_then(|file| file.set_len(len)),
    };
}
/// What a provider's switch changed outside `config.toml`, to take back
/// when a later step fails.
struct EnvChanges {
    env: Option<(PathBuf, Option<String>)>,
    rc: Option<(PathBuf, Appended)>,
}
impl EnvChanges {
    fn undo(&self) {
        if let Some((rc, appended)) = &self.rc {
            unload_env_in(rc, appended);
        }
        if let Some((env, previous)) = &self.env {
            let _ = put_text(env, previous.as_deref());
        }
    }
}

fn apply(
    doc: &mut DocumentMut,
    record: &mut ConfigRecord,
    gateway: &str,
    variable: &str,
) -> Result<()> {
    restore(doc, record)?;
    record.layout = Some(Layout::apply(doc, gateway, variable)?);
    record.models.clear();
    record.gateway = gateway.into();
    Ok(())
}

/// Puts back what the switch replaced, including older per-model layouts.
fn restore(doc: &mut DocumentMut, record: &ConfigRecord) -> Result<()> {
    if let Some(layout) = &record.layout {
        return layout.restore(doc);
    }
    let ours: Vec<&ModelRecord> = record
        .models
        .iter()
        .filter(|m| on_gateway(doc, &record.gateway, &m.id))
        .collect();
    if ours.is_empty() {
        return Ok(());
    }
    let Some(table) = doc.get_mut("model").and_then(Item::as_table_mut) else {
        return Ok(());
    };
    for entry in ours {
        let Some(model) = table.get_mut(&entry.id).and_then(Item::as_table_mut) else {
            continue;
        };
        put_value(model, "base_url", entry.base_url.as_deref());
        put_value(model, "env_key", entry.env_key.as_deref());
        if let Some(backend) = &entry.backend {
            if model.get("api_backend").and_then(Item::as_str) == Some("responses") {
                put_value(model, "api_backend", backend.previous.as_deref());
            }
        }
        if entry.created && model.is_empty() {
            table.remove(&entry.id);
        }
    }
    if table.is_empty() {
        doc.remove("model");
    }
    Ok(())
}
/// Whether a legacy model's `base_url` is the gateway.
fn on_gateway(doc: &DocumentMut, gateway: &str, id: &str) -> bool {
    doc.get("model")
        .and_then(|t| t.get(id))
        .and_then(|t| t.get("base_url"))
        .and_then(Item::as_str)
        == Some(gateway)
}
/// Whether Grok Build still uses the recorded gateway.
fn uses_gateway(doc: &DocumentMut, record: &ConfigRecord) -> bool {
    if let Some(layout) = &record.layout {
        return layout.active(doc, &record.gateway);
    }
    record
        .models
        .iter()
        .any(|m| on_gateway(doc, &record.gateway, &m.id))
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CurrentRoute<'a> {
    Login,
    Provider(&'a str),
    // No card matches the applied route, or the recorded config cannot be read.
    Unmatched,
}

pub(super) fn current_route<'a>(path: &Path, store: &'a Store) -> CurrentRoute<'a> {
    let Some(record) = &store.config_record else {
        return CurrentRoute::Login;
    };
    let doc = match read_text(path).and_then(|text| parse(text.as_deref())) {
        Ok(doc) => doc,
        Err(_) => return CurrentRoute::Unmatched,
    };
    if !uses_gateway(&doc, record) {
        return CurrentRoute::Login;
    }
    let Some(provider) = store
        .providers
        .iter()
        .find(|p| Some(p.id.as_str()) == store.config_provider.as_deref())
    else {
        return CurrentRoute::Unmatched;
    };
    if gateway(provider).as_deref() != Ok(record.gateway.as_str()) {
        return CurrentRoute::Unmatched;
    }
    let Ok(variable) = key_variable(provider) else {
        return CurrentRoute::Unmatched;
    };
    let env_text = read_text(&env_path(path)).ok().flatten();
    let matches_key = match &record.layout {
        Some(layout) => {
            layout.matches_provider(&doc, env_text.as_deref(), &record.gateway, variable)
        }
        None => record.models.iter().any(|m| {
            on_gateway(&doc, &record.gateway, &m.id)
                && doc["model"][&m.id].get("env_key").and_then(Item::as_str) == Some(variable)
        }),
    };
    if matches_key {
        CurrentRoute::Provider(&provider.id)
    } else {
        CurrentRoute::Unmatched
    }
}
/// What leaving the gateway changed, to be undone on a failed switch.
pub(super) struct Left {
    text: Option<String>,
    env: Option<(PathBuf, Option<String>)>,
    record: Option<ConfigRecord>,
    provider: Option<String>,
}
/// Takes Grok Build off the gateway before an account's switch: what the
/// provider replaced goes back, Copool's block in `~/.grok/.env` is removed,
/// and the record is cleared.
pub(super) fn leave_gateway(path: &Path, store: &mut Store) -> Result<Left> {
    let text = read_text(path)?;
    let env = env_path(path);
    let env_changed = match read_text(&env) {
        Ok(Some(env_text)) if has_block(&env_text) => {
            let left = without_block(&env_text);
            put_text(&env, (!left.trim().is_empty()).then_some(&*left))
                .ok()
                .map(|()| (env, Some(env_text)))
        }
        _ => None,
    };
    let left = Left {
        text: text.clone(),
        env: env_changed,
        record: store.config_record.take(),
        provider: store.config_provider.take(),
    };
    let Some(record) = left.record.as_ref() else {
        return Ok(left);
    };
    // A file the user broke since is left for them to fix.
    let Ok(mut doc) = parse(text.as_deref()) else {
        return Ok(left);
    };
    restore(&mut doc, record)?;
    let rendered = doc.to_string();
    let rendered = (!(record.created && rendered.trim().is_empty())).then_some(rendered);
    if rendered != text {
        put_text(path, rendered.as_deref())?;
    }
    Ok(left)
}
/// Undoes `leave_gateway` after a failed switch.
pub(super) fn restore_left(path: &Path, store: &mut Store, left: Left) {
    if left.record.is_some() {
        let _ = put_text(path, left.text.as_deref());
    }
    if let Some((env, previous)) = left.env {
        let _ = put_text(&env, previous.as_deref());
    }
    store.config_record = left.record;
    store.config_provider = left.provider;
}

impl Application {
    /// The Grok providers for the settings, without their secrets.
    pub(crate) fn grok_providers(&self) -> Result<Vec<Provider>> {
        Ok(self.grok_load()?.providers)
    }
    pub(crate) async fn grok_save_provider(&self, mut draft: Provider) -> Result<Provider> {
        draft.name = draft.name.trim().into();
        draft.username = draft.username.trim().into();
        draft.base_url = root(&draft.base_url);
        draft.admin_base_url = draft.admin_base_url.trim().trim_end_matches('/').into();
        draft.proxy_url = draft.proxy_url.trim().into();
        // Grok Build's gateway is the Sub2api URL; the admin API only
        // overrides where Copool asks for the accounts.
        if draft.base_url.is_empty() {
            return Err(CONFIGURATION_INCOMPLETE.into());
        }
        draft.api_key_variable = key_variable(&draft)?.to_owned();
        if draft.id.trim().is_empty() {
            draft.id = uuid::Uuid::new_v4().to_string();
        }
        self.grok_edit_providers(|providers| {
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
            self.store_provider_password(Pool::Grok, &draft.id, &draft.password)?;
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
    pub(crate) async fn grok_remove_provider(&self, id: &str) -> Result<()> {
        self.grok_edit_providers(|providers| {
            providers.retain(|p| p.id != id);
            Ok(())
        })
        .await
    }
    /// Edits the providers alone; the logins stay as stored.
    async fn grok_edit_providers<T>(
        &self,
        edit: impl FnOnce(&mut Vec<Provider>) -> Result<T>,
    ) -> Result<T> {
        let _lock = self.grok.lock.lock().await;
        let mut store = self.grok_load()?;
        let result = edit(&mut store.providers)?;
        self.grok_save(&store)?;
        Ok(result)
    }
    pub(crate) fn grok_members(&self) -> Result<Vec<Member>> {
        Ok(self.grok_load()?.providers.iter().map(member).collect())
    }
    pub(crate) async fn grok_edit_member<T>(
        &self,
        key: &str,
        edit: impl FnOnce(&mut Member) -> T,
    ) -> Result<Option<T>> {
        self.grok_edit_providers(|providers| {
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
    pub(crate) fn grok_member_connection(&self, key: &str) -> Result<sub2api::Connection> {
        let p = self
            .grok_load()?
            .providers
            .into_iter()
            .find(|p| p.id.eq_ignore_ascii_case(key))
            .ok_or("Provider not found")?;
        let password = self
            .secrets
            .get(&sub2api::password_secret(Pool::Grok, &p.id))?
            .ok_or(CONFIGURATION_INCOMPLETE)?;
        // Sub2api itself, not a proxy in front of Grok Build's requests.
        let admin = if p.admin_base_url.is_empty() {
            upstream(&p.base_url)
        } else {
            &p.admin_base_url
        };
        Ok(sub2api::Connection {
            admin_base: sub2api::admin_base(admin)?,
            definition: Definition {
                id: p.name.clone(),
                base_url: Some(gateway(&p)?),
                wire_api: None,
                env_key: key_variable(&p).ok().map(str::to_owned),
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
    /// Variables in Grok Build's environment: with `now`, as a new shell
    /// exports them (including `~/.grok/.env`); otherwise the user's own
    /// exports alone (`OWN_EXPORTS_ONLY`). Asked off the runtime, for the
    /// shell may take seconds.
    async fn grok_exported(&self, names: &[&str], now: bool) -> Result<Vec<Option<String>>> {
        let environment = self.grok.environment.read().unwrap().clone();
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
    /// Whether a new shell exports `XAI_API_KEY` with the provider variable's
    /// value.
    async fn grok_env_loaded(&self, variable: &str) -> Result<bool> {
        let values = self.grok_exported(&[XAI_KEY, variable], true).await?;
        Ok(values[0].is_some() && values[0] == values[1])
    }
    /// Bridges a custom provider variable to `XAI_API_KEY` in `~/.grok/.env`
    /// (loading it from `~/.zshrc` or `~/.bashrc` when needed), or clears any
    /// previous bridge when the provider uses `XAI_API_KEY` directly.
    async fn grok_put_env(&self, config: &Path, variable: &str) -> Result<EnvChanges> {
        let env = env_path(config);
        let env_before = read_text(&env)?;
        if variable == XAI_KEY {
            if env_before.as_deref().is_some_and(has_block) {
                let left = without_block(env_before.as_deref().unwrap_or(""));
                put_text(&env, (!left.trim().is_empty()).then_some(&*left))?;
                return Ok(EnvChanges {
                    env: Some((env, env_before)),
                    rc: None,
                });
            }
            return Ok(EnvChanges {
                env: None,
                rc: None,
            });
        }
        put_text(&env, Some(&with_block(env_before.as_deref(), variable)?))?;
        let mut changes = EnvChanges {
            env: Some((env.clone(), env_before)),
            rc: None,
        };
        let mut loaded = self.grok_env_loaded(variable).await;
        if let (Ok(false), Some(rc)) = (&loaded, self.grok_shell_rc()) {
            loaded = match load_env_in(&rc, &env) {
                Ok(Some(before)) => {
                    changes.rc = Some((rc, before));
                    self.grok_env_loaded(variable).await
                }
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
    /// Puts Grok Build's session on the provider's gateway: sets
    /// `cli_chat_proxy_base_url` and `xai_api_base_url` and pins
    /// `[auth] preferred_method = "api_key"` in `config.toml`, bridging any
    /// custom key variable to `XAI_API_KEY` in `~/.grok/.env`; the login in
    /// `auth.json` stays, for an account's switch to return to. Called under
    /// the pool's operations lock.
    pub(super) async fn grok_switch_sub2api(&self, id: &str) -> Result<SwitchExecution> {
        let (member, account) = self.card(Pool::Grok, id)?;
        if !account.status.eq_ignore_ascii_case("active") {
            return Err("Account is deactivated".into());
        }
        let provider = self
            .grok_load()?
            .providers
            .into_iter()
            .find(|p| p.id == member.key)
            .ok_or("Account not found")?;
        let variable = key_variable(&provider)?;
        // A variable nothing exports would leave Grok Build without a key.
        let key = self
            .grok_exported(&[variable], false)
            .await?
            .into_iter()
            .next()
            .flatten()
            .filter(|key| !key.trim().is_empty())
            .ok_or(API_KEY_MISSING)?;
        let gateway = gateway(&provider)?;
        // Check the gateway with the key Grok Build will send: a key Sub2api
        // refuses is refused here, before Grok Build is changed.
        let (status, body) = self
            .grok_network()
            .gateway_models(
                &format!("{gateway}/models"),
                &key,
                &provider.proxy_url,
                provider.allow_insecure_tls,
            )
            .await?;
        if !(200..300).contains(&status) {
            return Err(api::status_error("Sub2api models request", status, &body));
        }
        let path = self.grok_config_path();
        // Outside the store's lock: probing a new shell may take seconds.
        let env_changes = self.grok_put_env(&path, variable).await?;
        let applied = self
            .grok_apply_config(&path, &provider.id, &member.key, &gateway, variable)
            .await;
        if applied.is_err() {
            env_changes.undo();
        }
        applied?;
        Ok(SwitchExecution {
            provider: Some(provider.name),
            ..Default::default()
        })
    }
    async fn grok_apply_config(
        &self,
        path: &Path,
        provider_id: &str,
        member_key: &str,
        gateway: &str,
        variable: &str,
    ) -> Result<()> {
        let _lock = self.grok.lock.lock().await;
        let mut store = self.grok_load()?;
        // The key, gateway and `.env` were prepared outside the lock.
        let now = store
            .providers
            .iter()
            .find(|p| p.id == provider_id)
            .ok_or("Account not found")?;
        if (self::gateway(now)?.as_str(), key_variable(now)?) != (gateway, variable) {
            return Err("The Grok provider changed during the switch; switch again".into());
        }
        let text = read_text(path)?;
        let mut doc = parse(text.as_deref())?;
        // Between providers, the record of the login's configuration stays.
        let mut record = store.config_record.clone().unwrap_or(ConfigRecord {
            created: text.is_none(),
            ..Default::default()
        });
        apply(&mut doc, &mut record, gateway, variable)?;
        put_text(path, Some(&doc.to_string()))?;
        store.config_record = Some(record);
        store.config_provider = Some(member_key.to_owned());
        if let Err(error) = self.grok_save(&store) {
            let _ = put_text(path, text.as_deref());
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gateway_is_the_root_with_v1_and_admin_is_the_sub2api_in_it() {
        for (typed, saved) in [
            (
                "https://relay.example.com:6060/",
                "https://relay.example.com:6060",
            ),
            (
                "https://relay.example.com:6060/v1",
                "https://relay.example.com:6060",
            ),
            (
                "https://relay.example.com:6060/v1/",
                "https://relay.example.com:6060",
            ),
        ] {
            assert_eq!(root(typed), saved);
        }
        let provider = Provider {
            base_url: "http://127.0.0.1:8787/https://relay.example.com".into(),
            ..Default::default()
        };
        assert_eq!(
            gateway(&provider).unwrap(),
            "http://127.0.0.1:8787/https://relay.example.com/v1"
        );
        assert_eq!(upstream(&provider.base_url), "https://relay.example.com");
        assert_eq!(
            key_variable(&provider).unwrap_err(),
            API_KEY_VARIABLE_INVALID
        );
        assert!(gateway(&Provider::default()).is_err());
    }

    #[test]
    fn a_switch_sets_session_endpoints_and_leaving_puts_back_what_it_had() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let env = env_path(&path);
        let before = "# the user's\n[features]\ntelemetry = false\n\n\
            [endpoints]\ncli_chat_proxy_base_url = 'http://127.0.0.1:8787/grok/v1'\nxai_api_base_url = 'http://127.0.0.1:8787/grok/v1' # Coport\n\n\
            [model.\"grok-4.6\"]\ntemperature = 0.5\n\n\
            [model.mine]\nmodel = \"local\"\nbase_url = \"http://localhost:11434/v1\"\n";
        put_text(&path, Some(before)).unwrap();
        let mut store = Store::default();
        let mut doc = parse(Some(before)).unwrap();
        let mut record = ConfigRecord::default();
        apply(
            &mut doc,
            &mut record,
            "https://relay.example.com/v1",
            "GROK_SUB2API_API_KEY",
        )
        .unwrap();
        let text = doc.to_string();
        let switched: DocumentMut = text.parse().unwrap();
        assert_eq!(
            switched["endpoints"]["cli_chat_proxy_base_url"].as_str(),
            Some("https://relay.example.com/v1")
        );
        assert_eq!(
            switched["endpoints"]["xai_api_base_url"].as_str(),
            Some("https://relay.example.com/v1")
        );
        assert!(switched["endpoints"].get("models_base_url").is_none());
        assert_eq!(
            switched["auth"]["preferred_method"].as_str(),
            Some("api_key")
        );
        // The user's own model tables stay untouched.
        assert_eq!(
            switched["model"]["mine"]["base_url"].as_str(),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(
            switched["model"]["grok-4.6"]["temperature"].as_float(),
            Some(0.5)
        );
        assert!(switched["model"]["grok-4.6"].get("base_url").is_none());
        assert!(text.starts_with("# the user's\n[features]"));
        put_text(&path, Some(&text)).unwrap();
        put_text(
            &env,
            Some(&with_block(None, "GROK_SUB2API_API_KEY").unwrap()),
        )
        .unwrap();
        store.config_record = Some(record.clone());
        store.config_provider = Some("p".into());
        store.providers.push(Provider {
            id: "p".into(),
            base_url: "https://relay.example.com".into(),
            api_key_variable: "GROK_SUB2API_API_KEY".into(),
            ..Provider::default()
        });
        assert_eq!(current_route(&path, &store), CurrentRoute::Provider("p"));
        store.providers[0].api_key_variable = "DIFFERENT".into();
        assert_eq!(current_route(&path, &store), CurrentRoute::Unmatched);

        // Another provider keeps the record of the login's configuration.
        apply(
            &mut doc,
            &mut record,
            "https://other.example.com/v1",
            XAI_KEY,
        )
        .unwrap();
        let other: DocumentMut = doc.to_string().parse().unwrap();
        assert_eq!(
            other["endpoints"]["cli_chat_proxy_base_url"].as_str(),
            Some("https://other.example.com/v1")
        );
        assert_eq!(
            other["endpoints"]["xai_api_base_url"].as_str(),
            Some("https://other.example.com/v1")
        );
        put_text(&path, Some(&doc.to_string())).unwrap();
        put_text(&env, None).unwrap();
        store.config_record = Some(record);
        store.providers[0].base_url = "https://other.example.com".into();
        store.providers[0].api_key_variable = XAI_KEY.into();
        assert_eq!(current_route(&path, &store), CurrentRoute::Provider("p"));

        // Put a custom-var .env block back to verify leave_gateway cleans it up too.
        put_text(
            &env,
            Some(&with_block(Some("KEEP=1\n"), "OTHER_KEY").unwrap()),
        )
        .unwrap();
        let left = leave_gateway(&path, &mut store).unwrap();
        assert_eq!(read_text(&path).unwrap().as_deref(), Some(before));
        assert_eq!(read_text(&env).unwrap().as_deref(), Some("KEEP=1\n"));
        assert!(store.config_record.is_none() && store.config_provider.is_none());
        assert_eq!(current_route(&path, &store), CurrentRoute::Login);
        // A failed account switch takes Grok Build back to the gateway.
        restore_left(&path, &mut store, left);
        assert!(env_bridges_variable(
            read_text(&env).unwrap().as_deref(),
            "OTHER_KEY"
        ));
    }

    #[test]
    fn migrating_legacy_per_model_and_models_base_url_layouts_restores_user_settings() {
        let before = "# custom endpoints\n[endpoints]\nmodels_base_url = 'https://example.invalid' # keep this\n";
        let mut file = before.to_owned();
        let gateway = "https://relay.example/v1";
        let listed = ["grok-4.5", "grok-build-0.1", "grok-4.3"];
        let mut record = ConfigRecord {
            gateway: gateway.to_owned(),
            ..Default::default()
        };
        for id in listed {
            file.push_str(&format!(
                "\n[model.\"{id}\"]\nbase_url = \"{gateway}\"\nenv_key = \"K\"\napi_backend = \"responses\"\n"
            ));
            record.models.push(ModelRecord {
                id: id.into(),
                created: true,
                backend: Some(BackendOverride { previous: None }),
                ..Default::default()
            });
        }
        let mut doc = parse(Some(&file)).unwrap();
        for variable in [XAI_KEY, "MY_GROK_KEY", XAI_KEY] {
            apply(&mut doc, &mut record, gateway, variable).unwrap();
            assert!(record.models.is_empty());
            assert!(uses_gateway(&doc, &record));
            assert!(doc.get("model").is_none());
            assert!(doc.get("models").is_none());
            assert!(doc["endpoints"].get("models_base_url").is_none());
            assert!(doc["endpoints"].get("models_list_url").is_none());
            assert_eq!(
                doc["endpoints"]["cli_chat_proxy_base_url"].as_str(),
                Some(gateway)
            );
            assert_eq!(doc["endpoints"]["xai_api_base_url"].as_str(), Some(gateway));
            assert_eq!(doc["auth"]["preferred_method"].as_str(), Some("api_key"));
            // Read both files back as the next switch does.
            doc = parse(Some(&doc.to_string())).unwrap();
            record = serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
        }
        restore(&mut doc, &record).unwrap();
        assert_eq!(doc.to_string(), before);
    }

    #[test]
    fn reapplying_the_four_model_layout_removes_model_blocks_and_keeps_user_edits() {
        let relay = "https://relay.invalid/v1";
        let mut template = format!(
            "[model_providers.copool-sub2api]\nbase_url = '{relay}'\n\
             [models]\ndefault = 'grok-c'\nweb_search = 'grok-c'\n\
             [session]\nauto_compact_threshold_percent = 80\n"
        );
        for id in ["grok-a", "grok-b", "grok-c", "grok-d"] {
            template += &format!(
                "[model.'{id}']\nmodel_provider = 'copool-sub2api'\nenv_key = 'TEAM_KEY'\napi_backend = 'responses'\n"
            );
        }
        let old: ConfigRecord = serde_json::from_value(serde_json::json!({
            "gateway": relay, "layout": {"provider":"copool-sub2api","applied":template,"previous":""}
        }))
        .unwrap();
        for variable in ["TEAM_KEY", XAI_KEY] {
            let mut doc = parse(Some(&template)).unwrap();
            doc["model"]["grok-a"]["temperature"] = toml_edit::value(0.3);
            let mut record = old.clone();
            apply(&mut doc, &mut record, relay, variable).unwrap();
            assert!(doc.get("session").is_none());
            assert!(doc.get("model_providers").is_none());
            assert!(doc.get("models").is_none());
            let tables = doc["model"].as_table().unwrap();
            assert_eq!(tables.len(), 1); // only the user's temperature
            assert_eq!(tables["grok-a"].as_table().unwrap().len(), 1);
            assert_eq!(
                doc["endpoints"]["cli_chat_proxy_base_url"].as_str(),
                Some(relay)
            );
            assert_eq!(doc["endpoints"]["xai_api_base_url"].as_str(), Some(relay));
            assert_eq!(doc["auth"]["preferred_method"].as_str(), Some("api_key"));
            restore(&mut doc, &record).unwrap();
            assert_eq!(doc["model"]["grok-a"]["temperature"].as_float(), Some(0.3));
            assert!(doc.get("endpoints").is_none());
            assert!(doc.get("auth").is_none());
        }
    }

    #[test]
    fn env_block_and_shell_rc_loader_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let env = dir.path().join(".env");
        let rc = dir.path().join(".zshrc");
        let existing = "FOO=bar\n";
        let written = with_block(Some(existing), "GROK_SUB2API_API_KEY").unwrap();
        assert!(env_bridges_variable(Some(&written), "GROK_SUB2API_API_KEY"));
        assert!(!env_bridges_variable(Some(&written), "OTHER_KEY"));
        assert_eq!(without_block(&written), existing);

        put_text(&rc, Some("export GROK_SUB2API_API_KEY=secret\n")).unwrap();
        let appended = load_env_in(&rc, &env).unwrap().unwrap();
        assert!(load_env_in(&rc, &env).unwrap().is_none());
        unload_env_in(&rc, &appended);
        assert_eq!(
            read_text(&rc).unwrap().as_deref(),
            Some("export GROK_SUB2API_API_KEY=secret\n")
        );
    }

    #[test]
    fn leaving_removes_a_file_the_switch_created_and_keeps_what_the_user_added() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut doc = parse(None).unwrap();
        let mut record = ConfigRecord {
            created: true,
            ..Default::default()
        };
        apply(&mut doc, &mut record, "https://r/v1", XAI_KEY).unwrap();
        put_text(&path, Some(&doc.to_string())).unwrap();
        let mut store = Store {
            config_record: Some(record.clone()),
            ..Default::default()
        };
        leave_gateway(&path, &mut store).unwrap();
        assert!(!path.exists());

        // The user tuned a model meanwhile: that stays.
        apply(&mut doc, &mut record, "https://r/v1", XAI_KEY).unwrap();
        doc["model"]["grok-4.7"]["temperature"] = toml_edit::value(0.2);
        put_text(&path, Some(&doc.to_string())).unwrap();
        store.config_record = Some(record);
        leave_gateway(&path, &mut store).unwrap();
        let left: DocumentMut = read_text(&path).unwrap().unwrap().parse().unwrap();
        assert_eq!(
            left["model"]["grok-4.7"]["temperature"].as_float(),
            Some(0.2)
        );
        assert!(left.get("endpoints").is_none());
        assert!(left.get("auth").is_none());
        // A configuration the user broke is left as it is.
        put_text(&path, Some("[model\n")).unwrap();
        store.config_record = Some(ConfigRecord {
            gateway: "https://r/v1".into(),
            models: vec![ModelRecord {
                id: "grok-4.7".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        leave_gateway(&path, &mut store).unwrap();
        assert_eq!(read_text(&path).unwrap().as_deref(), Some("[model\n"));
    }
}
