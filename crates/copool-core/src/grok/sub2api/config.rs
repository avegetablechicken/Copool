//! Reversible session-wide Grok Build endpoints and auth settings for Sub2API.
use super::{Result, CONFIG_INVALID};
use serde::{Deserialize, Serialize};
use toml_edit::{value, DocumentMut, Item, Table};

pub(super) const XAI_KEY: &str = "XAI_API_KEY";
/// Endpoint keys that override `xai_api_base_url` and switch Grok Build into
/// generic custom-models (`chat_completions`) mode; removed while on Sub2API
/// and restored on leave if they existed before.
const OVERRIDING_ENDPOINT_KEYS: [&str; 3] =
    ["models_base_url", "models_list_url", "models_endpoint"];

/// Sparse TOML documents containing just the fields the switch set, removed,
/// and the corresponding originals. An empty original table preserves a user's
/// pre-existing table when its last managed field is removed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(super) struct Layout {
    applied: String,
    previous: String,
    /// Keys temporarily removed while on the gateway, with their original values.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    removed: String,
    /// The provider's key variable applied by this switch (`XAI_API_KEY` or a
    /// custom variable bridged via `~/.grok/.env`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    variable: Option<String>,
    /// Name of the shared provider in older layouts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    /// Single model in older keyed-model layouts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
}

fn table_at<'a>(table: &'a mut Table, key: &str) -> Result<&'a mut Table> {
    table
        .entry(key)
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| CONFIG_INVALID.into())
}

/// Merge only the template's keys, remembering both old values and which
/// tables existed. A malformed section aborts the in-memory draft.
fn merge(doc: &mut Table, template: &Table, previous: &mut Table) -> Result<()> {
    for (key, item) in template {
        if let Some(children) = item.as_table() {
            let existed = doc.contains_key(key);
            let mut old = Table::new();
            let target = table_at(doc, key)?;
            if !existed {
                target.set_implicit(children.is_implicit());
            }
            merge(target, children, &mut old)?;
            if existed {
                previous.insert(key, Item::Table(old));
            }
        } else {
            if let Some(old) = doc.get(key) {
                if !old.is_value() {
                    return Err(CONFIG_INVALID.into());
                }
                previous.insert(key, old.clone());
            }
            doc.insert(key, item.clone());
        }
    }
    Ok(())
}

fn undo(doc: &mut Table, template: &Table, previous: Option<&Table>) {
    for (key, item) in template {
        let old = previous.and_then(|t| t.get(key));
        if let Some(children) = item.as_table() {
            if let Some(table) = doc.get_mut(key).and_then(Item::as_table_mut) {
                undo(table, children, old.and_then(Item::as_table));
                if table.is_empty() && old.is_none() {
                    doc.remove(key);
                }
            }
        } else if super::value_text(doc.get(key)) == super::value_text(Some(item)) {
            match old {
                Some(old) => {
                    doc.insert(key, old.clone());
                }
                None => {
                    doc.remove(key);
                }
            }
        }
    }
}

/// Puts back keys that were temporarily removed while on the gateway, unless
/// the user has since written their own value for that key.
fn restore_removed(doc: &mut Table, removed: &Table) {
    for (key, item) in removed {
        if let Some(children) = item.as_table() {
            let target = match doc.entry(key) {
                toml_edit::Entry::Occupied(entry) => entry.into_mut().as_table_mut(),
                toml_edit::Entry::Vacant(entry) => {
                    entry.insert(Item::Table(Table::new())).as_table_mut()
                }
            };
            if let Some(table) = target {
                restore_removed(table, children);
            }
        } else if !doc.contains_key(key) {
            doc.insert(key, item.clone());
        }
    }
}

