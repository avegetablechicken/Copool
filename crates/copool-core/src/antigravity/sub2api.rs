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
//! `provider_registry.rs`; here is where the Antigravity pool keeps them and how it
//! uses one.
use super::{Result, Store};
use crate::{
    application::{Application, SwitchExecution},
    env_bridge::{self, Bridge, Expect},
    pool::Pool,
    provider_registry::{embeds_record, strip_suffix, upstream, Adapter, ProviderRecord, Route},
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

/// An Antigravity provider. Its address (`baseURL`) is the Sub2api address
/// agy sends to, as its root or its `/antigravity` gateway: its switch sets
/// `GOOGLE_GEMINI_BASE_URL` to the gateway. Through Coport it is Coport's,
/// with Sub2api's in its path
/// (`http://127.0.0.1:8787/antigravity/https://relay.example.com`), as
/// `ANTHROPIC_BASE_URL` names a Claude gateway through Coport; the admin API
/// is then derived from the Sub2api address in it, reached directly. Its
/// admin API override is never taken from `GOOGLE_GEMINI_BASE_URL`: the
/// administrator's password goes only where the form says.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Provider {
    #[serde(flatten)]
    pub record: ProviderRecord,
    /// The variable holding the Sub2api API key agy sends, as a new shell
    /// exports it: `GEMINI_API_KEY` itself unless named. Never the key.
    pub api_key_variable: String,
}
embeds_record!(Provider);
/// The Sub2api root of an address: without its `/antigravity` gateway.
fn root(url: &str) -> String {
    strip_suffix(url, &["/antigravity"])
}
/// agy's gateway at a provider, its `GOOGLE_GEMINI_BASE_URL`.
pub(super) fn gateway(p: &Provider) -> Result<String> {
    if p.base_url.is_empty() {
        return Err(CONFIGURATION_INCOMPLETE.into());
    }
    Ok(format!("{}/antigravity", p.base_url))
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
    env: env_bridge::Changes,
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
    // A block Copool cannot take out is left for the next switch to rewrite.
    let env = BRIDGE.remove(&env_path(path)).unwrap_or_default();
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
    left.env.undo();
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
pub(super) const BRIDGE: Bridge = Bridge {
    block_start: "# >>> Copool: agy's Sub2api provider (rewritten on each switch) >>>",
    rc_comment: "# Copool: agy's Sub2api key and gateway, from Gemini's .env.",
    rc_override: "COPOOL_ANTIGRAVITY_SHELL_RC",
};
/// The block's lines: agy's key from the provider's variable, expanded
/// where the `.env` is loaded, after the user's own exports, and its
/// gateway. `GEMINI_API_KEY` itself is the user's to export.
fn bridge_lines(variable: &str, gateway: &str) -> Result<Vec<String>> {
    if !crate::proxy_integration::is_variable(variable) {
        return Err(API_KEY_VARIABLE_INVALID.into());
    }
    if !quotable(gateway) {
        return Err("Invalid Sub2api URL".into());
    }
    let mut lines = Vec::new();
    if variable != API_KEY {
        lines.push(format!("{API_KEY}=\"${{{variable}}}\""));
    }
    lines.push(format!("{BASE_URL}='{gateway}'"));
    Ok(lines)
}

/// The Antigravity pool's providers, for its Provider registry.
pub(crate) struct ProviderAdapter<'a>(pub(crate) &'a Application);
impl Adapter for ProviderAdapter<'_> {
    type Provider = Provider;
    const POOL: Pool = Pool::Antigravity;
    fn load_providers(&self) -> Result<Vec<Provider>> {
        Ok(self.0.antigravity_load()?.providers)
    }
    async fn edit_providers<T: Send>(
        &self,
        edit: impl FnOnce(&mut Vec<Provider>) -> Result<T> + Send,
    ) -> Result<T> {
        self.0.antigravity_edit_providers(edit).await
    }
    fn prepare(&self, draft: &mut Provider) -> Result<()> {
        draft.base_url = root(&draft.base_url);
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
        Ok(())
    }
    fn route(&self, p: &Provider) -> Result<Route> {
        Ok(Route {
            gateway: gateway(p)?,
            sub2api: upstream(&p.base_url).to_owned(),
        })
    }
}

impl Application {
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
    /// Variables in agy's environment, as the user's shell exports them;
    /// with `now`, as one shell opened now does. Asked off the runtime, for
    /// the shell may take seconds.
    async fn antigravity_exported(&self, names: &[&str], now: bool) -> Result<Vec<Option<String>>> {
        let environment = self.antigravity.environment.read().unwrap().clone();
        env_bridge::exported(environment, names, now).await
    }
    /// Assigns agy's key and gateway in Copool's block of Gemini's `.env`,
    /// and has the shell's startup file load it when a new shell does not
    /// export them so. Takes all back when even then it does not.
    async fn antigravity_put_env(
        &self,
        settings: &Path,
        variable: &str,
        gateway: &str,
    ) -> Result<env_bridge::Changes> {
        let lines = bridge_lines(variable, gateway)?;
        let environment = self.antigravity.environment.read().unwrap().clone();
        BRIDGE
            .put(
                &env_path(settings),
                &lines,
                self.antigravity_shell_rc(),
                environment,
                &[
                    Expect::Address(BASE_URL, gateway),
                    Expect::Same(API_KEY, variable),
                ],
                ENV_NOT_LOADED,
            )
            .await
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
        // A variable nothing exports, or exports blank, would leave agy
        // without a key.
        let exported = self.antigravity_exported(&[variable], false).await?;
        if !exported[0]
            .as_deref()
            .is_some_and(|key| !key.trim().is_empty())
        {
            return Err(API_KEY_MISSING.into());
        }
        let path = self.antigravity_settings_path();
        // Outside the store's lock: the shell may take seconds.
        let changes = self
            .antigravity_put_env(&path, variable, &gateway(&provider)?)
            .await?;
        let applied = self.antigravity_apply_gemini(&path, &provider).await;
        if applied.is_err() {
            changes.undo();
        }
        applied?;
        Ok(SwitchExecution {
            provider: Some(provider.record.name),
            ..Default::default()
        })
    }
    /// `"modelProvider": "gemini"` in agy's settings, for the provider.
    async fn antigravity_apply_gemini(&self, path: &Path, provider: &Provider) -> Result<()> {
        let _lock = self.antigravity.lock.lock().await;
        let mut store = self.antigravity_load()?;
        // The `.env` was written outside the lock, for the provider as it was.
        let now = store
            .providers
            .iter()
            .find(|p| p.id == provider.id)
            .ok_or("Account not found")?;
        if (gateway(now)?, key_variable(now)) != (gateway(provider)?, key_variable(provider)) {
            return Err("The Antigravity provider changed during the switch; switch again".into());
        }
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
        store.settings_provider = Some(provider.id.clone());
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

    const BLOCK_START: &str = BRIDGE.block_start;
    const BLOCK_END: &str = "# <<< Copool <<<";
    /// `.env` with Copool's block for the variable and gateway, as a switch
    /// writes it.
    fn with_block(text: Option<&str>, variable: &str, gateway: &str) -> Result<String> {
        Ok(BRIDGE.with_block(text, &bridge_lines(variable, gateway)?))
    }

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
                assert_eq!(previous.env, env_bridge::Changes::default());
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
                record: ProviderRecord {
                    base_url: root(&raw),
                    ..Default::default()
                },
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
    fn geminis_env_is_two_levels_above_agys_settings() {
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
