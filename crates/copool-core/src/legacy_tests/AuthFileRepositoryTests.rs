use super::{auth, jwt};
use crate::auth::*;
use serde_json::json;
#[test]
fn testExtractAuthReadsAccountAndClaims() {
    let token = jwt(
        json!({"email":"dev@example.com","https://api.openai.com/auth":{"chatgpt_account_id":"acct_12345","chatgpt_plan_type":"pro","chatgpt_team_name":"Alpha Team"}}),
    );
    let a =
        extract_auth(&json!({"tokens":{"access_token":"access-token","id_token":token}})).unwrap();
    assert_eq!(a.account_id, "acct_12345");
    assert_eq!(a.email.as_deref(), Some("dev@example.com"));
    assert_eq!(a.plan_type.as_deref(), Some("pro"));
    assert_eq!(a.team_name.as_deref(), Some("Alpha Team"));
    assert_eq!(a.access_token, "access-token");
}
#[test]
fn testExtractAuthResolvesPrincipalIDFromJWTSubject() {
    let a = extract_auth(&auth("user_123", "acct_12345")).unwrap();
    assert_eq!(a.principal_id.as_deref(), Some("user_123"));
    assert_eq!(
        account_identity(&auth("user_123", "acct_12345")).unwrap(),
        ("acct_12345".into(), "user_123".into())
    );
}
#[test]
fn testExtractAuthPrefersNonPersonalWorkspaceSlug() {
    let token = jwt(
        json!({"https://api.openai.com/auth":{"chatgpt_account_id":"acct_12345","chatgpt_plan_type":"team","active_organization_id":"org-team","organizations":[{"id":"org-personal","is_default":true,"title":"Personal","slug":"personal"},{"id":"org-team","is_active":true,"title":"Team Workspace","slug":"kqikiy"}]}}),
    );
    let a = extract_auth(&json!({"tokens":{"access_token":"test","id_token":token}})).unwrap();
    assert_eq!(a.team_name.as_deref(), Some("kqikiy"));
}
#[test]
fn testExtractAuthIgnoresWorkspaceContainersInUnrelatedNestedObjects() {
    let mut document = auth("p", "acct_12345");
    document["analytics"] =
        json!({"organizations":[{"id":"org-shadow","is_active":true,"slug":"shadow-workspace"}]});
    assert!(extract_auth(&document).unwrap().team_name.is_none());
}
#[test]
fn testExtractAuthFallsBackToTopLevelEmail() {
    let document = json!({"email":"fallback@example.com","tokens":{"access_token":"test","id_token":jwt(json!({"https://api.openai.com/auth":{"chatgpt_account_id":"acct_12345"}}))}});
    assert_eq!(
        extract_auth(&document).unwrap().email.as_deref(),
        Some("fallback@example.com")
    );
}
#[test]
fn testWriteCurrentAuthRejectsPayloadWithoutIDToken() {
    let error = normalized_auth_for_write(json!({"tokens":{"access_token":"test"}})).unwrap_err();
    assert!(error.contains("id_token"));
}
#[test]
fn testWriteCurrentAuthNormalizesFlatTokenShapeAndTimestamp() {
    let token = jwt(json!({"https://api.openai.com/auth":{"chatgpt_account_id":"acct_98765"}}));
    let written=normalized_auth_for_write(json!({"access_token":"access-token","refresh_token":"refresh-token","id_token":token,"account_id":"acct_98765","last_refresh":"2026-03-19T12:57:06.735503","organization":{"name":"workspace-alpha"},"OPENAI_API_KEY":"sk-proj-test"})).unwrap();
    assert!(written.get("access_token").is_none());
    assert!(written.get("id_token").is_none());
    assert_eq!(written["tokens"]["access_token"], "access-token");
    assert_eq!(written["tokens"]["refresh_token"], "refresh-token");
    assert_eq!(written["tokens"]["id_token"], token);
    assert_eq!(written["tokens"]["account_id"], "acct_98765");
    assert_eq!(written["organization"]["name"], "workspace-alpha");
    assert_eq!(written["OPENAI_API_KEY"], "sk-proj-test");
    chrono::DateTime::parse_from_rfc3339(written["last_refresh"].as_str().unwrap()).unwrap();
}
#[test]
fn testWriteCurrentAuthReplacesInvalidLastRefreshWithCompatibleTimestamp() {
    let mut document = auth("p", "a");
    document["last_refresh"] = "not-a-date".into();
    let written = normalized_auth_for_write(document).unwrap();
    assert_ne!(written["last_refresh"], "not-a-date");
    chrono::DateTime::parse_from_rfc3339(written["last_refresh"].as_str().unwrap()).unwrap();
}
#[test]
fn testMakeChatGPTAuthBuildsCodexCompatibleShape() {
    let original = auth("dev", "acct_67890");
    let mut flat = original["tokens"].clone();
    flat["OPENAI_API_KEY"] = "sk-proj-test".into();
    let written = normalized_auth_for_write(flat).unwrap();
    assert_eq!(written["auth_mode"], "chatgpt");
    assert_eq!(written["tokens"], original["tokens"]);
    assert_eq!(written["OPENAI_API_KEY"], "sk-proj-test");
    assert!(written["last_refresh"].is_string());
}