impl Layout {
    pub(super) fn apply(
        doc: &mut DocumentMut,
        gateway: &str,
        variable: &str,
        extra_models: &[String],
    ) -> Result<Self> {
        let mut template = DocumentMut::new();
        {
            let endpoints = table_at(template.as_table_mut(), "endpoints")?;
            endpoints.insert("cli_chat_proxy_base_url", value(gateway));
            endpoints.insert("xai_api_base_url", value(gateway));
        }
        table_at(template.as_table_mut(), "auth")?.insert("preferred_method", value("api_key"));
        // `[grok_com_config]` takes precedence over `[auth]` in Grok Build.
        if doc
            .get("grok_com_config")
            .and_then(|t| t.get("preferred_method"))
            .is_some()
        {
            table_at(template.as_table_mut(), "grok_com_config")?
                .insert("preferred_method", value("api_key"));
        }
        // Expose Grok text models that Sub2API forwards upstream even when its
        // `/v1/models` catalog omits them (e.g. `grok-4.7`), inheriting the
        // session's `[endpoints]` and `XAI_API_KEY` without per-model `base_url`.
        for id in extra_models {
            let own_endpoint = doc.get("model").and_then(|t| t.get(id)).is_some_and(|m| {
                ["base_url", "api_base_url", "model_provider"]
                    .iter()
                    .any(|k| m.get(k).is_some())
            });
            if own_endpoint {
                continue;
            }
            let models = table_at(template.as_table_mut(), "model")?;
            models.set_implicit(true);
            table_at(models, id)?.insert("api_backend", value("responses"));
        }

        // Remove endpoint keys that would override `xai_api_base_url` or
        // downgrade inference from `/v1/responses` to `/v1/chat/completions`,
        // recording their original values so leaving restores them.
        let mut removed = DocumentMut::new();
        if let Some(endpoints) = doc.get_mut("endpoints") {
            let endpoints = endpoints.as_table_mut().ok_or(CONFIG_INVALID)?;
            let mut removed_endpoints = Table::new();
            for key in OVERRIDING_ENDPOINT_KEYS {
                if let Some(old) = endpoints.remove(key) {
                    if !old.is_value() {
                        endpoints.insert(key, old);
                        return Err(CONFIG_INVALID.into());
                    }
                    removed_endpoints.insert(key, old);
                }
            }
            if !removed_endpoints.is_empty() {
                removed.insert("endpoints", Item::Table(removed_endpoints));
            }
        }

        let mut previous = DocumentMut::new();
        if let Err(err) = merge(
            doc.as_table_mut(),
            template.as_table(),
            previous.as_table_mut(),
        ) {
            restore_removed(doc.as_table_mut(), removed.as_table());
            return Err(err);
        }
        Ok(Self {
            applied: template.to_string(),
            previous: previous.to_string(),
            removed: removed.to_string(),
            variable: Some(variable.to_owned()),
            provider: None,
            model: None,
        })
    }

    pub(super) fn restore(&self, doc: &mut DocumentMut) -> Result<()> {
        let applied = super::parse(Some(&self.applied))?;
        let previous = super::parse(Some(&self.previous))?;
        undo(
            doc.as_table_mut(),
            applied.as_table(),
            Some(previous.as_table()),
        );
        if !self.removed.is_empty() {
            let removed = super::parse(Some(&self.removed))?;
            restore_removed(doc.as_table_mut(), removed.as_table());
        }
        Ok(())
    }

    /// Whether this layout is the session-wide `cli_chat_proxy_base_url` +
    /// `xai_api_base_url` layout (as opposed to an older `models_base_url`
    /// global layout still on disk before re-switching).
    fn is_session_layout(&self) -> bool {
        self.model.is_none()
            && self.provider.is_none()
            && self.applied.contains("cli_chat_proxy_base_url")
    }

    // The old gateway may still be live after a provider edit. CURRENT also
    // requires the credentials and session endpoints Grok would use now.
    pub(super) fn matches_provider(
        &self,
        doc: &DocumentMut,
        env_text: Option<&str>,
        gateway: &str,
        variable: &str,
    ) -> bool {
        if self.is_session_layout() {
            let endpoints = doc.get("endpoints");
            let url = |key| endpoints.and_then(|t| t.get(key)).and_then(Item::as_str);
            let preferred_ok = doc
                .get("grok_com_config")
                .and_then(|t| t.get("preferred_method"))
                .or_else(|| doc.get("auth").and_then(|t| t.get("preferred_method")))
                .and_then(Item::as_str)
                == Some("api_key");
            let overriding_present = OVERRIDING_ENDPOINT_KEYS
                .iter()
                .any(|key| endpoints.and_then(|t| t.get(key)).is_some());
            let env_ok = if variable == XAI_KEY {
                !super::env_has_block(env_text)
            } else {
                super::env_bridges_variable(env_text, variable)
            };
            return self.variable.as_deref() == Some(variable)
                && url("cli_chat_proxy_base_url") == Some(gateway)
                && url("xai_api_base_url") == Some(gateway)
                && !overriding_present
                && preferred_ok
                && env_ok;
        }
        if self.model.is_none() && self.provider.is_none() {
            // Older global layout (`models_base_url` + `models_list_url`).
            let endpoints = doc.get("endpoints");
            let url = |key| endpoints.and_then(|t| t.get(key)).and_then(Item::as_str);
            return variable == XAI_KEY
                && url("xai_api_base_url") == Some(gateway)
                && url("models_list_url") == Some(format!("{gateway}/models").as_str())
                && doc
                    .get("auth")
                    .and_then(|t| t.get("preferred_method"))
                    .and_then(Item::as_str)
                    == Some("api_key");
        }
        let Some(default) = doc
            .get("models")
            .and_then(|t| t.get("default"))
            .and_then(Item::as_str)
        else {
            return false;
        };
        let Some(model) = doc.get("model").and_then(|t| t.get(default)) else {
            return false;
        };
        let routed = match &self.model {
            Some(id) => default == id,
            None => {
                model.get("base_url").is_none()
                    && model.get("model_provider").and_then(Item::as_str)
                        == self.provider.as_deref()
            }
        };
        routed
            && model.get("api_key").is_none()
            && model.get("env_key").and_then(Item::as_str) == Some(variable)
            && model.get("api_backend").and_then(Item::as_str) == Some("responses")
    }

