use super::*;
use crate::{platform::after_switch, SwitchExecution};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

fn fake_jwt(payload: Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
    let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    format!("{header}.{body}.sig")
}

#[test]
fn sync_opencode_writes_openai_oauth_and_replaces_api_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    write_json(
            &path,
            &json!({
                "openai": { "type": "api", "key": "sk-old" },
                "xai": { "type": "oauth", "access": "xai-access", "refresh": "xai-refresh", "expires": 1_700_000_000_000_i64 }
            }),
        )
        .unwrap();

    let credential = OAuthCredential {
        access: "openai-access".into(),
        refresh: "openai-refresh".into(),
        expires_ms: 1_800_000_456_000,
        account_id: Some("acct-42".into()),
        org_id: Some("acct-42".into()),
        ..OAuthCredential::default()
    };
    sync_opencode(&credential, std::slice::from_ref(&path)).unwrap();

    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["openai"]["type"], "oauth");
    assert_eq!(saved["openai"]["access"], "openai-access");
    assert_eq!(saved["openai"]["refresh"], "openai-refresh");
    assert_eq!(saved["openai"]["expires"], 1_800_000_456_000_i64);
    assert_eq!(saved["openai"]["accountId"], "acct-42");
    assert!(saved["openai"].get("key").is_none());
    assert_eq!(saved["xai"]["access"], "xai-access");
    // A later login without an account id leaves no earlier one's.
    let unnamed = OAuthCredential {
        account_id: None,
        org_id: None,
        ..credential
    };
    sync_opencode(&unnamed, std::slice::from_ref(&path)).unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["openai"]["access"], "openai-access");
    assert!(saved["openai"].get("accountId").is_none());
}

#[test]
fn sync_opencode_grok_writes_xai_oauth_and_keeps_other_providers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    write_json(
        &path,
        &json!({
            "openai": {
                "type": "oauth",
                "access": "openai-access",
                "refresh": "openai-refresh",
                "expires": 1_700_000_000_000_i64,
                "accountId": "acct-1"
            },
            "xai": {
                "type": "api",
                "key": "xai-old-api-key"
            }
        }),
    )
    .unwrap();

    let credential = OAuthCredential {
        access: "grok-access-1".into(),
        refresh: "grok-refresh-1".into(),
        expires_ms: grok_expires_ms(),
        ..OAuthCredential::default()
    };
    sync_opencode_grok(&credential, std::slice::from_ref(&path)).unwrap();

    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["openai"]["access"], "openai-access");
    assert_eq!(saved["openai"]["accountId"], "acct-1");
    assert_eq!(saved["xai"]["type"], "oauth");
    assert_eq!(saved["xai"]["access"], "grok-access-1");
    assert_eq!(saved["xai"]["refresh"], "grok-refresh-1");
    assert_eq!(
        saved["xai"]["expires"],
        chrono::DateTime::parse_from_rfc3339("2026-11-26T12:00:00Z")
            .unwrap()
            .timestamp()
            * 1000
    );
    assert!(saved["xai"].get("key").is_none());
    assert!(saved["xai"].get("accountId").is_none());
}

/// Each of `agents` with its sync on, every pool chosen; the others off.
fn syncing(agents: &[SyncAgent]) -> std::collections::BTreeMap<SyncAgent, crate::AgentSync> {
    SyncAgent::ALL
        .into_iter()
        .map(|agent| {
            let sync = crate::AgentSync {
                enabled: agents.contains(&agent),
                ..Default::default()
            };
            (agent, sync)
        })
        .collect()
}

/// Each agent's own file in `dir`, as `locate` would find it at home.
fn located(dir: &Path, agent: SyncAgent) -> Vec<Target> {
    match agent {
        SyncAgent::Opencode => vec![Target::Json(dir.join("opencode/auth.json"))],
        SyncAgent::Pi => vec![Target::Json(dir.join("pi/auth.json"))],
        SyncAgent::Omp => vec![Target::Sqlite(dir.join("omp/agent.db"))],
    }
}

/// A switch writes an agent exactly when its sync is on, it takes the
/// pool, and the pool is chosen for it; the writes land in the files
/// `locate` names, never at home.
#[tokio::test]
async fn a_switch_syncs_each_agent_that_takes_its_pool_where_it_is_on_and_chosen() {
    let takes = |agent, pool| {
        matches!(
            (agent, pool),
            (SyncAgent::Opencode, Pool::Codex | Pool::Grok)
                | (SyncAgent::Pi, Pool::Codex | Pool::Grok | Pool::Claude)
                | (SyncAgent::Omp, _)
        )
    };
    let credential = |pool| match pool {
        Pool::Codex => codex_credential(
            "alice@example.com",
            "acct-1",
            "plus",
            1_780_000_000,
            "refresh-1",
        ),
        Pool::Claude => claude_credential("alice", "org-1", "rt-1"),
        Pool::Grok => grok_credential("alice", "grok-refresh"),
        Pool::Antigravity => OAuthCredential {
            access: "a".into(),
            refresh: "r".into(),
            expires_ms: 1_780_000_000_000,
            email: Some("alice@gmail.com".into()),
            ..OAuthCredential::default()
        },
    };
    for pool in Pool::ALL {
        for agent in SyncAgent::ALL {
            for (on, chosen) in [(true, true), (true, false), (false, true)] {
                let dir = tempfile::tempdir().unwrap();
                let on_agents: &[SyncAgent] = if on { &[agent] } else { &[] };
                let mut p = Preferences {
                    agent_sync: syncing(on_agents),
                    ..Default::default()
                };
                let pools = &mut p.agent_sync.get_mut(&agent).unwrap().pools;
                match pool {
                    Pool::Codex => pools.codex = chosen,
                    Pool::Claude => pools.claude = chosen,
                    Pool::Antigravity => pools.antigravity = chosen,
                    Pool::Grok => pools.grok = chosen,
                }
                let root = dir.path().to_path_buf();
                let outcomes = sync_to_agents(pool, &p, Ok(credential(pool)), |agent| {
                    let targets = located(&root, agent);
                    async move { targets }
                })
                .await;
                let expected = on && chosen && takes(agent, pool);
                let case = format!("{agent:?} in {pool:?}, on {on}, chosen {chosen}");
                let written = if expected {
                    vec![AgentSyncOutcome { agent, error: None }]
                } else {
                    vec![]
                };
                assert_eq!(outcomes, written, "{case}");
                for other in SyncAgent::ALL {
                    let path = located(dir.path(), other)[0].path().to_path_buf();
                    assert_eq!(
                        path.exists(),
                        expected && other == agent,
                        "{case}: {other:?}"
                    );
                }
            }
        }
    }
}

