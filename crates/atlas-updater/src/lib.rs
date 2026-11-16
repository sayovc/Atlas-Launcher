//! Self-update.
//!
//! A release publishes `atlas-update.json` plus `atlas-update.json.sig`
//! (an Ed25519 signature over the exact JSON bytes). We only trust a
//! manifest signed by the key compiled into this build, never go
//! backwards in version, and check the binary's SHA-256 before it
//! replaces anything. The old binary is kept so a bad update can be
//! rolled back, automatically if the new one keeps failing to start.

use atlas_downloads::{CancellationToken, Checksum, DownloadRequest, Downloader, Progress};
use atlas_shared::fs_safety::atomic_write;
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const MANIFEST_URL: &str = "https://github.com/sayovc/Atlas-Launcher/releases/latest/download/atlas-update.json";

/// Base64 Ed25519 public key for release signing. Set at build time with
/// ATLAS_UPDATE_PUBKEY (see `cargo xtask keygen`), or paste it here.
pub const BUILTIN_PUBLIC_KEY: Option<&str> = None;

pub fn public_key() -> Option<String> {
    option_env!("ATLAS_UPDATE_PUBKEY").map(str::to_owned).or_else(|| BUILTIN_PUBLIC_KEY.map(str::to_owned))
}

/// e.g. "x86_64-linux", "aarch64-macos", "x86_64-windows"
pub fn platform_key() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub published: String,
    pub assets: BTreeMap<String, Asset>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Available {
    pub version: String,
    pub notes: String,
    pub asset: Asset,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct PendingMarker {
    version: String,
    attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum StartupOutcome {
    Normal,
    /// Running a freshly installed update; call `mark_healthy` once things look fine.
    TryingUpdate(String),
    /// The update kept failing, so the previous binary was put back.
    RolledBack(String),
}

/// "1.2.10" > "1.2.9"; a pre-release suffix sorts before the plain version.
pub fn newer(candidate: &str, current: &str) -> bool {
    fn parse(v: &str) -> (Vec<u64>, bool) {
        let v = v.trim_start_matches('v');
        let (core, pre) = match v.split_once('-') {
            Some((c, _)) => (c, true),
            None => (v, false),
        };
        (core.split('.').map(|p| p.parse().unwrap_or(0)).collect(), pre)
    }
    let (a, apre) = parse(candidate);
    let (b, bpre) = parse(current);
    let n = a.len().max(b.len());
    for i in 0..n {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    !apre && bpre
}

fn verify_err(msg: &str, d: impl std::fmt::Display) -> AtlasError {
    AtlasError::new(ErrorCode::UpdateVerificationFailed, msg.to_string())
        .details(d.to_string())
        .suggest("Nothing was installed. Download Atlas from the official GitHub releases page if this keeps happening.")
}

pub fn parse_key(b64: &str) -> AtlasResult<VerifyingKey> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|e| verify_err("The update key in this build is invalid.", e))?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| verify_err("The update key in this build is invalid.", "wrong length"))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| verify_err("The update key in this build is invalid.", e))
}

/// Check the signature over the raw bytes, then parse.
pub fn verify_manifest(key: &VerifyingKey, manifest: &[u8], sig_b64: &[u8]) -> AtlasResult<Manifest> {
    let sig_raw = base64::engine::general_purpose::STANDARD
        .decode(String::from_utf8_lossy(sig_b64).trim())
        .map_err(|e| verify_err("The update signature is unreadable.", e))?;
    let sig = Signature::from_slice(&sig_raw).map_err(|e| verify_err("The update signature is unreadable.", e))?;
    key.verify(manifest, &sig).map_err(|e| verify_err("The update isn't signed by the Atlas release key.", e))?;
    serde_json::from_slice(manifest).map_err(|e| verify_err("The update manifest is malformed.", e))
}

pub struct Updater {
    pub dl: Downloader,
    pub key: VerifyingKey,
    pub manifest_url: String,
    pub dir: PathBuf,
}

impl Updater {
    /// None when this build has no release key (self-update is then off).
    pub fn from_build(dl: Downloader, dir: PathBuf) -> AtlasResult<Option<Self>> {
        let Some(k) = public_key() else { return Ok(None) };
        Ok(Some(Self { dl, key: parse_key(&k)?, manifest_url: MANIFEST_URL.into(), dir }))
    }

