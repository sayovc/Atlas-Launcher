//! Modrinth browsing and bundles, scoped to an installation.

use crate::{Atlas, Confirmation, Installation, InstallationChanges, JobId, JobKind};
use atlas_content::bundles::{self, Bundle};
use atlas_content::modrinth::{self, Installed, Modrinth, ProjectType, SearchResult, Target, UpdateInfo};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use atlas_versions::LoaderKind;
use std::path::PathBuf;

fn needs_loader(inst: &Installation) -> AtlasError {
    AtlasError::new(ErrorCode::InvalidArgument, format!("{} is a vanilla installation, so mods won't load in it.", inst.name))
        .suggest("Switch it to Fabric (Installations → Edit), or let Atlas do it when asked.")
}

impl Atlas {
    fn inst_or_selected(&self, id: Option<&str>) -> AtlasResult<Installation> {
        match id {
            Some(i) => self.installation(i),
            None => self.selected_installation().ok_or_else(|| AtlasError::new(ErrorCode::InvalidInstallation, "There's no installation yet.").suggest("Create one first.")),
        }
    }

    fn target_parts(&self, inst: &Installation) -> (PathBuf, &'static str, String) {
        (self.installations_store().game_dir(inst), inst.loader.modrinth(), inst.version.clone())
    }

    /// Search Modrinth for things that fit this installation's version and loader.
    pub fn search_content(&self, installation: Option<&str>, query: &str, kind: ProjectType, page: u32) -> AtlasResult<SearchResult> {
        let inst = self.inst_or_selected(installation)?;
        let loader = (inst.loader != LoaderKind::Vanilla).then(|| inst.loader.modrinth());
        let mr = Modrinth::new()?;
        self.rt.block_on(mr.search(query, kind, loader, Some(&inst.version), page * 20, 20))
    }

    /// Switch a vanilla installation to Fabric so mods can load.
    pub fn enable_mods(&self, installation: Option<&str>) -> AtlasResult<Installation> {
        let inst = self.inst_or_selected(installation)?;
        if inst.loader != LoaderKind::Vanilla {
            return Ok(inst);
        }
        self.update_installation(&inst.id, InstallationChanges { loader: Some(LoaderKind::Fabric), ..Default::default() })
    }

    /// Install a Modrinth project (plus what it needs) in the background.
    pub fn install_content(&self, installation: Option<&str>, project: &str, kind: ProjectType) -> AtlasResult<JobId> {
        let inst = self.inst_or_selected(installation)?;
        if kind == ProjectType::Mod && inst.loader == LoaderKind::Vanilla {
            return Err(needs_loader(&inst));
        }
        let (dir, loader, mc) = self.target_parts(&inst);
        let dl = modrinth::downloader(self.settings().downloads.max_concurrent as usize)?;
        let (job, progress, cancel) = self.register_job(JobKind::Content, project)?;
        let jobs = self.jobs.clone();
        let project = project.to_string();
        self.rt.spawn(async move {
            let r = async {
                let mr = Modrinth::new()?;
                let t = Target { game_dir: &dir, loader, game_version: &mc };
                mr.install(&dl, &project, kind, &t, progress, cancel).await.map(|_| ())
            }
            .await;
            Self::finish_job(&jobs, job, r);
        });
        Ok(job)
    }

    pub fn installed_content(&self, installation: Option<&str>) -> AtlasResult<Vec<Installed>> {
        let inst = self.inst_or_selected(installation)?;
        Ok(modrinth::installed(&self.installations_store().game_dir(&inst)))
    }

    pub fn uninstall_content(&self, installation: Option<&str>, project_id: &str, confirm: Confirmation) -> AtlasResult<()> {
        confirm.require("remove this mod")?;
        let inst = self.inst_or_selected(installation)?;
        modrinth::uninstall(&self.installations_store().game_dir(&inst), project_id)
    }

    pub fn content_updates(&self, installation: Option<&str>) -> AtlasResult<Vec<UpdateInfo>> {
        let inst = self.inst_or_selected(installation)?;
        let (dir, loader, mc) = self.target_parts(&inst);
        let mr = Modrinth::new()?;
        self.rt.block_on(mr.check_updates(&Target { game_dir: &dir, loader, game_version: &mc }))
    }

