//! Named settings discovery follows Coport's selector rules. The source remains
//! authoritative and is read again on every switch; no credential-type flag is stored.
use super::{write_text, Location, Result};
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::PathBuf};

pub(super) struct Source {
    pub document: Value,
    pub base_url: String,
    pub auth_key: &'static str,
}

pub(super) fn load(location: &Location, name: &str) -> Result<Source> {
    if name.is_empty()
        || name.trim() != name
        || name.contains(['/', '\\', ':'])
        || name.chars().any(char::is_control)
    {
        return Err(
            "Claude provider name must be a settings filename or selector, not a path".into(),
        );
    }
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
    let path = matches
        .into_iter()
        .next()
        .ok_or("Claude provider settings file not found")?;
    let raw = fs::read_to_string(path).map_err(|_| "Cannot read Claude provider settings file")?;
    let mut document: Value =
        serde_json::from_str(&raw).map_err(|_| "Invalid Claude provider settings JSON")?;
    let object = document
        .as_object_mut()
        .ok_or("Claude provider settings must be an object")?;
    if object.get("env").is_some_and(|v| !v.is_object()) {
        return Err("Claude provider settings env must be an object".into());
    }
    // Coport also accepts root-level environment fields. Normalize these into
    // Claude's native env block, with an existing env entry taking precedence.
    let mut env = object
        .remove("env")
        .unwrap_or_else(|| serde_json::json!({}));
    let keys: Vec<_> = object
        .keys()
        .filter(|k| {
            k.starts_with("ANTHROPIC_")
                || k.starts_with("CLAUDE_CODE_")
                || k.starts_with("_CLAUDE_CODE_")
        })
        .cloned()
        .collect();
    for key in keys {
        let value = object.remove(&key).unwrap();
        env.as_object_mut().unwrap().entry(key).or_insert(value);
    }
    if env.as_object().unwrap().values().any(|v| !v.is_string()) {
        return Err("Claude provider environment values must be strings".into());
    }
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
    object.insert("env".into(), env);
    Ok(Source {
        document,
        base_url,
        auth_key,
    })
}

impl Source {
    pub fn matches(&self, main: &Value) -> bool {
        self.document
            .as_object()
            .unwrap()
            .iter()
            .all(|(key, value)| main.get(key) == Some(value))
    }

    /// Replace the main settings document with the selected native settings.
    /// Validate both before writing and retain the prior document for rollback.
    pub fn sync(&self, location: &Location) -> Result<Option<String>> {
        let path: PathBuf = location.config_dir.join("settings.json");
        let previous = match fs::read_to_string(&path) {
            Ok(raw) => {
                let value: Value =
                    serde_json::from_str(&raw).map_err(|_| "Invalid main Claude settings JSON")?;
                if !value.is_object() {
                    return Err("Main Claude settings must be an object".into());
                }
                Some(raw)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("Cannot read main Claude settings".into()),
        };
        let raw = serde_json::to_string_pretty(&self.document).map_err(|e| e.to_string())?;
        write_text(&path, &raw)?;
        Ok(previous)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn write(location: &Location, file: &str, key: &str) {
        let path = location.config_dir.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, json!({"env":{"ANTHROPIC_BASE_URL":"https://relay.invalid", key:"fake"},"model":"source-model"}).to_string()).unwrap();
    }
    #[test]
    fn discovery_matches_coport_priority_and_ambiguity() {
        let dir = tempfile::tempdir().unwrap();
        let location = Location::files(dir.path().into());
        write(&location, "profiles/team.json", "ANTHROPIC_API_KEY");
        assert_eq!(
            load(&location, "team").unwrap().auth_key,
            "ANTHROPIC_API_KEY"
        );
        write(&location, "settings-team.json", "ANTHROPIC_AUTH_TOKEN");
        assert_eq!(
            load(&location, "team").unwrap().auth_key,
            "ANTHROPIC_AUTH_TOKEN"
        );
        write(
            &location,
            "profiles/team-settings.json",
            "ANTHROPIC_API_KEY",
        );
        assert!(load(&location, "team").is_err());
        assert!(load(&location, "settings-team.json").is_ok());
        assert!(load(&location, "../team").is_err());
        write(&location, ".git/settings-hidden.json", "ANTHROPIC_API_KEY");
        assert!(load(&location, "hidden").is_err());
        let main_only = tempfile::tempdir().unwrap();
        let main_location = Location::files(main_only.path().into());
        write(&main_location, "settings.json", "ANTHROPIC_API_KEY");
        assert!(load(&main_location, "settings.json").is_err());
    }
    #[test]
    fn sync_rereads_source_and_removes_old_main_settings_without_changing_source() {
        let dir = tempfile::tempdir().unwrap();
        let location = Location::files(dir.path().into());
        write(&location, "team.settings.json", "ANTHROPIC_AUTH_TOKEN");
        let source_path = dir.path().join("team.settings.json");
        let original = fs::read(&source_path).unwrap();
        fs::write(dir.path().join("settings.json"), json!({"env":{"ANTHROPIC_API_KEY":"stale","_CLAUDE_CODE_ASSUME_FIRST_PARTY_BASE_URL":"1"},"model":"old"}).to_string()).unwrap();
        load(&location, "team").unwrap().sync(&location).unwrap();
        assert_eq!(fs::read(&source_path).unwrap(), original);
        let main: Value =
            serde_json::from_slice(&fs::read(dir.path().join("settings.json")).unwrap()).unwrap();
        assert_eq!(main["model"], "source-model");
        assert!(main["env"].get("ANTHROPIC_API_KEY").is_none());
        assert!(main["env"]
            .get("_CLAUDE_CODE_ASSUME_FIRST_PARTY_BASE_URL")
            .is_none());
        write(&location, "team.settings.json", "ANTHROPIC_API_KEY");
        let source = load(&location, "team").unwrap();
        assert_eq!(source.auth_key, "ANTHROPIC_API_KEY");
        source.sync(&location).unwrap();
        let before = fs::read(dir.path().join("settings.json")).unwrap();
        fs::write(&source_path, "broken").unwrap();
        assert!(load(&location, "team").is_err());
        assert_eq!(fs::read(dir.path().join("settings.json")).unwrap(), before);
        fs::remove_file(source_path).unwrap();
        assert!(load(&location, "team").is_err());
    }
    #[test]
    fn root_fields_are_normalized_and_broken_main_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let location = Location::files(dir.path().into());
        fs::write(dir.path().join("team.json"), json!({"ANTHROPIC_BASE_URL":"https://root.invalid", "ANTHROPIC_API_KEY":"root-key", "env":{"ANTHROPIC_BASE_URL":"https://env.invalid"}, "permissions":{"allow":["Read"]}}).to_string()).unwrap();
        let source = load(&location, "team").unwrap();
        assert_eq!(source.base_url, "https://env.invalid");
        assert_eq!(source.document["env"]["ANTHROPIC_API_KEY"], "root-key");
        assert!(source.document.get("ANTHROPIC_API_KEY").is_none());
        fs::write(dir.path().join("settings.json"), "broken").unwrap();
        assert!(source.sync(&location).is_err());
        assert_eq!(
            fs::read_to_string(dir.path().join("settings.json")).unwrap(),
            "broken"
        );
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let location = Location::files(dir.path().into());
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        write(
            &Location::files(outside.path().into()),
            "settings-team.json",
            "ANTHROPIC_API_KEY",
        );
        assert!(load(&location, "team").is_err());
    }
}
