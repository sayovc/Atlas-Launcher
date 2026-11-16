//! Read-only detection of other Minecraft installations whose verified files
//! Atlas may copy instead of downloading. Atlas never writes to these folders.

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    OfficialLauncher,
    OfficialLauncherFlatpak,
    PrismLauncher,
    MultiMc,
    Manual,
}

impl InstallKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::OfficialLauncher => "Official launcher",
            Self::OfficialLauncherFlatpak => "Official launcher (Flatpak)",
            Self::PrismLauncher => "Prism Launcher",
            Self::MultiMc => "MultiMC",
            Self::Manual => "Selected folder",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DetectedInstall {
    pub kind: InstallKind,
    pub path: PathBuf,
    pub has_assets: bool,
    pub has_libraries: bool,
    /// Version folders present (official layout only).
    pub versions: Vec<String>,
}

fn inspect(kind: InstallKind, path: PathBuf) -> Option<DetectedInstall> {
    if !path.is_dir() {
        return None;
    }
    let has_assets = path.join("assets").is_dir();
    let has_libraries = path.join("libraries").is_dir();
    if !has_assets && !has_libraries {
        return None;
    }
    let mut versions: Vec<String> = std::fs::read_dir(path.join("versions"))
        .map(|rd| rd.filter_map(Result::ok).filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    versions.sort();
    Some(DetectedInstall { kind, path, has_assets, has_libraries, versions })
}

/// Candidate locations, in priority order. `manual` (from settings) first.
pub fn candidates(manual: Option<&Path>) -> Vec<(InstallKind, PathBuf)> {
    let mut c = Vec::new();
    if let Some(m) = manual {
        c.push((InstallKind::Manual, m.to_path_buf()));
    }
    let Some(base) = directories::BaseDirs::new() else { return c };
    let home = base.home_dir();
    if cfg!(target_os = "windows") {
        c.push((InstallKind::OfficialLauncher, base.config_dir().join(".minecraft")));
    } else if cfg!(target_os = "macos") {
        c.push((InstallKind::OfficialLauncher, base.data_dir().join("minecraft")));
    } else {
        c.push((InstallKind::OfficialLauncher, home.join(".minecraft")));
        c.push((InstallKind::OfficialLauncherFlatpak, home.join(".var/app/com.mojang.Minecraft/.minecraft")));
        c.push((InstallKind::PrismLauncher, home.join(".var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher")));
    }
    c.push((InstallKind::PrismLauncher, base.data_dir().join("PrismLauncher")));
    c.push((InstallKind::MultiMc, base.data_dir().join("multimc")));
    c
}

pub fn detect_minecraft_installs(manual: Option<&Path>) -> Vec<DetectedInstall> {
    let mut seen = std::collections::HashSet::new();
    candidates(manual)
        .into_iter()
        .filter_map(|(k, p)| inspect(k, p))
        .filter(|d| seen.insert(std::fs::canonicalize(&d.path).unwrap_or(d.path.clone())))
        .collect()
}
