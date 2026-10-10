//! SSH transport for the same deployment plans used by the original app.
use crate::{
    application::Application,
    credentials::{SecretStore, SystemSecrets},
    deployment,
    models::{RemoteAuthMode, RemoteServerConfig},
    remote,
};
use serde_json::{json, Value};
use std::{io::Write, path::PathBuf, time::Duration};
use tokio::io::AsyncWriteExt;

fn validate(s: &RemoteServerConfig) -> Result<(), String> {
    if s.host.is_empty()
        || s.host.starts_with('-')
        || s.host
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "/@;|&`$\\\"'".contains(c))
    {
        return Err("Invalid SSH host".into());
    }
    if s.ssh_user.is_empty()
        || !s
            .ssh_user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
        || s.ssh_port == 0
        || s.listen_port == 0
    {
        return Err("Invalid SSH user or port".into());
    }
    if !s.remote_dir.starts_with('/')
        || s.remote_dir == "/"
        || s.remote_dir.contains('\n')
        || s.remote_dir.contains('\r')
        || s.remote_dir.split('/').any(|p| p == "..")
    {
        return Err("Invalid remote installation directory".into());
    }
    Ok(())
}
async fn ssh(
    s: &RemoteServerConfig,
    command: &str,
    input: Option<Vec<u8>>,
    timeout: u64,
) -> Result<String, String> {
    validate(s)?;
    let mut server = s.clone();
    let mut key = None;
    if matches!(server.auth_mode, RemoteAuthMode::KeyContent) {
        let content = server
            .private_key
            .take()
            .filter(|s| !s.is_empty())
            .or(SystemSecrets.get(&format!("ssh-key:{}", server.id))?)
            .ok_or("Private key is empty")?;
        let mut file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
        file.write_all(content.as_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| e.to_string())?;
        server.identity_file = Some(file.path().to_string_lossy().into());
        key = Some(file);
    }
    let mut args = remote::ssh_arguments(&server, command);
    let mut process = if matches!(server.auth_mode, RemoteAuthMode::Password) {
        let password = server
            .password
            .take()
            .filter(|s| !s.is_empty())
            .or(SystemSecrets.get(&format!("ssh-password:{}", server.id))?)
            .ok_or("SSH password is empty")?;
        args.retain(|s| s != "BatchMode=yes");
        // Remove the now-orphaned -o before -- or -i.
        if let Some(i) = args
            .windows(2)
            .position(|w| w[0] == "-o" && (w[1] == "--" || w[1] == "-i"))
        {
            args.remove(i);
        }
        let mut cmd = tokio::process::Command::new(
            crate::platform::executable("sshpass")
                .ok_or("Install sshpass to use SSH password authentication")?,
        );
        cmd.arg("-e").env("SSHPASS", password);
        cmd
    } else {
        args.remove(0);
        tokio::process::Command::new("ssh")
    };
    process
        .args(args)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = process.spawn().map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().unwrap();
    let operation = async move {
        if let Some(bytes) = input {
            stdin.write_all(&bytes).await.map_err(|e| e.to_string())?;
        }
        drop(stdin);
        let result = child.wait_with_output().await.map_err(|e| e.to_string())?;
        if !result.status.success() {
            return Err(String::from_utf8_lossy(&result.stderr).trim().into());
        }
        Ok(String::from_utf8_lossy(&result.stdout).into_owned())
    };
    let result = tokio::time::timeout(Duration::from_secs(timeout), operation)
        .await
        .map_err(|_| "SSH operation timed out".to_string())?;
    drop(key);
    result
}
fn parse(output: &str) -> Value {
    let mut values = serde_json::Map::new();
    for line in output.lines() {
        if let Some((k, v)) = line.split_once('=') {
            values.insert(k.trim().into(), v.trim().into());
        }
    }
    values.into()
}
async fn status(server: &RemoteServerConfig, unit: &str) -> Result<Value, String> {
    let raw = parse(
        &ssh(
            server,
            &remote::with_root(&remote::status_command(&server.remote_dir, unit)),
            None,
            15,
        )
        .await?,
    );
    Ok(
        json!({"installed":raw["installed"]=="1","serviceInstalled":raw["service_installed"]=="1","running":raw["running"]=="1","enabled":raw["enabled"]=="1","pid":raw["pid"].as_str().and_then(|s|s.parse::<u32>().ok()),"apiKey":raw["api_key"].as_str().filter(|s|!s.is_empty()),"serviceName":unit,"baseURL":format!("http://{}:{}/v1",server.host,server.listen_port),"lastError":null}),
    )
}
impl Application {
    pub async fn save_remote(
        &self,
        value: Value,
        previous_id: Option<&str>,
    ) -> Result<Value, String> {
        let _op = self.operations.lock().await;
        let mut settings = self.configuration()?;
        let mut server = remote::normalize(value, || uuid::Uuid::new_v4().to_string())?;
        if let Some(password) = server.password.take().filter(|v| !v.is_empty()) {
            self.secrets
                .set(&format!("ssh-password:{}", server.id), &password)?;
        }
        if let Some(key) = server.private_key.take().filter(|v| !v.is_empty()) {
            self.secrets.set(&format!("ssh-key:{}", server.id), &key)?;
        }
        if let Some(old) = previous_id.filter(|id| *id != server.id) {
            for prefix in ["ssh-password", "ssh-key"] {
                if (prefix == "ssh-password"
                    && matches!(server.auth_mode, RemoteAuthMode::Password))
                    || (prefix == "ssh-key"
                        && matches!(server.auth_mode, RemoteAuthMode::KeyContent))
                {
                    if let Some(secret) = self.secrets.get(&format!("{prefix}:{old}"))? {
                        self.secrets
                            .set(&format!("{prefix}:{}", server.id), &secret)?;
                    }
                }
            }
            settings
                .remote_servers
                .retain(|v| v["id"].as_str() != Some(old));
        }
        let value = serde_json::to_value(server).map_err(|e| e.to_string())?;
        if let Some(old) = settings
            .remote_servers
            .iter_mut()
            .find(|v| v["id"] == value["id"])
        {
            *old = value.clone();
        } else {
            settings.remote_servers.push(value.clone());
        }
        self.persist_configuration(&settings)?;
        Ok(value)
    }
    pub async fn remove_remote(&self, id: &str) -> Result<(), String> {
        let _op = self.operations.lock().await;
        let mut settings = self.configuration()?;
        settings
            .remote_servers
            .retain(|v| v["id"].as_str() != Some(id));
        self.persist_configuration(&settings)?;
        self.secrets.remove(&format!("ssh-password:{id}"))?;
        self.secrets.remove(&format!("ssh-key:{id}"))
    }
    pub async fn remote_action(
        &self,
        id: &str,
        action: &str,
        resources: Option<PathBuf>,
    ) -> Result<Value, String> {
        let server: RemoteServerConfig = serde_json::from_value(
            self.configuration()?
                .remote_servers
                .into_iter()
                .find(|v| v["id"].as_str() == Some(id))
                .ok_or("Server not found")?,
        )
        .map_err(|e| e.to_string())?;
        validate(&server)?;
        let unit = remote::service_name(id);
        match action {
            "status" => status(&server, &unit).await,
            "logs" => Ok(
                json!({"logs":ssh(&server,&remote::with_root(&format!("journalctl -u {} -n 200 --no-pager",remote::quote(&unit))),None,20).await?}),
            ),
            "discover" => {
                let out = ssh(
                    &server,
                    &remote::with_root(&remote::discover_command()),
                    None,
                    20,
                )
                .await?;
                Ok(Value::Array(
                    out.split("__COPool_DISCOVERY__")
                        .filter(|s| s.contains("service_name="))
                        .map(parse)
                        .collect(),
                ))
            }
            "start" | "stop" | "restart" => {
                ssh(
                    &server,
                    &remote::with_root(&format!("systemctl {action} {}", remote::quote(&unit))),
                    None,
                    60,
                )
                .await?;
                status(&server, &unit).await
            }
            "uninstall" => {
                ssh(
                    &server,
                    &remote::with_root(&remote::uninstall_command(
                        &server.remote_dir,
                        &unit,
                        false,
                    )),
                    None,
                    60,
                )
                .await?;
                status(&server, &unit).await
            }
            "deploy" | "sync" => {
                let stage = format!("/tmp/copool-stage-{}", uuid::Uuid::new_v4());
                ssh(
                    &server,
                    &format!("umask 077; mkdir {}", remote::quote(&stage)),
                    None,
                    15,
                )
                .await?;
                let operation = async {
                    let payload = remote::accounts_payload(&self.paths.store())?;
                    ssh(
                        &server,
                        &format!("cat > {}", remote::quote(&format!("{stage}/accounts.json"))),
                        Some(payload),
                        90,
                    )
                    .await?;
                    if action == "deploy" {
                        let arch = ssh(&server, "uname -m", None, 15).await?;
                        let target = match arch.trim() {
                            "x86_64" => "x86_64-unknown-linux-musl",
                            "aarch64" | "arm64" => "aarch64-unknown-linux-musl",
                            _ => return Err("Unsupported remote CPU architecture".into()),
                        };
                        let root = std::env::current_exe()
                            .ok()
                            .and_then(|p| deployment::find_root(&p));
                        let binary = deployment::prebuilt(
                            root.as_deref(),
                            resources.as_deref(),
                            &self.paths.data_dir,
                            target,
                        )?
                        .ok_or("Bundled Linux daemon is missing")?;
                        ssh(
                            &server,
                            &format!(
                                "cat > {}",
                                remote::quote(&format!("{stage}/codex-tools-proxyd"))
                            ),
                            Some(std::fs::read(binary).map_err(|e| e.to_string())?),
                            90,
                        )
                        .await?;
                        ssh(
                            &server,
                            &format!("cat > {}", remote::quote(&format!("{stage}/{unit}"))),
                            Some(remote::systemd_unit(&server, &unit).into_bytes()),
                            30,
                        )
                        .await?;
                        ssh(
                            &server,
                            &remote::with_root(&remote::install_command(&server, &unit, &stage)),
                            None,
                            90,
                        )
                        .await?;
                    } else {
                        ssh(
                            &server,
                            &remote::with_root(&remote::sync_command(
                                &server.remote_dir,
                                &unit,
                                &stage,
                                false,
                            )),
                            None,
                            60,
                        )
                        .await?;
                    }
                    status(&server, &unit).await
                }
                .await;
                let _ = ssh(
                    &server,
                    &format!("rm -rf -- {}", remote::quote(&stage)),
                    None,
                    10,
                )
                .await;
                operation
            }
            _ => Err("Unknown remote operation".into()),
        }
    }
}
