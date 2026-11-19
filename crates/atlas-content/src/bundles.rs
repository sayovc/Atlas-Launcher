//! Bundles: hand-picked sets of open-source mods you can add in one click.
//!
//! They're resolved against Modrinth for whatever Minecraft version the
//! installation runs, so the same bundle works on 1.20 and 26.x. Mods that
//! don't have a build for that version yet are skipped and listed, never
//! swapped for something else behind your back.

use crate::modrinth::{InstallReport, Modrinth, ProjectType, Target};
use atlas_downloads::{CancellationToken, Downloader, Progress};
use atlas_shared::AtlasResult;
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
pub struct Bundle {
    pub id: &'static str,
    pub name: &'static str,
    pub blurb: &'static str,
    /// Modrinth slugs. All of these are Fabric mods.
    pub mods: &'static [&'static str],
    /// Picked by default in the one-click setup.
    pub default_on: bool,
}

pub const BUNDLES: &[Bundle] = &[
    Bundle {
        id: "performance",
        name: "Performance",
        blurb: "More FPS, less memory, faster loading.",
        mods: &["sodium", "lithium", "ferrite-core", "entityculling", "immediatelyfast", "modernfix", "dynamic-fps"],
        default_on: true,
    },
    Bundle {
        id: "shaders",
        name: "Shaders & visuals",
        blurb: "Shader pack support, connected textures, dynamic lights.",
        mods: &["iris", "continuity", "lambdynamiclights"],
        default_on: false,
    },
    Bundle {
        id: "qol",
        name: "Quality of life",
        blurb: "Mod menu, zoom, better inventory handling, food info.",
        mods: &["modmenu", "zoomify", "mouse-tweaks", "appleskin"],
        default_on: true,
    },
    Bundle {
        id: "info",
        name: "Info & maps",
        blurb: "Minimap, what-am-I-looking-at tooltips, cleaner F3.",
        mods: &["xaeros-minimap", "jade", "betterf3"],
        default_on: false,
    },
];

pub fn get(id: &str) -> Option<&'static Bundle> {
    BUNDLES.iter().find(|b| b.id == id)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct BundleReport {
    pub installed: Vec<String>,
    pub already_had: Vec<String>,
    /// Mods with no build for this version/loader (yet).
    pub skipped: Vec<String>,
}

impl BundleReport {
    fn absorb(&mut self, r: InstallReport) {
        self.installed.extend(r.installed.into_iter().map(|i| i.title));
        self.already_had.extend(r.already_had);
        self.skipped.extend(r.skipped);
    }
}

/// Install every mod in the given bundles. One mod missing doesn't stop the rest.
pub async fn install(mr: &Modrinth, dl: &Downloader, ids: &[&str], t: &Target<'_>, progress: Arc<Progress>, cancel: CancellationToken) -> AtlasResult<BundleReport> {
    let mut rep = BundleReport::default();
    for id in ids {
        let Some(b) = get(id) else { continue };
        for slug in b.mods {
            if cancel.is_cancelled() {
                return Err(atlas_shared::AtlasError::cancelled());
            }
            match mr.install(dl, slug, ProjectType::Mod, t, progress.clone(), cancel.clone()).await {
                Ok(r) => rep.absorb(r),
                Err(e) if e.code == atlas_shared::ErrorCode::VersionNotFound => rep.skipped.push(format!("{slug} (no build for {} yet)", t.game_version)),
                Err(e) => return Err(e),
            }
        }
    }
    rep.installed.sort();
    rep.installed.dedup();
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_are_sane() {
        let mut seen = std::collections::HashSet::new();
        for b in BUNDLES {
            assert!(!b.mods.is_empty());
            for m in b.mods {
                assert!(seen.insert(*m), "{m} is in two bundles");
                assert!(m.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{m}");
            }
        }
        assert!(get("performance").is_some());
        assert!(BUNDLES.iter().any(|b| b.default_on));
    }
}
