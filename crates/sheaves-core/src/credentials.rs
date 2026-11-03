//! Where the Harvest token is kept. Each shell provides the store: the Keychain on
//! a Mac, the Secret Service keyring on GNOME.

use std::sync::Mutex;

use crate::client::Credentials;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum CredentialError {
    /// The store failed; `message` says why, in words a person can act on.
    #[error("{message}")]
    Failed { message: String },
}

/// A callback into the shell that failed in a way its signature did not allow,
/// such as a Swift trap the bindings caught.
impl From<uniffi::UnexpectedUniFFICallbackError> for CredentialError {
    fn from(error: uniffi::UnexpectedUniFFICallbackError) -> Self {
        CredentialError::Failed {
            message: error.reason,
        }
    }
}

/// Who the app is connected as: the credentials, and the Harvest user they were
/// verified to belong to.
///
/// The one durable record of the connection. The queue's owner and the cached user
/// are checked against it, never the other way round, so a crash between writes
/// cannot leave the app trusting whichever it happens to read first.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Login {
    pub credentials: Credentials,
    /// `None` for a login saved before user ids were recorded; the tracker asks
    /// Harvest once and saves it.
    pub user_id: Option<i64>,
}

#[uniffi::export(with_foreign)]
pub trait CredentialStore: Send + Sync {
    /// The stored login, or `None` before anything is stored.
    fn read(&self) -> Result<Option<Login>, CredentialError>;
    /// Replaces whatever is stored.
    fn write(&self, login: Login) -> Result<(), CredentialError>;
    /// Removes the stored credentials. Deleting nothing is not an error.
    fn delete(&self) -> Result<(), CredentialError>;
}

/// Credentials held only in memory, for tests and demos.
#[derive(Default)]
pub struct MemoryCredentialStore {
    stored: Mutex<Option<Login>>,
}

impl MemoryCredentialStore {
    pub fn new(login: Option<Login>) -> Self {
        Self {
            stored: Mutex::new(login),
        }
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn read(&self) -> Result<Option<Login>, CredentialError> {
        Ok(self.stored.lock().unwrap().clone())
    }

    fn write(&self, login: Login) -> Result<(), CredentialError> {
        *self.stored.lock().unwrap() = Some(login);
        Ok(())
    }

    fn delete(&self) -> Result<(), CredentialError> {
        *self.stored.lock().unwrap() = None;
        Ok(())
    }
}
