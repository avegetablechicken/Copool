//! Sub2api in the Claude pool: its Claude (anthropic) OAuth accounts as cards,
//! and Claude Code pointed at its gateway through `env` in `settings.json`, as
//! cc-switch does for third-party providers.
use super::{login_overrides, set_member, write_text, Location, Result, Store};
use crate::{
    application::{Application, SwitchExecution},
    codex_config::Definition,
    models::AccountSummary,
    persistence::write_json,
    sub2api::{self, Account, Platform},
    utils::now_unix_seconds,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, fs, path::PathBuf};

const CARD_PREFIX: &str = "claude-sub2api-";
const BASE_URL: &str = "ANTHROPIC_BASE_URL";
const AUTH_TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    /// The Sub2api address: the gateway root, or its admin API (`…/api/v1`).
    #[serde(rename = "baseURL")]
    pub base_url: String,
    /// Optional admin API override, independent of the Claude Code gateway.
    #[serde(rename = "adminBaseURL")]
    pub admin_base_url: String,
    pub username: String,
    /// Only in a draft from the form; the system keyring keeps it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub password: String,
    /// The Sub2api API key Claude Code sends; only in a draft.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    pub has_api_key: bool,
    #[serde(rename = "proxyURL")]
    pub proxy_url: String,
    #[serde(rename = "allowInsecureTLS")]
    pub allow_insecure_tls: bool,
    #[serde(rename = "importedAccountIDs")]
    pub imported_account_ids: Vec<i64>,
    pub cached_accounts: Vec<Account>,
    #[serde(rename = "accountProxyURLs")]
    pub account_proxy_urls: BTreeMap<String, String>,
}
impl Provider {
    /// The root Claude Code sends its requests to (`ANTHROPIC_BASE_URL`).
    fn gateway(&self) -> String {
        gateway(&self.base_url)
    }
    fn card_id(&self, account: i64) -> String {
        format!("{CARD_PREFIX}{}-{account}", self.id)
    }
}
fn gateway(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    url.strip_suffix("/api/v1")
        .or_else(|| url.strip_suffix("/v1"))
        .unwrap_or(url)
        .trim_end_matches('/')
        .to_owned()
}
fn password_secret(id: &str) -> String {
    format!("claude-sub2api-password:{id}")
}
fn api_key_secret(id: &str) -> String {
    format!("claude-sub2api-key:{id}")
}
pub(super) fn is_card(id: &str) -> bool {
    id.starts_with(CARD_PREFIX)
}
/// The provider index and account of a card.
fn card(store: &Store, id: &str) -> Option<(usize, Account)> {
    store.providers.iter().enumerate().find_map(|(i, p)| {
        p.cached_accounts
            .iter()
            .find(|a| p.card_id(a.id) == id)
            .map(|a| (i, a.clone()))
    })
}

fn settings_path(location: &Location) -> PathBuf {
    location.config_dir.join("settings.json")
}
fn settings_env(location: &Location) -> Map<String, Value> {
    fs::read_to_string(settings_path(location))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| v.get("env").and_then(Value::as_object).cloned())
        .unwrap_or_default()
}
/// The provider whose gateway Claude Code uses, if any.
pub(super) fn active(store: &Store, location: &Location) -> Option<usize> {
    let used = settings_env(location)
        .get(BASE_URL)
        .and_then(Value::as_str)
        .map(gateway)
        .filter(|g| !g.is_empty())?;
    store
        .providers
        .iter()
        .position(|p| p.gateway().eq_ignore_ascii_case(&used))
}
pub(super) fn summaries(store: &Store, active: Option<usize>) -> Vec<AccountSummary> {
    let mut result = vec![];
    for (i, p) in store.providers.iter().enumerate() {
        for a in &p.cached_accounts {
            let mut summary = a.summary();
            summary.id = p.card_id(a.id);
            summary.is_current = active == Some(i);
            // Sub2api names Claude accounts freely; the name stands in for an email.
            summary.email = summary.email.or_else(|| Some(a.name.clone()));
            summary.label = summary.email.clone().unwrap_or_default();
            summary.proxy_url = p
                .account_proxy_urls
                .get(&a.id.to_string())
                .cloned()
                .unwrap_or_default();
            result.push(summary);
        }
    }
    result
}

