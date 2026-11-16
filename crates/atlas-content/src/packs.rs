//! Resource packs and shader packs. Packs are zips or folders; resource
//! packs carry a `pack.mcmeta` we can read a description from.

use atlas_shared::fs_safety::{remove_tree_no_follow, safe_join};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::Serialize;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PackKind {
    Resource,
    Shader,
}

impl PackKind {
    pub fn folder(self) -> &'static str {
        match self {
            Self::Resource => "resourcepacks",
            Self::Shader => "shaderpacks",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PackInfo {
    pub file: String,
    pub is_folder: bool,
    pub size: u64,
    pub description: Option<String>,
    pub pack_format: Option<i64>,
}

fn describe(v: &serde_json::Value) -> (Option<String>, Option<i64>) {
    let p = v.get("pack");
    let mut d = String::new();
    if let Some(desc) = p.and_then(|p| p.get("description")) {
        match desc {
            serde_json::Value::String(s) => d.push_str(s),
            other => {
                // chat component: collect "text" fields
                fn walk(v: &serde_json::Value, out: &mut String) {
                    match v {
                        serde_json::Value::String(s) => out.push_str(s),
                        serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
                        serde_json::Value::Object(o) => {
                            if let Some(t) = o.get("text") {
                                walk(t, out);
                            }
                            if let Some(e) = o.get("extra") {
                                walk(e, out);
                            }
                        }
                        _ => {}
                    }
                }
                walk(other, &mut d);
            }
        }
    }
    let d: String = d.chars().filter(|c| *c != '§').collect();
    (Some(d.trim().to_string()).filter(|s| !s.is_empty()), p.and_then(|p| p.get("pack_format")).and_then(|f| f.as_i64()))
}

fn read_mcmeta(path: &Path, is_folder: bool) -> Option<serde_json::Value> {
    let text = if is_folder {
        std::fs::read_to_string(path.join("pack.mcmeta")).ok()?
    } else {
        let f = std::fs::File::open(path).ok()?;
        let mut z = zip::ZipArchive::new(f).ok()?;
        let mut e = z.by_name("pack.mcmeta").ok()?;
        if e.size() > 256 * 1024 {
            return None;
        }
        let mut s = String::new();
        e.read_to_string(&mut s).ok()?;
        s
    };
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

pub fn list(game_dir: &Path, kind: PackKind) -> AtlasResult<Vec<PackInfo>> {
    let dir = game_dir.join(kind.folder());
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(AtlasError::io(ErrorCode::InvalidInstallation, "list packs", &dir, e)),
    };
    let mut out = vec![];
    for e in rd.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
        let is_folder = meta.is_dir();
        if name.starts_with('.') || !(is_folder || name.to_lowercase().ends_with(".zip")) {
            continue;
        }
        let (description, pack_format) = match kind {
            PackKind::Resource => read_mcmeta(&e.path(), is_folder).map(|v| describe(&v)).unwrap_or((None, None)),
            PackKind::Shader => (None, None),
        };
        out.push(PackInfo { file: name, is_folder, size: if is_folder { crate::dir_size(&e.path()) } else { meta.len() }, description, pack_format });
    }
    out.sort_by_key(|p| p.file.to_lowercase());
    Ok(out)
}

pub fn add(game_dir: &Path, kind: PackKind, source: &Path) -> AtlasResult<String> {
    let name = source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !name.to_lowercase().ends_with(".zip") {
        return Err(AtlasError::new(ErrorCode::InvalidArgument, "Packs need to be .zip files."));
    }
    let f = std::fs::File::open(source).map_err(|e| AtlasError::io(ErrorCode::InvalidArgument, "open the pack", source, e))?;
    zip::ZipArchive::new(f).map_err(|e| AtlasError::new(ErrorCode::InvalidArgument, "That file isn't a valid zip.").details(e))?;
    let dir = game_dir.join(kind.folder());
    std::fs::create_dir_all(&dir).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "create the packs folder", &dir, e))?;
    let dest = safe_join(&dir, &name)?;
    if dest.exists() {
        return Err(AtlasError::new(ErrorCode::InvalidArgument, format!("{name} is already there.")));
    }
    std::fs::copy(source, &dest).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "copy the pack", &dest, e))?;
    Ok(name)
}

pub fn remove(game_dir: &Path, kind: PackKind, file: &str) -> AtlasResult<()> {
    if file.contains('/') || file.contains('\\') || file.starts_with('.') {
        return Err(AtlasError::new(ErrorCode::InvalidArgument, "Bad pack name."));
    }
    remove_tree_no_follow(&safe_join(&game_dir.join(kind.folder()), file)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_zip_and_folder_packs() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("resourcepacks");
        std::fs::create_dir_all(d.join("Folder Pack")).unwrap();
        std::fs::write(d.join("Folder Pack/pack.mcmeta"), r#"{"pack":{"pack_format":46,"description":{"text":"§6Gold ","extra":["pack"]}}}"#).unwrap();
        crate::mods::tests::jar(&d.join("Faithful.zip"), &[("pack.mcmeta", r#"{"pack":{"pack_format":34,"description":"32x textures"}}"#)]);
        std::fs::write(d.join("readme.txt"), "x").unwrap();
        let p = list(t.path(), PackKind::Resource).unwrap();
        assert_eq!(p.len(), 2);
        let f = p.iter().find(|x| x.file == "Folder Pack").unwrap();
        assert!(f.description.as_deref().unwrap().contains("Gold pack"));
        assert_eq!(f.pack_format, Some(46));
        remove(t.path(), PackKind::Resource, "Faithful.zip").unwrap();
        assert!(remove(t.path(), PackKind::Resource, "../x").is_err());
    }
}
