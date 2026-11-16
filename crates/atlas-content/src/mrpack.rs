//! Modrinth modpack (`.mrpack`) import.
//!
//! A pack is a zip with `modrinth.index.json` (files to download, each with
//! hashes) and `overrides/` folders copied straight into the game folder.
//! We only download from the hosts Modrinth's format allows, check every
//! hash, and keep every path inside the game folder.

use atlas_downloads::{CancellationToken, Checksum, DownloadRequest, Downloader, Progress};
use atlas_shared::fs_safety::safe_join;
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

const ALLOWED_HOSTS: &[&str] = &["cdn.modrinth.com", "github.com", "raw.githubusercontent.com", "gitlab.com"];
const MAX_OVERRIDE_FILE: u64 = 256 * 1024 * 1024;
const MAX_OVERRIDES_TOTAL: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Index {
    format_version: u32,
    game: String,
    name: String,
    version_id: Option<String>,
    #[serde(default)]
    files: Vec<PackFile>,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackFile {
    path: String,
    hashes: BTreeMap<String, String>,
    #[serde(default)]
    downloads: Vec<String>,
    file_size: Option<u64>,
    env: Option<BTreeMap<String, String>>,
}

/// What we learned from reading the pack, before downloading anything.
#[derive(Debug, Clone, Serialize)]
pub struct PackSummary {
    pub name: String,
    pub version: Option<String>,
    pub minecraft: String,
    /// e.g. ("fabric-loader", "0.16.9")
    pub loaders: Vec<(String, String)>,
    pub files: usize,
}

fn bad(msg: impl Into<String>) -> AtlasError {
    AtlasError::new(ErrorCode::InvalidArgument, "That modpack file can't be used.").details(msg.into()).suggest("Make sure it's a Modrinth .mrpack file and not damaged.")
}

fn open_index(pack: &Path) -> AtlasResult<(zip::ZipArchive<std::fs::File>, Index)> {
    let f = std::fs::File::open(pack).map_err(|e| AtlasError::io(ErrorCode::InvalidArgument, "open the modpack", pack, e))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| bad(e.to_string()))?;
    let mut text = String::new();
    {
        let mut e = z.by_name("modrinth.index.json").map_err(|_| bad("no modrinth.index.json inside"))?;
        if e.size() > 16 * 1024 * 1024 {
            return Err(bad("index is unreasonably large"));
        }
        e.read_to_string(&mut text).map_err(|e| bad(e.to_string()))?;
    }
    let idx: Index = serde_json::from_str(&text).map_err(|e| bad(e.to_string()))?;
    if idx.format_version != 1 || idx.game != "minecraft" {
        return Err(bad(format!("unsupported format {} / game {}", idx.format_version, idx.game)));
    }
    Ok((z, idx))
}

pub fn inspect(pack: &Path) -> AtlasResult<PackSummary> {
    let (_, idx) = open_index(pack)?;
    let minecraft = idx.dependencies.get("minecraft").cloned().ok_or_else(|| bad("pack doesn't say which Minecraft version it needs"))?;
    Ok(PackSummary {
        name: idx.name,
        version: idx.version_id,
        minecraft,
        loaders: idx.dependencies.into_iter().filter(|(k, _)| k != "minecraft").collect(),
        files: idx.files.len(),
    })
}

fn allowed_url(u: &str) -> bool {
    let Some(rest) = u.strip_prefix("https://") else { return false };
    let host = rest.split(['/', ':']).next().unwrap_or_default();
    ALLOWED_HOSTS.contains(&host)
}

