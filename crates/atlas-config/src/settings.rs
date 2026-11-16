//! `settings.toml`, as a typed struct.
//!
//! Every field has a default so old or half-written files still load.
//! Unknown keys are kept as warnings rather than errors, so running an
//! older Atlas doesn't quietly throw settings away.

use atlas_shared::{fs_safety::atomic_write, AtlasError, AtlasResult, ErrorCode};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SETTINGS_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema: u32,
    pub general: General,
    pub appearance: Appearance,
    pub downloads: Downloads,
    pub game: Game,
    pub memory: Memory,
    pub installations: Installations,
    pub accounts: Accounts,
    pub privacy: Privacy,
    pub advanced: Advanced,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema: SETTINGS_SCHEMA,
            general: General::default(),
            appearance: Appearance::default(),
            downloads: Downloads::default(),
            game: Game::default(),
            memory: Memory::default(),
            installations: Installations::default(),
            accounts: Accounts::default(),
            privacy: Privacy::default(),
            advanced: Advanced::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AfterLaunch {
    #[default]
    KeepOpen,
    Minimize,
    Close,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub language: String,
    pub after_launch: AfterLaunch,
    pub check_updates_on_start: bool,
}
impl Default for General {
    fn default() -> Self {
        Self { language: "en".into(), after_launch: AfterLaunch::KeepOpen, check_updates_on_start: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    #[default]
    Aurora,
    Ember,
    Moss,
    Iris,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub accent: Accent,
    /// UI zoom factor, 0.75 – 2.0.
    pub ui_scale: f32,
    pub reduce_motion: bool,
}
impl Default for Appearance {
    fn default() -> Self {
        Self { accent: Accent::Aurora, ui_scale: 1.0, reduce_motion: false }
    }
}

/// There's deliberately no switch to turn off checksum checks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Downloads {
    pub max_concurrent: u8,
    /// `None` = unlimited.
    pub bandwidth_limit_kib: Option<u32>,
    pub keep_cache: bool,
}
impl Default for Downloads {
    fn default() -> Self {
        Self { max_concurrent: 8, bandwidth_limit_kib: None, keep_cache: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Game {
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    /// Explicit Java executable; `None` = auto-detect / managed runtime.
    pub java_path: Option<PathBuf>,
    pub extra_jvm_args: Vec<String>,
    /// Manually selected Minecraft directory when auto-detection fails.
    pub minecraft_dir: Option<PathBuf>,
}
impl Default for Game {
    fn default() -> Self {
        Self { width: 1280, height: 720, fullscreen: false, java_path: None, extra_jvm_args: vec![], minecraft_dir: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Memory {
    pub min_mb: u32,
    pub max_mb: u32,
}
impl Default for Memory {
    fn default() -> Self {
        Self { min_mb: 1024, max_mb: 4096 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Installations {
    pub default_installation: Option<String>,
    pub show_snapshots: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Accounts {
    /// Opaque Atlas account id. Never a token.
    pub active_account: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Privacy {
    /// Replace the signed-in username with a placeholder in log files.
    pub hide_username_in_logs: bool,
}
impl Default for Privacy {
    fn default() -> Self {
        Self { hide_username_in_logs: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}
impl LogLevel {
    pub fn as_filter(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Advanced {
    pub log_level: LogLevel,
    pub keep_log_files: u32,
    pub update_channel: UpdateChannel,
    pub ai: Ai,
}
impl Default for Advanced {
    fn default() -> Self {
        Self { log_level: LogLevel::Info, keep_log_files: 14, update_channel: UpdateChannel::Stable, ai: Ai::default() }
    }
}

/// AI features. `enabled` is the master switch: with it off, the MCP server
/// won't run no matter what `mcp_enabled` says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Ai {
    pub enabled: bool,
    pub mcp_enabled: bool,
}
impl Ai {
    pub fn mcp_allowed(&self) -> bool {
        self.enabled && self.mcp_enabled
    }
}

impl Settings {
    /// Check invariants. Returns the first violation.
    pub fn validate(&self) -> AtlasResult<()> {
        let bad = |msg: String, fix: &str| Err(AtlasError::new(ErrorCode::ConfigInvalid, msg).suggest(fix));
        if self.schema > SETTINGS_SCHEMA {
            return bad(
                format!("Settings were written by a newer Atlas (schema {}).", self.schema),
                "Update Atlas, or reset settings.",
            );
        }
        if self.memory.min_mb < 512 {
            return bad("Minimum memory must be at least 512 MB.".into(), "Increase memory.min_mb.");
        }
        if self.memory.min_mb > self.memory.max_mb {
            return bad("Minimum memory is larger than maximum memory.".into(), "Lower memory.min_mb or raise memory.max_mb.");
        }
        if self.memory.max_mb > 262_144 {
            return bad("Maximum memory is unrealistically large (over 256 GB).".into(), "Lower memory.max_mb.");
        }
        if !(1..=32).contains(&self.downloads.max_concurrent) {
            return bad("Concurrent downloads must be between 1 and 32.".into(), "Adjust downloads.max_concurrent.");
        }
        if !(0.75..=2.0).contains(&self.appearance.ui_scale) {
            return bad("UI scale must be between 0.75 and 2.0.".into(), "Adjust appearance.ui_scale.");
        }
        if self.game.width < 320 || self.game.height < 240 {
            return bad("Game window must be at least 320×240.".into(), "Adjust game.width / game.height.");
        }
        if self.advanced.keep_log_files == 0 {
            return bad("Keep at least one log file.".into(), "Set advanced.keep_log_files to 1 or more.");
        }
        Ok(())
    }

    pub fn to_toml(&self) -> AtlasResult<String> {
        toml::to_string_pretty(self)
            .map_err(|e| AtlasError::new(ErrorCode::Internal, "Could not encode settings.").details(e))
    }

    /// Read a value by dotted key, e.g. `memory.max_mb`.
    pub fn get_key(&self, key: &str) -> AtlasResult<toml::Value> {
        let root = toml::Value::try_from(self)
            .map_err(|e| AtlasError::new(ErrorCode::Internal, "Could not encode settings.").details(e))?;
        let mut cur = &root;
        for part in key.split('.') {
            cur = cur.get(part).ok_or_else(|| unknown_key(key))?;
        }
        Ok(cur.clone())
    }

    /// Set a value by dotted key. `raw` is parsed as a TOML value
    /// (`4096`, `true`, `"text"`, `["-XX:+UseG1GC"]`); bare words are taken as
    /// strings. The result is type-checked and validated before being returned.
    pub fn with_key(&self, key: &str, raw: &str) -> AtlasResult<Settings> {
        let mut root = toml::Value::try_from(self)
            .map_err(|e| AtlasError::new(ErrorCode::Internal, "Could not encode settings.").details(e))?;
        let value = parse_value(raw);
        let parts: Vec<&str> = key.split('.').collect();
        let (last, parents) = parts.split_last().ok_or_else(|| unknown_key(key))?;
        let mut cur = &mut root;
        for p in parents {
            cur = cur.get_mut(*p).ok_or_else(|| unknown_key(key))?;
        }
        let table = cur.as_table_mut().ok_or_else(|| unknown_key(key))?;
        // Optional fields are absent when None, so allow setting keys that
        // exist in the schema even if currently missing: verify by round-trip.
        let existed = table.contains_key(*last);
        if raw.trim().is_empty() || raw.trim() == "none" {
            table.remove(*last);
        } else {
            table.insert((*last).to_string(), value);
        }
        let text = toml::to_string(&root)
            .map_err(|e| AtlasError::new(ErrorCode::Internal, "Could not encode settings.").details(e))?;
        let updated: Settings = toml::from_str(&text).map_err(|e| {
            AtlasError::new(ErrorCode::InvalidArgument, format!("'{raw}' is not a valid value for {key}."))
                .details(e)
                .suggest("Check the expected type with `atlas settings show`.")
        })?;
        // Detect unknown keys: if it didn't exist and the round-trip dropped it, it's unknown.
        if !existed && updated.get_key(key).is_err() && !(raw.trim().is_empty() || raw.trim() == "none") {
            return Err(unknown_key(key));
        }
        updated.validate()?;
        Ok(updated)
    }
}

fn parse_value(raw: &str) -> toml::Value {
    let doc = format!("v = {raw}");
    match toml::from_str::<toml::Table>(&doc) {
        Ok(mut t) => t.remove("v").unwrap_or_else(|| toml::Value::String(raw.into())),
        Err(_) => toml::Value::String(raw.into()),
    }
}

fn unknown_key(key: &str) -> AtlasError {
    AtlasError::new(ErrorCode::InvalidArgument, format!("Unknown setting '{key}'."))
        .suggest("Run `atlas settings show` to list valid keys.")
}

/// Loaded settings plus anything worth telling the user about.
#[derive(Debug, Clone)]
pub struct LoadOutcome {
    pub settings: Settings,
    /// Non-fatal problems that the UI/CLI should surface.
    pub warnings: Vec<AtlasError>,
}

/// Loads and saves `settings.toml` atomically.
#[derive(Debug, Clone)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load settings. A missing file yields defaults (and writes them). A
    /// corrupted or invalid file is backed up next to itself and defaults are
    /// used, with a warning describing what happened.
    pub fn load(&self) -> AtlasResult<LoadOutcome> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let settings = Settings::default();
                self.save(&settings)?;
                return Ok(LoadOutcome { settings, warnings: vec![] });
            }
            Err(e) => return Err(AtlasError::io(ErrorCode::ConfigIo, "read settings", &self.path, e)),
        };
        let mut warnings = vec![];
        let parsed: Result<Settings, AtlasError> = toml::from_str::<Settings>(&text)
            .map_err(|e| AtlasError::new(ErrorCode::ConfigInvalid, "Settings file could not be parsed.").details(e))
            .and_then(|s| s.validate().map(|_| s));
        let settings = match parsed {
            Ok(s) => {
                if let Some(unknown) = unknown_keys(&text) {
                    warnings.push(
                        AtlasError::new(ErrorCode::ConfigInvalid, "Settings contain keys this Atlas version does not know.")
                            .details(unknown.join(", "))
                            .suggest("They will be dropped the next time settings are saved."),
                    );
                }
                s
            }
            Err(err) => {
                let backup = self.path.with_extension(format!("toml.invalid-{}", unix_now()));
                std::fs::rename(&self.path, &backup)
                    .map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "back up invalid settings", &self.path, e))?;
                tracing::warn!(code = %err.code, backup = %backup.display(), "settings invalid; using defaults");
                warnings.push(AtlasError {
                    suggestion: format!("Your previous file was kept at {}. Defaults are in use.", backup.display()),
                    ..err
                });
                let s = Settings::default();
                self.save(&s)?;
                s
            }
        };
        Ok(LoadOutcome { settings, warnings })
    }

    pub fn save(&self, settings: &Settings) -> AtlasResult<()> {
        settings.validate()?;
        let body = settings.to_toml()?;
        let text = format!("# Atlas Launcher settings (schema {SETTINGS_SCHEMA}).\n# Edit with care; invalid files are backed up and replaced by defaults.\n\n{body}");
        atomic_write(&self.path, text.as_bytes())
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Compare the raw file's keys against the canonical serialisation.
fn unknown_keys(text: &str) -> Option<Vec<String>> {
    let raw: toml::Table = toml::from_str(text).ok()?;
    let canon = toml::Value::try_from(Settings::default()).ok()?;
    let mut out = vec![];
    fn walk(raw: &toml::Table, canon: &toml::Value, prefix: &str, out: &mut Vec<String>) {
        for (k, v) in raw {
            let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
            match canon.get(k) {
                Some(c) => {
                    if let (Some(t), true) = (v.as_table(), c.is_table()) {
                        walk(t, c, &path, out);
                    }
                }
                // Optional fields are omitted when None; tolerate known optionals.
                None if KNOWN_OPTIONAL.contains(&path.as_str()) => {}
                None => out.push(path),
            }
        }
    }
    walk(&raw, &canon, "", &mut out);
    (!out.is_empty()).then_some(out)
}

const KNOWN_OPTIONAL: &[&str] = &[
    "downloads.bandwidth_limit_kib",
    "game.java_path",
    "game.minecraft_dir",
    "installations.default_installation",
    "accounts.active_account",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_roundtrip() {
        let s = Settings::default();
        s.validate().unwrap();
        let back: Settings = toml::from_str(&s.to_toml().unwrap()).unwrap();
        assert_eq!(back, s);
        assert!(!s.advanced.ai.mcp_allowed(), "AI must be off by default");
    }

    #[test]
    fn partial_file_fills_defaults() {
        let s: Settings = toml::from_str("[memory]\nmax_mb = 8192\n").unwrap();
        assert_eq!(s.memory.max_mb, 8192);
        assert_eq!(s.memory.min_mb, 1024);
        assert_eq!(s.downloads.max_concurrent, 8);
    }

    #[test]
    fn validation_rejects_bad_memory() {
        let mut s = Settings::default();
        s.memory.min_mb = 8000;
        s.memory.max_mb = 4000;
        assert_eq!(s.validate().unwrap_err().code, ErrorCode::ConfigInvalid);
    }

    #[test]
    fn key_get_set() {
        let s = Settings::default();
        assert_eq!(s.get_key("memory.max_mb").unwrap().as_integer(), Some(4096));
        let s2 = s.with_key("memory.max_mb", "8192").unwrap();
        assert_eq!(s2.memory.max_mb, 8192);
        let s3 = s2.with_key("appearance.accent", "ember").unwrap();
        assert_eq!(s3.appearance.accent, Accent::Ember);
        let s4 = s3.with_key("game.java_path", "/usr/bin/java").unwrap();
        assert_eq!(s4.game.java_path.as_deref(), Some(Path::new("/usr/bin/java")));
        let s5 = s4.with_key("game.java_path", "none").unwrap();
        assert_eq!(s5.game.java_path, None);
        assert_eq!(s.with_key("memory.nope", "1").unwrap_err().code, ErrorCode::InvalidArgument);
        assert_eq!(s.with_key("memory.max_mb", "lots").unwrap_err().code, ErrorCode::InvalidArgument);
        assert_eq!(s.with_key("memory.max_mb", "100").unwrap_err().code, ErrorCode::ConfigInvalid);
    }

    #[test]
    fn store_creates_defaults_and_backs_up_corrupt_files() {
        let t = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(t.path().join("settings.toml"));
        let out = store.load().unwrap();
        assert!(out.warnings.is_empty());
        assert!(store.path().exists());

        std::fs::write(store.path(), "memory = \"broken").unwrap();
        let out = store.load().unwrap();
        assert_eq!(out.settings, Settings::default());
        assert_eq!(out.warnings.len(), 1);
        let backups: Vec<_> = std::fs::read_dir(t.path()).unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("invalid-"))
            .collect();
        assert_eq!(backups.len(), 1, "corrupt file must be preserved");
    }

    #[test]
    fn unknown_keys_warn_but_load() {
        let t = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(t.path().join("settings.toml"));
        std::fs::write(store.path(), "[memory]\nmax_mb = 6000\nturbo = true\n").unwrap();
        let out = store.load().unwrap();
        assert_eq!(out.settings.memory.max_mb, 6000);
        assert_eq!(out.warnings.len(), 1);
        assert!(out.warnings[0].details.contains("memory.turbo"));
    }
}
