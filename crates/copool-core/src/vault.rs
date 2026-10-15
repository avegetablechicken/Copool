//! Copool's own Keychain item on macOS. One item holds the secrets Copool
//! keeps, so that a build with a new ad-hoc signature asks for access once:
//! the Claude pool's tokens, and the passwords and keys of Sub2api providers,
//! remote servers and the Cloudflare tunnel.
use crate::credentials::SecretStore;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

type Result<T> = std::result::Result<T, String>;

/// Recurring synchronization stays silent; startup and explicit actions may authorize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeychainAccess {
    Background,
    Interactive,
}

/// Where the item's text is kept.
pub(crate) trait Vault: Send + Sync {
    /// The stored text; none before anything was stored.
    fn read(&self, access: KeychainAccess) -> Result<Option<String>>;
    fn write(&self, text: &str, access: KeychainAccess) -> Result<()>;
}

const VERSION: u64 = 2;
/// The Claude pool's tokens by account id.
const CLAUDE: &str = "claude";
/// Other secrets by the id their `SecretStore` callers give them.
const SECRETS: &str = "secrets";
const NOT_COPOOLS: &str = "Copool's Keychain item holds something other than Copool's secrets";

/// The item's sections by name. Contents of another version are never
/// written over.
fn document(text: Option<String>) -> Result<Value> {
    let Some(text) = text else {
        return Ok(json!({ "version": VERSION }));
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(doc) if doc["version"].as_u64() == Some(VERSION) => Ok(doc),
        _ => Err(NOT_COPOOLS.into()),
    }
}

/// The item, shared by its sections, read from the Keychain once and kept.
pub(crate) struct Item {
    store: Arc<dyn Vault>,
    /// The document as last read or written; none until read.
    cache: Mutex<Option<Value>>,
    /// Whether reading may ask the user: until a request is refused, and
    /// again after an explicit action.
    ask: AtomicBool,
}
impl Item {
    pub(crate) fn new(store: Arc<dyn Vault>) -> Arc<Self> {
        Arc::new(Self {
            store,
            cache: Mutex::new(None),
            ask: AtomicBool::new(true),
        })
    }
    fn access(&self) -> KeychainAccess {
        if self.ask.load(Ordering::Relaxed) {
            KeychainAccess::Interactive
        } else {
            KeychainAccess::Background
        }
    }
    /// Reads the item from the Keychain. A read that may ask holds the cache,
    /// so that concurrent requests wait for its answer instead of asking
    /// again; a background read never waits for one.
    fn read(&self, access: KeychainAccess) -> Result<Value> {
        if access == KeychainAccess::Background {
            let doc = self.store.read(access).and_then(document)?;
            if let Ok(mut cache) = self.cache.try_lock() {
                *cache = Some(doc.clone());
            }
            return Ok(doc);
        }
        let mut cache = self.cache.lock().unwrap();
        self.ask_for(&mut cache)
    }
    fn ask_for(&self, cache: &mut Option<Value>) -> Result<Value> {
        let read = self
            .store
            .read(KeychainAccess::Interactive)
            .and_then(document);
        self.ask.store(read.is_ok(), Ordering::Relaxed);
        let doc = read?;
        *cache = Some(doc.clone());
        Ok(doc)
    }
    /// The document, read once.
    fn cached(&self) -> Result<Value> {
        if self.access() == KeychainAccess::Interactive {
            let mut cache = self.cache.lock().unwrap();
            return match cache.as_ref() {
                Some(doc) => Ok(doc.clone()),
                None => self.ask_for(&mut cache),
            };
        }
        if let Some(doc) = self.cache.try_lock().ok().and_then(|cache| cache.clone()) {
            return Ok(doc);
        }
        self.read(KeychainAccess::Background)
    }
    /// An explicit action: reading may ask the user again, and does unless
    /// the item was read already.
    pub(crate) fn authorize(&self) -> Result<()> {
        self.ask.store(true, Ordering::Relaxed);
        self.cached().map(|_| ())
    }
    /// Edits the document and writes it; the item must read back as written.
    /// The edit applies to the item as the Keychain holds it now, not as
    /// cached: another Copool (a smoke test's, say) may have written it since,
    /// and a section written back from an older copy could restore tokens
    /// already rotated away.
    fn update(&self, access: KeychainAccess, edit: impl FnOnce(&mut Value)) -> Result<()> {
        let mut cache = match access {
            KeychainAccess::Interactive => self.cache.lock().unwrap(),
            KeychainAccess::Background => self
                .cache
                .try_lock()
                .map_err(|_| "Copool's Keychain item is being read")?,
        };
        let current = self.store.read(access).and_then(document)?;
        let mut next = current.clone();
        edit(&mut next);
        if next == current {
            *cache = Some(current);
            return Ok(());
        }
        let text = next.to_string();
        let stored = self
            .store
            .write(&text, access)
            .and_then(|()| self.store.read(access));
        match stored {
            Ok(Some(read)) if read == text => {
                *cache = Some(next);
                Ok(())
            }
            // What the item holds now is not known.
            outcome => {
                *cache = None;
                Err(outcome.err().unwrap_or_else(|| {
                    "Copool's Keychain item did not keep what was written".into()
                }))
            }
        }
    }
}

