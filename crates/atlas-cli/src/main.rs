//! `atlas` — command-line front-end for Atlas Launcher.
//!
//! Every command is a thin call into `atlas_core::Atlas`; no business logic
//! lives here. With `--json`, output is a single JSON object:
//! `{"ok":true,"data":...}` or `{"ok":false,"error":{code,message,details,suggestion}}`.

use atlas_core::{Atlas, Confirmation};
use atlas_logging::{LogFilter, LogLevel, LogSource};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "atlas", version, about = "Atlas Launcher command-line interface")]
struct Cli {
    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,
    /// Also print launcher logs to stderr.
    #[arg(long, short, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the real backend state (account, installation, game state).
    Status,
    /// Show the folders Atlas uses.
    Paths,
    /// View or change settings.
    #[command(subcommand)]
    Settings(SettingsCmd),
    /// Manage the Atlas data folder.
    #[command(subcommand, name = "data-dir")]
    DataDir(DataDirCmd),
    /// List Minecraft versions (from Mojang's official list).
    Versions {
        /// Fetch a fresh list from the network.
        #[arg(long)]
        refresh: bool,
        /// Only installed versions.
        #[arg(long)]
        installed: bool,
        /// Include snapshots.
        #[arg(long)]
        snapshots: bool,
        /// Include everything (snapshots, betas, alphas).
        #[arg(long)]
        all: bool,
        /// Maximum rows (human output).
        #[arg(long, short = 'n', default_value_t = 30)]
        limit: usize,
    },
    /// Install a version: an id, `latest` or `latest-snapshot`.
    Install { version: String },
    /// Check every file of an installed version against its checksum.
    Verify { version: String },
    /// Re-download missing or corrupt files of an installed version.
    Repair { version: String },
    /// Remove an installed version (destructive). Shared libraries and assets are kept.
    Remove {
        version: String,
        #[arg(long)]
        yes: bool,
    },
    /// Find existing Minecraft folders Atlas can reuse verified files from (read-only).
    Detect,
    /// List and manage installations.
    Instances {
        #[command(subcommand)]
        cmd: Option<InstCmd>,
    },
    /// Microsoft accounts: list, login, logout, use.
    Account {
        #[command(subcommand)]
        cmd: Option<AccountCmd>,
    },
    /// Start the game. Installs the version and Java first if needed.
    Play {
        /// Installation id (defaults to the selected one).
        id: Option<String>,
        /// Join a server right away, e.g. play.example.net:25565
        #[arg(long)]
        server: Option<String>,
        /// Return as soon as the game has started instead of waiting for it to exit.
        #[arg(long)]
        detach: bool,
    },
    /// Stop a running game.
    Stop { id: Option<String> },
    /// Your server list.
    Servers {
        #[command(subcommand)]
        cmd: Option<ServerCmd>,
    },
    /// Mods in an installation (the selected one unless --instance is given).
    Mods {
        #[arg(long, global = true)]
        instance: Option<String>,
        #[command(subcommand)]
        cmd: Option<ModCmd>,
    },
    /// Resource packs and shader packs.
    Packs {
        #[arg(long, global = true)]
        instance: Option<String>,
        /// resource or shader
        #[arg(long, global = true, default_value = "resource")]
        kind: String,
        #[command(subcommand)]
        cmd: Option<PackCmd>,
    },
    /// Worlds in an installation.
    Worlds {
        #[arg(long, global = true)]
        instance: Option<String>,
        #[command(subcommand)]
        cmd: Option<WorldCmd>,
    },
    /// Import a Modrinth modpack (.mrpack) as a new installation.
    Import { pack: PathBuf },
    /// Check for and install Atlas updates.
    Update {
        #[command(subcommand)]
        cmd: Option<UpdateCmd>,
    },
    /// Local MCP server for AI assistants (off unless enabled in settings).
    Mcp {
        #[command(subcommand)]
        cmd: Option<McpCmd>,
    },
    /// Show launcher logs.
    Logs {
        /// Minimum level: trace, debug, info, warn, error.
        #[arg(long, default_value = "info")]
        level: String,
        /// Case-insensitive text search.
        #[arg(long, short)]
        search: Option<String>,
        /// Only this source: launcher, game, crash, install, download.
        #[arg(long)]
        source: Option<String>,
        /// Maximum number of most recent records.
        #[arg(long, short = 'n', default_value_t = 200)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum UpdateCmd {
    Check,
    /// Download, verify and install the newest version.
    Install,
    /// Go back to the version before the last update.
    Rollback,
}

#[derive(Subcommand)]
enum McpCmd {
    /// Serve MCP over stdin/stdout (what your assistant runs). Needs ATLAS_MCP_TOKEN.
    Serve,
    /// Show the token to put in your assistant's config.
    Token,
    /// List requests waiting for your approval.
    Pending,
    /// Allow one destructive action an assistant asked for.
    Approve { code: String },
}

#[derive(Subcommand)]
enum ServerCmd {
    List,
    Add { name: String, address: String },
    Edit {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        address: Option<String>,
    },
    Remove {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    Favorite { id: String, #[arg(long)] off: bool },
    /// Show status, players and version (works with a saved id or any address).
    Ping { target: String },
    /// Launch the selected installation and join this server.
    Connect { id: String },
}

#[derive(Subcommand)]
enum ModCmd {
    List,
    Enable { file: String },
    Disable { file: String },
    Add { path: PathBuf },
    Remove {
        file: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum PackCmd {
    List,
    Add { path: PathBuf },
    Remove {
        file: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum WorldCmd {
    List,
    Backup { folder: String },
    /// Delete a world. A backup zip is always written first.
    Delete {
        folder: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum AccountCmd {
    List,
    /// Sign in with Microsoft (opens your browser).
    Login {
        /// Use a code you type in on another device instead of a browser redirect.
        #[arg(long)]
        device: bool,
    },
    /// Sign out and delete this account's saved tokens.
    Logout { id: String },
    /// Play with this account from now on.
    Use { id: String },
}

#[derive(Subcommand)]
enum InstCmd {
    /// List installations.
    List,
    /// Create one, e.g. `atlas instances create Survival --version latest`.
    Create {
        name: String,
        #[arg(long, default_value = "latest")]
        version: String,
        /// Use an existing folder as the game folder (it will never be deleted by Atlas).
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    Rename { id: String, name: String },
    /// Change settings of one installation.
    Set {
        id: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        memory_min: Option<u32>,
        #[arg(long)]
        memory_max: Option<u32>,
        /// Extra JVM argument (repeatable). Replaces the current list.
        #[arg(long = "jvm-arg")]
        jvm_args: Vec<String>,
        #[arg(long)]
        java: Option<PathBuf>,
    },
    Duplicate { id: String },
    /// Delete an installation (its managed game folder and worlds too).
    Delete {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Make this the installation Play uses.
    Select { id: String },
}

#[derive(Subcommand)]
enum SettingsCmd {
    /// Print all settings.
    Show,
    /// Print one setting by dotted key, e.g. `memory.max_mb`.
    Get { key: String },
    /// Set one setting, e.g. `atlas settings set memory.max_mb 6144`. Use `none` to clear optional values.
    Set { key: String, value: String },
    /// Reset all settings to defaults (destructive).
    Reset {
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum DataDirCmd {
    /// Print the current data folder.
    Show,
    /// Move all Atlas data to a new, empty folder (destructive).
    Move {
        target: PathBuf,
        #[arg(long)]
        yes: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    match run(cli) {
        Ok(out) => {
            if json {
                println!("{}", json!({ "ok": true, "data": out.data }));
            } else if !out.human.is_empty() {
                println!("{}", out.human.trim_end());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            if json {
                println!("{}", json!({ "ok": false, "error": e }));
            } else {
                eprintln!("error[{}]: {}", e.code, e.message);
                if !e.details.is_empty() {
                    eprintln!("  details: {}", e.details);
                }
                if !e.suggestion.is_empty() {
                    eprintln!("  suggestion: {}", e.suggestion);
                }
            }
            match e.code {
                ErrorCode::ConfirmationRequired => ExitCode::from(3),
                ErrorCode::InvalidArgument => ExitCode::from(2),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

struct Output {
    data: Value,
    human: String,
}

fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn run(cli: Cli) -> AtlasResult<Output> {
    let mut atlas = Atlas::open_default()?;
    let _log = atlas.init_logging(cli.verbose)?;
    atlas_core::crash::install_panic_hook(atlas.paths().launcher_logs_dir());
    tracing::debug!("cli invoked");

    Ok(match cli.command {
        Command::Status => {
            let s = atlas.status();
            let mut human = format!(
                "Atlas {}\nData folder:    {}\nAccount:        {}\nInstallation:   {}\nMinecraft:      {}\nGame state:     {:?}\nNot set up:     {}",
                s.atlas_version,
                s.data_root.display(),
                s.account.as_deref().unwrap_or("not signed in"),
                s.installation.as_deref().unwrap_or("none"),
                s.minecraft_version.as_deref().unwrap_or("-"),
                s.game_state,
                s.unavailable.join(", "),
            );
            for w in &s.warnings {
                human.push_str(&format!("\nwarning[{}]: {} {}", w.code, w.message, w.suggestion));
            }
            Output { data: to_value(&s), human }
        }
        Command::Paths => {
            let p = atlas.paths();
            let data = json!({
                "bootstrap_dir": p.bootstrap_dir,
                "data_root": p.data_root,
                "data_root_is_custom": p.data_root_is_custom,
                "settings_file": p.settings_file(),
                "logs_dir": p.launcher_logs_dir(),
                "installations_dir": p.installations_dir(),
                "versions_dir": p.versions_dir(),
                "downloads_cache_dir": p.downloads_cache_dir(),
            });
            let human = data.as_object().map(|o| {
                o.iter().map(|(k, v)| format!("{k:22} {}", v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()))).collect::<Vec<_>>().join("\n")
            }).unwrap_or_default();
            Output { data, human }
        }
        Command::Settings(cmd) => match cmd {
            SettingsCmd::Show => {
                let s = atlas.settings();
                Output { data: to_value(&s), human: s.to_toml()? }
            }
            SettingsCmd::Get { key } => {
                let v = atlas.get_setting(&key)?;
                Output { data: to_value(&v), human: v.to_string() }
            }
            SettingsCmd::Set { key, value } => {
                let s = atlas.set_setting(&key, &value)?;
                let v = s.get_key(&key).map(|v| v.to_string()).unwrap_or_else(|_| "(unset)".into());
                Output { data: json!({ "key": key, "value": to_value(&s.get_key(&key).ok()) }), human: format!("{key} = {v}") }
            }
            SettingsCmd::Reset { yes } => {
                atlas.reset_settings(Confirmation::from_bool(yes))?;
                Output { data: json!({ "reset": true }), human: "Settings reset to defaults.".into() }
            }
        },
        Command::DataDir(cmd) => match cmd {
            DataDirCmd::Show => {
                let p = atlas.paths();
                Output { data: json!({ "data_root": p.data_root, "custom": p.data_root_is_custom }), human: p.data_root.display().to_string() }
            }
            DataDirCmd::Move { target, yes } => {
                let target = if target.is_absolute() {
                    target
                } else {
                    std::env::current_dir().map(|c| c.join(&target)).unwrap_or(target)
                };
                let r = atlas.move_data_root(&target, Confirmation::from_bool(yes))?;
                let mut human = format!(
                    "Moved {} files ({} bytes) from {} to {}.",
                    r.files, r.bytes, r.old_root.display(), r.new_root.display()
                );
                if r.symlinks_skipped > 0 {
                    human.push_str(&format!("\n{} symbolic link(s) were not copied; their targets were left untouched.", r.symlinks_skipped));
                }
                for l in &r.leftovers {
                    human.push_str(&format!("\nCould not remove {} (safe to delete manually).", l.display()));
                }
                Output { data: to_value(&r), human }
            }
        },
        Command::Versions { refresh, installed, snapshots, all, limit } => {
            if installed {
                let list = atlas.installed_versions()?;
                let human = if list.is_empty() {
                    "No versions installed. Try: atlas install latest".to_string()
                } else {
                    list.iter().map(|v| format!("{:<22} {:<9} {:>5} files  {:>8}", v.id, v.kind.label(), v.files, human_bytes(v.bytes))).collect::<Vec<_>>().join("\n")
                };
                Output { data: to_value(&list), human }
            } else {
                let list = atlas.versions(refresh)?;
                let rows: Vec<_> = list.versions.iter().filter(|v| all || v.kind == atlas_core::VersionKind::Release || (snapshots && v.kind == atlas_core::VersionKind::Snapshot)).collect();
                let mut human = format!("Latest release: {}   Latest snapshot: {}{}\n", list.latest_release, list.latest_snapshot, if list.stale { "   (offline: cached list)" } else { "" });
                for v in rows.iter().take(limit) {
                    human.push_str(&format!("{:<22} {:<9} {}  {}\n", v.id, v.kind.label(), v.release_time.get(..10).unwrap_or(""), if v.installed { "installed" } else { "" }));
                }
                if rows.len() > limit {
                    human.push_str(&format!("… {} more (use -n)", rows.len() - limit));
                }
                Output { data: to_value(&list), human }
            }
        }
        Command::Install { version } => run_job(&atlas, atlas.start_install(&version)?, cli.json)?,
        Command::Repair { version } => run_job(&atlas, atlas.start_repair(&version)?, cli.json)?,
        Command::Verify { version } => {
            let r = atlas.verify_version(&version)?;
            let human = if r.ok() {
                format!("{}: all {} files verified.", r.id, r.checked)
            } else {
                format!("{}: {} missing, {} corrupt of {} files. Run: atlas repair {}", r.id, r.missing.len(), r.corrupt.len(), r.checked, r.id)
            };
            if !r.ok() {
                return Err(AtlasError::new(ErrorCode::InvalidInstallation, human).details(format!("missing: {:?}\ncorrupt: {:?}", r.missing, r.corrupt)).suggest(format!("Run `atlas repair {}`.", r.id)));
            }
            Output { data: to_value(&r), human }
        }
        Command::Remove { version, yes } => {
            atlas.remove_version(&version, Confirmation::from_bool(yes))?;
            Output { data: json!({ "removed": version }), human: format!("Removed {version}. Shared libraries and assets were kept.") }
        }
        Command::Account { cmd } => match cmd.unwrap_or(AccountCmd::List) {
            AccountCmd::List => {
                let list = atlas.accounts()?;
                let active = atlas.active_account().map(|a| a.id);
                let human = if list.is_empty() {
                    if atlas.sign_in_available() { "No accounts. Try: atlas account login".into() } else { "No accounts. (Sign-in isn't set up in this build yet; you can still play the demo.)".into() }
                } else {
                    list.iter().map(|a| format!("{} {}  {}", if Some(&a.id) == active.as_ref() { "*" } else { " " }, a.name, a.id)).collect::<Vec<_>>().join("\n")
                };
                Output { data: to_value(&list), human }
            }
            AccountCmd::Login { device } => {
                if device {
                    let dc = atlas.start_device_login()?;
                    eprintln!("Go to {} and enter the code {}", dc.verification_uri, dc.user_code);
                } else {
                    let url = atlas.start_browser_login()?;
                    eprintln!("Your browser should open. If it doesn't, open this link:\n{url}");
                }
                loop {
                    match atlas.login_state() {
                        atlas_core::LoginState::Done { account } => break Output { human: format!("Signed in as {}.", account.name), data: to_value(&account) },
                        atlas_core::LoginState::Failed { error } => return Err(error),
                        atlas_core::LoginState::Idle => return Err(AtlasError::cancelled()),
                        _ => std::thread::sleep(std::time::Duration::from_millis(300)),
                    }
                }
            }
            AccountCmd::Logout { id } => {
                atlas.remove_account(&id)?;
                Output { data: json!({ "removed": id }), human: "Signed out.".into() }
            }
            AccountCmd::Use { id } => {
                atlas.set_active_account(&id)?;
                Output { data: json!({ "active": id }), human: "Done.".into() }
            }
        },
        Command::Instances { cmd } => match cmd.unwrap_or(InstCmd::List) {
            InstCmd::List => {
                let list = atlas.installations()?;
                let sel = atlas.selected_installation().map(|i| i.id);
                let human = if list.is_empty() {
                    "No installations yet. Try: atlas instances create Survival".into()
                } else {
                    list.iter().map(|i| format!("{} {:<14} {:<24} {}", if Some(&i.id) == sel.as_ref() { "*" } else { " " }, i.id, i.name, i.version)).collect::<Vec<_>>().join("\n")
                };
                Output { data: to_value(&list), human }
            }
            InstCmd::Create { name, version, dir } => {
                let i = atlas.create_installation(&name, &version, dir)?;
                Output { human: format!("Created {} ({}) for Minecraft {}.", i.name, i.id, i.version), data: to_value(&i) }
            }
            InstCmd::Rename { id, name } => {
                let i = atlas.update_installation(&id, atlas_core::InstallationChanges { name: Some(name), ..Default::default() })?;
                Output { human: format!("Renamed to {}.", i.name), data: to_value(&i) }
            }
            InstCmd::Set { id, version, memory_min, memory_max, jvm_args, java } => {
                let ch = atlas_core::InstallationChanges {
                    version,
                    memory_min_mb: memory_min.map(Some),
                    memory_max_mb: memory_max.map(Some),
                    jvm_args: (!jvm_args.is_empty()).then_some(jvm_args),
                    java_path: java.map(Some),
                    ..Default::default()
                };
                let i = atlas.update_installation(&id, ch)?;
                Output { human: format!("Updated {}.", i.name), data: to_value(&i) }
            }
            InstCmd::Duplicate { id } => {
                let i = atlas.duplicate_installation(&id)?;
                Output { human: format!("Created {} ({}).", i.name, i.id), data: to_value(&i) }
            }
            InstCmd::Delete { id, yes } => {
                let kept = atlas.delete_installation(&id, Confirmation::from_bool(yes))?;
                let human = match &kept {
                    Some(d) => format!("Deleted. Your own game folder {} was left untouched.", d.display()),
                    None => "Deleted.".into(),
                };
                Output { data: json!({ "deleted": id, "kept_game_dir": kept }), human }
            }
            InstCmd::Select { id } => {
                atlas.select_installation(&id)?;
                Output { data: json!({ "selected": id }), human: "Selected.".into() }
            }
        },
        Command::Play { id, server, detach } => {
            atlas.play(id.as_deref(), atlas_core::PlayOptions { server })?;
            let mut last = String::new();
            loop {
                let st = atlas.game_state();
                let label = format!("{st:?}");
                if !cli.json && label != last && !matches!(st, atlas_core::GameState::Installing { .. } | atlas_core::GameState::Preparing { .. }) {
                    eprintln!("{}", st.action_label().trim_end_matches('…'));
                    last = label;
                }
                match st {
                    atlas_core::GameState::Running { pid } if detach => {
                        break Output { data: json!({ "running": true, "pid": pid }), human: format!("Game started (pid {pid}).") };
                    }
                    atlas_core::GameState::Error { error } => return Err(error),
                    _ => {}
                }
                if let Some(x) = atlas.last_exit() {
                    break Output { human: format!("Game exited (code {}).", x.code.map(|c| c.to_string()).unwrap_or("?".into())), data: to_value(&x) };
                }
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        }
        Command::Stop { id } => {
            atlas.stop(id.as_deref())?;
            Output { data: json!({ "stopping": true }), human: "Asked the game to quit.".into() }
        }
        Command::Servers { cmd } => match cmd.unwrap_or(ServerCmd::List) {
            ServerCmd::List => {
                let list = atlas.servers()?;
                let human = if list.is_empty() {
                    "No servers yet. Try: atlas servers add \"My server\" play.example.net".into()
                } else {
                    list.iter().map(|s| format!("{} {:<10} {:<24} {:<28} {}", if s.favorite { "★" } else { " " }, s.id, s.name, s.host_port(), s.version.as_deref().unwrap_or(""))).collect::<Vec<_>>().join("\n")
                };
                Output { data: to_value(&list), human }
            }
            ServerCmd::Add { name, address } => {
                let s = atlas.add_server(&name, &address)?;
                Output { human: format!("Added {} ({}).", s.name, s.id), data: to_value(&s) }
            }
            ServerCmd::Edit { id, name, address } => {
                let s = atlas.edit_server(&id, name.as_deref(), address.as_deref())?;
                Output { human: "Saved.".into(), data: to_value(&s) }
            }
            ServerCmd::Remove { id, yes } => {
                atlas.remove_server(&id, Confirmation::from_bool(yes))?;
                Output { human: "Removed.".into(), data: json!({ "removed": id }) }
            }
            ServerCmd::Favorite { id, off } => {
                let s = atlas.set_server_favorite(&id, !off)?;
                Output { human: if s.favorite { "Starred.".into() } else { "Unstarred.".into() }, data: to_value(&s) }
            }
            ServerCmd::Ping { target } => {
                let r = if atlas.servers()?.iter().any(|s| s.id == target) { atlas.ping_server(&target)? } else { atlas.ping_address(&target)? };
                Output { human: format!("{}  ·  {}/{} players  ·  {}  ·  {} ms", r.motd, r.online, r.max, r.version, r.latency_ms), data: to_value(&r) }
            }
            ServerCmd::Connect { id } => {
                atlas.connect_server(&id)?;
                let st = atlas.wait_for_game();
                if let atlas_core::GameState::Error { error } = st {
                    return Err(error);
                }
                Output { human: "Game closed.".into(), data: json!({ "done": true }) }
            }
        },
        Command::Mods { instance, cmd } => {
            let inst = instance.as_deref();
            match cmd.unwrap_or(ModCmd::List) {
                ModCmd::List => {
                    let list = atlas.mods(inst)?;
                    let human = if list.is_empty() {
                        "No mods in this installation.".into()
                    } else {
                        list.iter().map(|m| {
                            let mut line = format!("{} {:<28} {:<10} {:<9} {}", if m.enabled { "+" } else { "-" }, m.name, m.version.as_deref().unwrap_or(""), m.loader.label(), m.file);
                            if !m.missing.is_empty() {
                                line.push_str(&format!("  (needs: {})", m.missing.join(", ")));
                            }
                            line
                        }).collect::<Vec<_>>().join("\n") + "\n\nNote: Atlas doesn't install Fabric/Forge/NeoForge loaders yet, so these only load if you add a loader yourself."
                    };
                    Output { data: to_value(&list), human }
                }
                ModCmd::Enable { file } => {
                    let f = atlas.set_mod_enabled(inst, &file, true)?;
                    Output { human: format!("Enabled ({f})."), data: json!({ "file": f }) }
                }
                ModCmd::Disable { file } => {
                    let f = atlas.set_mod_enabled(inst, &file, false)?;
                    Output { human: format!("Disabled ({f})."), data: json!({ "file": f }) }
                }
                ModCmd::Add { path } => {
                    let f = atlas.add_mod(inst, &path)?;
                    Output { human: format!("Added {f}."), data: json!({ "file": f }) }
                }
                ModCmd::Remove { file, yes } => {
                    atlas.remove_mod(inst, &file, Confirmation::from_bool(yes))?;
                    Output { human: "Deleted.".into(), data: json!({ "removed": file }) }
                }
            }
        }
        Command::Packs { instance, kind, cmd } => {
            let inst = instance.as_deref();
            let kind = match kind.as_str() {
                "resource" => atlas_core::PackKind::Resource,
                "shader" => atlas_core::PackKind::Shader,
                other => return Err(AtlasError::new(ErrorCode::InvalidArgument, format!("Unknown pack kind '{other}'.")).suggest("Use resource or shader.")),
            };
            match cmd.unwrap_or(PackCmd::List) {
                PackCmd::List => {
                    let list = atlas.packs(inst, kind)?;
                    let human = if list.is_empty() { "No packs.".into() } else { list.iter().map(|p| format!("{:<36} {}", p.file, p.description.as_deref().unwrap_or(""))).collect::<Vec<_>>().join("\n") };
                    Output { data: to_value(&list), human }
                }
                PackCmd::Add { path } => {
                    let f = atlas.add_pack(inst, kind, &path)?;
                    Output { human: format!("Added {f}."), data: json!({ "file": f }) }
                }
                PackCmd::Remove { file, yes } => {
                    atlas.remove_pack(inst, kind, &file, Confirmation::from_bool(yes))?;
                    Output { human: "Deleted.".into(), data: json!({ "removed": file }) }
                }
            }
        }
        Command::Worlds { instance, cmd } => {
            let inst = instance.as_deref();
            match cmd.unwrap_or(WorldCmd::List) {
                WorldCmd::List => {
                    let list = atlas.worlds(inst)?;
                    let human = if list.is_empty() { "No worlds yet.".into() } else {
                        list.iter().map(|w| format!("{:<26} {:<10} {:<8} {:>8}  {}", w.name, w.game_mode.as_deref().unwrap_or(""), w.version.as_deref().unwrap_or(""), human_bytes(w.size), w.folder)).collect::<Vec<_>>().join("\n")
                    };
                    Output { data: to_value(&list), human }
                }
                WorldCmd::Backup { folder } => {
                    let p = atlas.backup_world(inst, &folder)?;
                    Output { human: format!("Backed up to {}", p.display()), data: json!({ "backup": p }) }
                }
                WorldCmd::Delete { folder, yes } => {
                    let p = atlas.delete_world(inst, &folder, Confirmation::from_bool(yes))?;
                    Output { human: format!("Deleted. A backup was saved to {}", p.display()), data: json!({ "backup": p }) }
                }
            }
        }
        Command::Import { pack } => {
            let summary = atlas.inspect_modpack(&pack)?;
            let (inst, job) = atlas.import_modpack(&pack)?;
            if !summary.loaders.is_empty() && !cli.json {
                eprintln!("Heads up: this pack needs {} which Atlas can't install yet. Files are imported, but the mods won't load in plain Minecraft.", summary.loaders.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "));
            }
            let out = run_job(&atlas, job, cli.json)?;
            Output { human: format!("{}\nCreated installation {} ({}).", out.human, inst.name, inst.id), data: json!({ "installation": inst, "summary": summary }) }
        }
        Command::Update { cmd } => match cmd.unwrap_or(UpdateCmd::Check) {
            UpdateCmd::Check => match atlas.check_for_update()? {
                Some(u) => Output { human: format!("Atlas {} is available (you have {}).\n{}\nInstall with: atlas update install", u.version, atlas_shared::ATLAS_VERSION, u.notes), data: to_value(&u) },
                None => Output { human: format!("You're on the latest version ({}).", atlas_shared::ATLAS_VERSION), data: json!({ "update": null }) },
            },
            UpdateCmd::Install => match atlas.check_for_update()? {
                Some(u) => {
                    atlas.install_update(&u)?;
                    Output { human: format!("Installed Atlas {}. Restart Atlas to use it.", u.version), data: to_value(&u) }
                }
                None => Output { human: "Already up to date.".into(), data: json!({ "update": null }) },
            },
            UpdateCmd::Rollback => {
                atlas.rollback_update()?;
                Output { human: "Restored the previous version.".into(), data: json!({ "rolled_back": true }) }
            }
        },
        Command::Mcp { cmd } => match cmd.unwrap_or(McpCmd::Serve) {
            McpCmd::Serve => {
                atlas_mcp::check_enabled(&atlas)?;
                atlas_mcp::check_token(&atlas, std::env::var("ATLAS_MCP_TOKEN").ok().as_deref())?;
                tracing::info!("mcp server started on stdio");
                atlas_mcp::Server::new(&atlas).run_stdio()?;
                Output { data: Value::Null, human: String::new() }
            }
            McpCmd::Token => {
                let t = atlas_mcp::ensure_token(&atlas.paths().bootstrap_dir)?;
                let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or("atlas".into());
                let human = format!(
                    "Token: {t}\n\nExample MCP client config:\n{{\n  \"mcpServers\": {{\n    \"atlas\": {{ \"command\": \"{exe}\", \"args\": [\"mcp\"], \"env\": {{ \"ATLAS_MCP_TOKEN\": \"{t}\" }} }}\n  }}\n}}\n\nThe server only runs while Settings → Advanced → AI and MCP are both on."
                );
                Output { data: json!({ "token": t }), human }
            }
            McpCmd::Pending => {
                let list = atlas_mcp::Approvals::new(atlas.paths().data_root.join("mcp")).pending();
                let human = if list.is_empty() { "Nothing waiting.".into() } else { list.iter().map(|p| format!("{}  {}", p.code, p.action)).collect::<Vec<_>>().join("\n") };
                Output { data: to_value(&list), human }
            }
            McpCmd::Approve { code } => {
                let p = atlas_mcp::Approvals::new(atlas.paths().data_root.join("mcp")).approve(&code)?;
                Output { human: format!("Approved: {}. The assistant can go ahead now.", p.action), data: to_value(&p) }
            }
        },
        Command::Detect => {
            let found = atlas.detect_minecraft();
            let human = if found.is_empty() {
                "No existing Minecraft folders found. Set one with: atlas settings set game.minecraft_dir \"/path/to/.minecraft\"".to_string()
            } else {
                found.iter().map(|d| format!("{:<28} {}  ({} versions)", d.kind.label(), d.path.display(), d.versions.len())).collect::<Vec<_>>().join("\n")
            };
            Output { data: to_value(&found), human }
        }
        Command::Logs { level, search, source, limit } => {
            let min_level = LogLevel::parse(&level).ok_or_else(|| {
                AtlasError::new(ErrorCode::InvalidArgument, format!("Unknown log level '{level}'."))
                    .suggest("Use trace, debug, info, warn or error.")
            })?;
            let source = source
                .map(|s| {
                    serde_json::from_value::<LogSource>(Value::String(s.to_lowercase())).map_err(|_| {
                        AtlasError::new(ErrorCode::InvalidArgument, format!("Unknown log source '{s}'."))
                            .suggest("Use launcher, game, crash, install or download.")
                    })
                })
                .transpose()?;
            let filter = LogFilter { min_level, search: search.unwrap_or_default(), source };
            let recs = atlas.file_logs(&filter, limit)?;
            let human = recs.iter().map(|r| r.to_line()).collect::<Vec<_>>().join("\n");
            Output { data: to_value(&recs), human }
        }
    })
}

fn human_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

/// Wait for a background job, drawing a progress line on stderr (human mode).
fn run_job(atlas: &Atlas, id: atlas_core::JobId, json: bool) -> AtlasResult<Output> {
    use std::io::Write;
    let tty = !json;
    let snap = atlas
        .wait_job(id, |s| {
            if tty {
                let p = &s.progress;
                eprint!(
                    "\r{:>5.1}%  {}/{} files  {} / {}  {}/s   ",
                    p.fraction() * 100.0,
                    p.done_files,
                    p.total_files,
                    human_bytes(p.done_bytes),
                    human_bytes(p.total_bytes),
                    human_bytes(s.speed_bps as u64)
                );
                let _ = std::io::stderr().flush();
            }
        })
        .ok_or_else(|| AtlasError::new(ErrorCode::Internal, "The job disappeared."))?;
    if tty {
        eprintln!();
    }
    match &snap.state {
        atlas_core::JobState::Done => {
            let p = &snap.progress;
            let human = format!(
                "{} {} in {:.1}s: {} downloaded, {} reused from other launchers, {} already valid.",
                match snap.kind { atlas_core::JobKind::Install => "Installed", atlas_core::JobKind::Repair => "Repaired", atlas_core::JobKind::Modpack => "Imported" },
                snap.version, snap.elapsed_secs, p.downloaded_files, p.reused_files, p.already_valid_files
            );
            Ok(Output { data: to_value(&snap), human })
        }
        atlas_core::JobState::Failed { error } => Err(error.clone()),
        atlas_core::JobState::Cancelled => Err(AtlasError::cancelled()),
        atlas_core::JobState::Running => Err(AtlasError::new(ErrorCode::Internal, "Job still running.")),
    }
}
