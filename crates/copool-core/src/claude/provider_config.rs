//! Select native Claude authentication/routing settings, without a Copool marker.
use super::{set_member, write_text, Location, Result};
use serde_json::{json, Value};
use std::fs;

const FIRST_PARTY: &str = "_CLAUDE_CODE_ASSUME_FIRST_PARTY_BASE_URL";
const BASE: &str = "ANTHROPIC_BASE_URL";
const OVERRIDES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_GATEWAY",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
    "CLAUDE_CODE_USE_MANTLE",
    "ANTHROPIC_PROFILE",
    "ANTHROPIC_FEDERATION_RULE_ID",
    "ANTHROPIC_ORGANIZATION_ID",
];

fn nonempty(value: &Value) -> bool {
    value.as_str().is_some_and(|s| !s.trim().is_empty()) || value.as_bool() == Some(true)
}

fn pool_login(settings: &Value) -> bool {
    if !settings.is_object() || (!settings["env"].is_null() && !settings["env"].is_object()) {
        return false;
    }
    let env = &settings["env"];
    if nonempty(&settings["apiKeyHelper"])
        || nonempty(&settings["forceLoginGatewayUrl"])
        || matches!(
            settings["forceLoginMethod"].as_str(),
            Some("gateway" | "console")
        )
        || OVERRIDES.iter().any(|key| nonempty(&env[*key]))
    {
        return false;
    }
    if nonempty(&env[FIRST_PARTY]) {
        return true;
    }
    match env[BASE].as_str().filter(|s| !s.trim().is_empty()) {
        None => true,
        Some(url) => reqwest::Url::parse(url).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str() == Some("api.anthropic.com")
                && url.port_or_known_default() == Some(443)
                && url.username().is_empty()
                && url.password().is_none()
        }),
    }
}

