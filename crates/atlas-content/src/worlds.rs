//! Worlds in an installation's `saves` folder, with zip backups.

use crate::nbt;
use atlas_shared::fs_safety::{remove_tree_no_follow, safe_join};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct WorldInfo {
    pub folder: String,
    pub name: String,
    pub last_played: Option<i64>,
    pub game_mode: Option<String>,
    pub hardcore: bool,
    pub version: Option<String>,
    pub size: u64,
    pub has_icon: bool,
}

fn mode(n: i64) -> &'static str {
    match n {
        0 => "Survival",
        1 => "Creative",
        2 => "Adventure",
        3 => "Spectator",
        _ => "Unknown",
    }
}

fn check(folder: &str) -> AtlasResult<()> {
    if folder.is_empty() || folder.contains('/') || folder.contains('\\') || folder == "." || folder == ".." {
        return Err(AtlasError::new(ErrorCode::InvalidArgument, "Bad world name."));
    }
    Ok(())
}

pub fn list(game_dir: &Path) -> AtlasResult<Vec<WorldInfo>> {
    let dir = game_dir.join("saves");
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(AtlasError::io(ErrorCode::InvalidInstallation, "list worlds", &dir, e)),
    };
    let mut out = vec![];
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
        if !meta.is_dir() || !p.join("level.dat").is_file() {
            continue;
        }
        let folder = e.file_name().to_string_lossy().into_owned();
        let root = std::fs::read(p.join("level.dat")).ok().and_then(|b| nbt::parse(&b));
        let data = root.as_ref().and_then(|r| r.get("Data"));
        let g = |k: &str| data.and_then(|d| d.get(k));
        out.push(WorldInfo {
            name: g("LevelName").and_then(|t| t.as_str()).map(str::to_owned).unwrap_or_else(|| folder.clone()),
            last_played: g("LastPlayed").and_then(|t| t.as_i64()),
            game_mode: g("GameType").and_then(|t| t.as_i64()).map(|m| mode(m).to_string()),
            hardcore: g("hardcore").and_then(|t| t.as_i64()).unwrap_or(0) != 0,
            version: data.and_then(|d| d.get("Version")).and_then(|v| v.get("Name")).and_then(|t| t.as_str()).map(str::to_owned),
            size: crate::dir_size(&p),
            has_icon: p.join("icon.png").is_file(),
            folder,
        });
    }
    out.sort_by_key(|w| std::cmp::Reverse(w.last_played.unwrap_or(0)));
    Ok(out)
}

/// Zip a world into `backups_dir`. Symlinks are skipped, never followed.
pub fn backup(game_dir: &Path, folder: &str, backups_dir: &Path) -> AtlasResult<PathBuf> {
    check(folder)?;
    let src = safe_join(&game_dir.join("saves"), folder)?;
    if !src.join("level.dat").is_file() {
        return Err(AtlasError::new(ErrorCode::InvalidArgument, format!("'{folder}' isn't a world.")));
    }
    std::fs::create_dir_all(backups_dir).map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "create the backups folder", backups_dir, e))?;
    let safe_name: String = folder.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let out = backups_dir.join(format!("{safe_name}-{}.zip", crate::unix_now()));
    let tmp = out.with_extension("zip.part");
    let result = (|| -> std::io::Result<()> {
        let f = std::fs::File::create(&tmp)?;
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut stack = vec![src.clone()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir)? {
                let e = e?;
                let p = e.path();
                let m = std::fs::symlink_metadata(&p)?;
                let rel = p.strip_prefix(&src).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                let rel = format!("{folder}/{rel}");
                if m.is_dir() {
                    z.add_directory(&rel, opts)?;
                    stack.push(p);
                } else if m.is_file() {
                    // session.lock is held open by a running game; skip it
                    if p.file_name().is_some_and(|n| n == "session.lock") {
                        continue;
                    }
                    z.start_file(&rel, opts)?;
                    std::io::copy(&mut std::fs::File::open(&p)?, &mut z)?;
                }
            }
        }
        z.finish()?.flush()
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(AtlasError::io(ErrorCode::ConfigIo, "back up the world", &out, e));
    }
    std::fs::rename(&tmp, &out).map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "save the backup", &out, e))?;
    tracing::info!(world = folder, backup = %out.display(), "world backed up");
    Ok(out)
}

/// Delete a world, but only after a backup has been written.
pub fn delete(game_dir: &Path, folder: &str, backups_dir: &Path) -> AtlasResult<PathBuf> {
    let b = backup(game_dir, folder, backups_dir)?;
    remove_tree_no_follow(&safe_join(&game_dir.join("saves"), folder)?)?;
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;

    fn level_dat(name: &str, mode: i32, last: i64) -> Vec<u8> {
        // hand-built NBT: root { Data { LevelName, GameType, LastPlayed, hardcore, Version { Name } } }
        let mut b = vec![10, 0, 0];
        b.extend([10, 0, 4]);
        b.extend(b"Data");
        b.extend([8, 0, 9]);
        b.extend(b"LevelName");
        b.extend((name.len() as u16).to_be_bytes());
        b.extend(name.as_bytes());
        b.extend([3, 0, 8]);
        b.extend(b"GameType");
        b.extend(mode.to_be_bytes());
        b.extend([4, 0, 10]);
        b.extend(b"LastPlayed");
        b.extend(last.to_be_bytes());
        b.extend([1, 0, 8]);
        b.extend(b"hardcore");
        b.push(1);
        b.extend([9, 0, 4]);
        b.extend(b"list");
        b.extend([3, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2]);
        b.extend([10, 0, 7]);
        b.extend(b"Version");
        b.extend([8, 0, 4]);
        b.extend(b"Name");
        b.extend([0, 4]);
        b.extend(b"26.3");
        b.push(0); // end Version
        b.push(0); // end Data
        b.push(0); // end root
        let mut gz = GzEncoder::new(vec![], flate2::Compression::default());
        gz.write_all(&b).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn list_backup_delete() {
        let t = tempfile::tempdir().unwrap();
        let w = t.path().join("saves/My World");
        std::fs::create_dir_all(w.join("region")).unwrap();
        std::fs::write(w.join("level.dat"), level_dat("Castle", 1, 1_700_000_000_000)).unwrap();
        std::fs::write(w.join("region/r.0.0.mca"), vec![7u8; 4096]).unwrap();
        std::fs::create_dir_all(t.path().join("saves/not-a-world")).unwrap();

        let worlds = list(t.path()).unwrap();
        assert_eq!(worlds.len(), 1);
        let x = &worlds[0];
        assert_eq!((x.name.as_str(), x.game_mode.as_deref(), x.hardcore, x.version.as_deref()), ("Castle", Some("Creative"), true, Some("26.3")));

        let backups = t.path().join("backups");
        let zip_path = backup(t.path(), "My World", &backups).unwrap();
        let z = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        assert!(z.file_names().any(|n| n == "My World/region/r.0.0.mca"));

        let b = delete(t.path(), "My World", &backups).unwrap();
        assert!(b.exists() && !w.exists());
        assert!(backup(t.path(), "../etc", &backups).is_err());
    }

    #[test]
    fn junk_nbt_does_not_panic() {
        assert!(nbt::parse(b"").is_none());
        assert!(nbt::parse(&[10, 0, 0, 10, 0, 1]).is_none());
        assert!(nbt::parse(&[0x1f, 0x8b, 1, 2, 3]).is_none());
    }
}