/// Every agent on reports in the table's order, each written or why
/// not, and a login granting no credential is each one's error.
#[tokio::test]
async fn a_switch_reports_each_agent_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let p = Preferences {
        agent_sync: syncing(&SyncAgent::ALL),
        ..Default::default()
    };
    // OpenCode's file is a directory: its write fails, the others' not.
    std::fs::create_dir_all(dir.path().join("opencode/auth.json")).unwrap();
    let root = dir.path().to_path_buf();
    let locate = |agent| {
        let targets = located(&root, agent);
        async move { targets }
    };
    let codex = codex_credential(
        "alice@example.com",
        "acct-1",
        "plus",
        1_780_000_000,
        "refresh-1",
    );
    let outcomes = sync_to_agents(Pool::Codex, &p, Ok(codex), locate).await;
    assert_eq!(
        outcomes.iter().map(|o| o.agent).collect::<Vec<_>>(),
        SyncAgent::ALL
    );
    assert!(outcomes[0].error.is_some());
    assert_eq!(outcomes[1].error, None);
    assert_eq!(outcomes[2].error, None);
    let none = "Missing refresh token".to_string();
    let outcomes = sync_to_agents(Pool::Codex, &p, Err(none.clone()), locate).await;
    assert_eq!(
        outcomes,
        SyncAgent::ALL
            .map(|agent| AgentSyncOutcome {
                agent,
                error: Some(none.clone()),
            })
            .to_vec()
    );
    // No agent's file is found: each says where it looked for none.
    let outcomes = sync_to_agents(
        Pool::Codex,
        &p,
        Ok(codex_credential(
            "alice@example.com",
            "acct-1",
            "plus",
            1_780_000_000,
            "refresh-1",
        )),
        |_| async { vec![] },
    )
    .await;
    assert_eq!(
        outcomes[0].error.as_deref(),
        Some("Unable to locate OpenCode auth file path")
    );
}

#[test]
fn abbreviated_paths_start_from_home() {
    let home = Path::new("/Users/someone");
    assert_eq!(
        abbreviated(&home.join(".pi/agent/auth.json"), Some(home)),
        Path::new("~").join(".pi/agent/auth.json").to_string_lossy()
    );
    assert_eq!(abbreviated(Path::new("/tmp/a.db"), Some(home)), "/tmp/a.db");
}

#[tokio::test]
async fn after_switch_outside_codex_skips_editors_codex_launch_and_unsupported_agents() {
    // Agents stay off for Grok: enabled, they would write the real home.
    let grok_preferences = Preferences {
        agent_sync: syncing(&[]),
        restart_editors_on_switch: true,
        restart_editor_targets: vec!["not-installed-editor".into()],
        launch_codex_after_switch: true,
        ..Default::default()
    };
    let execution = after_switch(
        Pool::Grok,
        &grok_preferences,
        Some(Ok(grok_credential("alice", "grok-refresh"))),
    )
    .await
    .unwrap();
    assert_eq!(execution, SwitchExecution::default());
    // OpenCode and pi do not take an Antigravity login, and OpenCode does
    // not take Claude; keep supported agents off so tests never touch home.
    let antigravity_preferences = Preferences {
        agent_sync: syncing(&[SyncAgent::Opencode, SyncAgent::Pi]),
        ..grok_preferences.clone()
    };
    let credential = OAuthCredential {
        access: "a".into(),
        refresh: "r".into(),
        expires_ms: 1_780_000_000_000,
        email: Some("alice@gmail.com".into()),
        ..OAuthCredential::default()
    };
    let execution = after_switch(
        Pool::Antigravity,
        &antigravity_preferences,
        Some(Ok(credential)),
    )
    .await
    .unwrap();
    assert_eq!(execution, SwitchExecution::default());

    let claude_preferences = Preferences {
        agent_sync: syncing(&[SyncAgent::Opencode]),
        ..grok_preferences
    };
    let execution = after_switch(
        Pool::Claude,
        &claude_preferences,
        Some(Ok(claude_credential("alice", "org-1", "rt-1"))),
    )
    .await
    .unwrap();
    assert_eq!(execution, SwitchExecution::default());
}

#[tokio::test]
async fn after_switch_reports_a_login_granting_no_credential_to_each_agent() {
    // A login granting no Credential writes nothing, so no home is
    // touched; Grok's switch neither restarts editors nor launches Codex.
    let preferences = Preferences {
        agent_sync: syncing(&SyncAgent::ALL),
        restart_editors_on_switch: false,
        launch_codex_after_switch: false,
        ..Default::default()
    };
    let error = "Missing refresh token".to_string();
    let execution = after_switch(Pool::Grok, &preferences, Some(Err(error.clone())))
        .await
        .unwrap();
    assert_eq!(
        execution,
        SwitchExecution {
            agent_sync: SyncAgent::ALL
                .map(|agent| AgentSyncOutcome {
                    agent,
                    error: Some(error.clone()),
                })
                .to_vec(),
            ..SwitchExecution::default()
        }
    );
    // A provider's card hands over none: no agent is synced.
    let execution = after_switch(Pool::Grok, &preferences, None).await.unwrap();
    assert_eq!(execution, SwitchExecution::default());
}

