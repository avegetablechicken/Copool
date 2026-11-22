//! Reversible Grok Build settings for model discovery or a single keyed route.
use super::{configured_models, Result, CONFIG_INVALID, MODELS_TAKEN, NO_MODELS};
use serde::{Deserialize, Serialize};
use toml_edit::{value, DocumentMut, Item, Table};

pub(super) const XAI_KEY: &str = "XAI_API_KEY";
/// Sparse TOML documents containing just the fields the switch set and the
/// corresponding originals. An empty original table preserves a user's
/// pre-existing table when its last managed field is removed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(super) struct Layout {
    applied: String,
    previous: String,
    /// Name of the shared provider in older layouts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
}

fn selected_model(doc: &DocumentMut, listed: &[String], previous: Option<&str>) -> Result<String> {
    let roles = configured_models(doc);
    if roles.is_empty() && listed.is_empty() {
        return Err(NO_MODELS.into());
    }
    // Prefer the user's default. Keep a formerly managed default when the
    // new gateway also offers it, otherwise follow its catalog order.
    let available = |id: &&str| {
        !doc.get("model")
            .and_then(|t| t.get(*id))
            .is_some_and(own_route)
    };
    roles
        .iter()
        .map(String::as_str)
        .chain(previous.filter(|id| listed.iter().any(|m| m == id)))
        .chain(listed.iter().map(String::as_str))
        .find(available)
        .map(str::to_owned)
        .ok_or_else(|| MODELS_TAKEN.into())
}

fn own_route(model: &Item) -> bool {
    [
        "base_url",
        "api_base_url",
        "model_provider",
        "api_key",
        "auth_provider",
        "env_http_headers",
        "extra_headers",
        "mtls_cert_dir",
    ]
    .iter()
    .any(|k| model.get(k).is_some())
}

/// Global endpoints also apply to models without a routing override. Keep
/// the gateway scoped if that would redirect a user's other credentials.
fn global_endpoints(doc: &DocumentMut, variable: &str) -> bool {
    if variable != XAI_KEY
        || ["auth", "grok_com_config"].iter().any(|section| {
            doc.get(section)
                .and_then(|t| t.get("auth_provider_command"))
                .is_some()
        })
    {
        return false;
    }
    !doc.get("model")
        .and_then(Item::as_table)
        .is_some_and(|models| {
            models.iter().any(|(_, m)| {
                m.get("base_url").is_none()
                    && m.get("model_provider").is_none()
                    && (own_route(m) || m.get("env_key").is_some())
            })
        })
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

impl Layout {
    pub(super) fn apply(
        doc: &mut DocumentMut,
        listed: &[String],
        gateway: &str,
        variable: &str,
        previous_default: Option<&str>,
    ) -> Result<Self> {
        let selected = selected_model(doc, listed, previous_default)?;
        let mut template = DocumentMut::new();

        // The CLI's global catalog authenticates with XAI_API_KEY regardless
        // of a model's env_key. For a provider-specific key keep the gateway
        // and key on the selected model only.
        // Likewise do not redirect an unrelated model's existing credentials.
        let model = if global_endpoints(doc, variable) {
            let endpoints = table_at(template.as_table_mut(), "endpoints")?;
            endpoints.insert("models_base_url", value(gateway));
            endpoints.insert("models_list_url", value(format!("{gateway}/models")));
            endpoints.insert("xai_api_base_url", value(gateway));
            // cli_chat_proxy_base_url also receives the signed-in session's
            // settings and feedback requests, even with preferred_method.
            table_at(template.as_table_mut(), "auth")?.insert("preferred_method", value("api_key"));
            None
        } else {
            let models = table_at(template.as_table_mut(), "model")?;
            models.set_implicit(true);
            let model = table_at(models, &selected)?;
            model.insert("base_url", value(gateway));
            model.insert("env_key", value(variable));
            model.insert("api_backend", value("responses"));
            if doc
                .get("models")
                .and_then(|t| t.get("default"))
                .and_then(Item::as_str)
                != Some(&selected)
            {
                table_at(template.as_table_mut(), "models")?
                    .insert("default", value(selected.as_str()));
            }
            Some(selected)
        };
        let mut previous = DocumentMut::new();
        merge(
            doc.as_table_mut(),
            template.as_table(),
            previous.as_table_mut(),
        )?;
        Ok(Self {
            applied: template.to_string(),
            previous: previous.to_string(),
            provider: None,
            model,
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
        Ok(())
    }

    // The old gateway may still be live after a provider edit. CURRENT also
    // requires the credentials and default route Grok would use now.
    pub(super) fn matches_provider(
        &self,
        doc: &DocumentMut,
        gateway: &str,
        variable: &str,
    ) -> bool {
        if self.model.is_none() && self.provider.is_none() {
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
    fn another_models_credentials_keep_the_gateway_scoped_and_a_user_provider_keeps_its_name() {
        let before = "[model.mine]\napi_key = 'fixture'\n[model_providers.copool-sub2api]\nbase_url = 'https://mine.invalid'\n";
        let mut doc = super::super::parse(Some(before)).unwrap();
        let layout = Layout::apply(
            &mut doc,
            &["grok-4.5".into()],
            "https://relay.invalid/v1",
            XAI_KEY,
            None,
        )
        .unwrap();
        assert!(doc.get("endpoints").is_none());
        assert_eq!(
            doc["model"]["grok-4.5"]["base_url"].as_str(),
            Some("https://relay.invalid/v1")
        );
        assert!(!layout.previous.contains("fixture"));
        layout.restore(&mut doc).unwrap();
        assert_eq!(doc.to_string(), before);
    }

    #[test]
    fn a_gateway_with_other_models_gets_one_routable_default() {
        let mut doc =
            super::super::parse(Some("[model.grok-a]\nbase_url = 'https://mine.invalid'\n"))
                .unwrap();
        let listed = ["grok-a".into(), "grok-b".into(), "grok-c".into()];
        Layout::apply(
            &mut doc,
            &listed,
            "https://relay.invalid/v1",
            "MY_KEY",
            None,
        )
        .unwrap();
        assert_eq!(doc["models"]["default"].as_str(), Some("grok-b"));
        assert!(doc["model"]["grok-a"].get("env_key").is_none());
        assert!(doc["model"].get("grok-c").is_none());
    }
}
