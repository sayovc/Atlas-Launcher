use super::*;
use atlas_config::AtlasPaths;
use std::fs;

fn fixture() -> (tempfile::TempDir, Atlas) {
    let t = tempfile::tempdir().unwrap();
    let paths = AtlasPaths::explicit(t.path().join("boot"), t.path().join("data"));
    let atlas = Atlas::open(paths).unwrap();
    (t, atlas)
}

#[test]
fn open_creates_layout_and_defaults() {
    let (_t, a) = fixture();
    let p = a.paths();
    assert!(p.settings_file().is_file());
    assert!(p.installations_dir().is_dir());
    assert_eq!(a.settings(), Settings::default());
}

#[test]
fn settings_persist_across_reopen() {
    let (t, a) = fixture();
    a.set_setting("memory.max_mb", "6144").unwrap();
    drop(a);
    let a2 = Atlas::open(AtlasPaths::explicit(t.path().join("boot"), t.path().join("data"))).unwrap();
    assert_eq!(a2.settings().memory.max_mb, 6144);
}

#[test]
fn invalid_setting_is_not_persisted() {
    let (_t, a) = fixture();
    assert!(a.set_setting("memory.min_mb", "999999").is_err());
    assert_eq!(a.settings().memory.min_mb, 1024);
}

#[test]
fn reset_requires_confirmation() {
    let (_t, a) = fixture();
    a.set_setting("memory.max_mb", "6144").unwrap();
    let e = a.reset_settings(Confirmation::NotConfirmed).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationRequired);
    assert_eq!(a.settings().memory.max_mb, 6144);
    a.reset_settings(Confirmation::Confirmed).unwrap();
    assert_eq!(a.settings().memory.max_mb, 4096);
}

#[test]
fn status_is_honest_without_installations() {
    let (_t, a) = fixture();
    let s = a.status();
    assert_eq!(s.game_state, GameState::NoInstallation);
    assert!(s.account.is_none());
    assert!(s.unavailable.contains(&"launch"));
}

#[test]
fn move_requires_confirmation() {
    let (t, a) = fixture();
    let e = a.move_data_root(&t.path().join("new"), Confirmation::NotConfirmed).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationRequired);
}

#[test]
fn move_data_root_preserves_worlds_and_unmanaged_files() {
    let (t, a) = fixture();
    a.set_setting("memory.max_mb", "5000").unwrap();
    let old = a.paths().data_root;
    let world = old.join("installations/abc/game/saves/My World");
    fs::create_dir_all(&world).unwrap();
    fs::write(world.join("level.dat"), b"precious").unwrap();
    fs::write(old.join("user-notes.txt"), b"not ours").unwrap();

    let new = t.path().join("moved");
    let r = a.move_data_root(&new, Confirmation::Confirmed).unwrap();
    assert!(r.files >= 2);
    assert_eq!(fs::read(new.join("installations/abc/game/saves/My World/level.dat")).unwrap(), b"precious");
    assert!(!old.join("installations").exists(), "managed originals removed");
    assert_eq!(fs::read(old.join("user-notes.txt")).unwrap(), b"not ours", "unmanaged file kept");
    assert_eq!(a.paths().data_root, new);
    assert_eq!(a.settings().memory.max_mb, 5000);

    // Locator persisted: reopening via bootstrap finds the new root.
    let reopened = AtlasPaths::from_bootstrap(t.path().join("boot")).unwrap();
    assert_eq!(reopened.data_root, new);
    // And settings saved after the move land in the new root.
    a.set_setting("memory.max_mb", "5100").unwrap();
    assert!(fs::read_to_string(new.join("settings.toml")).unwrap().contains("5100"));
}

#[test]
fn move_refuses_non_empty_and_nested_targets() {
    let (t, a) = fixture();
    let occupied = t.path().join("other-launcher");
    fs::create_dir_all(&occupied).unwrap();
    fs::write(occupied.join("launcher_profiles.json"), b"{}").unwrap();
    assert_eq!(a.move_data_root(&occupied, Confirmation::Confirmed).unwrap_err().code, ErrorCode::DataDirInvalid);
    assert_eq!(fs::read(occupied.join("launcher_profiles.json")).unwrap(), b"{}");

    let nested = a.paths().data_root.join("inner");
    assert_eq!(a.move_data_root(&nested, Confirmation::Confirmed).unwrap_err().code, ErrorCode::DataDirInvalid);
    assert_eq!(a.move_data_root(Path::new("relative/dir"), Confirmation::Confirmed).unwrap_err().code, ErrorCode::DataDirInvalid);
}

#[cfg(unix)]
#[test]
fn move_does_not_follow_symlinks_out_of_tree() {
    let (t, a) = fixture();
    let outside = t.path().join("dot-minecraft/saves");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("level.dat"), b"real world").unwrap();
    let inst = a.paths().data_root.join("installations/linked");
    fs::create_dir_all(&inst).unwrap();
    std::os::unix::fs::symlink(&outside, inst.join("saves")).unwrap();

    let r = a.move_data_root(&t.path().join("moved"), Confirmation::Confirmed).unwrap();
    assert_eq!(r.symlinks_skipped, 1);
    assert_eq!(fs::read(outside.join("level.dat")).unwrap(), b"real world", "link target must survive");
}
