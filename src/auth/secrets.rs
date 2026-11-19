//! The ONLY module allowed to call the `keyring` crate (architecture §5).
//!
//! Stored entries (service `dev.ferro.ferrotune`):
//! * `librespot` — JSON with librespot's reusable credentials blob. This is
//!   the only credential Ferrotune keeps.
//! * `oauth` — legacy (v0.1/v0.2 Web API tokens); deleted on sight.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use secrecy::{ExposeSecret, SecretString};

use crate::config::APP_ID;
use crate::error::{Error, Result};

/// Legacy entry from the Web-API-based releases; only ever deleted.
pub const KEY_LEGACY_OAUTH: &str = "oauth";
pub const KEY_LIBRESPOT: &str = "librespot";

/// A secret value. Zeroised on drop, never printed.
#[derive(Clone)]
pub struct Secret(SecretString);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(SecretString::from(value.into()))
    }

    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl PartialEq for Secret {
    fn eq(&self, other: &Self) -> bool {
        self.expose() == other.expose()
    }
}

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Secret>>;
    fn set(&self, key: &str, value: &Secret) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}

/// OS keyring (Secret Service / Keychain / Credential Manager).
/// Calls are blocking: run them in `spawn_blocking`.
pub struct KeyringStore {
    service: String,
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self {
            service: APP_ID.to_owned(),
        }
    }
}

fn map_err(e: keyring::Error) -> Error {
    match e {
        keyring::Error::NoDefaultStore => {
            Error::Keyring("no Secret Service / keyring is available".into())
        }
        other => Error::Keyring(other.to_string()),
    }
}

impl KeyringStore {
    fn entry(&self, key: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, key).map_err(map_err)
    }

    /// Fail early (on the login screen) when no keyring exists.
    pub fn check_available() -> Result<()> {
        match keyring::Entry::store_status() {
            Ok(()) => Ok(()),
            Err(_) => Err(Error::Keyring(
                "no Secret Service / keyring is available".into(),
            )),
        }
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> Result<Option<Secret>> {
        match self.entry(key)?.get_password() {
            Ok(v) => Ok(Some(Secret::new(v))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    fn set(&self, key: &str, value: &Secret) -> Result<()> {
        self.entry(key)?
            .set_password(value.expose())
            .map_err(map_err)
    }

    fn delete(&self, key: &str) -> Result<()> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_err(e)),
        }
    }
}

/// In-memory store for tests and `--mock` mode.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, Secret>>);

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<Secret>> {
        let map = self
            .0
            .lock()
            .map_err(|_| Error::Keyring("memory store poisoned".into()))?;
        Ok(map.get(key).cloned())
    }

    fn set(&self, key: &str, value: &Secret) -> Result<()> {
        let mut map = self
            .0
            .lock()
            .map_err(|_| Error::Keyring("memory store poisoned".into()))?;
        map.insert(key.to_owned(), value.clone());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        let mut map = self
            .0
            .lock()
            .map_err(|_| Error::Keyring("memory store poisoned".into()))?;
        map.remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrip() {
        let s = MemoryStore::new();
        assert!(s.get("k").unwrap().is_none());
        s.set("k", &Secret::new("v")).unwrap();
        assert_eq!(s.get("k").unwrap().unwrap().expose(), "v");
        s.delete("k").unwrap();
        assert!(s.get("k").unwrap().is_none());
    }

    #[test]
    fn secret_debug_is_redacted() {
        let s = Secret::new("hunter2");
        assert_eq!(format!("{s:?}"), "[REDACTED]");
    }
}
