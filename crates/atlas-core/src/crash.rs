//! When Atlas itself panics: write a report (redacted) so it can be
//! looked at, and tell the user about it on the next start.

use std::path::{Path, PathBuf};

fn crash_dir(logs: &Path) -> PathBuf {
    logs.join("crash")
}

/// Install a panic hook that writes `logs/crash/atlas-<time>.txt`.
pub fn install_panic_hook(logs_dir: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let bt = std::backtrace::Backtrace::force_capture();
        let report = format!(
            "Atlas Launcher {} crashed\nos: {} {}\n\n{info}\n\n{bt}\n",
            atlas_shared::ATLAS_VERSION,
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        let report = atlas_shared::redact::redact(&report).into_owned();
        let dir = crash_dir(&logs_dir);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(format!("atlas-{when}.txt")), &report);
        tracing::error!(log_source = "crash", "Atlas crashed: {info}");
        previous(info);
    }));
}

/// Crash reports written since the last time we looked (then marks them seen).
pub fn unseen_reports(logs_dir: &Path) -> Vec<PathBuf> {
    let dir = crash_dir(logs_dir);
    let seen_file = dir.join(".seen");
    let seen: u64 = std::fs::read_to_string(&seen_file).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let mut newest = seen;
    let mut out = vec![];
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(ts) = name.strip_prefix("atlas-").and_then(|n| n.strip_suffix(".txt")).and_then(|n| n.parse::<u64>().ok()) else { continue };
        if ts > seen {
            out.push(e.path());
            newest = newest.max(ts);
        }
    }
    if newest > seen {
        let _ = std::fs::write(seen_file, newest.to_string());
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_are_shown_once() {
        let t = tempfile::tempdir().unwrap();
        let d = crash_dir(t.path());
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("atlas-100.txt"), "x").unwrap();
        assert_eq!(unseen_reports(t.path()).len(), 1);
        assert!(unseen_reports(t.path()).is_empty());
        std::fs::write(d.join("atlas-200.txt"), "y").unwrap();
        assert_eq!(unseen_reports(t.path()).len(), 1);
    }
}
