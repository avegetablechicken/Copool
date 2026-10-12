//! Claude Code's user settings across official and provider switches. They
//! are official while nothing in them replaces the subscription login (the
//! Keychain entry or credentials file); a provider's settings file is merged
//! over them as Claude Code merges a `--settings` file over the user settings.
use serde_json::{Map, Value};

/// `env` variables that take Claude Code off its subscription login.
const ENV_OVERRIDES: &[&str] = &[
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

/// A non-blank string other than `0`/`false`, true, or any other value.
fn is_set(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(s) => !matches!(s.trim(), "" | "0" | "false"),
        Value::Bool(b) => *b,
        _ => true,
    }
}

/// What in the settings replaces the subscription login; none when official.
pub(super) fn overrides(settings: &Value) -> Vec<String> {
    let mut keys = vec![];
    for key in ["apiKeyHelper", "forceLoginGatewayUrl"] {
        if settings.get(key).is_some_and(is_set) {
            keys.push(key.to_owned());
        }
    }
    if matches!(
        settings["forceLoginMethod"].as_str(),
        Some("console" | "gateway")
    ) {
        keys.push("forceLoginMethod".into());
    }
    if let Some(env) = settings["env"].as_object() {
        keys.extend(
            ENV_OVERRIDES
                .iter()
                .filter(|key| env.get(**key).is_some_and(is_set))
                .map(|key| (*key).to_owned()),
        );
    }
    keys
}

/// Removes what replaces the login, keeping the rest of the settings.
pub(super) fn remove_overrides(settings: &mut Value) {
    let keys = overrides(settings);
    let Some(root) = settings.as_object_mut() else {
        return;
    };
    for key in &keys {
        if root.shift_remove(key).is_none() {
            if let Some(env) = root.get_mut("env").and_then(Value::as_object_mut) {
                env.shift_remove(key);
            }
        }
    }
}

/// Primitives compare by value; objects and arrays are distinct, as
/// lodash's `uniq` compares them by reference.
fn duplicate(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Object(_) | Value::Array(_), _) => false,
        _ => a == b,
    }
}

/// Merges a later settings source over an earlier one as Claude Code 2.1.285
/// does (lodash `mergeWith` with its `settingsMergeCustomizer`): objects merge
/// deeply and later values win; arrays concatenate without duplicates, except
/// `fallbackModel`, which the later replaces, as it replaces `modelPicker`;
/// `extraKnownMarketplaces` and `managedMcpServers` merge only by entry.
pub(super) fn merge(base: &mut Value, overlay: &Value) {
    let (Some(base), Some(overlay)) = (base.as_object_mut(), overlay.as_object()) else {
        return;
    };
    merge_object(base, overlay);
}
fn merge_object(base: &mut Map<String, Value>, overlay: &Map<String, Value>) {
    for (key, value) in overlay {
        match base.get_mut(key) {
            Some(slot) => merge_value(slot, value, key),
            None => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}
fn merge_value(slot: &mut Value, value: &Value, key: &str) {
    if key == "modelPicker" {
        *slot = value.clone();
        return;
    }
    match (&mut *slot, value) {
        (Value::Array(base), Value::Array(overlay)) if key != "fallbackModel" => {
            for item in overlay {
                if !base.iter().any(|existing| duplicate(existing, item)) {
                    base.push(item.clone());
                }
            }
        }
        (Value::Object(base), Value::Object(overlay))
            if matches!(key, "extraKnownMarketplaces" | "managedMcpServers") =>
        {
            for (k, v) in overlay {
                base.insert(k.clone(), v.clone());
            }
        }
        (Value::Object(base), Value::Object(overlay)) => merge_object(base, overlay),
        _ => *slot = value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn overrides_name_what_replaces_the_login() {
        let relay = json!({"apiKeyHelper": "", "forceLoginMethod": "claudeai",
            "env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:8787/anthropic",
                "ANTHROPIC_AUTH_TOKEN": "", "ANTHROPIC_API_KEY": "",
                "CLAUDE_CODE_USE_BEDROCK": "0", "CLAUDE_CODE_CUSTOM_OAUTH_URL": "x"}});
        assert!(overrides(&relay).is_empty());
        assert!(overrides(&json!({})).is_empty());
        assert!(overrides(&json!([])).is_empty());
        let gateway = json!({"apiKeyHelper": "helper", "forceLoginMethod": "console",
            "forceLoginGatewayUrl": "https://gateway.invalid",
            "env": {"ANTHROPIC_AUTH_TOKEN": "fake", "CLAUDE_CODE_USE_VERTEX": true,
                "CLAUDE_CODE_USE_POWERSHELL_TOOL": "1"}});
        assert_eq!(
            overrides(&gateway),
            [
                "apiKeyHelper",
                "forceLoginGatewayUrl",
                "forceLoginMethod",
                "ANTHROPIC_AUTH_TOKEN",
                "CLAUDE_CODE_USE_VERTEX"
            ]
        );
        let mut stripped = gateway.clone();
        remove_overrides(&mut stripped);
        assert_eq!(
            stripped,
            json!({"env": {"CLAUDE_CODE_USE_POWERSHELL_TOOL": "1"}})
        );
        let mut blank = relay.clone();
        remove_overrides(&mut blank);
        assert_eq!(blank, relay);
    }

    #[test]
    fn merge_follows_claude_code_settings_sources() {
        let mut base = json!({"model": "opus", "permissions": {"allow": ["Read", "Bash(ls)"],
                "defaultMode": "default"},
            "env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:8787/anthropic",
                "ANTHROPIC_AUTH_TOKEN": "", "DISABLE_TELEMETRY": "1"},
            "fallbackModel": ["sonnet"], "modelPicker": {"options": ["a"], "extra": 1},
            "extraKnownMarketplaces": {"one": {"source": {"repo": "a/one"}}},
            "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "a"}]}]},
            "statusLine": {"type": "command", "command": "status"}});
        let overlay = json!({"model": "relay-model", "permissions": {"allow": ["Read", "Edit"]},
            "env": {"ANTHROPIC_BASE_URL": "https://relay.invalid", "ANTHROPIC_AUTH_TOKEN": "fake"},
            "fallbackModel": ["haiku"], "modelPicker": {"options": ["b"]},
            "extraKnownMarketplaces": {"one": {"source": {"path": "/one"}}, "two": {}},
            "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "a"}]}]},
            "statusLine": null});
        merge(&mut base, &overlay);
        assert_eq!(
            base,
            json!({"model": "relay-model",
                "permissions": {"allow": ["Read", "Bash(ls)", "Edit"], "defaultMode": "default"},
                "env": {"ANTHROPIC_BASE_URL": "https://relay.invalid",
                    "ANTHROPIC_AUTH_TOKEN": "fake", "DISABLE_TELEMETRY": "1"},
                "fallbackModel": ["haiku"], "modelPicker": {"options": ["b"]},
                "extraKnownMarketplaces": {"one": {"source": {"path": "/one"}}, "two": {}},
                "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "a"}]},
                    {"hooks": [{"type": "command", "command": "a"}]}]},
                "statusLine": null})
        );
        // Base keys keep their place; new ones follow.
        let mut base = json!({"z": 1, "a": {"y": 1}});
        merge(&mut base, &json!({"b": 2, "a": {"x": 2}}));
        assert_eq!(base.to_string(), r#"{"z":1,"a":{"y":1,"x":2},"b":2}"#);
    }
}