/// The Credential of a Claude login: its account and organization
/// named after `name` and `org`, its email capitalized as Claude Code's
/// account may spell it.
fn claude_credential(name: &str, org: &str, refresh: &str) -> OAuthCredential {
    OAuthCredential {
        access: format!("sk-ant-oat-{name}"),
        refresh: refresh.into(),
        expires_ms: 1_780_000_000_000,
        account_id: Some(format!("acct-{name}")),
        org_id: Some(org.into()),
        email: Some(format!("{name}@Example.com")),
        org_name: Some(format!("{name}'s Organization")),
        scope: Some("user:inference".into()),
    }
}

fn grok_expires_ms() -> i64 {
    chrono::DateTime::parse_from_rfc3339("2026-11-26T12:00:00Z")
        .unwrap()
        .timestamp_millis()
}

/// The Credential of a Grok sign-in of `user`, its access token no JWT.
fn grok_credential(user: &str, refresh: &str) -> OAuthCredential {
    OAuthCredential {
        access: format!("grok-access-{user}"),
        refresh: refresh.into(),
        expires_ms: grok_expires_ms(),
        account_id: Some(format!("uid-{user}")),
        email: Some(format!("{user}@x.example")),
        ..OAuthCredential::default()
    }
}

#[test]
fn sync_pi_writes_anthropic_and_xai_entries_beside_openai_codex() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let codex = codex_credential("a@example.com", "acct-1", "plus", 1_780_000_000, "rt-1");
    sync_pi(&codex, std::slice::from_ref(&path)).unwrap();
    SyncAgent::Pi
        .sync_credential(
            Pool::Claude,
            &claude_credential("alice", "org-1", "claude-rt"),
            std::slice::from_ref(&path),
        )
        .unwrap();
    SyncAgent::Pi
        .sync_credential(
            Pool::Grok,
            &grok_credential("bob", "grok-rt"),
            std::slice::from_ref(&path),
        )
        .unwrap();

    let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(written["openai-codex"]["refresh"], "rt-1");
    assert_eq!(written["anthropic"]["type"], "oauth");
    assert_eq!(written["anthropic"]["access"], "sk-ant-oat-alice");
    assert_eq!(written["anthropic"]["refresh"], "claude-rt");
    assert_eq!(written["anthropic"]["expires"], 1_780_000_000_000_i64);
    assert!(written["anthropic"].get("accountId").is_none());
    assert_eq!(written["xai"]["type"], "oauth");
    assert_eq!(written["xai"]["access"], "grok-access-bob");
    assert_eq!(written["xai"]["refresh"], "grok-rt");
    assert!(written["xai"].get("accountId").is_none());
    assert_eq!(
        written["xai"]["expires"],
        chrono::DateTime::parse_from_rfc3339("2026-11-26T12:00:00Z")
            .unwrap()
            .timestamp()
            * 1000
    );
}

fn omp_rows(db_path: &Path, provider: &str) -> Vec<(i64, Value, Option<String>, Option<String>)> {
    let conn = Connection::open(db_path).unwrap();
    let mut stmt = conn
            .prepare("SELECT id, data, identity_key, disabled_cause FROM auth_credentials WHERE provider = ?1 ORDER BY id")
            .unwrap();
    let rows = stmt
        .query_map(params![provider], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                serde_json::from_str::<Value>(&r.get::<_, String>(1)?).unwrap(),
                r.get(2)?,
                r.get(3)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

#[test]
fn sync_omp_keys_anthropic_by_email_and_organization() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent.db");
    let sync = |credential: &OAuthCredential| {
        SyncAgent::Omp
            .sync_credential(Pool::Claude, credential, std::slice::from_ref(&db_path))
            .unwrap()
    };
    sync(&claude_credential("alice", "org-team", "rt-1"));
    let rows = omp_rows(&db_path, "anthropic");
    assert_eq!(rows.len(), 1);
    let (alice_id, data, key, cause) = &rows[0];
    assert_eq!(key.as_deref(), Some("email:alice@example.com|org:org-team"));
    assert_eq!(cause, &None);
    assert_eq!(data["access"], "sk-ant-oat-alice");
    assert_eq!(data["refresh"], "rt-1");
    assert_eq!(data["accountId"], "acct-alice");
    assert_eq!(data["orgId"], "org-team");
    // Written as the login spells it; keyed as rows are compared.
    assert_eq!(data["email"], "alice@Example.com");
    assert_eq!(data["orgName"], "alice's Organization");
    assert_eq!(data["scope"], "user:inference");

    // A renewal of the same login updates its row in place.
    sync(&claude_credential("alice", "org-team", "rt-2"));
    let rows = omp_rows(&db_path, "anthropic");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, *alice_id);
    assert_eq!(rows[0].1["refresh"], "rt-2");

    // The same email in another organization is another subscription.
    sync(&claude_credential("alice", "org-personal", "rt-3"));
    let rows = omp_rows(&db_path, "anthropic");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].3.as_deref(), Some(OMP_REPLACED_CAUSE));
    assert_eq!(
        rows[1].2.as_deref(),
        Some("email:alice@example.com|org:org-personal")
    );
    assert_eq!(rows[1].3, None);
}

#[test]
fn sync_omp_keys_xai_oauth_by_account_without_organization() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent.db");
    let sync = |credential: &OAuthCredential| {
        SyncAgent::Omp
            .sync_credential(Pool::Grok, credential, std::slice::from_ref(&db_path))
            .unwrap()
    };
    sync(&grok_credential("alice", "rt-a1"));
    let rows = omp_rows(&db_path, "xai-oauth");
    assert_eq!(rows.len(), 1);
    let (alice_id, data, key, _) = &rows[0];
    assert_eq!(key.as_deref(), Some("account:uid-alice"));
    assert_eq!(data["access"], "grok-access-alice");
    assert_eq!(data["accountId"], "uid-alice");
    assert_eq!(data["email"], "alice@x.example");
    assert!(data.get("orgId").is_none());

    sync(&grok_credential("alice", "rt-a2"));
    let rows = omp_rows(&db_path, "xai-oauth");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, *alice_id);
    assert_eq!(rows[0].1["refresh"], "rt-a2");

    // A teammate is another account: inserted, Alice's row disabled.
    sync(&grok_credential("bob", "rt-b1"));
    let rows = omp_rows(&db_path, "xai-oauth");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].3.as_deref(), Some(OMP_REPLACED_CAUSE));
    assert_eq!(rows[1].2.as_deref(), Some("account:uid-bob"));
    assert_eq!(rows[1].3, None);
}

