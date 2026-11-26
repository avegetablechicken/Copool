use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use crate::models::CurrentAuthStatus;
use crate::pool_login::codex::{auth_token_object, decode_jwt_payload, extract_auth};
use crate::utils::localized;
use crate::utils::truncate_for_error;

pub(crate) fn read_current_codex_auth() -> Result<Value, String> {
    let path = codex_auth_path()?;
    let raw = fs::read_to_string(&path).map_err(|e| read_failed(&path, e))?;
    serde_json::from_str(&raw).map_err(|_| localized("error.auth.auth_json_invalid", &[]))
}

pub(crate) fn read_current_codex_auth_optional() -> Result<Option<Value>, String> {
    let path = codex_auth_path()?;
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&path).map_err(|e| read_failed(&path, e))?;
    let value =
        serde_json::from_str(&raw).map_err(|_| localized("error.auth.auth_json_invalid", &[]))?;
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

    let metadata = fs::metadata(&path).map_err(|e| read_failed(&path, e))?;
    let modified_at = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64);

    let raw = fs::read_to_string(&path).map_err(|e| read_failed(&path, e))?;
    let value: Value =
        serde_json::from_str(&raw).map_err(|_| localized("error.auth.auth_json_invalid", &[]))?;

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
        .map_err(|e| localized("error.auth.write_auth_json_failed_format", &[&e]))?;
    // The login Codex is using holds its only refresh token, so replace it
    // atomically instead of truncating first.
    crate::utils::write_private_file_atomically(&path, serialized.as_bytes()).map_err(|e| {
        localized(
            "error.auth.write_auth_json_failed_format",
            &[&format!("{}: {e}", path.display())],
        )
    })
}

pub(crate) fn remove_active_codex_auth() -> Result<(), String> {
    let path = codex_auth_path()?;
    if !path.exists() {
        return Ok(());
    }
    fs::remove_file(&path).map_err(|e| {
        localized(
            "error.auth.delete_auth_json_failed_format",
            &[&format!("{}: {e}", path.display())],
        )
    })
}

pub(crate) fn current_auth_account_id() -> Option<String> {
    read_current_codex_auth()
        .ok()
        .and_then(|auth_json| extract_auth(&auth_json).ok())
        .map(|auth| auth.account_id)
}

/// Refreshes ChatGPT OAuth tokens with the refresh_token in auth.json.
///
/// Returns the updated auth.json in memory only; it is not written to disk.
pub(crate) async fn refresh_chatgpt_auth_tokens(
    auth_json: &Value,
    proxy_url: &str,
) -> Result<Value, String> {
    let tokens = auth_token_object(auth_json)
        .ok_or_else(|| localized("error.auth.auth_json_invalid_structure", &[]))?;

    let refresh_token = tokens
        .get("refresh_token")
        .and_then(Value::as_str)
        .ok_or_else(|| localized("error.auth.missing_refresh_token", &[]))?;
    let id_token = tokens
        .get("id_token")
        .and_then(Value::as_str)
        .ok_or_else(|| localized("error.auth.missing_id_token", &[]))?;

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
    .map_err(|e| localized("error.auth.refresh_failed_format", &[&e]))?;

    let response = client
        .post(&token_url)
        .form(&form_pairs)
        .send()
        .await
        .map_err(|e| {
            localized(
                "error.auth.refresh_failed_format",
                &[&format!("{token_url}: {e}")],
            )
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(localized(
            "error.auth.refresh_failed_format",
            &[&format!(
                "{token_url} -> {status}: {}",
                truncate_for_error(&body, 140)
            )],
        ));
    }

    let refreshed: RefreshedTokenPayload = response
        .json()
        .await
        .map_err(|e| localized("error.auth.refresh_failed_format", &[&e]))?;

    let mut updated = auth_json.clone();
    let root = updated
        .as_object_mut()
        .ok_or_else(|| localized("error.auth.auth_json_invalid_structure", &[]))?;
    let tokens = root
        .get_mut("tokens")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| localized("error.auth.auth_json_invalid_structure", &[]))?;

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

fn read_failed(path: &std::path::Path, error: std::io::Error) -> String {
    localized(
        "error.auth.read_auth_json_failed_format",
        &[&format!("{}: {error}", path.display())],
    )
}

fn codex_auth_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|p| p.join(".codex")))
        .ok_or_else(|| localized("error.auth.codex_home_not_found", &[]))?;
    Ok(home.join("auth.json"))
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
