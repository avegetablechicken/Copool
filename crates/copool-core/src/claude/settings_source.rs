//! A provider's settings file: named by its path, or by a name searched for
//! in the Claude configuration directory with Coport's selector rules. It is
//! read (and searched for) again on every use; a found path is not stored,
//! nor is any credential-type flag. The file is a Claude Code settings file,
//! as for `claude --settings`.
use super::{Location, Result};
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::PathBuf};

pub(super) struct Source {
    pub document: Value,
    pub base_url: String,
    pub auth_key: &'static str,
}

/// The name a settings file is found by, the inverse of the search: the
/// stem without `settings` and one separator joining it to the rest.
fn name_of(file: &str) -> String {
    let stem = file.strip_suffix(".json").unwrap_or(file);
    let joins = |c: char| matches!(c, '-' | '_' | '.');
    let Some(at) = stem.find("settings") else {
        return stem.into();
    };
    let (before, after) = (&stem[..at], &stem[at + "settings".len()..]);
    let name = match before.strip_suffix(joins) {
        Some(before) => format!("{before}{after}"),
        None => format!("{before}{}", after.strip_prefix(joins).unwrap_or(after)),
    };
    match name.trim_matches(joins) {
        "" => stem.into(),
        name => name.into(),
    }
}

/// The name and settings path of the Settings input: a path (with a path
/// separator or `~`) names its file, the name being the one the search would
/// find it by; anything else is a name to search for, with no path.
pub(super) fn parse(input: &str) -> Result<(String, String)> {
    let input = input.trim();
    if input.is_empty() {
        return Err("error.sub2api.configuration_incomplete".into());
    }
    if input.starts_with('~') || input.contains(['/', '\\']) {
        let file = input.rsplit(['/', '\\']).next().unwrap_or_default();
        if file.is_empty() || file == "~" {
            return Err("The Claude provider settings path must name a file".into());
        }
        return Ok((name_of(file), input.into()));
    }
    if input.contains(':') || input.chars().any(char::is_control) {
        return Err(
            "Claude provider settings must be a settings filename, selector or path".into(),
        );
    }
    Ok((input.into(), String::new()))
}

/// The absolute path of a settings file; `~/` names the home directory.
fn resolve(path: &str) -> Result<PathBuf> {
    let resolved = match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .ok_or("Cannot determine home directory")?
            .join(rest),
        None => PathBuf::from(path),
    };
    if !resolved.is_absolute() {
        return Err("The Claude provider settings path must be absolute".into());
    }
    Ok(resolved)
}

/// The one file `name` selects: files whose names contain both `settings`
/// and the name take precedence over the exact file name or stem. Symlinks,
/// `.git` and the main settings are skipped.
fn find(location: &Location, name: &str) -> Result<PathBuf> {
    let mut preferred = BTreeSet::new();
    let mut exact = BTreeSet::new();
    let mut pending = vec![location.config_dir.clone()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("Cannot search Claude settings directory".into()),
        };
        for entry in entries {
            let entry = entry.map_err(|_| "Cannot read Claude settings entry")?;
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err("Cannot read Claude settings entry".into()),
            };
            if kind.is_dir() {
                if entry.file_name() != ".git" {
                    pending.push(entry.path());
                }
                continue;
            }
            if !kind.is_file() || entry.path() == location.config_dir.join("settings.json") {
                continue;
            }
            let filename = entry.file_name();
            let Some(filename) = filename.to_str() else {
                continue;
            };
            let Some(stem) = filename.strip_suffix(".json") else {
                continue;
            };
            if filename.contains("settings") && filename.contains(name) {
                preferred.insert(entry.path());
            }
            if stem == name || filename == name {
                exact.insert(entry.path());
            }
        }
    }
    let matches = if preferred.is_empty() {
        exact
    } else {
        preferred
    };
    if matches.len() > 1 {
        return Err(
            "Multiple Claude settings files match the provider; use a more specific filename"
                .into(),
        );
    }
    matches
        .into_iter()
        .next()
        .ok_or_else(|| "Claude provider settings file not found".into())
}

