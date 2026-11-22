//! Sub2api in the Grok pool: its Grok OAuth accounts as cards, and Grok
//! Build pointed at its OpenAI-compatible gateway (`<Sub2api>/v1`).
//!
//! Grok Build sends a signed-in session's token to any endpoint a model
//! names without a key of its own, so its `[endpoints] models_base_url`
//! would hand the xAI login to Sub2api. A switch instead gives each model
//! Grok Build may use, in its `config.toml`, the gateway as `base_url` and
//! the provider's variable as `env_key`: Grok Build then sends that
//! variable's value there and nothing when it is unset, and its login stays
//! in `auth.json` for an account's switch to return to. The models are
//! those of Grok Build's catalog (`models_cache.json`), those `[models]`
//! names for its roles, and the Grok text models the gateway lists; a model
//! none of them names keeps using the login, at xAI. What the switch
//! replaced is recorded, and an account's switch puts it back.
//!
//! As a Codex provider's `env_key`, a provider names the variable holding
//! its key, and Copool never holds the key itself. What the pools do alike
//! with their providers is in `providers.rs`.
use super::{api, Result, Store};
use crate::{
    application::{Application, SwitchExecution},
    codex_config::Definition,
    pool::Pool,
    sub2api::{self, Account, Member},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use toml_edit::{DocumentMut, Item, Table};

/// The variable a provider's key is read from unless it names another.
pub(super) const API_KEY: &str = "GROK_SUB2API_API_KEY";
pub(crate) const API_KEY_MISSING: &str = "error.grok.sub2api_api_key_missing";
pub(crate) const API_KEY_VARIABLE_INVALID: &str = "error.grok.api_key_variable_invalid";
pub(crate) const CONFIG_INVALID: &str = "error.grok.config_invalid";
pub(crate) const NO_MODELS: &str = "error.grok.sub2api_no_models";
pub(crate) const MODELS_TAKEN: &str = "error.grok.sub2api_models_taken";
const CONFIGURATION_INCOMPLETE: &str = "error.sub2api.configuration_incomplete";
/// The roles `[models]` gives a model besides the default.
const ROLES: [&str; 4] = [
    "default",
    "web_search",
    "session_summary",
    "image_description",
];

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
    /// shell exports it: `GROK_SUB2API_API_KEY` unless named. Never the key.
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
pub(super) fn key_variable(p: &Provider) -> &str {
    match p.api_key_variable.as_str() {
        "" => API_KEY,
        name => name,
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
    /// The gateway the models were last given.
    gateway: String,
    /// Whether there was no `config.toml` before.
    created: bool,
    models: Vec<ModelRecord>,
}
/// A model the switch gave the gateway: its table's `base_url` and
/// `env_key` before, as TOML values, and whether the table was new.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
struct ModelRecord {
    id: String,
    created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    env_key: Option<String>,
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
fn models_table(doc: &mut DocumentMut) -> Result<&mut Table> {
    let models = doc
        .entry("model")
        .or_insert_with(|| {
            let mut table = Table::new();
            table.set_implicit(true);
            Item::Table(table)
        })
        .as_table_mut()
        .ok_or(CONFIG_INVALID)?;
    Ok(models)
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
/// The models Grok Build may use that `config.toml` names: those its
/// `[models]` gives a role.
fn configured_models(doc: &DocumentMut) -> Vec<String> {
    ROLES
        .iter()
        .filter_map(|role| doc.get("models")?.get(role)?.as_str())
        .map(str::to_owned)
        .collect()
}
/// The models of Grok Build's catalog, as it last fetched it.
pub(super) fn catalog_models(cache: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(cache) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return Vec::new();
    };
    let Some(models) = value.get("models").and_then(Value::as_object) else {
        return Vec::new();
    };
    models
        .iter()
        .flat_map(|(id, entry)| {
            [Some(id.as_str()), entry["info"]["model"].as_str()]
                .into_iter()
                .flatten()
        })
        .map(str::to_owned)
        .collect()
}
/// The Grok text models in a gateway's `/v1/models`: neither images nor
/// videos.
pub(super) fn gateway_models(body: &Value) -> Vec<String> {
    body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| model["id"].as_str())
        .map(str::trim)
        .filter(|id| {
            let id = id.to_ascii_lowercase();
            id.starts_with("grok") && !["imagine", "image", "video"].iter().any(|m| id.contains(m))
        })
        .map(str::to_owned)
        .collect()
}
/// Gives the models the gateway and the variable, recording what each had.
/// What an earlier provider's switch recorded is handed over: a model it
/// still routes and this provider serves keeps its record of what it had
/// before; one this provider does not serve goes back; one the user has
/// taken off that gateway since is theirs. A model whose table names
/// another endpoint of the user's own is left alone.
fn apply(
    doc: &mut DocumentMut,
    record: &mut ConfigRecord,
    models: &[String],
    gateway: &str,
    variable: &str,
) -> Result<()> {
    let earlier = std::mem::take(&mut record.models);
    let (kept, back): (Vec<_>, Vec<_>) = earlier
        .into_iter()
        .filter(|m| on_gateway(doc, &record.gateway, &m.id))
        .partition(|m| models.contains(&m.id));
    restore(
        doc,
        &ConfigRecord {
            gateway: record.gateway.clone(),
            created: false,
            models: back,
        },
    );
    record.models = kept;
    let table = models_table(doc)?;
    for id in models {
        let recorded = record.models.iter().any(|m| &m.id == id);
        let entry = table.entry(id);
        let created = matches!(entry, toml_edit::Entry::Vacant(_));
        let model = entry
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_mut()
            .ok_or(CONFIG_INVALID)?;
        if !recorded && model.contains_key("base_url") {
            continue;
        }
        if !recorded {
            record.models.push(ModelRecord {
                id: id.clone(),
                created,
                base_url: value_text(model.get("base_url")),
                env_key: value_text(model.get("env_key")),
            });
        }
        model.insert("base_url", toml_edit::value(gateway));
        model.insert("env_key", toml_edit::value(variable));
    }
    if table.is_empty() {
        doc.remove("model");
    }
    record.gateway = gateway.to_owned();
    Ok(())
}
/// Puts back what the switch replaced: a model's earlier `base_url` and
/// `env_key`, and no table where there was none (with what the user added
/// to it since kept). A model the user has taken off the gateway since is
/// theirs.
fn restore(doc: &mut DocumentMut, record: &ConfigRecord) {
    let ours: Vec<&ModelRecord> = record
        .models
        .iter()
        .filter(|m| on_gateway(doc, &record.gateway, &m.id))
        .collect();
    let Some(table) = doc.get_mut("model").and_then(Item::as_table_mut) else {
        return;
    };
    for entry in ours {
        let Some(model) = table.get_mut(&entry.id).and_then(Item::as_table_mut) else {
            continue;
        };
        put_value(model, "base_url", entry.base_url.as_deref());
        put_value(model, "env_key", entry.env_key.as_deref());
        if entry.created && model.is_empty() {
            table.remove(&entry.id);
        }
    }
    if table.is_empty() {
        doc.remove("model");
    }
}
/// Whether the model's `base_url` is the gateway.
fn on_gateway(doc: &DocumentMut, gateway: &str, id: &str) -> bool {
    doc.get("model")
        .and_then(|t| t.get(id))
        .and_then(|t| t.get("base_url"))
        .and_then(Item::as_str)
        == Some(gateway)
}
/// Whether Grok Build still uses the recorded gateway: some recorded model
/// has it as `base_url`.
fn uses_gateway(doc: &DocumentMut, record: &ConfigRecord) -> bool {
    record
        .models
        .iter()
        .any(|m| on_gateway(doc, &record.gateway, &m.id))
}
/// Whether Grok Build uses a provider's gateway, as its `config.toml` says.
pub(super) fn provider_mode(path: &Path, store: &Store) -> bool {
    let Some(record) = store.config_record.as_ref() else {
        return false;
    };
    read_text(path)
        .ok()
        .flatten()
        .and_then(|text| text.parse::<DocumentMut>().ok())
        .is_some_and(|doc| uses_gateway(&doc, record))
}
/// What leaving the gateway changed, to be undone on a failed switch.
pub(super) struct Left {
    text: Option<String>,
    record: Option<ConfigRecord>,
    provider: Option<String>,
}
/// Takes Grok Build off the gateway before an account's switch: what the
/// provider replaced goes back, and the record with it.
pub(super) fn leave_gateway(path: &Path, store: &mut Store) -> Result<Left> {
    let text = read_text(path)?;
    let left = Left {
        text: text.clone(),
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
    restore(&mut doc, record);
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
        draft.api_key_variable = match draft.api_key_variable.trim() {
            "" => API_KEY.into(),
            name if crate::proxy_integration::is_variable(name) => name.into(),
            // A key typed here would be kept in plain text: never.
            _ => return Err(API_KEY_VARIABLE_INVALID.into()),
        };
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
                env_key: Some(key_variable(&p).into()),
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
    /// A variable's value as a terminal opened now exports it, which Grok
    /// Build started there has. Asked off the runtime, for the shell may
    /// take seconds.
    async fn grok_exported(&self, name: &str) -> Result<Option<String>> {
        let environment = self.grok.environment.read().unwrap().clone();
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || environment(&name))
            .await
            .map_err(|e| e.to_string())
    }
    /// Puts Grok Build on the provider's gateway: each model it may use
    /// sends the provider's variable there, as `config.toml` gives it,
    /// recording what that replaced; the login in `auth.json` stays, for an
    /// account's switch to return to. Called under the pool's operations
    /// lock.
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
        let variable = key_variable(&provider);
        // A variable nothing exports would leave Grok Build without a key.
        let key = self
            .grok_exported(variable)
            .await?
            .filter(|key| !key.trim().is_empty())
            .ok_or(API_KEY_MISSING)?;
        let gateway = gateway(&provider)?;
        // The gateway's models, with the key Grok Build will send: a key
        // Sub2api refuses is refused here, before Grok Build is changed.
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
        let _lock = self.grok.lock.lock().await;
        let mut store = self.grok_load()?;
        // The key and models were checked outside the lock, for the provider
        // as it was.
        let now = store
            .providers
            .iter()
            .find(|p| p.id == provider.id)
            .ok_or("Account not found")?;
        if (self::gateway(now)?.as_str(), key_variable(now)) != (gateway.as_str(), variable) {
            return Err("The Grok provider changed during the switch; switch again".into());
        }
        let text = read_text(&path)?;
        let mut doc = parse(text.as_deref())?;
        let mut models = catalog_models(&self.grok_models_cache_path());
        models.extend(configured_models(&doc));
        models.extend(gateway_models(&body));
        let mut seen = std::collections::BTreeSet::new();
        models.retain(|m| !m.trim().is_empty() && seen.insert(m.clone()));
        if models.is_empty() {
            return Err(NO_MODELS.into());
        }
        // Between providers, the record of the login's configuration stays.
        let mut record = store.config_record.clone().unwrap_or(ConfigRecord {
            created: text.is_none(),
            ..Default::default()
        });
        apply(&mut doc, &mut record, &models, &gateway, variable)?;
        // Every model names an endpoint of the user's own: Grok Build would
        // not use the gateway.
        if !uses_gateway(&doc, &record) {
            return Err(MODELS_TAKEN.into());
        }
        put_text(&path, Some(&doc.to_string()))?;
        store.config_record = Some(record);
        store.config_provider = Some(member.key);
        if let Err(error) = self.grok_save(&store) {
            let _ = put_text(&path, text.as_deref());
            return Err(error);
        }
        Ok(SwitchExecution {
            provider: Some(provider.name),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

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
        assert_eq!(key_variable(&provider), API_KEY);
        assert!(gateway(&Provider::default()).is_err());
    }

    #[test]
    fn models_come_from_the_catalog_the_roles_and_the_gateways_text_models() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("models_cache.json");
        std::fs::write(
            &cache,
            r#"{"models": {"grok-4.7": {"info": {"id": "grok-4.7", "model": "grok-4.7-0920"}}}}"#,
        )
        .unwrap();
        assert_eq!(catalog_models(&cache), ["grok-4.7", "grok-4.7-0920"]);
        assert!(catalog_models(&dir.path().join("missing.json")).is_empty());
        let doc: DocumentMut = "[models]\ndefault = \"grok-4.6\"\nweb_search = \"grok-4.5\"\n"
            .parse()
            .unwrap();
        assert_eq!(configured_models(&doc), ["grok-4.6", "grok-4.5"]);
        let listed = serde_json::json!({"object": "list", "data": [
            {"id": "grok-4.6"}, {"id": "grok-imagine-image"}, {"id": "grok-imagine-video-1.5"},
            {"id": "grok-3-mini"}, {"id": "gpt-5"}
        ]});
        assert_eq!(gateway_models(&listed), ["grok-4.6", "grok-3-mini"]);
    }

    #[test]
    fn a_switch_gives_each_model_the_gateway_and_leaving_puts_back_what_it_had() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let before = "# the user's\n[features]\ntelemetry = false\n\n\
            [model.\"grok-4.6\"]\ntemperature = 0.5\nenv_key = [\"A\", \"B\"]\n\n\
            [model.mine]\nmodel = \"local\"\nbase_url = \"http://localhost:11434/v1\"\n";
        put_text(&path, Some(before)).unwrap();
        let mut store = Store::default();
        let mut doc = parse(Some(before)).unwrap();
        let mut record = ConfigRecord::default();
        apply(
            &mut doc,
            &mut record,
            &models(&["grok-4.7", "grok-4.6", "mine"]),
            "https://relay.example.com/v1",
            "GROK_SUB2API_API_KEY",
        )
        .unwrap();
        let text = doc.to_string();
        let switched: DocumentMut = text.parse().unwrap();
        for id in ["grok-4.7", "grok-4.6"] {
            assert_eq!(
                switched["model"][id]["base_url"].as_str(),
                Some("https://relay.example.com/v1")
            );
            assert_eq!(
                switched["model"][id]["env_key"].as_str(),
                Some("GROK_SUB2API_API_KEY")
            );
        }
        // The user's own endpoint stays theirs.
        assert_eq!(
            switched["model"]["mine"]["base_url"].as_str(),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(
            switched["model"]["grok-4.6"]["temperature"].as_float(),
            Some(0.5)
        );
        assert!(text.starts_with("# the user's\n[features]"));
        put_text(&path, Some(&text)).unwrap();
        store.config_record = Some(record.clone());
        store.config_provider = Some("p".into());
        assert!(provider_mode(&path, &store));

        // Another provider keeps the record of the login's configuration;
        // a model it does not serve goes back as it was.
        apply(
            &mut doc,
            &mut record,
            &models(&["grok-4.7", "grok-4.5"]),
            "https://other.example.com/v1",
            "OTHER",
        )
        .unwrap();
        assert_eq!(record.models.len(), 2);
        let other: DocumentMut = doc.to_string().parse().unwrap();
        assert!(other["model"]["grok-4.6"].get("base_url").is_none());
        assert_eq!(
            other["model"]["grok-4.6"]["env_key"]
                .as_array()
                .map(|a| a.len()),
            Some(2)
        );
        for id in ["grok-4.7", "grok-4.5"] {
            assert_eq!(
                other["model"][id]["base_url"].as_str(),
                Some("https://other.example.com/v1")
            );
        }
        put_text(&path, Some(&doc.to_string())).unwrap();
        store.config_record = Some(record);
        assert!(provider_mode(&path, &store));

        let left = leave_gateway(&path, &mut store).unwrap();
        assert_eq!(read_text(&path).unwrap().as_deref(), Some(before));
        assert!(store.config_record.is_none() && store.config_provider.is_none());
        assert!(!provider_mode(&path, &store));
        // A failed account switch takes Grok Build back to the gateway.
        restore_left(&path, &mut store, left);
        assert!(provider_mode(&path, &store));
    }

    #[test]
    fn a_model_the_user_took_off_the_gateway_stays_theirs() {
        let mut doc = parse(Some("[model.\"grok-4.7\"]\ntemperature = 0.3\n")).unwrap();
        let mut record = ConfigRecord::default();
        let relay = "https://relay.example.com/v1";
        apply(
            &mut doc,
            &mut record,
            &models(&["grok-4.7", "grok-4.6"]),
            relay,
            "K",
        )
        .unwrap();
        // The user sends both models elsewhere themselves.
        let mine = "http://localhost:11434/v1";
        for id in ["grok-4.7", "grok-4.6"] {
            doc["model"][id]["base_url"] = toml_edit::value(mine);
        }
        assert!(!uses_gateway(&doc, &record));
        // Another provider's switch leaves them alone, and has nothing to
        // route.
        apply(
            &mut doc,
            &mut record,
            &models(&["grok-4.7", "grok-4.6"]),
            "https://other.example.com/v1",
            "OTHER",
        )
        .unwrap();
        assert!(record.models.is_empty());
        assert!(!uses_gateway(&doc, &record));
        // Nor does leaving put back what the first switch replaced.
        let mut doc = parse(Some("[model.\"grok-4.7\"]\ntemperature = 0.3\n")).unwrap();
        let mut record = ConfigRecord::default();
        apply(&mut doc, &mut record, &models(&["grok-4.7"]), relay, "K").unwrap();
        assert_eq!(record.models.len(), 1);
        doc["model"]["grok-4.7"]["base_url"] = toml_edit::value("http://localhost:8080/v1");
        let mine = doc.to_string();
        restore(&mut doc, &record);
        assert_eq!(doc.to_string(), mine);
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
        apply(
            &mut doc,
            &mut record,
            &models(&["grok-4.7"]),
            "https://r/v1",
            "K",
        )
        .unwrap();
        put_text(&path, Some(&doc.to_string())).unwrap();
        let mut store = Store {
            config_record: Some(record.clone()),
            ..Default::default()
        };
        leave_gateway(&path, &mut store).unwrap();
        assert!(!path.exists());

        // The user tuned the model meanwhile: that stays.
        apply(
            &mut doc,
            &mut record,
            &models(&["grok-4.7"]),
            "https://r/v1",
            "K",
        )
        .unwrap();
        let mut tuned = doc.to_string();
        tuned.push_str("temperature = 0.2\n");
        put_text(&path, Some(&tuned)).unwrap();
        store.config_record = Some(record);
        leave_gateway(&path, &mut store).unwrap();
        let left: DocumentMut = read_text(&path).unwrap().unwrap().parse().unwrap();
        assert_eq!(
            left["model"]["grok-4.7"]["temperature"].as_float(),
            Some(0.2)
        );
        assert!(left["model"]["grok-4.7"].get("base_url").is_none());
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