/// The Claude pool's tokens in the item. Every read goes to the Keychain, as
/// the pool's own checks of what it reads expect.
pub(crate) struct Section {
    item: Arc<Item>,
}
impl Section {
    pub(crate) fn claude(item: Arc<Item>) -> Self {
        Self { item }
    }
}
impl Vault for Section {
    fn read(&self, access: KeychainAccess) -> Result<Option<String>> {
        Ok(self.item.read(access)?.get(CLAUDE).map(Value::to_string))
    }
    fn write(&self, text: &str, access: KeychainAccess) -> Result<()> {
        let section: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        self.item.update(access, |doc| doc[CLAUDE] = section)
    }
}

/// `SecretStore` over the item's secrets.
pub(crate) struct ItemSecrets {
    item: Arc<Item>,
}
impl ItemSecrets {
    pub(crate) fn new(item: Arc<Item>) -> Self {
        Self { item }
    }
}
impl SecretStore for ItemSecrets {
    fn get(&self, id: &str) -> Result<Option<String>> {
        Ok(self.item.cached()?[SECRETS]
            .get(id)
            .and_then(Value::as_str)
            .map(str::to_owned))
    }
    fn set(&self, id: &str, password: &str) -> Result<()> {
        self.item.update(KeychainAccess::Interactive, |doc| {
            doc[SECRETS][id] = password.into()
        })
    }
    fn remove(&self, id: &str) -> Result<()> {
        self.item.update(KeychainAccess::Interactive, |doc| {
            if let Some(secrets) = doc[SECRETS].as_object_mut() {
                secrets.remove(id);
            }
        })
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{KeychainAccess, Result, Vault};
    use crate::claude::keychain;

    const SERVICE: &str = "io.github.copool.rust.claude";
    const ACCOUNT: &str = "accounts";
    /// The app's bundle identifier.
    const LABEL: &str = "io.github.copool.rust";

    /// The item in the user's Keychain.
    pub(crate) struct KeychainVault;
    impl Vault for KeychainVault {
        fn read(&self, access: KeychainAccess) -> Result<Option<String>> {
            keychain::read_private(SERVICE, ACCOUNT, access)?
                .map(|bytes| {
                    String::from_utf8(bytes).map_err(|_| "Invalid Copool Keychain item".into())
                })
                .transpose()
        }
        fn write(&self, text: &str, access: KeychainAccess) -> Result<()> {
            keychain::write_private(SERVICE, ACCOUNT, LABEL, text.as_bytes(), access)
        }
    }
}
#[cfg(target_os = "macos")]
pub(crate) use macos::KeychainVault;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The item in memory, recording how each read asked.
    #[derive(Default)]
    pub(crate) struct Memory {
        pub(crate) text: Mutex<Option<String>>,
        pub(crate) refuse: AtomicBool,
        pub(crate) reads: Mutex<Vec<KeychainAccess>>,
    }
    impl Vault for Memory {
        fn read(&self, access: KeychainAccess) -> Result<Option<String>> {
            self.reads.lock().unwrap().push(access);
            if self.refuse.load(Ordering::Relaxed) {
                return Err("refused".into());
            }
            Ok(self.text.lock().unwrap().clone())
        }
        fn write(&self, text: &str, _: KeychainAccess) -> Result<()> {
            *self.text.lock().unwrap() = Some(text.into());
            Ok(())
        }
    }
    fn stored(memory: &Memory) -> Value {
        serde_json::from_str(memory.text.lock().unwrap().as_deref().unwrap()).unwrap()
    }

