//! `atlas-core`: the one backend every Atlas front-end talks to.
//!
//! The GUI (`atlas-ui`), the CLI (`atlas-cli`) and the future MCP server all
//! hold an [`Atlas`] and call its methods. No front-end contains business
//! logic of its own; they render [`Atlas`] state and forward user intent.

mod accounts;
mod content;
mod data_root;
mod play;
mod status;
mod versions;

pub use data_root::MoveReport;
pub use play::PlayOptions;
pub use atlas_content::{mods::{Loader, ModInfo}, mrpack::PackSummary, packs::{PackInfo, PackKind}, ping::PingResult, servers::Server, worlds::WorldInfo};
pub use accounts::LoginState;
pub use atlas_auth::{Account, DeviceCode};
pub use atlas_instances::{Changes as InstallationChanges, Installation};
pub use atlas_launch::GameExit;
pub use status::{GameState, StatusReport};
pub use versions::{JobId, JobKind, JobSnapshot, JobState, VersionList, VersionRow};
pub use atlas_versions::{DetectedInstall, InstalledVersion, VerifyReport, VersionKind};
pub use atlas_downloads::ProgressSnapshot;

use atlas_config::{AtlasPaths, Settings, SettingsStore};
use atlas_logging::{LogBuffer, LogConfig, LogFilter, LogHandle, LogRecord};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use std::path::Path;
use std::sync::RwLock;

/// Explicit consent token for destructive operations. Front-ends must obtain
/// it from the user (dialog, `--yes`, MCP confirmation round-trip).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    Confirmed,
    NotConfirmed,
}

impl Confirmation {
    pub fn from_bool(yes: bool) -> Self {
        if yes { Self::Confirmed } else { Self::NotConfirmed }
    }
    pub(crate) fn require(self, action: &str) -> AtlasResult<()> {
        match self {
            Self::Confirmed => Ok(()),
            Self::NotConfirmed => Err(AtlasError::confirmation_required(action)),
        }
    }
}

/// The Atlas backend.
pub struct Atlas {
    state: RwLock<State>,
    startup_warnings: Vec<AtlasError>,
    log_buffer: Option<LogBuffer>,
    pub(crate) rt: tokio::runtime::Runtime,
    pub(crate) jobs: std::sync::Arc<std::sync::Mutex<versions::Jobs>>,
    pub(crate) session: play::SessionRef,
    pub(crate) login: accounts::LoginRef,
    pub(crate) memory_secrets: std::sync::Arc<dyn atlas_auth::SecretStore>,
}

struct State {
    paths: AtlasPaths,
    store: SettingsStore,
    settings: Settings,
}

impl Atlas {
    /// Open Atlas on resolved paths: create managed directories, load settings.
    pub fn open(paths: AtlasPaths) -> AtlasResult<Self> {
        paths.ensure()?;
        let store = SettingsStore::new(paths.settings_file());
        let outcome = store.load()?;
        for w in &outcome.warnings {
            tracing::warn!(code = %w.code, details = %w.details, "{}", w.message);
        }
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("atlas-worker")
            .enable_all()
            .build()
            .map_err(|e| AtlasError::new(ErrorCode::Internal, "Could not start background workers.").details(e))?;
        Ok(Self {
            state: RwLock::new(State { paths, store, settings: outcome.settings }),
            startup_warnings: outcome.warnings,
            log_buffer: None,
            rt,
            jobs: Default::default(),
            session: Default::default(),
            login: Default::default(),
            memory_secrets: accounts::memory_store(),
        })
    }

    /// Resolve default paths (env → locator → platform) and open.
    pub fn open_default() -> AtlasResult<Self> {
        Self::open(AtlasPaths::resolve()?)
    }

