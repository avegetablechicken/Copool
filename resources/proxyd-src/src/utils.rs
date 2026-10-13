use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
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
        return value.to_string();
    }
    // Upstream errors are arbitrary text (often Chinese), so the cut has to land
    // on a character boundary instead of panicking mid-character.
    let mut end = max_len;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

/// Compares two secrets without leaking how much of a prefix matched.
pub(crate) fn constant_time_eq(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    std::hint::black_box(difference) == 0
}

/// Same-directory temporary file followed by an atomic replacement, so a crash
/// or a full disk cannot leave a half-written file behind.
pub(crate) fn write_private_file_atomically(path: &Path, contents: &[u8]) -> Result<(), String> {
    // Replace a symlink's target, not the link.
    let path = &fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let parent = path.parent().ok_or("Invalid file path")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(contents).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
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

pub(crate) fn validate_account_proxy(value: &str) -> Result<Option<reqwest::Url>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    // Persist an explicit direct connection in the existing per-account field.
    // It must remain nonempty so provider/global fallback cannot replace it.
    if value == "direct://" {
        return Ok(Some(reqwest::Url::parse(value).unwrap()));
    }
    let url = reqwest::Url::parse(value).map_err(|_| "Invalid account proxy URL".to_string())?;
    let authority = value
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
        .unwrap_or_default();
    let port = authority
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse::<u16>().ok());
    if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
        || url.host_str().is_none()
        || !port.is_some_and(|p| p > 0)
        || authority.contains('@')
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err("Invalid account proxy URL".into());
    }
    Ok(Some(url))
}
/// Chooses the proxy for one upstream URL from the system configuration (or
/// `None` to connect directly). Hosts register one; without it, reqwest's own
/// environment selection applies.
pub(crate) type SystemProxy = fn(&reqwest::Url) -> Option<reqwest::Url>;
static SYSTEM_PROXY: std::sync::OnceLock<SystemProxy> = std::sync::OnceLock::new();
#[allow(dead_code)] // registered by the desktop core, not the remote daemon
pub(crate) fn set_system_proxy(resolve: SystemProxy) {
    let _ = SYSTEM_PROXY.set(resolve);
}
// An explicit account proxy replaces environment selection and never fails
// over; without one, the system proxy for each URL applies. Loopback hosts are
// always reached directly.
pub(crate) fn with_account_proxy(
    builder: reqwest::ClientBuilder,
    value: &str,
) -> Result<reqwest::ClientBuilder, String> {
    // Honor macOS Keychain trust, including locally trusted relay certificates.
    // Loading native roots into Rustls does not reproduce Security Framework's
    // trust evaluation. Select explicitly because both TLS features are enabled.
    #[cfg(target_os = "macos")]
    let builder = builder.use_native_tls();
    if value.trim() == "direct://" {
        return Ok(builder.no_proxy());
    }
    let Some(mut url) = validate_account_proxy(value)? else {
        return Ok(match SYSTEM_PROXY.get() {
            Some(resolve) => builder.proxy(reqwest::Proxy::custom(move |url| resolve(url))),
            None => builder,
        });
    };
    if url.scheme() == "socks5" {
        url.set_scheme("socks5h")
            .map_err(|_| "Invalid proxy scheme")?;
    }
    // A loopback upstream, such as a local relay in chatgpt_base_url, is never
    // reachable through the account's proxy: it is connected to directly.
    let proxy = reqwest::Proxy::all(url.as_str())
        .map_err(|_| "Invalid account proxy URL")?
        .no_proxy(reqwest::NoProxy::from_string("localhost,127.0.0.0/8,::1"));
    Ok(builder.no_proxy().proxy(proxy))
}

#[cfg(test)]
mod account_proxy_tests {
    use super::with_account_proxy;

    #[tokio::test]
    async fn account_proxy_does_not_carry_loopback_upstreams() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await;
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        // Nothing listens on port 9: a request sent through it would fail.
        let client = with_account_proxy(reqwest::Client::builder(), "http://127.0.0.1:9")
            .unwrap()
            .build()
            .unwrap();
        let response = client
            .get(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 204);
    }

    #[tokio::test]
    async fn direct_connection_bypasses_existing_proxy() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await;
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let builder = reqwest::Client::builder()
            .resolve("direct-test.invalid", address)
            .proxy(reqwest::Proxy::all("http://127.0.0.1:9").unwrap())
            .timeout(std::time::Duration::from_secs(3));
        let client = with_account_proxy(builder, "direct://")
            .unwrap()
            .build()
            .unwrap();
        let response = client
            .get(format!("http://direct-test.invalid:{}/", address.port()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 204);
    }

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
        assert_eq!(
            serde_json::to_value(account).unwrap()["proxyURL"],
            "socks5://127.0.0.1:1080"
        );
    }

    #[test]
    fn accepts_supported_proxies_and_rejects_invalid_values() {
        for value in [
            "",
            "http://127.0.0.1:8080",
            "https://127.0.0.1:8080",
            "socks5://127.0.0.1:1080",
        ] {
            assert!(with_account_proxy(reqwest::Client::builder(), value).is_ok());
        }
        for value in [
            "bad",
            "ftp://localhost:21",
            "http://user:pass@localhost:80",
            "http://localhost:80/path",
        ] {
            assert!(with_account_proxy(reqwest::Client::builder(), value).is_err());
        }
    }
}