/// A provider's settings: they route to its gateway with its own credential.
/// Without a path, the file is searched for by `name`.
pub(super) fn load(location: &Location, name: &str, path: &str) -> Result<Source> {
    let path = if path.trim().is_empty() {
        parse(name)?;
        find(location, name.trim())?
    } else {
        resolve(path.trim())?
    };
    let raw =
        fs::read_to_string(path).map_err(|_| "Cannot read the Claude provider settings file")?;
    let document: Value =
        serde_json::from_str(&raw).map_err(|_| "Invalid Claude provider settings JSON")?;
    if !document.is_object() {
        return Err("Claude provider settings must be an object".into());
    }
    if document.get("env").is_some_and(|v| !v.is_object()) {
        return Err("Claude provider settings env must be an object".into());
    }
    let env = &document["env"];
    let base_url = env["ANTHROPIC_BASE_URL"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or("Claude provider settings require ANTHROPIC_BASE_URL")?
        .trim()
        .to_owned();
    let url = reqwest::Url::parse(&base_url).map_err(|_| "Invalid Claude provider base URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Invalid Claude provider base URL".into());
    }
    let auth_key = ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"]
        .into_iter()
        .find(|key| env[*key].as_str().is_some_and(|s| !s.trim().is_empty()))
        .ok_or("Claude provider settings require ANTHROPIC_AUTH_TOKEN or ANTHROPIC_API_KEY")?;
    Ok(Source {
        document,
        base_url,
        auth_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write(dir: &std::path::Path, file: &str, key: &str) -> PathBuf {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            json!({"env": {"ANTHROPIC_BASE_URL": "https://relay.invalid",
            key: "fake"}, "model": "source-model"})
            .to_string(),
        )
        .unwrap();
        path
    }

    #[test]
    fn the_settings_input_is_a_path_or_a_name() {
        assert_eq!(
            parse(" ~/.claude/profiles/team.json ").unwrap(),
            ("team".into(), "~/.claude/profiles/team.json".into())
        );
        assert_eq!(
            parse("/etc/claude/team-settings").unwrap(),
            ("team".into(), "/etc/claude/team-settings".into())
        );
        assert_eq!(
            parse(r"C:\Users\me\team.json").unwrap(),
            ("team".into(), r"C:\Users\me\team.json".into())
        );
        assert_eq!(parse("team").unwrap(), ("team".into(), String::new()));
        // The name is the one the search would find the file by.
        for (file, name) in [
            ("settings-team.json", "team"),
            ("team-settings.json", "team"),
            ("team.settings.json", "team"),
            ("team_settings", "team"),
            ("my-settings-team.json", "my-team"),
            ("teamsettings.json", "team"),
            ("settings.json", "settings"),
            ("myServer.json", "myServer"),
        ] {
            assert_eq!(
                parse(&format!("~/.claude/{file}")).unwrap().0,
                name,
                "{file}"
            );
        }
        assert_eq!(
            parse("settings-team.json").unwrap(),
            ("settings-team.json".into(), String::new())
        );
        assert_eq!(
            parse("  ").unwrap_err(),
            "error.sub2api.configuration_incomplete"
        );
        assert!(parse("/claude/").is_err());
        assert!(parse("a:b").is_err());
    }

    #[test]
    fn discovery_matches_coport_priority_and_ambiguity() {
        let dir = tempfile::tempdir().unwrap();
        let location = Location::files(dir.path().into());
        let load = |name: &str| load(&location, name, "");
        write(dir.path(), "profiles/team.json", "ANTHROPIC_API_KEY");
        assert_eq!(load("team").unwrap().auth_key, "ANTHROPIC_API_KEY");
        write(dir.path(), "settings-team.json", "ANTHROPIC_AUTH_TOKEN");
        assert_eq!(load("team").unwrap().auth_key, "ANTHROPIC_AUTH_TOKEN");
        write(
            dir.path(),
            "profiles/team-settings.json",
            "ANTHROPIC_API_KEY",
        );
        assert!(load("team").is_err());
        assert!(load("settings-team.json").is_ok());
        assert!(load("a:b").is_err());
        write(dir.path(), ".git/settings-hidden.json", "ANTHROPIC_API_KEY");
        assert!(load("hidden").is_err());
        let main_only = tempfile::tempdir().unwrap();
        write(main_only.path(), "settings.json", "ANTHROPIC_API_KEY");
        assert!(super::load(
            &Location::files(main_only.path().into()),
            "settings.json",
            ""
        )
        .is_err());
    }

    #[test]
    fn a_path_names_its_file_and_every_load_reads_it_again() {
        let dir = tempfile::tempdir().unwrap();
        let location = Location::files(dir.path().join("claude"));
        let path = write(dir.path(), "providers/team.json", "ANTHROPIC_AUTH_TOKEN");
        let name = path.to_str().unwrap();
        // The path is used as given, even outside the configuration directory.
        assert_eq!(
            load(&location, "team", name).unwrap().auth_key,
            "ANTHROPIC_AUTH_TOKEN"
        );
        write(dir.path(), "providers/team.json", "ANTHROPIC_API_KEY");
        let source = load(&location, "team", name).unwrap();
        assert_eq!(source.auth_key, "ANTHROPIC_API_KEY");
        assert_eq!(source.base_url, "https://relay.invalid");
        assert_eq!(source.document["model"], "source-model");
        assert!(load(&location, "team", "").is_err());
        fs::write(&path, "broken").unwrap();
        assert!(load(&location, "team", name).is_err());
        fs::remove_file(&path).unwrap();
        assert!(load(&location, "team", name).is_err());
        assert!(load(&location, "team", "team.json")
            .err()
            .unwrap()
            .contains("absolute"));
    }

    #[test]
    fn settings_need_a_gateway_and_its_credential() {
        let dir = tempfile::tempdir().unwrap();
        let location = Location::files(dir.path().into());
        let path = dir.path().join("relay.json");
        for document in [
            json!([]),
            json!({"env": []}),
            json!({"env": {"ANTHROPIC_AUTH_TOKEN": "fake"}}),
            json!({"env": {"ANTHROPIC_BASE_URL": "https://user:secret@relay.invalid",
                "ANTHROPIC_AUTH_TOKEN": "fake"}}),
            json!({"env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:8787/anthropic",
                "ANTHROPIC_AUTH_TOKEN": "", "ANTHROPIC_API_KEY": ""}}),
            // Root-level variables are not Claude Code settings.
            json!({"ANTHROPIC_BASE_URL": "https://relay.invalid", "ANTHROPIC_API_KEY": "fake"}),
        ] {
            fs::write(&path, document.to_string()).unwrap();
            assert!(load(&location, "relay", "").is_err(), "{document}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        write(outside.path(), "settings-team.json", "ANTHROPIC_API_KEY");
        assert!(load(&Location::files(dir.path().into()), "team", "").is_err());
    }
}
