//! Mod files in an installation's `mods` folder.
//!
//! We read whatever metadata the jar carries (Fabric, Quilt, Forge,
//! NeoForge) so the list can show names, versions and dependencies.
//! Disabling a mod just renames it to `*.jar.disabled`, which is what
//! every launcher does.

use atlas_shared::fs_safety::safe_join;
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::Serialize;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    Fabric,
    Quilt,
    Forge,
    NeoForge,
    Unknown,
}

impl Loader {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fabric => "Fabric",
            Self::Quilt => "Quilt",
            Self::Forge => "Forge",
            Self::NeoForge => "NeoForge",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModInfo {
    pub file: String,
    pub enabled: bool,
    pub size: u64,
    pub loader: Loader,
    pub id: Option<String>,
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    /// Required mod ids (minus the obvious ones like "minecraft" and the loader).
    pub depends: Vec<String>,
    /// Minecraft version requirement as written by the mod.
    pub minecraft: Option<String>,
    /// Required ids that aren't present among the enabled mods.
    pub missing: Vec<String>,
}

const MAX_META: u64 = 1024 * 1024;

fn read_entry(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    let mut e = zip.by_name(name).ok()?;
    if e.size() > MAX_META {
        return None;
    }
    let mut s = String::new();
    e.read_to_string(&mut s).ok()?;
    Some(s)
}

const IGNORE_DEPS: &[&str] = &["minecraft", "java", "fabricloader", "fabric-loader", "quilt_loader", "forge", "neoforge", "fabric"];

struct Meta {
    loader: Loader,
    id: Option<String>,
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    depends: Vec<String>,
    minecraft: Option<String>,
}

impl Meta {
    fn unknown() -> Self {
        Self { loader: Loader::Unknown, id: None, name: None, version: None, description: None, depends: vec![], minecraft: None }
    }
}

fn read_meta(path: &Path) -> Meta {
    let Ok(f) = std::fs::File::open(path) else { return Meta::unknown() };
    let Ok(mut zip) = zip::ZipArchive::new(f) else { return Meta::unknown() };

    if let Some(text) = read_entry(&mut zip, "fabric.mod.json")
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
    {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_owned);
        let deps: Vec<String> = v.get("depends").and_then(|d| d.as_object()).map(|o| o.keys().cloned().collect()).unwrap_or_default();
        let mc = v.pointer("/depends/minecraft").map(|m| m.as_str().map(str::to_owned).unwrap_or_else(|| m.to_string()));
        return Meta { loader: Loader::Fabric, id: s("id"), name: s("name"), version: s("version"), description: s("description"), depends: deps, minecraft: mc };
    }
    if let Some(text) = read_entry(&mut zip, "quilt.mod.json")
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
    {
        let ql = v.get("quilt_loader");
        let g = |p: &str| ql.and_then(|q| q.pointer(p)).and_then(|x| x.as_str()).map(str::to_owned);
        let deps = ql
            .and_then(|q| q.get("depends"))
            .and_then(|d| d.as_array())
            .map(|a| a.iter().filter_map(|d| d.as_str().map(str::to_owned).or_else(|| d.get("id").and_then(|i| i.as_str()).map(str::to_owned))).collect())
            .unwrap_or_default();
        return Meta { loader: Loader::Quilt, id: g("/id"), name: g("/metadata/name"), version: g("/version"), description: g("/metadata/description"), depends: deps, minecraft: None };
    }
    for (file, loader) in [("META-INF/neoforge.mods.toml", Loader::NeoForge), ("META-INF/mods.toml", Loader::Forge)] {
        if let Some(text) = read_entry(&mut zip, file)
            && let Ok(v) = toml::from_str::<toml::Value>(&text)
        {
            let m = v.get("mods").and_then(|m| m.as_array()).and_then(|a| a.first());
            let g = |k: &str| m.and_then(|m| m.get(k)).and_then(|x| x.as_str()).map(str::to_owned);
            let id = g("modId");
            let mut deps = vec![];
            let mut mc = None;
            if let Some(list) = id.as_ref().and_then(|i| v.get("dependencies").and_then(|d| d.get(i))).and_then(|l| l.as_array()) {
                for d in list {
                    let did = d.get("modId").and_then(|x| x.as_str()).unwrap_or_default().to_string();
                    let required = d.get("mandatory").and_then(|x| x.as_bool()).unwrap_or(false) || d.get("type").and_then(|x| x.as_str()) == Some("required");
                    if did == "minecraft" {
                        mc = d.get("versionRange").and_then(|x| x.as_str()).map(str::to_owned);
                    } else if required {
                        deps.push(did);
                    }
                }
            }
            // "${file.jarVersion}" means "look in the manifest"; not worth it, show nothing
            let version = g("version").filter(|v| !v.contains("${"));
            return Meta { loader, id, name: g("displayName"), version, description: g("description").map(|d| d.trim().to_string()), depends: deps, minecraft: mc };
        }
    }
    Meta::unknown()
}

pub fn mods_dir(game_dir: &Path) -> PathBuf {
    game_dir.join("mods")
}

