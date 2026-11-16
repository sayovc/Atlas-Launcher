//! Java runtimes from Mojang's own runtime index, so players don't need
//! to install Java themselves.
//!
//! Each runtime ends up in `runtimes/<component>/` with a small record
//! file listing every file and its hash.

use crate::rules::Platform;
use atlas_downloads::{sha1_hex, CancellationToken, Checksum, DownloadRequest, Downloader, Progress};
use atlas_shared::fs_safety::{atomic_write, safe_join};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

pub const RUNTIME_INDEX_URL: &str =
    "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
const RECORD: &str = ".atlas-runtime.toml";

#[derive(Deserialize)]
struct IndexEntry {
    manifest: Ref,
    version: VersionName,
}
#[derive(Deserialize, Clone)]
struct Ref {
    sha1: String,
    size: u64,
    url: String,
}
#[derive(Deserialize)]
struct VersionName {
    name: String,
}
#[derive(Deserialize)]
struct RuntimeManifest {
    files: BTreeMap<String, RuntimeFile>,
}
#[derive(Deserialize)]
struct RuntimeFile {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    executable: bool,
    downloads: Option<Downloads>,
    target: Option<String>,
}
#[derive(Deserialize)]
struct Downloads {
    raw: Ref,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeRecord {
    pub component: String,
    pub version: String,
    pub files: Vec<(String, String, u64)>,
}

/// Mojang's name for our platform in the runtime index.
pub fn index_platform(p: Platform) -> &'static str {
    match (p.os, p.arch) {
        ("windows", "x86") => "windows-x86",
        ("windows", "arm64") => "windows-arm64",
        ("windows", _) => "windows-x64",
        ("osx", "arm64") => "mac-os-arm64",
        ("osx", _) => "mac-os",
        (_, "x86") => "linux-i386",
        _ => "linux",
    }
}

pub fn runtime_dir(runtimes: &Path, component: &str) -> PathBuf {
    runtimes.join(component)
}

/// Path of the java binary inside an installed runtime.
pub fn java_binary(dir: &Path, p: Platform) -> PathBuf {
    match p.os {
        "windows" => dir.join("bin").join("javaw.exe"),
        "osx" => dir.join("jre.bundle/Contents/Home/bin/java"),
        _ => dir.join("bin/java"),
    }
}

fn bad(msg: impl Into<String>) -> AtlasError {
    AtlasError::new(ErrorCode::PreparationFailed, "The Java runtime metadata looks wrong.")
        .details(msg.into())
        .suggest("Try again later, or set your own Java in Settings → Game.")
}

