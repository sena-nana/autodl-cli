use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::AppError;
use crate::state::{read_json_or_default, write_json_atomic};

const SERVICE_NAME: &str = "autodl-cli";

/// Storage backend abstraction for AutoDL developer tokens.
pub trait CredentialStore: Send + Sync {
    fn backend_name(&self) -> &'static str;
    fn get(&self, profile: &str) -> Result<Option<String>, AppError>;
    fn set(&self, profile: &str, token: &str) -> Result<(), AppError>;
    fn delete(&self, profile: &str) -> Result<bool, AppError>;
}

/// Native OS credential manager backend (Windows Credential Manager, macOS Keychain,
/// Linux Secret Service) via the `keyring` crate.
pub struct KeyringStore;

impl KeyringStore {
    fn entry(profile: &str) -> Result<keyring::Entry, AppError> {
        let account = format!("profile:{profile}");
        keyring::Entry::new(SERVICE_NAME, &account).map_err(|e| {
            AppError::Keyring(format!("Failed to open OS credential entry for '{profile}': {e}"))
        })
    }
}

impl CredentialStore for KeyringStore {
    fn backend_name(&self) -> &'static str {
        #[cfg(target_os = "windows")]
        {
            "windows-credential-manager"
        }
        #[cfg(target_os = "macos")]
        {
            "macos-keychain"
        }
        #[cfg(target_os = "linux")]
        {
            "linux-secret-service"
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            "os-keyring"
        }
    }

    fn get(&self, profile: &str) -> Result<Option<String>, AppError> {
        let entry = Self::entry(profile)?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(AppError::Keyring(format!(
                "Failed to read token for profile '{profile}' from OS credential manager: {e}"
            ))),
        }
    }

    fn set(&self, profile: &str, token: &str) -> Result<(), AppError> {
        let entry = Self::entry(profile)?;
        entry.set_password(token).map_err(|e| {
            AppError::Keyring(format!(
                "Failed to store token for profile '{profile}' in OS credential manager: {e}"
            ))
        })
    }

    fn delete(&self, profile: &str) -> Result<bool, AppError> {
        let entry = Self::entry(profile)?;
        match entry.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(AppError::Keyring(format!(
                "Failed to delete token for profile '{profile}' from OS credential manager: {e}"
            ))),
        }
    }
}

/// File-backed credential store used ONLY when `AUTODL_CREDENTIAL_FILE` is explicitly set
/// (for automated integration tests or headless CI environments without an OS keyring).
pub struct FileCredentialStore {
    path: PathBuf,
}

impl FileCredentialStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl CredentialStore for FileCredentialStore {
    fn backend_name(&self) -> &'static str {
        "file-credential-store"
    }

    fn get(&self, profile: &str) -> Result<Option<String>, AppError> {
        let map: BTreeMap<String, String> = read_json_or_default(&self.path)?;
        Ok(map.get(profile).cloned())
    }

    fn set(&self, profile: &str, token: &str) -> Result<(), AppError> {
        let mut map: BTreeMap<String, String> = read_json_or_default(&self.path)?;
        map.insert(profile.to_string(), token.to_string());
        write_json_atomic(&self.path, &map)
    }

    fn delete(&self, profile: &str) -> Result<bool, AppError> {
        let mut map: BTreeMap<String, String> = read_json_or_default(&self.path)?;
        let existed = map.remove(profile).is_some();
        write_json_atomic(&self.path, &map)?;
        Ok(existed)
    }
}

/// Construct the active credential store. Defaults to the native OS keyring unless
/// `AUTODL_CREDENTIAL_FILE` is explicitly provided.
pub fn default_credential_store() -> Box<dyn CredentialStore> {
    if let Ok(path) = std::env::var("AUTODL_CREDENTIAL_FILE") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Box::new(FileCredentialStore::new(PathBuf::from(trimmed)));
        }
    }
    Box::new(KeyringStore)
}

/// Construct a file-backed credential store at an explicit path (helper for tests).
pub fn file_credential_store(path: &Path) -> Box<dyn CredentialStore> {
    Box::new(FileCredentialStore::new(path.to_path_buf()))
}
