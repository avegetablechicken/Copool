//! The Gemini API request agy sends in API-key mode, and Cloud Code's
//! envelope around the same request and each streamed response.
use super::{
    build_streamed_response, candidate_client, invalid_request_response, json_error_response,
    parse_json_request, read_current_api_key, send_over_candidates, sse_data_chunk, Outbound,
    ProxyCandidate, ProxyContext, SseDecoder,
};
use async_stream::stream;
use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, Response, StatusCode},
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    io,
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) const CLOUD_BASE: &str = "https://daily-cloudcode-pa.googleapis.com";
pub(super) type Catalog = Arc<tokio::sync::Mutex<HashMap<String, (Instant, Arc<HashSet<String>>)>>>;
pub(super) struct Generate {
    model: String,
    body: Value,
    streaming: bool,
}

pub(super) async fn handle(
    State(context): State<Arc<ProxyContext>>,
    Path(operation): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    if let Some(response) = super::ensure_authorized(&headers, &context) {
        return response;
    }
    let Some((model, method)) = operation.split_once(':') else {
        return invalid_request_response("Missing model operation");
    };
    if model.is_empty()
        || !model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return invalid_request_response("Invalid model name");
    }
    let streaming = match method {
        "streamGenerateContent" => true,
        "generateContent" => false,
        _ => return json_error_response(StatusCode::NOT_FOUND, "Unknown model operation"),
    };
    let body = match parse_json_request(&body) {
        Ok(body) if body.is_object() => body,
        Ok(_) => return invalid_request_response("Expected a Gemini request object"),
        Err(response) => return response,
    };
    let generate = Generate {
        model: model.into(),
        body,
        streaming,
    };
    let (candidate, response) =
        match send_over_candidates(&context, &headers, &Outbound::Antigravity(&generate)).await {
            Ok(pair) => pair,
            Err(response) => return response,
        };
    if candidate.auth_json.get("copoolProvider").is_some() {
        return build_streamed_response(response);
    }
    if streaming {
        unwrap_stream(response)
    } else {
        match response.json::<Value>().await {
            Ok(mut v) if v["response"].is_object() => {
                axum::response::IntoResponse::into_response(axum::Json(v["response"].take()))
            }
            _ => json_error_response(StatusCode::BAD_GATEWAY, "Invalid Cloud Code response"),
        }
    }
}

impl Generate {
    fn method(&self) -> &'static str {
        if self.streaming {
            "streamGenerateContent?alt=sse"
        } else {
            "generateContent"
        }
    }

    fn cloud_model(&self, models: &HashSet<String>) -> String {
        let base = self.model.strip_suffix("-preview").unwrap_or(&self.model);
        let thinking = &self.body["generationConfig"]["thinkingConfig"];
        let level = thinking["thinkingLevel"]
            .as_str()
            .and_then(thinking_level_suffix)
            .or_else(|| {
                thinking["thinkingBudget"].as_i64().map(|budget| {
                    if budget < 0 || budget > 1024 {
                        "high"
                    } else {
                        "low"
                    }
                })
            });
        if let Some(level) = level {
            let id = format!("{base}-{level}");
            // agy uses the family ID on the Gemini API and sends the effort
            // separately. Cloud Code expects that effort in the ID.
            if models.contains(&id)
                || (!models.contains(&self.model)
                    && !models.contains(base)
                    && known_effort_variant(base, level))
            {
                return id;
            }
        }
        if !models.contains(&self.model) && models.contains(base) {
            return base.into();
        }
        self.model.clone()
    }
}

fn thinking_level_suffix(raw: &str) -> Option<&'static str> {
    let normalized = raw.trim().to_ascii_lowercase();
    let normalized = normalized
        .strip_prefix("thinking_level_")
        .unwrap_or(&normalized);
    match normalized {
        "minimal" => Some("minimal"),
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" | "extra_high" | "extra-high" => Some("xhigh"),
        "max" => Some("max"),
        _ => None,
    }
}

fn known_effort_variant(base: &str, level: &str) -> bool {
    match base {
        "gemini-4-neon" | "gemini-4-hydrogen" => {
            matches!(level, "low" | "medium" | "high" | "xhigh")
        }
        "gemini-4-carbon" | "gemini-4-argon" | "gemini-3.8-flash" | "gemini-3.7-flash"
        | "gemini-3.6-flash" => matches!(level, "low" | "medium" | "high"),
        "gemini-3.1-pro" => matches!(level, "low" | "high"),
        _ => false,
    }
}

