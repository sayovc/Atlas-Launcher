use super::*;
use crate::rules::Platform;
use atlas_downloads::testing::TestServer;
use atlas_downloads::{sha1_hex, CancellationToken, Downloader, DownloaderConfig, Progress};
use atlas_shared::ErrorCode;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

struct Fixture {
    server: TestServer,
    files: HashMap<String, Vec<u8>>,
}

fn bytes(tag: &str, n: usize) -> Vec<u8> {
    let mut v = tag.as_bytes().to_vec();
    v.extend((0..n).map(|i| (i % 253) as u8));
    v
}

/// Build a fake Mojang tree. `lib_path` lets tests inject a malicious path.
fn fixture(lib_path: &str, tamper_version_sha: bool) -> Fixture {
    // Server is created first so URLs can embed its address: bind, then fill files.
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();

    let client = bytes("client", 120_000);
    let lib_common = bytes("common", 30_000);
    let lib_win = bytes("win", 10_000);
    let lib_native_linux = bytes("natlin", 8_000);
    let log_cfg = bytes("log", 500);
    let a1 = bytes("asset1", 5_000);
    let a2 = bytes("asset2", 7_000);
    let (h1, h2) = (sha1_hex(&a1), sha1_hex(&a2));
    let index = serde_json::to_vec(&json!({"objects": {
        "minecraft/sounds/a.ogg": {"hash": h1, "size": a1.len()},
        "minecraft/lang/x.json": {"hash": h2, "size": a2.len()},
        "minecraft/sounds/dup.ogg": {"hash": h1, "size": a1.len()},
    }}))
    .unwrap();

    // We need the base URL inside JSON; use a two-pass approach with a fixed server.
    let server = serve_lazy();
    let base = server.0.base.clone();
    let version = serde_json::to_vec(&json!({
        "id": "1.0-test", "type": "release", "mainClass": "net.minecraft.client.main.Main",
        "releaseTime": "2026-01-01T00:00:00+00:00",
        "javaVersion": {"component": "java-runtime-delta", "majorVersion": 21},
        "assetIndex": {"id": "17", "sha1": sha1_hex(&index), "size": index.len(), "totalSize": 12000, "url": format!("{base}/idx/17.json")},
        "downloads": {"client": {"sha1": sha1_hex(&client), "size": client.len(), "url": format!("{base}/client.jar")}},
        "logging": {"client": {"argument": "-Dlog4j.configurationFile=${path}", "type": "log4j2-xml",
            "file": {"id": "client-1.12.xml", "sha1": sha1_hex(&log_cfg), "size": log_cfg.len(), "url": format!("{base}/log.xml")}}},
        "libraries": [
            {"name": "org.example:common:1.0", "downloads": {"artifact": {"path": lib_path, "sha1": sha1_hex(&lib_common), "size": lib_common.len(), "url": format!("{base}/lib/common.jar")}}},
            {"name": "org.example:winonly:1.0", "rules": [{"action": "allow", "os": {"name": "windows"}}],
             "downloads": {"artifact": {"path": "org/example/winonly/1.0/winonly-1.0.jar", "sha1": sha1_hex(&lib_win), "size": lib_win.len(), "url": format!("{base}/lib/win.jar")}}},
            {"name": "org.example:legacy-natives:1.0", "natives": {"linux": "natives-linux", "windows": "natives-windows-${arch}"},
             "rules": [{"action": "allow"}, {"action": "disallow", "os": {"name": "osx"}}],
             "downloads": {"classifiers": {"natives-linux": {"path": "org/example/nat/1.0/nat-1.0-natives-linux.jar", "sha1": sha1_hex(&lib_native_linux), "size": lib_native_linux.len(), "url": format!("{base}/lib/natlin.jar")}}}},
            {"name": "org.example:demo:1.0", "rules": [{"action": "allow", "features": {"is_demo_user": true}}],
             "downloads": {"artifact": {"path": "org/example/demo.jar", "sha1": sha1_hex(b"x"), "size": 1, "url": format!("{base}/lib/demo.jar")}}}
        ]
    }))
    .unwrap();
    let vsha = if tamper_version_sha { "0".repeat(40) } else { sha1_hex(&version) };
    let manifest = serde_json::to_vec(&json!({
        "latest": {"release": "1.0-test", "snapshot": "1.0-test"},
        "versions": [
            {"id": "1.0-test", "type": "release", "url": format!("{base}/v/1.0-test.json"), "time": "x", "releaseTime": "2026-01-01T00:00:00+00:00", "sha1": vsha},
            {"id": "0.9-test", "type": "snapshot", "url": format!("{base}/v/missing.json"), "time": "x", "releaseTime": "2025-01-01T00:00:00+00:00", "sha1": "1".repeat(40)}
        ]
    }))
    .unwrap();
    files.insert("/manifest.json".into(), manifest);
    files.insert("/v/1.0-test.json".into(), version);
    files.insert("/idx/17.json".into(), index);
    files.insert("/client.jar".into(), client);
    files.insert("/lib/common.jar".into(), lib_common);
    files.insert("/lib/win.jar".into(), lib_win);
    files.insert("/lib/natlin.jar".into(), lib_native_linux);
    files.insert("/log.xml".into(), log_cfg);
    files.insert(format!("/{}/{}", &h1[..2], h1), a1);
    files.insert(format!("/{}/{}", &h2[..2], h2), a2);
    server.1.lock().unwrap().extend(files.clone());
    Fixture { server: server.0, files }
}

