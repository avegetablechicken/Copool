use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

pub(crate) fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

pub(crate) fn short_account(account_id: &str) -> String {
    account_id.chars().take(8).collect()
}

pub(crate) fn truncate_for_error(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        value.to_string()
    } else {
        format!("{}...", &value[..max_len])
    }
}

pub(crate) fn set_private_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if let Ok(metadata) = fs::metadata(path) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o600);
            let _ = fs::set_permissions(path, permissions);
        }
    }
}

pub(crate) fn prepare_process_path() {
    let mut merged = preferred_executable_dirs();
    if let Some(current_path) = env::var_os("PATH") {
        for dir in env::split_paths(&current_path) {
            push_unique_dir(&mut merged, dir);
        }
    }

    if let Ok(path_env) = env::join_paths(merged) {
        env::set_var("PATH", path_env);
    }
}

pub(crate) fn find_command_path(command: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(path_os) = env::var_os("PATH") {
        for dir in env::split_paths(&path_os) {
            push_command_candidates_from_dir(&mut candidates, &dir, command);
        }
    }

    for dir in preferred_executable_dirs() {
        push_command_candidates_from_dir(&mut candidates, &dir, command);
    }

    candidates.into_iter().find(|path| is_executable_file(path))
}

pub(crate) fn new_resolved_command(command: &str) -> Command {
    let program = find_command_path(command).unwrap_or_else(|| PathBuf::from(command));
    let mut command = Command::new(&program);
    if let Some(parent) = program.parent().filter(|_| program.is_absolute()) {
        if let Some(path_env) = prepend_path_entry(parent) {
            command.env("PATH", path_env);
        }
    }
    command
}

pub(crate) fn prepend_path_entry(path: &Path) -> Option<OsString> {
    let mut paths = vec![path.to_path_buf()];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    env::join_paths(paths).ok()
}

pub(crate) fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn preferred_executable_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    #[cfg(target_os = "macos")]
    {
        for dir in [
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/opt/homebrew/sbin"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/usr/local/sbin"),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
            PathBuf::from("/usr/sbin"),
            PathBuf::from("/sbin"),
            PathBuf::from("/Library/Apple/usr/bin"),
        ] {
            push_unique_dir(&mut dirs, dir);
        }
    }

    if let Some(home) = dirs::home_dir() {
        for dir in [
            home.join(".cargo").join("bin"),
            home.join(".local").join("bin"),
            home.join("bin"),
            home.join(".asdf").join("shims"),
            home.join(".volta").join("bin"),
            home.join(".npm-global").join("bin"),
            home.join("Library").join("pnpm"),
            home.join("AppData")
                .join("Local")
                .join("Microsoft")
                .join("WinGet")
                .join("Links"),
        ] {
            push_unique_dir(&mut dirs, dir);
        }
    }

    dirs
}

fn push_unique_dir(dirs: &mut Vec<PathBuf>, candidate: PathBuf) {
    if candidate.is_dir() && !dirs.iter().any(|existing| existing == &candidate) {
        dirs.push(candidate);
    }
}

fn push_command_candidates_from_dir(candidates: &mut Vec<PathBuf>, dir: &Path, command: &str) {
    #[cfg(windows)]
    {
        for name in [
            format!("{command}.exe"),
            format!("{command}.cmd"),
            format!("{command}.bat"),
        ] {
            candidates.push(dir.join(name));
        }
    }

    #[cfg(not(windows))]
    {
        candidates.push(dir.join(command));
    }
}

// An explicit account proxy replaces environment proxy selection and never falls back to direct.
pub(crate) fn with_account_proxy(
    builder: reqwest::ClientBuilder,
    proxy_url: &str,
) -> Result<reqwest::ClientBuilder, String> {
    let proxy_url = proxy_url.trim();
    if proxy_url.is_empty() {
        return Ok(builder);
    }
    let url = reqwest::Url::parse(proxy_url).map_err(|_| "Invalid account proxy URL".to_string())?;
    if !matches!(url.scheme(), "http" | "https" | "socks5")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (url.path() != "" && url.path() != "/")
    {
        return Err("Invalid account proxy URL".to_string());
    }
    let address = if let Some(rest) = proxy_url.strip_prefix("socks5://") {
        format!("socks5h://{rest}")
    } else {
        proxy_url.to_string()
    };
    let proxy = reqwest::Proxy::all(address).map_err(|_| "Invalid account proxy URL".to_string())?;
    Ok(builder.no_proxy().proxy(proxy))
}

#[cfg(test)]
mod account_proxy_tests {
    use super::with_account_proxy;

    #[test]
    fn account_store_preserves_swift_proxy_field_and_accepts_legacy_accounts() {
        let legacy = serde_json::json!({
            "id": "a", "label": "a", "accountId": "a", "authJson": null,
            "addedAt": 0, "updatedAt": 0
        });
        let account: crate::models::StoredAccount = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(account.proxy_url, "");
        let mut with_proxy = legacy;
        with_proxy["proxyURL"] = serde_json::json!("socks5://127.0.0.1:1080");
        let account: crate::models::StoredAccount = serde_json::from_value(with_proxy).unwrap();
        assert_eq!(account.proxy_url, "socks5://127.0.0.1:1080");
        assert_eq!(serde_json::to_value(account).unwrap()["proxyURL"], "socks5://127.0.0.1:1080");
    }

    #[test]
    fn accepts_supported_proxies_and_rejects_invalid_values() {
        for value in ["", "http://127.0.0.1:8080", "https://127.0.0.1:8080", "socks5://127.0.0.1:1080"] {
            assert!(with_account_proxy(reqwest::Client::builder(), value).is_ok());
        }
        for value in ["bad", "ftp://localhost:21", "http://user:pass@localhost:80", "http://localhost:80/path"] {
            assert!(with_account_proxy(reqwest::Client::builder(), value).is_err());
        }
    }
}