pub fn list(game_dir: &Path) -> AtlasResult<Vec<ModInfo>> {
    let dir = mods_dir(game_dir);
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(AtlasError::io(ErrorCode::InvalidInstallation, "list mods", &dir, e)),
    };
    let mut out = vec![];
    for e in rd.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().into_owned();
        let enabled = name.ends_with(".jar");
        if !enabled && !name.ends_with(".jar.disabled") {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
        if !meta.is_file() {
            continue;
        }
        let m = read_meta(&e.path());
        let fallback = name.trim_end_matches(".disabled").trim_end_matches(".jar").to_string();
        out.push(ModInfo {
            file: name,
            enabled,
            size: meta.len(),
            loader: m.loader,
            id: m.id,
            name: m.name.unwrap_or(fallback),
            version: m.version,
            description: m.description,
            depends: m.depends.into_iter().filter(|d| !IGNORE_DEPS.contains(&d.as_str())).collect(),
            minecraft: m.minecraft,
            missing: vec![],
        });
    }
    let have: std::collections::HashSet<String> = out.iter().filter(|m| m.enabled).filter_map(|m| m.id.clone()).collect();
    for m in &mut out {
        if m.enabled {
            m.missing = m.depends.iter().filter(|d| !have.contains(*d) && !d.starts_with("fabric-")).cloned().collect();
        }
    }
    out.sort_by_key(|m| m.name.to_lowercase());
    Ok(out)
}

fn check_file_name(name: &str) -> AtlasResult<()> {
    if name.contains('/') || name.contains('\\') || name.starts_with('.') || !(name.ends_with(".jar") || name.ends_with(".jar.disabled")) {
        return Err(AtlasError::new(ErrorCode::InvalidArgument, format!("'{name}' isn't a mod file name.")));
    }
    Ok(())
}

pub fn set_enabled(game_dir: &Path, file: &str, enabled: bool) -> AtlasResult<String> {
    check_file_name(file)?;
    let dir = mods_dir(game_dir);
    let from = safe_join(&dir, file)?;
    let base = file.trim_end_matches(".disabled");
    let target = if enabled { base.to_string() } else { format!("{base}.disabled") };
    if target == file {
        return Ok(target);
    }
    let to = safe_join(&dir, &target)?;
    if to.exists() {
        return Err(AtlasError::new(ErrorCode::InvalidArgument, format!("{target} already exists.")));
    }
    std::fs::rename(&from, &to).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "rename a mod", &from, e))?;
    Ok(target)
}

/// Copy a jar into the mods folder.
pub fn add(game_dir: &Path, source: &Path) -> AtlasResult<String> {
    let name = source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    check_file_name(&name)?;
    let f = std::fs::File::open(source).map_err(|e| AtlasError::io(ErrorCode::InvalidArgument, "open the mod file", source, e))?;
    zip::ZipArchive::new(f).map_err(|e| AtlasError::new(ErrorCode::InvalidArgument, "That file isn't a valid jar.").details(e))?;
    let dir = mods_dir(game_dir);
    std::fs::create_dir_all(&dir).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "create the mods folder", &dir, e))?;
    let dest = safe_join(&dir, &name)?;
    if dest.exists() {
        return Err(AtlasError::new(ErrorCode::InvalidArgument, format!("{name} is already in the mods folder.")));
    }
    std::fs::copy(source, &dest).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "copy the mod", &dest, e))?;
    Ok(name)
}

pub fn remove(game_dir: &Path, file: &str) -> AtlasResult<()> {
    check_file_name(file)?;
    let p = safe_join(&mods_dir(game_dir), file)?;
    std::fs::remove_file(&p).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "delete the mod", &p, e))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    pub fn jar(path: &Path, entries: &[(&str, &str)]) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        for (n, body) in entries {
            z.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn reads_metadata_and_dependencies() {
        let t = tempfile::tempdir().unwrap();
        let d = mods_dir(t.path());
        std::fs::create_dir_all(&d).unwrap();
        jar(&d.join("sodium.jar"), &[("fabric.mod.json", r#"{"id":"sodium","name":"Sodium","version":"0.6","depends":{"minecraft":">=1.21","fabricloader":"*","fabric-api":"*","indium":"*"}}"#)]);
        jar(&d.join("jei.jar.disabled"), &[("META-INF/neoforge.mods.toml", "[[mods]]\nmodId='jei'\ndisplayName='JEI'\nversion='19.0'\n[[dependencies.jei]]\nmodId='minecraft'\ntype='required'\nversionRange='[1.21,)'\n")]);
        jar(&d.join("plain.jar"), &[("a.txt", "x")]);
        std::fs::write(d.join("notes.txt"), "ignore me").unwrap();
        let mods = list(t.path()).unwrap();
        assert_eq!(mods.len(), 3);
        let s = mods.iter().find(|m| m.name == "Sodium").unwrap();
        assert_eq!(s.loader, Loader::Fabric);
        assert_eq!(s.minecraft.as_deref(), Some(">=1.21"));
        assert_eq!(s.missing, vec!["indium"], "fabric-api counts as part of the loader stack");
        let j = mods.iter().find(|m| m.name == "JEI").unwrap();
        assert!(!j.enabled);
        assert_eq!((j.loader, j.minecraft.as_deref()), (Loader::NeoForge, Some("[1.21,)")));
        assert_eq!(mods.iter().find(|m| m.file == "plain.jar").unwrap().loader, Loader::Unknown);
    }

    #[test]
    fn enable_disable_add_remove() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("cool.jar");
        jar(&src, &[("fabric.mod.json", r#"{"id":"cool"}"#)]);
        assert_eq!(add(t.path(), &src).unwrap(), "cool.jar");
        assert!(add(t.path(), &src).is_err(), "no duplicates");
        assert_eq!(set_enabled(t.path(), "cool.jar", false).unwrap(), "cool.jar.disabled");
        assert_eq!(set_enabled(t.path(), "cool.jar.disabled", true).unwrap(), "cool.jar");
        assert!(set_enabled(t.path(), "../evil.jar", true).is_err());
        remove(t.path(), "cool.jar").unwrap();
        assert!(list(t.path()).unwrap().is_empty());
        let txt = t.path().join("x.txt");
        std::fs::write(&txt, "x").unwrap();
        assert!(add(t.path(), &txt).is_err());
    }
}
