//! Game version management for Minecraft: Java Edition.
//!
//! Source of truth: Mojang's official version manifest (fetched over HTTPS).
//! Every subsequent file is verified against a SHA-1 published by the
//! previous link in the chain:
//!
//! manifest (HTTPS) → version JSON (sha1) → client jar, libraries,
//! asset index, logging config (sha1) → asset objects (sha1 = file name).
//!
//! Layout inside the Atlas data root:
//! ```text
//! versions/<id>/<id>.json, <id>.jar, install.toml   ← installed only when install.toml exists
//! versions/.staging/<id>/                            ← in-progress install (resumable)
//! libraries/<maven path>                             ← shared, verified
//! assets/indexes/<id>.json, assets/objects/<hh>/<hash>, assets/log_configs/<file>
//! cache/manifest/version_manifest_v2.json            ← offline fallback
//! ```

pub mod detect;
pub mod java;
pub mod model;
mod plan;
mod record;
pub mod rules;
mod service;

pub use detect::{detect_minecraft_installs, DetectedInstall, InstallKind};
pub use model::{Manifest, ManifestEntry, VersionKind};
pub use record::{InstallRecord, InstalledVersion, VerifyReport};
pub use service::{VersionPaths, VersionService, VersionSources};

/// Validate an identifier that becomes a file/folder name (version ids,
/// asset index ids). Mojang ids look like `1.21.4`, `24w14a`, `1.0`, `rd-132211`.
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id != "."
        && id != ".."
        && !id.starts_with('.')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+' | ' '))
}

pub(crate) fn valid_sha1(h: &str) -> bool {
    h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests;
