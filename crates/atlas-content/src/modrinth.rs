//! Modrinth: search, install with dependencies, and updates.
//!
//! Everything Atlas installs from Modrinth is written down in
//! `<game>/.atlas/content.json` (project, version, file, hash) so it can be
//! updated or removed cleanly later. Files are always checked against the
//! SHA-512 Modrinth publishes.

use atlas_downloads::{CancellationToken, Checksum, DownloadRequest, Downloader, DownloaderConfig, Progress};
use atlas_shared::fs_safety::{atomic_write, safe_join};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const API: &str = "https://api.modrinth.com/v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProjectType {
    #[default]
    Mod,
    Resourcepack,
    Shader,
    Modpack,
}

impl ProjectType {
    pub fn api(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::Resourcepack => "resourcepack",
            Self::Shader => "shader",
            Self::Modpack => "modpack",
        }
    }
    pub fn folder(self) -> &'static str {
        match self {
            Self::Mod | Self::Modpack => "mods",
            Self::Resourcepack => "resourcepacks",
            Self::Shader => "shaderpacks",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Mod => "Mods",
            Self::Resourcepack => "Resource packs",
            Self::Shader => "Shaders",
            Self::Modpack => "Modpacks",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub follows: u64,
    pub icon_url: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub project_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub hits: Vec<Hit>,
    pub total_hits: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionFile {
    pub url: String,
    pub filename: String,
    #[serde(default)]
    pub primary: bool,
    pub hashes: HashMap<String, String>,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub dependency_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Version {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    #[serde(default)]
    pub version_type: String,
    #[serde(default)]
    pub loaders: Vec<String>,
    #[serde(default)]
    pub game_versions: Vec<String>,
    pub files: Vec<VersionFile>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub date_published: String,
}

impl Version {
    fn primary_file(&self) -> Option<&VersionFile> {
        self.files.iter().find(|f| f.primary).or_else(|| self.files.first())
    }
}

/// One thing we installed from Modrinth.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Installed {
    pub project_id: String,
    pub title: String,
    pub version_id: String,
    pub version_number: String,
    pub file: String,
    pub folder: String,
    pub sha512: String,
    /// Installed only because something else needed it.
    #[serde(default)]
    pub dependency: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Ledger {
    #[serde(default)]
    items: BTreeMap<String, Installed>,
}

/// Where a project is being installed: loader + game version + folder.
#[derive(Debug, Clone)]
pub struct Target<'a> {
    pub game_dir: &'a Path,
    pub loader: &'a str,
    pub game_version: &'a str,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallReport {
    pub installed: Vec<Installed>,
    pub already_had: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub project_id: String,
    pub title: String,
    pub current: String,
    pub latest: String,
    pub latest_version_id: String,
}

fn net(e: impl std::fmt::Display) -> AtlasError {
    AtlasError::new(ErrorCode::NetworkError, "Couldn't reach Modrinth.").details(e.to_string()).suggest("Check your connection and try again.")
}

pub struct Modrinth {
    http: reqwest::Client,
    pub base: String,
    /// Downloads must come from here (tests point it at a local server).
    pub cdn: String,
}

impl Modrinth {
    pub fn new() -> AtlasResult<Self> {
        Self::with_base(API)
    }

    pub fn with_base(base: &str) -> AtlasResult<Self> {
        // Modrinth asks clients to identify themselves this way
        let ua = format!("sayovc/Atlas-Launcher/{} (github.com/sayovc/Atlas-Launcher)", atlas_shared::ATLAS_VERSION);
        let http = reqwest::Client::builder()
            .user_agent(ua)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| AtlasError::new(ErrorCode::Internal, "Couldn't start the network client.").details(e))?;
        Ok(Self { http, base: base.trim_end_matches('/').to_string(), cdn: "https://cdn.modrinth.com/".into() })
    }

    async fn get<T: for<'de> Deserialize<'de>>(&self, path: &str, query: &[(&str, String)]) -> AtlasResult<T> {
        let r = self.http.get(format!("{}{path}", self.base)).query(query).send().await.map_err(net)?;
        let st = r.status();
        if st.as_u16() == 404 {
            return Err(AtlasError::new(ErrorCode::VersionNotFound, "Modrinth doesn't have that.").details(path.to_string()));
        }
        if !st.is_success() {
            return Err(net(format!("{path}: HTTP {st}")));
        }
        r.json().await.map_err(net)
    }

    pub async fn search(&self, query: &str, kind: ProjectType, loader: Option<&str>, game_version: Option<&str>, offset: u32, limit: u32) -> AtlasResult<SearchResult> {
        let mut facets = vec![vec![format!("project_type:{}", kind.api())]];
        // loaders only make sense for mods; packs and shaders have their own "loaders"
        if kind == ProjectType::Mod
            && let Some(l) = loader.filter(|l| *l != "minecraft")
        {
            facets.push(vec![format!("categories:{l}")]);
        }
        if let Some(v) = game_version {
            facets.push(vec![format!("versions:{v}")]);
        }
        let index = if query.trim().is_empty() { "downloads" } else { "relevance" };
        self.get("/search", &[("query", query.into()), ("facets", serde_json::to_string(&facets).unwrap_or_default()), ("index", index.into()), ("offset", offset.to_string()), ("limit", limit.min(100).to_string())]).await
    }

    /// Versions of a project that fit the target, newest first.
    pub async fn versions(&self, project: &str, kind: ProjectType, t: &Target<'_>) -> AtlasResult<Vec<Version>> {
        let mut q = vec![("game_versions", serde_json::to_string(&[t.game_version]).unwrap_or_default())];
        if kind == ProjectType::Mod {
            q.push(("loaders", serde_json::to_string(&[t.loader]).unwrap_or_default()));
        }
        let mut v: Vec<Version> = self.get(&format!("/project/{project}/version"), &q).await?;
        // releases before betas before alphas, then newest
        let rank = |t: &str| match t {
            "release" => 0,
            "beta" => 1,
            _ => 2,
        };
        v.sort_by(|a, b| rank(&a.version_type).cmp(&rank(&b.version_type)).then(b.date_published.cmp(&a.date_published)));
        Ok(v)
    }

    pub async fn version(&self, id: &str) -> AtlasResult<Version> {
        self.get(&format!("/version/{id}"), &[]).await
    }

    pub async fn project(&self, id_or_slug: &str) -> AtlasResult<Value> {
        self.get(&format!("/project/{id_or_slug}"), &[]).await
    }

    /// Install a project (and the projects it needs) into the target.
    pub async fn install(&self, dl: &Downloader, project: &str, kind: ProjectType, t: &Target<'_>, progress: Arc<Progress>, cancel: CancellationToken) -> AtlasResult<InstallReport> {
        let mut ledger = read_ledger(t.game_dir);
        let have: HashSet<String> = ledger.items.keys().cloned().collect();
        let mut plan: Vec<(Version, String, bool)> = vec![]; // version, title, is_dependency
        let mut report = InstallReport { installed: vec![], already_had: vec![], skipped: vec![] };
        let mut queue: Vec<(String, Option<String>, bool)> = vec![(project.to_string(), None, false)];
        let mut seen = HashSet::new();
        while let Some((pid, vid, is_dep)) = queue.pop() {
            if !seen.insert(pid.clone()) {
                continue;
            }
            let info = self.project(&pid).await?;
            let real_id = info.get("id").and_then(|v| v.as_str()).unwrap_or(&pid).to_string();
            let title = info.get("title").and_then(|v| v.as_str()).unwrap_or(&pid).to_string();
            if have.contains(&real_id) {
                report.already_had.push(title);
                continue;
            }
            let ptype = match info.get("project_type").and_then(|v| v.as_str()) {
                Some("resourcepack") => ProjectType::Resourcepack,
                Some("shader") => ProjectType::Shader,
                _ => kind,
            };
            let v = match vid {
                Some(v) => self.version(&v).await?,
                None => match self.versions(&real_id, ptype, t).await?.into_iter().next() {
                    Some(v) => v,
                    None if is_dep => {
                        report.skipped.push(format!("{title} (no build for {} {})", t.loader, t.game_version));
                        continue;
                    }
                    None => {
                        return Err(AtlasError::new(ErrorCode::VersionNotFound, format!("{title} has no build for {} on Minecraft {}.", t.loader, t.game_version))
                            .suggest("Try another Minecraft version or loader."));
                    }
                },
            };
            for d in &v.dependencies {
                if d.dependency_type == "required"
                    && let Some(p) = d.project_id.clone()
                {
                    queue.push((p, d.version_id.clone(), true));
                }
            }
            plan.push((v, title, is_dep));
        }

        let mut reqs = vec![];
        let mut entries = vec![];
        for (v, title, is_dep) in &plan {
            let f = v.primary_file().ok_or_else(|| AtlasError::new(ErrorCode::DownloadFailed, format!("{title} has no file to download.")))?;
            let sha = f.hashes.get("sha512").cloned().ok_or_else(|| AtlasError::new(ErrorCode::DownloadFailed, format!("{title} has no checksum.")))?;
            if f.filename.contains('/') || f.filename.contains('\\') || f.filename.starts_with('.') {
                return Err(AtlasError::new(ErrorCode::PathTraversal, "Modrinth sent an odd file name.").details(&f.filename));
            }
            if !f.url.starts_with(&self.cdn) {
                return Err(AtlasError::new(ErrorCode::DownloadFailed, "Unexpected download host.").details(&f.url));
            }
            let folder = match v.loaders.iter().any(|l| l == "iris" || l == "optifine" || l == "canvas") && !v.loaders.iter().any(|l| l == "fabric" || l == "forge" || l == "neoforge" || l == "quilt") {
                true => "shaderpacks",
                false if v.loaders.iter().all(|l| l == "minecraft") && !v.loaders.is_empty() => "resourcepacks",
                false => kind_folder_for(&plan, v, kind),
            };
            let dest = safe_join(&t.game_dir.join(folder), &f.filename)?;
            reqs.push(DownloadRequest { url: f.url.clone(), dest, checksum: Checksum::Sha512(sha.to_ascii_lowercase()), size: Some(f.size), reuse_from: vec![] });
            entries.push(Installed {
                project_id: v.project_id.clone(),
                title: title.clone(),
                version_id: v.id.clone(),
                version_number: v.version_number.clone(),
                file: f.filename.clone(),
                folder: folder.into(),
                sha512: sha.to_ascii_lowercase(),
                dependency: *is_dep,
            });
        }
        dl.download_all(reqs, progress, cancel).await?;
        for e in entries {
            ledger.items.insert(e.project_id.clone(), e.clone());
            report.installed.push(e);
        }
        write_ledger(t.game_dir, &ledger)?;
        Ok(report)
    }

    /// Newer builds for everything installed from Modrinth, in one request.
    pub async fn check_updates(&self, t: &Target<'_>) -> AtlasResult<Vec<UpdateInfo>> {
        let ledger = read_ledger(t.game_dir);
        if ledger.items.is_empty() {
            return Ok(vec![]);
        }
        let hashes: Vec<&str> = ledger.items.values().map(|i| i.sha512.as_str()).collect();
        let body = json!({"hashes": hashes, "algorithm": "sha512", "loaders": [t.loader], "game_versions": [t.game_version]});
        let r = self.http.post(format!("{}/version_files/update", self.base)).json(&body).send().await.map_err(net)?;
        if !r.status().is_success() {
            return Err(net(format!("update check: HTTP {}", r.status())));
        }
        let map: HashMap<String, Version> = r.json().await.map_err(net)?;
        let mut out = vec![];
        for i in ledger.items.values() {
            if let Some(v) = map.get(&i.sha512)
                && v.id != i.version_id
            {
                out.push(UpdateInfo { project_id: i.project_id.clone(), title: i.title.clone(), current: i.version_number.clone(), latest: v.version_number.clone(), latest_version_id: v.id.clone() });
            }
        }
        Ok(out)
    }

    /// Swap installed files for their newest builds.
    pub async fn apply_updates(&self, dl: &Downloader, t: &Target<'_>, updates: &[UpdateInfo], progress: Arc<Progress>, cancel: CancellationToken) -> AtlasResult<usize> {
        let mut ledger = read_ledger(t.game_dir);
        let mut n = 0;
        for u in updates {
            let Some(old) = ledger.items.get(&u.project_id).cloned() else { continue };
            let v = self.version(&u.latest_version_id).await?;
            let Some(f) = v.primary_file() else { continue };
            let Some(sha) = f.hashes.get("sha512") else { continue };
            if f.filename.contains('/') || f.filename.contains('\\') || !f.url.starts_with(&self.cdn) {
                continue;
            }
            let dir = t.game_dir.join(&old.folder);
            let dest = safe_join(&dir, &f.filename)?;
            dl.download(&DownloadRequest { url: f.url.clone(), dest, checksum: Checksum::Sha512(sha.to_ascii_lowercase()), size: Some(f.size), reuse_from: vec![] }, &progress, &cancel).await?;
            if old.file != f.filename {
                let _ = std::fs::remove_file(safe_join(&dir, &old.file)?);
                let _ = std::fs::remove_file(safe_join(&dir, &format!("{}.disabled", old.file))?);
            }
            ledger.items.insert(u.project_id.clone(), Installed { version_id: v.id.clone(), version_number: v.version_number.clone(), file: f.filename.clone(), sha512: sha.to_ascii_lowercase(), ..old });
            n += 1;
        }
        write_ledger(t.game_dir, &ledger)?;
        Ok(n)
    }
}