/// Lexically resolve a link target relative to the link's folder and make
/// sure it stays inside the runtime. Returns the cleaned relative path.
fn link_target_inside(link_rel: &str, target: &str) -> Option<PathBuf> {
    if Path::new(target).is_absolute() {
        return None;
    }
    let mut parts: Vec<_> = Path::new(link_rel).parent()?.components().map(|c| c.as_os_str().to_owned()).collect();
    for c in Path::new(target).components() {
        match c {
            Component::Normal(n) => parts.push(n.to_owned()),
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts.iter().collect())
}

pub fn installed(dir: &Path) -> Option<RuntimeRecord> {
    toml::from_str(&std::fs::read_to_string(dir.join(RECORD)).ok()?).ok()
}

/// Make sure the runtime `component` is installed and return the java path.
pub async fn ensure_runtime(
    dl: &Downloader,
    runtimes: &Path,
    component: &str,
    platform: Platform,
    index_url: &str,
    progress: Arc<Progress>,
    cancel: CancellationToken,
) -> AtlasResult<PathBuf> {
    if !crate::valid_id(component) {
        return Err(bad(format!("odd component name {component:?}")));
    }
    let dir = runtime_dir(runtimes, component);
    if installed(&dir).is_some() && java_binary(&dir, platform).is_file() {
        return Ok(java_binary(&dir, platform));
    }

    let index: BTreeMap<String, BTreeMap<String, Vec<IndexEntry>>> =
        serde_json::from_slice(&dl.fetch_bytes(index_url, &cancel).await?).map_err(|e| bad(e.to_string()))?;
    let plat = index_platform(platform);
    let entry = index
        .get(plat)
        .and_then(|m| m.get(component))
        .and_then(|v| v.first())
        .ok_or_else(|| {
            AtlasError::new(ErrorCode::PreparationFailed, format!("Mojang has no '{component}' Java runtime for {plat}."))
                .suggest("Install Java yourself and set it in Settings → Game.")
        })?;

    let mbytes = dl.fetch_bytes(&entry.manifest.url, &cancel).await?;
    if !sha1_hex(&mbytes).eq_ignore_ascii_case(&entry.manifest.sha1) || mbytes.len() as u64 != entry.manifest.size {
        return Err(AtlasError::new(ErrorCode::ChecksumMismatch, "The Java runtime file list didn't match its checksum.")
            .details(&entry.manifest.url)
            .suggest("Try again; if it keeps happening something is tampering with downloads."));
    }
    let manifest: RuntimeManifest = serde_json::from_slice(&mbytes).map_err(|e| bad(e.to_string()))?;

    tracing::info!(log_source = "install", component, version = %entry.version.name, files = manifest.files.len(), "installing java runtime");
    let mut reqs = vec![];
    let mut execs = vec![];
    let mut links = vec![];
    let mut record = RuntimeRecord { component: component.into(), version: entry.version.name.clone(), files: vec![] };
    for (rel, f) in &manifest.files {
        let dest = safe_join(&dir, rel)?;
        match f.kind.as_str() {
            "directory" => std::fs::create_dir_all(&dest).map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "create a folder", &dest, e))?,
            "file" => {
                let d = f.downloads.as_ref().ok_or_else(|| bad(format!("{rel} has no download")))?;
                if !crate::valid_sha1(&d.raw.sha1) {
                    return Err(bad(format!("{rel} has a bad hash")));
                }
                reqs.push(DownloadRequest {
                    url: d.raw.url.clone(),
                    dest: dest.clone(),
                    checksum: Checksum::Sha1(d.raw.sha1.to_ascii_lowercase()),
                    size: Some(d.raw.size),
                    reuse_from: vec![],
                });
                record.files.push((rel.clone(), d.raw.sha1.to_ascii_lowercase(), d.raw.size));
                if f.executable {
                    execs.push(dest);
                }
            }
            "link" => {
                let t = f.target.as_deref().ok_or_else(|| bad(format!("{rel} link has no target")))?;
                // links pointing outside the runtime folder are refused outright
                let inside = link_target_inside(rel, t).ok_or_else(|| {
                    AtlasError::new(ErrorCode::UnsafeSymlink, "The Java runtime contains a link that points outside it.")
                        .details(format!("{rel} -> {t}"))
                })?;
                links.push((dest, t.to_string(), dir.join(inside)));
            }
            _ => {}
        }
    }
    dl.download_all(reqs, progress, cancel).await?;

    #[cfg(unix)]
    for p in &execs {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "mark a file executable", p, e))?;
    }
    #[cfg(not(unix))]
    let _ = &execs;

    for (link, target, resolved) in links {
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "create a link", &link, e))?;
        #[cfg(not(unix))]
        {
            // no unprivileged symlinks on Windows; a copy works just as well here
            let _ = target;
            if resolved.is_file() {
                std::fs::copy(&resolved, &link).map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "copy a file", &link, e))?;
            }
        }
        #[cfg(unix)]
        let _ = resolved;
    }

    let text = toml::to_string(&record).map_err(|e| AtlasError::new(ErrorCode::Internal, "Couldn't save runtime record.").details(e))?;
    atomic_write(&dir.join(RECORD), text.as_bytes())?;
    let java = java_binary(&dir, platform);
    if !java.is_file() {
        return Err(AtlasError::new(ErrorCode::PreparationFailed, "Java was downloaded but the java binary isn't where it should be.").details(java.display()));
    }
    tracing::info!(log_source = "install", component, "java runtime ready");
    Ok(java)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_targets_must_stay_inside() {
        assert_eq!(link_target_inside("legal/java.compiler/LICENSE", "../java.base/LICENSE"), Some(PathBuf::from("legal/java.base/LICENSE")));
        assert_eq!(link_target_inside("a/b", "../../../etc/passwd"), None);
        assert_eq!(link_target_inside("a/b", "/etc/passwd"), None);
        assert_eq!(link_target_inside("lib/x", "y"), Some(PathBuf::from("lib/y")));
    }

    #[test]
    fn platform_names() {
        assert_eq!(index_platform(Platform { os: "linux", arch: "x86_64" }), "linux");
        assert_eq!(index_platform(Platform { os: "osx", arch: "arm64" }), "mac-os-arm64");
        assert_eq!(index_platform(Platform { os: "windows", arch: "x86_64" }), "windows-x64");
    }
}
