//! Sub2api in the Claude pool: its Claude (anthropic) OAuth accounts as cards,
//! and Claude Code pointed at its gateway by merging the provider's settings
//! file over the official settings, as `claude --settings` would.
use super::{
    login_overrides, parse_settings, provider_config, put_settings, read_settings, render_settings,
    settings_source, Result, Store,
};
use crate::{
    application::{Application, SwitchExecution},
    codex_config::Definition,
    models::AccountSummary,
    sub2api::{self, Account, Platform},
    utils::now_unix_seconds,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const CARD_PREFIX: &str = "claude-sub2api-";

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
    /// The Claude Code settings file naming the gateway and its credential;
    /// empty when it is searched for by `name` on every use.
    pub settings_path: String,
    /// The form's Settings input, a name or a settings file path; only in a
    /// draft and in the listing, not stored.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub settings: String,
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
    /// The Settings input showing this provider's settings.
    fn settings_input(&self) -> String {
        if self.settings_path.is_empty() {
            self.name.clone()
        } else {
            self.settings_path.clone()
        }
    }
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

/// The cards of every provider; those of the `current` provider are current.
pub(super) fn summaries(store: &Store, current: Option<&str>) -> Vec<AccountSummary> {
    let mut result = vec![];
    for p in &store.providers {
        for a in &p.cached_accounts {
            let mut summary = a.summary();
            summary.id = p.card_id(a.id);
            summary.is_current = current == Some(p.id.as_str());
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

impl Application {
    /// Claude Sub2api providers, without their secrets.
    pub fn claude_sub2api_providers(&self) -> Result<Vec<Provider>> {
        let location = self.claude_location();
        let mut providers = self.claude_load()?.providers;
        for p in &mut providers {
            p.has_api_key = settings_source::load(&location, &p.name, &p.settings_path).is_ok();
            p.settings = p.settings_input();
        }
        Ok(providers)
    }
    pub async fn claude_save_sub2api_provider(&self, mut draft: Provider) -> Result<Provider> {
        // A path is kept; a name found by searching is searched for again.
        let input = match draft.settings.trim() {
            "" => draft.settings_input(),
            input => input.to_owned(),
        };
        (draft.name, draft.settings_path) = settings_source::parse(&input)?;
        draft.username = draft.username.trim().into();
        draft.base_url = draft.base_url.trim().trim_end_matches('/').into();
        draft.admin_base_url = draft.admin_base_url.trim().trim_end_matches('/').into();
        draft.proxy_url = draft.proxy_url.trim().into();
        let source =
            settings_source::load(&self.claude_location(), &draft.name, &draft.settings_path)?;
        draft.base_url = source.base_url.clone();
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
        // Model credentials remain in the source JSON, never inferred from
        // the current main settings or substituted with the admin token.
        draft.password.clear();
        draft.api_key.clear();
        draft.has_api_key = true;
        draft.settings.clear();
        match existing {
            Some(i) => store.providers[i] = draft.clone(),
            None => store.providers.push(draft.clone()),
        }
        self.claude_save(&store)?;
        draft.settings = draft.settings_input();
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
        let source = settings_source::load(&self.claude_location(), &p.name, &p.settings_path)?;
        let password = self
            .secrets
            .get(&password_secret(&p.id))?
            .ok_or("error.sub2api.configuration_incomplete")?;
        Ok(sub2api::Connection {
            account_request_proxy: String::new(),
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
                base_url: Some(gateway(&source.base_url)),
                wire_api: None,
                env_key: None,
                requires_openai_auth: None,
            },
            admin_base: sub2api::admin_base(if p.admin_base_url.is_empty() {
                &source.base_url
            } else {
                &p.admin_base_url
            })?,
        })
    }
    async fn claude_fetch_sub2api(
        &self,
        p: &Provider,
        ids: Option<&[i64]>,
        refresh: bool,
    ) -> Result<Vec<Account>> {
        let mut connection = self.claude_sub2api_connection(p)?;
        if refresh {
            connection.account_request_proxy = self.preferences()?.account_proxy_url;
        }
        let mut accounts = connection
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
            match self.claude_fetch_sub2api(&p, None, false).await {
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
        let fetched = self.claude_fetch_sub2api(&p, ids.as_deref(), true).await?;
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
    /// Merges the provider's settings file over the official main settings,
    /// recording these when leaving them; the file is read again each time.
    pub(super) async fn claude_switch_sub2api(&self, id: &str) -> Result<SwitchExecution> {
        let location = self.claude_location();
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        let (i, account) = card(&store, id).ok_or("Account not found")?;
        if !account.status.eq_ignore_ascii_case("active") {
            return Err("Account is deactivated".into());
        }
        let p = store.providers[i].clone();
        let source = settings_source::load(&location, &p.name, &p.settings_path)?;
        let settings = read_settings(&location)?;
        let current = parse_settings(settings.as_deref())?;
        let leaving_official = provider_config::overrides(&current).is_empty();
        let recorded = self.claude_official_settings()?;
        // Without a record, other settings lose only what replaces the login.
        let mut merged = match (&recorded, leaving_official) {
            (Some(official), false) => parse_settings(official.as_deref())?,
            (None, false) => {
                let mut stripped = current;
                provider_config::remove_overrides(&mut stripped);
                stripped
            }
            (_, true) => current,
        };
        provider_config::merge(&mut merged, &source.document);
        let merged = render_settings(&merged, settings.as_deref())?;
        if leaving_official {
            self.claude_record_official(settings.as_deref())?;
        }
        let undo = || {
            let _ = put_settings(&location, settings.as_deref());
            if leaving_official {
                let _ = match &recorded {
                    Some(previous) => self.claude_record_official(previous.as_deref()),
                    None => {
                        std::fs::remove_file(self.claude_official_path()).map_err(|e| e.to_string())
                    }
                };
            }
        };
        if let Err(error) = put_settings(&location, Some(&merged)) {
            undo();
            return Err(error);
        }
        store.settings_provider = Some(p.id.clone());
        if let Err(error) = self.claude_save(&store) {
            undo();
            return Err(error);
        }
        Ok(SwitchExecution {
            provider: Some(p.name.clone()),
            login_overrides: login_overrides(&location)
                .into_iter()
                .filter(|k| k != source.auth_key)
                .collect(),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{tests::sign_in, tests::Fake, Location};
    use super::*;
    use serde_json::{json, Value};
    use std::{collections::HashMap, fs, path::PathBuf, sync::Mutex};

    const BASE_URL: &str = "ANTHROPIC_BASE_URL";
    const AUTH_TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";

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
                if !uri.starts_with("/api/v1/auth/login") {
                    assert_eq!(request.headers().get("authorization").unwrap(), "Bearer admin");
                }
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

    /// A provider settings file outside the Claude configuration directory.
    fn source_file(dir: &std::path::Path, base: &str, key_name: &str, key: &str) -> PathBuf {
        let path = dir.join("providers/team.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            json!({"model": "source-model", "permissions": {"allow": ["Edit"]},
                "env": {BASE_URL: base, key_name: key}})
            .to_string(),
        )
        .unwrap();
        path
    }
    fn main_path(location: &Location) -> PathBuf {
        location.config_dir.join("settings.json")
    }
    fn main_settings(location: &Location) -> Value {
        serde_json::from_str(&fs::read_to_string(main_path(location)).unwrap()).unwrap()
    }
    fn write_main(location: &Location, settings: &Value) {
        fs::create_dir_all(&location.config_dir).unwrap();
        fs::write(main_path(location), settings.to_string()).unwrap();
    }
    fn current(accounts: &[AccountSummary]) -> Vec<String> {
        accounts
            .iter()
            .filter(|a| a.is_current)
            .map(|a| a.id.clone())
            .collect()
    }
    fn open(dir: &std::path::Path) -> (Application, Location) {
        let app = Application::open_with_secrets(
            crate::Paths {
                data_dir: dir.join("data"),
                codex_auth: dir.join("codex/auth.json"),
            },
            std::sync::Arc::new(Secrets::default()),
        )
        .unwrap();
        let location = Location::files(dir.join("claude"));
        app.set_claude_location(location.clone());
        (app, location)
    }
    /// A synced provider, and an imported login as the official account.
    async fn pool(
        dir: &std::path::Path,
    ) -> (Application, Location, String, PathBuf, String, String) {
        let (app, location) = open(dir);
        let base = server(std::sync::Arc::new(Mutex::new(vec![]))).await;
        let path = source_file(dir, &base, AUTH_TOKEN, "sk-team");
        let saved = app
            .claude_save_sub2api_provider(Provider {
                settings: path.to_str().unwrap().into(),
                username: "admin@example.com".into(),
                password: "secret".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        app.claude_sync_sub2api().await.unwrap();
        app.set_claude_network(std::sync::Arc::new(Fake::default()));
        sign_in(&location, "alice", 4_000_000_000_000);
        app.claude_import_current().await.unwrap();
        let alice = app
            .claude_accounts()
            .await
            .unwrap()
            .into_iter()
            .find(|a| a.source_tag.is_none())
            .unwrap()
            .id;
        (app, location, base, path, saved.card_id(7), alice)
    }

    #[tokio::test]
    async fn provider_form_derives_gateway_and_key_and_keeps_admin_url_separate() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location) = open(dir.path());
        write_main(
            &location,
            &json!({"env": {BASE_URL: "https://gateway.example", AUTH_TOKEN: "sk-from-claude"}}),
        );
        let path = source_file(
            dir.path(),
            "https://gateway.example",
            AUTH_TOKEN,
            "sk-from-source",
        );
        let draft = Provider {
            username: "admin".into(),
            password: "admin-password".into(),
            ..Default::default()
        };
        // The settings file is required, never taken from the main settings.
        assert_eq!(
            app.claude_save_sub2api_provider(draft.clone())
                .await
                .unwrap_err(),
            "error.sub2api.configuration_incomplete"
        );
        let mut saved = app
            .claude_save_sub2api_provider(Provider {
                settings: format!(" {} ", path.display()),
                ..draft
            })
            .await
            .unwrap();
        assert_eq!(saved.name, "team");
        assert_eq!(saved.settings_path, path.to_str().unwrap());
        assert_eq!(saved.settings, saved.settings_path);
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
        assert!(!stored.contains("sk-from") && !stored.contains("admin-password"));
        assert!(app
            .claude_save_sub2api_provider(Provider {
                settings: dir.path().join("missing.json").to_str().unwrap().into(),
                username: "admin".into(),
                password: "password".into(),
                ..Default::default()
            })
            .await
            .unwrap_err()
            .contains("Cannot read"));
        saved.admin_base_url = "https://user:password@admin.example".into();
        assert_eq!(
            app.claude_save_sub2api_provider(saved).await.unwrap_err(),
            "error.sub2api.invalid_base_url"
        );
    }

    #[tokio::test]
    async fn switches_merge_the_provider_over_the_official_settings_and_restore_them() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, base, path, card, alice) = pool(dir.path()).await;
        let relay = "http://127.0.0.1:8787/anthropic";
        let official = format!(
            "{{\n  \"model\": \"opus\",\n  \"permissions\": {{\"allow\": [\"Read\"]}},\n  \"env\": {{\"{BASE_URL}\": \"{relay}\", \"{AUTH_TOKEN}\": \"\", \"DISABLE_TELEMETRY\": \"1\"}}\n}}\n"
        );
        fs::write(main_path(&location), &official).unwrap();
        // Settings without a replacing credential are official, at any address.
        assert_eq!(
            current(&app.claude_accounts().await.unwrap()),
            [alice.clone()]
        );

        // A missing settings file fails without changing anything.
        let source_raw = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(app.claude_switch_account(&card).await.is_err());
        assert_eq!(fs::read_to_string(main_path(&location)).unwrap(), official);
        assert!(!app.claude_official_path().exists());
        fs::write(&path, &source_raw).unwrap();

        let execution = app.claude_switch_account(&card).await.unwrap();
        assert_eq!(execution.provider.as_deref(), Some("team"));
        assert!(execution.login_overrides.is_empty());
        assert_eq!(
            main_settings(&location),
            json!({"model": "source-model", "permissions": {"allow": ["Read", "Edit"]},
                "env": {BASE_URL: base, AUTH_TOKEN: "sk-team", "DISABLE_TELEMETRY": "1"}})
        );
        assert_eq!(fs::read(&path).unwrap(), source_raw);
        assert_eq!(
            current(&app.claude_accounts().await.unwrap()),
            [card.clone()]
        );

        // Provider to provider merges over the recorded official settings,
        // and the file is read again: the old credential does not remain.
        let mut edited = main_settings(&location);
        edited["statusLine"] = json!({"type": "command"});
        write_main(&location, &edited);
        source_file(dir.path(), &base, "ANTHROPIC_API_KEY", "rotated-model-key");
        let execution = app.claude_switch_account(&card).await.unwrap();
        assert!(execution.login_overrides.is_empty());
        assert_eq!(
            main_settings(&location)["env"],
            json!({BASE_URL: base, AUTH_TOKEN: "", "DISABLE_TELEMETRY": "1",
                "ANTHROPIC_API_KEY": "rotated-model-key"})
        );
        assert_eq!(
            current(&app.claude_accounts().await.unwrap()),
            [card.clone()]
        );

        // Back to the login: the recorded settings return verbatim.
        let execution = app.claude_switch_account(&alice).await.unwrap();
        assert!(execution.login_overrides.is_empty());
        assert_eq!(fs::read_to_string(main_path(&location)).unwrap(), official);
        assert_eq!(
            current(&app.claude_accounts().await.unwrap()),
            [alice.clone()]
        );
        // Between logins the settings are left alone.
        let mut changed = main_settings(&location);
        changed["theme"] = "dark".into();
        write_main(&location, &changed);
        app.claude_switch_account(&alice).await.unwrap();
        assert_eq!(main_settings(&location), changed);

        // A credential written outside Copool takes Claude Code off the login.
        let outside = json!({"apiKeyHelper": "helper", "theme": "dark",
            "env": {AUTH_TOKEN: "outside", "DISABLE_TELEMETRY": "1"}});
        write_main(&location, &outside);
        assert!(current(&app.claude_accounts().await.unwrap()).is_empty());
        // Without a record, only what replaces the login goes.
        fs::remove_file(app.claude_official_path()).unwrap();
        app.claude_switch_account(&alice).await.unwrap();
        assert_eq!(
            main_settings(&location),
            json!({"theme": "dark", "env": {"DISABLE_TELEMETRY": "1"}})
        );
        assert!(!app.claude_official_path().exists());
        write_main(&location, &outside);
        app.claude_switch_account(&card).await.unwrap();
        assert_eq!(
            main_settings(&location),
            json!({"theme": "dark", "env": {"DISABLE_TELEMETRY": "1",
                BASE_URL: base, "ANTHROPIC_API_KEY": "rotated-model-key"},
                "model": "source-model", "permissions": {"allow": ["Edit"]}})
        );
        assert!(!app.claude_official_path().exists());
        app.claude_delete_account(&card).await.unwrap();
        assert_eq!(app.claude_accounts().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_settings_name_is_searched_for_on_every_use_and_never_stored() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location) = open(dir.path());
        let base = server(std::sync::Arc::new(Mutex::new(vec![]))).await;
        let found = location.config_dir.join("profiles/team-settings.json");
        fs::create_dir_all(found.parent().unwrap()).unwrap();
        let write = |path: &PathBuf, key: &str| {
            fs::write(
                path,
                json!({"env": {BASE_URL: base, AUTH_TOKEN: key}}).to_string(),
            )
            .unwrap()
        };
        write(&found, "sk-found");
        let saved = app
            .claude_save_sub2api_provider(Provider {
                settings: "team".into(),
                username: "admin@example.com".into(),
                password: "secret".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            (saved.name.as_str(), saved.settings_path.as_str()),
            ("team", "")
        );
        assert_eq!(saved.settings, "team");
        let stored = fs::read_to_string(app.claude_store_path()).unwrap();
        assert!(!stored.contains("team-settings.json") && !stored.contains("\"settings\""));
        assert_eq!(app.claude_sub2api_providers().unwrap()[0].settings, "team");
        app.claude_sync_sub2api().await.unwrap();
        // The file moved: the next switch searches again.
        let moved = location.config_dir.join("other/settings-team.json");
        fs::create_dir_all(moved.parent().unwrap()).unwrap();
        fs::rename(&found, &moved).unwrap();
        write(&moved, "sk-moved");
        app.claude_switch_account(&saved.card_id(7)).await.unwrap();
        assert_eq!(main_settings(&location)["env"][AUTH_TOKEN], "sk-moved");
        // A path chosen later is kept instead.
        let picked = dir.path().join("elsewhere/team.json");
        fs::create_dir_all(picked.parent().unwrap()).unwrap();
        write(&picked, "sk-picked");
        let saved = app
            .claude_save_sub2api_provider(Provider {
                settings: picked.to_str().unwrap().into(),
                ..saved
            })
            .await
            .unwrap();
        assert_eq!(saved.settings_path, picked.to_str().unwrap());
        app.claude_switch_account(&saved.card_id(7)).await.unwrap();
        assert_eq!(main_settings(&location)["env"][AUTH_TOKEN], "sk-picked");
    }

    #[tokio::test]
    async fn a_missing_official_settings_file_is_restored_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, _, _, card, alice) = pool(dir.path()).await;
        assert!(!main_path(&location).exists());
        app.claude_switch_account(&card).await.unwrap();
        assert!(main_path(&location).exists());
        app.claude_switch_account(&alice).await.unwrap();
        assert!(!main_path(&location).exists());
        assert_eq!(current(&app.claude_accounts().await.unwrap()), [alice]);
    }
}
