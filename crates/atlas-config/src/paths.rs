use atlas_shared::{fs_safety::atomic_write, AtlasError, AtlasResult, ErrorCode};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Folder name used under the platform base directories.
/// Linux follows XDG lowercase convention; other platforms use the product casing.
#[cfg(target_os = "linux")]
pub const APP_DIR_NAME: &str = "atlaslauncher";
#[cfg(not(target_os = "linux"))]
pub const APP_DIR_NAME: &str = "AtlasLauncher";

/// Environment overrides (portable installs, tests).
pub const ENV_BOOTSTRAP_DIR: &str = "ATLAS_CONFIG_DIR";
pub const ENV_DATA_DIR: &str = "ATLAS_DATA_DIR";

const LOCATOR_FILE: &str = "locator.toml";
const LOCATOR_SCHEMA: u32 = 1;

/// Contents of `locator.toml` in the bootstrap dir.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Locator {
    #[serde(default = "locator_schema")]
    pub schema: u32,
    /// Custom data root chosen by the user. `None` = platform default.
    pub data_root: Option<PathBuf>,
}

fn locator_schema() -> u32 {
    LOCATOR_SCHEMA
}

/// Every path Atlas uses, resolved once at startup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AtlasPaths {
    pub bootstrap_dir: PathBuf,
    pub data_root: PathBuf,
    /// True when `data_root` came from the locator or env, not the default.
    pub data_root_is_custom: bool,
}

/// Subdirectories of the data root that Atlas owns and will move/manage.
/// Anything else found in the data root is left untouched.
pub const MANAGED_SUBDIRS: &[&str] = &[
    "installations",
    "versions",
    "cache",
    "logs",
    "accounts",
    "runtimes",
    "libraries",
    "assets",
];
/// Files in the data root that Atlas owns.
pub const MANAGED_FILES: &[&str] = &["settings.toml"];

impl AtlasPaths {
    /// Resolve paths from env overrides, then `locator.toml`, then platform defaults.
    pub fn resolve() -> AtlasResult<Self> {
        let bootstrap_dir = match std::env::var_os(ENV_BOOTSTRAP_DIR) {
            Some(p) if !p.is_empty() => PathBuf::from(p),
            _ => default_bootstrap_dir()?,
        };
        if let Some(p) = std::env::var_os(ENV_DATA_DIR).filter(|p| !p.is_empty()) {
            return Ok(Self { bootstrap_dir, data_root: PathBuf::from(p), data_root_is_custom: true });
        }
        Self::from_bootstrap(bootstrap_dir)
    }

    /// Resolve from an explicit bootstrap dir (reads its locator).
    pub fn from_bootstrap(bootstrap_dir: PathBuf) -> AtlasResult<Self> {
        let locator = read_locator(&bootstrap_dir)?;
        match locator.data_root {
            Some(root) => Ok(Self { bootstrap_dir, data_root: root, data_root_is_custom: true }),
            None => Ok(Self { bootstrap_dir, data_root: default_data_root()?, data_root_is_custom: false }),
        }
    }

    /// Fully explicit construction (tests, portable mode).
    pub fn explicit(bootstrap_dir: PathBuf, data_root: PathBuf) -> Self {
        Self { bootstrap_dir, data_root, data_root_is_custom: true }
    }

    pub fn locator_file(&self) -> PathBuf { self.bootstrap_dir.join(LOCATOR_FILE) }
    pub fn settings_file(&self) -> PathBuf { self.data_root.join("settings.toml") }
    pub fn logs_dir(&self) -> PathBuf { self.data_root.join("logs") }
    pub fn launcher_logs_dir(&self) -> PathBuf { self.logs_dir().join("launcher") }
    pub fn installations_dir(&self) -> PathBuf { self.data_root.join("installations") }
    pub fn versions_dir(&self) -> PathBuf { self.data_root.join("versions") }
    pub fn downloads_cache_dir(&self) -> PathBuf { self.data_root.join("cache").join("downloads") }
    pub fn accounts_dir(&self) -> PathBuf { self.data_root.join("accounts") }
    pub fn runtimes_dir(&self) -> PathBuf { self.data_root.join("runtimes") }

