//! Ports of the original XCTest assertions. Keep suite/case names traceable.
#![allow(non_snake_case)]
mod AccountRankingTests;
mod AuthFileRepositoryTests;
mod UsageWindowSelectorTests;
use crate::models::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
fn jwt(claims: Value) -> String {
    format!(
        "e30.{}.test",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
    )
}
fn auth(principal: &str, workspace: &str) -> Value {
    json!({"auth_mode":"chatgpt","tokens":{"access_token":"test-only","refresh_token":"refresh-test","id_token":jwt(json!({"sub":principal,"email":format!("{principal}@example.test"),"https://api.openai.com/auth":{"chatgpt_account_id":workspace,"chatgpt_plan_type":"pro"}})),"account_id":workspace}})
}
fn account(id: &str, week: f64, hour: f64, current: bool) -> AccountSummary {
    let a:StoredAccount=serde_json::from_value(json!({"id":id,"label":id,"accountId":id,"authJson":auth(id,id),"addedAt":0,"updatedAt":0,"usage":{"fetchedAt":0,"fiveHour":{"usedPercent":hour,"windowSeconds":18000},"oneWeek":{"usedPercent":week,"windowSeconds":604800}}})).unwrap();
    let mut result = a.to_summary(None);
    result.is_current = current;
    result
}

mod AccountsSnapshotFreshnessPolicyTests;

mod ResetCreditsTests;
mod http_fixture;

mod EndpointRequestCoordinatorTests;

mod AccountsIdentityRegressionTests;
fn fixture() -> (tempfile::TempDir, crate::Application) {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::Application::open(crate::Paths {
        data_dir: dir.path().join("data"),
        codex_auth: dir.path().join("codex/auth.json"),
    })
    .unwrap();
    (dir, app)
}
fn stored(id: &str, principal: &str, workspace: &str) -> serde_json::Value {
    json!({"id":id,"label":id,"email":format!("{principal}@example.test"),"accountId":workspace,"authJson":auth(principal,workspace),"addedAt":1,"updatedAt":1,"principalId":principal})
}
fn save_fixture(app: &crate::Application, value: serde_json::Value) {
    crate::persistence::write_json(&app.paths.data_dir.join("accounts.json"), &value).unwrap();
}

mod RemoteServerConfigurationTests;
mod RemoteShellCommandRunnerTests;

mod AccountsCoordinatorTests;

mod AccountsWidgetSnapshotBuilderTests;
mod AccountsWidgetSnapshotStoreTests;

mod UsageServiceTests;

mod CommandRunnerTimeoutTests;
mod RemoteProxydBinaryBuilderTests;
mod RepositoryLocatorTests;

mod AppSettingsCodableTests;

mod StoreFileRepositoryTests;

mod ProviderProxySessionTests;
mod RemoteProxyAccountsPayloadBuilderTests;

mod WorkspaceMetadataServiceTests;

#[derive(Default)]
struct MemorySecrets {
    values: std::sync::Mutex<std::collections::HashMap<String, String>>,
    gets: std::sync::Mutex<Vec<String>>,
    sets: std::sync::Mutex<Vec<String>>,
    removed: std::sync::Mutex<Vec<String>>,
    deny_reads: std::sync::atomic::AtomicBool,
}
impl crate::credentials::SecretStore for MemorySecrets {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        self.gets.lock().unwrap().push(id.into());
        if self.deny_reads.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("Unexpected credential read".into());
        }
        Ok(self.values.lock().unwrap().get(id).cloned())
    }
    fn set(&self, id: &str, password: &str) -> Result<(), String> {
        self.sets.lock().unwrap().push(id.into());
        self.values
            .lock()
            .unwrap()
            .insert(id.into(), password.into());
        Ok(())
    }
    fn remove(&self, id: &str) -> Result<(), String> {
        self.removed.lock().unwrap().push(id.into());
        self.values.lock().unwrap().remove(id);
        Ok(())
    }
}
mod AppSmokeTests;
mod SettingsPageModelTests;
mod SimpleHTTPServerTests;
mod SwiftNativeProxyRuntimeServiceTests;
pub(crate) mod network_fixture;

#[path = "../usage_regression_tests.rs"]
mod usage_regressions;
