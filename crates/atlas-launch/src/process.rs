//! Running the game: pipe its output into our logs, notice when it exits,
//! and tell a crash from a normal quit.

use crate::command::{display_command, LaunchCommand};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::Serialize;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

#[derive(Debug, Clone, Serialize)]
pub struct GameExit {
    pub code: Option<i32>,
    pub crashed: bool,
    pub stopped_by_user: bool,
    pub crash_report: Option<PathBuf>,
    /// Last lines the game printed, handy for error screens.
    pub tail: Vec<String>,
}

pub struct GameProcess {
    child: Child,
    pub pid: u32,
    started: SystemTime,
    game_dir: PathBuf,
    tail: Arc<Mutex<VecDeque<String>>>,
}

const TAIL: usize = 60;

fn pipe<R: tokio::io::AsyncRead + Unpin + Send + 'static>(r: R, stream: &'static str, tail: Arc<Mutex<VecDeque<String>>>) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(r).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let clean = atlas_shared::redact::redact(&line).into_owned();
            if stream == "stderr" {
                tracing::warn!(target: "minecraft", log_source = "game", "{clean}");
            } else {
                tracing::info!(target: "minecraft", log_source = "game", "{clean}");
            }
            let mut t = tail.lock().unwrap_or_else(|p| p.into_inner());
            if t.len() == TAIL {
                t.pop_front();
            }
            t.push_back(clean);
        }
    });
}

impl GameProcess {
    pub fn spawn(cmd: &LaunchCommand) -> AtlasResult<Self> {
        std::fs::create_dir_all(&cmd.cwd).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "create the game folder", &cmd.cwd, e))?;
        tracing::info!(log_source = "game", "starting: {}", display_command(cmd));
        let mut c = Command::new(&cmd.program);
        c.args(&cmd.args).current_dir(&cmd.cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(false);
        let mut child = c.spawn().map_err(|e| {
            AtlasError::new(ErrorCode::PreparationFailed, "Java couldn't be started.")
                .details(format!("{}: {e}", cmd.program.display()))
                .suggest("Check the Java path in Settings → Game, or clear it to let Atlas download Java.")
        })?;
        let pid = child.id().unwrap_or(0);
        let tail: Arc<Mutex<VecDeque<String>>> = Arc::default();
        if let Some(o) = child.stdout.take() {
            pipe(o, "stdout", tail.clone());
        }
        if let Some(e) = child.stderr.take() {
            pipe(e, "stderr", tail.clone());
        }
        Ok(Self { child, pid, started: SystemTime::now(), game_dir: cmd.cwd.clone(), tail })
    }

    /// Wait for the game to exit. `stop` resolves when the user hits Stop.
    pub async fn wait(mut self, stop: tokio::sync::oneshot::Receiver<()>) -> GameExit {
        let mut stopped = false;
        let status = tokio::select! {
            s = self.child.wait() => s,
            _ = stop => {
                stopped = true;
                terminate(self.pid);
                // give it a few seconds to save the world, then pull the plug
                match tokio::time::timeout(std::time::Duration::from_secs(10), self.child.wait()).await {
                    Ok(s) => s,
                    Err(_) => {
                        let _ = self.child.start_kill();
                        self.child.wait().await
                    }
                }
            }
        };
        // let the pipe readers drain
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let code = status.ok().and_then(|s| s.code());
        let crash_report = newest_crash_report(&self.game_dir, self.started);
        // no exit code + no crash report = killed by a signal (e.g. `atlas stop`
        // from another terminal); that's a stop, not a crash
        let crashed = !stopped && (crash_report.is_some() || code.is_some_and(|c| c != 0));
        let tail: Vec<String> = self.tail.lock().unwrap_or_else(|p| p.into_inner()).iter().cloned().collect();
        if crashed {
            tracing::error!(log_source = "crash", code = ?code, report = ?crash_report, "the game crashed");
            if let Some(r) = &crash_report
                && let Ok(text) = std::fs::read_to_string(r)
            {
                for line in text.lines().take(80) {
                    tracing::error!(target: "minecraft", log_source = "crash", "{}", atlas_shared::redact::redact(line));
                }
            }
        } else {
            tracing::info!(log_source = "game", code = ?code, stopped, "game exited");
        }
        GameExit { code, crashed, stopped_by_user: stopped, crash_report, tail }
    }
}

fn newest_crash_report(game_dir: &Path, since: SystemTime) -> Option<PathBuf> {
    // file times can be coarser than the clock, so allow a little slack
    let since = since.checked_sub(std::time::Duration::from_secs(2)).unwrap_or(since);
    std::fs::read_dir(game_dir.join("crash-reports"))
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t >= since))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

/// Ask a process to quit politely. Used for our own child and by
/// `atlas stop` from another process.
pub fn terminate(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    let _ = std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).status();
    #[cfg(windows)]
    let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string()]).status();
}

pub fn is_alive(pid: u32) -> bool {
    process_line(pid).is_some()
}

/// Alive *and* a java process. A pid file can outlive its game (crash,
/// killed launcher) and the OS may hand the number to something else.
pub fn is_game_process(pid: u32) -> bool {
    process_line(pid).is_some_and(|l| l.to_ascii_lowercase().contains("java"))
}

fn process_line(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    #[cfg(unix)]
    let out = std::process::Command::new("ps").args(["-p", &pid.to_string(), "-o", "command="]).stderr(Stdio::null()).output().ok()?;
    #[cfg(windows)]
    let out = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !text.is_empty() && !text.starts_with("INFO:")).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn normal_exit_and_crash_detection() {
        let t = tempfile::tempdir().unwrap();
        let ok = LaunchCommand { program: "sh".into(), args: vec!["-c".into(), "echo hello; exit 0".into()], cwd: t.path().into(), native_jars: vec![] };
        let (_tx, rx) = tokio::sync::oneshot::channel();
        let exit = GameProcess::spawn(&ok).unwrap().wait(rx).await;
        assert!(!exit.crashed);
        assert_eq!(exit.tail, vec!["hello"]);

        let bad = LaunchCommand {
            program: "sh".into(),
            args: vec!["-c".into(), "mkdir -p crash-reports; echo boom > crash-reports/crash-1.txt; echo dying >&2; exit 1".into()],
            cwd: t.path().into(),
            native_jars: vec![],
        };
        let (_tx, rx) = tokio::sync::oneshot::channel();
        let exit = GameProcess::spawn(&bad).unwrap().wait(rx).await;
        assert!(exit.crashed);
        assert_eq!(exit.code, Some(1));
        assert!(exit.crash_report.is_some());
    }

    #[tokio::test]
    async fn stop_is_not_a_crash() {
        let t = tempfile::tempdir().unwrap();
        let c = LaunchCommand { program: "sleep".into(), args: vec!["30".into()], cwd: t.path().into(), native_jars: vec![] };
        let p = GameProcess::spawn(&c).unwrap();
        assert!(is_alive(p.pid));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let h = tokio::spawn(p.wait(rx));
        tx.send(()).unwrap();
        let exit = h.await.unwrap();
        assert!(exit.stopped_by_user);
        assert!(!exit.crashed);
    }

    #[test]
    fn missing_java_is_a_clear_error() {
        let c = LaunchCommand { program: "/nope/java".into(), args: vec![], cwd: std::env::temp_dir(), native_jars: vec![] };
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let e = rt.block_on(async { GameProcess::spawn(&c).err().unwrap() });
        assert_eq!(e.code, ErrorCode::PreparationFailed);
    }
}
