//! Storage for the Bandcamp `identity` session cookie.
//!
//! The cookie grants full access to the account, so it lives in the OS keyring by
//! default. A private file is available for machines without a Secret Service.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const KEYRING_SERVICE: &str = "bandcamp-tui";
pub const KEYRING_USER: &str = "identity";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum SecretStoreKind {
    /// Secret Service on Linux, Keychain on macOS, Credential Manager on Windows.
    #[default]
    Keyring,
    /// A file readable only by the current user, under the app data directory.
    File,
}

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("keyring: {0}")]
    Keyring(#[from] keyring::Error),
    /// The platform store could not be opened at all (no Secret Service, no D-Bus session, ...).
    #[error("no OS keyring available: {0}")]
    KeyringUnavailable(String),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl SecretError {
    /// A recovery suggestion for the user, when there is one.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            SecretError::KeyringUnavailable(_)
            | SecretError::Keyring(
                keyring::Error::PlatformFailure(_)
                | keyring::Error::NoStorageAccess(_)
                | keyring::Error::NoDefaultStore,
            ) => Some(
                "Restart with --secret-store file to keep the cookie in a private file instead of the OS keyring.",
            ),
            _ => None,
        }
    }
}

pub trait CookieStore: Send + Sync {
    fn load(&self) -> Result<Option<String>, SecretError>;
    fn save(&self, cookie: &str) -> Result<(), SecretError>;
    fn clear(&self) -> Result<(), SecretError>;
    /// Short human description, shown on the login screen.
    fn describe(&self) -> String;
}

pub fn open(kind: SecretStoreKind) -> anyhow::Result<Arc<dyn CookieStore>> {
    Ok(match kind {
        SecretStoreKind::Keyring => Arc::new(KeyringStore),
        SecretStoreKind::File => {
            Arc::new(FileStore::new(crate::config::data_dir()?.join("identity")))
        }
    })
}

pub struct KeyringStore;

impl KeyringStore {
    fn entry() -> Result<keyring::Entry, SecretError> {
        // The crate opens the platform store lazily and afterwards only reports
        // "no default store"; ask for the original failure so the user sees why.
        if let Err(err) = keyring::Entry::store_status() {
            return Err(SecretError::KeyringUnavailable(err.to_string()));
        }
        Ok(keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)?)
    }
}

impl CookieStore for KeyringStore {
    fn load(&self) -> Result<Option<String>, SecretError> {
        match Self::entry()?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn save(&self, cookie: &str) -> Result<(), SecretError> {
        Ok(Self::entry()?.set_password(cookie)?)
    }

    fn clear(&self) -> Result<(), SecretError> {
        match Self::entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    fn describe(&self) -> String {
        format!("OS keyring (service \"{KEYRING_SERVICE}\")")
    }
}

pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn io(&self, source: io::Error) -> SecretError {
        SecretError::Io {
            path: self.path.clone(),
            source,
        }
    }
}

impl CookieStore for FileStore {
    fn load(&self) -> Result<Option<String>, SecretError> {
        match fs::read_to_string(&self.path) {
            Ok(text) => {
                let value = text.trim();
                Ok((!value.is_empty()).then(|| value.to_owned()))
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(self.io(err)),
        }
    }

    fn save(&self, cookie: &str) -> Result<(), SecretError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| self.io(e))?;
        }
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&self.path).map_err(|e| self.io(e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // The mode above only applies when the file is created; enforce it on reuse too.
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|e| self.io(e))?;
        }
        use std::io::Write;
        file.write_all(cookie.as_bytes()).map_err(|e| self.io(e))?;
        file.write_all(b"\n").map_err(|e| self.io(e))
    }

    fn clear(&self) -> Result<(), SecretError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(self.io(err)),
        }
    }

    fn describe(&self) -> String {
        format!("private file {}", self.path.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bandcamp-tui-test-{}", std::process::id()));
        dir.join(name)
    }

    #[test]
    fn file_store_round_trip() {
        let store = FileStore::new(temp_path("identity"));
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);

        store.save("abc%09def").unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some("abc%09def"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&store.path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
        store.clear().unwrap();
    }
}