/// Edits `env` in Claude Code's user settings, keeping the rest of the file;
/// returns the previous file (none when it was missing) for a rollback.
fn edit_env(
    location: &Location,
    edit: impl FnOnce(&mut Map<String, Value>),
) -> Result<Option<String>> {
    let path = settings_path(location);
    let previous = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let text = previous
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "{}".into());
    let document: Value =
        serde_json::from_str(&text).map_err(|e| format!("Invalid Claude settings: {e}"))?;
    let mut env = document
        .get("env")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let before = env.clone();
    edit(&mut env);
    if env != before {
        write_text(&path, &set_member(&text, "env", &Value::Object(env))?)?;
    }
    Ok(previous)
}
pub(super) fn restore_settings(location: &Location, previous: Option<String>) -> Result<()> {
    let path = settings_path(location);
    match previous {
        Some(text) => write_text(&path, &text),
        None if path.exists() => fs::remove_file(path).map_err(|e| e.to_string()),
        None => Ok(()),
    }
}
/// Removes the gateway a Claude Sub2api provider set, so that Claude Code
/// uses the account login again. Keys Copool did not set are left alone.
pub(super) fn leave(store: &Store, location: &Location) -> Result<Option<String>> {
    edit_env(location, |env| {
        let ours = env
            .get(BASE_URL)
            .and_then(Value::as_str)
            .map(gateway)
            .is_some_and(|g| {
                store
                    .providers
                    .iter()
                    .any(|p| p.gateway().eq_ignore_ascii_case(&g))
            });
        if ours {
            env.remove(BASE_URL);
            env.remove(AUTH_TOKEN);
        }
    })
}