pub(super) async fn forward(
    context: &ProxyContext,
    candidate: &ProxyCandidate,
    generate: &Generate,
) -> Result<reqwest::Response, String> {
    let client = candidate_client(context, candidate)?;
    let method = generate.method();
    let token = &candidate.access_token;
    let net_error = |e: reqwest::Error| e.without_url().to_string();
    if let Some(provider) = candidate.auth_json.get("copoolProvider") {
        if crate::utils::constant_time_eq(token, &read_current_api_key(&context.api_key)) {
            return Err("The gateway requires its own upstream API key".into());
        }
        let base = provider["baseUrl"]
            .as_str()
            .ok_or("Missing Antigravity gateway")?
            .trim_end_matches('/');
        let url = format!("{base}/v1beta/models/{}:{method}", generate.model);
        let response = client
            .post(url)
            .header("x-goog-api-key", token)
            .json(&generate.body)
            .send()
            .await
            .map_err(net_error)?;
        if response.status().is_redirection() {
            return Err("Antigravity gateway returned a redirect".into());
        }
        return Ok(response);
    }
    let cloud = |path: &str, user_agent: String| {
        client
            .post(format!(
                "{}/v1internal:{path}",
                context.upstream_base_url.trim_end_matches('/')
            ))
            .bearer_auth(token)
            .header("user-agent", user_agent)
    };
    let project = match candidate.auth_json["project"].as_str() {
        Some(project) => project.to_owned(),
        None => {
            let response = cloud("loadCodeAssist", agent())
                .json(&json!({}))
                .send()
                .await
                .map_err(net_error)?;
            if !response.status().is_success() {
                return Ok(response);
            }
            let body = response.json::<Value>().await.map_err(net_error)?;
            body["cloudaicompanionProject"]
                .as_str()
                .filter(|v| !v.is_empty())
                .ok_or("Antigravity reported no project")?
                .to_owned()
        }
    };
    let cli_ua = cli_agent();
    let cached = {
        let mut catalog = context.antigravity_models.lock().await;
        catalog.retain(|_, (since, _)| since.elapsed() < Duration::from_secs(300));
        catalog.get(&candidate.id).map(|(_, models)| models.clone())
    };
    let model = if let Some(models) = cached {
        generate.cloud_model(&models)
    } else {
        let response = cloud("fetchAvailableModels", cli_ua.clone())
            .json(&json!({"project": project}))
            .send()
            .await
            .map_err(net_error)?;
        if !response.status().is_success() {
            return Ok(response);
        }
        let body = response.json::<Value>().await.map_err(net_error)?;
        let models = Arc::new(
            body["models"]
                .as_object()
                .ok_or("Antigravity reported no model catalog")?
                .keys()
                .cloned()
                .collect(),
        );
        context
            .antigravity_models
            .lock()
            .await
            .insert(candidate.id.clone(), (Instant::now(), Arc::clone(&models)));
        generate.cloud_model(&models)
    };
    cloud(method, cli_ua)
        .json(&proxy_envelope(&project, &model, &generate.body))
        .send()
        .await
        .map_err(net_error)
}

pub(crate) fn envelope(project: &str, model: &str, request: &Value) -> Value {
    json!({"project":project, "model":model, "userAgent":"antigravity",
        "requestId":uuid::Uuid::new_v4().to_string(), "request":request})
}

fn proxy_envelope(project: &str, model: &str, request: &Value) -> Value {
    json!({
        "project": project,
        "model": model,
        "userAgent": "antigravity",
        "requestId": format!("agent/{}", uuid::Uuid::new_v4()),
        "requestType": "agent",
        "request": request,
    })
}

fn unwrap_stream(mut upstream: reqwest::Response) -> Response<Body> {
    let events = stream! {
        let mut decoder = SseDecoder::default();
        loop {
            let chunk = upstream.chunk().await.map_err(io::Error::other)?;
            let events = match &chunk {
                Some(bytes) => decoder.push(bytes),
                None => decoder.finish(),
            };
            for event in events {
                let mut value: Value = serde_json::from_str(&event.data).map_err(io::Error::other)?;
                let data = if value["response"].is_object() {
                    value["response"].take()
                } else if value.get("error").is_some() {
                    value
                } else {
                    Err(io::Error::other("Invalid Cloud Code stream response"))?
                };
                yield Ok::<_, io::Error>(sse_data_chunk(&data));
            }
            if chunk.is_none() { break; }
        }
    };
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(events))
        .unwrap()
}

/// The Antigravity version Copool's quota/account requests name.
const CLIENT_VERSION: &str = "3.0.0";
/// The Antigravity CLI version and changelist Copool's proxy requests name.
/// Cloud Code gates newer CLI models (such as Gemini 4 Neon) behind the
/// `antigravity/cli/<version> (aidev_client; ...)` User-Agent used by `agy`.
const CLI_VERSION: &str = "1.5.12";
const CLI_CL: &str = "1024904911";

fn go_platform() -> (&'static str, &'static str) {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        "x86" => "386",
        other => other,
    };
    (os, arch)
}

/// Antigravity's agent, `antigravity/<version> <os>/<arch>` with Go's names
/// of the system. Cloud Code answers by it: `loadCodeAssist` refuses other
/// clients (`agy`'s included) and reports no plan, and the quota summary
/// leaves out the Gemini pool for a client without a version or one before
/// 2.6, so that a Google AI Pro account would show only its Claude pool.
pub(crate) fn agent() -> String {
    let (os, arch) = go_platform();
    format!("antigravity/{CLIENT_VERSION} {os}/{arch}")
}

