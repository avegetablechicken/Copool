//! cloudflared process lifecycle and Cloudflare named-tunnel API.
use crate::platform;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Configuration {
    pub enabled: bool,
    pub tunnel_mode: String,
    #[serde(rename = "useHTTP2", alias = "useHttp2")]
    pub use_http2: bool,
    pub named_hostname: String,
    pub account_id: String,
    pub zone_id: String,
    #[serde(skip_serializing)]
    pub api_token: String,
    /// Whether a token is kept in the Keychain; the token itself never is.
    #[serde(rename = "hasAPIToken")]
    pub has_api_token: bool,
}
impl Default for Configuration {
    fn default() -> Self {
        Self {
            enabled: false,
            tunnel_mode: "quick".into(),
            use_http2: false,
            named_hostname: String::new(),
            account_id: String::new(),
            zone_id: String::new(),
            api_token: String::new(),
            has_api_token: false,
        }
    }
}
/// One pool's public tunnel to its API proxy.
#[derive(Default)]
pub struct Tunnel {
    pool: crate::Pool,
    child: Option<tokio::process::Child>,
    log: Option<PathBuf>,
    hostname: Option<String>,
    cleanup: Option<(String, String, String)>,
    /// Named tunnels whose remote cleanup failed, tried again on each stop.
    orphans: Vec<(String, String, String)>,
    /// Why a cleanup still pending failed; cleared once none is.
    cleanup_error: Option<String>,
    last_error: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    installed: bool,
    running: bool,
    binary_path: Option<PathBuf>,
    public_url: Option<String>,
    last_error: Option<String>,
}
async fn api(
    client: &reqwest::Client,
    token: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let mut request = client
        .request(
            method,
            format!("https://api.cloudflare.com/client/v4{path}"),
        )
        .bearer_auth(token)
        .timeout(Duration::from_secs(30));
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    let result: Value = response.json().await.map_err(|e| e.to_string())?;
    if result["success"] != true {
        return Err(result["errors"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| e["message"].as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or("Cloudflare request failed".into()));
    }
    Ok(result["result"].clone())
}
fn binary(data: &Path) -> Option<PathBuf> {
    platform::executable("cloudflared").or_else(|| {
        let p = data.join("bin").join(if cfg!(windows) {
            "cloudflared.exe"
        } else {
            "cloudflared"
        });
        p.is_file().then_some(p)
    })
}
/// The public base URL of a tunnel's root, as the pool's proxy gives it.
fn public_base(pool: crate::Pool, root: &str) -> String {
    format!("{}{}", root.trim_end_matches('/'), pool.base_path())
}
impl Tunnel {
    pub fn new(pool: crate::Pool) -> Self {
        Self {
            pool,
            ..Default::default()
        }
    }
    pub fn status(&mut self, data: &Path) -> Status {
        let running = self
            .child
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)));
        if !running {
            self.child = None;
        }
        let url = if running {
            self.hostname
                .as_ref()
                .map(|h| format!("https://{h}"))
                .or_else(|| {
                    self.log
                        .as_ref()
                        .and_then(|p| std::fs::read_to_string(p).ok())
                        .and_then(|s| quick_url(&s))
                })
                .map(|root| public_base(self.pool, &root))
        } else {
            None
        };
        let path = binary(data);
        Status {
            installed: path.is_some(),
            running,
            binary_path: path,
            public_url: url,
            last_error: match (&self.last_error, &self.cleanup_error) {
                (Some(error), Some(cleanup)) => Some(format!("{error}\n{cleanup}")),
                (error, cleanup) => error.clone().or_else(|| cleanup.clone()),
            },
        }
    }
    pub async fn start(
        &mut self,
        data: &Path,
        port: u16,
        config: &Configuration,
    ) -> Result<Status, String> {
        if self.status(data).running {
            return Ok(self.status(data));
        }
        if port == 0 {
            return Err("error.cloudflared.start_api_proxy_first".into());
        }
        let bin = binary(data).ok_or("error.cloudflared.not_installed")?;
        self.stop().await?;
        let log = data.join(match self.pool {
            crate::Pool::Codex => "cloudflared.log",
            crate::Pool::Claude => "claude-cloudflared.log",
            crate::Pool::Antigravity => "antigravity-cloudflared.log",
            crate::Pool::Grok => "grok-cloudflared.log",
        });
        std::fs::create_dir_all(data).map_err(|e| e.to_string())?;
        let file = std::fs::File::create(&log).map_err(|e| e.to_string())?;
        let mut cmd = tokio::process::Command::new(bin);
        cmd.args(["tunnel", "--no-autoupdate", "--loglevel", "info"]);
        if config.use_http2 {
            cmd.args(["--protocol", "http2"]);
        }
        let local = format!("http://127.0.0.1:{port}");
        if config.tunnel_mode == "named" {
            let hostname = config.named_hostname.trim().trim_end_matches('.');
            if hostname.is_empty()
                || !hostname.contains('.')
                || !hostname
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c))
            {
                return Err("Invalid tunnel hostname".into());
            }
            for id in [&config.account_id, &config.zone_id] {
                if id.len() != 32 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
                    return Err("Invalid Cloudflare account or zone ID".into());
                }
            }
            // The caller supplies the saved token (`Application::with_tunnel_token`).
            if config.api_token.is_empty() {
                return Err("Cloudflare API token is missing".into());
            }
            let token = config.api_token.clone();
            let client = reqwest::Client::new();
            let created=api(&client,&token,reqwest::Method::POST,&format!("/accounts/{}/cfd_tunnel",config.account_id),Some(json!({"name":format!("copool-{}",uuid::Uuid::new_v4()),"config_src":"cloudflare"}))).await?;
            let id = created["id"]
                .as_str()
                .ok_or("Tunnel ID missing")?
                .to_string();
            self.cleanup = Some((token.clone(), config.account_id.clone(), id.clone()));
            let setup=async {
                api(&client,&token,reqwest::Method::PUT,&format!("/accounts/{}/cfd_tunnel/{id}/configurations",config.account_id),Some(json!({"config":{"ingress":[{"hostname":hostname,"service":local},{"service":"http_status:404"}]}}))).await?;
                let records=api(&client,&token,reqwest::Method::GET,&format!("/zones/{}/dns_records?type=CNAME&name={hostname}",config.zone_id),None).await?;
                let record_id=records.as_array().and_then(|r|r.first()).and_then(|r|r["id"].as_str());
                let (method,path)=if let Some(record)=record_id{(reqwest::Method::PUT,format!("/zones/{}/dns_records/{record}",config.zone_id))}else{(reqwest::Method::POST,format!("/zones/{}/dns_records",config.zone_id))};
                api(&client,&token,method,&path,Some(json!({"type":"CNAME","name":hostname,"content":format!("{id}.cfargotunnel.com"),"proxied":true}))).await?;
                let run_token=if let Some(token)=created["token"].as_str(){token.to_owned()}else{api(&client,&token,reqwest::Method::GET,&format!("/accounts/{}/cfd_tunnel/{id}/token",config.account_id),None).await?.as_str().ok_or("Tunnel token missing")?.into()};
                Ok::<_,String>(run_token)
            }.await;
            match setup {
                Ok(token) => {
                    cmd.arg("run").env("TUNNEL_TOKEN", token);
                    self.hostname = Some(hostname.into());
                }
                Err(error) => {
                    let _ = self.stop().await;
                    return Err(error);
                }
            }
        } else if config.tunnel_mode == "quick" {
            if let Some(home) = dirs::home_dir() {
                if ["config.yml", "config.yaml"]
                    .iter()
                    .any(|name| home.join(".cloudflared").join(name).exists())
                {
                    return Err(
                        "Quick tunnel conflicts with ~/.cloudflared/config.yml or config.yaml"
                            .into(),
                    );
                }
            }
            cmd.args(["--url", &local]);
            self.hostname = None;
        } else {
            return Err("Invalid tunnel mode".into());
        }
        cmd.kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(file.try_clone().map_err(|e| e.to_string())?)
            .stderr(file);
        match cmd.spawn() {
            Ok(child) => self.child = Some(child),
            Err(error) => {
                let _ = self.stop().await;
                return Err(error.to_string());
            }
        }
        self.log = Some(log);
        self.last_error = None;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let status = self.status(data);
            if !status.running {
                let error = self
                    .log
                    .as_ref()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .unwrap_or("cloudflared exited early".into());
                self.last_error = Some(error.clone());
                let _ = self.stop().await;
                return Err(error);
            }
            if status.public_url.is_some() {
                return Ok(status);
            }
        }
        Ok(self.status(data))
    }
    pub async fn stop(&mut self) -> Result<(), String> {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        self.hostname = None;
        self.orphans.extend(self.cleanup.take());
        if self.orphans.is_empty() {
            return Ok(());
        }
        let client = reqwest::Client::new();
        let mut failed = vec![];
        for (token, account, id) in std::mem::take(&mut self.orphans) {
            // A killed cloudflared leaves its connections registered, and
            // Cloudflare refuses to delete a tunnel that still has any.
            let path = format!("/accounts/{account}/cfd_tunnel/{id}");
            let _ = api(
                &client,
                &token,
                reqwest::Method::DELETE,
                &format!("{path}/connections"),
                None,
            )
            .await;
            // The local tunnel is already gone; a failed remote cleanup must not
            // keep the proxy from stopping or the next tunnel from starting.
            match api(&client, &token, reqwest::Method::DELETE, &path, None).await {
                Ok(_) => {}
                Err(error) => failed.push((error, (token, account, id))),
            }
        }
        self.cleanup_error = failed
            .last()
            .map(|(error, _)| format!("Cloudflare tunnel cleanup failed: {error}"));
        self.orphans = failed.into_iter().map(|(_, orphan)| orphan).collect();
        Ok(())
    }
}
pub fn quick_url(log: &str) -> Option<String> {
    log.split_whitespace()
        .filter_map(|word| {
            let start = word.find("https://")?;
            let value = word[start..]
                .trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '/' && c != '.');
            let url = reqwest::Url::parse(value).ok()?;
            url.host_str()
                .filter(|h| h.ends_with(".trycloudflare.com") && *h != "trycloudflare.com")?;
            Some(url.as_str().trim_end_matches('/').to_owned())
        })
        .next()
}
/// Held while cloudflared is installed, which every pool's tunnel shares.
static INSTALLING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
pub async fn install(data: &Path) -> Result<(), String> {
    // Another pool may be installing it: one install, which this waits for.
    let _installing = INSTALLING.lock().await;
    if binary(data).is_some() {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        let brew =
            platform::executable("brew").ok_or("Homebrew is required to install cloudflared")?;
        platform::checked(&brew.to_string_lossy(), &["install", "cloudflared"], 300).await?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        use sha2::Digest;
        let os = if cfg!(windows) { "windows" } else { "linux" };
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "amd64"
        };
        let name = format!(
            "cloudflared-{os}-{arch}{}",
            if cfg!(windows) { ".exe" } else { "" }
        );
        let client = reqwest::Client::builder()
            .user_agent("Copool")
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|e| e.to_string())?;
        let release: Value = client
            .get("https://api.github.com/repos/cloudflare/cloudflared/releases/latest")
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        let asset = release["assets"]
            .as_array()
            .and_then(|a| a.iter().find(|a| a["name"] == name))
            .ok_or("cloudflared asset missing")?;
        let digest = asset["digest"]
            .as_str()
            .and_then(|s| s.strip_prefix("sha256:"))
            .ok_or("Release asset SHA256 is missing")?;
        let url = asset["browser_download_url"]
            .as_str()
            .filter(|s| {
                s.starts_with("https://github.com/cloudflare/cloudflared/releases/download/")
            })
            .ok_or("Invalid release asset URL")?;
        let bytes = client
            .get(url)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .bytes()
            .await
            .map_err(|e| e.to_string())?;
        if format!("{:x}", sha2::Sha256::digest(&bytes)) != digest {
            return Err("cloudflared checksum mismatch".into());
        }
        let folder = data.join("bin");
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let path = folder.join(if cfg!(windows) {
            "cloudflared.exe"
        } else {
            "cloudflared"
        });
        put_binary(&path, &bytes)
    }
}
/// Writes an executable beside its path and moves it there whole: a tunnel
/// started meanwhile finds none, never part of one.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn put_binary(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut partial = path.as_os_str().to_owned();
    partial.push(".download");
    let partial = PathBuf::from(partial);
    let written = std::fs::write(&partial, bytes).and_then(|()| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o700))?;
        }
        std::fs::rename(&partial, path)
    });
    if written.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    written.map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_installed_binary_is_moved_into_place_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cloudflared");
        put_binary(&path, b"binary").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["cloudflared"]);
        // A failed write leaves nothing behind.
        let missing = dir.path().join("missing/cloudflared");
        assert!(put_binary(&missing, b"binary").is_err());
        assert!(!dir.path().join("missing").exists());
    }
    #[test]
    fn each_pool_is_given_its_proxy_base_url() {
        let log = "INF |  https://quiet-river.trycloudflare.com  |";
        let root = quick_url(log).unwrap();
        assert_eq!(root, "https://quiet-river.trycloudflare.com");
        assert_eq!(
            public_base(crate::Pool::Codex, &root),
            "https://quiet-river.trycloudflare.com/v1"
        );
        assert_eq!(
            public_base(crate::Pool::Claude, &root),
            "https://quiet-river.trycloudflare.com/anthropic"
        );
    }
}
