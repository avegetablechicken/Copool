//! The login Antigravity uses: one item of the system's credential store,
//! service `gemini` and account `antigravity`, as `agy` keeps it through
//! go-keyring. It holds an OAuth token as JSON (`{"token": {"access_token",
//! "refresh_token", "expiry", …}}`), on macOS wrapped as
//! `go-keyring-base64:`, and on macOS reached only through `/usr/bin/security`
//! (see `super::security`). A switch writes another account's document there,
//! in the same form; a renewal of the login it holds updates its tokens in
//! place, and nothing else. Over SSH agy uses `TOKEN_FILE` beside its
//! settings instead (on macOS only while the Keychain is locked): the pool
//! writes it alike and reads whichever of the two holds the later login. The document itself is read and written by
//! `crate::pool_login::antigravity`.
use crate::vault::KeychainAccess;

type Result<T> = std::result::Result<T, String>;

pub(crate) const SERVICE: &str = "gemini";
pub(crate) const ACCOUNT: &str = "antigravity";
/// agy's login file beside its settings, which it uses instead of the item
/// over SSH: the same document, unwrapped.
pub(crate) const TOKEN_FILE: &str = "antigravity-oauth-token";
/// The item, as errors name it.
pub(super) const ITEM: &str = "the Antigravity login (Keychain item gemini/antigravity)";

/// Where Antigravity's login item is kept; tests use memory.
pub(crate) trait LiveStore: Send + Sync {
    fn read(&self, access: KeychainAccess) -> crate::pool::Response<'_, Option<Vec<u8>>>;
    fn write(&self, bytes: Vec<u8>, access: KeychainAccess) -> crate::pool::Response<'_, ()>;
}

/// The item in the system's credential store: on macOS through
/// `/usr/bin/security`, as agy reaches it.
#[cfg(all(target_os = "macos", not(test)))]
pub(crate) fn system_store() -> std::sync::Arc<dyn LiveStore> {
    std::sync::Arc::new(super::security::Keychain::default())
}
#[cfg(all(not(target_os = "macos"), not(test)))]
pub(crate) fn system_store() -> std::sync::Arc<dyn LiveStore> {
    std::sync::Arc::new(SystemStore)
}
/// go-keyring's entry, through the system's keyring API.
#[cfg(not(target_os = "macos"))]
pub(crate) struct SystemStore;
/// go-keyring's entry: on Windows the credential `gemini:antigravity`, on
/// Linux the Secret Service item of that service and user name.
#[cfg(not(target_os = "macos"))]
fn with_entry<T: Send + 'static>(
    call: impl FnOnce(keyring::Entry) -> Result<T> + Send + 'static,
) -> Result<T> {
    let entry = || {
        #[cfg(target_os = "windows")]
        let entry =
            keyring::Entry::new_with_target(&format!("{SERVICE}:{ACCOUNT}"), SERVICE, ACCOUNT);
        #[cfg(not(target_os = "windows"))]
        let entry = keyring::Entry::new(SERVICE, ACCOUNT);
        entry.map_err(|e| e.to_string())
    };
    // keyring's Linux backend must not run on a Tokio thread (see
    // `crate::credentials`).
    std::thread::spawn(move || call(entry()?))
        .join()
        .map_err(|_| "The system keyring failed".to_string())?
}
#[cfg(not(target_os = "macos"))]
impl LiveStore for SystemStore {
    fn read(&self, _: KeychainAccess) -> crate::pool::Response<'_, Option<Vec<u8>>> {
        Box::pin(async {
            tokio::task::spawn_blocking(|| {
                with_entry(|entry| match entry.get_secret() {
                    Ok(bytes) => Ok(Some(bytes)),
                    Err(keyring::Error::NoEntry) => Ok(None),
                    Err(e) => Err(format!("{ITEM} access failed: {e}")),
                })
            })
            .await
            .map_err(|e| e.to_string())?
        })
    }
    fn write(&self, bytes: Vec<u8>, _: KeychainAccess) -> crate::pool::Response<'_, ()> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                with_entry(move |entry| {
                    entry
                        .set_secret(&bytes)
                        .map_err(|e| format!("Could not write {ITEM}: {e}"))
                })
            })
            .await
            .map_err(|e| e.to_string())?
        })
    }
}
