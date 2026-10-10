//! Credentials are kept out of serialized provider settings.
pub trait SecretStore: Send + Sync {
    fn get(&self, id: &str) -> Result<Option<String>, String>;
    fn set(&self, id: &str, password: &str) -> Result<(), String>;
    fn remove(&self, id: &str) -> Result<(), String>;
}
pub struct SystemSecrets;
impl SecretStore for SystemSecrets {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        match keyring::Entry::new("io.github.copool.rust.sub2api", id)
            .map_err(|e| e.to_string())?
            .get_password()
        {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn set(&self, id: &str, password: &str) -> Result<(), String> {
        keyring::Entry::new("io.github.copool.rust.sub2api", id)
            .map_err(|e| e.to_string())?
            .set_password(password)
            .map_err(|e| e.to_string())
    }
    fn remove(&self, id: &str) -> Result<(), String> {
        match keyring::Entry::new("io.github.copool.rust.sub2api", id)
            .map_err(|e| e.to_string())?
            .delete_credential()
        {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}
