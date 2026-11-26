//! Sub2api in the Claude pool: its Claude (anthropic) OAuth accounts as cards,
//! and Claude Code pointed at its gateway by merging the provider's settings
//! file over the official settings, as `claude --settings` would. A provider
//! given instead by its Sub2api URL and the variable holding its key is
//! merged as such a file: its gateway, and an `apiKeyHelper` printing the
//! variable, so that the key stays in the user's environment. What the
//! pools do alike with their providers is in `provider_registry.rs`; here is where
//! the Claude pool keeps them and how it uses one.
use super::{
    login_overrides, official_base, parse_settings, provider_config, put_settings, read_settings,
    render_settings, settings_source, without_login_replacement, Official, Result,
};
use crate::{
    application::{Application, SwitchExecution},
    pool::Pool,
    provider_registry::{embeds_record, upstream, Adapter, ProviderRecord, Route},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The mode of a provider given by its Sub2api URL; any other, a settings file.
const ADDRESS: &str = "address";
/// The variable an address's key is in, unless named.
const API_KEY: &str = "CLAUDE_SUB2API_API_KEY";
pub(crate) const API_KEY_MISSING: &str = "error.claude.sub2api_api_key_variable_missing";
pub(crate) const API_KEY_VARIABLE_INVALID: &str = "error.claude.api_key_variable_invalid";
const CONFIGURATION_INCOMPLETE: &str = "error.sub2api.configuration_incomplete";

/// A Claude provider. Its address (`baseURL`) is the Sub2api address: the
/// gateway root, or its admin API (`…/api/v1`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Provider {
    #[serde(flatten)]
    pub record: ProviderRecord,
    /// `"address"` for a provider given by its Sub2api URL and the variable
    /// holding its key; none for a settings file, as saved before there was
    /// a choice.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    /// The Claude Code settings file naming the gateway and its credential;
    /// empty when it is searched for by `name` on every use.
    pub settings_path: String,
    /// The form's Settings input, a name or a settings file path; only in a
    /// draft and in the listing, not stored.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub settings: String,
    /// The Sub2api API key Claude Code sends; only in a draft.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    pub has_api_key: bool,
    /// For an address, the variable holding the Sub2api API key Claude Code
    /// sends, as a new shell exports it: `CLAUDE_SUB2API_API_KEY` unless
    /// named. Never the key.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key_variable: String,
}
embeds_record!(Provider);
impl Provider {
    /// Whether the provider is its Sub2api URL and key variable.
    fn by_address(&self) -> bool {
        self.mode == ADDRESS
    }
    /// The Settings input showing this provider's settings; none for an
    /// address.
    fn settings_input(&self) -> String {
        if self.by_address() {
            String::new()
        } else if self.settings_path.is_empty() {
            self.name.clone()
        } else {
            self.settings_path.clone()
        }
    }
    /// The root Claude Code sends its requests to (`ANTHROPIC_BASE_URL`).
    fn gateway(&self) -> String {
        gateway(&self.base_url)
    }
}
pub(super) fn gateway(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    url.strip_suffix("/api/v1")
        .or_else(|| url.strip_suffix("/v1"))
        .unwrap_or(url)
        .trim_end_matches('/')
        .to_owned()
}
/// The variable holding an address's key.
pub(super) fn key_variable(p: &Provider) -> &str {
    match p.api_key_variable.as_str() {
        "" => API_KEY,
        name => name,
    }
}
/// What a provider given by its address merges over Claude Code's settings:
/// its gateway, and a helper printing the key its variable holds, which
/// Claude Code sends as a bearer token.
fn address_settings(p: &Provider) -> Value {
    json!({
        "env": {"ANTHROPIC_BASE_URL": p.base_url},
        "apiKeyHelper": format!("printf '%s' \"${}\"", key_variable(p)),
    })
}

/// The Claude pool's providers, for its Provider registry.
pub(crate) struct ProviderAdapter<'a>(pub(crate) &'a Application);
impl Adapter for ProviderAdapter<'_> {
    type Provider = Provider;
    const POOL: Pool = Pool::Claude;
    fn load_providers(&self) -> Result<Vec<Provider>> {
        // Provider metadata does not need the official accounts' vault tokens.
        Ok(self.0.claude_store_file()?.providers)
    }
    async fn edit_providers<T: Send>(
        &self,
        edit: impl FnOnce(&mut Vec<Provider>) -> Result<T> + Send,
    ) -> Result<T> {
        self.0.claude_edit_providers(edit).await
    }
    fn prepare(&self, draft: &mut Provider) -> Result<()> {
        if draft.by_address() {
            // Claude Code's gateway is the Sub2api root; the admin API only
            // overrides.
            draft.base_url = gateway(&draft.base_url);
            if draft.base_url.is_empty() {
                return Err(CONFIGURATION_INCOMPLETE.into());
            }
            draft.api_key_variable = match draft.api_key_variable.trim() {
                "" => API_KEY.into(),
                name if crate::proxy_integration::is_variable(name) => name.into(),
                // A key typed here would be kept in plain text: never.
                _ => return Err(API_KEY_VARIABLE_INVALID.into()),
            };
            draft.settings_path.clear();
        } else {
            draft.mode.clear();
            draft.api_key_variable.clear();
            // A path is kept; a name found by searching is searched for again.
            let input = match draft.settings.trim() {
                "" => draft.settings_input(),
                input => input.to_owned(),
            };
            (draft.name, draft.settings_path) = settings_source::parse(&input)?;
            let source = settings_source::load(
                &self.0.claude_location(),
                &draft.name,
                &draft.settings_path,
            )?;
            draft.base_url = source.base_url;
        }
        // Model credentials remain in the source JSON or the user's
        // environment, never inferred from the current main settings or
        // substituted with the admin token.
        draft.api_key.clear();
        draft.has_api_key = true;
        draft.settings.clear();
        Ok(())
    }
    fn route(&self, p: &Provider) -> Result<Route> {
        // An address's admin API is the Sub2api one in it, reached directly.
        if p.by_address() {
            return Ok(Route {
                gateway: gateway(&p.base_url),
                sub2api: upstream(&p.base_url).to_owned(),
            });
        }
        let source = settings_source::load(&self.0.claude_location(), &p.name, &p.settings_path)?;
        Ok(Route {
            gateway: gateway(&source.base_url),
            sub2api: source.base_url,
        })
    }
    fn present(&self, p: &mut Provider) {
        // An address's key is looked for when it is used.
        p.has_api_key = p.by_address()
            || settings_source::load(&self.0.claude_location(), &p.name, &p.settings_path).is_ok();
        p.settings = p.settings_input();
    }
}

