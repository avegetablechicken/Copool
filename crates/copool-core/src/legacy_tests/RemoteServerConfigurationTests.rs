use crate::remote::*;
use serde_json::json;
#[test]
fn testMakeDraftUsesSharedDefaults() {
    let d = draft("server-1");
    assert_eq!(d.id, "server-1");
    assert_eq!(d.label, "new-server");
    assert_eq!(d.ssh_port, 22);
    assert_eq!(d.ssh_user, "root");
    assert_eq!(serde_json::to_value(d.auth_mode).unwrap(), "keyPath");
    assert_eq!(d.remote_dir, "/opt/codex-tools");
    assert_eq!(d.listen_port, 8787);
}
#[test]
fn testNormalizeTrimsFieldsAndBackfillsID() {
    let d=normalize(json!({"id":"   ","label":" Tokyo ","host":" 1.2.3.4 ","sshPort":22,"sshUser":" root ","authMode":" keyPath ","identityFile":" ~/.ssh/id_ed25519 ","privateKey":"  ","password":"\nsecret\n","remoteDir":" /opt/codex-tools ","listenPort":8787}),||"generated-id".into()).unwrap();
    assert_eq!(d.id, "generated-id");
    assert_eq!(d.label, "Tokyo");
    assert_eq!(d.host, "1.2.3.4");
    assert_eq!(d.ssh_user, "root");
    assert_eq!(d.identity_file.as_deref(), Some("~/.ssh/id_ed25519"));
    assert!(d.private_key.is_none());
    assert_eq!(d.password.as_deref(), Some("secret"));
    assert_eq!(d.remote_dir, "/opt/codex-tools");
}
#[test]
fn testUpsertReplacesExistingServerByID() {
    let mut next = draft("server-1");
    next.label = " New ".into();
    next.host = " 2.2.2.2 ".into();
    next.ssh_user = " admin ".into();
    next.auth_mode = crate::models::RemoteAuthMode::Password;
    next.ssh_port = 2200;
    next.password = Some(" pass ".into());
    next.remote_dir = " /srv/codex ".into();
    next.listen_port = 9797;
    let merged = upsert(next, vec![draft("server-1")]).unwrap();
    assert_eq!(merged.len(), 1);
    assert_eq!(
        serde_json::to_value(&merged[0].auth_mode).unwrap(),
        "password"
    );
    assert_eq!(merged[0].label, "New");
    assert_eq!(merged[0].host, "2.2.2.2");
    assert_eq!(merged[0].ssh_user, "admin");
    assert_eq!(merged[0].password.as_deref(), Some("pass"));
    assert_eq!(merged[0].remote_dir, "/srv/codex");
    assert_eq!(merged[0].listen_port, 9797);
}
#[test]
fn testDeploymentPlanSanitizesServiceName() {
    assert_eq!(
        service_name("prod server/01"),
        "codex-tools-proxyd-prod-server-01.service"
    );
}
#[test]
fn testAdoptingDiscoveredInstanceBackfillsIdentityLabelAndEndpoint() {
    let mut d = draft("draft-1");
    d.host = "1.2.3.4".into();
    let a = adopt(
        d,
        &json!({"serverId":"server-1","label":"Tokyo","remoteDir":"/srv/copool","listenPort":9898}),
    );
    assert_eq!(a.id, "server-1");
    assert_eq!(a.label, "Tokyo");
    assert_eq!(a.remote_dir, "/srv/copool");
    assert_eq!(a.listen_port, 9898);
    assert_eq!(a.host, "1.2.3.4");
}
#[test]
fn testDeploymentPlanSystemdUnitCarriesRemoteServerMetadata() {
    let mut d = draft("server-1");
    d.label = "Tokyo 01".into();
    let s = systemd_unit(&d, "codex-tools-proxyd-server-1.service");
    assert!(s.contains("Environment=\"COPOOL_SERVER_ID=server-1\""));
    assert!(s.contains("Environment=\"COPOOL_SERVER_LABEL=Tokyo 01\""));
}
#[test]
fn testDeploymentPlanInstallCommandReplacesConflictingServiceByDirectoryOrPort() {
    let mut d = draft("server-1");
    d.remote_dir = "/srv/codex tools".into();
    let s = install_command(
        &d,
        &service_name(&d.id),
        "/tmp/codex-tools-remote-server-1-123",
    );
    for text in [
        "TARGET_DIR='/srv/codex tools'",
        "TARGET_PORT='8787'",
        r#"[ "$UNIT_NAME" = "$TARGET_UNIT" ] && continue"#,
        r#"WORK_DIR=$(sed -n 's/^WorkingDirectory=//p' "$UNIT_PATH" | head -n 1)"#,
        r#"PORT=$(sed -n 's/^ExecStart=.* --port \([0-9][0-9]*\).*$/\1/p' "$UNIT_PATH" | head -n 1)"#,
        r#"if [ "$WORK_DIR" = "$TARGET_DIR" ] || [ "$PORT" = "$TARGET_PORT" ]; then"#,
        r#"rm -f "/etc/systemd/system/$UNIT_NAME" "/lib/systemd/system/$UNIT_NAME""#,
    ] {
        assert!(s.contains(text), "{text}");
    }
}
#[test]
fn testDeploymentPlanDiscoverCommandParsesWorkingDirectoryAndPort() {
    let s = discover_command();
    for text in [
        "MARKER='__COPool_DISCOVERY__'",
        "/usr/lib/systemd/system/codex-tools-proxyd-*.service",
        r#"WORK_DIR=$(sed -n 's/^WorkingDirectory=//p' "$UNIT_PATH" | head -n 1)"#,
        "SERVER_ID=$(sed -n",
        "COPOOL_SERVER_ID",
        "LABEL=$(sed -n",
        "COPOOL_SERVER_LABEL",
        r#"PORT=$(sed -n 's/^ExecStart=.* --port \([0-9][0-9]*\).*$/\1/p' "$UNIT_PATH" | head -n 1)"#,
        "server_id=%s",
        "label=%s",
        "api_key_present=%s",
    ] {
        assert!(s.contains(text), "{text}");
    }
}
#[test]
fn testDeploymentPlanSyncAccountsCommandRestartsOnlyWhenRequested() {
    assert!(sync_command("/srv/codex", "unit", "/tmp/stage", true).contains("systemctl restart"));
    assert!(!sync_command("/srv/codex", "unit", "/tmp/stage", false).contains("systemctl restart"));
    assert!(sync_command("/srv/codex", "unit", "/tmp/stage", false).contains("true"));
}
#[test]
fn testDeploymentPlanUninstallCommandStopsDisablesAndRemovesServiceArtifacts() {
    let s = uninstall_command("/srv/codex", "unit", false);
    for text in [
        "systemctl stop",
        "systemctl disable",
        "/etc/systemd/system/$UNIT",
        "/lib/systemd/system/$UNIT",
        "/usr/lib/systemd/system/$UNIT",
        "systemctl daemon-reload",
        "rm -f '/srv/codex/codex-tools-proxyd' '/srv/codex/accounts.json'",
    ] {
        assert!(s.contains(text), "{text}");
    }
}
#[test]
fn testDeploymentPlanStatusCommandChecksUsrLibSystemdDirectory() {
    assert!(status_command("/srv/codex", "unit").contains("/usr/lib/systemd/system/$UNIT"));
}
