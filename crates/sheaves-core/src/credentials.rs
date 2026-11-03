//! Where the Harvest token is kept. Each shell provides the store: the Keychain on
//! a Mac, the Secret Service keyring on GNOME.

use std::sync::Mutex;

use crate::client::Credentials;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CredentialError(pub String);

pub trait CredentialStore: Send + Sync {
    /// The stored credentials, or `None` before anything is stored.
    fn read(&self) -> Result<Option<Credentials>, CredentialError>;
    /// Replaces whatever is stored.
    fn write(&self, credentials: &Credentials) -> Result<(), CredentialError>;
    /// Removes the stored credentials. Deleting nothing is not an error.
    fn delete(&self) -> Result<(), CredentialError>;
}

/// Credentials held only in memory, for tests and demos.
#[derive(Default)]
pub struct MemoryCredentialStore {
    stored: Mutex<Option<Credentials>>,
}

impl MemoryCredentialStore {
    pub fn new(credentials: Option<Credentials>) -> Self {
        Self {
            stored: Mutex::new(credentials),
        }
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn read(&self) -> Result<Option<Credentials>, CredentialError> {
        Ok(self.stored.lock().unwrap().clone())
    }

    fn write(&self, credentials: &Credentials) -> Result<(), CredentialError> {
        *self.stored.lock().unwrap() = Some(credentials.clone());
        Ok(())
    }

    fn delete(&self) -> Result<(), CredentialError> {
        *self.stored.lock().unwrap() = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The other Keychain tests exercise the Keychain itself, which stays in the
    /// Mac shell; this one is about the credentials, so it moves to the core.
    #[test]
    fn trims_pasted_whitespace() {
        let store = MemoryCredentialStore::default();
        store
            .write(&Credentials::new("  12345\n", " sekrit "))
            .unwrap();
        let read = store.read().unwrap().unwrap();
        assert_eq!(read.token, "sekrit");
        assert_eq!(read.account_id, "12345");
    }
}