/// Download the pack's files and copy its overrides into `game_dir`.
pub async fn install(dl: &Downloader, pack: &Path, game_dir: &Path, progress: Arc<Progress>, cancel: CancellationToken) -> AtlasResult<PackSummary> {
    let summary = inspect(pack)?;
    let (mut z, idx) = open_index(pack)?;
    std::fs::create_dir_all(game_dir).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "create the game folder", game_dir, e))?;

    let mut reqs = vec![];
    for f in &idx.files {
        // files marked unsupported on the client are server-only
        if f.env.as_ref().and_then(|e| e.get("client")).is_some_and(|c| c == "unsupported") {
            continue;
        }
        let dest = safe_join(game_dir, &f.path)?;
        let checksum = if let Some(h) = f.hashes.get("sha512") {
            Checksum::Sha512(h.to_ascii_lowercase())
        } else if let Some(h) = f.hashes.get("sha1") {
            Checksum::Sha1(h.to_ascii_lowercase())
        } else {
            return Err(bad(format!("{} has no hash", f.path)));
        };
        let url = f.downloads.iter().find(|u| allowed_url(u)).ok_or_else(|| {
            AtlasError::new(ErrorCode::DownloadFailed, "The modpack wants to download from a site Atlas doesn't trust.")
                .details(format!("{}: {:?}", f.path, f.downloads))
                .suggest("Only Modrinth, GitHub and GitLab downloads are allowed, as in Modrinth's own format rules.")
        })?;
        reqs.push(DownloadRequest { url: url.clone(), dest, checksum, size: f.file_size, reuse_from: vec![] });
    }
    tracing::info!(log_source = "install", pack = %summary.name, files = reqs.len(), "installing modpack");
    dl.download_all(reqs, progress, cancel).await?;

    // overrides, then client-overrides on top
    let mut total = 0u64;
    for prefix in ["overrides/", "client-overrides/"] {
        for i in 0..z.len() {
            let mut e = z.by_index(i).map_err(|e| bad(e.to_string()))?;
            let name = e.name().to_string();
            let Some(rel) = name.strip_prefix(prefix) else { continue };
            if rel.is_empty() || e.is_dir() {
                continue;
            }
            if e.is_symlink() {
                return Err(AtlasError::new(ErrorCode::UnsafeSymlink, "The modpack contains a symbolic link.").details(name));
            }
            let dest = safe_join(game_dir, rel)?;
            if e.size() > MAX_OVERRIDE_FILE {
                return Err(bad(format!("{rel} is too large")));
            }
            total += e.size();
            if total > MAX_OVERRIDES_TOTAL {
                return Err(bad("overrides are too large"));
            }
            if let Some(d) = dest.parent() {
                std::fs::create_dir_all(d).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "create a folder", d, e))?;
            }
            let mut out = std::fs::File::create(&dest).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "write a file", &dest, e))?;
            let n = std::io::copy(&mut (&mut e).take(MAX_OVERRIDE_FILE + 1), &mut out).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "write a file", &dest, e))?;
            if n > MAX_OVERRIDE_FILE {
                return Err(bad(format!("{rel} is larger than it claimed")));
            }
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_pack(path: &Path, index: serde_json::Value, extra: &[(&str, &[u8])]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("modrinth.index.json", o).unwrap();
        z.write_all(index.to_string().as_bytes()).unwrap();
        for (n, b) in extra {
            z.start_file(*n, o).unwrap();
            z.write_all(b).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn url_allowlist() {
        assert!(allowed_url("https://cdn.modrinth.com/data/x/y.jar"));
        assert!(!allowed_url("http://cdn.modrinth.com/x"));
        assert!(!allowed_url("https://cdn.modrinth.com.evil.net/x"));
        assert!(!allowed_url("https://example.com/mod.jar"));
    }

    #[tokio::test]
    async fn rejects_bad_hosts_and_paths() {
        let t = tempfile::tempdir().unwrap();
        let dl = Downloader::new(Default::default()).unwrap();
        let p = t.path().join("a.mrpack");
        make_pack(&p, serde_json::json!({"formatVersion":1,"game":"minecraft","name":"P","dependencies":{"minecraft":"1.21.1"},
            "files":[{"path":"mods/x.jar","hashes":{"sha1":"a"},"downloads":["https://example.com/x.jar"]}]}), &[]);
        let e = install(&dl, &p, &t.path().join("g"), Arc::new(Progress::new()), CancellationToken::new()).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::DownloadFailed);

        make_pack(&p, serde_json::json!({"formatVersion":1,"game":"minecraft","name":"P","dependencies":{"minecraft":"1.21.1"},
            "files":[{"path":"../../escape.jar","hashes":{"sha1":"a"},"downloads":["https://cdn.modrinth.com/x.jar"]}]}), &[]);
        let e = install(&dl, &p, &t.path().join("g"), Arc::new(Progress::new()), CancellationToken::new()).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::PathTraversal);

        make_pack(&p, serde_json::json!({"formatVersion":1,"game":"minecraft","name":"P","dependencies":{"minecraft":"1.21.1"},"files":[]}), &[("overrides/../../evil.txt", b"x")]);
        let e = install(&dl, &p, &t.path().join("g"), Arc::new(Progress::new()), CancellationToken::new()).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::PathTraversal);
    }

    #[tokio::test]
    async fn summary_and_overrides() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("b.mrpack");
        make_pack(&p, serde_json::json!({"formatVersion":1,"game":"minecraft","name":"Cozy","versionId":"2.0","dependencies":{"minecraft":"1.21.1","fabric-loader":"0.16.9"},
            "files":[{"path":"mods/server-only.jar","hashes":{"sha1":"a"},"downloads":["https://example.com/never.jar"],"env":{"client":"unsupported","server":"required"}}]}),
            &[("overrides/config/a.toml", b"x=1"), ("client-overrides/options.txt", b"fov:90")]);
        let s = inspect(&p).unwrap();
        assert_eq!((s.name.as_str(), s.minecraft.as_str()), ("Cozy", "1.21.1"));
        assert_eq!(s.loaders, vec![("fabric-loader".to_string(), "0.16.9".to_string())]);
        let g = t.path().join("game");
        let dl = Downloader::new(Default::default()).unwrap();
        install(&dl, &p, &g, Arc::new(Progress::new()), CancellationToken::new()).await.unwrap();
        assert_eq!(std::fs::read_to_string(g.join("config/a.toml")).unwrap(), "x=1");
        assert_eq!(std::fs::read_to_string(g.join("options.txt")).unwrap(), "fov:90");
        assert!(!g.join("mods/server-only.jar").exists());
    }
}
