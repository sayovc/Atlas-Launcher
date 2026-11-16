use super::*;
use std::collections::HashMap;
use std::sync::atomic::Ordering;

use crate::testing::{serve, TestServer};

fn blob(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 31 % 251) as u8).collect()
}

fn req(server: &TestServer, path: &str, dest: &Path, data: &[u8]) -> DownloadRequest {
    DownloadRequest {
        url: format!("{}{path}", server.base),
        dest: dest.to_path_buf(),
        checksum: Checksum::Sha1(sha1_hex(data)),
        size: Some(data.len() as u64),
        reuse_from: vec![],
    }
}

fn dl(retries: u32) -> Downloader {
    Downloader::new(DownloaderConfig { retries, ..Default::default() }).unwrap()
}

#[tokio::test]
async fn downloads_and_verifies() {
    let data = blob(300_000);
    let s = serve(HashMap::from([("/a".into(), data.clone())]), true);
    let t = tempfile::tempdir().unwrap();
    let dest = t.path().join("x/a.bin");
    let p = Progress::new();
    let o = dl(0).download(&req(&s, "/a", &dest, &data), &p, &CancellationToken::new()).await.unwrap();
    assert_eq!(o, Outcome::Downloaded);
    assert_eq!(std::fs::read(&dest).unwrap(), data);
    assert!(!part_path(&dest).exists());
    assert_eq!(p.snapshot().done_bytes, data.len() as u64);

    // Second time: no network.
    let before = s.hits.load(Ordering::SeqCst);
    let o = dl(0).download(&req(&s, "/a", &dest, &data), &p, &CancellationToken::new()).await.unwrap();
    assert_eq!(o, Outcome::AlreadyValid);
    assert_eq!(s.hits.load(Ordering::SeqCst), before);
}

#[tokio::test]
async fn checksum_mismatch_leaves_no_file() {
    let data = blob(10_000);
    let s = serve(HashMap::from([("/a".into(), data.clone())]), true);
    let t = tempfile::tempdir().unwrap();
    let dest = t.path().join("a.bin");
    let mut r = req(&s, "/a", &dest, &data);
    r.checksum = Checksum::Sha1("0".repeat(40));
    let e = dl(1).download(&r, &Progress::new(), &CancellationToken::new()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ChecksumMismatch);
    assert!(!dest.exists());
    assert!(!part_path(&dest).exists());
}

#[tokio::test]
async fn oversized_response_rejected() {
    let data = blob(10_000);
    let s = serve(HashMap::from([("/a".into(), data.clone())]), true);
    let t = tempfile::tempdir().unwrap();
    let dest = t.path().join("a.bin");
    let mut r = req(&s, "/a", &dest, &data);
    r.size = Some(5_000);
    let e = dl(0).download(&r, &Progress::new(), &CancellationToken::new()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ChecksumMismatch);
    assert!(!dest.exists());
}

#[tokio::test]
async fn interrupted_download_resumes_with_range() {
    let data = blob(500_000);
    let s = serve(HashMap::from([("/a".into(), data.clone())]), true);
    s.cut_after.lock().unwrap().insert("/a".into(), 200_000);
    let t = tempfile::tempdir().unwrap();
    let dest = t.path().join("a.bin");
    let o = dl(2).download(&req(&s, "/a", &dest, &data), &Progress::new(), &CancellationToken::new()).await.unwrap();
    assert_eq!(o, Outcome::Downloaded);
    assert_eq!(std::fs::read(&dest).unwrap(), data);
    assert_eq!(s.range_hits.load(Ordering::SeqCst), 1, "second attempt must resume");
}

#[tokio::test]
async fn server_without_range_restarts_cleanly() {
    let data = blob(200_000);
    let s = serve(HashMap::from([("/a".into(), data.clone())]), false);
    let t = tempfile::tempdir().unwrap();
    let dest = t.path().join("a.bin");
    // Simulate a stale partial file with wrong bytes.
    std::fs::write(part_path(&dest), vec![0xFFu8; 50_000]).unwrap();
    dl(0).download(&req(&s, "/a", &dest, &data), &Progress::new(), &CancellationToken::new()).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), data);
}

