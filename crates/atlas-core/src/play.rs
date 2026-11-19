//! Installations and playing. The UI only ever reads [`crate::GameState`]
//! from here; it never decides on its own whether the game is running.

use crate::{Atlas, Confirmation, GameState};
use atlas_downloads::{CancellationToken, Progress};
use atlas_instances::{Changes, Installation, Installations};
use atlas_launch::{build_command, GameExit, GameProcess, LaunchAuth, LaunchSpec};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use atlas_versions::rules::Platform;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(crate) enum Phase {
    #[default]
    Idle,
    Installing(Arc<Progress>),
    Preparing(Arc<Progress>),
    Launching,
    Running(u32),
    Stopping,
    Exited(GameExit),
    Failed(AtlasError),
}

#[derive(Default)]
pub(crate) struct Session {
    pub installation: Option<String>,
    pub phase: Phase,
    pub stop: Option<tokio::sync::oneshot::Sender<()>>,
}

pub(crate) type SessionRef = Arc<Mutex<Session>>;

const PID_FILE: &str = ".running.pid";

#[derive(Debug, Clone, Default)]
pub struct PlayOptions {
    /// "host" or "host:port" to join on start.
    pub server: Option<String>,
}

impl Atlas {
    pub(crate) fn installations_store(&self) -> Installations {
        Installations::new(self.paths().installations_dir())
    }

    fn session(&self) -> std::sync::MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn installations(&self) -> AtlasResult<Vec<Installation>> {
        self.installations_store().list()
    }

    pub fn installation(&self, id: &str) -> AtlasResult<Installation> {
        self.installations_store().load(id)
    }

    pub fn create_installation(&self, name: &str, version: &str, game_dir: Option<PathBuf>) -> AtlasResult<Installation> {
        self.create_modded_installation(name, version, atlas_versions::LoaderKind::Vanilla, game_dir)
    }

    pub fn create_modded_installation(&self, name: &str, version: &str, loader: atlas_versions::LoaderKind, game_dir: Option<PathBuf>) -> AtlasResult<Installation> {
        let version = self.resolve_version(version)?;
        let mut inst = self.installations_store().create(name, &version, game_dir)?;
        if loader != atlas_versions::LoaderKind::Vanilla {
            inst = self.installations_store().update(&inst.id, Changes { loader: Some(loader), ..Default::default() })?;
        }
        if self.settings().installations.default_installation.is_none() {
            self.select_installation(&inst.id)?;
        }
        Ok(inst)
    }

    pub fn update_installation(&self, id: &str, changes: Changes) -> AtlasResult<Installation> {
        let changes = match changes.version {
            Some(v) => Changes { version: Some(self.resolve_version(&v)?), ..changes },
            None => changes,
        };
        self.installations_store().update(id, changes)
    }

    pub fn duplicate_installation(&self, id: &str) -> AtlasResult<Installation> {
        self.installations_store().duplicate(id)
    }

    /// Removes the installation folder (worlds inside a managed folder go
    /// with it, which is why this needs confirmation). A custom game folder
    /// is never deleted; its path is returned so the UI can say so.
    pub fn delete_installation(&self, id: &str, confirm: Confirmation) -> AtlasResult<Option<PathBuf>> {
        confirm.require("delete this installation")?;
        if self.running_installation().as_deref() == Some(id) {
            return Err(AtlasError::new(ErrorCode::InvalidArgument, "That installation is running.").suggest("Stop the game first."));
        }
        let left = self.installations_store().delete(id)?;
        if self.settings().installations.default_installation.as_deref() == Some(id) {
            let mut s = self.settings();
            s.installations.default_installation = self.installations()?.first().map(|i| i.id.clone());
            self.save_settings(s)?;
        }
        Ok(left)
    }

    pub fn select_installation(&self, id: &str) -> AtlasResult<()> {
        self.installation(id)?;
        let mut s = self.settings();
        s.installations.default_installation = Some(id.to_string());
        self.save_settings(s).map(|_| ())
    }

    /// The selected installation, falling back to the most recently played.
    pub fn selected_installation(&self) -> Option<Installation> {
        let list = self.installations().ok()?;
        let want = self.settings().installations.default_installation;
        want.and_then(|id| list.iter().find(|i| i.id == id).cloned()).or_else(|| list.into_iter().next())
    }

    fn running_installation(&self) -> Option<String> {
        let s = self.session();
        match s.phase {
            Phase::Installing(_) | Phase::Preparing(_) | Phase::Launching | Phase::Running(_) | Phase::Stopping => s.installation.clone(),
            _ => None,
        }
    }

    fn pid_file(&self, id: &str) -> PathBuf {
        self.paths().installations_dir().join(id).join(PID_FILE)
    }

    /// pid of a game started by another Atlas process (e.g. the CLI), if alive.
    fn external_pid(&self, id: &str) -> Option<u32> {
        let file = self.pid_file(id);
        let pid: u32 = std::fs::read_to_string(&file).ok()?.trim().parse().ok()?;
        if atlas_launch::process::is_game_process(pid) {
            Some(pid)
        } else {
            // leftover from a game that's gone; clean it up
            let _ = std::fs::remove_file(&file);
            None
        }
    }