#[test]
fn sync_omp_keys_antigravity_cli_by_account_else_email() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent.db");
    let alice = OAuthCredential {
        access: "agy-access-alice".into(),
        refresh: "agy-rt-a1".into(),
        expires_ms: grok_expires_ms(),
        account_id: Some("google-sub-alice".into()),
        email: Some("Alice@Gmail.com".into()),
        ..OAuthCredential::default()
    };
    SyncAgent::Omp
        .sync_credential(Pool::Antigravity, &alice, std::slice::from_ref(&db_path))
        .unwrap();
    let rows = omp_rows(&db_path, "antigravity-cli");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].2.as_deref(), Some("account:google-sub-alice"));
    assert_eq!(rows[0].1["access"], "agy-access-alice");
    assert_eq!(rows[0].1["refresh"], "agy-rt-a1");
    assert_eq!(rows[0].1["email"], "Alice@Gmail.com");
    assert_eq!(rows[0].1["accountId"], "google-sub-alice");

    // Email-only grant (as `agy acp --login` often exports) keys by email.
    let bob = OAuthCredential {
        access: "agy-access-bob".into(),
        refresh: "agy-rt-b1".into(),
        expires_ms: grok_expires_ms() + 3_600_000,
        email: Some("Bob@Gmail.com".into()),
        ..OAuthCredential::default()
    };
    SyncAgent::Omp
        .sync_credential(Pool::Antigravity, &bob, std::slice::from_ref(&db_path))
        .unwrap();
    let rows = omp_rows(&db_path, "antigravity-cli");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].3.as_deref(), Some(OMP_REPLACED_CAUSE));
    assert_eq!(rows[1].2.as_deref(), Some("email:bob@gmail.com"));
    assert_eq!(rows[1].3, None);
}

/// The Credential of a ChatGPT login of `email` in workspace
/// `account_id`, its access token carrying the claims agents' rows are
/// told apart by.
fn codex_credential(
    email: &str,
    account_id: &str,
    plan_type: &str,
    exp_sec: i64,
    refresh: &str,
) -> OAuthCredential {
    let access = fake_jwt(json!({
        "exp": exp_sec,
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id,
            "chatgpt_plan_type": plan_type,
        },
        "https://api.openai.com/profile": {
            "email": email,
        }
    }));
    OAuthCredential {
        access,
        refresh: refresh.into(),
        expires_ms: exp_sec * 1000,
        account_id: Some(account_id.into()),
        org_id: Some(account_id.into()),
        email: Some(email.into()),
        org_name: Some(plan_type.to_ascii_lowercase()),
        scope: None,
    }
}

#[test]
fn pi_paths_respect_explicit_agent_dir_and_default() {
    let home = PathBuf::from("/Users/tester");
    assert_eq!(
        pi_paths_from(Some(&home), Some("~/custom/pi-auth.json"), Some("/ignored")),
        vec![home.join("custom/pi-auth.json")]
    );
    assert_eq!(
        pi_paths_from(Some(&home), None, Some("~/my-pi")),
        vec![home.join("my-pi/auth.json")]
    );
    assert_eq!(
        pi_paths_from(Some(&home), None, None),
        vec![home.join(".pi/agent/auth.json")]
    );
}

#[test]
fn sync_opencode_updates_openai_entry_and_preserves_existing_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode/auth.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            r#"{"anthropic":{"type":"api_key","key":"sk-ant"},"openai":{"type":"oauth","extra":"keep"}}"#,
        )
        .unwrap();

    let auth = codex_credential("Alice@Example.com", "acct-1", "Pro", 1_780_000_000, "rt-1");
    sync_opencode(&auth, std::slice::from_ref(&path)).unwrap();

    let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(written["anthropic"]["key"], "sk-ant");
    assert_eq!(written["openai"]["type"], "oauth");
    assert_eq!(written["openai"]["extra"], "keep");
    assert_eq!(written["openai"]["access"], auth.access);
    assert_eq!(written["openai"]["refresh"], "rt-1");
    assert_eq!(written["openai"]["expires"], 1_780_000_000_000i64);
    assert_eq!(written["openai"]["accountId"], "acct-1");
}

#[test]
fn sync_pi_writes_openai_codex_oauth_and_preserves_other_providers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent/auth.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            "\u{feff}{\"anthropic\":{\"type\":\"api_key\",\"key\":\"sk-ant\"},\"openai-codex\":{\"type\":\"api_key\",\"key\":\"stale\"}}",
        )
        .unwrap();

    let auth = codex_credential("Alice@Example.com", "acct-1", "Pro", 1_780_000_000, "rt-1");
    sync_pi(&auth, std::slice::from_ref(&path)).unwrap();

    let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(written["anthropic"]["type"], "api_key");
    assert_eq!(written["anthropic"]["key"], "sk-ant");
    assert_eq!(written["openai-codex"]["type"], "oauth");
    assert_eq!(written["openai-codex"]["access"], auth.access);
    assert_eq!(written["openai-codex"]["refresh"], "rt-1");
    assert_eq!(written["openai-codex"]["expires"], 1_780_000_000_000i64);
    assert_eq!(written["openai-codex"]["accountId"], "acct-1");
    assert!(written["openai-codex"].get("key").is_none());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn sync_pi_rejects_non_object_document_and_empty_targets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    std::fs::write(&path, "[]").unwrap();
    let auth = codex_credential("a@example.com", "acct-1", "plus", 1_780_000_000, "rt-1");
    assert!(sync_pi(&auth, std::slice::from_ref(&path)).is_err());
    assert!(sync_pi(&auth, &[]).is_err());
}

