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
        "",
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
    assert!(
        sync_command("/srv/codex", "unit", "/tmp/stage", true, "").contains("systemctl restart")
    );
    assert!(
        !sync_command("/srv/codex", "unit", "/tmp/stage", false, "").contains("systemctl restart")
    );
    assert!(sync_command("/srv/codex", "unit", "/tmp/stage", false, "").contains("true"));
}
#[cfg(unix)]
#[test]
fn testSyncRefusesAccountsTheDaemonWroteSinceTheSnapshot() {
    let d = tempfile::tempdir().unwrap();
    let (dir, stage) = (d.path().join("srv"), d.path().join("stage"));
    std::fs::create_dir_all(&dir).unwrap();
    let run = |command: &str| {
        std::process::Command::new("sh")
            .args(["-c", command])
            .output()
            .unwrap()
    };
    let (dir_s, stage_s) = (dir.to_str().unwrap(), stage.to_str().unwrap());
    let remote = dir.join("accounts.json");
    let snapshot = || {
        std::fs::create_dir_all(&stage).unwrap();
        let output = run(&crate::remote::snapshot_command(dir_s, stage_s));
        crate::remote::parse_snapshot(&String::from_utf8_lossy(&output.stdout))
    };
    std::fs::write(&remote, r#"{"accounts":[{"id":"older"}]}"#).unwrap();
    let (checksum, accounts) = snapshot();
    assert_eq!(accounts.unwrap()["accounts"][0]["id"], "older");
    // The daemon renews a token after the snapshot was taken.
    std::fs::write(&remote, r#"{"accounts":[{"id":"renewed"}]}"#).unwrap();
    std::fs::write(stage.join("accounts.json"), "uploaded").unwrap();
    let refused = run(&sync_command(dir_s, "unit", stage_s, false, &checksum));
    assert_eq!(refused.status.code(), Some(75));
    assert!(String::from_utf8_lossy(&refused.stderr).contains(crate::remote::ACCOUNTS_CHANGED));
    assert_eq!(
        std::fs::read_to_string(&remote).unwrap(),
        r#"{"accounts":[{"id":"renewed"}]}"#
    );
    let (checksum, _) = snapshot();
    assert!(run(&sync_command(dir_s, "unit", stage_s, false, &checksum))
        .status
        .success());
    assert_eq!(std::fs::read_to_string(&remote).unwrap(), "uploaded");
    // A first sync finds no accounts, and expects none.
    std::fs::remove_file(&remote).unwrap();
    let (checksum, accounts) = snapshot();
    assert_eq!((checksum.as_str(), accounts), ("", None));
    std::fs::write(stage.join("accounts.json"), "first").unwrap();
    assert!(run(&sync_command(dir_s, "unit", stage_s, false, &checksum))
        .status
        .success());
    assert_eq!(std::fs::read_to_string(&remote).unwrap(), "first");
}
/// An executable shell script at `path` running `body`.
#[cfg(unix)]
fn executable(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}
#[cfg(unix)]
#[test]
fn testSyncStopsADaemonBeforeTheLockWhatEverItsUnitIsDoing() {
    let d = tempfile::tempdir().unwrap();
    let (dir, stage, bin) = (
        d.path().join("srv"),
        d.path().join("stage"),
        d.path().join("bin"),
    );
    for path in [&dir, &stage, &bin] {
        std::fs::create_dir_all(path).unwrap();
    }
    // A daemon before the lock refuses the question.
    executable(&dir.join("codex-tools-proxyd"), "exit 2");
    // The unit as systemd has it: its state, and whether it will not stop.
    let (state, stuck, log) = (
        d.path().join("state"),
        d.path().join("stuck"),
        d.path().join("log"),
    );
    executable(
        &bin.join("systemctl"),
        &format!(
            r#"case "$1" in
show) echo LoadState=loaded ;;
is-active) cat '{state}' ;;
stop) echo "stop $(cat '{accounts}' 2>/dev/null)" >> '{log}'; [ -f '{stuck}' ] || echo inactive > '{state}' ;;
start|restart) echo "$1 $(cat '{accounts}' 2>/dev/null)" >> '{log}'; echo active > '{state}' ;;
esac"#,
            state = state.display(),
            stuck = stuck.display(),
            log = log.display(),
            accounts = dir.join("accounts.json").display(),
        ),
    );
    let run = |command: &str| {
        std::process::Command::new("sh")
            .args(["-c", command])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap()
    };
    let (dir_s, stage_s) = (dir.to_str().unwrap(), stage.to_str().unwrap());
    let upload = |content: &str| {
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("accounts.json"), content).unwrap();
    };
    let take_log = || {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let _ = std::fs::remove_file(&log);
        text
    };
    // A unit restarting automatically is stopped, then started again.
    std::fs::write(&state, "activating\n").unwrap();
    upload("uploaded");
    assert!(run(&sync_command(dir_s, "unit", stage_s, false, ""))
        .status
        .success());
    assert_eq!(take_log(), "stop \nstart uploaded\n");
    // Refused as changed, it is started again too.
    upload("again");
    let refused = run(&sync_command(dir_s, "unit", stage_s, false, "stale"));
    assert_eq!(refused.status.code(), Some(75));
    assert_eq!(take_log(), "stop uploaded\nstart uploaded\n");
    // A unit stopped already stays so.
    std::fs::write(&state, "inactive\n").unwrap();
    let (checksum, _) = parse_snapshot(&String::from_utf8_lossy(
        &run(&format!(
            "cksum < '{}'",
            dir.join("accounts.json").display()
        ))
        .stdout,
    ));
    assert!(run(&sync_command(dir_s, "unit", stage_s, false, &checksum))
        .status
        .success());
    assert_eq!(take_log(), "stop uploaded\n");
    // A unit that will not stop refuses the sync, the accounts untouched.
    std::fs::write(&state, "activating\n").unwrap();
    std::fs::write(&stuck, "").unwrap();
    upload("unheld");
    let (checksum, _) = parse_snapshot(&String::from_utf8_lossy(
        &run(&format!(
            "cksum < '{}'",
            dir.join("accounts.json").display()
        ))
        .stdout,
    ));
    let unheld = run(&sync_command(dir_s, "unit", stage_s, false, &checksum));
    assert!(!unheld.status.success());
    assert!(String::from_utf8_lossy(&unheld.stderr).contains(DAEMON_NOT_HELD));
    assert_eq!(
        std::fs::read_to_string(dir.join("accounts.json")).unwrap(),
        "again"
    );
    take_log();
}
/// Run by no unit, the daemon is found from `/proc`, which only Linux has:
/// at its path, behind a link, and replaced while it runs.
#[cfg(target_os = "linux")]
#[test]
fn testSyncRefusesADaemonNoUnitRuns() {
    let d = tempfile::tempdir().unwrap();
    let (dir, stage, bin, elsewhere) = (
        d.path().join("srv"),
        d.path().join("stage"),
        d.path().join("bin"),
        d.path().join("elsewhere"),
    );
    for path in [&dir, &stage, &bin, &elsewhere] {
        std::fs::create_dir_all(path).unwrap();
    }
    // No unit is loaded.
    executable(&bin.join("systemctl"), "echo LoadState=not-found; exit 3");
    std::fs::write(dir.join("accounts.json"), "kept").unwrap();
    let checksum = String::from_utf8_lossy(
        &std::process::Command::new("sh")
            .args([
                "-c",
                &format!("cksum < '{}'", dir.join("accounts.json").display()),
            ])
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_owned();
    let daemon = dir.join("codex-tools-proxyd");
    let sync = || {
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("accounts.json"), "uploaded").unwrap();
        std::process::Command::new("sh")
            .args([
                "-c",
                &sync_command(
                    dir.to_str().unwrap(),
                    "unit",
                    stage.to_str().unwrap(),
                    false,
                    &checksum,
                ),
            ])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap()
    };
    let refused_while = |start: &dyn Fn() -> std::process::Child, after: &dyn Fn()| {
        let mut running = start();
        after();
        let output = sync();
        running.kill().unwrap();
        running.wait().unwrap();
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(DAEMON_NOT_HELD),
            "{output:?}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("accounts.json")).unwrap(),
            "kept"
        );
    };
    let spawn = || {
        std::process::Command::new(&daemon)
            .arg("30")
            .spawn()
            .unwrap()
    };
    // At its path.
    std::fs::copy("/bin/sleep", &daemon).unwrap();
    refused_while(&spawn, &|| {});
    // Replaced while it runs.
    refused_while(&spawn, &|| {
        std::fs::remove_file(&daemon).unwrap();
        std::fs::copy("/bin/sleep", &daemon).unwrap();
    });
    // Behind a link.
    std::fs::remove_file(&daemon).unwrap();
    let target = elsewhere.join("codex-tools-proxyd-build");
    std::fs::copy("/bin/sleep", &target).unwrap();
    std::os::unix::fs::symlink(&target, &daemon).unwrap();
    refused_while(&spawn, &|| {});
    // None running, the sync goes on.
    assert!(sync().status.success());
    assert_eq!(
        std::fs::read_to_string(dir.join("accounts.json")).unwrap(),
        "uploaded"
    );
}
#[cfg(unix)]
#[test]
fn testSyncWaitsForTheDaemonsLockToCheckTheAccounts() {
    let has_flock = std::process::Command::new("sh")
        .args(["-c", "command -v flock"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_flock {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (dir, stage) = (d.path().join("srv"), d.path().join("stage"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&stage).unwrap();
    let remote = dir.join("accounts.json");
    std::fs::write(&remote, "older").unwrap();
    let output = std::process::Command::new("sh")
        .args(["-c", &format!("cksum < '{}'", remote.display())])
        .output()
        .unwrap();
    let checksum = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    std::fs::write(stage.join("accounts.json"), "uploaded").unwrap();
    // A daemon that takes the lock says so.
    executable(
        &dir.join("codex-tools-proxyd"),
        "[ \"$1\" = --locks-store ] && echo accounts.json.lock",
    );
    // The daemon is changing its accounts as the sync starts.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("accounts.json.lock"))
        .unwrap();
    lock.lock().unwrap();
    let sync = std::process::Command::new("sh")
        .args([
            "-c",
            &sync_command(
                dir.to_str().unwrap(),
                "unit",
                stage.to_str().unwrap(),
                false,
                &checksum,
            ),
        ])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    std::fs::write(&remote, "renewed").unwrap();
    drop(lock);
    let refused = sync.wait_with_output().unwrap();
    assert_eq!(refused.status.code(), Some(75));
    assert_eq!(std::fs::read_to_string(&remote).unwrap(), "renewed");
}
#[cfg(unix)]
#[test]
fn testSyncAndInstallReportAnAccountsFileNotInstalled() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let (dir, stage, bin) = (
        d.path().join("srv"),
        d.path().join("stage"),
        d.path().join("bin"),
    );
    for path in [&stage, &bin] {
        std::fs::create_dir_all(path).unwrap();
    }
    // A read-only file system refuses every move.
    let mv = bin.join("mv");
    std::fs::write(
        &mv,
        "#!/bin/sh\necho 'mv: read-only file system' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&mv, std::fs::Permissions::from_mode(0o700)).unwrap();
    // No unit of this machine's is touched.
    let systemctl = bin.join("systemctl");
    std::fs::write(&systemctl, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o700)).unwrap();
    let run = |command: &str| {
        std::process::Command::new("sh")
            .args(["-c", command])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap()
    };
    let (dir_s, stage_s) = (dir.to_str().unwrap(), stage.to_str().unwrap());
    std::fs::write(stage.join("accounts.json"), "uploaded").unwrap();
    let synced = run(&sync_command(dir_s, "unit", stage_s, false, ""));
    assert!(!synced.status.success());
    assert!(!dir.join("accounts.json").exists());
    let mut server = draft("server-1");
    server.remote_dir = dir_s.into();
    server.listen_port = 1;
    let installed = run(&install_command(&server, "unit", stage_s, ""));
    assert!(!installed.status.success());
    assert!(!dir.join("accounts.json").exists());
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
#[test]
fn a_claude_server_runs_the_daemon_for_its_pool() {
    let mut server = draft("server-1");
    // A Codex server keeps the command daemons before pools understand.
    assert_eq!(server.pool, crate::Pool::Codex);
    let unit = systemd_unit(&server, "codex-tools-proxyd-server-1.service");
    assert!(!unit.contains("--pool"));
    server.pool = crate::Pool::Claude;
    let unit = systemd_unit(&server, "codex-tools-proxyd-server-1.service");
    assert!(unit.contains("--port 8787 --pool claude --no-sync-current-auth"));
    let saved = serde_json::to_value(&server).unwrap();
    assert_eq!(saved["pool"], "claude");
    // Discovery reports each instance's pool, Codex for a command without one.
    let discover = discover_command();
    assert!(discover.contains("--pool \\([a-z]*\\)"));
    assert!(discover.contains("[ -n \"$POOL\" ] || POOL=codex"));
    assert!(discover.contains("pool=%s"));
}
