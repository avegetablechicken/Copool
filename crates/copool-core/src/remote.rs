//! Deterministic remote deployment plans. Rendering never executes SSH or sudo.
use crate::models::RemoteServerConfig;
use serde_json::{json, Value};

pub(crate) fn draft(id: &str) -> RemoteServerConfig {
    serde_json::from_value(json!({"id":id,"label":"new-server","host":"","sshPort":22,"sshUser":"root","authMode":"keyPath","remoteDir":"/opt/codex-tools","listenPort":8787})).unwrap()
}
pub(crate) fn normalize(
    mut value: Value,
    make_id: impl FnOnce() -> String,
) -> Result<RemoteServerConfig, String> {
    let object = value.as_object_mut().ok_or("Invalid remote server")?;
    for name in [
        "id",
        "label",
        "host",
        "sshUser",
        "authMode",
        "identityFile",
        "privateKey",
        "password",
        "remoteDir",
    ] {
        if let Some(text) = object.get(name).and_then(Value::as_str) {
            let text = text.trim().to_owned();
            object.insert(
                name.into(),
                if text.is_empty() && ["identityFile", "privateKey", "password"].contains(&name) {
                    Value::Null
                } else {
                    text.into()
                },
            );
        }
    }
    if object
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .is_empty()
    {
        object.insert("id".into(), make_id().into());
    }
    if object
        .get("authMode")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .is_empty()
    {
        object.insert("authMode".into(), "keyPath".into());
    }
    serde_json::from_value(value).map_err(|e| e.to_string())
}
pub(crate) fn upsert(
    server: RemoteServerConfig,
    mut servers: Vec<RemoteServerConfig>,
) -> Result<Vec<RemoteServerConfig>, String> {
    let server = normalize(
        serde_json::to_value(server).map_err(|e| e.to_string())?,
        || uuid::Uuid::new_v4().to_string(),
    )?;
    if let Some(existing) = servers.iter_mut().find(|v| v.id == server.id) {
        *existing = server;
    } else {
        servers.push(server);
    }
    Ok(servers)
}
pub(crate) fn adopt(mut server: RemoteServerConfig, instance: &Value) -> RemoteServerConfig {
    if let Some(id) = instance["serverId"]
        .as_str()
        .filter(|v| !v.trim().is_empty())
    {
        server.id = id.trim().into();
    }
    if let Some(label) = instance["label"].as_str().filter(|v| !v.trim().is_empty()) {
        server.label = label.trim().into();
    }
    if let Some(dir) = instance["remoteDir"].as_str() {
        server.remote_dir = dir.into();
    }
    if let Some(port) = instance["listenPort"]
        .as_u64()
        .and_then(|p| u16::try_from(p).ok())
    {
        server.listen_port = port;
    }
    server
}
pub(crate) fn service_name(id: &str) -> String {
    let fragment: String = id
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || "-_".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!(
        "codex-tools-proxyd-{}.service",
        if fragment.is_empty() {
            "default"
        } else {
            &fragment
        }
    )
}
pub(crate) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
fn systemd_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('%', "%%")
}
pub(crate) fn systemd_unit(server: &RemoteServerConfig, unit: &str) -> String {
    let dir = systemd_string(&server.remote_dir);
    format!("[Unit]\nDescription=Codex Tools Remote API Proxy ({unit})\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nWorkingDirectory={dir}\nEnvironment=\"COPOOL_SERVER_ID={}\"\nEnvironment=\"COPOOL_SERVER_LABEL={}\"\nExecStart=\"{dir}/codex-tools-proxyd\" serve --data-dir \"{dir}\" --host 0.0.0.0 --port {} --no-sync-current-auth\nRestart=always\nRestartSec=3\nEnvironment=RUST_LOG=info\n\n[Install]\nWantedBy=multi-user.target\n",systemd_string(&server.id),systemd_string(&server.label),server.listen_port)
}
const UNIT_DIRS:&str="/etc/systemd/system/codex-tools-proxyd-*.service /lib/systemd/system/codex-tools-proxyd-*.service /usr/lib/systemd/system/codex-tools-proxyd-*.service";
pub(crate) fn install_command(server: &RemoteServerConfig, unit: &str, stage: &str) -> String {
    let mut command = format!(
        "mkdir -p {}; TARGET_DIR={}; TARGET_PORT={}; TARGET_UNIT={}; ",
        quote(&server.remote_dir),
        quote(&server.remote_dir),
        quote(&server.listen_port.to_string()),
        quote(unit)
    );
    command.push_str(&format!("for UNIT_PATH in {UNIT_DIRS}; do "));
    command.push_str(r#"[ -f "$UNIT_PATH" ] || continue; UNIT_NAME=$(basename "$UNIT_PATH"); [ "$UNIT_NAME" = "$TARGET_UNIT" ] && continue; WORK_DIR=$(sed -n 's/^WorkingDirectory=//p' "$UNIT_PATH" | head -n 1); PORT=$(sed -n 's/^ExecStart=.* --port \([0-9][0-9]*\).*$/\1/p' "$UNIT_PATH" | head -n 1); if [ "$WORK_DIR" = "$TARGET_DIR" ] || [ "$PORT" = "$TARGET_PORT" ]; then systemctl stop "$UNIT_NAME" >/dev/null 2>&1 || true; systemctl disable "$UNIT_NAME" >/dev/null 2>&1 || true; rm -f "/etc/systemd/system/$UNIT_NAME" "/lib/systemd/system/$UNIT_NAME" "/usr/lib/systemd/system/$UNIT_NAME"; fi; done; "#);
    for (name, mode, dest) in [
        (
            "codex-tools-proxyd",
            700,
            format!("{}/codex-tools-proxyd", server.remote_dir),
        ),
        (
            "accounts.json",
            600,
            format!("{}/accounts.json", server.remote_dir),
        ),
        (unit, 644, format!("/etc/systemd/system/{unit}")),
    ] {
        command.push_str(&format!(
            "mv {} {}; chmod {mode} {}; ",
            quote(&format!("{stage}/{name}")),
            quote(&dest),
            quote(&dest)
        ));
    }
    command.push_str(&format!("rm -rf {}; systemctl daemon-reload; systemctl enable {} >/dev/null 2>&1 || true; if systemctl is-active --quiet {}; then systemctl restart {}; else systemctl start {}; fi",quote(stage),quote(unit),quote(unit),quote(unit),quote(unit)));
    command
}
pub(crate) fn sync_command(dir: &str, unit: &str, stage: &str, restart: bool) -> String {
    let mut command=format!("DIR={}; UNIT={}; mkdir -p \"$DIR\"; mv {} \"$DIR/accounts.json\"; chmod 600 \"$DIR/accounts.json\"; rm -rf {}; ",quote(dir),quote(unit),quote(&format!("{stage}/accounts.json")),quote(stage));
    command.push_str(if restart{r#"if [ -f "/etc/systemd/system/$UNIT" ] || [ -f "/lib/systemd/system/$UNIT" ] || [ -f "/usr/lib/systemd/system/$UNIT" ]; then systemctl restart "$UNIT"; fi;"#}else{"true"});
    command
}
pub(crate) fn uninstall_command(dir: &str, unit: &str, remove_dir: bool) -> String {
    let mut command = format!("UNIT={}; ", quote(unit));
    command.push_str(r#"systemctl stop "$UNIT" >/dev/null 2>&1 || true; systemctl disable "$UNIT" >/dev/null 2>&1 || true; rm -f "/etc/systemd/system/$UNIT" "/lib/systemd/system/$UNIT" "/usr/lib/systemd/system/$UNIT"; systemctl daemon-reload; "#);
    if remove_dir {
        command.push_str(&format!("rm -rf {};", quote(dir)));
    } else {
        command.push_str(&format!(
            "rm -f {} {};",
            quote(&format!("{dir}/codex-tools-proxyd")),
            quote(&format!("{dir}/accounts.json"))
        ));
    }
    command
}
pub(crate) fn status_command(dir: &str, unit: &str) -> String {
    format!("DIR={}; UNIT={}; ", quote(dir), quote(unit))
        + r#"BIN="$DIR/codex-tools-proxyd"; KEYFILE="$DIR/api-proxy.key"; INSTALLED=0; SERVICE_INSTALLED=0; RUNNING=0; ENABLED=0; PID=""; API_KEY=""; if [ -x "$BIN" ]; then INSTALLED=1; fi; if command -v systemctl >/dev/null 2>&1; then if [ -f "/etc/systemd/system/$UNIT" ] || [ -f "/lib/systemd/system/$UNIT" ] || [ -f "/usr/lib/systemd/system/$UNIT" ]; then SERVICE_INSTALLED=1; fi; ENABLED_STATE=$(systemctl is-enabled "$UNIT" 2>/dev/null || true); [ "$ENABLED_STATE" = "enabled" ] && ENABLED=1; ACTIVE_STATE=$(systemctl is-active "$UNIT" 2>/dev/null || true); [ "$ACTIVE_STATE" = "active" ] && RUNNING=1; PID=$(systemctl show -p MainPID --value "$UNIT" 2>/dev/null || true); [ "$PID" = "0" ] && PID=""; fi; if [ -f "$KEYFILE" ]; then API_KEY=$(cat "$KEYFILE" 2>/dev/null || true); fi; printf 'installed=%s\nservice_installed=%s\nrunning=%s\nenabled=%s\npid=%s\napi_key=%s\n' "$INSTALLED" "$SERVICE_INSTALLED" "$RUNNING" "$ENABLED" "$PID" "$API_KEY""#
}
pub(crate) fn discover_command() -> String {
    format!("MARKER='__COPool_DISCOVERY__'; SEEN_UNITS=\"\"; for UNIT_PATH in {UNIT_DIRS}; do ")
        + r#"[ -f "$UNIT_PATH" ] || continue; UNIT_NAME=$(basename "$UNIT_PATH"); case " $SEEN_UNITS " in *" $UNIT_NAME "*) continue ;; esac; SEEN_UNITS="$SEEN_UNITS $UNIT_NAME"; WORK_DIR=$(sed -n 's/^WorkingDirectory=//p' "$UNIT_PATH" | head -n 1); SERVER_ID=$(sed -n 's/^Environment="COPOOL_SERVER_ID=//p' "$UNIT_PATH" | head -n 1 | sed 's/"$//'); LABEL=$(sed -n 's/^Environment="COPOOL_SERVER_LABEL=//p' "$UNIT_PATH" | head -n 1 | sed 's/"$//'); PORT=$(sed -n 's/^ExecStart=.* --port \([0-9][0-9]*\).*$/\1/p' "$UNIT_PATH" | head -n 1); [ -n "$WORK_DIR" ] || continue; [ -n "$PORT" ] || continue; INSTALLED=0; RUNNING=0; ENABLED=0; PID=""; API_KEY_PRESENT=0; [ -x "$WORK_DIR/codex-tools-proxyd" ] && INSTALLED=1; if command -v systemctl >/dev/null 2>&1; then ENABLED_STATE=$(systemctl is-enabled "$UNIT_NAME" 2>/dev/null || true); [ "$ENABLED_STATE" = "enabled" ] && ENABLED=1; ACTIVE_STATE=$(systemctl is-active "$UNIT_NAME" 2>/dev/null || true); [ "$ACTIVE_STATE" = "active" ] && RUNNING=1; PID=$(systemctl show -p MainPID --value "$UNIT_NAME" 2>/dev/null || true); [ "$PID" = "0" ] && PID=""; fi; [ -s "$WORK_DIR/api-proxy.key" ] && API_KEY_PRESENT=1; printf '%s\nservice_name=%s\nserver_id=%s\nlabel=%s\nremote_dir=%s\nlisten_port=%s\ninstalled=%s\nservice_installed=1\nrunning=%s\nenabled=%s\npid=%s\napi_key_present=%s\n' "$MARKER" "$UNIT_NAME" "$SERVER_ID" "$LABEL" "$WORK_DIR" "$PORT" "$INSTALLED" "$RUNNING" "$ENABLED" "$PID" "$API_KEY_PRESENT"; done"#
}
pub(crate) fn ssh_arguments(server: &RemoteServerConfig, command: &str) -> Vec<String> {
    let mut args = vec![
        "ssh".into(),
        "-p".into(),
        server.ssh_port.to_string(),
        "-o".into(),
        "ConnectTimeout=12".into(),
        "-o".into(),
        "BatchMode=yes".into(),
    ];
    if let Some(path) = &server.identity_file {
        args.extend(["-i".into(), path.clone()]);
    }
    args.extend([
        "--".into(),
        format!("{}@{}", server.ssh_user, server.host),
        command.into(),
    ]);
    args
}
pub(crate) fn with_root(command: &str) -> String {
    let command = command.trim().trim_end_matches(';').trim();
    format!(
        "if [ \"$(id -u)\" = \"0\" ]; then {command}; else sudo sh -lc {}; fi",
        quote(command)
    )
}

pub(crate) fn accounts_payload(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let loaded = crate::store::load_store_from_path(path)?;
    let mut merged: Vec<crate::models::StoredAccount> = Vec::new();
    for incoming in loaded.accounts {
        let key = crate::auth::account_identity(&incoming.auth_json).ok();
        if let Some(old) = merged
            .iter_mut()
            .find(|a| key.is_some() && crate::auth::account_identity(&a.auth_json).ok() == key)
        {
            if incoming.updated_at > old.updated_at {
                *old = incoming;
            }
        } else {
            merged.push(incoming);
        }
    }
    if !merged
        .iter()
        .any(|a| crate::auth::extract_auth(&a.auth_json).is_ok())
    {
        return Err("No usable accounts for remote deployment".into());
    }
    serde_json::to_vec_pretty(
        &json!({"version":loaded.version,"accounts":merged,"currentSelection":null}),
    )
    .map_err(|e| e.to_string())
}
