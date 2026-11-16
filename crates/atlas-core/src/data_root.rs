//! Relocating the Atlas data root.
//!
//! Rules:
//! 1. Target must be absolute, outside the current root (and vice versa), and
//!    either missing or an empty directory: Atlas never merges into or
//!    overwrites an existing folder (so another launcher's data is safe).
//! 2. Only Atlas-managed items move ([`MANAGED_SUBDIRS`], [`MANAGED_FILES`]).
//! 3. Copy without following symlinks, then verify file counts and byte
//!    totals per item. Any mismatch rolls back the copy; the original is
//!    untouched.
//! 4. Only after the locator points at the verified copy are originals
//!    removed. A failed removal is reported, never fatal: data is safe in
//!    the new location.

use atlas_config::paths::{AtlasPaths, MANAGED_FILES, MANAGED_SUBDIRS};
use atlas_shared::fs_safety::{copy_tree_no_follow, is_within, measure_tree, remove_tree_no_follow};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct MoveReport {
    pub old_root: PathBuf,
    pub new_root: PathBuf,
    pub files: u64,
    pub bytes: u64,
    /// Symlinks inside managed folders are not copied (their targets are
    /// outside Atlas's control); listed so the user knows.
    pub symlinks_skipped: u64,
    /// Originals that could not be removed (e.g. a log file held open on
    /// Windows). Safe to delete manually.
    pub leftovers: Vec<PathBuf>,
    /// Logging keeps writing to the old location until Atlas restarts.
    pub restart_required: bool,
}

fn invalid(msg: impl Into<String>, target: &Path) -> AtlasError {
    AtlasError::new(ErrorCode::DataDirInvalid, msg)
        .details(target.display())
        .suggest("Choose a new, empty folder outside the current Atlas data folder.")
}

pub(crate) fn move_root(paths: &AtlasPaths, target: &Path) -> AtlasResult<MoveReport> {
    let old = paths.data_root.clone();
    if !target.is_absolute() {
        return Err(invalid("The new data folder must be an absolute path.", target));
    }
    if is_within(&old, target) || is_within(target, &old) {
        return Err(invalid("The new data folder cannot be inside the current one, or contain it.", target));
    }
    match std::fs::symlink_metadata(target) {
        Ok(m) if m.file_type().is_symlink() => {
            return Err(AtlasError::new(ErrorCode::UnsafeSymlink, "The new data folder is a symbolic link.")
                .details(target.display())
                .suggest("Choose the real folder instead of a link."));
        }
        Ok(m) if !m.is_dir() => return Err(invalid("The new data folder path points to a file.", target)),
        Ok(_) => {
            let non_empty = std::fs::read_dir(target)
                .map_err(|e| AtlasError::io(ErrorCode::DataDirInvalid, "read the new data folder", target, e))?
                .next()
                .is_some();
            if non_empty {
                return Err(invalid("The new data folder is not empty. Atlas never merges into existing folders.", target));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(AtlasError::io(ErrorCode::DataDirInvalid, "check the new data folder", target, e)),
    }
    std::fs::create_dir_all(target)
        .map_err(|e| AtlasError::io(ErrorCode::DataDirMoveFailed, "create the new data folder", target, e))?;

    tracing::info!(from = %old.display(), to = %target.display(), "moving data folder");

    let items: Vec<&str> = MANAGED_SUBDIRS.iter().chain(MANAGED_FILES).copied()
        .filter(|n| std::fs::symlink_metadata(old.join(n)).is_ok())
        .collect();

    let mut report = MoveReport {
        old_root: old.clone(),
        new_root: target.to_path_buf(),
        files: 0,
        bytes: 0,
        symlinks_skipped: 0,
        leftovers: vec![],
        restart_required: true,
    };

    let rollback = |why: AtlasError| -> AtlasError {
        // Only remove what we created; target was empty/missing at start.
        if let Err(e) = remove_tree_no_follow(target) {
            tracing::error!(code = %e.code, "rollback of partial copy failed: {}", e.details);
        }
        tracing::error!(code = %why.code, "data folder move aborted; original untouched");
        why
    };

    for name in &items {
        let (src, dst) = (old.join(name), target.join(name));
        let copied = copy_tree_no_follow(&src, &dst).map_err(rollback)?;
        let expected = measure_tree(&src).map_err(rollback)?;
        let actual = measure_tree(&dst).map_err(rollback)?;
        if expected.files != actual.files || expected.bytes != actual.bytes {
            return Err(rollback(
                AtlasError::new(ErrorCode::DataDirMoveFailed, "The copied data did not match the original.")
                    .details(format!("{name}: expected {} files/{} bytes, got {} files/{} bytes", expected.files, expected.bytes, actual.files, actual.bytes))
                    .suggest("Nothing was changed. Check free space on the target drive and try again."),
            ));
        }
        report.files += copied.files;
        report.bytes += copied.bytes;
        report.symlinks_skipped += copied.symlinks_skipped;
    }

    // Point Atlas at the verified copy before touching originals.
    paths.write_locator(Some(target)).map_err(rollback)?;

    for name in &items {
        let src = old.join(name);
        if let Err(e) = remove_tree_no_follow(&src) {
            tracing::warn!(path = %src.display(), "could not remove original after move: {}", e.details);
            report.leftovers.push(src);
        }
    }
    // Remove the old root only if nothing else (unmanaged) lives there.
    let _ = std::fs::remove_dir(&old);
    tracing::info!(files = report.files, bytes = report.bytes, "data folder moved");
    Ok(report)
}