impl Application {
    /// Claude Sub2api providers, without their secrets.
    pub fn claude_sub2api_providers(&self) -> Result<Vec<Provider>> {
        let mut providers = self.claude_load()?.providers;
        for p in &mut providers {
            p.has_api_key = self.secrets.get(&api_key_secret(&p.id))?.is_some();
        }
        Ok(providers)
    }
    pub async fn claude_save_sub2api_provider(&self, mut draft: Provider) -> Result<Provider> {
        draft.name = draft.name.trim().into();
        draft.username = draft.username.trim().into();
        draft.base_url = draft.base_url.trim().trim_end_matches('/').into();
        draft.admin_base_url = draft.admin_base_url.trim().trim_end_matches('/').into();
        draft.proxy_url = draft.proxy_url.trim().into();
        let env = settings_env(&self.claude_location());
        let configured_base = env.get(BASE_URL).and_then(Value::as_str).unwrap_or("");
        if draft.base_url.is_empty() {
            draft.base_url = if configured_base.trim().is_empty() {
                gateway(&draft.admin_base_url)
            } else {
                configured_base.trim().trim_end_matches('/').into()
            };
        }
        // Only reuse a credential when Claude Code points to this gateway.
        if draft.api_key.trim().is_empty()
            && !configured_base.trim().is_empty()
            && gateway(configured_base).eq_ignore_ascii_case(&draft.gateway())
        {
            draft.api_key = [AUTH_TOKEN, "ANTHROPIC_API_KEY"]
                .iter()
                .filter_map(|key| env.get(*key).and_then(Value::as_str))
                .find(|key| !key.trim().is_empty())
                .unwrap_or("")
                .into();
        }
        if draft.id.trim().is_empty() {
            draft.id = uuid::Uuid::new_v4().to_string();
        }
        crate::utils::validate_account_proxy(&draft.proxy_url)?;
        for base in [&draft.base_url, &draft.admin_base_url] {
            if base.is_empty() {
                continue;
            }
            let url = reqwest::Url::parse(base).map_err(|_| "error.sub2api.invalid_base_url")?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err("error.sub2api.invalid_base_url".into());
            }
        }
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        if store
            .providers
            .iter()
            .any(|p| p.id != draft.id && p.name.eq_ignore_ascii_case(&draft.name))
        {
            return Err("error.sub2api.duplicate_provider".into());
        }
        let existing = store.providers.iter().position(|p| p.id == draft.id);
        if draft.name.is_empty()
            || draft.username.is_empty()
            || (draft.password.is_empty() && existing.is_none())
        {
            return Err("error.sub2api.configuration_incomplete".into());
        }
        if let Some(old) = existing.map(|i| &store.providers[i]) {
            draft.imported_account_ids = old.imported_account_ids.clone();
            draft.cached_accounts = old.cached_accounts.clone();
            draft.account_proxy_urls = old.account_proxy_urls.clone();
        }
        // Empty secrets keep the stored ones, as for the Codex providers.
        if !draft.password.is_empty() {
            self.secrets
                .set(&password_secret(&draft.id), &draft.password)?;
        }
        if !draft.api_key.trim().is_empty() {
            self.secrets
                .set(&api_key_secret(&draft.id), draft.api_key.trim())?;
        }
        draft.password.clear();
        draft.api_key.clear();
        draft.has_api_key = self.secrets.get(&api_key_secret(&draft.id))?.is_some();
        match existing {
            Some(i) => store.providers[i] = draft.clone(),
            None => store.providers.push(draft.clone()),
        }
        self.claude_save(&store)?;
        Ok(draft)
    }
    pub async fn claude_remove_sub2api_provider(&self, id: &str) -> Result<()> {
        {
            let _lock = self.claude.lock.lock().await;
            let mut store = self.claude_load()?;
            store.providers.retain(|p| p.id != id);
            self.claude_save(&store)?;
        }
        self.secrets.remove(&password_secret(id))?;
        self.secrets.remove(&api_key_secret(id))
    }
    fn claude_sub2api_connection(&self, p: &Provider) -> Result<sub2api::Connection> {
        let password = self
            .secrets
            .get(&password_secret(&p.id))?
            .ok_or("error.sub2api.configuration_incomplete")?;
        Ok(sub2api::Connection {
            configuration: crate::settings::Provider {
                id: p.id.clone(),
                provider_id: p.name.clone(),
                username: p.username.clone(),
                password,
                allow_insecure_tls: p.allow_insecure_tls,
                proxy_url: p.proxy_url.clone(),
                account_proxy_urls: p.account_proxy_urls.clone(),
                ..Default::default()
            },
            definition: Definition {
                id: p.name.clone(),
                base_url: Some(p.gateway()),
                wire_api: None,
                env_key: None,
                requires_openai_auth: None,
            },
            admin_base: sub2api::admin_base(if p.admin_base_url.is_empty() {
                &p.base_url
            } else {
                &p.admin_base_url
            })?,
        })
    }
    async fn claude_fetch_sub2api(
        &self,
        p: &Provider,
        ids: Option<&[i64]>,
    ) -> Result<Vec<Account>> {
        let mut accounts = self
            .claude_sub2api_connection(p)?
            .fetch_platform_accounts(Platform::Anthropic, ids, now_unix_seconds())
            .await?;
        for a in &mut accounts {
            a.provider_id = Some(p.name.clone());
        }
        Ok(accounts)
    }
    /// Imports every active Claude OAuth account of each provider; returns how many.
    pub async fn claude_sync_sub2api(&self) -> Result<usize> {
        let providers = self.claude_load()?.providers;
        if providers.is_empty() {
            return Err("error.claude.sub2api_not_configured".into());
        }
        let mut count = 0;
        let mut errors = vec![];
        for p in providers {
            match self.claude_fetch_sub2api(&p, None).await {
                Ok(fetched) => {
                    count += fetched.len();
                    let _lock = self.claude.lock.lock().await;
                    let mut store = self.claude_load()?;
                    if let Some(saved) = store.providers.iter_mut().find(|s| s.id == p.id) {
                        saved.imported_account_ids = fetched.iter().map(|a| a.id).collect();
                        saved.cached_accounts = fetched;
                    }
                    self.claude_save(&store)?;
                }
                Err(error) => errors.push(format!("{}: {error}", p.name)),
            }
        }
        if errors.is_empty() {
            Ok(count)
        } else {
            Err(errors.join("\n"))
        }
    }
    /// Refreshes imported cards (all, or some) without changing membership.
    pub(super) async fn claude_refresh_sub2api(
        &self,
        provider: &str,
        ids: Option<Vec<i64>>,
    ) -> Result<()> {
        let Some(p) = self
            .claude_load()?
            .providers
            .into_iter()
            .find(|p| p.id == provider)
        else {
            return Err("Provider not found".into());
        };
        let cards: Vec<_> = p
            .cached_accounts
            .iter()
            .filter(|a| ids.as_ref().is_none_or(|ids| ids.contains(&a.id)))
            .map(|a| p.card_id(a.id))
            .collect();
        let _activity: Vec<_> = cards.iter().map(|id| self.begin_refresh(id)).collect();
        let fetched = self.claude_fetch_sub2api(&p, ids.as_deref()).await?;
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        if let Some(saved) = store.providers.iter_mut().find(|s| s.id == p.id) {
            for account in fetched {
                if !saved.imported_account_ids.contains(&account.id) {
                    continue;
                }
                match saved
                    .cached_accounts
                    .iter_mut()
                    .find(|a| a.id == account.id)
                {
                    Some(cached) => *cached = account,
                    None => saved.cached_accounts.push(account),
                }
            }
            self.claude_save(&store)?;
        }
        Ok(())
    }
    pub(super) async fn claude_refresh_sub2api_card(&self, id: &str) -> Result<()> {
        let store = self.claude_load()?;
        let (i, account) = card(&store, id).ok_or("Account not found")?;
        self.claude_refresh_sub2api(&store.providers[i].id, Some(vec![account.id]))
            .await
    }
    /// Refreshes the providers of these cards, or every provider.
    pub(super) async fn claude_refresh_sub2api_cards(&self, ids: Option<&[String]>) -> Vec<String> {
        let Ok(store) = self.claude_load() else {
            return vec![];
        };
        let refreshes = store.providers.iter().filter_map(|p| {
            let wanted = match ids {
                None => None,
                Some(ids) => {
                    let wanted: Vec<i64> = p
                        .cached_accounts
                        .iter()
                        .filter(|a| ids.contains(&p.card_id(a.id)))
                        .map(|a| a.id)
                        .collect();
                    if wanted.is_empty() {
                        return None;
                    }
                    Some(wanted)
                }
            };
            Some(async move {
                self.claude_refresh_sub2api(&p.id, wanted)
                    .await
                    .map_err(|error| format!("{}: {error}", p.name))
            })
        });
        futures_util::future::join_all(refreshes)
            .await
            .into_iter()
            .filter_map(Result::err)
            .collect()
    }
    pub(super) fn claude_remove_sub2api_card(&self, store: &mut Store, id: &str) -> Result<()> {
        let (i, account) = card(store, id).ok_or("Account not found")?;
        let p = &mut store.providers[i];
        p.imported_account_ids.retain(|v| *v != account.id);
        p.cached_accounts.retain(|a| a.id != account.id);
        p.account_proxy_urls.remove(&account.id.to_string());
        Ok(())
    }
    pub(super) fn claude_sub2api_card_proxy(
        &self,
        store: &mut Store,
        id: &str,
        value: &str,
    ) -> Result<()> {
        let (i, account) = card(store, id).ok_or("Account not found")?;
        let proxies = &mut store.providers[i].account_proxy_urls;
        if value.trim().is_empty() {
            proxies.remove(&account.id.to_string());
        } else {
            proxies.insert(account.id.to_string(), value.trim().into());
        }
        Ok(())
    }
    /// Points Claude Code at the card's Sub2api gateway with the provider's
    /// API key. The previous `env` is kept in `claude-before-switch.json`.
    pub(super) async fn claude_switch_sub2api(&self, id: &str) -> Result<SwitchExecution> {
        let location = self.claude_location();
        let store = self.claude_load()?;
        let (i, account) = card(&store, id).ok_or("Account not found")?;
        if !account.status.eq_ignore_ascii_case("active") {
            return Err("Account is deactivated".into());
        }
        let p = &store.providers[i];
        let key = self
            .secrets
            .get(&api_key_secret(&p.id))?
            .filter(|k| !k.trim().is_empty())
            .ok_or("error.claude.sub2api_api_key_missing")?;
        write_json(
            &self.paths.data_dir.join("claude-before-switch.json"),
            &json!({"settingsEnv": settings_env(&location)}),
        )?;
        let gateway = p.gateway();
        edit_env(&location, |env| {
            env.insert(BASE_URL.into(), gateway.into());
            env.insert(AUTH_TOKEN.into(), key.into());
            // Both keys set make Claude Code warn; the gateway takes the token.
            env.remove("ANTHROPIC_API_KEY");
        })?;
        self.publish();
        Ok(SwitchExecution {
            provider: Some(p.name.clone()),
            login_overrides: login_overrides(&location)
                .into_iter()
                .filter(|k| k != BASE_URL && k != AUTH_TOKEN)
                .collect(),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Mutex};

    #[derive(Default)]
    struct Secrets(Mutex<HashMap<String, String>>);
    impl crate::credentials::SecretStore for Secrets {
        fn get(&self, id: &str) -> Result<Option<String>> {
            Ok(self.0.lock().unwrap().get(id).cloned())
        }
        fn set(&self, id: &str, value: &str) -> Result<()> {
            self.0.lock().unwrap().insert(id.into(), value.into());
            Ok(())
        }
        fn remove(&self, id: &str) -> Result<()> {
            self.0.lock().unwrap().remove(id);
            Ok(())
        }
    }
    /// A Sub2api admin API with one Claude account, recording request paths.
    async fn server(requests: std::sync::Arc<Mutex<Vec<String>>>) -> String {
        use axum::{extract::Request, routing::any, Json, Router};
        let router = Router::new().fallback(any(move |request: Request| {
            let requests = requests.clone();
            async move {
                let uri = request.uri().to_string();
                requests.lock().unwrap().push(uri.clone());
                let data = if uri.starts_with("/api/v1/auth/login") {
                    json!({"access_token": "admin"})
                } else if uri.starts_with("/api/v1/admin/accounts?") {
                    json!({"items": [
                        {"id": 7, "name": "claude-team", "platform": "anthropic", "type": "oauth", "status": "active"},
                        {"id": 8, "name": "child", "platform": "anthropic", "type": "oauth", "status": "active", "parent_account_id": 7}
                    ], "pages": 1})
                } else {
                    json!({"five_hour": {"utilization": 0, "resets_at": null, "remaining_seconds": 0},
                           "seven_day": {"utilization": 34, "resets_at": "2099-01-01T00:00:00Z"},
                           "subscription_tier": "MAX"})
                };
                Json(json!({"code": 0, "data": data}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn provider_form_derives_gateway_and_key_and_keeps_admin_url_separate() {
        let dir = tempfile::tempdir().unwrap();
        let app = Application::open_with_secrets(
            crate::Paths {
                data_dir: dir.path().join("data"),
                codex_auth: dir.path().join("codex/auth.json"),
            },
            std::sync::Arc::new(Secrets::default()),
        )
        .unwrap();
        let location = Location::files(dir.path().join("claude"));
        app.set_claude_location(location.clone());
        edit_env(&location, |env| {
            env.insert(BASE_URL.into(), "https://gateway.example".into());
            env.insert(AUTH_TOKEN.into(), "sk-from-claude".into());
        })
        .unwrap();
        let mut saved = app
            .claude_save_sub2api_provider(Provider {
                name: "team".into(),
                username: "admin".into(),
                password: "admin-password".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(saved.gateway(), "https://gateway.example");
        assert!(saved.has_api_key);
        assert!(saved.password.is_empty() && saved.api_key.is_empty());
        assert_eq!(
            app.claude_sub2api_connection(&saved).unwrap().admin_base,
            "https://gateway.example/api/v1"
        );

        saved.admin_base_url = "https://admin.example/api/v1/".into();
        saved = app.claude_save_sub2api_provider(saved).await.unwrap();
        let connection = app.claude_sub2api_connection(&saved).unwrap();
        assert_eq!(connection.admin_base, "https://admin.example/api/v1");
        assert_eq!(
            connection.definition.base_url.as_deref(),
            Some("https://gateway.example")
        );
        assert_eq!(connection.configuration.password, "admin-password");
        let stored = fs::read_to_string(app.claude_store_path()).unwrap();
        assert!(!stored.contains("sk-from-claude") && !stored.contains("admin-password"));

        // A credential for another gateway must never be copied to this provider.
        edit_env(&location, |env| {
            env.insert(BASE_URL.into(), "https://other.example".into());
            env.insert(AUTH_TOKEN.into(), "sk-other".into());
        })
        .unwrap();
        saved = app.claude_save_sub2api_provider(saved).await.unwrap();
        assert_eq!(
            app.secrets
                .get(&api_key_secret(&saved.id))
                .unwrap()
                .as_deref(),
            Some("sk-from-claude")
        );
        let other = app
            .claude_save_sub2api_provider(Provider {
                name: "unrelated".into(),
                base_url: "https://unrelated.example".into(),
                username: "admin".into(),
                password: "password".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(!other.has_api_key);
        saved.admin_base_url = "https://user:password@admin.example".into();
        assert_eq!(
            app.claude_save_sub2api_provider(saved).await.unwrap_err(),
            "error.sub2api.invalid_base_url"
        );
    }

    #[tokio::test]
    async fn syncs_claude_accounts_and_switches_claude_code_between_gateway_and_login() {
        let dir = tempfile::tempdir().unwrap();
        let app = Application::open_with_secrets(
            crate::Paths {
                data_dir: dir.path().join("data"),
                codex_auth: dir.path().join("codex/auth.json"),
            },
            std::sync::Arc::new(Secrets::default()),
        )
        .unwrap();
        let location = Location::files(dir.path().join("claude"));
        app.set_claude_location(location.clone());
        fs::create_dir_all(&location.config_dir).unwrap();
        fs::write(
            settings_path(&location),
            "{\n  \"model\": \"opus\",\n  \"env\": {\"ANTHROPIC_API_KEY\": \"old\", \"DISABLE_TELEMETRY\": \"1\"}\n}\n",
        )
        .unwrap();
        let requests = std::sync::Arc::new(Mutex::new(vec![]));
        let base = server(requests.clone()).await;
        assert_eq!(
            app.claude_sync_sub2api().await.unwrap_err(),
            "error.claude.sub2api_not_configured"
        );
        let saved = app
            .claude_save_sub2api_provider(Provider {
                name: "team".into(),
                base_url: format!("{base}/api/v1/"),
                username: "admin@example.com".into(),
                password: "secret".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(!saved.has_api_key);
        assert!(!fs::read_to_string(app.claude_store_path())
            .unwrap()
            .contains("secret"));
        assert_eq!(app.claude_sync_sub2api().await.unwrap(), 1);
        assert!(requests.lock().unwrap()[1].contains("platform=anthropic"));
        assert_eq!(
            requests.lock().unwrap()[2],
            "/api/v1/admin/accounts/7/usage"
        );

        let accounts = app.claude_accounts().await.unwrap();
        assert_eq!(accounts.len(), 1);
        let card = accounts[0].clone();
        assert_eq!(card.source_tag.as_deref(), Some("SUB2API"));
        assert_eq!(card.label, "claude-team");
        assert_eq!(card.plan_type.as_deref(), Some("max"));
        let usage = card.usage.as_ref().unwrap();
        assert!(usage.five_hour.is_none());
        assert_eq!(usage.one_week.as_ref().unwrap().used_percent, 34.0);
        assert!(!card.is_current);

        app.set_claude_network(std::sync::Arc::new(super::super::tests::Fake::default()));
        super::super::tests::sign_in(&location, "alice", 4_000_000_000_000);
        app.claude_import_current().await.unwrap();

        // A switch needs the API key Claude Code sends.
        assert_eq!(
            app.claude_switch_account(&card.id).await.unwrap_err(),
            "error.claude.sub2api_api_key_missing"
        );
        edit_env(&location, |env| {
            env.insert(BASE_URL.into(), base.clone().into());
            env.insert("ANTHROPIC_API_KEY".into(), "sk-team".into());
        })
        .unwrap();
        app.claude_save_sub2api_provider(saved.clone())
            .await
            .unwrap();
        let execution = app.claude_switch_account(&card.id).await.unwrap();
        assert_eq!(execution.provider.as_deref(), Some("team"));
        let text = fs::read_to_string(settings_path(&location)).unwrap();
        assert!(text.starts_with("{\n  \"model\": \"opus\",\n  \"env\": {"));
        let env = settings_env(&location);
        assert_eq!(env[BASE_URL], base.as_str());
        assert_eq!(env[AUTH_TOKEN], "sk-team");
        assert!(env.get("ANTHROPIC_API_KEY").is_none());
        assert_eq!(env["DISABLE_TELEMETRY"], "1");
        let accounts = app.claude_accounts().await.unwrap();
        assert!(
            accounts
                .iter()
                .find(|a| a.id == card.id)
                .unwrap()
                .is_current
        );
        assert!(accounts.iter().all(|a| a.is_current == (a.id == card.id)));

        // Back to an account login: only the gateway Copool set is removed.
        let alice = app
            .claude_accounts()
            .await
            .unwrap()
            .into_iter()
            .find(|a| a.source_tag.is_none())
            .unwrap();
        assert!(!alice.is_current);
        app.claude_switch_account(&alice.id).await.unwrap();
        let env = settings_env(&location);
        assert!(env.get(BASE_URL).is_none() && env.get(AUTH_TOKEN).is_none());
        assert_eq!(env["DISABLE_TELEMETRY"], "1");
        let accounts = app.claude_accounts().await.unwrap();
        assert!(
            accounts
                .iter()
                .find(|a| a.id == alice.id)
                .unwrap()
                .is_current
        );
        assert!(
            !accounts
                .iter()
                .find(|a| a.id == card.id)
                .unwrap()
                .is_current
        );

        app.claude_delete_account(&card.id).await.unwrap();
        assert_eq!(app.claude_accounts().await.unwrap().len(), 1);
    }
}
