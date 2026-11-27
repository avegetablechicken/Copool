use crate::{application::Paths, codex_config, models::StoredAccount};
use serde_json::json;
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
        return shell_key(name, Shell::Login);
    }
    None
}

/// The shell startup files a probe reads.
#[derive(Clone, Copy)]
pub(crate) enum Shell {
    /// A login shell's, as an app started from the Dock gets none of.
    Login,
    /// An interactive login shell's too, such as `~/.zshrc`, where keys for
    /// tools started from a terminal are often exported.
    Interactive,
}
/// Set while a shell is asked what the user exports: the startup file
/// line Copool adds to load agy's `.env` then loads nothing, so that a key
/// it assigns there for one provider is not taken for the user's own.
pub(crate) const OWN_EXPORTS_ONLY: &str = "COPOOL_OWN_EXPORTS_ONLY";
/// An environment variable as this process has it, else as the user's shell
/// exports it, asked at most every 30 seconds per variable.
pub(crate) fn environment_key(name: &str, shell: Shell) -> Option<String> {
    if let Ok(value) = std::env::var(name) {
        if !value.trim().is_empty() {
            return Some(value);
        }
    }
    shell_key(name, shell)
}
/// Environment variables as one shell the user opens now exports them,
/// which is what the tools started from it get: asked again, not this
/// process's. Elsewhere than Unix, where no shell is asked, this process's.
pub(crate) fn new_shell_keys(names: &[&str], shell: Shell) -> Vec<Option<String>> {
    new_shell_keys_filtered(names, shell, false)
}
/// Like `new_shell_keys`, but ignoring any `.env` block Copool loads from the
/// user's shell startup file (`OWN_EXPORTS_ONLY`).
pub(crate) fn new_shell_own_keys(names: &[&str], shell: Shell) -> Vec<Option<String>> {
    new_shell_keys_filtered(names, shell, true)
}
fn new_shell_keys_filtered(names: &[&str], shell: Shell, own_only: bool) -> Vec<Option<String>> {
    #[cfg(unix)]
    if names.iter().all(|name| is_variable(name)) {
        let program = std::env::var("SHELL").unwrap_or("/bin/sh".into());
        let (flags, timeout) = shell_flags(shell);
        return login_shell_keys(&program, flags, names, timeout, own_only);
    }
    #[cfg(not(unix))]
    let _ = (shell, own_only);
    names
        .iter()
        .map(|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
        .collect()
}
/// Whether `name` is an environment variable's name a shell can expand.
pub(crate) fn is_variable(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}
#[cfg(unix)]
fn shell_flags(shell: Shell) -> (&'static str, std::time::Duration) {
    match shell {
        Shell::Login => ("-lc", std::time::Duration::from_secs(3)),
        // Interactive startup files load more; they get longer.
        Shell::Interactive => ("-lic", std::time::Duration::from_secs(5)),
    }
}
#[cfg_attr(not(unix), allow(unused_variables))]
fn shell_key(name: &str, shell: Shell) -> Option<String> {
    #[cfg(unix)]
    if is_variable(name) {
        let program = std::env::var("SHELL").unwrap_or("/bin/sh".into());
        use std::sync::{LazyLock, Mutex};
        type Lookups = std::collections::BTreeMap<String, (std::time::Instant, Option<String>)>;
        static CACHE: LazyLock<Mutex<Lookups>> = LazyLock::new(Default::default);
        let (flags, timeout) = shell_flags(shell);
        let key = match shell {
            Shell::Login => name.to_owned(),
            Shell::Interactive => format!("interactive:{name}"),
        };
        if let Some((time, value)) = CACHE.lock().ok()?.get(&key) {
            if time.elapsed() < std::time::Duration::from_secs(30) {
                return value.clone();
            }
        }
        // Asked without the cache's lock: the shell may take seconds, and
        // other variables are read meanwhile.
        let value = login_shell_keys(&program, flags, &[name], timeout, true).remove(0);
        CACHE
            .lock()
            .ok()?
            .insert(key, (std::time::Instant::now(), value.clone()));
        return value;
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
/// Each of `names` as one shell exports them; none for each when it fails.
/// The names are variable names, which `is_variable` checked. With
/// `own_only`, without what Copool's line loads from agy's `.env`.
#[cfg(unix)]
fn login_shell_keys(
    shell: &str,
    flags: &str,
    names: &[&str],
    timeout: std::time::Duration,
    own_only: bool,
) -> Vec<Option<String>> {
    let output = login_shell_output(shell, flags, names, timeout, own_only);
    (0..names.len())
        .map(|i| {
            let value = output
                .as_deref()?
                .rsplit_once(&format!("__COPOOL_ENV{i}__"))?
                .1
                .split_once("__COPOOL_END__")?
                .0
                .trim();
            (!value.is_empty()).then(|| value.to_owned())
        })
        .collect()
}
#[cfg(unix)]
fn login_shell_output(
    shell: &str,
    flags: &str,
    names: &[&str],
    timeout: std::time::Duration,
    own_only: bool,
) -> Option<String> {
    use std::io::Read;
    let output = tempfile::NamedTempFile::new().ok()?;
    // Profiles may print banners; only the marked values are the keys.
    let command = names
        .iter()
        .enumerate()
        .map(|(i, name)| format!("printf '__COPOOL_ENV{i}__%s__COPOOL_END__' \"${name}\""))
        .collect::<Vec<_>>()
        .join("; ");
    let mut child = std::process::Command::new(shell);
    if own_only {
        child.env(OWN_EXPORTS_ONLY, "1");
    } else {
        child.env_remove(OWN_EXPORTS_ONLY);
    }
    let mut child = child
        .args([flags, &command])
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
    let mut text = String::new();
    output
        .reopen()
        .ok()?
        .take(65536)
        .read_to_string(&mut text)
        .ok()?;
    // Output cut at the limit may hold a partial value: none of it counts.
    (text.len() < 65536).then_some(text)
}

pub(crate) fn candidates(paths: &Paths) -> Result<Vec<StoredAccount>, String> {
    candidates_with_keys(paths, |definition| api_key(definition, paths))
}
pub(crate) fn candidates_with_keys(
    paths: &Paths,
    resolve_key: impl Fn(&codex_config::Definition) -> Option<String>,
) -> Result<Vec<StoredAccount>, String> {
    let providers = crate::providers::codex_providers(paths)?;
    let config = paths.codex_auth.with_file_name("config.toml");
    let defs = codex_config::definitions(&config);
    let current = codex_config::resolve_path(&config);
    let mut result = vec![];
    // Each provider by the `config.toml` provider it is named after.
    for provider in providers {
        let Some(def) = defs
            .iter()
            .find(|d| d.id.eq_ignore_ascii_case(&provider.name))
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
        // A provider none of whose cards Sub2api still serves is no route,
        // as in the Claude and Antigravity proxies.
        let ids = &provider.imported_account_ids;
        let Some(representative) = provider
            .cached_accounts
            .iter()
            .filter(|a| ids.contains(&a.id) && a.status.eq_ignore_ascii_case("active"))
            .max_by(|a, b| {
                crate::ranking::remaining_score(&a.summary(String::new()))
                    .total_cmp(&crate::ranking::remaining_score(&b.summary(String::new())))
            })
            .cloned()
        else {
            continue;
        };
        let route = format!("sub2api-provider:{}", provider.name.to_lowercase());
        let auth = json!({"tokens":{"access_token":key,"account_id":route,"id_token":""},"copoolProvider":{"id":provider.name,"baseUrl":base,"apiKey":key,"allowInsecureTLS":provider.allow_insecure_tls,"preferred":current.id.eq_ignore_ascii_case(&provider.name)}});
        let stored:StoredAccount=serde_json::from_value(json!({"id":route,"label":provider.name,"accountId":route,"authJson":auth,"proxyUrl":provider.proxy_url.trim(),"addedAt":representative.id,"updatedAt":0,"usage":representative.usage})).map_err(|e|e.to_string())?;
        result.push(stored);
    }
    Ok(result)
}
#[cfg(test)]
mod refresh_tests {
    use super::*;

    /// A shell asked what the user exports is told so, for the line that
    /// loads agy's `.env` to load nothing; one asked what a shell opened now
    /// exports is not, even when this process was.
    #[cfg(unix)]
    #[test]
    fn only_a_shell_asked_for_the_users_own_exports_leaves_out_agys_env() {
        let ask = |own_only| {
            login_shell_keys(
                "/bin/sh",
                "-c",
                &[OWN_EXPORTS_ONLY],
                std::time::Duration::from_secs(5),
                own_only,
            )
            .remove(0)
        };
        assert_eq!(ask(true).as_deref(), Some("1"));
        assert_eq!(ask(false), None);
    }
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
}