impl Application {
    /// Edits the providers alone: the official accounts' tokens stay as
    /// stored, the vault neither read nor rewritten.
    async fn claude_edit_providers<T>(
        &self,
        edit: impl FnOnce(&mut Vec<Provider>) -> Result<T>,
    ) -> Result<T> {
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_store_file()?;
        let result = edit(&mut store.providers)?;
        crate::persistence::write_json(&self.claude_store_path(), &store)?;
        self.publish();
        Ok(result)
    }
    /// What a provider's switch merges over Claude Code's settings: its
    /// settings file, read again each time, or what its address gives once
    /// a new shell exports the variable holding its key.
    async fn claude_provider_settings(
        &self,
        location: &super::Location,
        p: &Provider,
    ) -> Result<settings_source::Source> {
        if !p.by_address() {
            return settings_source::load_off_runtime(
                location.clone(),
                p.name.clone(),
                p.settings_path.clone(),
            )
            .await;
        }
        // A variable nothing exports would leave Claude Code without a key.
        if self
            .claude_exported_now(key_variable(p))
            .await?
            .is_none_or(|key| key.trim().is_empty())
        {
            return Err(API_KEY_MISSING.into());
        }
        Ok(settings_source::Source {
            document: address_settings(p),
            base_url: p.base_url.clone(),
            auth_key: "apiKeyHelper",
        })
    }
    /// Merges the provider's settings over the official main settings,
    /// recording these when leaving them.
    pub(super) async fn claude_switch_sub2api(&self, id: &str) -> Result<SwitchExecution> {
        let location = self.claude_location();
        // The settings are found before the pool is locked.
        let (member, account) = self.card(Pool::Claude, id)?;
        let found = self
            .claude_store_file()?
            .providers
            .into_iter()
            .find(|p| p.id == member.key)
            .ok_or("Account not found")?;
        let source = self.claude_provider_settings(&location, &found).await?;
        if !account.status.eq_ignore_ascii_case("active") {
            return Err("Account is deactivated".into());
        }
        let _lock = self.claude.lock.lock().await;
        let mut store = self.claude_load()?;
        let p = store
            .providers
            .iter()
            .find(|p| p.id == found.id)
            .ok_or("Account not found")?
            .clone();
        let given = |p: &Provider| {
            (
                p.mode.clone(),
                p.name.clone(),
                p.settings_path.clone(),
                p.base_url.clone(),
                p.api_key_variable.clone(),
            )
        };
        if given(&p) != given(&found) {
            return Err("The Claude provider changed during the switch; switch again".into());
        }
        let settings = read_settings(&location)?;
        let current = parse_settings(settings.as_deref())?;
        let recorded = self.claude_official_settings()?;
        // A record not yet restored keeps what changed since its provider was
        // applied, its credential removed or not. Without one, official
        // settings are recorded as they are, and others lose only what
        // replaces the login and its gateway.
        let base = match &recorded {
            Some(official) => official_base(official, &current, settings.as_deref())?,
            None if provider_config::overrides(&current).is_empty() => settings.clone(),
            None => {
                let stripped = without_login_replacement(current);
                Some(render_settings(&stripped, settings.as_deref())?)
            }
        };
        let mut merged = parse_settings(base.as_deref())?;
        provider_config::merge(&mut merged, &source.document);
        let merged = render_settings(&merged, settings.as_deref())?;
        // Leaving the provider drops what it added, and keeps what changed since.
        self.claude_record_official(&Official {
            settings: base,
            applied: Some(merged.clone()),
        })?;
        let undo = || {
            let _ = put_settings(&location, settings.as_deref());
            let _ = match &recorded {
                Some(previous) => self.claude_record_official(previous),
                None => {
                    std::fs::remove_file(self.claude_official_path()).map_err(|e| e.to_string())
                }
            };
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
    use crate::models::AccountSummary;
    /// Syncs every provider, as choosing each in the import menu would.
    async fn sync_all(app: &Application) {
        for p in listed(&app) {
            app.refresh_provider(Pool::Claude, &p.id, true)
                .await
                .unwrap();
        }
    }
    use super::super::{tests::sign_in, tests::Fake, Location};
    use super::*;

    /// A form saved through the pool's Provider registry, as the settings save it.
    async fn save(app: &Application, draft: Provider) -> Result<Provider> {
        let draft = serde_json::to_value(draft).unwrap();
        let saved = app.registry(Pool::Claude).unwrap().save(draft).await?;
        Ok(serde_json::from_value(saved).unwrap())
    }
    /// The providers as the settings list them.
    fn listed(app: &Application) -> Vec<Provider> {
        let listed = app.registry(Pool::Claude).unwrap().providers().unwrap();
        serde_json::from_value(listed).unwrap()
    }
    fn connection(app: &Application, key: &str) -> Result<crate::sub2api::Connection> {
        app.registry(Pool::Claude).unwrap().connection(key)
    }
    use serde_json::{json, Value};
    use std::{collections::HashMap, fs, path::PathBuf, sync::Mutex};

    const BASE_URL: &str = "ANTHROPIC_BASE_URL";
    const AUTH_TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";

    #[derive(Default)]
    struct Secrets {
        values: Mutex<HashMap<String, String>>,
        gets: Mutex<Vec<String>>,
    }
    impl Secrets {
        /// The secret ids read since the last `clear_reads`.
        fn reads(&self) -> Vec<String> {
            self.gets.lock().unwrap().clone()
        }
        fn clear_reads(&self) {
            self.gets.lock().unwrap().clear();
        }
    }
    impl crate::credentials::SecretStore for Secrets {
        fn get(&self, id: &str) -> Result<Option<String>> {
            self.gets.lock().unwrap().push(id.into());
            Ok(self.values.lock().unwrap().get(id).cloned())
        }
        fn set(&self, id: &str, value: &str) -> Result<()> {
            self.values.lock().unwrap().insert(id.into(), value.into());
            Ok(())
        }
        fn remove(&self, id: &str) -> Result<()> {
            self.values.lock().unwrap().remove(id);
            Ok(())
        }
    }
    /// A Sub2api admin API with one Claude account, recording request paths.
    async fn server(requests: std::sync::Arc<Mutex<Vec<String>>>) -> String {
        use axum::{extract::Request, response::IntoResponse, routing::any, Json, Router};
        let router = Router::new().fallback(any(move |request: Request| {
            let requests = requests.clone();
            async move {
                let uri = request.uri().path_and_query().unwrap().to_string();
                requests.lock().unwrap().push(uri.clone());
                if !uri.starts_with("/api/v1/auth/login") {
                    assert_eq!(request.headers().get("authorization").unwrap(), "Bearer admin");
                }
                if uri.ends_with("/test") {
                    let body = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .unwrap();
                    let body: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(body["model_id"], crate::claude::HELLO_MODEL);
                    return concat!(
                        "data: {\"type\":\"test_start\",\"model\":\"claude\"}\n\n",
                        "data: {\"type\":\"content\",\"text\":\"Hi\"}\n\n",
                        "data: {\"type\":\"test_complete\",\"success\":true}\n\n",
                    )
                    .into_response();
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
                Json(json!({"code": 0, "data": data})).into_response()
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
        let (app, location, _) = open_recording(dir);
        (app, location)
    }
    /// `open`, keeping the handle that records secret reads.
    fn open_recording(dir: &std::path::Path) -> (Application, Location, std::sync::Arc<Secrets>) {
        let secrets = std::sync::Arc::new(Secrets::default());
        let app = Application::open_with_secrets(
            crate::Paths {
                data_dir: dir.join("data"),
                codex_auth: dir.join("codex/auth.json"),
            },
            secrets.clone(),
        )
        .unwrap();
        let location = Location::files(dir.join("claude"));
        app.set_claude_location(location.clone());
        (app, location, secrets)
    }
    #[tokio::test]
    async fn selected_sync_leaves_other_providers_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _, _, _, _, _) = pool(dir.path()).await;
        let mut store = app.claude_load().unwrap();
        let id = store.providers[0].id.clone();
        let mut other = store.providers[0].clone();
        other.id = "unselected".into();
        other.name = "Unselected".into();
        other.admin_base_url = "http://127.0.0.1:1".into();
        let before = serde_json::to_value(&other).unwrap();
        store.providers.push(other);
        app.claude_save(&store).unwrap();
        app.refresh_provider(Pool::Claude, &id, true).await.unwrap();
        let after = app.claude_load().unwrap();
        assert_eq!(serde_json::to_value(&after.providers[1]).unwrap(), before);
        assert!(app
            .refresh_provider(Pool::Claude, "missing", true)
            .await
            .is_err());
    }
    #[tokio::test]
    async fn provider_listing_and_sync_do_not_access_official_vault() {
        struct UnavailableVault;
        impl crate::vault::Vault for UnavailableVault {
            fn read(&self, _: crate::vault::KeychainAccess) -> Result<Option<String>> {
                panic!("Provider metadata must not read official tokens");
            }
            fn write(&self, _: &str, _: crate::vault::KeychainAccess) -> Result<()> {
                panic!("Provider metadata must not rewrite official tokens");
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let (app, _, _, _, _, _) = pool(dir.path()).await;
        let mut store = app.claude_store_file().unwrap();
        assert!(!store.accounts.is_empty());
        store
            .accounts
            .iter_mut()
            .for_each(super::super::strip_secrets);
        crate::persistence::write_json(&app.claude_store_path(), &store).unwrap();
        let before = serde_json::to_value(&store.accounts).unwrap();
        app.set_claude_vault(std::sync::Arc::new(UnavailableVault));
        let providers = listed(&app);
        app.refresh_provider(Pool::Claude, &providers[0].id, true)
            .await
            .unwrap();
        let after = app.claude_store_file().unwrap();
        assert_eq!(serde_json::to_value(&after.accounts).unwrap(), before);
        assert!(!after.providers[0].cached_accounts.is_empty());
    }
    /// A synced provider, and an imported login as the official account.
    async fn pool(
        dir: &std::path::Path,
    ) -> (Application, Location, String, PathBuf, String, String) {
        let (app, location, base, path, card, alice, _, _) = pool_recording(dir).await;
        (app, location, base, path, card, alice)
    }
    /// `pool`, plus the handles showing what it read and requested.
    #[allow(clippy::type_complexity)]
    async fn pool_recording(
        dir: &std::path::Path,
    ) -> (
        Application,
        Location,
        String,
        PathBuf,
        String,
        String,
        std::sync::Arc<Secrets>,
        std::sync::Arc<Mutex<Vec<String>>>,
    ) {
        let requests = std::sync::Arc::new(Mutex::new(vec![]));
        let (app, location, secrets) = open_recording(dir);
        let base = server(requests.clone()).await;
        let path = source_file(dir, &base, AUTH_TOKEN, "sk-team");
        let saved = save(
            &app,
            Provider {
                record: ProviderRecord {
                    username: "admin@example.com".into(),
                    password: "secret".into(),
                    ..Default::default()
                },
                settings: path.to_str().unwrap().into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        sync_all(&app).await;
        app.set_claude_network(std::sync::Arc::new(Fake::default()));
        sign_in(&location, "alice", 4_000_000_000_000);
        app.claude_import_current().await.unwrap();
        let alice = app
            .accounts(Pool::Claude)
            .await
            .unwrap()
            .into_iter()
            .find(|a| a.source_tag.is_none())
            .unwrap()
            .id;
        (
            app,
            location,
            base,
            path,
            crate::sub2api::card_id(Pool::Claude, &saved.id, 7),
            alice,
            secrets,
            requests,
        )
    }
    /// A second synced provider on its own server, with its request log.
    async fn add_provider(
        app: &Application,
        dir: &std::path::Path,
        password: &str,
    ) -> (Provider, std::sync::Arc<Mutex<Vec<String>>>) {
        let requests = std::sync::Arc::new(Mutex::new(vec![]));
        let base = server(requests.clone()).await;
        let path = dir.join("providers/other.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            json!({"env": {BASE_URL: base, AUTH_TOKEN: "sk-other"}}).to_string(),
        )
        .unwrap();
        let saved = save(
            &app,
            Provider {
                record: ProviderRecord {
                    username: "other@example.com".into(),
                    password: password.into(),
                    ..Default::default()
                },
                settings: path.to_str().unwrap().into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        sync_all(&app).await;
        (saved, requests)
    }
    fn logins(requests: &std::sync::Arc<Mutex<Vec<String>>>) -> usize {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|uri| uri.starts_with("/api/v1/auth/login"))
            .count()
    }
    fn listings(requests: &std::sync::Arc<Mutex<Vec<String>>>) -> usize {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|uri| uri.starts_with("/api/v1/admin/accounts?"))
            .count()
    }

    #[tokio::test]
    async fn a_card_sub2api_stops_serving_says_why_instead_of_its_last_error() {
        use axum::{extract::Request, routing::any, Json, Router};
        let items = std::sync::Arc::new(Mutex::new(json!([
            {"id": 7, "name": "claude-team", "platform": "anthropic", "type": "oauth", "status": "active",
             "extra": {"email_address": "upstream@example.com", "email": "other@example.com"}}
        ])));
        let queries = std::sync::Arc::new(Mutex::new(vec![]));
        let (listed, asked) = (items.clone(), queries.clone());
        let router = Router::new().fallback(any(move |request: Request| {
            let (listed, asked) = (listed.clone(), asked.clone());
            async move {
                let uri = request.uri().path_and_query().unwrap().to_string();
                let data = if uri.starts_with("/api/v1/auth/login") {
                    json!({"access_token": "admin"})
                } else if uri.starts_with("/api/v1/admin/accounts?") {
                    asked.lock().unwrap().push(uri);
                    json!({"items": listed.lock().unwrap().clone(), "pages": 1})
                } else {
                    json!({"seven_day": {"utilization": 34, "resets_at": null}})
                };
                Json(json!({"code": 0, "data": data}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let (app, _) = open(dir.path());
        let path = source_file(dir.path(), &base, AUTH_TOKEN, "sk-team");
        let saved = save(
            &app,
            Provider {
                record: ProviderRecord {
                    username: "admin@example.com".into(),
                    password: "secret".into(),
                    ..Default::default()
                },
                settings: path.to_str().unwrap().into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        sync_all(&app).await;
        let card = crate::sub2api::card_id(Pool::Claude, &saved.id, 7);
        let shown = |app: &Application| {
            let summary = app.provider_cards(Pool::Claude, None).unwrap().remove(0);
            assert_eq!(summary.email.as_deref(), Some("upstream@example.com"));
            assert_eq!(summary.label, "upstream@example.com");
            (
                summary.workspace_status,
                summary.usage.is_some(),
                summary.usage_error,
            )
        };
        app.refresh_usage(Pool::Claude, &card).await.unwrap();
        assert_eq!(shown(&app), ("active".into(), true, None));
        // Only the full listing asks for active accounts.
        let asked = queries.lock().unwrap().clone();
        assert!(asked[0].contains("status=active"));
        assert!(!asked.last().unwrap().contains("status="));

        // A provider refresh discovers a second account and fetches its usage immediately.
        items.lock().unwrap().as_array_mut().unwrap().push(json!({
            "id": 9, "name": "new-account", "platform": "anthropic",
            "type": "oauth", "status": "active"
        }));
        app.refresh_provider(Pool::Claude, &saved.id, false)
            .await
            .unwrap();
        let store = app.claude_load().unwrap();
        assert_eq!(store.providers[0].imported_account_ids, [7, 9]);
        let added = store.providers[0]
            .cached_accounts
            .iter()
            .find(|a| a.id == 9)
            .unwrap();
        assert!(added.usage.is_some());
        assert!(added.usage_error.is_none());
        assert_eq!(added.summary("9".into()).label, "new-account");

        *items.lock().unwrap() = json!([{"id": 7, "name": "claude-team", "platform": "anthropic",
            "extra": {"email_address": "upstream@example.com"},
            "type": "oauth", "status": "error", "error_message": "Token expired"}]);
        app.refresh_usage(Pool::Claude, &card).await.unwrap();
        assert_eq!(
            shown(&app),
            ("deactivated".into(), false, Some("Token expired".into()))
        );

        *items.lock().unwrap() = json!([]);
        app.refresh_provider(Pool::Claude, &saved.id, false)
            .await
            .unwrap();
        assert_eq!(
            shown(&app),
            (
                "deactivated".into(),
                false,
                Some("The account is no longer in Sub2api".into())
            )
        );
    }

    #[tokio::test]
    async fn a_card_says_hi_through_the_sub2api_account_test() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _, _, _, card, _, _, requests) = pool_recording(dir.path()).await;
        requests.lock().unwrap().clear();
        app.say_hi(Pool::Claude, &card).await.unwrap();
        // The usage, which shows the window only later, is left to the card;
        // the login of the earlier sync is reused.
        assert_eq!(*requests.lock().unwrap(), ["/api/v1/admin/accounts/7/test"]);
    }

    #[tokio::test]
    async fn every_sub2api_request_goes_through_the_provider_proxy() {
        let unavailable = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", listener.local_addr().unwrap())
        };
        let dir = tempfile::tempdir().unwrap();
        let (app, _, base, path, card, _) = pool(dir.path()).await;
        source_file(dir.path(), "http://sub2api.invalid", AUTH_TOKEN, "sk-team");
        assert!(path.exists());
        // The card sets its provider's proxy; the default account proxy is not used.
        app.save_account_proxy(Pool::Claude, &card, &base)
            .await
            .unwrap();
        assert_eq!(app.claude_load().unwrap().providers[0].proxy_url, base);
        let mut preferences = app.preferences().unwrap();
        preferences.account_proxy_url = unavailable.clone();
        app.save_preferences(preferences).await.unwrap();
        let mut store = app.claude_load().unwrap();
        let mut other = store.providers[0].cached_accounts[0].clone();
        other.id = 9;
        store.providers[0].imported_account_ids.push(9);
        store.providers[0].cached_accounts.push(other);
        app.claude_save(&store).unwrap();
        app.refresh_all(Pool::Claude).await.unwrap();
        let accounts = app.accounts(Pool::Claude).await.unwrap();
        let card = accounts.iter().find(|a| a.id == card).unwrap();
        assert_eq!(card.proxy_url, base);
        assert!(card.usage_error.is_none());
        let card = card.id.clone();
        app.save_account_proxy(Pool::Claude, &card, &unavailable)
            .await
            .unwrap();
        assert!(app.refresh_all(Pool::Claude).await.is_err());
        assert!(app.claude_load().unwrap().providers[0]
            .cached_accounts
            .iter()
            .all(|a| a.usage_error.is_some()));
        // Explicit direct mode.
        source_file(dir.path(), &base, AUTH_TOKEN, "sk-team");
        app.save_account_proxy(Pool::Claude, &card, "direct://")
            .await
            .unwrap();
        let mut store = app.claude_load().unwrap();
        store.providers[0].cached_accounts.retain(|a| a.id != 9);
        store.providers[0]
            .imported_account_ids
            .retain(|id| *id != 9);
        app.claude_save(&store).unwrap();
        app.refresh_all(Pool::Claude).await.unwrap();
        assert!(app
            .accounts(Pool::Claude)
            .await
            .unwrap()
            .iter()
            .find(|a| a.id == card)
            .unwrap()
            .usage_error
            .is_none());
    }

    #[tokio::test]
    async fn deleted_sub2api_provider_stays_removed_until_explicit_sync() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _, _, _, _, _, _, requests) = pool_recording(dir.path()).await;
        for account in app.accounts(Pool::Claude).await.unwrap() {
            if crate::sub2api::is_card(Pool::Claude, &account.id) {
                app.delete_account(Pool::Claude, &account.id).await.unwrap();
            }
        }
        requests.lock().unwrap().clear();
        app.refresh_all(Pool::Claude).await.unwrap();
        app.refresh_recurring(Pool::Claude, 0).await.unwrap();
        assert!(requests.lock().unwrap().is_empty());
        assert!(app.claude_load().unwrap().providers[0]
            .cached_accounts
            .is_empty());
        sync_all(&app).await;
        assert!(!app.claude_load().unwrap().providers[0]
            .cached_accounts
            .is_empty());
    }

    #[tokio::test]
    async fn sub2api_refresh_failure_marks_only_affected_cards_and_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _, _, path, card, _) = pool(dir.path()).await;
        let mut store = app.claude_load().unwrap();
        let provider = &mut store.providers[0];
        let provider_id = provider.id.clone();
        let mut other = provider.cached_accounts[0].clone();
        other.id = 9;
        provider.imported_account_ids.push(9);
        provider.cached_accounts.push(other);
        app.claude_save(&store).unwrap();
        let original = fs::read(&path).unwrap();
        fs::write(&path, "invalid JSON").unwrap();
        let error = app.refresh_usage(Pool::Claude, &card).await.unwrap_err();
        let store = app.claude_load().unwrap();
        assert_eq!(
            store.providers[0].cached_accounts[0].usage_error.as_deref(),
            Some(error.as_str())
        );
        assert!(store.providers[0].cached_accounts[0].usage.is_some());
        assert!(store.providers[0].cached_accounts[1].usage_error.is_none());
        let error = app
            .refresh_provider(Pool::Claude, &provider_id, false)
            .await
            .unwrap_err();
        assert!(app.claude_load().unwrap().providers[0]
            .cached_accounts
            .iter()
            .all(|a| a.usage_error.as_deref() == Some(error.as_str())));
        fs::write(&path, original).unwrap();
        app.refresh_usage(Pool::Claude, &card).await.unwrap();
        let cards = app.accounts(Pool::Claude).await.unwrap();
        assert!(cards
            .iter()
            .find(|a| a.id == card)
            .unwrap()
            .usage_error
            .is_none());
        assert!(app.claude_load().unwrap().providers[0].cached_accounts[1]
            .usage_error
            .is_some());
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
            record: ProviderRecord {
                username: "admin".into(),
                password: "admin-password".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        // The settings file is required, never taken from the main settings.
        assert_eq!(
            save(&app, draft.clone()).await.unwrap_err(),
            "error.sub2api.configuration_incomplete"
        );
        let mut saved = save(
            &app,
            Provider {
                settings: format!(" {} ", path.display()),
                ..draft
            },
        )
        .await
        .unwrap();
        assert_eq!(saved.name, "team");
        assert_eq!(saved.settings_path, path.to_str().unwrap());
        assert_eq!(saved.settings, saved.settings_path);
        assert_eq!(saved.gateway(), "https://gateway.example");
        assert!(saved.has_api_key);
        assert!(saved.password.is_empty() && saved.api_key.is_empty());
        assert_eq!(
            connection(&app, &saved.id).unwrap().admin_base,
            "https://gateway.example/api/v1"
        );

        saved.admin_base_url = "https://admin.example/api/v1/".into();
        saved = save(&app, saved).await.unwrap();
        let connection = connection(&app, &saved.id).unwrap();
        assert_eq!(connection.admin_base, "https://admin.example/api/v1");
        assert_eq!(
            connection.definition.base_url.as_deref(),
            Some("https://gateway.example")
        );
        assert_eq!(connection.configuration.password, "admin-password");
        let stored = fs::read_to_string(app.claude_store_path()).unwrap();
        assert!(!stored.contains("sk-from") && !stored.contains("admin-password"));
        assert!(save(
            &app,
            Provider {
                record: ProviderRecord {
                    username: "admin".into(),
                    password: "password".into(),
                    ..Default::default()
                },
                settings: dir.path().join("missing.json").to_str().unwrap().into(),
                ..Default::default()
            }
        )
        .await
        .unwrap_err()
        .contains("Cannot read"));
        saved.admin_base_url = "https://user:password@admin.example".into();
        assert_eq!(
            save(&app, saved).await.unwrap_err(),
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
            current(&app.accounts(Pool::Claude).await.unwrap()),
            [alice.clone()]
        );

        // A missing settings file fails without changing anything.
        let source_raw = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(app.switch_account(Pool::Claude, &card).await.is_err());
        assert_eq!(fs::read_to_string(main_path(&location)).unwrap(), official);
        assert!(!app.claude_official_path().exists());
        fs::write(&path, &source_raw).unwrap();

        let execution = app.switch_account(Pool::Claude, &card).await.unwrap();
        assert_eq!(execution.provider.as_deref(), Some("team"));
        assert!(execution.login_overrides.is_empty());
        assert_eq!(
            main_settings(&location),
            json!({"model": "source-model", "permissions": {"allow": ["Read", "Edit"]},
                "env": {BASE_URL: base, AUTH_TOKEN: "sk-team", "DISABLE_TELEMETRY": "1"}})
        );
        assert_eq!(fs::read(&path).unwrap(), source_raw);
        assert_eq!(
            current(&app.accounts(Pool::Claude).await.unwrap()),
            [card.clone()]
        );

        // Provider to provider merges over the recorded official settings,
        // and the file is read again: the old credential does not remain.
        let mut edited = main_settings(&location);
        edited["statusLine"] = json!({"type": "command"});
        // An address set while a provider's credential is in use is not the
        // login's: the relay recorded with the login returns.
        edited["env"][BASE_URL] = "https://edited.invalid".into();
        write_main(&location, &edited);
        source_file(dir.path(), &base, "ANTHROPIC_API_KEY", "rotated-model-key");
        let execution = app.switch_account(Pool::Claude, &card).await.unwrap();
        assert!(execution.login_overrides.is_empty());
        assert_eq!(
            main_settings(&location)["env"],
            json!({BASE_URL: base, AUTH_TOKEN: "", "DISABLE_TELEMETRY": "1",
                "ANTHROPIC_API_KEY": "rotated-model-key"})
        );
        assert_eq!(
            current(&app.accounts(Pool::Claude).await.unwrap()),
            [card.clone()]
        );

        // Back to the login: the recorded settings return, with what changed
        // while a provider was applied, and the relay recorded with them.
        let execution = app.switch_account(Pool::Claude, &alice).await.unwrap();
        assert!(execution.login_overrides.is_empty());
        let mut restored: Value = serde_json::from_str(&official).unwrap();
        restored["statusLine"] = json!({"type": "command"});
        assert_eq!(main_settings(&location), restored);
        assert_eq!(
            current(&app.accounts(Pool::Claude).await.unwrap()),
            [alice.clone()]
        );
        // Between logins the settings are left alone.
        let mut changed = main_settings(&location);
        changed["theme"] = "dark".into();
        write_main(&location, &changed);
        app.switch_account(Pool::Claude, &alice).await.unwrap();
        assert_eq!(main_settings(&location), changed);

        // A credential written outside Copool takes Claude Code off the login.
        let outside = json!({"apiKeyHelper": "helper", "theme": "dark",
            "env": {AUTH_TOKEN: "outside", BASE_URL: "https://gateway.invalid",
                "DISABLE_TELEMETRY": "1"}});
        write_main(&location, &outside);
        assert!(current(&app.accounts(Pool::Claude).await.unwrap()).is_empty());
        // The record went with its restore, so only what replaces the login goes.
        assert!(!app.claude_official_path().exists());
        app.switch_account(Pool::Claude, &alice).await.unwrap();
        assert_eq!(
            main_settings(&location),
            json!({"theme": "dark", "env": {"DISABLE_TELEMETRY": "1"}})
        );
        assert!(!app.claude_official_path().exists());
        write_main(&location, &outside);
        app.switch_account(Pool::Claude, &card).await.unwrap();
        assert_eq!(
            main_settings(&location),
            json!({"theme": "dark", "env": {"DISABLE_TELEMETRY": "1",
                BASE_URL: base, "ANTHROPIC_API_KEY": "rotated-model-key"},
                "model": "source-model", "permissions": {"allow": ["Edit"]}})
        );
        // What remained is recorded, so the login does not keep the provider's
        // base URL and send its token to the gateway.
        assert!(app.claude_official_path().exists());
        app.switch_account(Pool::Claude, &alice).await.unwrap();
        assert_eq!(
            main_settings(&location),
            json!({"theme": "dark", "env": {"DISABLE_TELEMETRY": "1"}})
        );
        assert!(!app.claude_official_path().exists());
        app.delete_account(Pool::Claude, &card).await.unwrap();
        assert_eq!(app.accounts(Pool::Claude).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_provider_given_by_its_address_sends_the_key_its_variable_holds() {
        let dir = tempfile::tempdir().unwrap();
        let requests = std::sync::Arc::new(Mutex::new(vec![]));
        let (app, location) = open(dir.path());
        let base = server(requests).await;
        let draft = |url: &str, variable: &str| Provider {
            record: ProviderRecord {
                name: "relay".into(),
                base_url: url.into(),
                username: "admin@example.com".into(),
                password: "secret".into(),
                ..Default::default()
            },
            mode: "address".into(),
            api_key_variable: variable.into(),
            ..Default::default()
        };
        // A key itself is refused, never kept; the address is required.
        assert_eq!(
            save(&app, draft(&base, "sk-relay")).await.unwrap_err(),
            API_KEY_VARIABLE_INVALID
        );
        assert_eq!(
            save(&app, draft("", "")).await.unwrap_err(),
            CONFIGURATION_INCOMPLETE
        );
        // The gateway's `/v1` is the provider's root; the variable a default.
        let saved = save(&app, draft(&format!("{base}/v1/"), "")).await.unwrap();
        assert_eq!(saved.base_url, base);
        assert_eq!(saved.api_key_variable, "CLAUDE_SUB2API_API_KEY");
        assert_eq!(saved.settings, "");
        let listed = &listed(&app)[0];
        assert_eq!(
            (listed.mode.as_str(), listed.settings.as_str()),
            ("address", "")
        );
        sync_all(&app).await;
        let card = crate::sub2api::card_id(Pool::Claude, &saved.id, 7);
        app.set_claude_network(std::sync::Arc::new(Fake::default()));
        sign_in(&location, "alice", 4_000_000_000_000);
        app.claude_import_current().await.unwrap();
        let alice = app
            .accounts(Pool::Claude)
            .await
            .unwrap()
            .into_iter()
            .find(|a| a.source_tag.is_none())
            .unwrap()
            .id;
        let official = json!({"theme": "dark", "env": {"DISABLE_TELEMETRY": "1"}});
        write_main(&location, &official);

        // Without the variable exported, nothing changes, and the proxy has
        // no route for the provider.
        assert_eq!(
            app.switch_account(Pool::Claude, &card).await.unwrap_err(),
            API_KEY_MISSING
        );
        assert_eq!(main_settings(&location), official);
        assert!(app
            .claude_proxy_candidates()
            .unwrap()
            .iter()
            .all(|c| c.auth_json.get("copoolProvider").is_none()));
        app.set_claude_environment(std::sync::Arc::new(|name: &str, _: bool| {
            (name == "CLAUDE_SUB2API_API_KEY").then(|| "sk-relay".to_owned())
        }));
        let execution = app.switch_account(Pool::Claude, &card).await.unwrap();
        assert_eq!(execution.provider.as_deref(), Some("relay"));
        assert!(execution.login_overrides.is_empty());
        assert_eq!(
            main_settings(&location),
            json!({"theme": "dark", "env": {"DISABLE_TELEMETRY": "1", BASE_URL: base},
                "apiKeyHelper": "printf '%s' \"$CLAUDE_SUB2API_API_KEY\""})
        );
        assert_eq!(
            current(&app.accounts(Pool::Claude).await.unwrap()),
            [card.clone()]
        );
        let candidates = app.claude_proxy_candidates().unwrap();
        let route = &candidates
            .iter()
            .find(|c| c.auth_json.get("copoolProvider").is_some())
            .unwrap()
            .auth_json["copoolProvider"];
        assert_eq!(route["baseUrl"], base.as_str());
        assert_eq!(route["apiKey"], "sk-relay");
        assert_eq!(route["authHeader"], "authorization");

        // Back to the login, the helper and the gateway go.
        app.switch_account(Pool::Claude, &alice).await.unwrap();
        assert_eq!(main_settings(&location), official);

        // Through a proxy, the admin API is the Sub2api in its path.
        let proxied = "http://127.0.0.1:8787/anthropic/https://relay.invalid";
        let mut edited = draft(proxied, "RELAY_KEY");
        edited.id = saved.id.clone();
        edited.password = String::new();
        let saved = save(&app, edited).await.unwrap();
        let connection = connection(&app, &saved.id).unwrap();
        assert_eq!(connection.admin_base, "https://relay.invalid/api/v1");
        assert_eq!(connection.definition.base_url.as_deref(), Some(proxied));
    }

    #[tokio::test]
    async fn a_credential_removed_by_hand_does_not_leave_its_gateway() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, base, _, card, alice) = pool(dir.path()).await;
        let official = json!({"theme": "dark", "env": {"DISABLE_TELEMETRY": "1"}});
        write_main(&location, &official);
        app.switch_account(Pool::Claude, &card).await.unwrap();
        let without_credential = || {
            let mut edited = main_settings(&location);
            edited["env"]
                .as_object_mut()
                .unwrap()
                .shift_remove(AUTH_TOKEN);
            write_main(&location, &edited);
            assert_eq!(main_settings(&location)["env"][BASE_URL], base);
        };
        // Between providers, the record is kept rather than replaced.
        without_credential();
        app.switch_account(Pool::Claude, &card).await.unwrap();
        assert_eq!(main_settings(&location)["env"][AUTH_TOKEN], "sk-team");
        // Back to the login, the gateway goes with what the provider added.
        without_credential();
        app.switch_account(Pool::Claude, &alice).await.unwrap();
        assert_eq!(main_settings(&location), official);
        assert!(!app.claude_official_path().exists());
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
        let saved = save(
            &app,
            Provider {
                record: ProviderRecord {
                    username: "admin@example.com".into(),
                    password: "secret".into(),
                    ..Default::default()
                },
                settings: "team".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            (saved.name.as_str(), saved.settings_path.as_str()),
            ("team", "")
        );
        assert_eq!(saved.settings, "team");
        let stored = fs::read_to_string(app.claude_store_path()).unwrap();
        assert!(!stored.contains("team-settings.json") && !stored.contains("\"settings\""));
        assert_eq!(listed(&app)[0].settings, "team");
        sync_all(&app).await;
        // The file moved: the next switch searches again.
        let moved = location.config_dir.join("other/settings-team.json");
        fs::create_dir_all(moved.parent().unwrap()).unwrap();
        fs::rename(&found, &moved).unwrap();
        write(&moved, "sk-moved");
        app.switch_account(
            Pool::Claude,
            &crate::sub2api::card_id(Pool::Claude, &saved.id, 7),
        )
        .await
        .unwrap();
        assert_eq!(main_settings(&location)["env"][AUTH_TOKEN], "sk-moved");
        // A path chosen later is kept instead.
        let picked = dir.path().join("elsewhere/team.json");
        fs::create_dir_all(picked.parent().unwrap()).unwrap();
        write(&picked, "sk-picked");
        let saved = save(
            &app,
            Provider {
                settings: picked.to_str().unwrap().into(),
                ..saved
            },
        )
        .await
        .unwrap();
        assert_eq!(saved.settings_path, picked.to_str().unwrap());
        app.switch_account(
            Pool::Claude,
            &crate::sub2api::card_id(Pool::Claude, &saved.id, 7),
        )
        .await
        .unwrap();
        assert_eq!(main_settings(&location)["env"][AUTH_TOKEN], "sk-picked");
    }

    #[tokio::test]
    async fn a_missing_official_settings_file_is_restored_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, _, _, card, alice) = pool(dir.path()).await;
        assert!(!main_path(&location).exists());
        app.switch_account(Pool::Claude, &card).await.unwrap();
        assert!(main_path(&location).exists());
        app.switch_account(Pool::Claude, &alice).await.unwrap();
        assert!(!main_path(&location).exists());
        assert_eq!(current(&app.accounts(Pool::Claude).await.unwrap()), [alice]);
    }

    #[tokio::test]
    async fn recurring_refresh_reaches_an_inactive_provider_every_round() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _, _, _, card, alice, secrets, requests) = pool_recording(dir.path()).await;
        let provider = app.claude_load().unwrap().providers[0].id.clone();
        assert_eq!(current(&app.accounts(Pool::Claude).await.unwrap()), [alice]);
        secrets.clear_reads();
        requests.lock().unwrap().clear();
        // An inactive provider is included even on tick 1.
        app.refresh_recurring(Pool::Claude, 1).await.unwrap();
        assert_eq!(
            secrets.reads(),
            [crate::sub2api::password_secret(Pool::Claude, &provider)]
        );
        // The round reaches the provider with the login of the earlier sync.
        assert_eq!((logins(&requests), listings(&requests)), (0, 1));
        // Its card shows the very next round.
        app.set_schedule(
            Pool::Claude,
            crate::refresh_policy::Schedule {
                at: 1_000,
                tick: 2,
                interval: 300,
                full_every: 1,
            },
        );
        let accounts = app.accounts(Pool::Claude).await.unwrap();
        assert_eq!(
            accounts
                .iter()
                .find(|a| a.id == card)
                .unwrap()
                .next_refresh_at,
            Some(1_000)
        );
    }

    #[tokio::test]
    async fn every_round_refreshes_active_and_inactive_providers() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _, _, _, card, _, secrets, requests) = pool_recording(dir.path()).await;
        let (other, other_requests) = add_provider(&app, dir.path(), "other-secret").await;
        let first = app.claude_load().unwrap().providers[0].id.clone();
        app.switch_account(Pool::Claude, &card).await.unwrap();
        secrets.clear_reads();
        requests.lock().unwrap().clear();
        other_requests.lock().unwrap().clear();
        // Every round includes the inactive provider.
        app.refresh_recurring(Pool::Claude, 1).await.unwrap();
        let mut reads = secrets.reads();
        reads.sort();
        let mut expected = vec![
            crate::sub2api::password_secret(Pool::Claude, &first),
            crate::sub2api::password_secret(Pool::Claude, &other.id),
        ];
        expected.sort();
        assert_eq!(reads, expected);
        assert_eq!((logins(&requests), logins(&other_requests)), (0, 0));
        assert_eq!((listings(&requests), listings(&other_requests)), (1, 1));
    }

    #[tokio::test]
    async fn manual_refresh_still_reads_every_provider() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _, _, _, _, _, secrets, requests) = pool_recording(dir.path()).await;
        let (other, other_requests) = add_provider(&app, dir.path(), "other-secret").await;
        let first = app.claude_load().unwrap().providers[0].id.clone();
        secrets.clear_reads();
        requests.lock().unwrap().clear();
        other_requests.lock().unwrap().clear();
        app.refresh_all(Pool::Claude).await.unwrap();
        let mut reads = secrets.reads();
        reads.sort();
        let mut expected = vec![
            crate::sub2api::password_secret(Pool::Claude, &first),
            crate::sub2api::password_secret(Pool::Claude, &other.id),
        ];
        expected.sort();
        assert_eq!(reads, expected);
        assert_eq!((logins(&requests), logins(&other_requests)), (0, 0));
        assert_eq!((listings(&requests), listings(&other_requests)), (1, 1));
    }

    #[tokio::test]
    async fn the_claude_proxy_routes_a_provider_to_its_gateway_with_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _, base, _, _, alice) = pool(dir.path()).await;
        let candidates = app.claude_proxy_candidates().unwrap();
        // The official login, and one route for the provider.
        assert!(candidates.iter().any(|c| c.id == alice));
        let route = candidates
            .iter()
            .find(|c| c.auth_json.get("copoolProvider").is_some())
            .unwrap();
        let provider = &route.auth_json["copoolProvider"];
        assert_eq!(provider["baseUrl"], base.as_str());
        assert_eq!(provider["apiKey"], "sk-team");
        // ANTHROPIC_AUTH_TOKEN is sent as a bearer token.
        assert_eq!(provider["authHeader"], "authorization");
        assert_eq!(provider["preferred"], false);
        assert!(route.usage.is_some());
        // None of its cards active, the provider is no route.
        let mut store = app.claude_load().unwrap();
        for account in &mut store.providers[0].cached_accounts {
            account.status = "missing".into();
        }
        app.claude_save(&store).unwrap();
        let candidates = app.claude_proxy_candidates().unwrap();
        assert!(candidates
            .iter()
            .all(|c| c.auth_json.get("copoolProvider").is_none()));
    }
}
