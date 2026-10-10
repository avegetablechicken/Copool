//! Browser OAuth with PKCE, state validation, bounded callback listener and cancellation.
use axum::{extract::Query, http::StatusCode, response::Html, routing::get, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{oneshot, watch, Mutex};
const CLIENT: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
fn nonce() -> String {
    URL_SAFE_NO_PAD.encode(
        [
            uuid::Uuid::new_v4().as_bytes().as_slice(),
            uuid::Uuid::new_v4().as_bytes().as_slice(),
        ]
        .concat(),
    )
}
pub struct Login {
    pub url: String,
    pub id: String,
    receiver: Mutex<Option<oneshot::Receiver<Result<String, String>>>>,
    cancel: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    verifier: String,
    redirect: String,
    workspace: Option<String>,
}
impl Drop for Login {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Login {
    pub fn workspace(&self) -> Option<&str> {
        self.workspace.as_deref()
    }
    pub async fn begin(workspace: Option<String>) -> Result<Arc<Self>, String> {
        let mut listener = None;
        for port in 1455..=1467 {
            if let Ok(bound) =
                tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await
            {
                listener = Some(bound);
                break;
            }
        }
        let listener = listener.ok_or("OAuth callback ports 1455–1467 are occupied")?;
        let redirect = format!(
            "http://localhost:{}/auth/callback",
            listener.local_addr().map_err(|e| e.to_string())?.port()
        );
        let verifier = nonce();
        let state = nonce();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut url = reqwest::Url::parse("https://auth.openai.com/oauth/authorize").unwrap();
        url.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", CLIENT),
            ("redirect_uri", &redirect),
            (
                "scope",
                "openid profile email offline_access api.connectors.read api.connectors.invoke",
            ),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("state", &state),
            ("originator", "codex_cli_rs"),
        ]);
        if let Some(id) = &workspace {
            url.query_pairs_mut()
                .append_pair("allowed_workspace_id", id);
        }
        let (sender, receiver) = oneshot::channel();
        let sender = Arc::new(Mutex::new(Some(sender)));
        let router = Router::new().route(
            "/auth/callback",
            get(move |Query(params): Query<HashMap<String, String>>| {
                let sender = sender.clone();
                let state = state.clone();
                async move {
                    if params.get("state") != Some(&state) {
                        return (
                            StatusCode::BAD_REQUEST,
                            Html("Invalid OAuth state. Return to Copool and retry."),
                        );
                    }
                    let result = if let Some(code) = params.get("code").filter(|c| !c.is_empty()) {
                        Ok(code.clone())
                    } else {
                        Err(params
                            .get("error_description")
                            .or_else(|| params.get("error"))
                            .cloned()
                            .unwrap_or("Missing authorization code".into()))
                    };
                    let ok = result.is_ok();
                    if let Some(tx) = sender.lock().await.take() {
                        let _ = tx.send(result);
                    }
                    (
                        if ok {
                            StatusCode::OK
                        } else {
                            StatusCode::UNAUTHORIZED
                        },
                        Html(if ok {
                            "Authorization received. Return to Copool to finish signing in."
                        } else {
                            "Authorization failed. Return to Copool to retry."
                        }),
                    )
                }
            }),
        );
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let (cancel, _) = watch::channel(false);
        Ok(Arc::new(Self {
            url: url.into(),
            id: uuid::Uuid::new_v4().to_string(),
            receiver: Mutex::new(Some(receiver)),
            cancel,
            task,
            verifier,
            redirect,
            workspace,
        }))
    }
    pub async fn cancelled(&self) {
        let mut receiver = self.cancel.subscribe();
        while !*receiver.borrow_and_update() {
            if receiver.changed().await.is_err() {
                break;
            }
        }
    }
    pub fn cancel(&self) {
        self.cancel.send_replace(true);
        self.task.abort();
    }
    pub async fn finish(&self) -> Result<Value, String> {
        let mut cancellation = self.cancel.subscribe();
        if *cancellation.borrow() {
            return Err("error.oauth.request_cancelled".into());
        }
        let receiver = self
            .receiver
            .lock()
            .await
            .take()
            .ok_or("Login already awaited")?;
        let operation = async {
            let code = receiver
                .await
                .map_err(|_| "error.oauth.request_cancelled")??;
            let response = reqwest::Client::new()
                .post("https://auth.openai.com/oauth/token")
                .timeout(Duration::from_secs(30))
                .form(&[
                    ("grant_type", "authorization_code"),
                    ("code", &code),
                    ("redirect_uri", &self.redirect),
                    ("client_id", CLIENT),
                    ("code_verifier", &self.verifier),
                ])
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !response.status().is_success() {
                return Err(format!(
                    "OAuth token exchange failed: {}",
                    response.status()
                ));
            }
            let tokens: Value = response.json().await.map_err(|e| e.to_string())?;
            let auth = crate::auth::normalized_auth_for_write(json!({"tokens":tokens}))?;
            if let Some(expected) = &self.workspace {
                if crate::auth::extract_auth(&auth)?.account_id != *expected {
                    return Err("error.oauth.workspace_mismatch".into());
                }
            }
            Ok(auth)
        };
        let result = tokio::select! {v=tokio::time::timeout(Duration::from_secs(600),operation)=>v.map_err(|_|"error.accounts.add_account_timeout".to_string())?,_=cancellation.changed()=>Err("error.oauth.request_cancelled".into())};
        self.task.abort();
        result
    }
}