#[test]
fn omp_paths_follow_profiles_env_overrides_and_xdg() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let xdg = dir.path().join("xdg-data");
    std::fs::create_dir_all(&home).unwrap();

    assert_eq!(
        omp_paths_from(
            Some(&home),
            Some("~/custom/agent.db"),
            Some("work"),
            None,
            None,
            None,
            None,
            true,
        ),
        vec![home.join("custom/agent.db")]
    );
    assert_eq!(
        omp_paths_from(Some(&home), None, None, None, None, None, None, true),
        vec![home.join(".omp/agent/agent.db")]
    );
    assert_eq!(
        omp_paths_from(
            Some(&home),
            None,
            Some("work"),
            None,
            None,
            Some("/ignored/agent"),
            None,
            true,
        ),
        vec![home.join(".omp/profiles/work/agent/agent.db")]
    );
    // Explicit empty OMP_PROFILE selects default and ignores a PI_PROFILE-derived agent dir.
    let derived = home.join(".omp/profiles/work/agent");
    assert_eq!(
        omp_paths_from(
            Some(&home),
            None,
            Some(""),
            Some("work"),
            None,
            Some(&derived.to_string_lossy()),
            None,
            true,
        ),
        vec![home.join(".omp/agent/agent.db")]
    );
    assert_eq!(
        omp_paths_from(
            Some(&home),
            None,
            None,
            None,
            Some(".custom-omp"),
            Some("~/custom-agent"),
            None,
            true,
        ),
        vec![home.join("custom-agent/agent.db")]
    );
    // XDG applies only once $XDG_DATA_HOME/omp exists, flattening agent/.
    assert_eq!(
        omp_paths_from(
            Some(&home),
            None,
            None,
            None,
            None,
            None,
            Some(&xdg.to_string_lossy()),
            true,
        ),
        vec![home.join(".omp/agent/agent.db")]
    );
    std::fs::create_dir_all(xdg.join("omp")).unwrap();
    assert_eq!(
        omp_paths_from(
            Some(&home),
            None,
            None,
            None,
            None,
            None,
            Some(&xdg.to_string_lossy()),
            true,
        ),
        vec![xdg.join("omp/agent.db")]
    );
    // Named profiles require the profile-specific XDG dir to exist.
    assert_eq!(
        omp_paths_from(
            Some(&home),
            None,
            Some("work"),
            None,
            None,
            None,
            Some(&xdg.to_string_lossy()),
            true,
        ),
        vec![home.join(".omp/profiles/work/agent/agent.db")]
    );
    std::fs::create_dir_all(xdg.join("omp/profiles/work")).unwrap();
    assert_eq!(
        omp_paths_from(
            Some(&home),
            None,
            Some("work"),
            None,
            None,
            None,
            Some(&xdg.to_string_lossy()),
            true,
        ),
        vec![xdg.join("omp/profiles/work/agent.db")]
    );
}