    pub async fn check(&self, current: &str, cancel: &CancellationToken) -> AtlasResult<Option<Available>> {
        let body = self.dl.fetch_bytes(&self.manifest_url, cancel).await?;
        let sig = self.dl.fetch_bytes(&format!("{}.sig", self.manifest_url), cancel).await?;
        let m = verify_manifest(&self.key, &body, &sig)?;
        if !newer(&m.version, current) {
            return Ok(None);
        }
        let asset = m.assets.get(&platform_key()).cloned().ok_or_else(|| {
            AtlasError::new(ErrorCode::UpdateVerificationFailed, format!("Atlas {} has no build for this platform ({}).", m.version, platform_key())).suggest("Check the releases page.")
        })?;
        if asset.sha256.len() != 64 || !asset.url.starts_with("https://") {
            return Err(verify_err("The update manifest lists a bad download.", &asset.url));
        }
        Ok(Some(Available { version: m.version, notes: m.notes, asset }))
    }

    /// Download and verify into `updates/<version>/`.
    pub async fn download(&self, a: &Available, progress: Arc<Progress>, cancel: CancellationToken) -> AtlasResult<PathBuf> {
        let name = if cfg!(windows) { "atlas-launcher.exe" } else { "atlas-launcher" };
        let dest = self.dir.join(&a.version).join(name);
        let req = DownloadRequest { url: a.asset.url.clone(), dest: dest.clone(), checksum: Checksum::Sha256(a.asset.sha256.to_ascii_lowercase()), size: Some(a.asset.size), reuse_from: vec![] };
        self.dl.download_all(vec![req], progress, cancel).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).map_err(|e| AtlasError::io(ErrorCode::UpdateVerificationFailed, "mark the update executable", &dest, e))?;
        }
        Ok(dest)
    }
}

fn marker(dir: &Path) -> PathBuf {
    dir.join("pending.json")
}

fn backup_path(dir: &Path, target: &Path) -> PathBuf {
    dir.join("previous").join(target.file_name().unwrap_or_default())
}

/// Swap `staged` in for `target`, keeping a copy of the old one.
pub fn apply(dir: &Path, staged: &Path, target: &Path, version: &str) -> AtlasResult<()> {
    let backup = backup_path(dir, target);
    std::fs::create_dir_all(backup.parent().unwrap_or(dir)).map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "create the updates folder", dir, e))?;
    std::fs::copy(target, &backup).map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "back up the current version", target, e))?;
    let is_self = std::env::current_exe().ok().and_then(|c| std::fs::canonicalize(c).ok()) == std::fs::canonicalize(target).ok();
    if is_self {
        // handles "can't overwrite a running exe" on Windows
        self_replace::self_replace(staged).map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "replace Atlas", target, e))?;
    } else {
        let tmp = target.with_extension("new");
        std::fs::copy(staged, &tmp).map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "copy the update", &tmp, e))?;
        std::fs::rename(&tmp, target).map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "replace Atlas", target, e))?;
    }
    let m = serde_json::to_vec(&PendingMarker { version: version.into(), attempts: 0 }).unwrap_or_default();
    atomic_write(&marker(dir), &m)?;
    tracing::info!(version, "update applied; will confirm on next start");
    Ok(())
}

pub fn rollback(dir: &Path, target: &Path) -> AtlasResult<()> {
    let backup = backup_path(dir, target);
    if !backup.is_file() {
        return Err(AtlasError::new(ErrorCode::InvalidArgument, "There's no previous version to go back to."));
    }
    let is_self = std::env::current_exe().ok().and_then(|c| std::fs::canonicalize(c).ok()) == std::fs::canonicalize(target).ok();
    if is_self {
        self_replace::self_replace(&backup).map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "restore the previous version", target, e))?;
    } else {
        std::fs::copy(&backup, target).map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "restore the previous version", target, e))?;
    }
    let _ = std::fs::remove_file(marker(dir));
    tracing::warn!("rolled back to the previous version");
    Ok(())
}

/// Call early at startup. After 3 starts without `mark_healthy`, roll back.
pub fn startup_check(dir: &Path, target: &Path) -> StartupOutcome {
    let Some(mut m) = std::fs::read(marker(dir)).ok().and_then(|b| serde_json::from_slice::<PendingMarker>(&b).ok()) else {
        return StartupOutcome::Normal;
    };
    m.attempts += 1;
    if m.attempts >= 3 {
        return match rollback(dir, target) {
            Ok(()) => StartupOutcome::RolledBack(m.version),
            Err(_) => StartupOutcome::Normal,
        };
    }
    let _ = atomic_write(&marker(dir), &serde_json::to_vec(&m).unwrap_or_default());
    StartupOutcome::TryingUpdate(m.version)
}

pub fn mark_healthy(dir: &Path) {
    if std::fs::remove_file(marker(dir)).is_ok() {
        tracing::info!("update confirmed working");
    }
}

/// Restart into the (new) binary.
pub fn restart(target: &Path) -> AtlasResult<()> {
    std::process::Command::new(target).spawn().map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "restart Atlas", target, e))?;
    std::process::exit(0);
}

#[cfg(test)]
mod tests;
