//! Saved accounts. `accounts.toml` holds the public bits; tokens sit in the
//! secret store keyed by account id.

use crate::flow::{McSession, MsAuth, MsTokens};
use crate::store::SecretStore;
use crate::Secret;
use atlas_shared::fs_safety::atomic_write;
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    /// Minecraft profile uuid (no dashes).
    pub id: String,
    pub name: String,
    pub added: u64,
    pub last_used: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct File {
    #[serde(default)]
    accounts: Vec<Account>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    ms_refresh: Secret,
    mc: McSession,
}

/// What a launch needs. Not Serialize on purpose.
#[derive(Debug, Clone)]
pub struct AccountSession {
    pub name: String,
    pub uuid: String,
    pub xuid: String,
    pub token: Secret,
}

pub struct Accounts {
    file: PathBuf,
    store: Arc<dyn SecretStore>,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Accounts {
    pub fn new(dir: PathBuf, store: Arc<dyn SecretStore>) -> Self {
        Self { file: dir.join("accounts.toml"), store }
    }

    fn read(&self) -> AtlasResult<File> {
        match std::fs::read_to_string(&self.file) {
            Ok(t) => toml::from_str(&t).map_err(|e| AtlasError::new(ErrorCode::ConfigInvalid, "The account list is damaged.").details(e).suggest("Delete accounts.toml and sign in again.")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(File::default()),
            Err(e) => Err(AtlasError::io(ErrorCode::ConfigIo, "read the account list", &self.file, e)),
        }
    }

    fn write(&self, f: &File) -> AtlasResult<()> {
        let t = toml::to_string_pretty(f).map_err(|e| AtlasError::new(ErrorCode::Internal, "Couldn't save accounts.").details(e))?;
        atomic_write(&self.file, t.as_bytes())
    }

    pub fn list(&self) -> AtlasResult<Vec<Account>> {
        Ok(self.read()?.accounts)
    }

    /// Store a fresh sign-in. Secrets go to the store first; if that fails we
    /// don't add the account at all rather than keep it half-saved.
    pub fn add(&self, ms: &MsTokens, mc: McSession) -> AtlasResult<Account> {
        let stored = Stored { ms_refresh: ms.refresh.clone(), mc: mc.clone() };
        let json = serde_json::to_string(&stored).map_err(|e| AtlasError::new(ErrorCode::Internal, "Couldn't encode tokens.").details(e))?;
        self.store.set(&mc.uuid, &json)?;
        let mut f = self.read()?;
        f.accounts.retain(|a| a.id != mc.uuid);
        let acc = Account { id: mc.uuid.clone(), name: mc.name.clone(), added: now(), last_used: Some(now()) };
        f.accounts.insert(0, acc.clone());
        self.write(&f)?;
        tracing::info!(account = %acc.name, "account added");
        Ok(acc)
    }

    pub fn remove(&self, id: &str) -> AtlasResult<()> {
        let mut f = self.read()?;
        let before = f.accounts.len();
        f.accounts.retain(|a| a.id != id);
        if f.accounts.len() == before {
            return Err(AtlasError::new(ErrorCode::InvalidArgument, "No such account."));
        }
        self.store.delete(id)?;
        self.write(&f)?;
        tracing::info!("account removed");
        Ok(())
    }

    /// A launch-ready session, refreshing tokens if the game token is near expiry.
    pub async fn session(&self, id: &str, auth: Option<&MsAuth>) -> AtlasResult<AccountSession> {
        let raw = self.store.get(id)?.ok_or_else(|| {
            AtlasError::new(ErrorCode::AuthExpired, "This account's sign-in isn't saved on this computer anymore.").suggest("Sign in again.")
        })?;
        let mut stored: Stored = serde_json::from_str(&raw).map_err(|_| AtlasError::new(ErrorCode::AuthExpired, "Saved sign-in is unreadable.").suggest("Sign in again."))?;
        if stored.mc.expires_at < now() + 300 {
            let auth = auth.ok_or_else(|| AtlasError::new(ErrorCode::AuthExpired, "Your sign-in expired.").suggest("Sign in again."))?;
            tracing::info!("refreshing sign-in");
            let ms = auth.refresh(&stored.ms_refresh).await?;
            let mc = auth.minecraft(&ms).await?;
            stored = Stored { ms_refresh: if ms.refresh.expose().is_empty() { stored.ms_refresh } else { ms.refresh }, mc };
            let json = serde_json::to_string(&stored).map_err(|e| AtlasError::new(ErrorCode::Internal, "Couldn't encode tokens.").details(e))?;
            self.store.set(id, &json)?;
        }
        let mut f = self.read()?;
        if let Some(a) = f.accounts.iter_mut().find(|a| a.id == id) {
            a.last_used = Some(now());
            a.name = stored.mc.name.clone();
        }
        self.write(&f)?;
        Ok(AccountSession { name: stored.mc.name, uuid: stored.mc.uuid, xuid: stored.mc.xuid, token: stored.mc.token })
    }
}
