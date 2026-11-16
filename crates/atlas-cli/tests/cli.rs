//! End-to-end tests of the `atlas` binary against an isolated data folder.
#![allow(clippy::unwrap_used)]

use assert_cmd::Command;
use serde_json::Value;

struct Env {
    _t: tempfile::TempDir,
    boot: std::path::PathBuf,
    root: std::path::PathBuf,
}

fn env() -> Env {
    let t = tempfile::tempdir().unwrap();
    Env { boot: t.path().join("boot"), root: t.path().to_path_buf(), _t: t }
}

impl Env {
    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("atlas").unwrap();
        c.env("ATLAS_CONFIG_DIR", &self.boot).env_remove("ATLAS_DATA_DIR").env("HOME", &self.root).env("XDG_DATA_HOME", self.root.join("xdg-data"));
        c
    }
    fn json(&self, args: &[&str]) -> (bool, Value) {
        let out = self.cmd().arg("--json").args(args).output().unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("not json: {}", String::from_utf8_lossy(&out.stdout)));
        (out.status.success(), v)
    }
}

#[test]
fn status_reports_no_installation() {
    let e = env();
    let (ok, v) = e.json(&["status"]);
    assert!(ok);
    assert_eq!(v["data"]["game_state"]["state"], "no_installation");
    assert!(v["data"]["account"].is_null());
}

#[test]
fn settings_set_get_and_errors() {
    let e = env();
    let (ok, _) = e.json(&["settings", "set", "memory.max_mb", "6144"]);
    assert!(ok);
    let (_, v) = e.json(&["settings", "get", "memory.max_mb"]);
    assert_eq!(v["data"], 6144);

    let (ok, v) = e.json(&["settings", "set", "memory.max_mb", "10"]);
    assert!(!ok);
    assert_eq!(v["error"]["code"], "CONFIG_INVALID");
    assert!(!v["error"]["suggestion"].as_str().unwrap().is_empty());

    let (ok, v) = e.json(&["settings", "set", "nope.key", "1"]);
    assert!(!ok);
    assert_eq!(v["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn destructive_commands_need_yes() {
    let e = env();
    let out = e.cmd().args(["--json", "settings", "reset"]).output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["code"], "CONFIRMATION_REQUIRED");
    e.cmd().args(["settings", "reset", "--yes"]).assert().success();
}

#[test]
fn data_dir_move_roundtrip() {
    let e = env();
    let (_, before) = e.json(&["paths"]);
    let old = before["data"]["data_root"].as_str().unwrap().to_string();
    let target = e.root.join("relocated");
    let (ok, v) = e.json(&["data-dir", "move", target.to_str().unwrap(), "--yes"]);
    assert!(ok, "{v}");
    let (_, after) = e.json(&["paths"]);
    assert_eq!(after["data"]["data_root"].as_str().unwrap(), target.to_str().unwrap());
    assert!(after["data"]["data_root_is_custom"].as_bool().unwrap());
    assert!(!std::path::Path::new(&old).join("settings.toml").exists());
}

#[test]
fn logs_are_written_and_queryable() {
    let e = env();
    e.cmd().arg("status").assert().success();
    let (ok, v) = e.json(&["logs", "--search", "Atlas started"]);
    assert!(ok);
    assert!(!v["data"].as_array().unwrap().is_empty(), "{v}");
    let (ok, v) = e.json(&["logs", "--level", "loud"]);
    assert!(!ok);
    assert_eq!(v["error"]["code"], "INVALID_ARGUMENT");
}

// ---------------------------------------------------------------- versions

fn sha1(b: &[u8]) -> String {
    atlas_downloads::sha1_hex(b)
}

/// Minimal fake Mojang tree: one version with a client jar and one asset.
fn fake_mojang() -> (atlas_downloads::testing::TestServer, Vec<u8>) {
    let (srv, files) = atlas_downloads::testing::serve_dynamic(true);
    let base = srv.base.clone();
    let jar: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
    let asset = b"hello asset".to_vec();
    let ah = sha1(&asset);
    let index = serde_json::to_vec(&serde_json::json!({"objects": {"a.txt": {"hash": ah, "size": asset.len()}}})).unwrap();
    let version = serde_json::to_vec(&serde_json::json!({
        "id": "9.9-test", "type": "release", "mainClass": "x.Main",
        "assetIndex": {"id": "t", "sha1": sha1(&index), "size": index.len(), "totalSize": 1, "url": format!("{base}/idx.json")},
        "downloads": {"client": {"sha1": sha1(&jar), "size": jar.len(), "url": format!("{base}/client.jar")}},
        "libraries": []
    })).unwrap();
    let manifest = serde_json::to_vec(&serde_json::json!({
        "latest": {"release": "9.9-test", "snapshot": "9.9-test"},
        "versions": [{"id": "9.9-test", "type": "release", "url": format!("{base}/v.json"), "time": "x", "releaseTime": "2030-01-01T00:00:00+00:00", "sha1": sha1(&version)}]
    })).unwrap();
    let mut f = files.lock().unwrap();
    f.insert("/manifest.json".into(), manifest);
    f.insert("/v.json".into(), version);
    f.insert("/idx.json".into(), index);
    f.insert("/client.jar".into(), jar.clone());
    f.insert(format!("/{}/{}", &ah[..2], ah), asset);
    drop(f);
    (srv, jar)
}

#[test]
fn version_lifecycle_through_cli() {
    let e = env();
    let (srv, _jar) = fake_mojang();
    let run = |args: &[&str]| {
        let out = e.cmd()
            .env("ATLAS_VERSION_MANIFEST_URL", format!("{}/manifest.json", srv.base))
            .env("ATLAS_ASSETS_BASE_URL", &srv.base)
            .arg("--json").args(args).output().unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("not json: {}", String::from_utf8_lossy(&out.stdout)));
        (out.status.code(), v)
    };

    let (code, v) = run(&["versions", "--refresh"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["data"]["latest_release"], "9.9-test");

    let (code, v) = run(&["install", "latest"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["data"]["state"]["state"], "done");

    let (_, v) = run(&["versions", "--installed"]);
    assert_eq!(v["data"][0]["id"], "9.9-test");

    let (code, _) = run(&["verify", "9.9-test"]);
    assert_eq!(code, Some(0));

    // Corrupt the jar: verify fails, repair fixes it.
    let (_, paths) = run(&["paths"]);
    let jar_path = std::path::Path::new(paths["data"]["data_root"].as_str().unwrap()).join("versions/9.9-test/9.9-test.jar");
    std::fs::write(&jar_path, b"broken").unwrap();
    let (code, v) = run(&["verify", "9.9-test"]);
    assert_eq!(code, Some(1));
    assert_eq!(v["error"]["code"], "INVALID_INSTALLATION");
    let (code, v) = run(&["repair", "9.9-test"]);
    assert_eq!(code, Some(0), "{v}");
    let (code, _) = run(&["verify", "9.9-test"]);
    assert_eq!(code, Some(0));

    // Remove needs --yes.
    let (code, v) = run(&["remove", "9.9-test"]);
    assert_eq!(code, Some(3));
    assert_eq!(v["error"]["code"], "CONFIRMATION_REQUIRED");
    let (code, _) = run(&["remove", "9.9-test", "--yes"]);
    assert_eq!(code, Some(0));
    assert!(!jar_path.exists());

    let (code, v) = run(&["install", "0.0-nope"]);
    assert_eq!(code, Some(1));
    assert_eq!(v["error"]["code"], "VERSION_NOT_FOUND");
}