/// Antigravity CLI's agent for Cloud Code model catalog and generation proxy
/// calls, matching `agy` 1.5.12's `codeassistclient.userAgent` format.
pub(crate) fn cli_agent() -> String {
    let (os, arch) = go_platform();
    format!(
        "antigravity/cli/{CLI_VERSION} (aidev_client; os_type={os}; arch={arch}; cl={CLI_CL}; auth_method=consumer)"
    )
}

/// The remote node keeps only the login fields it needs in accounts.json.
pub(crate) async fn renew_remote(
    auth: &Value,
    proxy: &str,
    network: &dyn crate::antigravity_cloud::Network,
) -> Result<Value, String> {
    let refresh = auth["refreshToken"]
        .as_str()
        .ok_or("Missing Antigravity refresh token")?;
    let new = crate::antigravity_cloud::renew_token(network, refresh, proxy).await?;
    let mut auth = auth.clone();
    auth["accessToken"] = new.access.into();
    auth["expiresAt"] = new.expires_at.into();
    auth["expiresIn"] = new.lifetime.into();
    if let Some(token) = new.refresh {
        auth["refreshToken"] = token.into();
    }
    Ok(auth)
}

pub(super) async fn remote_usage(
    storage: &super::ProxyStorageContext,
    candidate: &ProxyCandidate,
) -> Result<crate::models::UsageSnapshot, String> {
    use crate::antigravity_cloud::{fetch_usage, Failure, Http};
    async fn fetch(candidate: &ProxyCandidate) -> Result<crate::models::UsageSnapshot, Failure> {
        fetch_usage(
            &Http,
            &candidate.access_token,
            &candidate.proxy_url,
            crate::utils::now_unix_seconds(),
        )
        .await
    }
    let result = match fetch(candidate).await {
        Err(Failure::Expired) if candidate.renews => {
            fetch(&super::refresh_proxy_candidate_auth(storage, candidate).await?).await
        }
        other => other,
    };
    let mut usage = result.map_err(|failure| match failure {
        Failure::Expired => "Antigravity login expired".to_owned(),
        Failure::Other(error) => error,
    })?;
    usage.plan_type = candidate.plan_type.clone();
    Ok(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(model: &str, body: Value) -> Generate {
        Generate {
            model: model.into(),
            body,
            streaming: true,
        }
    }

    #[test]
    fn cloud_model_resolves_family_and_thinking_level_variants() {
        let catalog: HashSet<String> = [
            "gemini-4-neon-high",
            "gemini-4-neon-xhigh",
            "gemini-3.8-flash-high",
            "gemini-3.1-pro-high",
            "gemini-3.1-pro-low",
            "gemini-3-flash",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();

        assert_eq!(
            req(
                "gemini-4-neon",
                json!({"generationConfig": {"thinkingConfig": {"thinkingLevel": "HIGH"}}})
            )
            .cloud_model(&catalog),
            "gemini-4-neon-high"
        );
        assert_eq!(
            req(
                "gemini-4-neon",
                json!({"generationConfig": {"thinkingConfig": {"thinkingLevel": "EXTRA_HIGH"}}})
            )
            .cloud_model(&catalog),
            "gemini-4-neon-xhigh"
        );
        assert_eq!(
            req(
                "gemini-4-neon",
                json!({"generationConfig": {"thinkingConfig": {"thinkingLevel": "THINKING_LEVEL_XHIGH"}}})
            )
            .cloud_model(&catalog),
            "gemini-4-neon-xhigh"
        );
        assert_eq!(
            req(
                "gemini-3.8-flash",
                json!({"generationConfig": {"thinkingConfig": {"thinkingLevel": "HIGH"}}})
            )
            .cloud_model(&catalog),
            "gemini-3.8-flash-high"
        );
        assert_eq!(
            req(
                "gemini-3.1-pro-preview",
                json!({"generationConfig": {"thinkingConfig": {"thinkingBudget": 512}}})
            )
            .cloud_model(&catalog),
            "gemini-3.1-pro-low"
        );
        assert_eq!(
            req("gemini-3-flash-preview", json!({})).cloud_model(&catalog),
            "gemini-3-flash"
        );
        assert_eq!(
            req(
                "gemini-4-neon-high",
                json!({"generationConfig": {"thinkingConfig": {"thinkingLevel": "HIGH"}}})
            )
            .cloud_model(&catalog),
            "gemini-4-neon-high"
        );
        // Fallback for known effort families when absent from the catalog.
        let empty = HashSet::new();
        assert_eq!(
            req(
                "gemini-4-neon",
                json!({"generationConfig": {"thinkingConfig": {"thinkingLevel": "MEDIUM"}}})
            )
            .cloud_model(&empty),
            "gemini-4-neon-medium"
        );
    }
}
