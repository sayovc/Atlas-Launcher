//! Installations ("instances"): a name, a game version and a game folder,
//! plus whatever launch settings the user wants to override.
//!
//! Each one lives in `installations/<id>/installation.toml`. The game folder
//! is `installations/<id>/game` unless the user pointed it somewhere else,
//! in which case we treat it as theirs and never delete it.

use atlas_shared::fs_safety::{atomic_write, copy_tree_no_follow, remove_tree_no_follow};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const FILE: &str = "installation.toml";
const SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Installation {
    #[serde(default = "schema")]
    pub schema: u32,
    pub id: String,
    pub name: String,
    /// Minecraft version id, e.g. "26.3".
    pub version: String,
    #[serde(default)]
    pub loader: atlas_versions::LoaderKind,
    /// None = newest stable loader for this Minecraft version.
    #[serde(default)]
    pub loader_version: Option<String>,
    pub created: u64,
    pub last_played: Option<u64>,
    /// Built-in icon name (the UI maps it to a drawing).
    #[serde(default)]
    pub icon: String,
    /// None means the managed folder inside the installation.
    pub game_dir: Option<PathBuf>,
    pub java_path: Option<PathBuf>,
    pub memory_min_mb: Option<u32>,
    pub memory_max_mb: Option<u32>,
    #[serde(default)]
    pub jvm_args: Vec<String>,
    #[serde(default)]
    pub game_args: Vec<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fullscreen: Option<bool>,
}

fn schema() -> u32 {
    SCHEMA
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Installation {
    pub fn uses_external_dir(&self) -> bool {
        self.game_dir.is_some()
    }
}

/// What the user can change on an existing installation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Changes {
    pub name: Option<String>,
    pub version: Option<String>,
    pub loader: Option<atlas_versions::LoaderKind>,
    pub loader_version: Option<Option<String>>,
    pub icon: Option<String>,
    pub java_path: Option<Option<PathBuf>>,
    pub memory_min_mb: Option<Option<u32>>,
    pub memory_max_mb: Option<Option<u32>>,
    pub jvm_args: Option<Vec<String>>,
    pub game_args: Option<Vec<String>>,
    pub width: Option<Option<u32>>,
    pub height: Option<Option<u32>>,
    pub fullscreen: Option<Option<bool>>,
}

pub struct Installations {
    root: PathBuf,
}

fn not_found(id: &str) -> AtlasError {
    AtlasError::new(ErrorCode::InvalidInstallation, format!("No installation with id '{id}'."))
        .suggest("Run `atlas instances` to see the ids.")
}

fn check_name(name: &str) -> AtlasResult<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 64 {
        return Err(AtlasError::new(ErrorCode::InvalidArgument, "Installation names need 1 to 64 characters."));
    }
    Ok(n.to_string())
}

// ids we generate are hex uuids, so anything else is either hand-edited or hostile
fn check_id(id: &str) -> AtlasResult<()> {
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(not_found(id));
    }
    Ok(())
}

impl Installations {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    /// Folder the game actually runs in.
    pub fn game_dir(&self, inst: &Installation) -> PathBuf {
        inst.game_dir.clone().unwrap_or_else(|| self.dir(&inst.id).join("game"))
    }

