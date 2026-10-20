use crate::{
    application::Paths,
    auth, codex_config,
    models::{CurrentAccountSelection, StoredAccount},
    persistence::write_json,
    repository,
    utils::now_unix_seconds,
};
use serde_json::{json, Value};
use std::fs;

fn api_key(definition: &codex_config::Definition, paths: &Paths) -> Option<String> {
    if let Some(name) = &definition.env_key {
        if let Ok(value) = std::env::var(name) {
            if !value.trim().is_empty() {
                return Some(value);
            }
        }
        // Codex loads CODEX_HOME/.env into its environment; a key kept there
        // is as available to Codex as an exported one.
        if let Some(value) = codex_dotenv(paths, name) {
            return Some(value);
        }
        #[cfg(unix)]
        if !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            let shell = std::env::var("SHELL").unwrap_or("/bin/sh".into());
            use std::sync::{LazyLock, Mutex};
            static CACHE: LazyLock<
                Mutex<std::collections::BTreeMap<String, (std::time::Instant, Option<String>)>>,
            > = LazyLock::new(Default::default);
            let mut cache = CACHE.lock().ok()?;
            if let Some((time, value)) = cache.get(name) {
                if time.elapsed() < std::time::Duration::from_secs(30) {
                    return value.clone();
                }
            }
            let value = login_shell_key(&shell, name, std::time::Duration::from_secs(3));
            cache.insert(name.clone(), (std::time::Instant::now(), value.clone()));
            return value;
        }
    }
    None
}