fn kind_folder_for(_plan: &[(Version, String, bool)], _v: &Version, kind: ProjectType) -> &'static str {
    kind.folder()
}

fn ledger_path(game_dir: &Path) -> PathBuf {
    game_dir.join(".atlas").join("content.json")
}

fn read_ledger(game_dir: &Path) -> Ledger {
    std::fs::read(ledger_path(game_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_ledger(game_dir: &Path, l: &Ledger) -> AtlasResult<()> {
    let b = serde_json::to_vec_pretty(l).map_err(|e| AtlasError::new(ErrorCode::Internal, "Couldn't save the content list.").details(e))?;
    atomic_write(&ledger_path(game_dir), &b)
}

/// What's been installed from Modrinth (files the user deleted by hand are dropped).
pub fn installed(game_dir: &Path) -> Vec<Installed> {
    let mut l = read_ledger(game_dir);
    let before = l.items.len();
    l.items.retain(|_, i| {
        let d = game_dir.join(&i.folder);
        d.join(&i.file).exists() || d.join(format!("{}.disabled", i.file)).exists()
    });
    if l.items.len() != before {
        let _ = write_ledger(game_dir, &l);
    }
    l.items.into_values().collect()
}

/// Forget and delete one Modrinth project.
pub fn uninstall(game_dir: &Path, project_id: &str) -> AtlasResult<()> {
    let mut l = read_ledger(game_dir);
    let i = l.items.remove(project_id).ok_or_else(|| AtlasError::new(ErrorCode::InvalidArgument, "That wasn't installed from Modrinth."))?;
    let dir = game_dir.join(&i.folder);
    for name in [i.file.clone(), format!("{}.disabled", i.file)] {
        let p = safe_join(&dir, &name)?;
        if p.exists() {
            std::fs::remove_file(&p).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "delete a file", &p, e))?;
        }
    }
    write_ledger(game_dir, &l)
}

/// A downloader set up the way Modrinth's CDN likes.
pub fn downloader(max_concurrent: usize) -> AtlasResult<Downloader> {
    Downloader::new(DownloaderConfig { max_concurrent, user_agent: format!("sayovc/Atlas-Launcher/{}", atlas_shared::ATLAS_VERSION), ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use atlas_downloads::testing::serve_dynamic;
    use sha2::Digest;

    fn sha512(b: &[u8]) -> String {
        sha2::Sha512::digest(b).iter().map(|x| format!("{x:02x}")).collect()
    }

    fn version(base: &str, pid: &str, file: &str, body: &[u8], deps: Value) -> Value {
        json!({"id": format!("v-{pid}"), "project_id": pid, "name": pid, "version_number": "1.0", "version_type": "release",
               "loaders": ["fabric"], "game_versions": ["1.21.4"], "date_published": "2026-01-01",
               "files": [{"url": format!("{base}/cdn/{file}"), "filename": file, "primary": true, "hashes": {"sha512": sha512(body)}, "size": body.len()}],
               "dependencies": deps})
    }

    #[tokio::test]
    async fn installs_with_required_dependencies_only() {
        let (srv, files) = serve_dynamic(true);
        let b = srv.base.clone();
        let (main, lib, opt) = (b"main mod".to_vec(), b"library".to_vec(), b"optional".to_vec());
        {
            let mut f = files.lock().unwrap();
            for (pid, title) in [("aaa", "Main"), ("bbb", "Lib"), ("ccc", "Optional")] {
                f.insert(format!("/api/project/{pid}"), serde_json::to_vec(&json!({"id": pid, "title": title, "project_type": "mod"})).unwrap());
            }
            // query strings are ignored by the test server, which is fine here
            f.insert("/api/project/aaa/version".into(), serde_json::to_vec(&json!([version(&b, "aaa", "main.jar", &main, json!([{"project_id": "bbb", "dependency_type": "required"}, {"project_id": "ccc", "dependency_type": "optional"}]))])).unwrap());
            f.insert("/api/project/bbb/version".into(), serde_json::to_vec(&json!([version(&b, "bbb", "lib.jar", &lib, json!([]))])).unwrap());
            f.insert("/api/project/ccc/version".into(), serde_json::to_vec(&json!([version(&b, "ccc", "opt.jar", &opt, json!([]))])).unwrap());
            f.insert("/cdn/main.jar".into(), main.clone());
            f.insert("/cdn/lib.jar".into(), lib.clone());
        }
        let t = tempfile::tempdir().unwrap();
        let mut mr = Modrinth::with_base(&format!("{b}/api")).unwrap();
        mr.cdn = format!("{b}/cdn/");
        let dl = Downloader::new(Default::default()).unwrap();
        let target = Target { game_dir: t.path(), loader: "fabric", game_version: "1.21.4" };
        let rep = mr.install(&dl, "aaa", ProjectType::Mod, &target, Arc::new(Progress::new()), CancellationToken::new()).await.unwrap();
        assert_eq!(rep.installed.len(), 2, "main + required lib, not the optional one");
        assert_eq!(std::fs::read(t.path().join("mods/lib.jar")).unwrap(), lib);
        assert!(!t.path().join("mods/opt.jar").exists());
        let inst = installed(t.path());
        assert!(inst.iter().any(|i| i.project_id == "bbb" && i.dependency));

        // installing again is a no-op
        let rep = mr.install(&dl, "aaa", ProjectType::Mod, &target, Arc::new(Progress::new()), CancellationToken::new()).await.unwrap();
        assert!(rep.installed.is_empty() && rep.already_had.len() == 1);

        uninstall(t.path(), "aaa").unwrap();
        assert!(!t.path().join("mods/main.jar").exists());
        assert_eq!(installed(t.path()).len(), 1);
    }

    #[tokio::test]
    async fn rejects_bad_files() {
        let (srv, files) = serve_dynamic(true);
        let b = srv.base.clone();
        let body = b"x".to_vec();
        {
            let mut f = files.lock().unwrap();
            f.insert("/api/project/evil".into(), serde_json::to_vec(&json!({"id": "evil", "title": "Evil", "project_type": "mod"})).unwrap());
            let mut v = version(&b, "evil", "../../escape.jar", &body, json!([]));
            v["files"][0]["filename"] = json!("../../escape.jar");
            f.insert("/api/project/evil/version".into(), serde_json::to_vec(&json!([v])).unwrap());
            f.insert("/api/project/tamper".into(), serde_json::to_vec(&json!({"id": "tamper", "title": "Tamper", "project_type": "mod"})).unwrap());
            f.insert("/api/project/tamper/version".into(), serde_json::to_vec(&json!([version(&b, "tamper", "t.jar", b"what modrinth said", json!([]))])).unwrap());
            f.insert("/cdn/t.jar".into(), b"something else entirely".to_vec());
        }
        let t = tempfile::tempdir().unwrap();
        let mut mr = Modrinth::with_base(&format!("{b}/api")).unwrap();
        mr.cdn = format!("{b}/cdn/");
        let dl = Downloader::new(DownloaderConfig { retries: 0, ..Default::default() }).unwrap();
        let target = Target { game_dir: t.path(), loader: "fabric", game_version: "1.21.4" };
        let e = mr.install(&dl, "evil", ProjectType::Mod, &target, Arc::new(Progress::new()), CancellationToken::new()).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::PathTraversal);
        let e = mr.install(&dl, "tamper", ProjectType::Mod, &target, Arc::new(Progress::new()), CancellationToken::new()).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::ChecksumMismatch);
        assert!(installed(t.path()).is_empty());
    }
}
