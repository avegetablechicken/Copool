//! Direct SecItem access: background work fails instead of displaying UI.
use super::{parse_credentials, KeychainAccess, Result};
use core_foundation::{
    base::TCFType,
    data::CFData,
    dictionary::CFDictionary,
    string::{CFString, CFStringRef},
};
use security_framework::{
    base::Error,
    passwords::{self, PasswordOptions},
};
use security_framework_sys::{
    item::{kSecAttrAccount, kSecUseAuthenticationUI, kSecValueData},
    keychain_item::SecItemUpdate,
};
use serde_json::Value;

#[link(name = "Security", kind = "framework")]
extern "C" {
    // Not exposed by security-framework-sys 2.x.
    static kSecUseAuthenticationUIFail: CFStringRef;
}

#[allow(deprecated)] // PasswordOptions has no authentication-UI setter.
fn options(service: &str, account: Option<&str>, access: KeychainAccess) -> PasswordOptions {
    let mut options = PasswordOptions::new_generic_password(service, account.unwrap_or_default());
    if account.is_none() {
        let key = unsafe { CFString::wrap_under_get_rule(kSecAttrAccount) };
        options.query.retain(|(name, _)| *name != key);
    }
    if access == KeychainAccess::Background {
        options.query.push(unsafe {
            (
                CFString::wrap_under_get_rule(kSecUseAuthenticationUI),
                CFString::wrap_under_get_rule(kSecUseAuthenticationUIFail).into_CFType(),
            )
        });
    }
    options
}

fn read_with(
    user: &str,
    mut query: impl FnMut(Option<&str>) -> std::result::Result<Vec<u8>, Error>,
) -> Result<Option<Value>> {
    let accounts = if user.is_empty() {
        vec![None]
    } else {
        vec![Some(user), None]
    };
    for account in accounts {
        match query(account) {
            Ok(bytes) => {
                return parse_credentials(&bytes)
                    .map(Some)
                    .ok_or_else(|| "Invalid Claude Code Keychain credentials".into())
            }
            Err(error) if error.code() == -25300 => continue, // Only absent items allow fallback.
            Err(error) => return Err(format!("Claude Code Keychain access failed: {error}")),
        }
    }
    Ok(None)
}