    /// Install global logging using this instance's paths and settings and
    /// attach the in-memory buffer for live viewing.
    pub fn init_logging(&mut self, stderr: bool) -> AtlasResult<LogHandle> {
        let (dir, settings) = {
            let s = self.read();
            (s.paths.launcher_logs_dir(), s.settings.clone())
        };
        let cfg = LogConfig {
            dir: dir.clone(),
            level: std::env::var("ATLAS_LOG").unwrap_or_else(|_| {
                // Keep third-party windowing/graphics crates quiet unless debugging.
                let lvl = settings.advanced.log_level.as_filter();
                format!("{lvl},winit=warn,sctk=warn,calloop=warn,egui_winit=warn,eframe=warn,egui_glow=warn,zbus=warn,tracing=warn")
            }),
            keep_files: settings.advanced.keep_log_files as usize,
            stderr,
            buffer_capacity: 10_000,
        };
        let handle = atlas_logging::init(&cfg).map_err(|e| {
            AtlasError::new(ErrorCode::ConfigIo, "Could not start logging.")
                .details(format!("{}: {e}", dir.display()))
                .suggest("Check that the Atlas data folder is writable.")
        })?;
        self.log_buffer = Some(handle.buffer.clone());
        tracing::info!(version = atlas_shared::ATLAS_VERSION, data_root = %self.paths().data_root.display(), "Atlas started");
        Ok(handle)
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(|p| p.into_inner())
    }
    fn write(&self) -> std::sync::RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(|p| p.into_inner())
    }

    pub fn paths(&self) -> AtlasPaths {
        self.read().paths.clone()
    }

    pub fn startup_warnings(&self) -> &[AtlasError] {
        &self.startup_warnings
    }

    // ---------------------------------------------------------------- settings

    pub fn settings(&self) -> Settings {
        self.read().settings.clone()
    }

    /// Replace settings wholesale (validated, persisted atomically).
    pub fn save_settings(&self, new: Settings) -> AtlasResult<Settings> {
        let mut st = self.write();
        st.store.save(&new)?;
        if st.settings.advanced.ai != new.advanced.ai {
            tracing::info!(ai_enabled = new.advanced.ai.enabled, mcp_enabled = new.advanced.ai.mcp_enabled, "AI settings changed");
        }
        st.settings = new.clone();
        tracing::info!("settings saved");
        Ok(new)
    }

    /// Set one setting by dotted key (`memory.max_mb`).
    pub fn set_setting(&self, key: &str, raw_value: &str) -> AtlasResult<Settings> {
        let updated = self.settings().with_key(key, raw_value)?;
        self.save_settings(updated)
    }

    pub fn get_setting(&self, key: &str) -> AtlasResult<toml::Value> {
        self.settings().get_key(key)
    }

    /// Reset all settings to defaults. Destructive: needs confirmation.
    pub fn reset_settings(&self, confirm: Confirmation) -> AtlasResult<Settings> {
        confirm.require("reset settings")?;
        tracing::warn!("settings reset to defaults");
        self.save_settings(Settings::default())
    }

    // ------------------------------------------------------------------ status

    /// The real backend state for the Home page and `atlas status`.
    pub fn status(&self) -> StatusReport {
        status::compute(self)
    }

    // -------------------------------------------------------------------- logs

    /// Live records from this process (GUI). Empty if logging wasn't initialised.
    pub fn live_logs(&self, filter: &LogFilter) -> Vec<LogRecord> {
        self.log_buffer.as_ref().map(|b| b.snapshot(filter)).unwrap_or_default()
    }

    pub fn live_log_generation(&self) -> u64 {
        self.log_buffer.as_ref().map(|b| b.generation()).unwrap_or(0)
    }

    /// Records persisted on disk (any process).
    pub fn file_logs(&self, filter: &LogFilter, limit: usize) -> AtlasResult<Vec<LogRecord>> {
        let dir = self.paths().launcher_logs_dir();
        atlas_logging::read_log_dir(&dir, filter, limit)
            .map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "read launcher logs", &dir, e))
    }

    // --------------------------------------------------------------- data root

    /// Move every Atlas-managed item to `new_root`, verify the copy, repoint
    /// the locator, then remove the originals. See [`data_root`] for the
    /// safety rules.
    pub fn move_data_root(&self, new_root: &Path, confirm: Confirmation) -> AtlasResult<MoveReport> {
        confirm.require("move the Atlas data folder")?;
        if self.jobs.lock().unwrap_or_else(|p| p.into_inner()).any_running() {
            return Err(AtlasError::new(ErrorCode::DataDirInvalid, "Downloads are running.")
                .suggest("Wait for installs and repairs to finish (or cancel them), then move the data folder."));
        }
        let mut st = self.write();
        if std::env::var_os(atlas_config::paths::ENV_DATA_DIR).is_some_and(|v| !v.is_empty()) {
            return Err(AtlasError::new(ErrorCode::DataDirInvalid, "The data folder is fixed by the ATLAS_DATA_DIR environment variable.")
                .suggest("Unset ATLAS_DATA_DIR to move the data folder from Atlas."));
        }
        let report = data_root::move_root(&st.paths, new_root)?;
        let new_paths = AtlasPaths::explicit(st.paths.bootstrap_dir.clone(), report.new_root.clone());
        st.store = SettingsStore::new(new_paths.settings_file());
        st.paths = new_paths;
        Ok(report)
    }
}

#[cfg(test)]
mod tests;