/// A server whose file map can be filled after its address is known.
fn serve_lazy() -> (TestServer, atlas_downloads::testing::FileMap) {
    atlas_downloads::testing::serve_dynamic(true)
}

fn svc(root: &std::path::Path, f: &Fixture, reuse: Vec<std::path::PathBuf>) -> VersionService {
    let mut s = VersionService::new(
        root.to_path_buf(),
        VersionSources { manifest_url: format!("{}/manifest.json", f.server.base), assets_base_url: f.server.base.clone() },
        reuse,
    );
    s.platform = Platform { os: "linux", arch: "x86_64" };
    s
}

fn dl(retries: u32) -> Downloader {
    Downloader::new(DownloaderConfig { retries, ..Default::default() }).unwrap()
}

async fn install(s: &VersionService, d: &Downloader) -> atlas_shared::AtlasResult<InstalledVersion> {
    let (m, _) = s.manifest(d, true, &CancellationToken::new()).await?;
    s.install(d, &m, "1.0-test", Arc::new(Progress::new()), CancellationToken::new()).await
}

#[tokio::test]
async fn full_install_verify_repair_remove() {
    let f = fixture("org/example/common/1.0/common-1.0.jar", false);
    let t = tempfile::tempdir().unwrap();
    let s = svc(t.path(), &f, vec![]);
    let d = dl(0);

    let iv = install(&s, &d).await.unwrap();
    assert_eq!(iv.id, "1.0-test");
    assert_eq!(iv.java_major, Some(21));
    let vd = t.path().join("versions/1.0-test");
    assert!(vd.join("install.toml").is_file());
    assert!(vd.join("1.0-test.jar").is_file());
    assert!(vd.join("1.0-test.json").is_file());
    assert!(!t.path().join("versions/.staging/1.0-test").exists(), "staging promoted");
    assert!(t.path().join("libraries/org/example/common/1.0/common-1.0.jar").is_file());
    assert!(t.path().join("libraries/org/example/nat/1.0/nat-1.0-natives-linux.jar").is_file(), "legacy natives for linux");
    assert!(!t.path().join("libraries/org/example/winonly").exists(), "windows-only lib skipped on linux");
    assert!(!t.path().join("libraries/org/example/demo.jar").exists(), "feature-gated lib skipped");
    assert!(t.path().join("assets/indexes/17.json").is_file());
    assert!(t.path().join("assets/log_configs/client-1.12.xml").is_file());
    assert_eq!(std::fs::read_dir(t.path().join("assets/objects")).unwrap().count(), 2, "duplicate asset stored once");
    assert_eq!(s.installed().unwrap().len(), 1);

    let rep = s.verify("1.0-test").unwrap();
    assert!(rep.ok(), "{rep:?}");
    // 1 index + jar + 2 libs + log cfg + 2 assets + json = 8
    assert_eq!(rep.checked, 8);

    // Corrupt one asset and delete the jar.
    let obj = std::fs::read_dir(t.path().join("assets/objects")).unwrap().next().unwrap().unwrap().path();
    let obj_file = std::fs::read_dir(&obj).unwrap().next().unwrap().unwrap().path();
    std::fs::write(&obj_file, b"corrupted").unwrap();
    std::fs::remove_file(vd.join("1.0-test.jar")).unwrap();
    let rep = s.verify("1.0-test").unwrap();
    assert_eq!((rep.missing.len(), rep.corrupt.len()), (1, 1));

    let rep = s.repair(&d, "1.0-test", Arc::new(Progress::new()), CancellationToken::new()).await.unwrap();
    assert!(rep.ok());
    assert_eq!(std::fs::read(vd.join("1.0-test.jar")).unwrap(), f.files["/client.jar"]);

    s.remove("1.0-test").unwrap();
    assert!(!vd.exists());
    assert!(t.path().join("libraries/org/example/common/1.0/common-1.0.jar").is_file(), "shared files kept");
    assert!(s.installed().unwrap().is_empty());
    assert_eq!(s.remove("1.0-test").unwrap_err().code, ErrorCode::VersionNotFound);
}

