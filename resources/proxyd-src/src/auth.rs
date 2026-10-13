use base64::engine::general_purpose::URL_SAFE;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::Map;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use crate::models::CurrentAuthStatus;
use crate::models::ExtractedAuth;
use crate::utils::truncate_for_error;

pub(crate) struct CodexOAuthTokens {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) account_id: Option<String>,
    pub(crate) expires_at_ms: Option<i64>,
}

pub(crate) fn read_current_codex_auth() -> Result<Value, String> {
    let path = codex_auth_path()?;
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("读取当前 Codex 认证文件失败 {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("当前 Codex 认证文件不是合法 JSON: {e}"))
}

pub(crate) fn read_current_codex_auth_optional() -> Result<Option<Value>, String> {
    let path = codex_auth_path()?;
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("读取当前 Codex 认证文件失败 {}: {e}", path.display()))?;
    let value =
        serde_json::from_str(&raw).map_err(|e| format!("当前 Codex 认证文件不是合法 JSON: {e}"))?;
    Ok(Some(value))
}

pub(crate) fn read_current_auth_status() -> Result<CurrentAuthStatus, String> {
    let path = codex_auth_path()?;
    if !path.exists() {
        return Ok(CurrentAuthStatus {
            available: false,
            account_id: None,
            email: None,
            plan_type: None,
            auth_mode: None,
            last_refresh: None,
            file_modified_at: None,
            fingerprint: None,
        });
    }

    let metadata = fs::metadata(&path)
        .map_err(|e| format!("读取 auth.json 文件信息失败 {}: {e}", path.display()))?;
    let modified_at = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64);

    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("读取 auth.json 失败 {}: {e}", path.display()))?;
    let value: Value =
        serde_json::from_str(&raw).map_err(|e| format!("auth.json 不是合法 JSON: {e}"))?;

    let auth_mode = value
        .get("auth_mode")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let last_refresh = value
        .get("last_refresh")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let extracted = extract_auth(&value).ok();
    let account_id = extracted.as_ref().map(|auth| auth.account_id.clone());
    let email = extracted.as_ref().and_then(|auth| auth.email.clone());
    let plan_type = extracted.as_ref().and_then(|auth| auth.plan_type.clone());

    let fingerprint = Some(format!(
        "{}|{}|{}|{}",
        account_id.clone().unwrap_or_default(),
        last_refresh.clone().unwrap_or_default(),
        modified_at.unwrap_or_default(),
        auth_mode.clone().unwrap_or_default()
    ));

    Ok(CurrentAuthStatus {
        available: true,
        account_id,
        email,
        plan_type,
        auth_mode,
        last_refresh,
        file_modified_at: modified_at,
        fingerprint,
    })
}

pub(crate) fn write_active_codex_auth(auth_json: &Value) -> Result<(), String> {
    let path = codex_auth_path()?;
    let serialized = serde_json::to_string_pretty(auth_json)
        .map_err(|e| format!("序列化 auth.json 失败: {e}"))?;
    // The login Codex is using holds its only refresh token, so replace it
    // atomically instead of truncating first.
    crate::utils::write_private_file_atomically(&path, serialized.as_bytes())
        .map_err(|e| format!("写入 auth.json 失败 {}: {e}", path.display()))
}

pub(crate) fn remove_active_codex_auth() -> Result<(), String> {
    let path = codex_auth_path()?;
    if !path.exists() {
        return Ok(());
    }
    fs::remove_file(&path).map_err(|e| format!("删除 auth.json 失败 {}: {e}", path.display()))
}

pub(crate) fn normalize_imported_auth_json(mut auth_json: Value) -> Value {
    let Some(root) = auth_json.as_object_mut() else {
        return auth_json;
    };
    if root.get("tokens").and_then(Value::as_object).is_some() {
        return auth_json;
    }
    if root.get("access_token").and_then(Value::as_str).is_none() {
        return auth_json;
    }
    let mut tokens = Map::new();
    for key in ["access_token", "refresh_token", "id_token", "account_id"] {
        if let Some(value) = root.remove(key) {
            tokens.insert(key.into(), value);
        }
    }
    root.insert("tokens".into(), tokens.into());
    root.entry("auth_mode").or_insert("chatgpt".into());
    auth_json
}

