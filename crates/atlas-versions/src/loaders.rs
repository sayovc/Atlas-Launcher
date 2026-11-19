//! Mod loaders. Each loader gives us a "profile": a small version file that
//! sits on top of a vanilla version (`inheritsFrom`) and adds libraries,
//! a different main class and a few arguments. We install it like any other
//! version (`versions/<profile>/`), so verify and repair just work, and the
//! launcher merges it with the vanilla file at start time.
//!
//! Fabric and Quilt come from their official metadata servers. Forge and
//! NeoForge come from Modrinth's launcher-meta, which already unpacks their
//! installers; they also need a post-install step, see `forge.rs`.

use crate::record::{InstallRecord, RecordedFile, RECORD_SCHEMA};
use crate::service::VersionService;
use atlas_downloads::{CancellationToken, Checksum, DownloadRequest, Downloader, Progress};
use atlas_shared::fs_safety::{atomic_write, safe_join};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    #[default]
    Vanilla,
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

impl LoaderKind {
    pub const ALL: [LoaderKind; 5] = [Self::Vanilla, Self::Fabric, Self::Quilt, Self::NeoForge, Self::Forge];

    pub fn label(self) -> &'static str {
        match self {
            Self::Vanilla => "Vanilla",
            Self::Fabric => "Fabric",
            Self::Quilt => "Quilt",
            Self::Forge => "Forge",
            Self::NeoForge => "NeoForge",
        }
    }

    /// Name Modrinth uses for this loader in its API.
    pub fn modrinth(self) -> &'static str {
        match self {
            Self::Vanilla => "minecraft",
            Self::Fabric => "fabric",
            Self::Quilt => "quilt",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "vanilla" | "none" => Self::Vanilla,
            "fabric" => Self::Fabric,
            "quilt" => Self::Quilt,
            "forge" => Self::Forge,
            "neoforge" | "neo" => Self::NeoForge,
            _ => return None,
        })
    }
}

/// Where loader metadata comes from. Overridable for tests.
#[derive(Debug, Clone)]
pub struct LoaderSources {
    pub fabric: String,
    pub quilt: String,
    pub modrinth_meta: String,
}

