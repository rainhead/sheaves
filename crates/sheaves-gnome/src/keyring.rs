//! The Harvest login, kept in the desktop's keyring through the Secret Service.

use serde::{Deserialize, Serialize};
use sheaves_core::client::Credentials;
use sheaves_core::credentials::{CredentialError, CredentialStore, Login};

/// The attributes the login is stored under, which is how it is found again.
const ATTRIBUTES: [(&str, &str); 1] = [("application", "com.rainhead.Sheaves")];

/// What is stored as the secret: the login, as JSON.
#[derive(Serialize, Deserialize)]
struct Stored {
    account_id: String,
    token: String,
    user_id: Option<i64>,
}

/// The core asks for credentials from inside its own async runtime, through a
/// trait whose methods return at once. oo7 is async, so each call runs on a thread
/// of its own with a runtime of its own, and the caller waits for it: the same
/// short block the Keychain causes on a Mac.
pub struct Keyring;

impl Keyring {
    fn run<T: Send>(
        work: impl AsyncFnOnce(oo7::Keyring) -> Result<T, oo7::Error> + Send,
    ) -> Result<T, CredentialError> {
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(failed)?;
                    runtime.block_on(async {
                        let keyring = oo7::Keyring::new().await.map_err(failed)?;
                        work(keyring).await.map_err(failed)
                    })
                })
                .join()
                .unwrap_or_else(|_| {
                    Err(CredentialError::Failed {
                        message: "the keyring call panicked".to_string(),
                    })
                })
        })
    }
}

fn failed(error: impl std::fmt::Display) -> CredentialError {
    CredentialError::Failed {
        message: format!("The keyring could not be used: {error}"),
    }
}

/// Asks for the keyring to be unlocked if it is locked, as after an automatic
/// login. Writing or deleting in a locked keyring fails rather than asking, and a
/// failed delete on disconnect would leave the token there for the next launch.
// The error is oo7's own, and the closures in `run` return it too.
#[allow(clippy::result_large_err)]
async fn unlocked(keyring: &oo7::Keyring) -> Result<(), oo7::Error> {
    if keyring.is_locked().await? {
        keyring.unlock().await?;
    }
    Ok(())
}

impl CredentialStore for Keyring {
    fn read(&self) -> Result<Option<Login>, CredentialError> {
        let secret = Self::run(async |keyring| {
            let items = keyring.search_items(&ATTRIBUTES).await?;
            let Some(item) = items.into_iter().next() else {
                return Ok(None);
            };
            if item.is_locked().await? {
                item.unlock().await?;
            }
            Ok(Some(item.secret().await?.as_bytes().to_vec()))
        })?;
        let Some(secret) = secret else {
            return Ok(None);
        };
        let stored: Stored =
            serde_json::from_slice(&secret).map_err(|error| CredentialError::Failed {
                message: format!("The login in the keyring could not be read: {error}"),
            })?;
        Ok(Some(Login {
            credentials: Credentials::new(stored.account_id, stored.token),
            user_id: stored.user_id,
        }))
    }

    fn write(&self, login: Login) -> Result<(), CredentialError> {
        let secret = serde_json::to_vec(&Stored {
            account_id: login.credentials.account_id,
            token: login.credentials.token,
            user_id: login.user_id,
        })
        .expect("a login encodes");
        Self::run(async move |keyring| {
            unlocked(&keyring).await?;
            keyring
                .create_item("Sheaves: Harvest login", &ATTRIBUTES, secret, true)
                .await
        })
    }

    fn delete(&self) -> Result<(), CredentialError> {
        Self::run(async |keyring| {
            unlocked(&keyring).await?;
            keyring.delete(&ATTRIBUTES).await
        })
    }
}
