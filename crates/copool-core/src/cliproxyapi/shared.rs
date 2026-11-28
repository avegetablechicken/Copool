//! A CLIProxyAPI provider as every pool shares it. One server lists the
//! credentials of every pool, so where and how Copool reaches its management
//! API (its name, address, proxy and TLS check, and its key in the keyring)
//! is kept once, in `cliproxyapi-providers.json`, and is the same in every
//! pool. Each pool keeps its own record of it by the same id beside: how
//! its tool is switched to the provider, and its cards. A pool without one
//! yet shows the provider with the switch's defaults, ready to import from.
use crate::{
    application::Application,
    pool::Pool,
    provider_registry::{Kind, ProviderRecord},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

type Result<T> = std::result::Result<T, String>;

const FILE: &str = "cliproxyapi-providers.json";

/// The part of a CLIProxyAPI provider every pool shares.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Shared {
    pub id: String,
    pub name: String,
    /// The management site.
    #[serde(rename = "adminBaseURL")]
    pub admin_base_url: String,
    #[serde(rename = "proxyURL")]
    pub proxy_url: String,
    #[serde(rename = "allowInsecureTLS")]
    pub allow_insecure_tls: bool,
}
impl Shared {
    pub(crate) fn of(record: &ProviderRecord) -> Self {
        Self {
            id: record.id.clone(),
            name: record.name.clone(),
            admin_base_url: record.admin_base_url.clone(),
            proxy_url: record.proxy_url.clone(),
            allow_insecure_tls: record.allow_insecure_tls,
        }
    }
    /// Gives a pool's record of the provider what every pool shares.
    pub(crate) fn apply(&self, record: &mut ProviderRecord) {
        record.name = self.name.clone();
        record.admin_base_url = self.admin_base_url.clone();
        record.proxy_url = self.proxy_url.clone();
        record.allow_insecure_tls = self.allow_insecure_tls;
    }
}

/// The keyring item of a CLIProxyAPI provider's management key, one for
/// every pool.
pub(crate) fn key_secret(id: &str) -> String {
    format!("cliproxyapi-management-key:{id}")
}

impl Application {
    /// The CLIProxyAPI providers, as every pool shares them.
    pub(crate) fn cliproxyapi_providers(&self) -> Result<Vec<Shared>> {
        let path = self.paths.data_dir.join(FILE);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.to_string()),
        }
    }
    /// Edits the shared CLIProxyAPI providers under their lock, saving them
    /// unless the edit fails.
    pub(crate) fn edit_cliproxyapi_providers<T>(
        &self,
        edit: impl FnOnce(&mut Vec<Shared>) -> Result<T>,
    ) -> Result<T> {
        let _lock = self
            .cliproxyapi_providers
            .lock()
            .map_err(|_| "The CLIProxyAPI providers are unavailable")?;
        let mut providers = self.cliproxyapi_providers()?;
        let result = edit(&mut providers)?;
        crate::persistence::write_json(&self.paths.data_dir.join(FILE), &providers)?;
        Ok(result)
    }
}

/// The variable holding a CLIProxyAPI provider's API key, which every
/// pool's tool sends, unless its form names another.
pub(crate) const API_KEY_VARIABLE: &str = "CLIPROXYAPI_API_KEY";
/// A provider with the same management site and key is the same one.
pub(crate) const DUPLICATE: &str = "error.cliproxyapi.duplicate_provider";

/// A pool's refusal of a form as CLIProxyAPI's settings word it: the pools
/// share the checks of Sub2api's forms, and their messages.
fn own_error(error: String) -> String {
    match error.as_str() {
        "error.sub2api.configuration_incomplete" => "error.cliproxyapi.configuration_incomplete",
        "error.sub2api.invalid_base_url" => "error.cliproxyapi.invalid_base_url",
        "error.sub2api.duplicate_provider" => "error.cliproxyapi.duplicate_name",
        _ => return error,
    }
    .to_owned()
}