impl Default for LoaderSources {
    fn default() -> Self {
        Self {
            fabric: "https://meta.fabricmc.net/v2".into(),
            quilt: "https://meta.quiltmc.org/v3".into(),
            modrinth_meta: "https://launcher-meta.modrinth.com".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LoaderVersion {
    pub version: String,
    pub stable: bool,
}

fn meta_err(what: &str, e: impl std::fmt::Display) -> AtlasError {
    AtlasError::new(ErrorCode::PreparationFailed, format!("Couldn't read the {what} version list."))
        .details(e.to_string())
        .suggest("Check your connection, or pick a different Minecraft version.")
}

/// Turn "group:artifact:version[:classifier][@ext]" into a maven path.
pub fn maven_path(coord: &str) -> Option<String> {
    let (coord, ext) = match coord.split_once('@') {
        Some((c, e)) => (c, e),
        None => (coord, "jar"),
    };
    let parts: Vec<&str> = coord.split(':').collect();
    let (group, artifact, version, classifier) = match parts.as_slice() {
        [g, a, v] => (*g, *a, *v, None),
        [g, a, v, c] => (*g, *a, *v, Some(*c)),
        _ => return None,
    };
    if [group, artifact, version].iter().any(|p| p.is_empty() || p.contains("..") || p.contains('/') || p.contains('\\')) {
        return None;
    }
    let file = match classifier {
        Some(c) => format!("{artifact}-{version}-{c}.{ext}"),
        None => format!("{artifact}-{version}.{ext}"),
    };
    Some(format!("{}/{artifact}/{version}/{file}", group.replace('.', "/")))
}

impl VersionService {
    /// Loader versions available for a Minecraft version, newest first.
    pub async fn loader_versions(&self, dl: &Downloader, loader: LoaderKind, mc: &str, cancel: &CancellationToken) -> AtlasResult<Vec<LoaderVersion>> {
        let src = &self.loader_sources;
        match loader {
            LoaderKind::Vanilla => Ok(vec![]),
            LoaderKind::Fabric | LoaderKind::Quilt => {
                let base = if loader == LoaderKind::Fabric { &src.fabric } else { &src.quilt };
                let b = dl.fetch_bytes(&format!("{base}/versions/loader/{mc}"), cancel).await?;
                let v: Vec<Value> = serde_json::from_slice(&b).map_err(|e| meta_err(loader.label(), e))?;
                Ok(v.iter()
                    .filter_map(|x| {
                        let l = x.get("loader")?;
                        let ver = l.get("version")?.as_str()?.to_string();
                        // Quilt has no "stable" flag; treat non-beta builds as stable
                        let stable = l.get("stable").and_then(|s| s.as_bool()).unwrap_or(!ver.contains("beta"));
                        Some(LoaderVersion { version: ver, stable })
                    })
                    .collect())
            }
            LoaderKind::Forge | LoaderKind::NeoForge => crate::forge::versions(self, dl, loader, mc, cancel).await,
        }
    }

    /// Pick the loader version to use: the requested one, or the newest stable.
    pub async fn pick_loader_version(&self, dl: &Downloader, loader: LoaderKind, mc: &str, wanted: Option<&str>, cancel: &CancellationToken) -> AtlasResult<String> {
        let list = self.loader_versions(dl, loader, mc, cancel).await?;
        if let Some(w) = wanted {
            return list.iter().find(|v| v.version == w).map(|v| v.version.clone()).ok_or_else(|| {
                AtlasError::new(ErrorCode::VersionNotFound, format!("{} {w} isn't available for Minecraft {mc}.", loader.label()))
            });
        }
        list.iter()
            .find(|v| v.stable)
            .or_else(|| list.first())
            .map(|v| v.version.clone())
            .ok_or_else(|| AtlasError::new(ErrorCode::VersionNotFound, format!("{} doesn't support Minecraft {mc} yet.", loader.label())).suggest("Pick another loader or Minecraft version."))
    }

    /// Install a loader profile on top of an installed vanilla version and
    /// return the profile's version id (what the launcher should start).
    pub async fn install_loader(&self, dl: &Downloader, loader: LoaderKind, mc: &str, loader_version: &str, progress: Arc<Progress>, cancel: CancellationToken) -> AtlasResult<String> {
        match loader {
            LoaderKind::Vanilla => Ok(mc.to_string()),
            LoaderKind::Fabric | LoaderKind::Quilt => self.install_fabric_like(dl, loader, mc, loader_version, progress, cancel).await,
            LoaderKind::Forge | LoaderKind::NeoForge => crate::forge::install(self, dl, loader, mc, loader_version, progress, cancel).await,
        }
    }

    async fn install_fabric_like(&self, dl: &Downloader, loader: LoaderKind, mc: &str, lv: &str, progress: Arc<Progress>, cancel: CancellationToken) -> AtlasResult<String> {
        let base = if loader == LoaderKind::Fabric { &self.loader_sources.fabric } else { &self.loader_sources.quilt };
        let raw = dl.fetch_bytes(&format!("{base}/versions/loader/{mc}/{lv}/profile/json"), &cancel).await?;
        let mut profile: Value = serde_json::from_slice(&raw).map_err(|e| meta_err(loader.label(), e))?;
        let id = profile.get("id").and_then(|v| v.as_str()).map(str::to_owned).ok_or_else(|| meta_err(loader.label(), "no id"))?;
        if !crate::valid_id(&id) || profile.get("inheritsFrom").and_then(|v| v.as_str()) != Some(mc) {
            return Err(meta_err(loader.label(), format!("unexpected profile {id}")));
        }
        if self.record(&id).is_ok() {
            return Ok(id); // already there
        }

        // turn {name, url, sha1?} libraries into normal downloads.artifact entries
        let mut reqs = vec![];
        let mut libs_out = vec![];
        for lib in profile.get("libraries").and_then(|l| l.as_array()).cloned().unwrap_or_default() {
            let name = lib.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            let repo = lib.get("url").and_then(|v| v.as_str()).unwrap_or("https://libraries.minecraft.net/");
            let path = maven_path(name).ok_or_else(|| meta_err(loader.label(), format!("bad library name {name}")))?;
            if !repo.starts_with("https://") {
                return Err(meta_err(loader.label(), format!("library {name} isn't served over https")));
            }
            let url = format!("{}/{path}", repo.trim_end_matches('/'));
            let sha1 = match lib.get("sha1").and_then(|v| v.as_str()) {
                Some(s) => s.to_ascii_lowercase(),
                // Quilt's profiles leave hashes out; the maven repo publishes them next to the jar
                None => {
                    let t = dl.fetch_bytes(&format!("{url}.sha1"), &cancel).await?;
                    String::from_utf8_lossy(&t).split_whitespace().next().unwrap_or_default().to_ascii_lowercase()
                }
            };
            if !crate::valid_sha1(&sha1) {
                return Err(meta_err(loader.label(), format!("no usable checksum for {name}")));
            }
            let size = lib.get("size").and_then(|v| v.as_u64());
            let dest = safe_join(&self.paths.libraries_dir(), &path)?;
            reqs.push(DownloadRequest { url: url.clone(), dest, checksum: Checksum::Sha1(sha1.clone()), size, reuse_from: self.reuse_roots.iter().filter_map(|r| safe_join(&r.join("libraries"), &path).ok()).collect() });
            let mut art = json!({"path": path, "sha1": sha1, "url": url});
            if let Some(s) = size {
                art["size"] = json!(s);
            }
            libs_out.push(json!({"name": name, "downloads": {"artifact": art}}));
        }
        tracing::info!(log_source = "install", loader = loader.label(), version = %id, libraries = reqs.len(), "installing loader");
        let records: Vec<RecordedFile> = reqs
            .iter()
            .map(|r| RecordedFile {
                path: r.dest.strip_prefix(&self.paths.data_root).unwrap_or(&r.dest).to_string_lossy().replace('\\', "/"),
                sha1: r.checksum.hex().to_string(),
                size: r.size.unwrap_or(0),
                url: r.url.clone(),
            })
            .collect();
        dl.download_all(reqs, progress, cancel).await?;

        profile["libraries"] = Value::Array(libs_out);
        self.write_profile(&id, &profile, records, loader, lv, mc)?;
        Ok(id)
    }

    /// Save a loader profile as an installed version. Library sizes we didn't
    /// know up front are filled in from disk so verify can check them.
    pub(crate) fn write_profile(&self, id: &str, profile: &Value, mut files: Vec<RecordedFile>, loader: LoaderKind, lv: &str, mc: &str) -> AtlasResult<()> {
        for f in &mut files {
            if f.size == 0
                && let Ok(m) = std::fs::metadata(self.paths.data_root.join(&f.path))
            {
                f.size = m.len();
            }
        }
        let dir = self.paths.versions_dir().join(id);
        let body = serde_json::to_vec_pretty(profile).map_err(|e| AtlasError::new(ErrorCode::Internal, "Couldn't save the loader profile.").details(e))?;
        atomic_write(&dir.join(format!("{id}.json")), &body)?;
        let record = InstallRecord {
            schema: RECORD_SCHEMA,
            id: id.into(),
            kind: crate::model::VersionKind::Release,
            source: format!("{}:{lv}", loader.modrinth()),
            release_time: profile.get("releaseTime").and_then(|v| v.as_str()).map(str::to_owned),
            installed_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            java_major: None,
            java_component: None,
            main_class: profile.get("mainClass").and_then(|v| v.as_str()).unwrap_or_default().into(),
            asset_index: String::new(),
            inherits_from: Some(mc.into()),
            files,
        };
        record.save(&dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maven_paths() {
        assert_eq!(maven_path("net.fabricmc:fabric-loader:0.20.1").unwrap(), "net/fabricmc/fabric-loader/0.20.1/fabric-loader-0.20.1.jar");
        assert_eq!(maven_path("net.minecraft:client:1.21.1:mappings@txt").unwrap(), "net/minecraft/client/1.21.1/client-1.21.1-mappings.txt");
        assert!(maven_path("a:b").is_none());
        assert!(maven_path("a:../x:1").is_none());
        assert!(maven_path("a:b/c:1").is_none());
    }

    #[test]
    fn loader_names() {
        assert_eq!(LoaderKind::parse("NeoForge"), Some(LoaderKind::NeoForge));
        assert_eq!(LoaderKind::parse("neo"), Some(LoaderKind::NeoForge));
        assert_eq!(LoaderKind::parse("rift"), None);
        assert_eq!(LoaderKind::Quilt.modrinth(), "quilt");
    }
}
