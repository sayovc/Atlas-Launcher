//! `install.toml`: the record that makes a version count as installed.

use atlas_downloads::{verify_file, Checksum};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::model::VersionKind;

pub const RECORD_FILE: &str = "install.toml";
pub const RECORD_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordedFile {
    /// Relative to the data root, `/`-separated.
    pub path: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallRecord {
    pub schema: u32,
    pub id: String,
    pub kind: VersionKind,
    pub source: String,
    pub release_time: Option<String>,
    pub installed_at: u64,
    pub java_major: Option<u32>,
    pub main_class: String,
    pub asset_index: String,
    pub files: Vec<RecordedFile>,
}

/// Public summary of an installed version.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InstalledVersion {
    pub id: String,
    pub kind: VersionKind,
    pub release_time: Option<String>,
    pub installed_at: u64,
    pub java_major: Option<u32>,
    pub files: usize,
    pub bytes: u64,
    pub dir: PathBuf,
}

impl InstallRecord {
    pub fn summary(&self, dir: PathBuf) -> InstalledVersion {
        InstalledVersion {
            id: self.id.clone(),
            kind: self.kind,
            release_time: self.release_time.clone(),
            installed_at: self.installed_at,
            java_major: self.java_major,
            files: self.files.len(),
            bytes: self.files.iter().map(|f| f.size).sum(),
            dir,
        }
    }

    pub fn load(dir: &Path) -> AtlasResult<Option<Self>> {
        let p = dir.join(RECORD_FILE);
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(AtlasError::io(ErrorCode::InvalidInstallation, "read a version record", &p, e)),
        };
        let r: Self = toml::from_str(&text).map_err(|e| {
            AtlasError::new(ErrorCode::InvalidInstallation, "A version's install record is damaged.")
                .details(format!("{}: {e}", p.display()))
                .suggest("Repair or reinstall the version.")
        })?;
        Ok(Some(r))
    }

    pub fn save(&self, dir: &Path) -> AtlasResult<()> {
        let text = toml::to_string_pretty(self)
            .map_err(|e| AtlasError::new(ErrorCode::Internal, "Could not encode install record.").details(e))?;
        atlas_shared::fs_safety::atomic_write(&dir.join(RECORD_FILE), text.as_bytes())
    }

    /// Hash every recorded file. Paths are re-validated so a tampered record
    /// can't point verification outside the data root.
    pub fn verify(&self, data_root: &Path) -> AtlasResult<VerifyReport> {
        let mut rep = VerifyReport { id: self.id.clone(), checked: 0, missing: vec![], corrupt: vec![] };
        for f in &self.files {
            let p = atlas_shared::fs_safety::safe_join(data_root, &f.path)?;
            rep.checked += 1;
            if !p.exists() {
                rep.missing.push(f.path.clone());
            } else if !verify_file(&p, &Checksum::Sha1(f.sha1.clone()), Some(f.size))? {
                rep.corrupt.push(f.path.clone());
            }
        }
        Ok(rep)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VerifyReport {
    pub id: String,
    pub checked: usize,
    pub missing: Vec<String>,
    pub corrupt: Vec<String>,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.missing.is_empty() && self.corrupt.is_empty()
    }
}
