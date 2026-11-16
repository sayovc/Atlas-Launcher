#![allow(clippy::unwrap_used)]
use super::*;
use atlas_downloads::testing::serve_dynamic;
use ed25519_dalek::{Signer, SigningKey};
use sha2_shim::sha256_hex;

mod sha2_shim {
    // the downloads crate already depends on sha2; reuse it through a tiny helper
    pub fn sha256_hex(b: &[u8]) -> String {
        use std::fmt::Write;
        let d = <sha2::Sha256 as sha2::Digest>::digest(b);
        d.iter().fold(String::new(), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
    }
}

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn sign(k: &SigningKey, body: &[u8]) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.encode(k.sign(body).to_bytes()).into_bytes()
}

#[test]
fn version_ordering() {
    assert!(newer("0.2.0", "0.1.9"));
    assert!(newer("1.10.0", "1.9.0"));
    assert!(newer("v1.0.0", "1.0.0-beta.2"));
    assert!(!newer("0.1.0", "0.1.0"));
    assert!(!newer("0.0.9", "0.1.0"), "never downgrade");
    assert!(!newer("1.0.0-rc.1", "1.0.0"));
}

#[test]
fn signature_must_match_key_and_bytes() {
    let k = key();
    let vk = k.verifying_key();
    let body = br#"{"version":"0.2.0","assets":{}}"#;
    assert!(verify_manifest(&vk, body, &sign(&k, body)).is_ok());
    // tampered body
    let evil = br#"{"version":"9.9.9","assets":{}}"#;
    assert_eq!(verify_manifest(&vk, evil, &sign(&k, body)).unwrap_err().code, ErrorCode::UpdateVerificationFailed);
    // signed by someone else
    let other = SigningKey::from_bytes(&[9u8; 32]);
    assert!(verify_manifest(&vk, body, &sign(&other, body)).is_err());
    assert!(verify_manifest(&vk, body, b"not base64!").is_err());
}

#[tokio::test]
async fn check_download_apply_rollback() {
    let k = key();
    let (srv, files) = serve_dynamic(true);
    let new_bin = b"#!/bin/sh\necho new atlas\n".to_vec();
    // check() insists on https asset URLs, so download() is tested with a local URL separately
    let manifest = serde_json::to_vec(&serde_json::json!({
        "version": "0.9.0", "notes": "faster", "assets": {platform_key(): {"url": "https://example.invalid/atlas", "sha256": sha256_hex(&new_bin), "size": new_bin.len()}}
    })).unwrap();
    files.lock().unwrap().insert("/atlas-update.json".into(), manifest.clone());
    files.lock().unwrap().insert("/atlas-update.json.sig".into(), sign(&k, &manifest));
    files.lock().unwrap().insert("/bin".into(), new_bin.clone());

    let t = tempfile::tempdir().unwrap();
    let up = Updater { dl: Downloader::new(Default::default()).unwrap(), key: k.verifying_key(), manifest_url: format!("{}/atlas-update.json", srv.base), dir: t.path().join("updates") };
    let c = CancellationToken::new();
    let a = up.check("0.1.0", &c).await.unwrap().expect("update available");
    assert_eq!(a.version, "0.9.0");
    assert!(up.check("0.9.0", &c).await.unwrap().is_none(), "same version is not an update");

    // download with the local URL; a wrong hash must be refused
    let mut local = a.clone();
    local.asset.url = format!("{}/bin", srv.base);
    let staged = up.download(&local, Arc::new(Progress::new()), CancellationToken::new()).await.unwrap();
    assert_eq!(std::fs::read(&staged).unwrap(), new_bin);
    let mut bad = local.clone();
    bad.version = "0.9.1".into();
    bad.asset.sha256 = "0".repeat(64);
    assert_eq!(up.download(&bad, Arc::new(Progress::new()), CancellationToken::new()).await.unwrap_err().code, ErrorCode::ChecksumMismatch);

    // apply onto a stand-in "installed" binary, then simulate three failed starts
    let target = t.path().join("installed-atlas");
    std::fs::write(&target, b"old atlas").unwrap();
    apply(&up.dir, &staged, &target, "0.9.0").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), new_bin);
    assert_eq!(startup_check(&up.dir, &target), StartupOutcome::TryingUpdate("0.9.0".into()));
    assert_eq!(startup_check(&up.dir, &target), StartupOutcome::TryingUpdate("0.9.0".into()));
    assert_eq!(startup_check(&up.dir, &target), StartupOutcome::RolledBack("0.9.0".into()));
    assert_eq!(std::fs::read(&target).unwrap(), b"old atlas");
    assert_eq!(startup_check(&up.dir, &target), StartupOutcome::Normal);

    // a healthy start clears the marker
    apply(&up.dir, &staged, &target, "0.9.0").unwrap();
    assert_eq!(startup_check(&up.dir, &target), StartupOutcome::TryingUpdate("0.9.0".into()));
    mark_healthy(&up.dir);
    assert_eq!(startup_check(&up.dir, &target), StartupOutcome::Normal);
}

#[tokio::test]
async fn unsigned_or_wrongly_signed_manifest_is_refused() {
    let (srv, files) = serve_dynamic(true);
    let manifest = br#"{"version":"5.0.0","assets":{}}"#.to_vec();
    files.lock().unwrap().insert("/m.json".into(), manifest.clone());
    files.lock().unwrap().insert("/m.json.sig".into(), sign(&SigningKey::from_bytes(&[1u8; 32]), &manifest));
    let t = tempfile::tempdir().unwrap();
    let up = Updater { dl: Downloader::new(Default::default()).unwrap(), key: key().verifying_key(), manifest_url: format!("{}/m.json", srv.base), dir: t.path().into() };
    assert_eq!(up.check("0.1.0", &CancellationToken::new()).await.unwrap_err().code, ErrorCode::UpdateVerificationFailed);
}
