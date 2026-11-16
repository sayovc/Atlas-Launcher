//! Microsoft sign-in for Minecraft: Java Edition.
//!
//! The chain is Microsoft OAuth -> Xbox Live -> XSTS -> Minecraft services.
//! We never see a password: the user signs in on Microsoft's own page in
//! their browser (or with a device code on another screen).
//!
//! Secrets (refresh token, game token) only ever go to the OS keychain via
//! [`store::SecretStore`]. The account list on disk has names and ids only.

pub mod accounts;
pub mod flow;
pub mod store;

pub use accounts::{Account, AccountSession, Accounts};
pub use flow::{BrowserLogin, DeviceCode, Endpoints, MsAuth, MsTokens, McSession};
pub use store::{KeyringStore, MemoryStore, SecretStore};

/// Azure app (client) id. Baked in at build time with `ATLAS_MS_CLIENT_ID`,
/// or overridden at runtime by the same environment variable.
pub fn client_id() -> Option<String> {
    std::env::var("ATLAS_MS_CLIENT_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| option_env!("ATLAS_MS_CLIENT_ID").map(str::to_owned))
        .or_else(|| BUILTIN_CLIENT_ID.map(str::to_owned))
}

/// Set this once the Azure app is registered.
pub const BUILTIN_CLIENT_ID: Option<&str> = None;

/// Wrapper that keeps tokens out of Debug output.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests;
