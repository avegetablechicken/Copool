//! Source/binary discovery and local preparation; no implicit download or install.
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
const CURRENT: &str = "resources/proxyd-src/proxyd/Cargo.toml";
pub fn manifest(root: &Path) -> Option<PathBuf> {
    [CURRENT, "src-tauri/proxyd/Cargo.toml"]
        .iter()
        .map(|p| root.join(p))
        .find(|p| p.is_file())
}
pub fn find_root(start: &Path) -> Option<PathBuf> {
    let start = if start.is_file() {
        start.parent()?
    } else {
        start
    };
    for dir in start.ancestors().take(12) {
        if manifest(dir).is_some() {
            return Some(dir.into());
        }
    }
    std::env::current_dir()
        .ok()?
        .ancestors()
        .take(8)
        .find(|p| manifest(p).is_some())
        .map(Path::to_path_buf)
}
pub fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}
pub fn prepare_binary(path: &Path, force: bool) -> Result<bool, String> {
    if !executable(path) {
        return Ok(false);
    }
    if force {
        fs::remove_file(path).map_err(|e| e.to_string())?;
        Ok(false)
    } else {
        Ok(true)
    }
}
pub fn prebuilt(
    repo: Option<&Path>,
    resources: Option<&Path>,
    cache: &Path,
    target: &str,
) -> Result<Option<PathBuf>, String> {
    if target.is_empty()
        || !target
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("Invalid Rust target".into());
    }
    if let Some(repo) = repo {
        let path = repo
            .join("resources/proxyd-prebuilt")
            .join(target)
            .join("codex-tools-proxyd");
        if path.is_file() {
            return Ok(Some(path));
        }
    }
    let Some(resources) = resources else {
        return Ok(None);
    };
    let archive = resources
        .join("proxyd-prebuilt-archives")
        .join(target)
        .join("codex-tools-proxyd.zlib");
    if !archive.is_file() {
        return Ok(None);
    }
    let compressed = fs::read(&archive).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    let max = 128 * 1024 * 1024;
    if flate2::read::ZlibDecoder::new(&compressed[..])
        .take(max + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        bytes.clear();
        flate2::read::DeflateDecoder::new(&compressed[..])
            .take(max + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
    }
    if bytes.is_empty() || bytes.len() > max as usize {
        return Err("Invalid or oversized proxyd archive".into());
    }
    let path = cache
        .join("proxyd-prebuilt")
        .join(target)
        .join("codex-tools-proxyd");
    let parent = path.parent().ok_or("Invalid cache path")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    file.persist(&path).map_err(|e| e.to_string())?;
    Ok(Some(path))
}
pub fn bundled_manifest(resources: &Path, cache: &Path) -> Result<Option<PathBuf>, String> {
    let structured = resources.join("proxyd-src/proxyd/Cargo.toml");
    if structured.is_file() {
        return Ok(Some(structured));
    }
    if !resources.join("Cargo.toml").is_file() || !resources.join("main.rs").is_file() {
        return Ok(None);
    }
    let root = cache.join("proxyd-src");
    let crate_root = root.join("proxyd");
    fs::create_dir_all(crate_root.join("src")).map_err(|e| e.to_string())?;
    fs::create_dir_all(root.join("src")).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(resources).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name();
        let dest = if name == "Cargo.toml" || name == "Cargo.lock" {
            crate_root.join(&name)
        } else if name == "main.rs" {
            crate_root.join("src/main.rs")
        } else if path.extension().is_some_and(|v| v == "rs") {
            root.join("src").join(&name)
        } else {
            continue;
        };
        fs::copy(path, dest).map_err(|e| e.to_string())?;
    }
    Ok(Some(crate_root.join("Cargo.toml")))
}
