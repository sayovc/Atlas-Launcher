use crate::Atlas;
use atlas_shared::{AtlasError, ATLAS_VERSION};
use serde::Serialize;

/// What the Play button shows. Only the backend sets this.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GameState {
    /// No installation exists yet.
    NoInstallation,
    /// Installation exists but its game files are missing.
    NotInstalled,
    Installing { progress: f32 },
    Preparing { progress: f32 },
    Updating { progress: f32 },
    Ready,
    Launching,
    Running { pid: u32 },
    Stopping,
    Error { error: AtlasError },
}

impl GameState {
    /// Label for the primary action button.
    pub fn action_label(&self) -> &'static str {
        match self {
            Self::NoInstallation => "Create installation",
            Self::NotInstalled => "Install",
            Self::Installing { .. } => "Installing…",
            Self::Preparing { .. } => "Preparing…",
            Self::Updating { .. } => "Updating…",
            Self::Ready => "Play",
            Self::Launching => "Launching…",
            Self::Running { .. } => "Stop",
            Self::Stopping => "Stopping…",
            Self::Error { .. } => "Retry",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusReport {
    pub atlas_version: &'static str,
    pub data_root: std::path::PathBuf,
    /// Name of the signed-in account, if any.
    pub account: Option<String>,
    pub installation: Option<String>,
    pub installation_id: Option<String>,
    pub minecraft_version: Option<String>,
    pub game_state: GameState,
    pub installed_versions: usize,
    /// Newest release not installed yet (from the cached version list).
    pub version_update: Option<String>,
    /// Things this build can't do (e.g. self-update without a release key).
    pub unavailable: Vec<&'static str>,
    pub warnings: Vec<AtlasError>,
}

pub(crate) fn compute(atlas: &Atlas) -> StatusReport {
    let paths = atlas.paths();
    let sel = atlas.selected_installation();
    StatusReport {
        atlas_version: ATLAS_VERSION,
        data_root: paths.data_root,
        account: atlas.active_account_name(),
        installation: sel.as_ref().map(|i| i.name.clone()),
        installation_id: sel.as_ref().map(|i| i.id.clone()),
        minecraft_version: sel.as_ref().map(|i| i.version.clone()),
        game_state: atlas.game_state(),
        installed_versions: atlas.installed_versions().map(|v| v.len()).unwrap_or(0),
        version_update: atlas.version_update(),
        unavailable: if atlas.updates_available_in_build() { vec![] } else { vec!["self_update"] },
        warnings: atlas.startup_warnings().to_vec(),
    }
}