#[test]
fn sync_omp_creates_fresh_db_with_expected_schema_and_payload() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent/agent.db");
    let auth = codex_credential("Alice@Example.com", "ws-1", "Plus", 1_780_000_000, "rt-a");

    sync_omp(&auth, std::slice::from_ref(&db_path)).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    let (provider, ctype, data_raw, disabled_cause, identity_key): (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT provider, credential_type, data, disabled_cause, identity_key FROM auth_credentials",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
    assert_eq!(provider, "openai-codex");
    assert_eq!(ctype, "oauth");
    assert_eq!(disabled_cause, None);
    assert_eq!(
        identity_key.as_deref(),
        Some("email:alice@example.com|org:ws-1")
    );

    let data: Value = serde_json::from_str(&data_raw).unwrap();
    assert!(data.get("type").is_none());
    assert_eq!(data["access"], auth.access);
    assert_eq!(data["refresh"], "rt-a");
    assert_eq!(data["expires"], 1_780_000_000_000i64);
    assert_eq!(data["accountId"], "ws-1");
    assert_eq!(data["orgId"], "ws-1");
    assert_eq!(data["email"], "Alice@Example.com");
    assert_eq!(data["orgName"], "plus");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&db_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn sync_omp_replaces_active_codex_fires_revision_triggers_and_isolates_team_members() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE auth_credentials (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider TEXT NOT NULL,
                credential_type TEXT NOT NULL,
                data TEXT NOT NULL,
                disabled_cause TEXT DEFAULT NULL,
                identity_key TEXT DEFAULT NULL,
                created_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER)),
                updated_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER))
            );
            CREATE TABLE auth_change_revision (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                revision INTEGER NOT NULL
            );
            INSERT INTO auth_change_revision (id, revision) VALUES (1, 0);
            CREATE TRIGGER auth_change_revision_auth_credentials_insert
            AFTER INSERT ON auth_credentials
            BEGIN
                UPDATE auth_change_revision SET revision = revision + 1 WHERE id = 1;
            END;
            CREATE TRIGGER auth_change_revision_auth_credentials_update
            AFTER UPDATE ON auth_credentials
            BEGIN
                UPDATE auth_change_revision SET revision = revision + 1 WHERE id = 1;
            END;
            CREATE TRIGGER auth_change_revision_auth_credentials_delete
            AFTER DELETE ON auth_credentials
            BEGIN
                UPDATE auth_change_revision SET revision = revision + 1 WHERE id = 1;
            END;
            INSERT INTO auth_credentials (provider, credential_type, data, identity_key)
            VALUES ('anthropic', 'api_key', '{\"key\":\"sk-ant\"}', NULL);
            INSERT INTO auth_credentials (provider, credential_type, data, identity_key)
            VALUES ('openai-codex', 'api_key', '{\"key\":\"sk-old\"}', NULL);",
    )
    .unwrap();
    drop(conn);

    let revision = || -> i64 {
        Connection::open(&db_path)
            .unwrap()
            .query_row(
                "SELECT revision FROM auth_change_revision WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    let rev0 = revision();

    // 1. First switch to Alice in Team workspace ws-team: inserts Alice, disables old api_key.
    let alice1 = codex_credential(
        "alice@example.com",
        "ws-team",
        "team",
        1_780_000_000,
        "rt-a1",
    );
    sync_omp(&alice1, std::slice::from_ref(&db_path)).unwrap();
    let rev1 = revision();
    assert!(rev1 > rev0);

    // 2. Re-syncing identical credentials is a no-op (does not churn revision).
    sync_omp(&alice1, std::slice::from_ref(&db_path)).unwrap();
    assert_eq!(revision(), rev1);

    let conn = Connection::open(&db_path).unwrap();
    let alice_id: i64 = conn
            .query_row(
                "SELECT id FROM auth_credentials WHERE provider = 'openai-codex' AND disabled_cause IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
    drop(conn);

    // 3. Refreshing Alice's token updates her row in-place, preserving row id.
    let alice2 = codex_credential(
        "alice@example.com",
        "ws-team",
        "team",
        1_780_003_600,
        "rt-a2",
    );
    sync_omp(&alice2, std::slice::from_ref(&db_path)).unwrap();
    let conn = Connection::open(&db_path).unwrap();
    let (alice_id_after, alice_data): (i64, String) = conn
            .query_row(
                "SELECT id, data FROM auth_credentials WHERE provider = 'openai-codex' AND disabled_cause IS NULL",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
    assert_eq!(alice_id_after, alice_id);
    assert_eq!(
        serde_json::from_str::<Value>(&alice_data).unwrap()["refresh"],
        "rt-a2"
    );
    drop(conn);

    // 4. Switching to Bob in the SAME Team workspace inserts a distinct row and disables Alice.
    let bob = codex_credential("bob@example.com", "ws-team", "team", 1_780_007_200, "rt-b1");
    sync_omp(&bob, std::slice::from_ref(&db_path)).unwrap();
    let conn = Connection::open(&db_path).unwrap();
    let (bob_id, bob_key): (i64, Option<String>) = conn
            .query_row(
                "SELECT id, identity_key FROM auth_credentials WHERE provider = 'openai-codex' AND disabled_cause IS NULL",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
    assert_ne!(bob_id, alice_id);
    assert_eq!(
        bob_key.as_deref(),
        Some("email:bob@example.com|org:ws-team")
    );
    let alice_disabled_cause: Option<String> = conn
        .query_row(
            "SELECT disabled_cause FROM auth_credentials WHERE id = ?1",
            params![alice_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(alice_disabled_cause.as_deref(), Some(OMP_REPLACED_CAUSE));
    // Unrelated provider rows remain untouched.
    let anthropic_active: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM auth_credentials WHERE provider = 'anthropic' AND disabled_cause IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(anthropic_active, 1);
    drop(conn);

    // 5. Switching back to Alice activates Alice, disables Bob, and purges Alice's old tombstone.
    sync_omp(&alice2, std::slice::from_ref(&db_path)).unwrap();
    let conn = Connection::open(&db_path).unwrap();
    let alice_disabled_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM auth_credentials WHERE provider = 'openai-codex' AND identity_key = 'email:alice@example.com|org:ws-team' AND disabled_cause IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(alice_disabled_count, 0);
    let active_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM auth_credentials WHERE provider = 'openai-codex' AND disabled_cause IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(active_count, 1);
}

fn init_opencode_v2_db(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE credential (
                id TEXT PRIMARY KEY,
                integration_id TEXT,
                label TEXT NOT NULL,
                value TEXT NOT NULL,
                connector_id TEXT,
                method_id TEXT,
                active INTEGER,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL
            );",
    )
    .unwrap();
}

fn opencode_db_rows(
    db_path: &Path,
    integration_id: &str,
) -> Vec<(String, String, Value, Option<i64>, i64)> {
    let conn = Connection::open(db_path).unwrap();
    let mut stmt = conn
            .prepare(
                "SELECT id, label, value, active, time_updated FROM credential WHERE integration_id = ?1 ORDER BY time_created ASC, id ASC",
            )
            .unwrap();
    let rows = stmt
        .query_map(params![integration_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                serde_json::from_str::<Value>(&r.get::<_, String>(2)?).unwrap(),
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

#[test]
fn parse_opencode_major_version_handles_cli_formats() {
    assert_eq!(parse_opencode_major_version("1.19.6\n"), Some(1));
    assert_eq!(parse_opencode_major_version("2.4.1\n"), Some(2));
    assert_eq!(parse_opencode_major_version("v2.0.0-beta.1"), Some(2));
    assert_eq!(
        parse_opencode_major_version("opencode 2.3.1 (abc)"),
        Some(2)
    );
    assert_eq!(parse_opencode_major_version("opencode/1.2.3"), Some(1));
    assert_eq!(parse_opencode_major_version(""), None);
    assert_eq!(parse_opencode_major_version("unknown"), None);
}

#[test]
fn resolve_opencode_target_selects_storage_by_version_env_and_existing_db() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let xdg = dir.path().join("xdg");
    let fallback_json = vec![home.join(".config/opencode/auth.json")];

    // Explicit OPENCODE_AUTH_PATH picks SQLite or JSON by extension.
    assert_eq!(
        resolve_opencode_target(
            Some(1),
            Some(&home),
            Some("~/custom/opencode.db"),
            None,
            None,
            fallback_json.clone(),
        ),
        OpencodeTarget::Sqlite(home.join("custom/opencode.db"))
    );
    assert_eq!(
        resolve_opencode_target(
            Some(2),
            Some(&home),
            Some("~/custom/auth.json"),
            None,
            None,
            fallback_json.clone(),
        ),
        OpencodeTarget::Json(vec![home.join("custom/auth.json")])
    );

    // Explicit OPENCODE_DB resolves relative to data dir and ignores `:memory:`.
    assert_eq!(
        resolve_opencode_target(
            Some(1),
            Some(&home),
            None,
            Some("opencode-beta.db"),
            Some(&xdg.to_string_lossy()),
            fallback_json.clone(),
        ),
        OpencodeTarget::Sqlite(xdg.join("opencode/opencode-beta.db"))
    );
    assert_eq!(
        resolve_opencode_target(
            Some(1),
            Some(&home),
            None,
            Some(":memory:"),
            None,
            fallback_json.clone(),
        ),
        OpencodeTarget::Json(fallback_json.clone())
    );

    // OpenCode 1.x uses auth.json.
    assert_eq!(
        resolve_opencode_target(
            Some(1),
            Some(&home),
            None,
            None,
            None,
            fallback_json.clone(),
        ),
        OpencodeTarget::Json(fallback_json.clone())
    );

    // OpenCode 2.x before first launch writes `<data>/auth.json` for migration import.
    let default_db = home.join(".local/share/opencode/opencode.db");
    assert_eq!(
        resolve_opencode_target(
            Some(2),
            Some(&home),
            None,
            None,
            None,
            fallback_json.clone(),
        ),
        OpencodeTarget::Json(vec![home.join(".local/share/opencode/auth.json")])
    );

    // Once opencode.db exists, OpenCode 2.x targets SQLite.
    init_opencode_v2_db(&default_db);
    assert_eq!(
        resolve_opencode_target(
            Some(2),
            Some(&home),
            None,
            None,
            None,
            fallback_json.clone(),
        ),
        OpencodeTarget::Sqlite(default_db.clone())
    );

    // Unknown version falls back to SQLite when an initialized opencode.db exists.
    assert_eq!(
        resolve_opencode_target(None, Some(&home), None, None, None, fallback_json.clone()),
        OpencodeTarget::Sqlite(default_db)
    );
}

#[test]
fn sync_opencode_db_manages_openai_credentials_preserves_api_key_and_isolates_team_members() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("opencode.db");
    init_opencode_v2_db(&db_path);
    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
            "INSERT INTO credential (id, integration_id, label, value, active, time_created, time_updated) VALUES (?1, ?2, ?3, ?4, 1, 1000, 1000)",
            params!["cred_openai_key", "openai", "API key", r#"{"type":"key","key":"sk-old"}"#],
        )
        .unwrap();
    conn.execute(
            "INSERT INTO credential (id, integration_id, label, value, active, time_created, time_updated) VALUES (?1, ?2, ?3, ?4, 1, 1000, 1000)",
            params!["cred_anthropic_key", "anthropic", "API key", r#"{"type":"key","key":"sk-ant"}"#],
        )
        .unwrap();
    drop(conn);

    // 1. Switch to Alice in Team workspace ws-team.
    let alice1 = codex_credential(
        "Alice@Example.com",
        "ws-team",
        "team",
        1_780_000_000,
        "rt-a1",
    );
    sync_opencode(&alice1, std::slice::from_ref(&db_path)).unwrap();

    let rows = opencode_db_rows(&db_path, "openai");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "cred_openai_key");
    assert_eq!(rows[0].3, Some(0));
    let (alice_id, alice_label, alice_val, alice_active, alice_updated) = rows[1].clone();
    assert!(alice_id.starts_with("cred_"));
    assert_eq!(alice_label, "Alice@Example.com");
    assert_eq!(alice_active, Some(1));
    assert_eq!(alice_val["type"], "oauth");
    assert_eq!(alice_val["methodID"], "chatgpt-browser");
    assert_eq!(alice_val["refresh"], "rt-a1");
    assert_eq!(alice_val["expires"], 1_780_000_000_000_i64);
    assert_eq!(alice_val["metadata"]["accountID"], "ws-team");

    // Unrelated integrations remain active.
    let anthropic_rows = opencode_db_rows(&db_path, "anthropic");
    assert_eq!(anthropic_rows[0].3, Some(1));

    // 2. Re-syncing identical credentials is a no-op.
    sync_opencode(&alice1, std::slice::from_ref(&db_path)).unwrap();
    let rows_after_noop = opencode_db_rows(&db_path, "openai");
    assert_eq!(rows_after_noop.len(), 2);
    assert_eq!(rows_after_noop[1].4, alice_updated);

    // 3. Refreshing Alice updates her row in place and preserves headless methodID + custom metadata.
    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE credential SET label = 'Alice Work', value = ?1 WHERE id = ?2",
        params![
            json!({
                "type": "oauth",
                "methodID": "chatgpt-headless",
                "refresh": "rt-a1",
                "access": alice1.access,
                "expires": 1_780_000_000_000_i64,
                "metadata": { "accountID": "ws-team", "custom": "keep" }
            })
            .to_string(),
            &alice_id
        ],
    )
    .unwrap();
    drop(conn);

    let alice2 = codex_credential(
        "Alice@Example.com",
        "ws-team",
        "team",
        1_780_003_600,
        "rt-a2",
    );
    sync_opencode(&alice2, std::slice::from_ref(&db_path)).unwrap();
    let rows = opencode_db_rows(&db_path, "openai");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].0, alice_id);
    assert_eq!(rows[1].1, "Alice Work");
    assert_eq!(rows[1].2["methodID"], "chatgpt-headless");
    assert_eq!(rows[1].2["refresh"], "rt-a2");
    assert_eq!(rows[1].2["metadata"]["accountID"], "ws-team");
    assert_eq!(rows[1].2["metadata"]["custom"], "keep");

    // 4. Switching to Bob in the SAME Team workspace creates a separate row and deactivates Alice.
    let bob = codex_credential("bob@example.com", "ws-team", "team", 1_780_007_200, "rt-b1");
    sync_opencode(&bob, std::slice::from_ref(&db_path)).unwrap();
    let rows = opencode_db_rows(&db_path, "openai");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1].0, alice_id);
    assert_eq!(rows[1].3, Some(0));
    let bob_id = rows[2].0.clone();
    assert_ne!(bob_id, alice_id);
    assert_eq!(rows[2].1, "bob@example.com");
    assert_eq!(rows[2].3, Some(1));
    assert_eq!(rows[2].2["metadata"]["accountID"], "ws-team");

    // 5. Switching back to Alice reactivates Alice's existing row without duplicating.
    sync_opencode(&alice2, std::slice::from_ref(&db_path)).unwrap();
    let rows = opencode_db_rows(&db_path, "openai");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1].3, Some(1));
    assert_eq!(rows[2].3, Some(0));
}

