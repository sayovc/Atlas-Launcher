//! Version management and background jobs, exposed through [`crate::Atlas`].

use crate::{Atlas, Confirmation};
use atlas_downloads::{CancellationToken, Downloader, DownloaderConfig, Progress, ProgressSnapshot};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use atlas_versions::{DetectedInstall, InstalledVersion, Manifest, VerifyReport, VersionKind, VersionService, VersionSources};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub type JobId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Install,
    Repair,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JobState {
    Running,
    Done,
    Failed { error: AtlasError },
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobSnapshot {
    pub id: JobId,
    pub kind: JobKind,
    pub version: String,
    pub state: JobState,
    pub progress: ProgressSnapshot,
    pub started_unix: u64,
    pub elapsed_secs: f64,
    /// Average bytes/second since start.
    pub speed_bps: f64,
}

struct Job {
    kind: JobKind,
    version: String,
    state: JobState,
    progress: Arc<Progress>,
    cancel: CancellationToken,
    started: Instant,
    started_unix: u64,
    finished: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct Jobs {
    next: JobId,
    map: BTreeMap<JobId, Job>,
}

impl Jobs {
    pub(crate) fn any_running(&self) -> bool {
        self.map.values().any(|j| j.state == JobState::Running)
    }
}

/// One row of the version list.
#[derive(Debug, Clone, Serialize)]
pub struct VersionRow {
    pub id: String,
    pub kind: VersionKind,
    pub release_time: String,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct VersionList {
    pub latest_release: String,
    pub latest_snapshot: String,
    /// True when the network was unreachable and a cached list is shown.
    pub stale: bool,
    pub versions: Vec<VersionRow>,
}

fn sources_from_env() -> VersionSources {
    let mut s = VersionSources::default();
    if let Ok(u) = std::env::var("ATLAS_VERSION_MANIFEST_URL") && !u.is_empty() {
        s.manifest_url = u;
    }
    if let Ok(u) = std::env::var("ATLAS_ASSETS_BASE_URL") && !u.is_empty() {
        s.assets_base_url = u.trim_end_matches('/').to_string();
    }
    s
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Atlas {
    fn version_service(&self) -> VersionService {
        let settings = self.settings();
        let reuse = atlas_versions::detect_minecraft_installs(settings.game.minecraft_dir.as_deref())
            .into_iter()
            .map(|d| d.path)
            .collect();
        VersionService::new(self.paths().data_root, sources_from_env(), reuse)
    }

    pub(crate) fn version_service_pub(&self) -> VersionService {
        self.version_service()
    }

    pub(crate) fn downloader_pub(&self) -> AtlasResult<Downloader> {
        self.downloader()
    }

    fn downloader(&self) -> AtlasResult<Downloader> {
        let s = self.settings();
        Downloader::new(DownloaderConfig {
            max_concurrent: s.downloads.max_concurrent as usize,
            rate_limit: s.downloads.bandwidth_limit_kib.map(|k| k as u64 * 1024),
            ..Default::default()
        })
    }

    fn manifest(&self, refresh: bool) -> AtlasResult<(Manifest, bool)> {
        let svc = self.version_service();
        let dl = self.downloader()?;
        self.rt.block_on(svc.manifest(&dl, refresh, &CancellationToken::new()))
    }

    /// The version list. `refresh` forces a network fetch (falls back to the
    /// cache when offline, marking the result `stale`).
    pub fn versions(&self, refresh: bool) -> AtlasResult<VersionList> {
        let (m, stale) = self.manifest(refresh)?;
        let installed: std::collections::HashSet<String> = self.installed_versions()?.into_iter().map(|v| v.id).collect();
        Ok(VersionList {
            latest_release: m.latest.release.clone(),
            latest_snapshot: m.latest.snapshot.clone(),
            stale,
            versions: m
                .versions
                .iter()
                .map(|e| VersionRow { id: e.id.clone(), kind: e.kind, release_time: e.release_time.clone(), installed: installed.contains(&e.id) })
                .collect(),
        })
    }

    pub fn installed_versions(&self) -> AtlasResult<Vec<InstalledVersion>> {
        self.version_service().installed()
    }

    /// Newest release not yet installed, from the cached list (no network).
    pub fn version_update(&self) -> Option<String> {
        let svc = self.version_service();
        let m = svc.cached_manifest()?;
        svc.update_available(&m).ok().flatten()
    }

    pub fn detect_minecraft(&self) -> Vec<DetectedInstall> {
        atlas_versions::detect_minecraft_installs(self.settings().game.minecraft_dir.as_deref())
    }

    /// Resolve `latest` / `latest-snapshot` aliases.
    pub fn resolve_version(&self, id: &str) -> AtlasResult<String> {
        if id != "latest" && id != "latest-snapshot" {
            return Ok(id.to_string());
        }
        let (m, _) = self.manifest(false)?;
        Ok(if id == "latest" { m.latest.release } else { m.latest.snapshot })
    }

    fn job_lock(&self) -> std::sync::MutexGuard<'_, Jobs> {
        self.jobs.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn register_job(&self, kind: JobKind, version: &str) -> AtlasResult<(JobId, Arc<Progress>, CancellationToken)> {
        let mut jobs = self.job_lock();
        if jobs.map.values().any(|j| j.version == version && j.state == JobState::Running) {
            return Err(AtlasError::new(ErrorCode::InvalidArgument, format!("Version '{version}' is already being installed or repaired."))
                .suggest("Wait for it to finish, or cancel it on the Downloads page."));
        }
        jobs.next += 1;
        let id = jobs.next;
        let progress = Arc::new(Progress::new());
        let cancel = CancellationToken::new();
        jobs.map.insert(id, Job {
            kind,
            version: version.to_string(),
            state: JobState::Running,
            progress: progress.clone(),
            cancel: cancel.clone(),
            started: Instant::now(),
            started_unix: now_unix(),
            finished: None,
        });
        Ok((id, progress, cancel))
    }

    fn finish_job(jobs: &Mutex<Jobs>, id: JobId, result: AtlasResult<()>) {
        let mut j = jobs.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(job) = j.map.get_mut(&id) {
            job.finished = Some(Instant::now());
            job.state = match result {
                Ok(()) => JobState::Done,
                Err(e) if e.code == ErrorCode::Cancelled => JobState::Cancelled,
                Err(e) => JobState::Failed { error: e },
            };
        }
    }

    /// Start installing a version in the background. Returns immediately.
    pub fn start_install(&self, id: &str) -> AtlasResult<JobId> {
        let id = self.resolve_version(id)?;
        let (m, _) = self.manifest(false)?;
        // Unknown in cached list? refresh once before failing.
        let m = if m.get(&id).is_none() { self.manifest(true)?.0 } else { m };
        if m.get(&id).is_none() {
            return Err(AtlasError::new(ErrorCode::VersionNotFound, format!("Version '{id}' was not found."))
                .suggest("Check the id with `atlas versions --all`."));
        }
        let svc = self.version_service();
        let dl = self.downloader()?;
        let (job, progress, cancel) = self.register_job(JobKind::Install, &id)?;
        let jobs = self.jobs.clone();
        self.rt.spawn(async move {
            let r = svc.install(&dl, &m, &id, progress, cancel).await.map(|_| ());
            Self::finish_job(&jobs, job, r);
        });
        Ok(job)
    }

    /// Start repairing an installed version in the background.
    pub fn start_repair(&self, id: &str) -> AtlasResult<JobId> {
        let svc = self.version_service();
        svc.record(id)?;
        let dl = self.downloader()?;
        let (job, progress, cancel) = self.register_job(JobKind::Repair, id)?;
        let jobs = self.jobs.clone();
        let id = id.to_string();
        self.rt.spawn(async move {
            let r = svc.repair(&dl, &id, progress, cancel).await.and_then(|rep| {
                if rep.ok() { Ok(()) } else {
                    Err(AtlasError::new(ErrorCode::InvalidInstallation, "Some files are still invalid after repair.")
                        .details(format!("missing: {:?}, corrupt: {:?}", rep.missing, rep.corrupt)))
                }
            });
            Self::finish_job(&jobs, job, r);
        });
        Ok(job)
    }

    pub fn verify_version(&self, id: &str) -> AtlasResult<VerifyReport> {
        self.version_service().verify(id)
    }

    /// Delete an installed version (destructive). Shared libraries and assets are kept.
    pub fn remove_version(&self, id: &str, confirm: Confirmation) -> AtlasResult<()> {
        confirm.require(&format!("remove version {id}"))?;
        if self.job_lock().map.values().any(|j| j.version == id && j.state == JobState::Running) {
            return Err(AtlasError::new(ErrorCode::InvalidArgument, format!("Version '{id}' is busy."))
                .suggest("Wait for its install or repair to finish, or cancel it."));
        }
        self.version_service().remove(id)
    }

    pub fn jobs(&self) -> Vec<JobSnapshot> {
        let jobs = self.job_lock();
        jobs.map
            .iter()
            .rev()
            .map(|(id, j)| {
                let elapsed = j.finished.unwrap_or_else(Instant::now).duration_since(j.started).as_secs_f64();
                let progress = j.progress.snapshot();
                JobSnapshot {
                    id: *id,
                    kind: j.kind,
                    version: j.version.clone(),
                    state: j.state.clone(),
                    progress,
                    started_unix: j.started_unix,
                    elapsed_secs: elapsed,
                    speed_bps: if elapsed > 0.0 { progress.done_bytes as f64 / elapsed } else { 0.0 },
                }
            })
            .collect()
    }

    pub fn job(&self, id: JobId) -> Option<JobSnapshot> {
        self.jobs().into_iter().find(|j| j.id == id)
    }

    pub fn cancel_job(&self, id: JobId) -> bool {
        match self.job_lock().map.get(&id) {
            Some(j) if j.state == JobState::Running => {
                j.cancel.cancel();
                true
            }
            _ => false,
        }
    }

    /// Remove finished jobs from the list.
    pub fn clear_finished_jobs(&self) {
        self.job_lock().map.retain(|_, j| j.state == JobState::Running);
    }

    /// Block until a job finishes, calling `on_tick` about every 250 ms.
    pub fn wait_job(&self, id: JobId, mut on_tick: impl FnMut(&JobSnapshot)) -> Option<JobSnapshot> {
        loop {
            let snap = self.job(id)?;
            on_tick(&snap);
            if snap.state != JobState::Running {
                return Some(snap);
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
}
