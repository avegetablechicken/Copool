//! Shared account/settings transactions, including legacy Swift data migrations.
use crate::{models::AccountsStore, settings::Settings};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};
static FILES: Mutex<()> = Mutex::new(());
fn sibling(path: &Path, name: &str) -> Result<PathBuf, String> {
    Ok(path.parent().ok_or("Invalid data path")?.join(name))
}
fn read(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
fn atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Invalid data path")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
fn write(path: &Path, value: &Value) -> Result<(), String> {
    atomic(
        path,
        &serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
}
fn settings_value(account_path: &Path) -> Result<Value, String> {
    let path = sibling(account_path, "settings.json")?;
    if path.exists() {
        let value = read(&path)?;
        if !value.is_object() {
            return Err("Invalid settings.json".into());
        }
        return Ok(value);
    }
    let mut value = serde_json::to_value(Settings::default()).map_err(|e| e.to_string())?;
    if let Ok(legacy) = read(account_path) {
        if let Some(settings) = legacy.get("settings").and_then(Value::as_object) {
            value.as_object_mut().unwrap().extend(settings.clone());
        }
    }
    Ok(value)
}
fn proxies(settings: &Value) -> Result<BTreeMap<String, String>, String> {
    settings
        .get("accountProxyURLs")
        .map(|v| {
            serde_json::from_value(v.clone())
                .map_err(|_| "Invalid accountProxyURLs in settings.json".into())
        })
        .unwrap_or_else(|| Ok(BTreeMap::new()))
}
fn strip_proxy(account: &mut Value) {
    if let Some(object) = account.as_object_mut() {
        object.remove("proxyURL");
        object.remove("proxyUrl");
    }
}
fn account_document(store: &AccountsStore) -> Result<Value, String> {
    let mut value = serde_json::to_value(store).map_err(|e| e.to_string())?;
    value.as_object_mut().unwrap().remove("settings");
    if let Some(accounts) = value.get_mut("accounts").and_then(Value::as_array_mut) {
        for account in accounts {
            strip_proxy(account);
        }
    }
    if let Some(cache) = value
        .get_mut("cachedAccounts")
        .and_then(Value::as_object_mut)
    {
        for accounts in cache.values_mut().filter_map(Value::as_array_mut) {
            for account in accounts {
                strip_proxy(account);
            }
        }
    }
    Ok(value)
}
fn load_accounts_inner(path: &Path) -> Result<AccountsStore, String> {
    if !path.exists() {
        return Ok(AccountsStore::default());
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let mut store: AccountsStore = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            let backup = sibling(
                path,
                &format!(
                    "accounts.corrupt-{}-{}.json",
                    crate::utils::now_unix_seconds(),
                    uuid::Uuid::new_v4()
                ),
            )?;
            atomic(&backup, &bytes)?;
            let store = AccountsStore::default();
            write(path, &account_document(&store)?)?;
            return Ok(store);
        }
    };
    // Settings errors are deliberately outside the account-corruption recovery.
    let settings = settings_value(path)?;
    let map = proxies(&settings)?;
    for account in &mut store.accounts {
        if let Some(proxy) = map.get(&account.id) {
            account.proxy_url = proxy.clone();
        }
    }
    if let Some(key) = settings.get("apiProxyApiKey").and_then(Value::as_str) {
        store.settings.api_proxy_api_key = Some(key.into());
    }
    let raw: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let inline = raw["accounts"].as_array().is_some_and(|rows| {
        rows.iter()
            .any(|row| row.get("proxyURL").is_some() || row.get("proxyUrl").is_some())
    }) || raw["cachedAccounts"].as_object().is_some_and(|cache| {
        cache
            .values()
            .filter_map(Value::as_array)
            .flatten()
            .any(|a| a.get("proxyURL").is_some())
    });
    if inline {
        save_accounts_inner(path, &store)?;
    }
    Ok(store)
}
fn save_accounts_inner(path: &Path, store: &AccountsStore) -> Result<(), String> {
    let mut settings = settings_value(path)?;
    settings["accountProxyURLs"] = serde_json::to_value(
        store
            .accounts
            .iter()
            .map(|a| (a.id.clone(), a.proxy_url.clone()))
            .collect::<BTreeMap<_, _>>(),
    )
    .map_err(|e| e.to_string())?;
    if let Some(key) = &store.settings.api_proxy_api_key {
        settings["apiProxyApiKey"] = key.clone().into();
    }
    write(&sibling(path, "settings.json")?, &settings)?;
    write(path, &account_document(store)?)
}
pub(crate) fn load_accounts(path: &Path) -> Result<AccountsStore, String> {
    let _guard = FILES.lock().map_err(|_| "Account store lock poisoned")?;
    load_accounts_inner(path)
}
pub(crate) fn save_accounts(path: &Path, store: &AccountsStore) -> Result<(), String> {
    let _guard = FILES.lock().map_err(|_| "Account store lock poisoned")?;
    let mut updated = store.clone();
    if path.exists() {
        let latest = load_accounts_inner(path)?;
        updated.extra.insert(
            "cachedAccounts".into(),
            latest
                .extra
                .get("cachedAccounts")
                .cloned()
                .unwrap_or(json!({})),
        );
    }
    save_accounts_inner(path, &updated)
}
pub(crate) fn mutate_accounts(
    path: &Path,
    edit: impl FnOnce(&mut AccountsStore) -> Result<(), String>,
) -> Result<AccountsStore, String> {
    let _guard = FILES.lock().map_err(|_| "Account store lock poisoned")?;
    let mut value = load_accounts_inner(path)?;
    edit(&mut value)?;
    save_accounts_inner(path, &value)?;
    Ok(value)
}
fn cache_from(store: &AccountsStore) -> serde_json::Map<String, Value> {
    store
        .extra
        .get("cachedAccounts")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}
fn without_caches(settings: &Settings) -> Result<Value, String> {
    let mut value = serde_json::to_value(settings).map_err(|e| e.to_string())?;
    if let Some(providers) = value
        .pointer_mut("/sub2APIProvider/providers")
        .and_then(Value::as_array_mut)
    {
        for p in providers {
            if let Some(object) = p.as_object_mut() {
                object.remove("cachedAccounts");
            }
        }
    }
    Ok(value)
}
pub(crate) fn load_settings(path: &Path) -> Result<Settings, String> {
    let _guard = FILES.lock().map_err(|_| "Account store lock poisoned")?;
    let account_path = sibling(path, "accounts.json")?;
    let raw = settings_value(&account_path)?;
    let mut settings: Settings = serde_json::from_value(raw.clone()).map_err(|e| e.to_string())?;
    let absent_settings = !path.exists();
    let mut store = load_accounts_inner(&account_path)?;
    settings.account_proxy_urls = proxies(&settings_value(&account_path)?)?;
    if absent_settings {
        for account in &store.accounts {
            settings
                .account_proxy_urls
                .entry(account.id.clone())
                .or_insert_with(|| account.proxy_url.clone());
        }
    }
    let mut cache = cache_from(&store);
    let mut migration = absent_settings;
    for provider in &mut settings.sub2_api_provider.providers {
        let legacy = raw
            .pointer("/sub2APIProvider/providers")
            .and_then(Value::as_array)
            .and_then(|rows| {
                rows.iter().find(|row| {
                    row["id"].as_str() == Some(&provider.id)
                        || row["providerID"]
                            .as_str()
                            .is_some_and(|id| id.trim().eq_ignore_ascii_case(&provider.provider_id))
                })
            })
            .or_else(|| {
                raw.get("sub2APIProvider")
                    .filter(|v| v.get("providers").is_none())
            });
        if legacy.is_some_and(|v| v.get("cachedAccounts").is_some()) {
            cache.insert(
                provider.id.clone(),
                Value::Array(provider.cached_accounts.clone()),
            );
            migration = true;
        } else {
            provider.cached_accounts = cache
                .get(&provider.id)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
        }
        *provider = provider.normalized();
    }
    if migration {
        store.extra.insert("cachedAccounts".into(), cache.into());
        write(&account_path, &account_document(&store)?)?;
        let mut persisted = without_caches(&settings)?;
        // Do not discard credentials already present in a legacy file during a
        // cache-only migration. Explicit Settings serialization never writes them.
        if let Some(rows) = persisted
            .pointer_mut("/sub2APIProvider/providers")
            .and_then(Value::as_array_mut)
        {
            for row in rows {
                if let Some(provider) = settings
                    .sub2_api_provider
                    .providers
                    .iter()
                    .find(|p| row["id"].as_str() == Some(&p.id))
                {
                    if !provider.password.is_empty() {
                        row["password"] = provider.password.clone().into();
                    }
                }
            }
        }
        write(path, &persisted)?;
    }
    Ok(settings)
}
pub(crate) fn save_settings(path: &Path, settings: &Settings) -> Result<(), String> {
    let _guard = FILES.lock().map_err(|_| "Account store lock poisoned")?;
    let account_path = sibling(path, "accounts.json")?;
    let mut store = load_accounts_inner(&account_path)?;
    let mut settings = settings.clone();
    settings.account_proxy_urls = proxies(&settings_value(&account_path)?)?;
    settings.sub2_api_provider = settings.sub2_api_provider.normalized();
    let cache: serde_json::Map<String, Value> = settings
        .sub2_api_provider
        .providers
        .iter()
        .map(|p| (p.id.clone(), Value::Array(p.cached_accounts.clone())))
        .collect();
    store.extra.insert("cachedAccounts".into(), cache.into());
    let before = fs::read(&account_path).ok();
    write(&account_path, &account_document(&store)?)?;
    if let Err(error) = write(path, &without_caches(&settings)?) {
        if let Some(bytes) = before {
            atomic(&account_path, &bytes)?;
        } else {
            let _ = fs::remove_file(&account_path);
        }
        return Err(error);
    }
    Ok(())
}
