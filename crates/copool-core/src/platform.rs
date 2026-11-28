//! Explicit desktop integrations. Never run by core tests or during account reads.
use crate::{pool_login::OAuthCredential, process, Pool, Preferences, SwitchExecution};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

mod agents;
#[cfg(test)]
pub(crate) use agents::sync_opencode_grok;
pub use agents::{sync_agents, SyncAgent, SyncAgentInfo};

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
/// the switch itself has succeeded whatever fails here. `credential` is
/// what the login switched to grants, for the agents to sync; none for a
/// provider's card, and an error, which each of them reports, for a login
/// that grants none.
pub(crate) async fn after_switch(
    pool: Pool,
    p: &Preferences,
    credential: Option<Result<OAuthCredential, String>>,
) -> Result<SwitchExecution, String> {
    let mut result = SwitchExecution::default();
    if let Some(credential) = credential {
        result.agent_sync = agents::sync(pool, p, credential).await;
    }
    if pool == Pool::Codex && p.restart_editors_on_switch {
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
    if pool == Pool::Codex && p.launch_codex_after_switch {
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