pub(crate) fn normalized_auth_for_write(document: Value) -> Result<Value, String> {
    let mut document = normalize_imported_auth_json(document);
    let extracted = extract_auth(&document)?;
    let root = document.as_object_mut().ok_or("Invalid auth document")?;
    root.entry("auth_mode").or_insert("chatgpt".into());
    if let Some(tokens) = root.get_mut("tokens").and_then(Value::as_object_mut) {
        tokens.insert("account_id".into(), extracted.account_id.into());
    }
    let timestamp = root
        .get("last_refresh")
        .and_then(Value::as_str)
        .and_then(|value| {
            chrono::DateTime::parse_from_rfc3339(value)
                .ok()
                .or_else(|| chrono::DateTime::parse_from_rfc3339(&format!("{value}Z")).ok())
        })
        .map(|date| date.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    root.insert("last_refresh".into(), timestamp.into());
    Ok(document)
}

fn workspace_name(claims: &Value) -> Option<String> {
    let auth = claims.get("https://api.openai.com/auth")?;
    let clean = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    if let Some(name) = clean(auth.get("chatgpt_team_name")) {
        return Some(name);
    }
    let organizations = auth.get("organizations").and_then(Value::as_array)?;
    let active = auth.get("active_organization_id").and_then(Value::as_str);
    let organization = organizations
        .iter()
        .find(|o| active.is_some() && o.get("id").and_then(Value::as_str) == active)
        .or_else(|| {
            organizations
                .iter()
                .find(|o| o.get("is_active").and_then(Value::as_bool) == Some(true))
        })?;
    clean(organization.get("slug"))
        .filter(|s| s.to_lowercase() != "personal")
        .or_else(|| clean(organization.get("title")).filter(|s| s.to_lowercase() != "personal"))
}

/// 解析当前 auth.json，提取账号标识和用量接口所需 token。
///
/// 注意：`auth_mode` 在某些版本可能缺失，因此优先按 `tokens` 字段判断是否可用。
pub(crate) fn extract_auth(auth_json: &Value) -> Result<ExtractedAuth, String> {
    let mode = auth_json
        .get("auth_mode")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();

    let tokens = auth_token_object(auth_json);
    let tokens = match tokens {
        Some(value) => value,
        None => {
            if !mode.is_empty() && mode != "chatgpt" && mode != "chatgpt_auth_tokens" {
                return Err(
                    "当前账号不是 ChatGPT 登录模式，无法读取 Codex 5h/1week 用量。请先执行 codex login。"
                        .to_string(),
                );
            }
            return Err("当前未检测到 ChatGPT 登录令牌，请先执行 codex login。".to_string());
        }
    };

    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| "auth.json 缺少 access_token".to_string())?
        .to_string();

    let id_token = tokens
        .get("id_token")
        .and_then(Value::as_str)
        .ok_or_else(|| "auth.json 缺少 id_token".to_string())?;

    let mut account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let mut email = None;
    let mut plan_type = None;
    let mut principal_id = None;
    let mut team_name = None;

    if let Ok(claims) = decode_jwt_payload(id_token) {
        principal_id = claims.get("sub").and_then(Value::as_str).map(str::to_owned);
        team_name = workspace_name(&claims);
        email = claims
            .get("email")
            .and_then(Value::as_str)
            .map(ToString::to_string);

        let auth_claim = claims.get("https://api.openai.com/auth");
        if account_id.is_none() {
            account_id = auth_claim
                .and_then(|value| value.get("chatgpt_account_id"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
        }
        plan_type = auth_claim
            .and_then(|value| value.get("chatgpt_plan_type"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }

    let account_id =
        account_id.ok_or_else(|| "无法从 auth.json 识别 chatgpt_account_id".to_string())?;

    if email.is_none() {
        email = auth_json
            .get("email")
            .and_then(Value::as_str)
            .map(str::to_owned);
    }
    Ok(ExtractedAuth {
        principal_id,
        team_name,
        account_id,
        access_token,
        email,
        plan_type,
    })
}

/// Stable identity: a workspace can contain multiple independently logged-in users.
pub(crate) fn account_identity(document: &Value) -> Result<(String, String), String> {
    let extracted = extract_auth(document)?;
    let principal = auth_token_object(document)
        .and_then(|v| v.get("id_token"))
        .and_then(Value::as_str)
        .and_then(|t| decode_jwt_payload(t).ok())
        .and_then(|v| v.get("sub").and_then(Value::as_str).map(str::to_owned))
        .or(extracted.email)
        .unwrap_or_default();
    Ok((extracted.account_id, principal))
}

/// Whether `live` is a later login of the same account than `stored`: other
/// tokens, refreshed later. A login without a refresh time is older.
pub(crate) fn newer_login(live: &Value, stored: &Value) -> bool {
    let refreshed = |document: &Value| {
        document["last_refresh"]
            .as_str()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t.trim()).ok())
    };
    account_identity(live).is_ok()
        && account_identity(live).ok() == account_identity(stored).ok()
        && live["tokens"] != stored["tokens"]
        && match (refreshed(live), refreshed(stored)) {
            (Some(live), Some(stored)) => live > stored,
            (live, stored) => live.is_some() || stored.is_none(),
        }
}

/// Whether a login renewed from `from` may replace `current`, wherever it is
/// kept: `current` still holds that generation, or an older one. A renewal
/// answered late must not replace a login renewed since.
pub(crate) fn renewal_replaces(renewed: &Value, from: &Value, current: &Value) -> bool {
    current["tokens"] == from["tokens"] || newer_login(renewed, current)
}

pub(crate) fn current_auth_account_id() -> Option<String> {
    read_current_codex_auth()
        .ok()
        .and_then(|auth_json| extract_auth(&auth_json).ok())
        .map(|auth| auth.account_id)
}

/// 为第三方客户端同步登录态时，提取可复用的 OpenAI OAuth token。
pub(crate) fn extract_codex_oauth_tokens(auth_json: &Value) -> Result<CodexOAuthTokens, String> {
    let tokens = auth_token_object(auth_json).ok_or_else(|| "auth.json 缺少 tokens".to_string())?;

    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| "auth.json 缺少 access_token".to_string())?
        .to_string();
    let refresh_token = tokens
        .get("refresh_token")
        .and_then(Value::as_str)
        .ok_or_else(|| "auth.json 缺少 refresh_token".to_string())?
        .to_string();
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let expires_at_ms = tokens
        .get("id_token")
        .and_then(Value::as_str)
        .and_then(|id_token| decode_jwt_payload(id_token).ok())
        .and_then(|payload| payload.get("exp").and_then(Value::as_i64))
        .map(|value| value * 1000);

    Ok(CodexOAuthTokens {
        access_token,
        refresh_token,
        account_id,
        expires_at_ms,
    })
}

/// 使用 auth.json 内的 refresh_token 刷新 ChatGPT OAuth 令牌。
///
/// 返回更新后的 auth.json（仅内存对象，不会自动写盘）。
pub(crate) async fn refresh_chatgpt_auth_tokens(
    auth_json: &Value,
    proxy_url: &str,
) -> Result<Value, String> {
    let tokens = auth_token_object(auth_json).ok_or_else(|| "auth.json 缺少 tokens".to_string())?;

    let refresh_token = tokens
        .get("refresh_token")
        .and_then(Value::as_str)
        .ok_or_else(|| "auth.json 缺少 refresh_token".to_string())?;
    let id_token = tokens
        .get("id_token")
        .and_then(Value::as_str)
        .ok_or_else(|| "auth.json 缺少 id_token".to_string())?;

    let claims = decode_jwt_payload(id_token)?;
    let issuer = claims
        .get("iss")
        .and_then(Value::as_str)
        .unwrap_or("https://auth.openai.com")
        .trim_end_matches('/')
        .to_string();
    if issuer != "https://auth.openai.com" {
        return Err("Unsupported OAuth issuer".into());
    }
    let token_url = format!("{issuer}/oauth/token");

    let mut form_pairs: Vec<(&str, String)> = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.to_string()),
    ];
    if let Some(client_id) = extract_client_id_from_claims(&claims) {
        form_pairs.push(("client_id", client_id));
    }

    let client = crate::utils::with_account_proxy(
        reqwest::Client::builder()
            .user_agent("codex-tools/0.1")
            .timeout(std::time::Duration::from_secs(30)),
        proxy_url,
    )?
    .build()
    .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;

    let response = client
        .post(&token_url)
        .form(&form_pairs)
        .send()
        .await
        .map_err(|e| format!("刷新登录令牌失败 {token_url}: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "刷新登录令牌失败 {token_url} -> {status}: {}",
            truncate_for_error(&body, 140)
        ));
    }

    let refreshed: RefreshedTokenPayload = response
        .json()
        .await
        .map_err(|e| format!("解析刷新令牌响应失败: {e}"))?;

    let mut updated = auth_json.clone();
    let root = updated
        .as_object_mut()
        .ok_or_else(|| "auth.json 结构异常（根节点不是对象）".to_string())?;
    let tokens = root
        .get_mut("tokens")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "auth.json 缺少 tokens".to_string())?;

    tokens.insert(
        "access_token".to_string(),
        Value::String(refreshed.access_token),
    );
    tokens.insert("id_token".to_string(), Value::String(refreshed.id_token));
    if let Some(refresh_token) = refreshed.refresh_token {
        tokens.insert("refresh_token".to_string(), Value::String(refresh_token));
    }
    // As Codex does, so the rotated login reads as newer than its previous copy.
    root.insert(
        "last_refresh".to_string(),
        Value::String(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
    );

    Ok(updated)
}