    pub(super) fn active(&self, doc: &DocumentMut, gateway: &str) -> bool {
        if let Some(id) = &self.model {
            super::on_gateway(doc, gateway, id)
        } else if let Some(name) = &self.provider {
            doc.get("model_providers")
                .and_then(|t| t.get(name))
                .and_then(|p| p.get("base_url"))
                .and_then(Item::as_str)
                == Some(gateway)
                && doc.get("model").and_then(Item::as_table).is_some_and(|t| {
                    t.iter().any(|(_, m)| {
                        m.get("base_url").is_none()
                            && m.get("model_provider").and_then(Item::as_str) == Some(name)
                    })
                })
        } else if self.is_session_layout() {
            let endpoints = doc.get("endpoints");
            let url = |key| endpoints.and_then(|t| t.get(key)).and_then(Item::as_str);
            url("cli_chat_proxy_base_url") == Some(gateway)
                && url("xai_api_base_url") == Some(gateway)
        } else {
            doc.get("endpoints")
                .and_then(|t| t.get("models_base_url"))
                .and_then(Item::as_str)
                == Some(gateway)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_layout_sets_both_endpoints_preserves_user_models_and_restores_verbatim() {
        let before = "[endpoints]\ncli_chat_proxy_base_url = 'http://127.0.0.1:8787/grok/v1'\nxai_api_base_url = 'http://127.0.0.1:8787/grok/v1' # Coport\nmodels_base_url = 'https://old.invalid/v1' # custom\n\n[model.mine]\napi_key = 'fixture'\n[model_providers.copool-sub2api]\nbase_url = 'https://mine.invalid'\n";
        let mut doc = super::super::parse(Some(before)).unwrap();
        let layout = Layout::apply(
            &mut doc,
            "https://relay.invalid/v1",
            "MY_SUB2API_KEY",
            &["grok-4.7".into()],
        )
        .unwrap();
        assert_eq!(
            doc["endpoints"]["cli_chat_proxy_base_url"].as_str(),
            Some("https://relay.invalid/v1")
        );
        assert_eq!(
            doc["endpoints"]["xai_api_base_url"].as_str(),
            Some("https://relay.invalid/v1")
        );
        assert!(doc["endpoints"].get("models_base_url").is_none());
        assert_eq!(doc["auth"]["preferred_method"].as_str(), Some("api_key"));
        assert_eq!(
            doc["model"]["grok-4.7"]["api_backend"].as_str(),
            Some("responses")
        );
        assert!(doc["model"]["grok-4.7"].get("base_url").is_none());
        assert!(doc["model"]["grok-4.7"].get("env_key").is_none());
        assert_eq!(doc["model"]["mine"]["api_key"].as_str(), Some("fixture"));
        assert!(!layout.previous.contains("fixture"));
        layout.restore(&mut doc).unwrap();
        assert_eq!(doc.to_string(), before);
    }

    #[test]
    fn grok_com_config_preferred_method_is_updated_and_restored() {
        let before = "[grok_com_config]\npreferred_method = \"oidc\"\n";
        let mut doc = super::super::parse(Some(before)).unwrap();
        let layout = Layout::apply(&mut doc, "https://relay.invalid/v1", XAI_KEY, &[]).unwrap();
        assert_eq!(
            doc["grok_com_config"]["preferred_method"].as_str(),
            Some("api_key")
        );
        assert_eq!(doc["auth"]["preferred_method"].as_str(), Some("api_key"));
        layout.restore(&mut doc).unwrap();
        assert_eq!(doc.to_string(), before);
    }
}
