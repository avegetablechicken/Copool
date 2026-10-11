//! The original settings schema and Sub2api configuration contract.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    #[serde(rename = "providerID")]
    pub provider_id: String,
    pub username: String,
    pub password: String,
    #[serde(rename = "allowInsecureTLS")]
    pub allow_insecure_tls: bool,
    #[serde(rename = "proxyURL")]
    pub proxy_url: String,
    #[serde(rename = "accountProxyURLs")]
    pub account_proxy_urls: BTreeMap<String, String>,
    #[serde(rename = "importedAccountIDs")]
    pub imported_account_ids: Vec<i64>,
    pub cached_accounts: Vec<Value>,
    #[serde(rename = "adminBaseURL")]
    pub admin_base_url: String,
    #[serde(rename = "legacyAdminBaseURL")]
    pub legacy_admin_base_url: String,
}
impl Default for Provider {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string().to_uppercase(),
            provider_id: String::new(),
            username: String::new(),
            password: String::new(),
            allow_insecure_tls: false,
            proxy_url: String::new(),
            account_proxy_urls: BTreeMap::new(),
            imported_account_ids: vec![],
            cached_accounts: vec![],
            admin_base_url: String::new(),
            legacy_admin_base_url: String::new(),
        }
    }
}
impl Provider {
    pub fn normalized(&self) -> Self {
        let mut value = self.clone();
        value.provider_id = self.provider_id.trim().into();
        value.username = self.username.trim().into();
        value.proxy_url = self.proxy_url.trim().into();
        value.admin_base_url = self.admin_base_url.trim().trim_matches('/').into();
        value.legacy_admin_base_url = self.legacy_admin_base_url.trim().trim_matches('/').into();
        value.imported_account_ids.retain(|id| *id > 0);
        value.imported_account_ids.sort();
        value.imported_account_ids.dedup();
        let mut seen = std::collections::HashSet::new();
        value.cached_accounts = self
            .cached_accounts
            .iter()
            .filter_map(|account| {
                let id = account.get("id")?.as_i64()?;
                if !value.imported_account_ids.contains(&id) || !seen.insert(id) {
                    return None;
                }
                let mut account = account.clone();
                if let Some(proxy) = value.account_proxy_urls.get(&id.to_string()) {
                    account["proxyURL"] = proxy.clone().into();
                } else if let Some(object) = account.as_object_mut() {
                    object.remove("proxyURL");
                }
                Some(account)
            })
            .collect();
        value
    }
    pub fn is_complete(&self) -> bool {
        !self.username.trim().is_empty() && !self.password.is_empty()
    }
    pub fn is_enabled(&self) -> bool {
        !self.provider_id.trim().is_empty() && self.is_complete()
    }
    pub fn has_content(&self) -> bool {
        !self.provider_id.trim().is_empty()
            || !self.username.trim().is_empty()
            || !self.password.is_empty()
            || !self.proxy_url.trim().is_empty()
            || !self.imported_account_ids.is_empty()
            || !self.cached_accounts.is_empty()
            || !self.admin_base_url.trim().is_empty()
            || !self.legacy_admin_base_url.trim().is_empty()
    }
    pub fn proxy_for(&self, id: i64) -> String {
        self.account_proxy_urls
            .get(&id.to_string())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&self.proxy_url)
            .trim()
            .into()
    }
}
impl Serialize for Provider {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let p = self.normalized();
        json!({"id":p.id,"providerID":p.provider_id,"username":p.username,"allowInsecureTLS":p.allow_insecure_tls,"proxyURL":p.proxy_url,"accountProxyURLs":p.account_proxy_urls,"importedAccountIDs":p.imported_account_ids,"cachedAccounts":p.cached_accounts,"adminBaseURL":p.admin_base_url,"legacyAdminBaseURL":p.legacy_admin_base_url}).serialize(serializer)
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Providers {
    pub confirmed_provider_ids: Vec<String>,
    pub providers: Vec<Provider>,
}
impl Providers {
    pub fn normalized(&self) -> Self {
        let mut ids = std::collections::HashSet::new();
        let confirmed_provider_ids = self
            .confirmed_provider_ids
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty() && ids.insert(s.to_lowercase()))
            .map(str::to_owned)
            .collect();
        let mut seen = std::collections::HashSet::new();
        let providers = self
            .providers
            .iter()
            .map(Provider::normalized)
            .filter(|p| {
                p.has_content()
                    && (p.provider_id.is_empty() || seen.insert(p.provider_id.to_lowercase()))
            })
            .collect();
        Self {
            confirmed_provider_ids,
            providers,
        }
    }
    pub fn confirms(&self, id: &str) -> bool {
        self.normalized()
            .confirmed_provider_ids
            .iter()
            .any(|v| v.eq_ignore_ascii_case(id))
    }
    pub fn provider(&self, id: &str) -> Option<Provider> {
        self.normalized()
            .providers
            .into_iter()
            .find(|p| p.provider_id.eq_ignore_ascii_case(id))
    }
    pub fn upsert(&mut self, provider: Provider) {
        if let Some(old) = self.providers.iter_mut().find(|p| p.id == provider.id) {
            *old = provider;
        } else {
            self.providers.push(provider);
        }
    }
}
impl Serialize for Providers {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let p = self.normalized();
        json!({"confirmedProviderIDs":p.confirmed_provider_ids,"providers":p.providers})
            .serialize(s)
    }
}
impl<'de> Deserialize<'de> for Providers {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(d)?;
        let ids = serde_json::from_value(
            value
                .get("confirmedProviderIDs")
                .cloned()
                .unwrap_or(json!([])),
        )
        .map_err(serde::de::Error::custom)?;
        let providers = if let Some(v) = value.get("providers") {
            serde_json::from_value(v.clone()).map_err(serde::de::Error::custom)?
        } else {
            let mut provider: Provider =
                serde_json::from_value(value.clone()).map_err(serde::de::Error::custom)?;
            provider.legacy_admin_base_url = provider.admin_base_url.clone();
            provider.admin_base_url.clear();
            if provider.has_content() {
                vec![provider]
            } else {
                vec![]
            }
        };
        Ok(Self {
            confirmed_provider_ids: ids,
            providers,
        }
        .normalized())
    }
}
fn used() -> String {
    "used".into()
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub launch_at_startup: bool,
    pub launch_codex_after_switch: bool,
    pub auto_smart_switch: bool,
    pub sync_opencode_openai_auth: bool,
    #[serde(rename = "localProxyHostAPIOnly")]
    pub local_proxy_host_api_only: bool,
    pub restart_editors_on_switch: bool,
    pub restart_editor_targets: Vec<String>,
    pub auto_start_api_proxy: bool,
    pub proxy_configuration: Value,
    pub remote_servers: Vec<Value>,
    pub locale: String,
    #[serde(default = "used")]
    pub usage_progress_display_mode: String,
    #[serde(default, rename = "sub2APIProvider")]
    pub sub2_api_provider: Providers,
    #[serde(default, rename = "accountProxyURLs")]
    pub account_proxy_urls: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            launch_at_startup: false,
            launch_codex_after_switch: true,
            auto_smart_switch: false,
            sync_opencode_openai_auth: false,
            local_proxy_host_api_only: false,
            restart_editors_on_switch: false,
            restart_editor_targets: vec![],
            auto_start_api_proxy: false,
            proxy_configuration: json!({"preferredPortText":"8787","cloudflared":{"enabled":false,"tunnelMode":"quick","useHTTP2":false,"namedHostname":""}}),
            remote_servers: vec![],
            locale: String::new(),
            usage_progress_display_mode: "used".into(),
            sub2_api_provider: Providers::default(),
            account_proxy_urls: BTreeMap::new(),
            extra: Default::default(),
        }
    }
}
