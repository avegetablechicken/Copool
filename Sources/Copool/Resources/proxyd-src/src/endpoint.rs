use serde_json::Value;
use std::{collections::HashMap, sync::Arc};
use tokio::{sync::Mutex, task::JoinSet};

#[derive(Default)]
pub(crate) struct EndpointRequests {
    preferred: Mutex<HashMap<String, String>>,
}
enum Failure {
    Message(String),
    Other(String),
}
impl Failure {
    fn text(self) -> String {
        match self {
            Self::Message(s) | Self::Other(s) => s,
        }
    }
}

pub(crate) struct EndpointResult<T> {
    pub(crate) value: T,
    pub(crate) endpoint: String,
}
impl EndpointRequests {
    pub(crate) async fn fetch<T, F, V>(
        &self,
        scope: &str,
        candidates: &[String],
        make: F,
        validate: V,
    ) -> Result<EndpointResult<T>, String>
    where
        T: Send + 'static,
        F: Fn(&str) -> reqwest::RequestBuilder + Send + Sync + 'static,
        V: Fn(Value) -> Result<T, String> + Send + Sync + 'static,
    {
        let preferred = self.preferred.lock().await.get(scope).cloned();
        let mut urls = candidates.to_vec();
        if let Some(index) = urls.iter().position(|url| Some(url) == preferred.as_ref()) {
            let url = urls.remove(index);
            urls.insert(0, url);
        }
        if urls.is_empty() {
            return Err("No candidate endpoints".into());
        }
        let make = Arc::new(make);
        let validate = Arc::new(validate);
        let mut errors = Vec::new();
        match attempt(urls.remove(0), make.clone(), validate.clone()).await {
            Ok(result) => {
                self.preferred
                    .lock()
                    .await
                    .insert(scope.into(), result.endpoint.clone());
                return Ok(result);
            }
            Err(error) => errors.push(error),
        }
        let mut tasks = JoinSet::new();
        for url in urls {
            let make = make.clone();
            let validate = validate.clone();
            tasks.spawn(attempt(url, make, validate));
        }
        while let Some(outcome) = tasks.join_next().await {
            match outcome {
                Ok(Ok(result)) => {
                    tasks.abort_all();
                    self.preferred
                        .lock()
                        .await
                        .insert(scope.into(), result.endpoint.clone());
                    return Ok(result);
                }
                Ok(Err(error)) => errors.push(error),
                Err(error) => errors.push(Failure::Other(error.to_string())),
            }
        }
        if let Some(index) = errors.iter().position(|e| matches!(e, Failure::Message(_))) {
            return Err(errors.remove(index).text());
        }
        Err(errors
            .into_iter()
            .map(Failure::text)
            .collect::<Vec<_>>()
            .join(" | "))
    }
}
async fn attempt<T, F, V>(
    endpoint: String,
    make: Arc<F>,
    validate: Arc<V>,
) -> Result<EndpointResult<T>, Failure>
where
    T: Send + 'static,
    F: Fn(&str) -> reqwest::RequestBuilder + Send + Sync + 'static,
    V: Fn(Value) -> Result<T, String> + Send + Sync + 'static,
{
    let response = make(&endpoint)
        .send()
        .await
        .map_err(|e| Failure::Other(format!("{endpoint}: {e}")))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        if let Ok(payload) = serde_json::from_str::<Value>(&body) {
            let message = payload
                .pointer("/error/message")
                .or_else(|| payload.pointer("/detail/message"))
                .or_else(|| payload.get("error_description"))
                .or_else(|| payload.get("detail"));
            if let Some(message) = message
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
            {
                return Err(Failure::Message(message.into()));
            }
        }
        return Err(Failure::Other(format!(
            "{endpoint}: {status}: {}",
            crate::utils::truncate_for_error(&body, 140)
        )));
    }
    let payload = response
        .json::<Value>()
        .await
        .map_err(|e| Failure::Other(format!("{endpoint}: {e}")))?;
    let value = validate(payload).map_err(|e| Failure::Other(format!("{endpoint}: {e}")))?;
    Ok(EndpointResult { value, endpoint })
}