    /// What the Play button should show for the selected installation.
    pub fn game_state(&self) -> GameState {
        {
            let s = self.session();
            let sel = self.settings().installations.default_installation;
            let ours = s.installation.is_some() && (sel.is_none() || s.installation == sel);
            if ours {
                match &s.phase {
                    Phase::Installing(p) => return GameState::Installing { progress: p.snapshot().fraction() },
                    Phase::Preparing(p) => return GameState::Preparing { progress: p.snapshot().fraction() },
                    Phase::Launching => return GameState::Launching,
                    Phase::Running(pid) => return GameState::Running { pid: *pid },
                    Phase::Stopping => return GameState::Stopping,
                    Phase::Failed(e) => return GameState::Error { error: e.clone() },
                    Phase::Exited(x) if x.crashed => {
                        return GameState::Error { error: crash_error(x) };
                    }
                    _ => {}
                }
            }
        }
        let Some(inst) = self.selected_installation() else { return GameState::NoInstallation };
        if let Some(pid) = self.external_pid(&inst.id) {
            return GameState::Running { pid };
        }
        if let Some(j) = self.jobs().into_iter().find(|j| j.version == inst.version && j.state == crate::JobState::Running) {
            return GameState::Installing { progress: j.progress.fraction() };
        }
        let installed = self.installed_versions().map(|v| v.iter().any(|x| x.id == inst.version)).unwrap_or(false);
        if installed { GameState::Ready } else { GameState::NotInstalled }
    }

    /// Clear a crash/error so the button goes back to Play.
    pub fn dismiss_game_error(&self) {
        let mut s = self.session();
        if matches!(s.phase, Phase::Failed(_) | Phase::Exited(_)) {
            s.phase = Phase::Idle;
        }
    }

    /// Start the selected (or given) installation in the background. Installs
    /// the version and Java first if they're missing.
    pub fn play(&self, installation_id: Option<&str>, opts: PlayOptions) -> AtlasResult<()> {
        let inst = match installation_id {
            Some(id) => {
                let i = self.installation(id)?;
                // playing something makes it the selected one, so Home follows along
                if self.settings().installations.default_installation.as_deref() != Some(id) {
                    self.select_installation(id)?;
                }
                i
            }
            None => self.selected_installation().ok_or_else(|| {
                AtlasError::new(ErrorCode::InvalidInstallation, "There's no installation to play.").suggest("Create one on the Installations page.")
            })?,
        };
        if self.running_installation().is_some() {
            return Err(AtlasError::new(ErrorCode::InvalidArgument, "A game is already starting or running.").suggest("Stop it first."));
        }
        if let Some(pid) = self.external_pid(&inst.id) {
            return Err(AtlasError::new(ErrorCode::InvalidArgument, format!("This installation is already running (pid {pid}).")));
        }
        let auth_fut = self.launch_auth_future();
        let settings = self.settings();
        let paths = self.paths();
        let store = self.installations_store();
        let game_dir = store.game_dir(&inst);
        let natives_dir = paths.installations_dir().join(&inst.id).join(".natives");
        let platform = Platform::current();
        let svc = self.version_service_pub();
        let dl = self.downloader_pub()?;
        let java_override = inst.java_path.clone().or(settings.game.java_path.clone());
        let resolution = match (inst.width, inst.height) {
            (Some(w), Some(h)) => Some((w, h)),
            _ if settings.game.width != 1280 || settings.game.height != 720 => Some((settings.game.width, settings.game.height)),
            _ => None,
        };
        let mut extra_jvm = settings.game.extra_jvm_args.clone();
        extra_jvm.extend(inst.jvm_args.clone());
        let (loader, loader_version) = (inst.loader, inst.loader_version.clone());
        let spec_base = LaunchSpec {
            data_root: paths.data_root.clone(),
            version_id: inst.version.clone(),
            game_dir,
            natives_dir: natives_dir.clone(),
            java: PathBuf::new(),
            min_mb: inst.memory_min_mb.unwrap_or(settings.memory.min_mb),
            max_mb: inst.memory_max_mb.unwrap_or(settings.memory.max_mb),
            extra_jvm,
            extra_game: inst.game_args.clone(),
            resolution,
            fullscreen: inst.fullscreen.unwrap_or(settings.game.fullscreen),
            server: opts.server,
            auth: LaunchAuth::demo(),
            platform,
        };
        let pid_file = self.pid_file(&inst.id);
        let session = self.session.clone();
        let runtimes = paths.runtimes_dir();
        let inst_id = inst.id.clone();
        {
            let mut s = self.session();
            s.installation = Some(inst.id.clone());
            s.phase = Phase::Launching;
        }
        let _ = store.touch_played(&inst.id);
        tracing::info!(installation = %inst.id, version = %inst.version, "play requested");

        self.rt.spawn(async move {
            let set = |p: Phase| session.lock().unwrap_or_else(|e| e.into_inner()).phase = p;
            let result: AtlasResult<()> = async {
                // 1. the version itself
                let installed = svc.installed()?.iter().any(|v| v.id == spec_base.version_id);
                if !installed {
                    let p = Arc::new(Progress::new());
                    set(Phase::Installing(p.clone()));
                    let (m, _) = svc.manifest(&dl, false, &CancellationToken::new()).await?;
                    svc.install(&dl, &m, &spec_base.version_id, p, CancellationToken::new()).await?;
                }
                // 1b. the mod loader, if any (its profile builds on the vanilla files)
                let mut spec_base = spec_base;
                if loader != atlas_versions::LoaderKind::Vanilla {
                    let p = Arc::new(Progress::new());
                    set(Phase::Preparing(p.clone()));
                    let lv = svc.pick_loader_version(&dl, loader, &spec_base.version_id, loader_version.as_deref(), &CancellationToken::new()).await?;
                    spec_base.version_id = svc.install_loader(&dl, loader, &spec_base.version_id, &lv, p, CancellationToken::new()).await?;
                }
                // 2. java
                let java = match java_override {
                    Some(j) => j,
                    None => {
                        let p = Arc::new(Progress::new());
                        set(Phase::Preparing(p.clone()));
                        let component = java_component(&spec_base.data_root, &spec_base.version_id).unwrap_or_else(|| "jre-legacy".into());
                        atlas_versions::java::ensure_runtime(&dl, &runtimes, &component, platform, atlas_versions::java::RUNTIME_INDEX_URL, p, CancellationToken::new()).await?
                    }
                };
                set(Phase::Launching);
                let auth = auth_fut.await?;
                let spec = LaunchSpec { java, auth, ..spec_base };
                let cmd = build_command(&spec)?;
                let _ = std::fs::remove_dir_all(&spec.natives_dir);
                if !cmd.native_jars.is_empty() {
                    atlas_launch::natives::extract(&cmd.native_jars, &spec.natives_dir)?;
                } else {
                    std::fs::create_dir_all(&spec.natives_dir).ok();
                }
                let proc = GameProcess::spawn(&cmd)?;
                let pid = proc.pid;
                let _ = std::fs::write(&pid_file, pid.to_string());
                let (tx, rx) = tokio::sync::oneshot::channel();
                {
                    let mut s = session.lock().unwrap_or_else(|e| e.into_inner());
                    s.phase = Phase::Running(pid);
                    s.stop = Some(tx);
                }
                let exit = proc.wait(rx).await;
                let _ = std::fs::remove_file(&pid_file);
                set(Phase::Exited(exit));
                Ok(())
            }
            .await;
            if let Err(e) = result {
                tracing::error!(installation = %inst_id, code = %e.code, "launch failed: {} {}", e.message, e.details);
                set(Phase::Failed(e));
            }
        });
        Ok(())
    }

