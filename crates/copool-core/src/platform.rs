//! Explicit desktop integrations. Never run by core tests or during account reads.
use crate::{
    auth, persistence::write_json, process, utils::now_unix_seconds, Preferences, SwitchExecution,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub fn executable(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    #[cfg(target_os = "macos")]
    dirs.extend(["/opt/homebrew/bin".into(), "/usr/local/bin".into()]);
    if let Some(home) = dirs::home_dir() {
        dirs.extend([home.join(".local/bin"), home.join(".cargo/bin")]);
    }
    dirs.into_iter()
        .map(|p| {
            p.join(if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.into()
            })
        })
        .find(|p| p.is_file())
}
pub async fn checked(program: &str, args: &[&str], timeout: u64) -> Result<String, String> {
    let output = process::run(program, args, Duration::from_secs(timeout))
        .await
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
pub fn open_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Unsupported URL".into());
    }
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("/usr/bin/open");
    #[cfg(target_os = "linux")]
    let mut cmd = std::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    cmd.arg(url).spawn().map(|_| ()).map_err(|e| e.to_string())
}
pub fn sync_opencode(auth_json: &Value, targets: &[PathBuf]) -> Result<(), String> {
    let tokens = auth_json.get("tokens").unwrap_or(auth_json);
    let access = tokens["access_token"]
        .as_str()
        .ok_or("Missing access token")?;
    let refresh = tokens["refresh_token"]
        .as_str()
        .ok_or("Missing refresh token")?;
    let expires = tokens["id_token"]
        .as_str()
        .and_then(|s| auth::decode_jwt_payload(s).ok())
        .and_then(|v| v["exp"].as_i64())
        .map(|v| v * 1000)
        .unwrap_or(now_unix_seconds() * 1000 + 55 * 60 * 1000);
    let mut failures = vec![];
    for path in targets {
        let result = (|| {
            let mut root: Value = if path.exists() {
                serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?
            } else {
                json!({})
            };
            if !root.is_object() {
                return Err("Invalid OpenCode auth document".into());
            }
            if !root["openai"].is_object() {
                root["openai"] = json!({});
            }
            let item = root["openai"].as_object_mut().unwrap();
            item.entry("type").or_insert(json!("oauth"));
            item.insert("access".into(), access.into());
            item.insert("refresh".into(), refresh.into());
            item.insert("expires".into(), expires.into());
            if let Some(id) = tokens["account_id"].as_str() {
                item.insert("accountId".into(), id.into());
            }
            write_json(path, &root)
        })();
        if let Err(e) = result {
            failures.push(e);
        }
    }
    if targets.is_empty() || failures.len() == targets.len() {
        Err(failures.join(" | "))
    } else {
        Ok(())
    }
}
pub fn opencode_paths() -> Vec<PathBuf> {
    if let Some(p) = std::env::var_os("OPENCODE_AUTH_PATH").filter(|p| !p.is_empty()) {
        return vec![p.into()];
    }
    let mut paths = vec![];
    for (env, suffix) in [
        ("OPENCODE_CONFIG_HOME", "auth.json"),
        ("XDG_CONFIG_HOME", "opencode/auth.json"),
    ] {
        if let Some(p) = std::env::var_os(env).filter(|p| !p.is_empty()) {
            paths.push(PathBuf::from(p).join(suffix));
        }
    }
    if let Some(home) = dirs::home_dir() {
        paths.extend([
            home.join(".config/opencode/auth.json"),
            home.join("Library/Application Support/opencode/auth.json"),
        ]);
    }
    if let Some(p) = std::env::var_os("XDG_DATA_HOME").filter(|p| !p.is_empty()) {
        paths.push(PathBuf::from(p).join("opencode/auth.json"));
    }
    if let Some(home) = dirs::home_dir() {
        paths.extend([
            home.join(".local/share/opencode/auth.json"),
            home.join(".opencode/auth.json"),
        ]);
    }
    let existing: Vec<_> = paths.iter().filter(|p| p.exists()).cloned().collect();
    if existing.is_empty() {
        paths.into_iter().take(1).collect()
    } else {
        existing
    }
}
#[derive(Clone, Serialize)]
pub struct Editor {
    pub id: String,
    pub label: String,
    pub path: PathBuf,
    pub process: String,
}
pub fn editors() -> Vec<Editor> {
    [
        ("vscode", "VS Code", "Visual Studio Code", "code", "Code"),
        (
            "vscodeInsiders",
            "VS Code Insiders",
            "Visual Studio Code - Insiders",
            "code-insiders",
            "Code - Insiders",
        ),
        ("cursor", "Cursor", "Cursor", "cursor", "Cursor"),
        (
            "antigravity",
            "Antigravity",
            "Antigravity",
            "antigravity",
            "Antigravity",
        ),
        ("kiro", "Kiro", "Kiro", "kiro", "Kiro"),
        ("trae", "Trae", "Trae", "trae", "Trae"),
        ("qoder", "Qoder", "Qoder", "qoder", "Qoder"),
    ]
    .into_iter()
    .filter_map(|(id, label, bundle, bin, process)| {
        let path = if cfg!(target_os = "macos") {
            [
                PathBuf::from("/Applications"),
                dirs::home_dir()?.join("Applications"),
            ]
            .into_iter()
            .map(|p| p.join(format!("{bundle}.app")))
            .find(|p| p.exists())
        } else {
            executable(bin)
        }?;
        Some(Editor {
            id: id.into(),
            label: label.into(),
            path,
            process: if cfg!(target_os = "linux") {
                bin
            } else {
                process
            }
            .into(),
        })
    })
    .collect()
}
/// Side effects of a switch, reported as Swift's SwitchAccountExecutionResult:
/// the switch itself has succeeded whatever fails here.
pub async fn after_switch(
    p: &Preferences,
    auth: Option<&Value>,
) -> Result<SwitchExecution, String> {
    let mut result = SwitchExecution::default();
    if p.sync_opencode_openai_auth {
        if let Some(auth) = auth {
            match sync_opencode(auth, &opencode_paths()) {
                Ok(()) => result.opencode_synced = true,
                Err(e) => result.opencode_sync_error = Some(e),
            }
        }
    }
    if p.restart_editors_on_switch {
        let installed = editors();
        let mut errors = vec![];
        for id in &p.restart_editor_targets {
            let Some(editor) = installed.iter().find(|e| &e.id == id) else {
                errors.push(format!("Editor not installed: {id}"));
                continue;
            };
            #[cfg(unix)]
            {
                let _ = checked("pkill", &["-TERM", "-x", &editor.process], 5).await;
            }
            #[cfg(windows)]
            {
                let _ = checked("taskkill", &["/IM", &format!("{}.exe", editor.process)], 5).await;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            #[cfg(target_os = "macos")]
            let restarted = checked(
                "/usr/bin/open",
                &["-na", &editor.path.to_string_lossy()],
                15,
            )
            .await
            .map(|_| ());
            #[cfg(not(target_os = "macos"))]
            let restarted = std::process::Command::new(&editor.path)
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string());
            match restarted {
                Ok(()) => result.restarted_editors.push(editor.label.clone()),
                Err(e) => errors.push(e),
            }
        }
        if !errors.is_empty() {
            result.editor_restart_error = Some(errors.join(" | "));
        }
    }
    if p.launch_codex_after_switch {
        result.launch_error = launch_codex().await.err();
    }
    Ok(result)
}
async fn launch_codex() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join("Applications"));
        }
        let app = roots
            .into_iter()
            .flat_map(|p| [p.join("Codex.app"), p.join("Codex Desktop.app")])
            .find(|p| p.is_dir());
        if let Some(app) = app {
            return checked("/usr/bin/open", &["-a", &app.to_string_lossy()], 15)
                .await
                .map(|_| ());
        }
    }
    #[cfg(target_os = "windows")]
    if let Some(root) = dirs::data_local_dir() {
        for candidate in [
            root.join("Programs/Codex/Codex.exe"),
            root.join("Programs/Codex Desktop/Codex.exe"),
        ] {
            if candidate.is_file() {
                return std::process::Command::new(candidate)
                    .spawn()
                    .map(|_| ())
                    .map_err(|e| e.to_string());
            }
        }
    }
    let cli = executable("codex").ok_or("Codex is not installed or could not be found")?;
    checked(&cli.to_string_lossy(), &["app"], 30)
        .await
        .map(|_| ())
}
pub fn startup(enabled: bool, executable: &Path) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Home directory unavailable")?;
    #[cfg(target_os = "macos")]
    {
        let path = home.join("Library/LaunchAgents/io.github.copool.rust.plist");
        if !enabled {
            if path.exists() {
                std::fs::remove_file(path).map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        let escaped = executable
            .to_string_lossy()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let content=format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>io.github.copool.rust</string><key>ProgramArguments</key><array><string>{escaped}</string></array><key>RunAtLoad</key><true/></dict></plist>");
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(path, content).map_err(|e| e.to_string())
    }
    #[cfg(target_os = "linux")]
    {
        let path = dirs::config_dir()
            .unwrap_or(home.join(".config"))
            .join("autostart/copool.desktop");
        if !enabled {
            if path.exists() {
                std::fs::remove_file(path).map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        let escaped = executable
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$");
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(path,format!("[Desktop Entry]\nType=Application\nName=Copool\nExec=\"{escaped}\"\nTerminal=false\n")).map_err(|e|e.to_string())
    }
    #[cfg(target_os = "windows")]
    {
        let _ = home;
        let mut command = std::process::Command::new("reg");
        let key = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run";
        if enabled {
            command.args([
                "add",
                key,
                "/v",
                "Copool",
                "/t",
                "REG_SZ",
                "/d",
                &format!("\"{}\"", executable.display()),
                "/f",
            ]);
        } else {
            command.args(["delete", key, "/v", "Copool", "/f"]);
        }
        let output = command.output().map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).into_owned())
        }
    }
}
