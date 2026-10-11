//! Credentials are kept out of serialized provider settings.
pub trait SecretStore: Send + Sync {
    fn get(&self, id: &str) -> Result<Option<String>, String>;
    fn set(&self, id: &str, password: &str) -> Result<(), String>;
    fn remove(&self, id: &str) -> Result<(), String>;
}
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

// Use SecItem APIs and the user's keychain search list on macOS. keyring's
// legacy backend first resolves SecKeychainCopyDomainDefault, which can fail
// with ENOENT before it even searches for the credential. Keep the same service
// and account attributes so existing generic passwords remain accessible.
#[cfg(target_os = "macos")]
mod macos {
    use super::{SecretStore, SystemSecrets, SERVICE};
    use security_framework::{base::Error, passwords};

    fn validate_id(id: &str) -> Result<(), String> {
        if id.is_empty() {
            Err("The credential account cannot be empty".into())
        } else {
            Ok(())
        }
    }

    fn optional<T>(result: Result<T, Error>) -> Result<Option<T>, String> {
        match result {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.code() == -25300 => Ok(None), // errSecItemNotFound
            Err(error) => Err(format!("System keychain error: {error}")),
        }
    }

    impl SecretStore for SystemSecrets {
        fn get(&self, id: &str) -> Result<Option<String>, String> {
            validate_id(id)?;
            optional(passwords::generic_password(
                passwords::PasswordOptions::new_generic_password(SERVICE, id),
            ))?
            .map(|bytes| String::from_utf8(bytes).map_err(|_| "Invalid UTF-8 credential".into()))
            .transpose()
        }

        fn set(&self, id: &str, password: &str) -> Result<(), String> {
            validate_id(id)?;
            passwords::set_generic_password(SERVICE, id, password.as_bytes())
                .map_err(|error| format!("System keychain error: {error}"))
        }

        fn remove(&self, id: &str) -> Result<(), String> {
            validate_id(id)?;
            optional(passwords::delete_generic_password(SERVICE, id)).map(|_| ())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_missing_items_are_treated_as_absent() {
            assert_eq!(optional::<()>(Err(Error::from_code(-25300))), Ok(None));
            for code in [2, -25294, -25308, -25293] {
                assert!(optional::<()>(Err(Error::from_code(code))).is_err());
            }
            assert_eq!(optional(Ok("secret")), Ok(Some("secret")));
        }

        #[test]
        fn empty_accounts_are_rejected_before_accessing_keychain() {
            assert!(SystemSecrets.get("").is_err());
            assert!(SystemSecrets.set("", "secret").is_err());
            assert!(SystemSecrets.remove("").is_err());
        }
    }
}