#[test]
fn sync_opencode_db_writes_and_updates_xai_device_oauth() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("opencode.db");
    init_opencode_v2_db(&db_path);

    let alice_access = fake_jwt(json!({
        "sub": "uid-alice",
        "email": "alice@x.example",
        "exp": 1_800_000_000_i64
    }));
    let alice1 = OAuthCredential {
        access: alice_access,
        refresh: "xai-rt-a1".into(),
        expires_ms: 1_800_000_000_000,
        account_id: Some("uid-alice".into()),
        email: Some("Alice@x.example".into()),
        ..OAuthCredential::default()
    };
    sync_opencode_grok(&alice1, std::slice::from_ref(&db_path)).unwrap();

    let rows = opencode_db_rows(&db_path, "xai");
    assert_eq!(rows.len(), 1);
    let alice_id = rows[0].0.clone();
    assert_eq!(rows[0].1, "Alice@x.example");
    assert_eq!(rows[0].2["type"], "oauth");
    assert_eq!(rows[0].2["methodID"], "device");
    assert_eq!(rows[0].2["refresh"], "xai-rt-a1");
    assert!(rows[0].2.get("metadata").is_none());
    assert_eq!(rows[0].3, Some(1));

    // Token renewal updates Alice in place by matching JWT `sub`.
    let alice_access2 = fake_jwt(json!({
        "sub": "uid-alice",
        "email": "alice@x.example",
        "exp": 1_800_003_600_i64
    }));
    let alice2 = OAuthCredential {
        access: alice_access2,
        refresh: "xai-rt-a2".into(),
        expires_ms: 1_800_003_600_000,
        account_id: Some("uid-alice".into()),
        email: Some("alice@x.example".into()),
        ..OAuthCredential::default()
    };
    sync_opencode_grok(&alice2, std::slice::from_ref(&db_path)).unwrap();
    let rows = opencode_db_rows(&db_path, "xai");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, alice_id);
    assert_eq!(rows[0].2["refresh"], "xai-rt-a2");

    // Switching to Bob adds a separate active xai row and deactivates Alice.
    let bob = grok_credential("bob", "xai-rt-b1");
    sync_opencode_grok(&bob, std::slice::from_ref(&db_path)).unwrap();
    let rows = opencode_db_rows(&db_path, "xai");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].3, Some(0));
    assert_eq!(rows[1].1, "bob@x.example");
    assert_eq!(rows[1].2["methodID"], "device");
    assert_eq!(rows[1].3, Some(1));
}

