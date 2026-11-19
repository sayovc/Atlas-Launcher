//! Servers, mods, packs, worlds and modpack imports, exposed to the UI/CLI.

use crate::{Atlas, Confirmation, JobId, JobKind, PlayOptions};
use atlas_content::mods::ModInfo;
use atlas_content::packs::{PackInfo, PackKind};
use atlas_content::ping::PingResult;
use atlas_content::servers::{Server, Servers};
use atlas_content::worlds::WorldInfo;
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use std::path::{Path, PathBuf};

impl Atlas {
    fn servers_store(&self) -> Servers {
        Servers::new(&self.paths().data_root)
    }

    fn game_dir_of(&self, installation: Option<&str>) -> AtlasResult<(String, PathBuf)> {
        let inst = match installation {
            Some(id) => self.installation(id)?,
            None => self.selected_installation().ok_or_else(|| AtlasError::new(ErrorCode::InvalidInstallation, "There's no installation yet.").suggest("Create one first."))?,
        };
        Ok((inst.id.clone(), self.installations_store().game_dir(&inst)))
    }

    // ---------------------------------------------------------------- servers

    pub fn servers(&self) -> AtlasResult<Vec<Server>> {
        self.servers_store().list()
    }

    pub fn add_server(&self, name: &str, address: &str) -> AtlasResult<Server> {
        self.servers_store().add(name, address)
    }

    pub fn edit_server(&self, id: &str, name: Option<&str>, address: Option<&str>) -> AtlasResult<Server> {
        self.servers_store().edit(id, name, address)
    }

    pub fn set_server_favorite(&self, id: &str, favorite: bool) -> AtlasResult<Server> {
        self.servers_store().update(id, |s| {
            s.favorite = favorite;
            Ok(())
        })
    }

    pub fn remove_server(&self, id: &str, confirm: Confirmation) -> AtlasResult<()> {
        confirm.require("remove this server")?;
        self.servers_store().remove(id)
    }

    /// Ask the server for its status and remember its version and icon.
    pub fn ping_server(&self, id: &str) -> AtlasResult<PingResult> {
        let s = self.servers_store().get(id)?;
        let r = self.rt.block_on(atlas_content::ping::ping(&s.address, s.port))?;
        let (v, icon) = (r.version.clone(), r.icon.clone());
        self.servers_store().update(id, move |s| {
            s.version = Some(v);
            if icon.is_some() {
                s.icon = icon;
            }
            Ok(())
        })?;
        Ok(r)
    }

    /// Ping any address without saving it.
    pub fn ping_address(&self, address: &str) -> AtlasResult<PingResult> {
        let (h, p) = atlas_content::servers::parse_address(address)?;
        self.rt.block_on(atlas_content::ping::ping(&h, p))
    }

    /// Launch the selected installation straight into this server.
    pub fn connect_server(&self, id: &str) -> AtlasResult<()> {
        let s = self.servers_store().get(id)?;
        self.play(None, PlayOptions { server: Some(s.host_port()) })?;
        self.servers_store().update(id, |s| {
            s.last_played = Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
            Ok(())
        })?;
        Ok(())
    }

    // ------------------------------------------------------------------- mods

    pub fn mods(&self, installation: Option<&str>) -> AtlasResult<Vec<ModInfo>> {
        atlas_content::mods::list(&self.game_dir_of(installation)?.1)
    }

    pub fn set_mod_enabled(&self, installation: Option<&str>, file: &str, enabled: bool) -> AtlasResult<String> {
        atlas_content::mods::set_enabled(&self.game_dir_of(installation)?.1, file, enabled)
    }

    pub fn add_mod(&self, installation: Option<&str>, source: &Path) -> AtlasResult<String> {
        atlas_content::mods::add(&self.game_dir_of(installation)?.1, source)
    }

    pub fn remove_mod(&self, installation: Option<&str>, file: &str, confirm: Confirmation) -> AtlasResult<()> {
        confirm.require("delete this mod")?;
        atlas_content::mods::remove(&self.game_dir_of(installation)?.1, file)
    }

