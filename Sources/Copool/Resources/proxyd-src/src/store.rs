use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use uuid::Uuid;

#[cfg(feature = "desktop")]
use tauri::AppHandle;
#[cfg(feature = "desktop")]
use tauri::Manager;

use crate::auth::extract_auth;
use crate::auth::read_current_codex_auth_optional;
use crate::models::AccountsStore;
use crate::models::StoredAccount;
use crate::utils::now_unix_seconds;
use crate::utils::set_private_permissions;
use crate::utils::short_account;

#[cfg(feature = "desktop")]
pub(crate) fn load_store(app: &AppHandle) -> Result<AccountsStore, String> {
    load_store_from_path(&account_store_path(app)?)
}

#[cfg(feature = "desktop")]
pub(crate) fn save_store(app: &AppHandle, store: &AccountsStore) -> Result<(), String> {
    save_store_to_path(&account_store_path(app)?, store)
}

/// 启动时自动同步当前登录账号：
/// 若本机已有 `~/.codex/auth.json` 且账号不在列表中，则自动写入存储。
#[cfg(feature = "desktop")]
pub(crate) fn sync_current_auth_account_on_startup(app: &AppHandle) -> Result<(), String> {
    sync_current_auth_account_on_startup_in_path(&account_store_path(app)?)
}

pub(crate) fn load_store_from_path(path: &Path) -> Result<AccountsStore, String> {
    crate::repository::load_accounts(path)
}
pub(crate) fn save_store_to_path(path: &Path, store: &AccountsStore) -> Result<(), String> {
    crate::repository::save_accounts(path, store)
}

pub(crate) fn sync_current_auth_account_on_startup_in_path(path: &Path) -> Result<(), String> {
    let auth_json = match read_current_codex_auth_optional()? {
        Some(value) => value,
        None => return Ok(()),
    };

    let extracted = match extract_auth(&auth_json) {
        Ok(value) => value,
        Err(err) => {
            log::warn!("跳过启动自动导入当前账号: {err}");
            return Ok(());
        }
    };

    let mut store = load_store_from_path(path)?;
    let already_exists = store
        .accounts
        .iter()
        .any(|account| account.account_id == extracted.account_id);
    if already_exists {
        return Ok(());
    }

    let now = now_unix_seconds();
    let label = extracted
        .email
        .clone()
        .unwrap_or_else(|| format!("Codex {}", short_account(&extracted.account_id)));

    let stored = StoredAccount {
        extra: Default::default(),
        id: Uuid::new_v4().to_string(),
        label,
        email: extracted.email,
        account_id: extracted.account_id,
        plan_type: extracted.plan_type,
        team_alias: None,
        auth_json,
        added_at: now,
        updated_at: now,
        usage: None,
        usage_error: None,
        principal_id: None,
        proxy_url: String::new(),
    };
    store.accounts.push(stored);
    save_store_to_path(path, &store)?;
    Ok(())
}

#[cfg(feature = "desktop")]
fn account_store_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法获取应用数据目录: {e}"))?;
    Ok(account_store_path_from_data_dir(&dir))
}

pub(crate) fn account_store_path_from_data_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("accounts.json")
}

fn write_store_file(path: &Path, store: &AccountsStore) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("无法解析存储目录 {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("创建存储目录失败 {}: {e}", parent.display()))?;

    let serialized =
        serde_json::to_string_pretty(store).map_err(|e| format!("序列化账号存储失败: {e}"))?;
    write_file_atomically(path, serialized.as_bytes())?;
    Ok(())
}

fn write_file_atomically(path: &Path, contents: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Invalid storage directory")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temp.write_all(contents).map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

fn backup_corrupted_store_file(path: &Path, raw: &str) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("无法解析存储目录 {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("创建存储目录失败 {}: {e}", parent.display()))?;

    let backup_path = parent.join(format!("accounts.corrupt-{}.json", now_unix_seconds()));
    fs::write(&backup_path, raw)
        .map_err(|e| format!("写入损坏备份文件失败 {}: {e}", backup_path.display()))?;
    set_private_permissions(&backup_path);
    Ok(backup_path)
}