    /// Stop the running game (ours, or one started by another Atlas process).
    pub fn stop(&self, installation_id: Option<&str>) -> AtlasResult<()> {
        {
            let mut s = self.session();
            if let Some(tx) = s.stop.take() {
                s.phase = Phase::Stopping;
                let _ = tx.send(());
                return Ok(());
            }
        }
        let id = match installation_id {
            Some(i) => i.to_string(),
            None => self.selected_installation().map(|i| i.id).ok_or_else(|| AtlasError::new(ErrorCode::InvalidArgument, "Nothing is running."))?,
        };
        match self.external_pid(&id) {
            Some(pid) => {
                atlas_launch::process::terminate(pid);
                Ok(())
            }
            None => Err(AtlasError::new(ErrorCode::InvalidArgument, "Nothing is running.")),
        }
    }

    /// Block until the current game session ends (used by `atlas play --wait`).
    pub fn wait_for_game(&self) -> GameState {
        loop {
            let st = self.game_state();
            let done = {
                let s = self.session();
                matches!(s.phase, Phase::Exited(_) | Phase::Failed(_) | Phase::Idle)
            };
            if done {
                return st;
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }

    pub fn last_exit(&self) -> Option<GameExit> {
        match &self.session().phase {
            Phase::Exited(x) => Some(x.clone()),
            _ => None,
        }
    }
}

/// Read `javaVersion.component` from the installed version file. Older
/// versions don't have it and run fine on Mojang's legacy Java 8.
fn java_component(root: &std::path::Path, id: &str) -> Option<String> {
    // loader profiles don't say; their vanilla parent does
    let (v, _) = atlas_launch::command::resolve_version(root, id).ok()?;
    v.get("javaVersion")?.get("component")?.as_str().map(str::to_owned)
}

fn crash_error(x: &GameExit) -> AtlasError {
    let mut details = String::new();
    if let Some(r) = &x.crash_report {
        details.push_str(&format!("Crash report: {}\n", r.display()));
    }
    details.push_str(&x.tail.iter().rev().take(15).rev().cloned().collect::<Vec<_>>().join("\n"));
    AtlasError::new(ErrorCode::GameCrashed, format!("The game crashed (exit code {}).", x.code.map(|c| c.to_string()).unwrap_or("unknown".into())))
        .details(details)
        .suggest("Open Logs and filter by Crash for the full report. Removing recently added mods or lowering memory often helps.")
}