    pub fn list(&self) -> AtlasResult<Vec<Installation>> {
        let rd = match std::fs::read_dir(&self.root) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(AtlasError::io(ErrorCode::InvalidInstallation, "list installations", &self.root, e)),
        };
        let mut out = vec![];
        for entry in rd.filter_map(Result::ok) {
            let id = entry.file_name().to_string_lossy().into_owned();
            if check_id(&id).is_err() {
                continue;
            }
            match self.load(&id) {
                Ok(i) => out.push(i),
                // a broken file shouldn't hide the others
                Err(e) => tracing::warn!(installation = %id, "skipping: {} {}", e.message, e.details),
            }
        }
        // most recently played first, then newest
        out.sort_by(|a, b| b.last_played.unwrap_or(0).cmp(&a.last_played.unwrap_or(0)).then(b.created.cmp(&a.created)));
        Ok(out)
    }

    pub fn load(&self, id: &str) -> AtlasResult<Installation> {
        check_id(id)?;
        let p = self.dir(id).join(FILE);
        let text = std::fs::read_to_string(&p).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => not_found(id),
            _ => AtlasError::io(ErrorCode::InvalidInstallation, "read an installation", &p, e),
        })?;
        let inst: Installation = toml::from_str(&text).map_err(|e| {
            AtlasError::new(ErrorCode::InvalidInstallation, "This installation's settings file is damaged.")
                .details(format!("{}: {e}", p.display()))
                .suggest("Fix installation.toml by hand or delete the installation.")
        })?;
        if inst.id != id {
            return Err(AtlasError::new(ErrorCode::InvalidInstallation, "Installation id doesn't match its folder.").details(p.display()));
        }
        Ok(inst)
    }

    fn save(&self, inst: &Installation) -> AtlasResult<()> {
        let text = toml::to_string_pretty(inst).map_err(|e| AtlasError::new(ErrorCode::Internal, "Couldn't save installation.").details(e))?;
        atomic_write(&self.dir(&inst.id).join(FILE), text.as_bytes())
    }

    pub fn create(&self, name: &str, version: &str, game_dir: Option<PathBuf>) -> AtlasResult<Installation> {
        let name = check_name(name)?;
        if let Some(d) = &game_dir
            && !d.is_absolute()
        {
            return Err(AtlasError::new(ErrorCode::InvalidArgument, "A custom game folder must be an absolute path."));
        }
        let inst = Installation {
            schema: SCHEMA,
            id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
            name,
            version: version.to_string(),
            loader: atlas_versions::LoaderKind::Vanilla,
            loader_version: None,
            created: now(),
            last_played: None,
            icon: "grass".into(),
            game_dir,
            java_path: None,
            memory_min_mb: None,
            memory_max_mb: None,
            jvm_args: vec![],
            game_args: vec![],
            width: None,
            height: None,
            fullscreen: None,
        };
        std::fs::create_dir_all(self.game_dir(&inst))
            .map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "create the game folder", &self.game_dir(&inst), e))?;
        self.save(&inst)?;
        tracing::info!(installation = %inst.id, name = %inst.name, version = %inst.version, "installation created");
        Ok(inst)
    }

    pub fn update(&self, id: &str, c: Changes) -> AtlasResult<Installation> {
        let mut i = self.load(id)?;
        if let Some(n) = c.name {
            i.name = check_name(&n)?;
        }
        if let Some(v) = c.version {
            if v != i.version {
                i.loader_version = None; // a pinned loader build rarely fits another MC version
            }
            i.version = v;
        }
        if let Some(l) = c.loader {
            if l != i.loader {
                i.loader_version = None;
            }
            i.loader = l;
        }
        if let Some(v) = c.loader_version {
            i.loader_version = v;
        }
        if let Some(v) = c.icon {
            i.icon = v;
        }
        if let Some(v) = c.java_path {
            i.java_path = v;
        }
        if let Some(v) = c.memory_min_mb {
            i.memory_min_mb = v;
        }
        if let Some(v) = c.memory_max_mb {
            i.memory_max_mb = v;
        }
        if let (Some(min), Some(max)) = (i.memory_min_mb, i.memory_max_mb)
            && min > max
        {
            return Err(AtlasError::new(ErrorCode::InvalidArgument, "Minimum memory is above maximum memory."));
        }
        if let Some(v) = c.jvm_args {
            i.jvm_args = v;
        }
        if let Some(v) = c.game_args {
            i.game_args = v;
        }
        if let Some(v) = c.width {
            i.width = v;
        }
        if let Some(v) = c.height {
            i.height = v;
        }
        if let Some(v) = c.fullscreen {
            i.fullscreen = v;
        }
        self.save(&i)?;
        Ok(i)
    }

    pub fn touch_played(&self, id: &str) -> AtlasResult<()> {
        let mut i = self.load(id)?;
        i.last_played = Some(now());
        self.save(&i)
    }

    /// Copies settings and (for managed folders) the whole game folder,
    /// worlds included. Symlinks are skipped, not followed.
    pub fn duplicate(&self, id: &str) -> AtlasResult<Installation> {
        let src = self.load(id)?;
        let mut copy = self.create(&format!("{} (copy)", src.name).chars().take(64).collect::<String>(), &src.version, None)?;
        let keep_id = copy.id.clone();
        let keep_created = copy.created;
        copy = Installation { id: keep_id, created: keep_created, last_played: None, game_dir: None, name: copy.name, ..src.clone() };
        if !src.uses_external_dir() {
            copy_tree_no_follow(&self.game_dir(&src), &self.game_dir(&copy))?;
        }
        self.save(&copy)?;
        Ok(copy)
    }

    /// Deletes the installation folder. A custom (external) game folder is
    /// left alone on purpose, it belongs to the user.
    pub fn delete(&self, id: &str) -> AtlasResult<Option<PathBuf>> {
        let inst = self.load(id)?;
        remove_tree_no_follow(&self.dir(id))?;
        tracing::info!(installation = %id, "installation deleted");
        Ok(inst.game_dir)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Installations) {
        let t = tempfile::tempdir().unwrap();
        let i = Installations::new(t.path().join("installations"));
        (t, i)
    }

    #[test]
    fn create_list_update() {
        let (_t, s) = setup();
        let a = s.create("Survival", "26.3", None).unwrap();
        assert!(s.game_dir(&a).is_dir());
        assert_eq!(s.list().unwrap().len(), 1);
        let a = s.update(&a.id, Changes { name: Some("  Hardcore ".into()), memory_max_mb: Some(Some(8192)), ..Default::default() }).unwrap();
        assert_eq!(a.name, "Hardcore");
        assert_eq!(s.load(&a.id).unwrap().memory_max_mb, Some(8192));
        assert!(s.update(&a.id, Changes { name: Some("   ".into()), ..Default::default() }).is_err());
        let bad = Changes { memory_min_mb: Some(Some(9000)), memory_max_mb: Some(Some(1000)), ..Default::default() };
        assert!(s.update(&a.id, bad).is_err());
    }

    #[test]
    fn duplicate_copies_worlds() {
        let (_t, s) = setup();
        let a = s.create("Main", "26.3", None).unwrap();
        let w = s.game_dir(&a).join("saves/World");
        std::fs::create_dir_all(&w).unwrap();
        std::fs::write(w.join("level.dat"), b"data").unwrap();
        let b = s.duplicate(&a.id).unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(b.name, "Main (copy)");
        assert_eq!(std::fs::read(s.game_dir(&b).join("saves/World/level.dat")).unwrap(), b"data");
    }

    #[test]
    fn delete_never_touches_external_folder() {
        let (t, s) = setup();
        let ext = t.path().join("my-real-minecraft");
        std::fs::create_dir_all(ext.join("saves/Precious")).unwrap();
        std::fs::write(ext.join("saves/Precious/level.dat"), b"keep me").unwrap();
        let a = s.create("External", "26.3", Some(ext.clone())).unwrap();
        let left = s.delete(&a.id).unwrap();
        assert_eq!(left, Some(ext.clone()));
        assert_eq!(std::fs::read(ext.join("saves/Precious/level.dat")).unwrap(), b"keep me");
        assert!(s.list().unwrap().is_empty());
    }

    #[test]
    fn bad_ids_are_rejected() {
        let (_t, s) = setup();
        for id in ["../x", "", "a/b", ".."] {
            assert!(s.load(id).is_err(), "{id}");
            assert!(s.delete(id).is_err(), "{id}");
        }
        assert!(s.create("x", "26.3", Some("relative/path".into())).is_err());
    }
}
