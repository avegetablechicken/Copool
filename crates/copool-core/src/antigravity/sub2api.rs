//! Sub2api in the Antigravity pool: its Antigravity OAuth accounts as cards,
//! and agy pointed at its `/antigravity` gateway by `"modelProvider":
//! "gemini"` in agy's settings. agy then sends `GEMINI_API_KEY` to
//! `GOOGLE_GEMINI_BASE_URL`, both read from its environment only: as for a
//! Codex provider's `env_key`, Copool never stores or writes the key, and
//! only checks that both are exported before it switches. What the pools do
//! alike with their providers is in `providers.rs`; here is where the
//! Antigravity pool keeps them and how it uses one.
use super::{Result, Store};
use crate::{
    application::{Application, SwitchExecution},
    codex_config::Definition,
    pool::Pool,
    sub2api::{self, Account, Member},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::Path;

/// agy's switch from its Google login to the Gemini API.
const MODEL_PROVIDER: &str = "modelProvider";
const GEMINI: &str = "gemini";
/// What agy reads in Gemini API mode, from its environment only.
pub(super) const API_KEY: &str = "GEMINI_API_KEY";
pub(super) const BASE_URL: &str = "GOOGLE_GEMINI_BASE_URL";
pub(crate) const API_KEY_MISSING: &str = "error.antigravity.gemini_api_key_missing";
pub(crate) const BASE_URL_MISSING: &str = "error.antigravity.gemini_base_url_missing";
pub(crate) const SETTINGS_INVALID: &str = "error.antigravity.settings_invalid";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    /// The Sub2api address: its root, or the `/antigravity` gateway agy's
    /// `GOOGLE_GEMINI_BASE_URL` names. Never taken from that variable, which
    /// may name a local proxy instead: the administrator's password goes
    /// only where the form says.
    #[serde(rename = "baseURL")]
    pub base_url: String,
    /// Optional admin API override, independent of agy's gateway.
    #[serde(rename = "adminBaseURL")]
    pub admin_base_url: String,
    pub username: String,
    /// Only in a draft from the form; the system keyring keeps it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub password: String,
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

/// agy's settings as text; none when there are none.
fn read_settings(path: &Path) -> Result<Option<String>> {
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
/// Writes the settings verbatim, or removes them for none.
fn put_settings(path: &Path, text: Option<&str>) -> Result<()> {
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
    read_settings(path)
        .ok()
        .and_then(|text| parse_settings(text.as_deref()).ok())
        .is_some_and(|settings| uses_api_key(&settings))
}
/// Puts agy back on its login: `modelProvider` as it was before a provider
/// set it, else none. Forgets the provider and its record. Returns the
/// settings as they were, to restore on a later failure; none unchanged.
/// Settings Copool cannot read refuse the switch only when it put a
/// provider in them: an account's switch never depends on agy's settings
/// otherwise.
pub(super) fn leave_api_key(path: &Path, store: &mut Store) -> Result<Option<Option<String>>> {
    let applied = store.settings_provider.is_some() || store.settings_record.is_some();
    let record = store.settings_record.take();
    store.settings_provider = None;
    let read = read_settings(path)
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
    put_settings(path, Some(&render_settings(&settings, text.as_deref())?))?;
    Ok(Some(text))
}
/// Restores settings `leave_api_key` changed.
pub(super) fn restore_settings(path: &Path, previous: Option<Option<String>>) {
    if let Some(previous) = previous {
        let _ = put_settings(path, previous.as_deref());
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
        if draft.base_url.is_empty() && draft.admin_base_url.is_empty() {
            return Err("error.sub2api.configuration_incomplete".into());
        }
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
                &[&draft.base_url, &draft.admin_base_url],
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
        let admin = if p.admin_base_url.is_empty() {
            &p.base_url
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
    /// Whether agy's environment holds `name`, as the user's shell exports
    /// it; asked off the runtime, for the shell may take seconds.
    async fn antigravity_exported(&self, name: &'static str) -> Result<bool> {
        let environment = self.antigravity.environment.read().unwrap().clone();
        tokio::task::spawn_blocking(move || environment(name).is_some())
            .await
            .map_err(|e| e.to_string())
    }
    /// Puts agy on the provider's gateway: `"modelProvider": "gemini"` in its
    /// settings, recording what that replaced; the login in the Keychain
    /// stays, for an account's switch to return to. Called under the pool's
    /// operations lock.
    pub(super) async fn antigravity_switch_sub2api(&self, id: &str) -> Result<SwitchExecution> {
        let (member, account) = self.card(Pool::Antigravity, id)?;
        if !account.status.eq_ignore_ascii_case("active") {
            return Err("Account is deactivated".into());
        }
        // agy without them would exit at once, or send the key to Google.
        if !self.antigravity_exported(API_KEY).await? {
            return Err(API_KEY_MISSING.into());
        }
        if !self.antigravity_exported(BASE_URL).await? {
            return Err(BASE_URL_MISSING.into());
        }
        let path = self.antigravity_settings_path();
        let _lock = self.antigravity.lock.lock().await;
        let mut store = self.antigravity_load()?;
        let name = store
            .providers
            .iter()
            .find(|p| p.id == member.key)
            .ok_or("Account not found")?
            .name
            .clone();
        let text = read_settings(&path)?;
        let mut settings = parse_settings(text.as_deref())?;
        // Between providers, the record of the login's settings stays.
        if store.settings_record.is_none() {
            store.settings_record = Some(SettingsRecord {
                model_provider: settings.get(MODEL_PROVIDER).cloned(),
            });
        }
        settings.insert(MODEL_PROVIDER.into(), GEMINI.into());
        put_settings(&path, Some(&render_settings(&settings, text.as_deref())?))?;
        store.settings_provider = Some(member.key);
        if let Err(error) = self.antigravity_save(&store) {
            let _ = put_settings(&path, text.as_deref());
            return Err(error);
        }
        Ok(SwitchExecution {
            provider: Some(name),
            ..Default::default()
        })
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
        let path = dir.path().join("settings.json");
        for newline in ["\n", "\r\n"] {
            for bom in ["", "\u{feff}"] {
                let applied = format!(
                    "{bom}{{{newline}  \"trustedWorkspaces\": [\"/w\"],{newline}  \"modelProvider\": \"gemini\",{newline}  \"agentMode\": \"auto\"{newline}}}{newline}"
                );
                fs_write(&path, &applied);
                let mut store = record(None);
                let previous = leave_api_key(&path, &mut store).unwrap();
                assert_eq!(previous, Some(Some(applied.clone())));
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
        let path = dir.path().join("settings.json");
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
        assert_eq!(leave_api_key(&path, &mut record(None)).unwrap(), None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), unchanged);
        // No settings at all stay none.
        let missing = dir.path().join("missing/settings.json");
        assert_eq!(leave_api_key(&missing, &mut record(None)).unwrap(), None);
        assert!(!missing.exists());
        // Settings that are not a JSON object are refused, not replaced,
        // once Copool put a provider in them; otherwise left alone.
        for invalid in ["[]", "{not json"] {
            fs_write(&path, invalid);
            assert_eq!(
                leave_api_key(&path, &mut record(None)).unwrap_err(),
                SETTINGS_INVALID
            );
            assert_eq!(leave_api_key(&path, &mut Store::default()).unwrap(), None);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
        }
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

    fn fs_write(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
    }
}
