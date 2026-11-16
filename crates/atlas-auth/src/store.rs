//! Where tokens live. Production uses the OS keychain; tests (and the
//! "don't remember me" case) use memory.

use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use std::collections::HashMap;
use std::sync::Mutex;

pub trait SecretStore: Send + Sync {
    fn get(&self, account: &str) -> AtlasResult<Option<String>>;
    fn set(&self, account: &str, value: &str) -> AtlasResult<()>;
    fn delete(&self, account: &str) -> AtlasResult<()>;
}

const SERVICE: &str = "AtlasLauncher";

/// Windows Credential Manager, macOS Keychain, Secret Service on Linux.
pub struct KeyringStore;

fn kr_err(e: keyring::Error) -> AtlasError {
    AtlasError::new(ErrorCode::AuthFailed, "Your system's secure credential store isn't available.")
        .details(e.to_string())
        .suggest("On Linux, make sure a keyring service (GNOME Keyring, KWallet) is running. Atlas won't save sign-in tokens in plain files.")
}

impl SecretStore for KeyringStore {
    fn get(&self, account: &str) -> AtlasResult<Option<String>> {
        let e = keyring::Entry::new(SERVICE, account).map_err(kr_err)?;
        match e.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(kr_err(err)),
        }
    }
    fn set(&self, account: &str, value: &str) -> AtlasResult<()> {
        keyring::Entry::new(SERVICE, account).and_then(|e| e.set_password(value)).map_err(kr_err)
    }
    fn delete(&self, account: &str) -> AtlasResult<()> {
        match keyring::Entry::new(SERVICE, account).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(kr_err(e)),
        }
    }
}

#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, account: &str) -> AtlasResult<Option<String>> {
        Ok(self.0.lock().unwrap_or_else(|p| p.into_inner()).get(account).cloned())
    }
    fn set(&self, account: &str, value: &str) -> AtlasResult<()> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).insert(account.into(), value.into());
        Ok(())
    }
    fn delete(&self, account: &str) -> AtlasResult<()> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).remove(account);
        Ok(())
    }
}
