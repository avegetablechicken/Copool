use crate::{
    account_network::{AccountNetwork, Response},
    models::UsageSnapshot,
    workspaces::Metadata,
};
use serde_json::{json, Value};
use std::sync::Mutex;
#[derive(Default)]
pub struct Network {
    pub calls: Mutex<Vec<(String, String, String)>>,
    pub usage_error: Mutex<Option<String>>,
    pub refresh_error: Mutex<Option<String>>,
    pub metadata: Mutex<Vec<Metadata>>,
    pub metadata_by_token: Mutex<std::collections::HashMap<String, Vec<Metadata>>>,
    pub metadata_error: Mutex<Option<String>>,
    pub started: tokio::sync::Notify,
    pub resume: tokio::sync::Notify,
    pub block: std::sync::atomic::AtomicBool,
}
impl AccountNetwork for Network {
    fn usage<'a>(&'a self, auth: &'a Value, proxy: &'a str) -> Response<'a, UsageSnapshot> {
        Box::pin(async move {
            let id = crate::auth::extract_auth(auth)?.account_id;
            self.calls
                .lock()
                .unwrap()
                .push(("usage".into(), id, proxy.into()));
            if self.block.swap(false, std::sync::atomic::Ordering::SeqCst) {
                self.started.notify_one();
                self.resume.notified().await;
            }
            if let Some(error) = self.usage_error.lock().unwrap().clone() {
                return Err(error);
            }
            Ok(serde_json::from_value(json!({"fetchedAt":crate::utils::now_unix_seconds(),"fiveHour":{"usedPercent":10,"windowSeconds":18000},"oneWeek":{"usedPercent":20,"windowSeconds":604800}})).unwrap())
        })
    }
    fn refresh<'a>(&'a self, auth: &'a Value, proxy: &'a str) -> Response<'a, Value> {
        Box::pin(async move {
            self.calls.lock().unwrap().push((
                "refresh".into(),
                crate::auth::extract_auth(auth)?.account_id,
                proxy.into(),
            ));
            if let Some(error) = self.refresh_error.lock().unwrap().clone() {
                return Err(error);
            }
            let mut updated = auth.clone();
            updated["tokens"]["access_token"] = "refreshed-test-only".into();
            Ok(updated)
        })
    }
    fn workspaces<'a>(&'a self, _token: &'a str, proxy: &'a str) -> Response<'a, Vec<Metadata>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push(("workspaces".into(), _token.into(), proxy.into()));
            if let Some(error) = self.metadata_error.lock().unwrap().clone() {
                return Err(error);
            }
            Ok(self
                .metadata_by_token
                .lock()
                .unwrap()
                .get(_token)
                .cloned()
                .unwrap_or_else(|| self.metadata.lock().unwrap().clone()))
        })
    }
}
pub fn fixture() -> (
    tempfile::TempDir,
    crate::Application,
    std::sync::Arc<Network>,
) {
    let (dir, mut app) = super::fixture();
    let network = std::sync::Arc::new(Network::default());
    app.network = network.clone();
    (dir, app, network)
}