fn codex_auth_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|p| p.join(".codex")))
        .ok_or_else(|| "无法读取 Codex 目录".to_string())?;
    Ok(home.join("auth.json"))
}

fn auth_token_object(auth_json: &Value) -> Option<&Map<String, Value>> {
    auth_json
        .get("tokens")
        .and_then(Value::as_object)
        .or_else(|| {
            let root = auth_json.as_object()?;
            if root.contains_key("access_token") && root.contains_key("id_token") {
                Some(root)
            } else {
                None
            }
        })
}

/// When the stored access token expires (Unix seconds), from its JWT claims;
/// a JWT NumericDate may be fractional.
pub(crate) fn access_token_expiry(auth_json: &Value) -> Option<i64> {
    auth_json
        .pointer("/tokens/access_token")
        .or_else(|| auth_json.get("access_token"))
        .and_then(Value::as_str)
        .and_then(|t| decode_jwt_payload(t).ok())
        .and_then(|v| v["exp"].as_f64())
        .map(|exp| exp as i64)
}

pub(crate) fn decode_jwt_payload(token: &str) -> Result<Value, String> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| "id_token 格式无效".to_string())?;

    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| {
            let remainder = payload.len() % 4;
            let padded = if remainder == 0 {
                payload.to_string()
            } else {
                format!("{payload}{}", "=".repeat(4 - remainder))
            };
            URL_SAFE.decode(padded)
        })
        .map_err(|e| format!("解码 id_token 失败: {e}"))?;

    serde_json::from_slice(&decoded).map_err(|e| format!("解析 id_token payload 失败: {e}"))
}

#[derive(Debug, serde::Deserialize)]
struct RefreshedTokenPayload {
    access_token: String,
    id_token: String,
    refresh_token: Option<String>,
}

fn extract_client_id_from_claims(claims: &Value) -> Option<String> {
    let aud = claims.get("aud")?;
    match aud {
        Value::String(value) => {
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        }
        Value::Array(items) => items.iter().find_map(|item| {
            item.as_str().and_then(|value| {
                if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                }
            })
        }),
        _ => None,
    }
}
