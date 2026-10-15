//! Credentials are kept out of serialized provider settings.
pub trait SecretStore: Send + Sync {
    fn get(&self, id: &str) -> Result<Option<String>, String>;
    fn set(&self, id: &str, password: &str) -> Result<(), String>;
    fn remove(&self, id: &str) -> Result<(), String>;
}
#[cfg(not(target_os = "macos"))]
const SERVICE: &str = "io.github.copool.rust.sub2api";
#[cfg(not(target_os = "macos"))]
fn with_entry<T: Send + 'static>(
    id: &str,
    call: impl FnOnce(keyring::Entry) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let entry = |id: &str| keyring::Entry::new(SERVICE, id).map_err(|e| e.to_string());
    // keyring's Linux backend drives D-Bus with its own `block_on`, which
    // panics on a Tokio thread ("Cannot start a runtime from within a
    // runtime"), and the callers are async commands: run it on a plain thread.
    #[cfg(target_os = "linux")]
    {
        let id = id.to_owned();
        std::thread::spawn(move || call(entry(&id)?))
            .join()
            .map_err(|_| "The system keyring failed".to_string())?
    }
    #[cfg(not(target_os = "linux"))]
    call(entry(id)?)
}
/// The system keyring, one entry per secret; on macOS the secrets are in
/// Copool's own Keychain item (`crate::vault`).
#[cfg(not(target_os = "macos"))]
pub struct SystemSecrets;
#[cfg(not(target_os = "macos"))]
impl SecretStore for SystemSecrets {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        with_entry(id, |entry| match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        })
    }
    fn set(&self, id: &str, password: &str) -> Result<(), String> {
        let password = password.to_owned();
        with_entry(id, move |entry| {
            entry.set_password(&password).map_err(|e| e.to_string())
        })
    }
    fn remove(&self, id: &str) -> Result<(), String> {
        with_entry(id, |entry| match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        })
    }
}