#[test]
fn agents_match_an_existing_row_whose_email_differs_only_in_case() {
    let dir = tempfile::tempdir().unwrap();
    // An omp row keyed by oh-my-pi itself, with the email spelled otherwise.
    let db_path = dir.path().join("agent.db");
    sync_omp_one(
        &OAuthCredential {
            access: "old-access".into(),
            refresh: "old-refresh".into(),
            expires_ms: 1,
            org_id: Some("org-team".into()),
            ..OAuthCredential::default()
        },
        ANTHROPIC_PROVIDER,
        &db_path,
    )
    .unwrap();
    Connection::open(&db_path)
        .unwrap()
        .execute(
            "UPDATE auth_credentials SET identity_key = 'email:ALICE@Example.COM|org:org-team'",
            [],
        )
        .unwrap();
    let existing = omp_rows(&db_path, "anthropic")[0].0;
    SyncAgent::Omp
        .sync_credential(
            Pool::Claude,
            &claude_credential("alice", "org-team", "rt-new"),
            std::slice::from_ref(&db_path),
        )
        .unwrap();
    let rows = omp_rows(&db_path, "anthropic");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, existing);
    assert_eq!(rows[0].1["refresh"], "rt-new");
    assert_eq!(rows[0].1["email"], "alice@Example.com");

    // An OpenCode row of the same ChatGPT workspace member.
    let db_path = dir.path().join("opencode.db");
    init_opencode_v2_db(&db_path);
    let old_access = fake_jwt(json!({
        "https://api.openai.com/auth": {"chatgpt_account_id": "ws-team"},
        "https://api.openai.com/profile": {"email": "ALICE@EXAMPLE.COM"}
    }));
    Connection::open(&db_path)
            .unwrap()
            .execute(
                "INSERT INTO credential (id, integration_id, label, value, active, time_created, time_updated) VALUES ('cred_alice', 'openai', 'Alice', ?1, 1, 1000, 1000)",
                params![json!({"type": "oauth", "access": old_access, "refresh": "old-refresh", "metadata": {"accountID": "ws-team"}}).to_string()],
            )
            .unwrap();
    let alice = codex_credential(
        "Alice@Example.com",
        "ws-team",
        "team",
        1_780_000_000,
        "rt-a",
    );
    sync_opencode(&alice, std::slice::from_ref(&db_path)).unwrap();
    let rows = opencode_db_rows(&db_path, "openai");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "cred_alice");
    assert_eq!(rows[0].2["refresh"], "rt-a");
}

#[test]
fn sync_opencode_db_rejects_uninitialized_database() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("opencode.db");
    let auth = codex_credential("a@example.com", "acct-1", "plus", 1_780_000_000, "rt-1");
    assert!(sync_opencode(&auth, std::slice::from_ref(&missing))
        .unwrap_err()
        .contains("not found"));
    assert!(!missing.exists());

    // An empty DB without the `credential` table is rejected without mutating schema.
    Connection::open(&missing).unwrap();
    assert!(sync_opencode(&auth, std::slice::from_ref(&missing))
        .unwrap_err()
        .contains("missing the credential table"));
}
