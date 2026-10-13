use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// `StoredAccount.extra` key of `awaiting_first_request`.
pub(crate) const FIRST_REQUEST_PENDING: &str = "firstRequestPending";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AccountsStore {
    #[serde(flatten)]
    pub(crate) extra: serde_json::Map<String, Value>,
    #[serde(default = "default_store_version")]
    pub(crate) version: u8,
    #[serde(default)]
    pub(crate) accounts: Vec<StoredAccount>,
    #[serde(default)]
    #[serde(rename = "currentSelection", alias = "current_selection")]
    pub(crate) current_selection: Option<CurrentAccountSelection>,
    #[serde(default)]
    pub(crate) settings: AppSettings,
}

fn default_store_version() -> u8 {
    1
}

impl Default for AccountsStore {
    fn default() -> Self {
        Self {
            extra: Default::default(),
            version: default_store_version(),
            accounts: Vec::new(),
            current_selection: None,
            settings: AppSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredAccount {
    #[serde(flatten)]
    pub(crate) extra: serde_json::Map<String, Value>,
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) email: Option<String>,
    pub(crate) account_id: String,
    pub(crate) plan_type: Option<String>,
    pub(crate) team_alias: Option<String>,
    pub(crate) auth_json: Value,
    pub(crate) added_at: i64,
    pub(crate) updated_at: i64,
    pub(crate) usage: Option<UsageSnapshot>,
    pub(crate) usage_error: Option<String>,
    #[serde(default)]
    pub(crate) principal_id: Option<String>,
    #[serde(default, rename = "proxyURL", alias = "proxyUrl")]
    pub(crate) proxy_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CurrentAccountSelection {
    #[serde(rename = "cardId", alias = "accountId")]
    pub(crate) account_id: String,
    pub(crate) selected_at: i64,
    #[serde(rename = "sourceDeviceID", alias = "sourceDeviceId")]
    pub(crate) source_device_id: String,
    #[serde(default)]
    pub(crate) account_key: Option<String>,
}

/// A token expiry a card can show: after 1970 and by 9999-12-31. Anything
/// else is malformed, e.g. milliseconds taken for seconds.
pub(crate) fn displayed_expiry(at: i64) -> Option<i64> {
    (1..=253_402_300_799).contains(&at).then_some(at)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub display_status: String,
    pub workspace_status: String,
    pub source_tag: Option<String>,
    pub team_name: Option<String>,
    pub available: bool,
    pub proxy_url: String,
    pub id: String,
    pub label: String,
    pub email: Option<String>,
    pub account_id: String,
    pub plan_type: Option<String>,
    pub added_at: i64,
    pub updated_at: i64,
    pub usage: Option<UsageSnapshot>,
    pub usage_error: Option<String>,
    pub is_current: bool,
    /// A newly imported account Copool has not contacted upstream yet.
    #[serde(default)]
    pub awaiting_first_request: bool,
    /// When the access token expires (Unix seconds); a later usage refresh
    /// renews it. None without a readable expiry.
    pub token_expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    #[serde(default)]
    pub remaining_reset_count: Option<i64>,
    pub fetched_at: i64,
    pub plan_type: Option<String>,
    pub five_hour: Option<UsageWindow>,
    pub one_week: Option<UsageWindow>,
    pub credits: Option<CreditSnapshot>,
}

impl UsageSnapshot {
    /// Snapshots stored by earlier versions may hold another window (weekly,
    /// or a free plan's 30 days) in a slot; only a five-hour window, a weekly
    /// window or one without a duration is shown.
    pub(crate) fn with_matching_windows(mut self) -> Self {
        let keep = |w: &UsageWindow, seconds| w.window_seconds == seconds || w.window_seconds <= 0;
        self.five_hour = self.five_hour.filter(|w| keep(w, 5 * 60 * 60));
        self.one_week = self.one_week.filter(|w| keep(w, 7 * 24 * 60 * 60));
        self
    }

    /// Percent left in the five-hour window. With a weekly window but none of
    /// five hours, the account has no five-hour limit, so none of it is used.
    pub fn five_hour_remaining(&self) -> Option<f64> {
        match (&self.five_hour, &self.one_week) {
            (Some(window), _) => Some((100.0 - window.used_percent).clamp(0.0, 100.0)),
            (None, Some(_)) => Some(100.0),
            (None, None) => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub used_percent: f64,
    pub window_seconds: i64,
    pub reset_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditSnapshot {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SwitchAccountResult {
    pub(crate) account_id: String,
    pub(crate) launched_app_path: Option<String>,
    pub(crate) used_fallback_cli: bool,
    pub(crate) opencode_synced: bool,
    pub(crate) opencode_sync_error: Option<String>,
    pub(crate) restarted_editor_apps: Vec<EditorAppId>,
    pub(crate) editor_restart_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CurrentAuthStatus {
    pub(crate) available: bool,
    pub(crate) account_id: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) plan_type: Option<String>,
    pub(crate) auth_mode: Option<String>,
    pub(crate) last_refresh: Option<String>,
    pub(crate) file_modified_at: Option<i64>,
    pub(crate) fingerprint: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ExtractedAuth {
    pub(crate) principal_id: Option<String>,
    pub(crate) team_name: Option<String>,
    pub(crate) account_id: String,
    pub(crate) access_token: String,
    pub(crate) email: Option<String>,
    pub(crate) plan_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthJsonImportInput {
    pub(crate) source: String,
    pub(crate) content: String,
    pub(crate) label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportAccountFailure {
    pub(crate) source: String,
    pub(crate) error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportAccountsResult {
    pub(crate) total_count: usize,
    pub(crate) imported_count: usize,
    pub(crate) updated_count: usize,
    pub(crate) failures: Vec<ImportAccountFailure>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiProxyStatus {
    pub running: bool,
    pub port: Option<u16>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub active_account_id: Option<String>,
    pub active_account_label: Option<String>,
    pub last_error: Option<String>,
    /// Accounts the proxy can route to now, cooling-down ones excluded, as
    /// Swift's ApiProxyStatus.availableAccounts; -1 when they cannot be read.
    #[serde(default)]
    pub available_accounts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RemoteAuthMode {
    KeyContent,
    KeyFile,
    KeyPath,
    Password,
}

impl Default for RemoteAuthMode {
    fn default() -> Self {
        Self::KeyPath
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteServerConfig {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) host: String,
    pub(crate) ssh_port: u16,
    pub(crate) ssh_user: String,
    #[serde(default)]
    pub(crate) auth_mode: RemoteAuthMode,
    #[serde(default)]
    pub(crate) identity_file: Option<String>,
    #[serde(default)]
    pub(crate) private_key: Option<String>,
    #[serde(default)]
    pub(crate) password: Option<String>,
    pub(crate) remote_dir: String,
    pub(crate) listen_port: u16,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteProxyStatus {
    pub(crate) installed: bool,
    pub(crate) service_installed: bool,
    pub(crate) running: bool,
    pub(crate) enabled: bool,
    pub(crate) service_name: String,
    pub(crate) pid: Option<u32>,
    pub(crate) base_url: String,
    pub(crate) api_key: Option<String>,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeployRemoteProxyInput {
    pub(crate) server: RemoteServerConfig,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CloudflaredTunnelMode {
    Quick,
    Named,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CloudflaredStatus {
    pub(crate) installed: bool,
    pub(crate) binary_path: Option<String>,
    pub(crate) running: bool,
    pub(crate) tunnel_mode: Option<CloudflaredTunnelMode>,
    pub(crate) public_url: Option<String>,
    pub(crate) custom_hostname: Option<String>,
    pub(crate) use_http2: bool,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NamedCloudflaredTunnelInput {
    pub(crate) api_token: String,
    pub(crate) account_id: String,
    pub(crate) zone_id: String,
    pub(crate) hostname: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartCloudflaredTunnelInput {
    pub(crate) api_proxy_port: u16,
    pub(crate) use_http2: bool,
    pub(crate) mode: CloudflaredTunnelMode,
    pub(crate) named: Option<NamedCloudflaredTunnelInput>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TrayUsageDisplayMode {
    Used,
    Hidden,
    #[default]
    Remaining,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub(crate) enum EditorAppId {
    Vscode,
    VscodeInsiders,
    Cursor,
    Antigravity,
    Kiro,
    Trae,
    Qoder,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) enum AppLocale {
    #[default]
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en-US")]
    EnUs,
    #[serde(rename = "ja-JP")]
    JaJp,
    #[serde(rename = "ko-KR")]
    KoKr,
    #[serde(rename = "ru-RU")]
    RuRu,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstalledEditorApp {
    pub(crate) id: EditorAppId,
    pub(crate) label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct AppSettings {
    pub(crate) launch_at_startup: bool,
    pub(crate) tray_usage_display_mode: TrayUsageDisplayMode,
    pub(crate) launch_codex_after_switch: bool,
    pub(crate) sync_opencode_openai_auth: bool,
    pub(crate) restart_editors_on_switch: bool,
    pub(crate) restart_editor_targets: Vec<EditorAppId>,
    pub(crate) auto_start_api_proxy: bool,
    pub(crate) remote_servers: Vec<RemoteServerConfig>,
    pub(crate) api_proxy_api_key: Option<String>,
    pub(crate) locale: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            launch_at_startup: false,
            tray_usage_display_mode: TrayUsageDisplayMode::Remaining,
            launch_codex_after_switch: true,
            sync_opencode_openai_auth: false,
            restart_editors_on_switch: false,
            restart_editor_targets: Vec::new(),
            auto_start_api_proxy: false,
            remote_servers: Vec::new(),
            api_proxy_api_key: None,
            locale: "en".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppSettingsPatch {
    pub(crate) launch_at_startup: Option<bool>,
    pub(crate) tray_usage_display_mode: Option<TrayUsageDisplayMode>,
    pub(crate) launch_codex_after_switch: Option<bool>,
    pub(crate) sync_opencode_openai_auth: Option<bool>,
    pub(crate) restart_editors_on_switch: Option<bool>,
    pub(crate) restart_editor_targets: Option<Vec<EditorAppId>>,
    pub(crate) auto_start_api_proxy: Option<bool>,
    pub(crate) remote_servers: Option<Vec<RemoteServerConfig>>,
    pub(crate) locale: Option<AppLocale>,
}

impl StoredAccount {
    /// Newly imported and not yet released by a manual refresh: no request
    /// goes upstream for it until then.
    pub(crate) fn awaiting_first_request(&self) -> bool {
        self.extra
            .get(FIRST_REQUEST_PENDING)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
    pub(crate) fn is_available(&self) -> bool {
        self.extra.get("workspaceStatus").and_then(Value::as_str) != Some("deactivated")
            && self
                .extra
                .get("displayStatus")
                .and_then(Value::as_str)
                .map_or(true, |s| s == "list")
    }

    pub(crate) fn to_summary(&self, current_account_id: Option<&str>) -> AccountSummary {
        AccountSummary {
            display_status: self
                .extra
                .get("displayStatus")
                .and_then(Value::as_str)
                .unwrap_or("list")
                .into(),
            workspace_status: self
                .extra
                .get("workspaceStatus")
                .and_then(Value::as_str)
                .unwrap_or("active")
                .into(),
            source_tag: None,
            team_name: self.team_alias.clone().or_else(|| {
                self.extra
                    .get("teamName")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }),
            available: self.is_available(),
            proxy_url: self.proxy_url.clone(),
            id: self.id.clone(),
            label: self.label.clone(),
            email: self.email.clone(),
            account_id: self.account_id.clone(),
            plan_type: self.plan_type.clone(),
            added_at: self.added_at,
            updated_at: self.updated_at,
            usage: self.usage.clone().map(UsageSnapshot::with_matching_windows),
            usage_error: self.usage_error.clone(),
            awaiting_first_request: self.awaiting_first_request(),
            token_expires_at: crate::auth::access_token_expiry(&self.auth_json)
                .and_then(displayed_expiry),
            is_current: current_account_id
                .map(|id| id == self.account_id)
                .unwrap_or(false),
        }
    }
}

impl AccountsStore {
    pub(crate) fn account_summaries(&self) -> Vec<AccountSummary> {
        let current = self.extra.get("currentAccountId").and_then(Value::as_str);
        self.accounts
            .iter()
            .map(|a| {
                let mut summary = a.to_summary(None);
                summary.is_current = current == Some(a.id.as_str());
                summary
            })
            .collect()
    }
}

impl AccountSummary {
    pub fn normalized_plan_label(&self) -> String {
        let raw = self
            .usage
            .as_ref()
            .and_then(|u| u.plan_type.as_ref())
            .filter(|s| !s.trim().is_empty())
            .or(self.plan_type.as_ref())
            .map(|s| s.trim().to_uppercase())
            .unwrap_or_default();
        if ["PRO", "PLUS", "FREE", "BUSINESS", "ENTERPRISE"].contains(&raw.as_str()) {
            raw
        } else {
            "TEAM".into()
        }
    }
}