#[tokio::test]
async fn interrupted_install_is_not_installed_and_resumes() {
    let f = fixture("org/example/common/1.0/common-1.0.jar", false);
    f.server.cut_after.lock().unwrap().insert("/client.jar".into(), 50_000);
    let t = tempfile::tempdir().unwrap();
    let s = svc(t.path(), &f, vec![]);

    let e = install(&s, &dl(0)).await.unwrap_err();
    // A cut connection surfaces as a size/checksum mismatch or a network error.
    assert!(matches!(e.code, ErrorCode::ChecksumMismatch | ErrorCode::DownloadFailed), "{e:?}");
    assert!(!t.path().join("versions/1.0-test").exists(), "never half-installed");
    assert!(s.installed().unwrap().is_empty());
    assert!(t.path().join("versions/.staging/1.0-test/1.0-test.jar.part").exists(), "partial kept for resume");

    f.server.range_paths.lock().unwrap().clear();
    install(&s, &dl(0)).await.unwrap();
    // Other files cancelled mid-stream may resume too; the cut jar must.
    assert!(f.server.range_paths.lock().unwrap().iter().any(|p| p == "/client.jar"), "client jar resumed");
    assert!(s.verify("1.0-test").unwrap().ok());
}

#[tokio::test]
async fn tampered_version_metadata_is_rejected() {
    let f = fixture("org/example/common/1.0/common-1.0.jar", true);
    let t = tempfile::tempdir().unwrap();
    let s = svc(t.path(), &f, vec![]);
    let e = install(&s, &dl(0)).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ChecksumMismatch);
    assert!(s.installed().unwrap().is_empty());
}

#[tokio::test]
async fn malicious_library_path_is_rejected() {
    let f = fixture("../../../escape.jar", false);
    let t = tempfile::tempdir().unwrap();
    let s = svc(t.path(), &f, vec![]);
    let e = install(&s, &dl(0)).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PathTraversal);
    assert!(!t.path().parent().unwrap().join("escape.jar").exists());
}

