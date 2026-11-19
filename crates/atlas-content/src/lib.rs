//! Everything that lives inside a game folder or next to it: servers,
//! mods, resource/shader packs, worlds, and modpack imports.

pub mod bundles;
pub mod modrinth;
pub mod mods;
pub mod mrpack;
pub mod nbt;
pub mod packs;
pub mod ping;
pub mod servers;
pub mod worlds;

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Size of a folder (files only, symlinks ignored).
pub(crate) fn dir_size(p: &std::path::Path) -> u64 {
    atlas_shared::fs_safety::measure_tree(p).map(|s| s.bytes).unwrap_or(0)
}
