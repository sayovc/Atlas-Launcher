//! Installing Minecraft versions from Mojang's official metadata.
//!
//! The manifest comes over HTTPS, and from there every file is checked
//! against a SHA-1 given by the file before it: manifest, version JSON,
//! then the jar, libraries, asset index and assets.
//!
//! On disk (inside the data folder):
//! ```text
//! versions/<id>/            installed (only once install.toml is written)
//! versions/.staging/<id>/   install in progress, resumable
//! libraries/, assets/       shared between versions
//! cache/manifest/           last version list, for offline use
//! ```

pub mod detect;
mod forge;
pub mod java;
pub mod loaders;
pub mod model;
mod plan;
mod record;
pub mod rules;
mod service;

pub use loaders::{LoaderKind, LoaderVersion};
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