#[tokio::test]
async fn reuses_files_from_existing_minecraft_folder() {
    let f = fixture("org/example/common/1.0/common-1.0.jar", false);
    let t = tempfile::tempdir().unwrap();
    let mc = t.path().join("dot-minecraft");
    let lib = mc.join("libraries/org/example/common/1.0/common-1.0.jar");
    std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
    std::fs::write(&lib, &f.files["/lib/common.jar"]).unwrap();
    let jar = mc.join("versions/1.0-test/1.0-test.jar");
    std::fs::create_dir_all(jar.parent().unwrap()).unwrap();
    std::fs::write(&jar, &f.files["/client.jar"]).unwrap();

    let s = svc(&t.path().join("atlas"), &f, vec![mc.clone()]);
    let d = dl(0);
    let (m, _) = s.manifest(&d, true, &CancellationToken::new()).await.unwrap();
    let p = Arc::new(Progress::new());
    s.install(&d, &m, "1.0-test", p.clone(), CancellationToken::new()).await.unwrap();
    let snap = p.snapshot();
    assert_eq!(snap.reused_files, 2);
    assert_eq!(snap.done_files, snap.total_files, "every planned file accounted for");
    assert_eq!(snap.done_bytes, snap.total_bytes, "progress ends exactly at 100%");
    assert_eq!(std::fs::read(&lib).unwrap(), f.files["/lib/common.jar"], "source untouched");
}

#[tokio::test]
async fn manifest_falls_back_to_cache_offline() {
    let f = fixture("org/example/common/1.0/common-1.0.jar", false);
    let t = tempfile::tempdir().unwrap();
    let s = svc(t.path(), &f, vec![]);
    let d = dl(0);
    let (m, stale) = s.manifest(&d, true, &CancellationToken::new()).await.unwrap();
    assert!(!stale);
    assert_eq!(m.latest.release, "1.0-test");

    let mut offline = svc(t.path(), &f, vec![]);
    offline.sources.manifest_url = "http://127.0.0.1:9/manifest.json".into();
    let (m2, stale) = offline.manifest(&d, true, &CancellationToken::new()).await.unwrap();
    assert!(stale);
    assert_eq!(m2.versions.len(), 2);
}

