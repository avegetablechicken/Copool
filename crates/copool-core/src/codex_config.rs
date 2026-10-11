//! Read Codex provider definitions and edit only the selected provider value.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, Item};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    pub id: String,
    pub base_url: Option<String>,
    pub wire_api: Option<String>,
    pub env_key: Option<String>,
    pub requires_openai_auth: Option<bool>,
}
impl Definition {
    pub fn is_official(&self) -> bool {
        self.id.eq_ignore_ascii_case("openai")
    }
}
fn definition(doc: &DocumentMut, id: &str) -> Definition {
    let table = doc
        .get("model_providers")
        .and_then(Item::as_table_like)
        .and_then(|t| {
            t.iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(id))
                .map(|(_, v)| v)
        });
    let value = |key: &str| {
        table
            .and_then(|t| t.get(key))
            .and_then(Item::as_str)
            .map(|s| s.trim().to_owned())
    };
    Definition {
        id: id.into(),
        base_url: value("base_url").map(|s| s.trim_matches('/').into()),
        wire_api: value("wire_api"),
        env_key: value("env_key"),
        requires_openai_auth: table
            .and_then(|t| t.get("requires_openai_auth"))
            .and_then(Item::as_bool),
    }
}
fn selected(doc: &DocumentMut) -> String {
    doc.get("profile")
        .and_then(Item::as_str)
        .and_then(|profile| {
            doc.get("profiles")
                .and_then(|p| p.get(profile))
                .and_then(|p| p.get("model_provider"))
                .and_then(Item::as_str)
        })
        .or_else(|| doc.get("model_provider").and_then(Item::as_str))
        .unwrap_or("openai")
        .trim()
        .into()
}
/// The top-level `chatgpt_base_url`: where Codex, the proxy and the account
/// usage and workspace requests send ChatGPT requests.
pub fn chatgpt_base_url(path: &Path) -> Option<String> {
    let doc = fs::read_to_string(path).ok()?.parse::<DocumentMut>().ok()?;
    doc.get("chatgpt_base_url")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
pub fn resolve(raw: &str) -> Definition {
    let doc = raw.parse::<DocumentMut>().unwrap_or_default();
    definition(&doc, &selected(&doc))
}
pub fn resolve_path(path: &Path) -> Definition {
    resolve(&fs::read_to_string(path).unwrap_or_default())
}
pub fn paths(path: &Path) -> Vec<PathBuf> {
    let mut result = vec![path.into()];
    if path.file_name().is_some_and(|name| name == "config.toml") {
        if let Some(parent) = path.parent() {
            let mut siblings = fs::read_dir(parent)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|p| {
                    p.file_name().and_then(|n| n.to_str()).is_some_and(|s| {
                        !s.starts_with('.')
                            && s.ends_with(".config.toml")
                            && s.len() > ".config.toml".len()
                    })
                })
                .collect::<Vec<_>>();
            siblings.sort();
            result.extend(siblings);
        }
    }
    result
}
pub fn definitions(path: &Path) -> Vec<Definition> {
    let mut result: Vec<Definition> = Vec::new();
    for path in paths(path) {
        let Ok(raw) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(doc) = raw.parse::<DocumentMut>() else {
            continue;
        };
        let mut ids = doc
            .get("model_providers")
            .and_then(Item::as_table_like)
            .map(|t| t.iter().map(|(key, _)| key.to_owned()).collect::<Vec<_>>())
            .unwrap_or_default();
        ids.sort_by_key(|v| v.to_lowercase());
        if let Some(id) = doc.get("model_provider").and_then(Item::as_str) {
            ids.push(id.into());
        }
        if let Some(profiles) = doc.get("profiles").and_then(Item::as_table_like) {
            for (_, profile) in profiles.iter() {
                if let Some(id) = profile.get("model_provider").and_then(Item::as_str) {
                    ids.push(id.into());
                }
            }
        }
        for id in ids {
            let d = definition(&doc, &id);
            if let Some(primary) = result.iter_mut().find(|p| p.id.eq_ignore_ascii_case(&id)) {
                if primary.base_url.is_none() {
                    primary.base_url = d.base_url;
                }
                if primary.wire_api.is_none() {
                    primary.wire_api = d.wire_api;
                }
                if primary.env_key.is_none() {
                    primary.env_key = d.env_key;
                }
                if primary.requires_openai_auth.is_none() {
                    primary.requires_openai_auth = d.requires_openai_auth;
                }
            } else {
                result.push(d);
            }
        }
    }
    result
}
pub fn matching_provider(url: &str, path: &Path) -> Option<String> {
    let origin = |raw: &str| {
        reqwest::Url::parse(raw)
            .ok()
            .map(|u| u.origin().ascii_serialization())
    };
    let target = origin(url)?;
    let matches = definitions(path)
        .into_iter()
        .filter(|d| d.base_url.as_deref().and_then(origin).as_deref() == Some(&target))
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        Some(matches[0].id.clone())
    } else {
        None
    }
}
fn unquoted(line: &str, wanted: char) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && quote == Some('"') {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
        } else if c == wanted {
            return Some(i);
        }
    }
    None
}
fn header_path(header: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut part = String::new();
    let mut quote = None;
    for c in header.chars() {
        if c == '\'' || c == '"' {
            if quote == Some(c) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(c);
            } else {
                part.push(c);
            }
        } else if c == '.' && quote.is_none() {
            parts.push(part.trim().into());
            part.clear();
        } else {
            part.push(c);
        }
    }
    parts.push(part.trim().into());
    parts
}
pub fn updating(raw: &str, provider: &str) -> Result<String, String> {
    let provider = provider.trim();
    if provider.is_empty() {
        return Err("Provider ID is required".into());
    }
    let doc = raw.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    let target: Vec<String> = doc
        .get("profile")
        .and_then(Item::as_str)
        .filter(|p| {
            doc.get("profiles")
                .and_then(|t| t.get(p))
                .and_then(|t| t.get("model_provider"))
                .is_some()
        })
        .map(|p| vec!["profiles".into(), p.into()])
        .unwrap_or_default();
    let mut lines: Vec<String> = if raw.is_empty() {
        vec![]
    } else {
        raw.split('\n').map(str::to_owned).collect()
    };
    let mut section = Vec::<String>::new();
    let mut insertion = lines
        .iter()
        .position(|line| line.trim_start().starts_with('['))
        .unwrap_or(if lines.last().is_some_and(String::is_empty) {
            lines.len() - 1
        } else {
            lines.len()
        });
    let quoted = serde_json::to_string(provider).map_err(|e| e.to_string())?;
    for line in &mut lines {
        let end = unquoted(line, '#').unwrap_or(line.len());
        let content = line[..end].trim();
        if content.starts_with('[') && content.ends_with(']') {
            section = header_path(content.trim_start_matches('[').trim_end_matches(']'));
            continue;
        }
        if section != target {
            continue;
        }
        let Some(eq) = unquoted(content, '=') else {
            continue;
        };
        if content[..eq].trim().trim_matches(['\'', '"']) != "model_provider" {
            continue;
        }
        let eq = unquoted(line, '=').unwrap();
        let old = &line[eq + 1..end];
        let leading = old.len() - old.trim_start().len();
        let trailing = old.len() - old.trim_end().len();
        *line = format!(
            "{}{}{}{}{}",
            &line[..eq + 1],
            &old[..leading],
            quoted,
            &old[old.len() - trailing..],
            &line[end..]
        );
        return Ok(lines.join("\n"));
    }
    if !target.is_empty() {
        return Err("Active profile provider assignment missing".into());
    }
    insertion = insertion.min(lines.len());
    lines.insert(insertion, format!("model_provider = {quoted}"));
    Ok(lines.join("\n"))
}
pub fn switch(path: &Path, provider: &str) -> Result<(), String> {
    let raw = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    let updated = updating(&raw, provider)?;
    let parent = path.parent().ok_or("Invalid Codex config path")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let permissions = fs::metadata(path).ok().map(|m| m.permissions());
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(updated.as_bytes())
        .map_err(|e| e.to_string())?;
    if let Some(permissions) = permissions {
        file.as_file()
            .set_permissions(permissions)
            .map_err(|e| e.to_string())?;
    }
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