/// `name` from CODEX_HOME/.env: `KEY=value` lines, optionally `export`ed or
/// quoted; blank lines and `#` comments are skipped.
fn codex_dotenv(paths: &Paths, name: &str) -> Option<String> {
    let text = fs::read_to_string(paths.codex_auth.with_file_name(".env")).ok()?;
    text.lines().find_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (key, value) = line.split_once('=')?;
        if line.starts_with('#') || key.trim() != name {
            return None;
        }
        let value = value.trim();
        let value = [('"', '"'), ('\'', '\'')]
            .iter()
            .find_map(|(open, close)| value.strip_prefix(*open)?.strip_suffix(*close))
            .unwrap_or(value);
        (!value.is_empty()).then(|| value.to_owned())
    })
}
#[cfg(unix)]
fn login_shell_key(shell: &str, name: &str, timeout: std::time::Duration) -> Option<String> {
    use std::io::Read;
    let output = tempfile::NamedTempFile::new().ok()?;
    let mut child = std::process::Command::new(shell)
        // Profiles may print banners; only the marked value is the key.
        .args([
            "-lc",
            &format!("printf '__COPOOL_ENV__%s__COPOOL_END__' \"${name}\""),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(output.reopen().ok()?)
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                break;
            }
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let mut value = String::new();
    output
        .reopen()
        .ok()?
        .take(65536)
        .read_to_string(&mut value)
        .ok()?;
    let value = value
        .rsplit_once("__COPOOL_ENV__")?
        .1
        .split_once("__COPOOL_END__")?
        .0
        .trim();
    (!value.is_empty() && value.len() < 65536).then(|| value.to_owned())
}

pub(crate) fn candidates(paths: &Paths) -> Result<Vec<StoredAccount>, String> {
    candidates_with_keys(paths, |definition| api_key(definition, paths))
}
pub(crate) fn candidates_with_keys(
    paths: &Paths,
    resolve_key: impl Fn(&codex_config::Definition) -> Option<String>,
) -> Result<Vec<StoredAccount>, String> {
    let settings = repository::load_settings(&paths.data_dir.join("settings.json"))?;
    let config = paths.codex_auth.with_file_name("config.toml");
    let defs = codex_config::definitions(&config);
    let current = codex_config::resolve_path(&config);
    let mut result = vec![];
    for provider in settings.sub2_api_provider.providers {
        let Some(def) = defs
            .iter()
            .find(|d| d.id.eq_ignore_ascii_case(&provider.provider_id))
        else {
            continue;
        };
        if def.requires_openai_auth == Some(true)
            || !def
                .wire_api
                .as_deref()
                .unwrap_or("responses")
                .eq_ignore_ascii_case("responses")
            || def.env_key.as_ref().is_none_or(|s| s.is_empty())
        {
            continue;
        }
        let Some(base) = &def.base_url else {
            continue;
        };
        let Some(key) = resolve_key(def).filter(|s| !s.is_empty()) else {
            continue;
        };
        let ids = &provider.imported_account_ids;
        let Some(&first) = ids.iter().min() else {
            continue;
        };
        let representative = provider
            .cached_accounts
            .iter()
            .filter_map(|raw| serde_json::from_value::<crate::sub2api::Account>(raw.clone()).ok())
            .filter(|a| ids.contains(&a.id))
            .max_by(|a, b| {
                crate::ranking::remaining_score(&a.summary())
                    .total_cmp(&crate::ranking::remaining_score(&b.summary()))
            });
        let route = format!("sub2api-provider:{}", provider.provider_id.to_lowercase());
        let auth = json!({"tokens":{"access_token":key,"account_id":route,"id_token":""},"copoolProvider":{"id":provider.provider_id,"baseUrl":base,"apiKey":key,"allowInsecureTLS":provider.allow_insecure_tls,"preferred":current.id.eq_ignore_ascii_case(&provider.provider_id)}});
        let stored:StoredAccount=serde_json::from_value(json!({"id":route,"label":provider.provider_id,"accountId":route,"authJson":auth,"proxyUrl":provider.proxy_url.trim(),"addedAt":representative.as_ref().map(|a|a.id).unwrap_or(first),"updatedAt":0,"usage":representative.and_then(|a|a.usage)})).map_err(|e|e.to_string())?;
        result.push(stored);
    }
    Ok(result)
}
pub(crate) fn select(paths: &Paths, id: &str, auth_json: &Value) -> Result<bool, String> {
    let settings = repository::load_settings(&paths.data_dir.join("settings.json"))?;
    if settings.local_proxy_host_api_only {
        return Ok(false);
    }
    let config = paths.codex_auth.with_file_name("config.toml");
    if let Some(provider) = auth_json.get("copoolProvider") {
        let id = provider["id"].as_str().ok_or("Invalid provider")?;
        if !codex_config::resolve_path(&config)
            .id
            .eq_ignore_ascii_case(id)
        {
            codex_config::switch(&config, id)?;
            return Ok(true);
        }
        return Ok(false);
    }
    let before = repository::load_accounts(&paths.store())?;
    let current_auth = fs::read(&paths.codex_auth)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let same_identity = current_auth
        .as_ref()
        .and_then(|v| auth::account_identity(v).ok())
        == auth::account_identity(auth_json).ok();
    if before.extra.get("currentAccountId").and_then(Value::as_str) == Some(id)
        && codex_config::resolve_path(&config).is_official()
        && same_identity
    {
        return Ok(false);
    }
    repository::mutate_accounts(&paths.store(), |store| {
        if !store.accounts.iter().any(|a| a.id == id) {
            return Err("Account was removed".into());
        }
        if !codex_config::resolve_path(&config).is_official() {
            codex_config::switch(&config, "openai")?;
        }
        let mut normalized = auth::normalized_auth_for_write(auth_json.clone())?;
        if normalized.get("OPENAI_API_KEY").is_none_or(Value::is_null) {
            if let Some(key) = current_auth
                .as_ref()
                .and_then(|v| v.get("OPENAI_API_KEY"))
                .filter(|v| !v.is_null())
            {
                normalized["OPENAI_API_KEY"] = key.clone();
            }
        }
        write_json(&paths.codex_auth, &normalized)?;
        store.extra.insert("currentAccountId".into(), id.into());
        store.current_selection = Some(CurrentAccountSelection {
            account_id: id.into(),
            selected_at: now_unix_seconds(),
            source_device_id: "rust-local".into(),
            account_key: None,
        });
        Ok(())
    })
    .map(|_| true)
}

/// Writes a login the proxy renewed from `from` to auth.json, when Codex
/// still uses that account at that generation or an older one.
pub(crate) fn sync_refreshed_auth(
    paths: &Paths,
    id: &str,
    from: &Value,
    auth_json: &Value,
) -> Result<(), String> {
    let settings = repository::load_settings(&paths.data_dir.join("settings.json"))?;
    if settings.local_proxy_host_api_only
        || !codex_config::resolve_path(&paths.codex_auth.with_file_name("config.toml"))
            .is_official()
    {
        return Ok(());
    }
    let store = repository::load_accounts(&paths.store())?;
    if store.extra.get("currentAccountId").and_then(Value::as_str) != Some(id) {
        return Ok(());
    }
    let current: Value = match fs::read(&paths.codex_auth) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    if auth::account_identity(&current).ok() != auth::account_identity(auth_json).ok()
        || !auth::renewal_replaces(auth_json, from, &current)
    {
        return Ok(());
    }
    let mut next = auth::normalized_auth_for_write(auth_json.clone())?;
    if let Some(key) = current.get("OPENAI_API_KEY").filter(|v| !v.is_null()) {
        next["OPENAI_API_KEY"] = key.clone();
    }
    write_json(&paths.codex_auth, &next)
}
#[cfg(test)]
mod refresh_tests {
    use super::*;
    #[test]
    fn provider_keys_are_read_from_the_codex_dotenv() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: dir.path().join("data"),
            codex_auth: dir.path().join("auth.json"),
        };
        assert_eq!(codex_dotenv(&paths, "SHARECODER_API_KEY"), None);
        fs::write(
            dir.path().join(".env"),
            "# SHARECODER_API_KEY=commented\nOTHER=1\nexport SHARECODER_API_KEY = \"sk-test\"\nEMPTY=\nSINGLE='sk-single'\n",
        )
        .unwrap();
        assert_eq!(
            codex_dotenv(&paths, "SHARECODER_API_KEY").as_deref(),
            Some("sk-test")
        );
        assert_eq!(codex_dotenv(&paths, "SINGLE").as_deref(), Some("sk-single"));
        assert_eq!(codex_dotenv(&paths, "EMPTY"), None);
        assert_eq!(codex_dotenv(&paths, "MISSING"), None);
    }
    #[tokio::test]
    async fn refreshed_proxy_auth_updates_only_current_account_and_respects_host_only() {
        let dir = tempfile::tempdir().unwrap();
        let app = crate::Application::open(Paths {
            data_dir: dir.path().join("data"),
            codex_auth: dir.path().join("codex/auth.json"),
        })
        .unwrap();
        let document = json!({"tokens":{"id_token":"","access_token":"old","refresh_token":"refresh","account_id":"workspace"}});
        app.import_json(&document.to_string()).await.unwrap();
        let id = app.accounts().await.unwrap()[0].id.clone();
        app.switch_account(&id).await.unwrap();
        let read =
            || serde_json::from_slice::<Value>(&fs::read(&app.paths.codex_auth).unwrap()).unwrap();
        let from = read();
        let mut refreshed = from.clone();
        refreshed["tokens"]["access_token"] = "new".into();
        sync_refreshed_auth(&app.paths, "different-card", &from, &refreshed).unwrap();
        assert_eq!(read()["tokens"]["access_token"], "old");
        let mut settings = app.configuration().unwrap();
        settings.local_proxy_host_api_only = true;
        app.persist_configuration(&settings).unwrap();
        sync_refreshed_auth(&app.paths, &id, &from, &refreshed).unwrap();
        assert_eq!(read()["tokens"]["access_token"], "old");
        settings.local_proxy_host_api_only = false;
        app.persist_configuration(&settings).unwrap();
        // A renewal answered after Codex renewed the login keeps Codex's.
        let mut renewed = from.clone();
        renewed["tokens"]["access_token"] = "renewed-by-codex".into();
        renewed["last_refresh"] = "2099-01-01T00:00:00Z".into();
        write_json(&app.paths.codex_auth, &renewed).unwrap();
        sync_refreshed_auth(&app.paths, &id, &from, &refreshed).unwrap();
        assert_eq!(read()["tokens"]["access_token"], "renewed-by-codex");
        write_json(&app.paths.codex_auth, &from).unwrap();
        sync_refreshed_auth(&app.paths, &id, &from, &refreshed).unwrap();
        assert_eq!(read()["tokens"]["access_token"], "new");
    }
}