pub(super) async fn read(service: &str, access: KeychainAccess) -> Result<Option<Value>> {
    let service = service.to_owned();
    tokio::task::spawn_blocking(move || {
        let user = std::env::var("USER").unwrap_or_default();
        read_with(&user, |account| {
            passwords::generic_password(options(&service, account, access))
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

// Update only value data: never replace the existing access/partition settings.
#[allow(deprecated)]
fn update(service: &str, user: &str, bytes: &[u8], access: KeychainAccess) -> Result<bool> {
    let query = options(service, Some(user), access);
    let query = CFDictionary::from_CFType_pairs(&query.query);
    let attributes = value_attributes(bytes);
    let status = unsafe {
        SecItemUpdate(
            query.as_concrete_TypeRef(),
            attributes.as_concrete_TypeRef(),
        )
    };
    match status {
        0 => Ok(true),
        -25300 => Ok(false),
        code => Err(format!(
            "Could not update the Claude Code Keychain entry: {}",
            Error::from_code(code)
        )),
    }
}
fn value_attributes(bytes: &[u8]) -> CFDictionary<CFString, core_foundation::base::CFType> {
    CFDictionary::from_CFType_pairs(&[(
        unsafe { CFString::wrap_under_get_rule(kSecValueData) },
        CFData::from_buffer(bytes).into_CFType(),
    )])
}

fn creation_command(
    service: &str,
    user: &str,
    bytes: &[u8],
    access: KeychainAccess,
) -> Result<String> {
    if access != KeychainAccess::Interactive {
        return Err("Import or switch the Claude login manually before automatic switching".into());
    }
    if user.is_empty()
        || [service, user]
            .iter()
            .any(|v| v.chars().any(|c| c.is_control() || c == '\"' || c == '\\'))
    {
        return Err("Unsupported Keychain service or account name".into());
    }
    // Deliberately omit -U: a concurrent creator must not have its item overwritten.
    let command = format!(
        "add-generic-password -a \"{user}\" -s \"{service}\" -X \"{}\"\n",
        super::hex(bytes)
    );
    // security's interactive parser has a bounded line buffer. Never fall back
    // to placing a large credential in process arguments.
    if command.len() > 4095 {
        return Err("Claude credentials are too large to create through the Keychain tool; sign in with Claude Code first".into());
    }
    Ok(command)
}

pub(super) async fn write(service: &str, document: &Value, access: KeychainAccess) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let user = std::env::var("USER").unwrap_or_default();
    if user.is_empty() {
        return Err("The Keychain account name is unavailable".into());
    }
    let bytes = serde_json::to_vec(document).map_err(|e| e.to_string())?;
    let (owned_service, owned_user, owned_bytes) =
        (service.to_owned(), user.clone(), bytes.clone());
    let updated = tokio::task::spawn_blocking(move || {
        update(&owned_service, &owned_user, &owned_bytes, access)
    })
    .await
    .map_err(|e| e.to_string())??;
    if updated {
        return Ok(());
    }
    let command = creation_command(service, &user, &bytes, access)?;
    // A new shared item is created by the same system tool Claude Code uses.
    // Credentials travel only over stdin, never in argv or diagnostic messages.
    let mut child = tokio::process::Command::new("/usr/bin/security")
        .arg("-i")
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("Keychain input unavailable")?;
    stdin
        .write_all(command.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    drop(stdin);
    // Do not kill a request while the user is answering its authorization UI.
    let output = child.wait_with_output().await.map_err(|e| e.to_string())?;
    // Interactive security can exit successfully after a command failure.
    // Errors go to stderr (alongside its `security>` prompt); do not echo them.
    let errors = String::from_utf8_lossy(&output.stderr).replace("security>", "");
    if !output.status.success() || !errors.trim().is_empty() {
        return Err("Could not create the Claude Code Keychain entry".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_cannot_replace_access_attributes() {
        let attributes = value_attributes(b"synthetic-secret");
        assert_eq!(attributes.len(), 1);
        let key = unsafe { CFString::wrap_under_get_rule(kSecValueData) };
        assert!(attributes.find(key).is_some());
    }

    #[test]
    fn creation_is_interactive_only_and_never_overwrites_an_item() {
        assert!(creation_command("test", "user", b"data", KeychainAccess::Background).is_err());
        let command = creation_command(
            "test",
            "user",
            b"synthetic-secret",
            KeychainAccess::Interactive,
        )
        .unwrap();
        assert!(!command.contains("-U"));
        assert!(!command.contains("synthetic-secret"));
        assert_eq!(command.lines().count(), 1);
        for invalid in ["", "bad\nname", "bad\"name", "bad\\name"] {
            assert!(
                creation_command("test", invalid, b"data", KeychainAccess::Interactive).is_err()
            );
        }
        assert!(creation_command("test", "user", &[0; 4096], KeychainAccess::Interactive).is_err());
    }

    #[test]
    #[allow(deprecated)]
    fn background_queries_explicitly_forbid_authorization_ui() {
        let key = unsafe { CFString::wrap_under_get_rule(kSecUseAuthenticationUI) };
        let fail =
            unsafe { CFString::wrap_under_get_rule(kSecUseAuthenticationUIFail).into_CFType() };
        for account in [Some("test-user"), None] {
            let background = options("test-service", account, KeychainAccess::Background);
            assert!(background
                .query
                .iter()
                .any(|(k, v)| *k == key && *v == fail));
            let interactive = options("test-service", account, KeychainAccess::Interactive);
            assert!(!interactive.query.iter().any(|(k, _)| *k == key));
        }
    }

    #[test]
    fn refusal_or_lock_does_not_retry_with_another_account() {
        for code in [-128, -25293, -25308, -25291] {
            let mut calls = 0;
            assert!(read_with("test-user", |_| {
                calls += 1;
                Err(Error::from_code(code))
            })
            .is_err());
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn missing_item_falls_back_and_empty_user_is_only_queried_once() {
        let mut calls = vec![];
        assert_eq!(
            read_with("test-user", |account| {
                calls.push(account.map(str::to_owned));
                Err(Error::from_code(-25300))
            }),
            Ok(None)
        );
        assert_eq!(calls, vec![Some("test-user".into()), None]);
        let mut count = 0;
        assert_eq!(
            read_with("", |_| {
                count += 1;
                Err(Error::from_code(-25300))
            }),
            Ok(None)
        );
        assert_eq!(count, 1);
    }
}
