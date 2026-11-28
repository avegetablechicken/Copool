use crate::{endpoint::EndpointRequests, usage};
use serde::{Deserialize, Serialize};
use std::time::Duration;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub account_id: String,
    pub workspace_name: Option<String>,
    pub structure: Option<String>,
}
#[derive(Deserialize)]
struct Envelope {
    items: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    id: String,
    name: Option<String>,
    structure: Option<String>,
}
pub struct Client {
    client: reqwest::Client,
    endpoints: Vec<String>,
    preferences: EndpointRequests,
}
impl Client {
    pub fn new(client: reqwest::Client, endpoints: Vec<String>) -> Self {
        Self {
            client,
            endpoints,
            preferences: Default::default(),
        }
    }
    pub fn request(&self, url: &str, token: &str) -> reqwest::RequestBuilder {
        self.client
            .get(url)
            .timeout(Duration::from_secs(18))
            .bearer_auth(token)
            .header("Accept", "application/json")
            .header("User-Agent", "codex-tools-swift/0.1")
    }
    pub async fn fetch(&self, token: &str) -> Result<Vec<Metadata>, String> {
        for attempt in 0..2 {
            let client = self.client.clone();
            let token = token.to_owned();
            let result = self
                .preferences
                .fetch(
                    "workspace-metadata",
                    &self.endpoints,
                    move |url| {
                        client
                            .get(url)
                            .timeout(Duration::from_secs(18))
                            .bearer_auth(&token)
                            .header("Accept", "application/json")
                            .header("User-Agent", "codex-tools-swift/0.1")
                    },
                    |value| serde_json::from_value::<Envelope>(value).map_err(|e| e.to_string()),
                )
                .await;
            match result {
                Ok(result) => {
                    return Ok(result
                        .value
                        .items
                        .into_iter()
                        .map(|i| Metadata {
                            account_id: i.id,
                            workspace_name: i.name,
                            structure: i.structure,
                        })
                        .collect())
                }
                Err(error) => {
                    if attempt == 0 && html_forbidden(&error) {
                        continue;
                    }
                    return Err(preferred_failure(&[error.clone()]).unwrap_or(error));
                }
            }
        }
        unreachable!("the second attempt always returns")
    }
}
fn html_forbidden(value: &str) -> bool {
    let value = value.to_lowercase();
    value.contains("403")
        && ["<html", "<head", "<meta "]
            .iter()
            .any(|s| value.contains(s))
}
pub fn preferred_failure(errors: &[String]) -> Option<String> {
    for error in errors {
        let lower = error.to_lowercase();
        if lower.contains("cancelled")
            || lower.contains("canceled")
            || lower.contains("timed out")
            || lower.contains("timeout")
            || html_forbidden(error)
        {
            continue;
        }
        let detail = error
            .split_once(": ")
            .map(|(_, v)| v)
            .unwrap_or(error)
            .trim();
        if !detail.is_empty() && !detail.starts_with('<') {
            return Some(detail.into());
        }
    }
    if errors
        .iter()
        .any(|s| s.to_lowercase().contains("cancelled") || s.to_lowercase().contains("canceled"))
    {
        return Some("Workspace discovery was cancelled. Please try again.".into());
    }
    if errors
        .iter()
        .any(|s| s.to_lowercase().contains("timed out") || s.to_lowercase().contains("timeout"))
    {
        return Some("Workspace discovery timed out. Please try again.".into());
    }
    if errors.iter().any(|s| html_forbidden(s)) {
        return Some("Workspace discovery was denied. Refresh or re-import this account auth, then try again.".into());
    }
    None
}
pub fn debug_request(request: &reqwest::Request) -> String {
    usage::debug_request_summary(request)
}
pub fn debug_response(bytes: &[u8]) -> String {
    usage::debug_response_body(bytes)
}