    pub fn apply_content_updates(&self, installation: Option<&str>) -> AtlasResult<JobId> {
        let inst = self.inst_or_selected(installation)?;
        let (dir, loader, mc) = self.target_parts(&inst);
        let dl = modrinth::downloader(self.settings().downloads.max_concurrent as usize)?;
        let (job, progress, cancel) = self.register_job(JobKind::Content, &format!("updates for {}", inst.name))?;
        let jobs = self.jobs.clone();
        self.rt.spawn(async move {
            let r = async {
                let mr = Modrinth::new()?;
                let t = Target { game_dir: &dir, loader, game_version: &mc };
                let ups = mr.check_updates(&t).await?;
                mr.apply_updates(&dl, &t, &ups, progress, cancel).await.map(|_| ())
            }
            .await;
            Self::finish_job(&jobs, job, r);
        });
        Ok(job)
    }

    /// Icon bytes for a Modrinth project (only from Modrinth's CDN).
    pub fn fetch_icon(&self, url: &str) -> AtlasResult<Vec<u8>> {
        if !url.starts_with("https://cdn.modrinth.com/") {
            return Err(AtlasError::new(ErrorCode::InvalidArgument, "Icons only come from Modrinth's CDN."));
        }
        let dl = modrinth::downloader(4)?;
        self.rt.block_on(dl.fetch_bytes(url, &atlas_downloads::CancellationToken::new()))
    }

    pub fn bundles(&self) -> &'static [Bundle] {
        bundles::BUNDLES
    }

    pub fn all_bundles() -> &'static [Bundle] {
        bundles::BUNDLES
    }

    /// Add bundles to an installation (switching it to Fabric if it's vanilla).
    pub fn install_bundles(&self, installation: Option<&str>, ids: &[String]) -> AtlasResult<JobId> {
        let mut inst = self.inst_or_selected(installation)?;
        if inst.loader == LoaderKind::Vanilla {
            inst = self.enable_mods(Some(&inst.id))?;
        } else if inst.loader != LoaderKind::Fabric && inst.loader != LoaderKind::Quilt {
            return Err(AtlasError::new(ErrorCode::InvalidArgument, format!("Bundles are Fabric mods, and {} uses {}.", inst.name, inst.loader.label()))
                .suggest("Use them on a Fabric or Quilt installation."));
        }
        for id in ids {
            if bundles::get(id).is_none() {
                return Err(AtlasError::new(ErrorCode::InvalidArgument, format!("No bundle called '{id}'.")));
            }
        }
        let (dir, loader, mc) = self.target_parts(&inst);
        let dl = modrinth::downloader(self.settings().downloads.max_concurrent as usize)?;
        let (job, progress, cancel) = self.register_job(JobKind::Content, &format!("bundles for {}", inst.name))?;
        let jobs = self.jobs.clone();
        let ids = ids.to_vec();
        self.rt.spawn(async move {
            let r = async {
                let mr = Modrinth::new()?;
                let t = Target { game_dir: &dir, loader, game_version: &mc };
                let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                bundles::install(&mr, &dl, &refs, &t, progress, cancel).await
            }
            .await;
            let note = r.as_ref().ok().filter(|rep| !rep.skipped.is_empty()).map(|rep| format!("Skipped (no build for this version yet): {}", rep.skipped.join(", ")));
            Self::finish_job_with_note(&jobs, job, r.map(|_| ()), note);
        });
        Ok(job)
    }

    /// The one-click setup: a new Fabric installation with the chosen bundles.
    pub fn create_client(&self, name: &str, version: &str, bundle_ids: &[String]) -> AtlasResult<(Installation, Option<JobId>)> {
        let inst = self.create_modded_installation(name, version, LoaderKind::Fabric, None)?;
        self.select_installation(&inst.id)?;
        let job = if bundle_ids.is_empty() { None } else { Some(self.install_bundles(Some(&inst.id), bundle_ids)?) };
        Ok((inst, job))
    }
}