    // ------------------------------------------------------------------ packs

    pub fn packs(&self, installation: Option<&str>, kind: PackKind) -> AtlasResult<Vec<PackInfo>> {
        atlas_content::packs::list(&self.game_dir_of(installation)?.1, kind)
    }

    pub fn add_pack(&self, installation: Option<&str>, kind: PackKind, source: &Path) -> AtlasResult<String> {
        atlas_content::packs::add(&self.game_dir_of(installation)?.1, kind, source)
    }

    pub fn remove_pack(&self, installation: Option<&str>, kind: PackKind, file: &str, confirm: Confirmation) -> AtlasResult<()> {
        confirm.require("delete this pack")?;
        atlas_content::packs::remove(&self.game_dir_of(installation)?.1, kind, file)
    }

    // ----------------------------------------------------------------- worlds

    fn backups_dir(&self, inst_id: &str) -> PathBuf {
        self.paths().data_root.join("backups").join(inst_id)
    }

    pub fn worlds(&self, installation: Option<&str>) -> AtlasResult<Vec<WorldInfo>> {
        atlas_content::worlds::list(&self.game_dir_of(installation)?.1)
    }

    pub fn backup_world(&self, installation: Option<&str>, folder: &str) -> AtlasResult<PathBuf> {
        let (id, dir) = self.game_dir_of(installation)?;
        atlas_content::worlds::backup(&dir, folder, &self.backups_dir(&id))
    }

    /// Always writes a backup first; returns where it went.
    pub fn delete_world(&self, installation: Option<&str>, folder: &str, confirm: Confirmation) -> AtlasResult<PathBuf> {
        confirm.require("delete this world")?;
        let (id, dir) = self.game_dir_of(installation)?;
        atlas_content::worlds::delete(&dir, folder, &self.backups_dir(&id))
    }

    pub fn world_backups_dir(&self, installation: Option<&str>) -> AtlasResult<PathBuf> {
        Ok(self.backups_dir(&self.game_dir_of(installation)?.0))
    }

    // --------------------------------------------------------------- modpacks

    pub fn inspect_modpack(&self, pack: &Path) -> AtlasResult<atlas_content::mrpack::PackSummary> {
        atlas_content::mrpack::inspect(pack)
    }

    /// Create an installation for the pack and fill it in the background.
    pub fn import_modpack(&self, pack: &Path) -> AtlasResult<(crate::Installation, JobId)> {
        let summary = atlas_content::mrpack::inspect(pack)?;
        let mut inst = self.installations_store().create(&summary.name, &summary.minecraft, None)?;
        // packs name their loader in dependencies: fabric-loader, quilt-loader, neoforge, forge
        let loader = summary.loaders.iter().find_map(|(k, v)| {
            let kind = match k.as_str() {
                "fabric-loader" => atlas_versions::LoaderKind::Fabric,
                "quilt-loader" => atlas_versions::LoaderKind::Quilt,
                "neoforge" => atlas_versions::LoaderKind::NeoForge,
                "forge" => atlas_versions::LoaderKind::Forge,
                _ => return None,
            };
            Some((kind, v.clone()))
        });
        if let Some((kind, ver)) = loader {
            inst = self.installations_store().update(&inst.id, atlas_instances::Changes { loader: Some(kind), loader_version: Some(Some(ver)), ..Default::default() })?;
        }
        let game_dir = self.installations_store().game_dir(&inst);
        let dl = self.downloader_pub()?;
        let (job, progress, cancel) = self.register_job(JobKind::Modpack, &summary.name)?;
        let jobs = self.jobs.clone();
        let pack = pack.to_path_buf();
        self.rt.spawn(async move {
            let r = atlas_content::mrpack::install(&dl, &pack, &game_dir, progress, cancel).await.map(|_| ());
            Self::finish_job(&jobs, job, r);
        });
        Ok((inst, job))
    }
}
