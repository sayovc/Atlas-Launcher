use crate::model::{self, AssetIndex, Manifest, VersionJson, VersionKind};
use crate::plan;
use crate::record::{InstallRecord, InstalledVersion, RecordedFile, VerifyReport};
use crate::rules::Platform;
use crate::valid_id;
use atlas_downloads::{sha1_hex, CancellationToken, DownloadRequest, Downloader, Progress};
use atlas_shared::fs_safety::{atomic_write, remove_tree_no_follow};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where version data lives (all inside the Atlas data root).
#[derive(Debug, Clone)]
pub struct VersionPaths {
    pub data_root: PathBuf,
}

impl VersionPaths {
    pub fn versions_dir(&self) -> PathBuf { self.data_root.join("versions") }
    pub fn staging_dir(&self) -> PathBuf { self.versions_dir().join(".staging") }
    pub fn libraries_dir(&self) -> PathBuf { self.data_root.join("libraries") }
    pub fn assets_dir(&self) -> PathBuf { self.data_root.join("assets") }
    pub fn manifest_cache(&self) -> PathBuf { self.data_root.join("cache/manifest/version_manifest_v2.json") }
    fn version_dir(&self, id: &str) -> PathBuf { self.versions_dir().join(id) }
}

/// Remote endpoints. Defaults are Mojang's official URLs; tests point them at
/// a local server.
#[derive(Debug, Clone)]
pub struct VersionSources {
    pub manifest_url: String,
    pub assets_base_url: String,
}

impl Default for VersionSources {
    fn default() -> Self {
        Self { manifest_url: model::MOJANG_MANIFEST_URL.into(), assets_base_url: model::MOJANG_ASSETS_URL.into() }
    }
}

pub struct VersionService {
    pub paths: VersionPaths,
    pub sources: VersionSources,
    pub loader_sources: crate::loaders::LoaderSources,
    /// Other launchers' folders to reuse verified files from (read-only).
    pub reuse_roots: Vec<PathBuf>,
    pub platform: Platform,
}

fn not_found(id: &str) -> AtlasError {
    AtlasError::new(ErrorCode::VersionNotFound, format!("Version '{id}' was not found."))
        .suggest("Run `atlas versions --refresh` (or press Refresh) to update the version list.")
}

fn invalid_id(id: &str) -> AtlasError {
    AtlasError::new(ErrorCode::InvalidArgument, format!("'{id}' is not a valid version id."))
        .suggest("Use an id from the version list, e.g. 1.21.4.")
}