/// Only the user settings are observable here; this is not process inspection.
pub(super) fn uses_pool_login(location: &Location) -> bool {
    match fs::read_to_string(location.config_dir.join("settings.json")) {
        Ok(raw) => serde_json::from_str(&raw).is_ok_and(|settings| pool_login(&settings)),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// A matching URL alone does not select a gateway's authentication.
pub(super) fn uses_gateway(location: &Location) -> bool {
    let Ok(raw) = fs::read_to_string(location.config_dir.join("settings.json")) else {
        return false;
    };
    let Ok(settings) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    let env = &settings["env"];
    ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"]
        .iter()
        .any(|key| nonempty(&env[*key]))
        && !OVERRIDES
            .iter()
            .filter(|key| !matches!(**key, "ANTHROPIC_AUTH_TOKEN" | "ANTHROPIC_API_KEY"))
            .any(|key| nonempty(&env[*key]))
        && !nonempty(&settings["forceLoginGatewayUrl"])
        && !nonempty(&settings["forceLoginMethod"])
}

/// A single settings write; the caller retains the returned document for rollback.
pub(super) fn select(
    location: &Location,
    gateway: Option<(&str, &str)>,
    leaving_known_gateway: bool,
) -> Result<Option<String>> {
    let path = location.config_dir.join("settings.json");
    let previous = match fs::read_to_string(&path) {
        Ok(raw) => Some(raw),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let raw = previous
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("{}");
    let settings: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    if !settings.is_object() || (!settings["env"].is_null() && !settings["env"].is_object()) {
        return Err("Invalid Claude settings object".into());
    }
    let mut env = settings["env"].as_object().cloned().unwrap_or_default();
    // Preserve a declared first-party relay, never reuse a third-party credential's URL.
    let keep_relay = !leaving_known_gateway
        && (nonempty(&env.get(FIRST_PARTY).cloned().unwrap_or_default())
            || (!OVERRIDES
                .iter()
                .any(|key| env.get(*key).is_some_and(nonempty))
                && !nonempty(&settings["apiKeyHelper"])));
    // Empty values also mask inherited shell variables in normal CLI sessions.
    for key in OVERRIDES {
        env.insert((*key).into(), json!(""));
    }
    env.insert("ANTHROPIC_CUSTOM_HEADERS".into(), json!(""));
    env.remove("ANTHROPIC_UNIX_SOCKET");
    if let Some((url, key)) = gateway {
        env.insert(BASE.into(), json!(url));
        env.insert("ANTHROPIC_API_KEY".into(), json!(key));
        env.insert(FIRST_PARTY.into(), json!(""));
    } else {
        if !keep_relay || !env.get(BASE).is_some_and(nonempty) {
            env.insert(BASE.into(), json!("https://api.anthropic.com"));
        }
        env.insert(FIRST_PARTY.into(), json!("1"));
    }
    let mut updated = set_member(raw, "env", &Value::Object(env))?;
    if settings.get("apiKeyHelper").is_some() {
        updated = set_member(&updated, "apiKeyHelper", &json!(""))?;
    }
    // These fields have enum/URL schemas; an empty value would be invalid.
    if settings.get("forceLoginMethod").is_some() || settings.get("forceLoginGatewayUrl").is_some()
    {
        let mut document: Value = serde_json::from_str(&updated).map_err(|e| e.to_string())?;
        let object = document.as_object_mut().unwrap();
        object.remove("forceLoginMethod");
        object.remove("forceLoginGatewayUrl");
        updated = serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?;
    }
    write_text(&path, &updated)?;
    Ok(previous)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routing_and_authentication_must_both_match() {
        assert!(pool_login(&json!({})));
        assert!(!pool_login(&json!([])));
        assert!(!pool_login(&json!({"env": []})));
        assert!(!pool_login(
            &json!({"env": {"ANTHROPIC_BASE_URL":"https://relay.invalid"}})
        ));
        let mut settings = json!({"env": {BASE:"https://relay.invalid", FIRST_PARTY:"1"}});
        assert!(pool_login(&settings));
        for key in OVERRIDES {
            settings["env"][*key] = json!("override");
            assert!(!pool_login(&settings), "{key}");
            settings["env"].as_object_mut().unwrap().remove(*key);
        }
        assert!(!pool_login(
            &json!({"env": {BASE:"https://api.anthropic.com.evil.invalid"}})
        ));
    }

    #[test]
    fn switches_native_settings_without_changing_unrelated_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let location = Location::files(dir.path().into());
        let path = dir.path().join("settings.json");
        let original = json!({"model":"opus", "permissions":{"allow":[]},
            "env":{BASE:"https://relay.invalid", "DISABLE_TELEMETRY":"1"}})
        .to_string();
        fs::write(&path, &original).unwrap();
        assert_eq!(select(&location, None, false).unwrap(), Some(original));
        assert!(uses_pool_login(&location));
        let settings: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(settings["env"][BASE], "https://relay.invalid");
        select(
            &location,
            Some(("https://gateway.invalid", "fake-key")),
            false,
        )
        .unwrap();
        assert!(!uses_pool_login(&location));
        let settings: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(settings["env"]["ANTHROPIC_API_KEY"], "fake-key");
        assert_eq!(settings["env"]["ANTHROPIC_AUTH_TOKEN"], "");
        select(&location, None, true).unwrap();
        assert!(uses_pool_login(&location));
        let settings: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(settings["env"][BASE], "https://api.anthropic.com");
        assert_eq!(settings["env"]["ANTHROPIC_AUTH_TOKEN"], "");
        assert_eq!(settings["env"]["ANTHROPIC_API_KEY"], "");
        assert_eq!(settings["model"], "opus");
        assert_eq!(settings["env"]["DISABLE_TELEMETRY"], "1");
        fs::write(&path, "broken").unwrap();
        assert!(!uses_pool_login(&location));
        assert!(select(&location, None, false).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "broken");
    }
    #[test]
    fn switching_clears_competing_credentials_and_does_not_reuse_unknown_gateway() {
        let dir = tempfile::tempdir().unwrap();
        let location = Location::files(dir.path().into());
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            json!({"apiKeyHelper":"old-helper", "forceLoginMethod":"gateway",
            "forceLoginGatewayUrl":"https://sso.invalid", "permissions":{"allow":["Read"]},
            "env":{BASE:"https://unknown.invalid", "ANTHROPIC_AUTH_TOKEN":"fake",
                "CLAUDE_CODE_USE_VERTEX":"1"}})
            .to_string(),
        )
        .unwrap();
        select(&location, None, false).unwrap();
        assert!(uses_pool_login(&location));
        let settings: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(settings["env"][BASE], "https://api.anthropic.com");
        assert!(settings.get("forceLoginMethod").is_none());
        assert!(settings.get("forceLoginGatewayUrl").is_none());
        assert_eq!(settings["apiKeyHelper"], "");
        assert_eq!(settings["permissions"]["allow"], json!(["Read"]));
        for key in OVERRIDES {
            assert_eq!(settings["env"][*key], "", "{key}");
        }
    }
}