impl Application {
    /// The CLIProxyAPI providers for the settings, without their keys: what
    /// every pool shares of each, with the address its API is reached at
    /// (`baseURL`) and the variable holding its API key, as every pool's
    /// switch takes them. Each is blank when its default is: the address
    /// then follows the management site as the site is edited.
    pub fn cliproxyapi_provider_list(&self) -> Result<Value> {
        // Every pool's record names the same; Claude's is read.
        let registry = self.registry(Pool::Claude);
        let listed = registry.providers()?;
        let providers = self
            .cliproxyapi_providers()?
            .into_iter()
            .map(|shared| {
                let mut value = serde_json::to_value(&shared).map_err(|e| e.to_string())?;
                value["kind"] = Kind::CliProxyApi.name().into();
                let record = listed
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|p| p["id"] == shared.id.as_str());
                let text = |key: &str| {
                    record
                        .and_then(|r| r[key].as_str())
                        .unwrap_or_default()
                        .to_owned()
                };
                let mut address = text("baseURL");
                if address == registry.default_gateway(&shared)? {
                    address.clear();
                }
                let mut variable = text("apiKeyVariable");
                if variable == API_KEY_VARIABLE {
                    variable.clear();
                }
                value["baseURL"] = address.into();
                value["apiKeyVariable"] = variable.into();
                Ok(value)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(providers.into())
    }
    /// Saves a CLIProxyAPI provider for every pool: what they share, its
    /// key (blank keeps the saved one), and the address of its API (blank:
    /// its management site's root) and the variable holding its API key,
    /// which every pool's switch takes, each pool's tool its own gateway
    /// at that address. One with another's management site and key is refused, as
    /// is a form any pool refuses, before any is saved.
    pub async fn save_cliproxyapi_provider(&self, draft: Value) -> Result<Value> {
        self.save_cliproxyapi_form(draft).await.map_err(own_error)
    }
    async fn save_cliproxyapi_form(&self, draft: Value) -> Result<Value> {
        let text = |key: &str| draft[key].as_str().unwrap_or_default().trim().to_owned();
        let id = text("id");
        if text("adminBaseURL").is_empty() {
            return Err("error.sub2api.configuration_incomplete".into());
        }
        let site = crate::cliproxyapi::management_base(&text("adminBaseURL"))
            .map_err(|_| "error.sub2api.invalid_base_url".to_owned())?;
        let key = match text("password") {
            key if key.is_empty() => self.secrets.get(&key_secret(&id))?.unwrap_or_default(),
            key => key,
        };
        for other in self.cliproxyapi_providers()?.iter().filter(|s| s.id != id) {
            let same_site = crate::cliproxyapi::management_base(&other.admin_base_url)
                .is_ok_and(|other| other.eq_ignore_ascii_case(&site));
            if same_site && self.secrets.get(&key_secret(&other.id))?.as_deref() == Some(&key) {
                return Err(DUPLICATE.into());
            }
        }
        // The API's root, whichever pool's gateway was typed.
        let address = crate::provider_registry::strip_suffix(&text("baseURL"), &["/v1"]);
        let form = || {
            json!({
                "id": id, "kind": Kind::CliProxyApi.name(), "name": text("name"),
                "adminBaseURL": text("adminBaseURL"),
                "allowInsecureTLS": draft["allowInsecureTLS"].as_bool().unwrap_or(false),
                "mode": "address", "password": "",
                "apiKeyVariable": match text("apiKeyVariable") {
                    variable if variable.is_empty() => API_KEY_VARIABLE.to_owned(),
                    variable => variable,
                },
                "baseURL": address,
            })
        };
        for pool in Pool::ALL {
            self.registry(pool).check(form())?;
        }
        // The first pool's save keeps what they share and the key, the
        // others their record of it. A new one any pool fails to save is removed
        // whole: none is left half saved.
        let new = id.is_empty();
        let mut id = id.clone();
        for (i, pool) in Pool::ALL.into_iter().enumerate() {
            let mut form = form();
            form["id"] = id.clone().into();
            if i == 0 {
                form["password"] = text("password").into();
            }
            match self.registry(pool).save(form).await {
                Ok(saved) => id = saved["id"].as_str().unwrap_or_default().to_owned(),
                Err(error) => {
                    if new && i > 0 {
                        if let Err(undo) = self.remove_cliproxyapi_provider(&id).await {
                            log::warn!("A CLIProxyAPI provider stays half saved: {undo}");
                        }
                    }
                    return Err(error);
                }
            }
        }
        self.cliproxyapi_provider_list()?
            .as_array()
            .and_then(|all| all.iter().find(|p| p["id"] == id.as_str()).cloned())
            .ok_or_else(|| crate::provider_registry::PROVIDER_NOT_FOUND.into())
    }
    /// Removes a CLIProxyAPI provider from every pool, with its key.
    pub async fn remove_cliproxyapi_provider(&self, id: &str) -> Result<()> {
        self.remove_provider(Pool::Codex, id).await
    }
}
