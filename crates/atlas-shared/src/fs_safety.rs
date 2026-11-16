//! Filesystem helpers for paths we didn't choose ourselves.
//!
//! Anything coming from an archive, a manifest or the user goes through
//! `safe_join`. The copy/remove helpers never follow symlinks, so a link
//! to someone's real `.minecraft` can't be copied or deleted by accident.

use crate::{AtlasError, AtlasResult, ErrorCode};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Join an untrusted relative path onto `base`, rejecting absolute paths,
/// `..` components, drive prefixes and empty paths.
pub fn safe_join(base: &Path, untrusted: &str) -> AtlasResult<PathBuf> {
    let rel = Path::new(untrusted);
    if untrusted.is_empty() || untrusted.contains('\0') {
        return Err(traversal(untrusted));
    }
    let mut out = base.to_path_buf();
    for comp in rel.components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(traversal(untrusted));
            }
        }
    }
    if out == base {
        return Err(traversal(untrusted));
    }
    Ok(out)
}

fn traversal(p: &str) -> AtlasError {
    AtlasError::new(ErrorCode::PathTraversal, "A file path tried to escape its allowed folder.")
        .details(format!("rejected path: {p:?}"))
        .suggest("The source (archive, manifest or input) may be malicious or corrupted. Do not use it.")
}

/// `true` if `child` is `parent` or lies beneath it (lexically, after
/// canonicalising whatever prefix exists).
pub fn is_within(parent: &Path, child: &Path) -> bool {
    let canon = |p: &Path| -> PathBuf {
        // Canonicalise the longest existing ancestor, then re-append the rest.
        let mut existing = p.to_path_buf();
        let mut tail = Vec::new();
        while !existing.exists() {
            match (existing.file_name(), existing.parent()) {
                (Some(name), Some(par)) => {
                    tail.push(name.to_owned());
                    existing = par.to_path_buf();
                }
                _ => break,
            }
        }
        let mut c = fs::canonicalize(&existing).unwrap_or(existing);
        for t in tail.into_iter().rev() {
            c.push(t);
        }
        c
    };
    canon(child).starts_with(canon(parent))
}

/// Statistics returned by tree operations so callers can verify copies.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TreeStats {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub symlinks_skipped: u64,
}

/// Recursively copy `src` into `dst` without following symlinks. Symlinks are
/// skipped (and counted) rather than copied, so a link pointing at e.g. the
/// user's real `.minecraft/saves` can never be duplicated or later deleted
/// through Atlas.
pub fn copy_tree_no_follow(src: &Path, dst: &Path) -> AtlasResult<TreeStats> {
    let mut stats = TreeStats::default();
    copy_inner(src, dst, &mut stats)?;
    Ok(stats)
}

fn copy_inner(src: &Path, dst: &Path, st: &mut TreeStats) -> AtlasResult<()> {
    let meta = fs::symlink_metadata(src)
        .map_err(|e| AtlasError::io(ErrorCode::DataDirMoveFailed, "read a file", src, e))?;
    if meta.file_type().is_symlink() {
        st.symlinks_skipped += 1;
        return Ok(());
    }
    if meta.is_dir() {
        fs::create_dir_all(dst)
            .map_err(|e| AtlasError::io(ErrorCode::DataDirMoveFailed, "create a folder", dst, e))?;
        st.dirs += 1;
        let rd = fs::read_dir(src)
            .map_err(|e| AtlasError::io(ErrorCode::DataDirMoveFailed, "list a folder", src, e))?;
        for entry in rd {
            let entry = entry
                .map_err(|e| AtlasError::io(ErrorCode::DataDirMoveFailed, "list a folder", src, e))?;
            copy_inner(&entry.path(), &dst.join(entry.file_name()), st)?;
        }
    } else if meta.is_file() {
        let n = fs::copy(src, dst)
            .map_err(|e| AtlasError::io(ErrorCode::DataDirMoveFailed, "copy a file", src, e))?;
        st.files += 1;
        st.bytes += n;
    }
    Ok(())
}

/// Measure a tree the same way [`copy_tree_no_follow`] would copy it.
pub fn measure_tree(root: &Path) -> AtlasResult<TreeStats> {
    let mut st = TreeStats::default();
    measure_inner(root, &mut st)?;
    Ok(st)
}

fn measure_inner(p: &Path, st: &mut TreeStats) -> AtlasResult<()> {
    let meta = fs::symlink_metadata(p)
        .map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "read a file", p, e))?;
    if meta.file_type().is_symlink() {
        st.symlinks_skipped += 1;
    } else if meta.is_dir() {
        st.dirs += 1;
        for entry in fs::read_dir(p).map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "list a folder", p, e))? {
            let entry = entry.map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "list a folder", p, e))?;
            measure_inner(&entry.path(), st)?;
        }
    } else if meta.is_file() {
        st.files += 1;
        st.bytes += meta.len();
    }
    Ok(())
}

/// Remove a tree without following symlinks: a symlink inside the tree is
/// unlinked, never traversed. Refuses to operate on a symlinked root.
pub fn remove_tree_no_follow(root: &Path) -> AtlasResult<()> {
    let meta = match fs::symlink_metadata(root) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(AtlasError::io(ErrorCode::ConfigIo, "read a folder", root, e)),
    };
    if meta.file_type().is_symlink() {
        return Err(AtlasError::new(ErrorCode::UnsafeSymlink, "Refusing to delete through a symbolic link.")
            .details(root.display())
            .suggest("Remove the link manually if this is intended."));
    }
    // std::fs::remove_dir_all does not follow symlinks on any tier-1 platform
    // (it unlinks them), and is hardened against TOCTOU races since Rust 1.58.
    if meta.is_dir() {
        fs::remove_dir_all(root)
    } else {
        fs::remove_file(root)
    }
    .map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "delete a folder", root, e))
}

/// Atomically write `bytes` to `path` (write to a sibling temp file, fsync,
/// rename). Readers never observe a half-written file.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> AtlasResult<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "create a folder", dir, e))?;
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{file_name}.tmp-{}", std::process::id()));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(AtlasError::io(ErrorCode::ConfigIo, "save a file", path, e));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_accepts_normal_paths() {
        let b = Path::new("/base");
        assert_eq!(safe_join(b, "a/b.txt").unwrap(), Path::new("/base/a/b.txt"));
        assert_eq!(safe_join(b, "./a").unwrap(), Path::new("/base/a"));
    }

    #[test]
    fn safe_join_rejects_traversal() {
        let b = Path::new("/base");
        for bad in ["../x", "a/../../x", "/etc/passwd", "", ".", "a\0b"] {
            let e = safe_join(b, bad).unwrap_err();
            assert_eq!(e.code, ErrorCode::PathTraversal, "{bad:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn safe_join_rejects_drive_prefix() {
        assert!(safe_join(Path::new("C:\\base"), "D:\\x").is_err());
    }

    #[test]
    fn is_within_works_for_missing_children() {
        let t = std::env::temp_dir();
        assert!(is_within(&t, &t.join("does/not/exist")));
        assert!(!is_within(&t.join("a"), &t.join("b")));
    }
}