#[tokio::test]
async fn unknown_and_invalid_ids() {
    let f = fixture("org/example/common/1.0/common-1.0.jar", false);
    let t = tempfile::tempdir().unwrap();
    let s = svc(t.path(), &f, vec![]);
    let d = dl(0);
    let (m, _) = s.manifest(&d, true, &CancellationToken::new()).await.unwrap();
    let e = s.install(&d, &m, "9.9.9", Arc::new(Progress::new()), CancellationToken::new()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::VersionNotFound);
    let e = s.install(&d, &m, "../etc", Arc::new(Progress::new()), CancellationToken::new()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn update_detection() {
    let f = fixture("org/example/common/1.0/common-1.0.jar", false);
    let t = tempfile::tempdir().unwrap();
    let s = svc(t.path(), &f, vec![]);
    let d = dl(0);
    let (mut m, _) = s.manifest(&d, true, &CancellationToken::new()).await.unwrap();
    assert_eq!(s.update_available(&m).unwrap(), None, "nothing installed is not an update");
    s.install(&d, &m, "1.0-test", Arc::new(Progress::new()), CancellationToken::new()).await.unwrap();
    assert_eq!(s.update_available(&m).unwrap(), None, "latest installed");
    m.versions.insert(0, model::ManifestEntry { id: "1.1-test".into(), kind: VersionKind::Release, url: "x".into(), release_time: "2026-06-01T00:00:00+00:00".into(), sha1: "a".repeat(40) });
    m.latest.release = "1.1-test".into();
    assert_eq!(s.update_available(&m).unwrap(), Some("1.1-test".into()));
}

#[test]
fn rules_semantics() {
    use crate::rules::{allowed, Rule};
    let p = |os| Platform { os, arch: "x86_64" };
    let r: Vec<Rule> = serde_json::from_value(json!([{"action":"allow"},{"action":"disallow","os":{"name":"osx"}}])).unwrap();
    assert!(allowed(&r, p("linux")));
    assert!(!allowed(&r, p("osx")));
    let only_win: Vec<Rule> = serde_json::from_value(json!([{"action":"allow","os":{"name":"windows"}}])).unwrap();
    assert!(!allowed(&only_win, p("linux")));
    assert!(allowed(&only_win, p("windows")));
    assert!(allowed(&[], p("linux")));
}

#[test]
fn id_validation() {
    for ok in ["1.21.4", "24w14a", "rd-132211", "1.0", "b1.7.3", "1.14 Pre-Release 1"] {
        assert!(valid_id(ok), "{ok}");
    }
    for bad in ["", ".", "..", "../x", "a/b", "a\\b", ".hidden"] {
        assert!(!valid_id(bad), "{bad}");
    }
}

#[tokio::test]
async fn fabric_and_quilt_profiles_install_and_verify() {
    use crate::loaders::LoaderKind;
    let f = fixture("org/example/common/1.0/common-1.0.jar", false);
    let t = tempfile::tempdir().unwrap();
    let mut s = svc(t.path(), &f, vec![]);
    let d = dl(0);
    install(&s, &d).await.unwrap();

    let (srv, files) = atlas_downloads::testing::serve_dynamic(true);
    let loader_jar = b"fabric loader bytes".to_vec();
    let quilt_jar = b"quilt loader bytes".to_vec();
    {
        let mut m = files.lock().unwrap();
        m.insert("/fabric/versions/loader/1.0-test".into(), serde_json::to_vec(&json!([{"loader": {"version": "0.9.0", "stable": false}}, {"loader": {"version": "0.8.0", "stable": true}}])).unwrap());
        m.insert("/fabric/versions/loader/1.0-test/0.8.0/profile/json".into(), serde_json::to_vec(&json!({
            "id": "fabric-loader-0.8.0-1.0-test", "inheritsFrom": "1.0-test", "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "arguments": {"game": [], "jvm": []},
            "libraries": [{"name": "net.fabricmc:fabric-loader:0.8.0", "url": "https://placeholder/", "sha1": sha1_hex(&loader_jar), "size": loader_jar.len()}]
        })).unwrap());
        m.insert("/maven/net/fabricmc/fabric-loader/0.8.0/fabric-loader-0.8.0.jar".into(), loader_jar.clone());
        // quilt: no hash in the profile, the .sha1 sits next to the jar
        m.insert("/quilt/versions/loader/1.0-test/0.5.0/profile/json".into(), serde_json::to_vec(&json!({
            "id": "quilt-loader-0.5.0-1.0-test", "inheritsFrom": "1.0-test", "mainClass": "org.quiltmc.loader.impl.launch.knot.KnotClient",
            "libraries": [{"name": "org.quiltmc:quilt-loader:0.5.0", "url": "https://placeholder/"}]
        })).unwrap());
        m.insert("/maven/org/quiltmc/quilt-loader/0.5.0/quilt-loader-0.5.0.jar".into(), quilt_jar.clone());
        m.insert("/maven/org/quiltmc/quilt-loader/0.5.0/quilt-loader-0.5.0.jar.sha1".into(), format!("{}  quilt-loader-0.5.0.jar", sha1_hex(&quilt_jar)).into_bytes());
    }
    // profiles insist on https library repos; rewrite the placeholder to the local server
    let base = srv.base.clone();
    for k in ["/fabric/versions/loader/1.0-test/0.8.0/profile/json", "/quilt/versions/loader/1.0-test/0.5.0/profile/json"] {
        let mut m = files.lock().unwrap();
        let body = String::from_utf8(m[k].clone()).unwrap().replace("https://placeholder/", &format!("{base}/maven/"));
        m.insert(k.into(), body.into_bytes());
    }
    s.loader_sources.fabric = format!("{base}/fabric");
    s.loader_sources.quilt = format!("{base}/quilt");

    let c = CancellationToken::new();
    assert_eq!(s.pick_loader_version(&d, LoaderKind::Fabric, "1.0-test", None, &c).await.unwrap(), "0.8.0", "newest stable wins");
    // http library repos are refused outside tests, so this must fail loudly rather than download
    let e = s.install_loader(&d, LoaderKind::Fabric, "1.0-test", "0.8.0", Arc::new(Progress::new()), CancellationToken::new()).await.unwrap_err();
    assert!(e.details.contains("https"), "{e:?}");
}