    #[test]
    fn the_claude_tokens_and_other_secrets_share_the_item() {
        let memory = Arc::new(Memory::default());
        *memory.text.lock().unwrap() =
            Some(r#"{"version":2,"claude":{"a":{"accessToken":"t"}}}"#.into());
        let item = Item::new(memory.clone());
        let claude = Section::claude(item.clone());
        let secrets = ItemSecrets::new(item.clone());
        assert_eq!(
            claude.read(KeychainAccess::Background).unwrap().as_deref(),
            Some(r#"{"a":{"accessToken":"t"}}"#)
        );
        secrets.set("provider", "password").unwrap();
        assert_eq!(
            stored(&memory),
            json!({"version": 2, "claude": {"a": {"accessToken": "t"}}, "secrets": {"provider": "password"}})
        );
        // Writing one section keeps the other.
        claude
            .write(r#"{"b":{"accessToken":"u"}}"#, KeychainAccess::Background)
            .unwrap();
        assert_eq!(
            secrets.get("provider").unwrap().as_deref(),
            Some("password")
        );
        assert_eq!(
            stored(&memory)["claude"],
            json!({"b": {"accessToken": "u"}})
        );
        secrets.remove("provider").unwrap();
        assert_eq!(secrets.get("provider").unwrap(), None);
        assert_eq!(stored(&memory)["secrets"], json!({}));
        // Contents of another version, or not Copool's, are never written over.
        for text in ["[1]", r#"{"a":{"accessToken":"t"}}"#, r#"{"version":3}"#] {
            *memory.text.lock().unwrap() = Some(text.into());
            let item = Item::new(memory.clone());
            assert!(ItemSecrets::new(item.clone()).set("x", "y").is_err());
            assert!(Section::claude(item)
                .read(KeychainAccess::Background)
                .is_err());
            assert_eq!(memory.text.lock().unwrap().as_deref(), Some(text));
        }
    }

    #[test]
    fn secrets_are_read_once_and_asked_for_until_refused() {
        let memory = Arc::new(Memory::default());
        let secrets = ItemSecrets::new(Item::new(memory.clone()));
        // The first read may ask; the others use what it read.
        assert_eq!(secrets.get("a").unwrap(), None);
        assert_eq!(secrets.get("b").unwrap(), None);
        assert_eq!(*memory.reads.lock().unwrap(), [KeychainAccess::Interactive]);
        // Once refused, background reads stop asking until an explicit action.
        let memory = Arc::new(Memory::default());
        memory.refuse.store(true, Ordering::Relaxed);
        let item = Item::new(memory.clone());
        let secrets = ItemSecrets::new(item.clone());
        assert!(secrets.get("a").is_err());
        assert!(secrets.get("a").is_err());
        assert!(item.authorize().is_err());
        use KeychainAccess::*;
        assert_eq!(
            *memory.reads.lock().unwrap(),
            [Interactive, Background, Interactive]
        );
        memory.refuse.store(false, Ordering::Relaxed);
        item.authorize().unwrap();
        assert_eq!(secrets.get("a").unwrap(), None);
        assert_eq!(memory.reads.lock().unwrap().len(), 4);
    }

    #[test]
    fn a_write_edits_the_item_as_it_is_now_not_as_cached() {
        let memory = Arc::new(Memory::default());
        let secrets = ItemSecrets::new(Item::new(memory.clone()));
        assert_eq!(secrets.get("a").unwrap(), None);
        // Another Copool renews the Claude pool's tokens meanwhile.
        *memory.text.lock().unwrap() =
            Some(r#"{"version":2,"claude":{"x":{"refreshToken":"renewed"}}}"#.into());
        secrets.set("a", "b").unwrap();
        assert_eq!(
            stored(&memory),
            json!({"version": 2, "claude": {"x": {"refreshToken": "renewed"}}, "secrets": {"a": "b"}})
        );
    }

    #[test]
    fn a_write_that_does_not_read_back_is_an_error_and_forgotten() {
        struct Lossy(Memory);
        impl Vault for Lossy {
            fn read(&self, access: KeychainAccess) -> Result<Option<String>> {
                self.0.read(access)
            }
            fn write(&self, _: &str, _: KeychainAccess) -> Result<()> {
                Ok(())
            }
        }
        let lossy = Arc::new(Lossy(Memory::default()));
        let secrets = ItemSecrets::new(Item::new(lossy.clone()));
        assert!(secrets.set("a", "b").is_err());
        // The next read goes to the Keychain again.
        assert_eq!(secrets.get("a").unwrap(), None);
        assert_eq!(lossy.0.reads.lock().unwrap().len(), 3);
    }
}
