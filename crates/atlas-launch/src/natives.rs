//! Old versions ship native libraries inside "natives" jars that have to be
//! unpacked before launch. Newer versions let LWJGL do this itself.

use atlas_shared::fs_safety::safe_join;
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use std::path::Path;

// big enough for any real native jar, small enough to stop zip bombs
const MAX_ENTRY: u64 = 64 * 1024 * 1024;
const MAX_TOTAL: u64 = 512 * 1024 * 1024;

pub fn extract(jars: &[impl AsRef<Path>], into: &Path) -> AtlasResult<usize> {
    std::fs::create_dir_all(into).map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "create the natives folder", into, e))?;
    let mut count = 0;
    let mut total = 0u64;
    for jar in jars {
        let jar = jar.as_ref();
        let f = std::fs::File::open(jar).map_err(|e| AtlasError::io(ErrorCode::InvalidInstallation, "open a natives jar", jar, e))?;
        let mut zip = zip::ZipArchive::new(f).map_err(|e| bad_zip(jar, e))?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| bad_zip(jar, e))?;
            let name = entry.name().to_string();
            if entry.is_dir() || name.starts_with("META-INF/") {
                continue;
            }
            if entry.is_symlink() {
                return Err(AtlasError::new(ErrorCode::UnsafeSymlink, "A natives jar contains a symbolic link.").details(format!("{}: {name}", jar.display())));
            }
            let dest = safe_join(into, &name)?;
            if entry.size() > MAX_ENTRY {
                return Err(bad_zip(jar, "entry too large"));
            }
            total += entry.size();
            if total > MAX_TOTAL {
                return Err(bad_zip(jar, "archive too large"));
            }
            if let Some(d) = dest.parent() {
                std::fs::create_dir_all(d).map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "create a folder", d, e))?;
            }
            let mut out = std::fs::File::create(&dest).map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "write a native library", &dest, e))?;
            let n = std::io::copy(&mut (&mut entry).take(MAX_ENTRY + 1), &mut out).map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "write a native library", &dest, e))?;
            if n > MAX_ENTRY {
                return Err(bad_zip(jar, "entry larger than it claimed"));
            }
            count += 1;
        }
    }
    Ok(count)
}

use std::io::Read as _;

fn bad_zip(jar: &Path, e: impl std::fmt::Display) -> AtlasError {
    AtlasError::new(ErrorCode::InvalidInstallation, "A natives jar couldn't be unpacked.")
        .details(format!("{}: {e}", jar.display()))
        .suggest("Repair the version and try again.")
}