fn rel(data_root: &Path, p: &Path) -> String {
    p.strip_prefix(data_root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl VersionService {
    pub fn new(data_root: PathBuf, sources: VersionSources, reuse_roots: Vec<PathBuf>) -> Self {
        Self { paths: VersionPaths { data_root }, sources, loader_sources: Default::default(), reuse_roots, platform: Platform::current() }
    }

    /// Last manifest fetched, if any (no network).
    pub fn cached_manifest(&self) -> Option<Manifest> {
        let b = std::fs::read(self.paths.manifest_cache()).ok()?;
        serde_json::from_slice(&b).ok()
    }

    /// Fetch the manifest. On network failure, fall back to the cached copy
    /// (returned with `stale = true`) so offline use still works.
    pub async fn manifest(&self, dl: &Downloader, refresh: bool, cancel: &CancellationToken) -> AtlasResult<(Manifest, bool)> {
        if !refresh && let Some(m) = self.cached_manifest() {
            return Ok((m, false));
        }
        match dl.fetch_bytes(&self.sources.manifest_url, cancel).await {
            Ok(bytes) => {
                let m: Manifest = serde_json::from_slice(&bytes).map_err(|e| {
                    AtlasError::new(ErrorCode::DownloadFailed, "The version list could not be read.").details(e)
                        .suggest("Try again later.")
                })?;
                atomic_write(&self.paths.manifest_cache(), &bytes)?;
                tracing::info!(versions = m.versions.len(), latest = %m.latest.release, "version list refreshed");
                Ok((m, false))
            }
            Err(e) if e.code == ErrorCode::Cancelled => Err(e),
            Err(e) => match self.cached_manifest() {
                Some(m) => {
                    tracing::warn!(code = %e.code, "using cached version list: {}", e.details);
                    Ok((m, true))
                }
                None => Err(e),
            },
        }
    }

    pub fn installed(&self) -> AtlasResult<Vec<InstalledVersion>> {
        let dir = self.paths.versions_dir();
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(AtlasError::io(ErrorCode::InvalidInstallation, "list installed versions", &dir, e)),
        };
        let mut out = Vec::new();
        for e in rd.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().into_owned();
            if !valid_id(&name) || !e.path().is_dir() {
                continue;
            }
            match InstallRecord::load(&e.path()) {
                Ok(Some(r)) if r.id == name => out.push(r.summary(e.path())),
                Ok(_) => {}
                Err(err) => tracing::warn!(code = %err.code, version = %name, "{}", err.details),
            }
        }
        out.sort_by(|a, b| b.release_time.cmp(&a.release_time));
        Ok(out)
    }

    pub fn record(&self, id: &str) -> AtlasResult<InstallRecord> {
        if !valid_id(id) {
            return Err(invalid_id(id));
        }
        InstallRecord::load(&self.paths.version_dir(id))?.ok_or_else(|| {
            AtlasError::new(ErrorCode::VersionNotFound, format!("Version '{id}' is not installed."))
                .suggest(format!("Install it first: atlas install {id}"))
        })
    }

    /// Install (or finish an interrupted install of) `id`. Idempotent: an
    /// already-installed version is verified and repaired instead.
    pub async fn install(&self, dl: &Downloader, manifest: &Manifest, id: &str, progress: Arc<Progress>, cancel: CancellationToken) -> AtlasResult<InstalledVersion> {
        if !valid_id(id) {
            return Err(invalid_id(id));
        }
        let entry = manifest.get(id).ok_or_else(|| not_found(id))?;
        let final_dir = self.paths.version_dir(id);
        let already = InstallRecord::load(&final_dir)?.is_some();
        // Work in staging unless repairing an installed version in place.
        let work_dir = if already { final_dir.clone() } else { self.paths.staging_dir().join(id) };
        std::fs::create_dir_all(&work_dir).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "create a version folder", &work_dir, e))?;
        tracing::info!(log_source = "install", version = id, repair = already, "installing version");

        // 1. Version JSON, verified against the manifest's sha1.
        let json_bytes = dl.fetch_bytes(&entry.url, &cancel).await?;
        let actual = sha1_hex(&json_bytes);
        if !actual.eq_ignore_ascii_case(&entry.sha1) {
            return Err(AtlasError::new(ErrorCode::ChecksumMismatch, "The version metadata did not match its checksum.")
                .details(format!("{}: expected {}, got {actual}", entry.url, entry.sha1))
                .suggest("Refresh the version list and try again."));
        }
        let v: VersionJson = serde_json::from_slice(&json_bytes).map_err(|e| {
            AtlasError::new(ErrorCode::InvalidInstallation, "The version metadata could not be read.").details(e)
        })?;
        if v.id != id {
            return Err(AtlasError::new(ErrorCode::InvalidInstallation, "The version metadata describes a different version.")
                .details(format!("asked for {id}, got {}", v.id)));
        }
        let json_path = work_dir.join(format!("{id}.json"));
        atomic_write(&json_path, &json_bytes)?;

        // 2. Asset index first (it lists the asset objects).
        let idx_req = plan::asset_index(&v, &self.paths, &self.reuse_roots)?;
        progress.plan(1, idx_req.size.unwrap_or(0));
        let o = dl.download(&idx_req, &progress, &cancel).await?;
        progress.file_done(o);
        let idx_bytes = std::fs::read(&idx_req.dest).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "read the asset index", &idx_req.dest, e))?;
        let index: AssetIndex = serde_json::from_slice(&idx_bytes).map_err(|e| {
            AtlasError::new(ErrorCode::InvalidInstallation, "The asset index could not be read.").details(e)
        })?;

        // 3. Everything else.
        let mut reqs: Vec<DownloadRequest> = Vec::new();
        reqs.push(plan::client_jar(&v, &work_dir, &self.reuse_roots)?);
        reqs.extend(plan::libraries(&v, &self.paths, &self.reuse_roots, self.platform)?);
        if let Some(l) = plan::logging_config(&v, &self.paths)? {
            reqs.push(l);
        }
        reqs.extend(plan::assets(&index, &self.paths, &self.reuse_roots, &self.sources.assets_base_url)?);
        tracing::info!(log_source = "install", version = id, files = reqs.len(), "downloading files");

        let mut recorded: Vec<RecordedFile> = Vec::with_capacity(reqs.len() + 1);
        let final_path = |p: &Path| -> PathBuf {
            // Record paths as they will be after staging is promoted.
            match p.strip_prefix(&work_dir) {
                Ok(r) => final_dir.join(r),
                Err(_) => p.to_path_buf(),
            }
        };
        for r in std::iter::once(&idx_req).chain(reqs.iter()) {
            recorded.push(RecordedFile {
                path: rel(&self.paths.data_root, &final_path(&r.dest)),
                sha1: r.checksum.hex().to_string(),
                size: r.size.unwrap_or(0),
                url: r.url.clone(),
            });
        }
        recorded.push(RecordedFile {
            path: rel(&self.paths.data_root, &final_dir.join(format!("{id}.json"))),
            sha1: entry.sha1.to_ascii_lowercase(),
            size: json_bytes.len() as u64,
            url: entry.url.clone(),
        });

        dl.download_all(reqs, progress.clone(), cancel.clone()).await?;

        // 4. Commit: write the record, then promote staging atomically.
        let record = InstallRecord {
            schema: crate::record::RECORD_SCHEMA,
            id: id.to_string(),
            kind: entry.kind,
            source: "mojang".into(),
            release_time: Some(entry.release_time.clone()),
            installed_at: unix_now(),
            java_major: v.java_version.as_ref().map(|j| j.major_version),
            java_component: v.java_version.as_ref().and_then(|j| j.component.clone()),
            main_class: v.main_class.clone(),
            asset_index: v.asset_index.id.clone(),
            inherits_from: None,
            files: recorded,
        };
        record.save(&work_dir)?;
        if !already {
            if final_dir.exists() {
                // A folder without install.toml (e.g. from another tool or a crash) – never merge into it.
                return Err(AtlasError::new(ErrorCode::InvalidInstallation, format!("A folder named '{id}' already exists but is not an Atlas install.")).details(final_dir.display())
                    .suggest("Move or delete that folder, then install again."));
            }
            std::fs::rename(&work_dir, &final_dir).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "finish installing the version", &final_dir, e))?;
        }
        tracing::info!(log_source = "install", version = id, "version installed");
        Ok(record.summary(final_dir))
    }

    /// Re-hash every file of an installed version.
    pub fn verify(&self, id: &str) -> AtlasResult<VerifyReport> {
        let r = self.record(id)?;
        let rep = r.verify(&self.paths.data_root)?;
        if rep.ok() {
            tracing::info!(log_source = "install", version = id, files = rep.checked, "verified");
        } else {
            tracing::warn!(log_source = "install", version = id, missing = rep.missing.len(), corrupt = rep.corrupt.len(), "verification found problems");
        }
        Ok(rep)
    }

    /// Re-download only files that are missing or corrupt, using the URLs and
    /// hashes in the install record (no manifest needed - works offline for
    /// files still in the reuse roots).
    pub async fn repair(&self, dl: &Downloader, id: &str, progress: Arc<Progress>, cancel: CancellationToken) -> AtlasResult<VerifyReport> {
        let r = self.record(id)?;
        let mut reqs = Vec::new();
        for f in &r.files {
            let dest = atlas_shared::fs_safety::safe_join(&self.paths.data_root, &f.path)?;
            reqs.push(DownloadRequest {
                url: f.url.clone(),
                dest,
                checksum: atlas_downloads::Checksum::Sha1(f.sha1.clone()),
                size: Some(f.size),
                reuse_from: vec![],
            });
        }
        tracing::info!(log_source = "install", version = id, files = reqs.len(), "repairing");
        dl.download_all(reqs, progress, cancel).await?;
        self.verify(id)
    }

    /// Delete an installed version's own folder. Shared libraries/assets are
    /// kept (other versions may use them). Confirmation is enforced by core.
    pub fn remove(&self, id: &str) -> AtlasResult<()> {
        let _ = self.record(id)?; // must be an Atlas-installed version
        remove_tree_no_follow(&self.paths.version_dir(id))?;
        let _ = remove_tree_no_follow(&self.paths.staging_dir().join(id));
        tracing::info!(log_source = "install", version = id, "version removed");
        Ok(())
    }

    /// Newest release in the manifest if it is newer than every installed release.
    pub fn update_available(&self, manifest: &Manifest) -> AtlasResult<Option<String>> {
        let installed = self.installed()?;
        let latest = &manifest.latest.release;
        if installed.iter().any(|v| &v.id == latest) {
            return Ok(None);
        }
        let Some(latest_entry) = manifest.get(latest) else { return Ok(None) };
        let newest_installed = installed.iter().filter(|v| v.kind == VersionKind::Release).filter_map(|v| v.release_time.clone()).max();
        Ok(match newest_installed {
            Some(t) if t >= latest_entry.release_time => None,
            Some(_) => Some(latest.clone()),
            None => None, // nothing installed: not an "update"
        })
    }
}
