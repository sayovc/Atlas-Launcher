//! Self-update, as seen by the app.

use crate::Atlas;
use atlas_downloads::{CancellationToken, Progress};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use atlas_updater::{Available, StartupOutcome, Updater};
use std::path::PathBuf;
use std::sync::Arc;

fn not_configured() -> AtlasError {
    AtlasError::new(ErrorCode::NotImplemented, "Automatic updates aren't set up in this build.")
        .suggest("Get new versions from the GitHub releases page. Official release builds check for updates automatically.")
}

impl Atlas {
    fn updates_dir(&self) -> PathBuf {
        self.paths().data_root.join("updates")
    }

    fn updater(&self) -> AtlasResult<Updater> {
        Updater::from_build(self.downloader_pub()?, self.updates_dir())?.ok_or_else(not_configured)
    }

    pub fn updates_available_in_build(&self) -> bool {
        atlas_updater::public_key().is_some()
    }

    /// Ask GitHub for a newer, properly signed Atlas.
    pub fn check_for_update(&self) -> AtlasResult<Option<Available>> {
        let up = self.updater()?;
        self.rt.block_on(up.check(atlas_shared::ATLAS_VERSION, &CancellationToken::new()))
    }

    /// Download, verify and swap in the update. Atlas needs a restart afterwards.
    pub fn install_update(&self, a: &Available) -> AtlasResult<PathBuf> {
        let up = self.updater()?;
        let staged = self.rt.block_on(up.download(a, Arc::new(Progress::new()), CancellationToken::new()))?;
        let exe = std::env::current_exe().map_err(|e| AtlasError::new(ErrorCode::Internal, "Can't find the Atlas program file.").details(e))?;
        atlas_updater::apply(&self.updates_dir(), &staged, &exe, &a.version)?;
        Ok(exe)
    }

    pub fn rollback_update(&self) -> AtlasResult<()> {
        let exe = std::env::current_exe().map_err(|e| AtlasError::new(ErrorCode::Internal, "Can't find the Atlas program file.").details(e))?;
        atlas_updater::rollback(&self.updates_dir(), &exe)
    }

    /// Run once at startup (GUI). Rolls back an update that keeps failing.
    pub fn update_startup_check(&self) -> StartupOutcome {
        match std::env::current_exe() {
            Ok(exe) => atlas_updater::startup_check(&self.updates_dir(), &exe),
            Err(_) => StartupOutcome::Normal,
        }
    }

    pub fn mark_update_healthy(&self) {
        atlas_updater::mark_healthy(&self.updates_dir());
    }
}