    /// Create the bootstrap dir and every managed subdirectory.
    pub fn ensure(&self) -> AtlasResult<()> {
        let mut dirs = vec![self.bootstrap_dir.clone(), self.data_root.clone(), self.launcher_logs_dir()];
        dirs.extend(MANAGED_SUBDIRS.iter().map(|d| self.data_root.join(d)));
        dirs.push(self.downloads_cache_dir());
        for d in dirs {
            std::fs::create_dir_all(&d)
                .map_err(|e| AtlasError::io(ErrorCode::DataDirInvalid, "create the Atlas data folder", &d, e))?;
        }
        Ok(())
    }

    /// Persist a new data root in the locator (`None` resets to default).
    pub fn write_locator(&self, data_root: Option<&Path>) -> AtlasResult<()> {
        let loc = Locator { schema: LOCATOR_SCHEMA, data_root: data_root.map(Path::to_path_buf) };
        let text = toml::to_string_pretty(&loc)
            .map_err(|e| AtlasError::new(ErrorCode::Internal, "Could not encode locator.").details(e))?;
        atomic_write(&self.locator_file(), text.as_bytes())
    }
}

fn read_locator(bootstrap_dir: &Path) -> AtlasResult<Locator> {
    let file = bootstrap_dir.join(LOCATOR_FILE);
    match std::fs::read_to_string(&file) {
        Ok(text) => toml::from_str(&text).map_err(|e| {
            AtlasError::new(ErrorCode::ConfigInvalid, "The Atlas locator file is damaged.")
                .details(format!("{}: {e}", file.display()))
                .suggest("Fix or delete locator.toml; Atlas will fall back to the default data folder.")
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Locator::default()),
        Err(e) => Err(AtlasError::io(ErrorCode::ConfigIo, "read the Atlas locator file", &file, e)),
    }
}

fn base_dirs() -> AtlasResult<directories::BaseDirs> {
    directories::BaseDirs::new().ok_or_else(|| {
        AtlasError::new(ErrorCode::DataDirInvalid, "Could not determine your home folder.")
            .suggest(format!("Set {ENV_BOOTSTRAP_DIR} and {ENV_DATA_DIR} to choose folders manually."))
    })
}

/// Linux: `~/.config/atlaslauncher`; Windows: `%APPDATA%\AtlasLauncher`;
/// macOS: `~/Library/Application Support/AtlasLauncher`.
pub fn default_bootstrap_dir() -> AtlasResult<PathBuf> {
    Ok(base_dirs()?.config_dir().join(APP_DIR_NAME))
}

/// Linux: `~/.local/share/atlaslauncher`; Windows: `%APPDATA%\AtlasLauncher`;
/// macOS: `~/Library/Application Support/AtlasLauncher`.
pub fn default_data_root() -> AtlasResult<PathBuf> {
    Ok(base_dirs()?.data_dir().join(APP_DIR_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locator_roundtrip_and_default() {
        let t = tempfile::tempdir().unwrap();
        let boot = t.path().join("boot");
        let p = AtlasPaths::from_bootstrap(boot.clone()).unwrap();
        assert!(!p.data_root_is_custom);
        assert!(p.data_root.ends_with(APP_DIR_NAME));

        let custom = t.path().join("custom");
        p.write_locator(Some(&custom)).unwrap();
        let p2 = AtlasPaths::from_bootstrap(boot).unwrap();
        assert_eq!(p2.data_root, custom);
        assert!(p2.data_root_is_custom);
    }

    #[test]
    fn damaged_locator_is_reported() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join(LOCATOR_FILE), "data_root = [").unwrap();
        let e = AtlasPaths::from_bootstrap(t.path().into()).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
    }

    #[test]
    fn ensure_creates_managed_dirs() {
        let t = tempfile::tempdir().unwrap();
        let p = AtlasPaths::explicit(t.path().join("b"), t.path().join("d"));
        p.ensure().unwrap();
        for d in MANAGED_SUBDIRS {
            assert!(p.data_root.join(d).is_dir(), "{d}");
        }
        assert!(p.launcher_logs_dir().is_dir());
    }
}