#[tokio::test]
async fn corrupt_existing_file_is_replaced() {
    let data = blob(50_000);
    let s = serve(HashMap::from([("/a".into(), data.clone())]), true);
    let t = tempfile::tempdir().unwrap();
    let dest = t.path().join("a.bin");
    std::fs::write(&dest, b"garbage").unwrap();
    let o = dl(0).download(&req(&s, "/a", &dest, &data), &Progress::new(), &CancellationToken::new()).await.unwrap();
    assert_eq!(o, Outcome::Downloaded);
    assert_eq!(std::fs::read(&dest).unwrap(), data);
}

#[tokio::test]
async fn reuses_matching_local_file_without_network() {
    let data = blob(40_000);
    let s = serve(HashMap::new(), true); // nothing served
    let t = tempfile::tempdir().unwrap();
    let other = t.path().join("dot-minecraft/obj");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(&other, &data).unwrap();
    let wrong = t.path().join("wrong");
    std::fs::write(&wrong, b"nope").unwrap();
    let dest = t.path().join("atlas/obj");
    let mut r = req(&s, "/missing", &dest, &data);
    r.reuse_from = vec![wrong, other.clone()];
    let o = dl(0).download(&r, &Progress::new(), &CancellationToken::new()).await.unwrap();
    assert_eq!(o, Outcome::Reused);
    assert_eq!(std::fs::read(&dest).unwrap(), data);
    assert_eq!(std::fs::read(&other).unwrap(), data, "source untouched");
    assert_eq!(s.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancellation_stops_and_reports() {
    let data = blob(100_000);
    let s = serve(HashMap::from([("/a".into(), data.clone())]), true);
    let t = tempfile::tempdir().unwrap();
    let c = CancellationToken::new();
    c.cancel();
    let e = dl(3).download(&req(&s, "/a", &t.path().join("a"), &data), &Progress::new(), &c).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled);
}

#[tokio::test]
async fn download_all_reports_first_real_error() {
    let good = blob(20_000);
    let s = serve(HashMap::from([("/g".into(), good.clone())]), true);
    let t = tempfile::tempdir().unwrap();
    let reqs = vec![
        req(&s, "/g", &t.path().join("g"), &good),
        DownloadRequest { url: format!("{}/404", s.base), dest: t.path().join("m"), checksum: Checksum::Sha1("a".repeat(40)), size: None, reuse_from: vec![] },
    ];
    let e = dl(0).download_all(reqs, Arc::new(Progress::new()), CancellationToken::new()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::DownloadFailed);
}

#[tokio::test]
async fn download_all_counts_outcomes() {
    let a = blob(10_000);
    let b = blob(20_000);
    let s = serve(HashMap::from([("/a".into(), a.clone()), ("/b".into(), b.clone())]), true);
    let t = tempfile::tempdir().unwrap();
    let p = Arc::new(Progress::new());
    let outs = dl(0)
        .download_all(vec![req(&s, "/a", &t.path().join("a"), &a), req(&s, "/b", &t.path().join("b"), &b)], p.clone(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(outs.len(), 2);
    let snap = p.snapshot();
    assert_eq!((snap.total_files, snap.done_files, snap.downloaded_files), (2, 2, 2));
    assert_eq!(snap.done_bytes, 30_000);
    assert!((snap.fraction() - 1.0).abs() < f32::EPSILON);
}

#[test]
fn verify_rejects_symlinks() {
    #[cfg(unix)]
    {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        std::fs::write(&real, b"x").unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let c = Checksum::Sha1(sha1_hex(b"x"));
        assert!(verify_file(&real, &c, Some(1)).unwrap());
        assert!(!verify_file(&link, &c, Some(1)).unwrap());
    }
}
