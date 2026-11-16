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
            } else {
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
    tracing::debug!("cli invoked");

    Ok(match cli.command {
        Command::Status => {
            let s = atlas.status();
            let mut human = format!(
                "Atlas {}\nData folder:    {}\nAccount:        {}\nInstallation:   {}\nMinecraft:      {}\nGame state:     {:?}\nNot yet built:  {}",
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
                match snap.kind { atlas_core::JobKind::Install => "Installed", atlas_core::JobKind::Repair => "Repaired" },
                snap.version, snap.elapsed_secs, p.downloaded_files, p.reused_files, p.already_valid_files
            );
            Ok(Output { data: to_value(&snap), human })
        }
        atlas_core::JobState::Failed { error } => Err(error.clone()),
        atlas_core::JobState::Cancelled => Err(AtlasError::cancelled()),
        atlas_core::JobState::Running => Err(AtlasError::new(ErrorCode::Internal, "Job still running.")),
    }
}
