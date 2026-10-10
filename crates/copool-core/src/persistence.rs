use serde::Serialize;
use std::{fs, io::Write, path::Path};

/// Same-directory, restrictive temporary file followed by atomic replacement,
/// including Windows (NamedTempFile::persist uses the platform replace primitive).
pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path.parent().ok_or("Invalid file path")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replaces_existing_file_without_leaving_temporary_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        write_json(&path, &serde_json::json!({"token":"first"})).unwrap();
        write_json(&path, &serde_json::json!({"token":"second"})).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("second"));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
