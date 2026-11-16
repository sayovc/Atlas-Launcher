//! Verified, resumable, cancellable downloads.
//!
//! Guarantees:
//! * A file only ever appears at its destination after its checksum (and
//!   size, when known) has been verified. Data is streamed into
//!   `<dest>.part` and atomically renamed.
//! * Interrupted downloads resume from `.part` with an HTTP `Range` request
//!   when the server supports it; otherwise they restart cleanly.
//! * Existing valid files are never downloaded again.
//! * Local candidate files (for example from an existing `.minecraft`) are
//!   reused only if their hash matches; they are copied, never moved or modified.

mod limiter;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod progress;

pub use progress::{Progress, ProgressSnapshot};
pub use tokio_util::sync::CancellationToken;

use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use futures_util::StreamExt;
use limiter::RateLimiter;
use serde::{Deserialize, Serialize};
use sha1::Digest as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// Expected digest of a file, lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "algo", content = "hex", rename_all = "lowercase")]
pub enum Checksum {
    Sha1(String),
    Sha256(String),
}

impl Checksum {
    pub fn hex(&self) -> &str {
        match self {
            Self::Sha1(h) | Self::Sha256(h) => h,
        }
    }
    fn hasher(&self) -> Hasher {
        match self {
            Self::Sha1(_) => Hasher::Sha1(sha1::Sha1::new()),
            Self::Sha256(_) => Hasher::Sha256(sha2::Sha256::new()),
        }
    }
}

enum Hasher {
    Sha1(sha1::Sha1),
    Sha256(sha2::Sha256),
}

impl Hasher {
    fn update(&mut self, b: &[u8]) {
        match self {
            Self::Sha1(h) => h.update(b),
            Self::Sha256(h) => h.update(b),
        }
    }
    fn finish(self) -> String {
        match self {
            Self::Sha1(h) => hex::encode(h.finalize()),
            Self::Sha256(h) => hex::encode(h.finalize()),
        }
    }
}

/// One file to obtain.
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub url: String,
    pub dest: PathBuf,
    pub checksum: Checksum,
    pub size: Option<u64>,
    /// Local files that may already contain the right bytes.
    pub reuse_from: Vec<PathBuf>,
}

/// How a file was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    AlreadyValid,
    Reused,
    Downloaded,
}

#[derive(Debug, Clone)]
pub struct DownloaderConfig {
    pub max_concurrent: usize,
    /// Bytes per second; `None` = unlimited.
    pub rate_limit: Option<u64>,
    pub retries: u32,
    pub user_agent: String,
}

impl Default for DownloaderConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 8,
            rate_limit: None,
            retries: 3,
            user_agent: format!("AtlasLauncher/{}", atlas_shared::ATLAS_VERSION),
        }
    }
}

/// Upper bound for metadata documents fetched into memory.
const MAX_METADATA_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct Downloader {
    client: reqwest::Client,
    cfg: DownloaderConfig,
    limiter: Option<Arc<RateLimiter>>,
}

fn net_err(url: &str, e: impl std::fmt::Display) -> AtlasError {
    AtlasError::new(ErrorCode::DownloadFailed, "A download failed.")
        .details(format!("{url}: {e}"))
        .suggest("Check your internet connection and try again. Finished files are kept, and partial ones resume.")
}

fn io_err(what: &str, path: &Path, e: std::io::Error) -> AtlasError {
    AtlasError::io(ErrorCode::DownloadFailed, what, path, e)
}

impl Downloader {
    pub fn new(cfg: DownloaderConfig) -> AtlasResult<Self> {
        let client = reqwest::Client::builder()
            .user_agent(&cfg.user_agent)
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60))
            .https_only(false) // tests use a local http server; production URLs are https
            .build()
            .map_err(|e| AtlasError::new(ErrorCode::Internal, "Could not start the network client.").details(e))?;
        let limiter = cfg.rate_limit.filter(|r| *r > 0).map(|r| Arc::new(RateLimiter::new(r)));
        Ok(Self { client, cfg, limiter })
    }

    /// Fetch a small trusted-source document (e.g. a manifest) into memory.
    /// Callers verify a checksum themselves when one is available.
    pub async fn fetch_bytes(&self, url: &str, cancel: &CancellationToken) -> AtlasResult<Vec<u8>> {
        let mut attempt = 0;
        loop {
            let result = async {
                let resp = self.client.get(url).send().await.map_err(|e| net_err(url, e))?;
                if !resp.status().is_success() {
                    return Err(net_err(url, format!("HTTP {}", resp.status())));
                }
                let mut out = Vec::new();
                let mut stream = resp.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|e| net_err(url, e))?;
                    if out.len() + chunk.len() > MAX_METADATA_BYTES {
                        return Err(net_err(url, "document is unreasonably large"));
                    }
                    out.extend_from_slice(&chunk);
                }
                Ok(out)
            };
            let r = tokio::select! {
                _ = cancel.cancelled() => return Err(AtlasError::cancelled()),
                r = result => r,
            };
            match r {
                Ok(b) => return Ok(b),
                Err(e) if attempt < self.cfg.retries => {
                    attempt += 1;
                    tracing::warn!(log_source = "download", url, attempt, "retrying: {}", e.details);
                    backoff(attempt, cancel).await?;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Obtain one file. See the crate docs for guarantees.
    pub async fn download(&self, req: &DownloadRequest, progress: &Progress, cancel: &CancellationToken) -> AtlasResult<Outcome> {
        // 1. Already present and valid?
        if verify_file_async(req.dest.clone(), req.checksum.clone(), req.size).await? {
            progress.add_done_bytes(req.size.unwrap_or(0));
            return Ok(Outcome::AlreadyValid);
        }
        // 2. Reusable local copy?
        for cand in &req.reuse_from {
            if verify_file_async(cand.clone(), req.checksum.clone(), req.size).await? {
                let part = part_path(&req.dest);
                ensure_parent(&req.dest)?;
                tokio::fs::copy(cand, &part).await.map_err(|e| io_err("copy a reusable file", cand, e))?;
                if verify_file_async(part.clone(), req.checksum.clone(), req.size).await? {
                    tokio::fs::rename(&part, &req.dest).await.map_err(|e| io_err("place a file", &req.dest, e))?;
                    progress.add_done_bytes(req.size.unwrap_or(0));
                    tracing::debug!(log_source = "download", from = %cand.display(), "reused verified local file");
                    return Ok(Outcome::Reused);
                }
                let _ = tokio::fs::remove_file(&part).await;
            }
        }
        // 3. Download with retries.
        let mut attempt = 0;
        loop {
            match self.try_download(req, progress, cancel).await {
                Ok(()) => return Ok(Outcome::Downloaded),
                Err(e) if e.code == ErrorCode::Cancelled => return Err(e),
                Err(e) if attempt < self.cfg.retries => {
                    attempt += 1;
                    tracing::warn!(log_source = "download", url = %req.url, attempt, code = %e.code, "retrying: {}", e.details);
                    backoff(attempt, cancel).await?;
                }
                Err(e) => {
                    tracing::error!(log_source = "download", url = %req.url, code = %e.code, "{}", e.details);
                    return Err(e);
                }
            }
        }
    }

    async fn try_download(&self, req: &DownloadRequest, progress: &Progress, cancel: &CancellationToken) -> AtlasResult<()> {
        let part = part_path(&req.dest);
        ensure_parent(&req.dest)?;
        let mut hasher = req.checksum.hasher();
        let mut have: u64 = 0;

        // Resume: hash what we already have.
        if let Ok(meta) = std::fs::metadata(&part) {
            let len = meta.len();
            if len > 0 && req.size.is_none_or(|s| len < s) {
                let p = part.clone();
                let (h, n) = tokio::task::spawn_blocking(move || -> std::io::Result<(Hasher, u64)> {
                    let mut f = std::fs::File::open(&p)?;
                    let mut buf = vec![0u8; 256 * 1024];
                    let mut n = 0u64;
                    loop {
                        let r = f.read(&mut buf)?;
                        if r == 0 {
                            break;
                        }
                        hasher.update(&buf[..r]);
                        n += r as u64;
                    }
                    Ok((hasher, n))
                })
                .await
                .map_err(|e| AtlasError::new(ErrorCode::Internal, "Hashing task failed.").details(e))?
                .map_err(|e| io_err("read a partial download", &part, e))?;
                hasher = h;
                have = n;
            } else {
                let _ = std::fs::remove_file(&part);
            }
        }

        let mut rq = self.client.get(&req.url);
        if have > 0 {
            rq = rq.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let resp = tokio::select! {
            _ = cancel.cancelled() => return Err(AtlasError::cancelled()),
            r = rq.send() => r.map_err(|e| net_err(&req.url, e))?,
        };
        let status = resp.status();
        let mut file = if have > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
            tracing::debug!(log_source = "download", url = %req.url, from = have, "resuming");
            progress.add_done_bytes(have);
            tokio::fs::OpenOptions::new().append(true).open(&part).await.map_err(|e| io_err("open a partial download", &part, e))?
        } else if status.is_success() {
            // Server ignored Range (or fresh start): restart from zero.
            hasher = req.checksum.hasher();
            have = 0;
            tokio::fs::File::create(&part).await.map_err(|e| io_err("create a download file", &part, e))?
        } else if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(net_err(&req.url, "server rejected resume; restarting"));
        } else {
            return Err(net_err(&req.url, format!("HTTP {status}")));
        };

        let mut written = have;
        let mut added_this_attempt = 0u64;
        let mut stream = resp.bytes_stream();
        let result: AtlasResult<()> = async {
            loop {
                let next = tokio::select! {
                    _ = cancel.cancelled() => return Err(AtlasError::cancelled()),
                    n = stream.next() => n,
                };
                let Some(chunk) = next else { break };
                let chunk = chunk.map_err(|e| net_err(&req.url, e))?;
                written += chunk.len() as u64;
                if req.size.is_some_and(|s| written > s) {
                    return Err(checksum_err(&req.url, "server sent more data than expected"));
                }
                if let Some(l) = &self.limiter {
                    l.acquire(chunk.len() as u64, cancel).await?;
                }
                hasher.update(&chunk);
                file.write_all(&chunk).await.map_err(|e| io_err("write a download", &part, e))?;
                progress.add_done_bytes(chunk.len() as u64);
                added_this_attempt += chunk.len() as u64;
            }
            file.flush().await.map_err(|e| io_err("write a download", &part, e))?;
            file.sync_all().await.map_err(|e| io_err("write a download", &part, e))?;
            Ok(())
        }
        .await;
        drop(file);
        if let Err(e) = result {
            // Keep .part for resume unless the data is known-bad.
            if e.code == ErrorCode::ChecksumMismatch {
                let _ = tokio::fs::remove_file(&part).await;
            }
            progress.sub_done_bytes(added_this_attempt + if status == reqwest::StatusCode::PARTIAL_CONTENT { have } else { 0 });
            return Err(e);
        }

        let actual = hasher.finish();
        let size_ok = req.size.is_none_or(|s| s == written);
        if actual != req.checksum.hex().to_ascii_lowercase() || !size_ok {
            let _ = tokio::fs::remove_file(&part).await;
            progress.sub_done_bytes(written);
            return Err(checksum_err(
                &req.url,
                format!("expected {} ({} bytes), got {actual} ({written} bytes)", req.checksum.hex(), req.size.map(|s| s.to_string()).unwrap_or("?".into())),
            ));
        }
        tokio::fs::rename(&part, &req.dest).await.map_err(|e| io_err("place a downloaded file", &req.dest, e))?;
        Ok(())
    }

    /// Obtain many files concurrently. Stops scheduling new work after the
    /// first error and cancels the rest; returns that first error.
    pub async fn download_all(&self, reqs: Vec<DownloadRequest>, progress: Arc<Progress>, cancel: CancellationToken) -> AtlasResult<Vec<Outcome>> {
        progress.add_total_files(reqs.len() as u64);
        progress.add_total_bytes(reqs.iter().filter_map(|r| r.size).sum());
        let inner_cancel = cancel.child_token();
        let n = self.cfg.max_concurrent.max(1);
        let results: Vec<AtlasResult<Outcome>> = futures_util::stream::iter(reqs.into_iter().map(|r| {
            let this = self.clone();
            let progress = progress.clone();
            let c = inner_cancel.clone();
            async move {
                let out = this.download(&r, &progress, &c).await;
                match &out {
                    Ok(o) => progress.file_done(*o),
                    Err(e) if e.code != ErrorCode::Cancelled => c.cancel(),
                    Err(_) => {}
                }
                out
            }
        }))
        .buffer_unordered(n)
        .collect()
        .await;
        if cancel.is_cancelled() {
            return Err(AtlasError::cancelled());
        }
        // Prefer a real error over the cancellations it caused.
        let mut first_cancel = None;
        let mut outs = Vec::with_capacity(results.len());
        for r in results {
            match r {
                Ok(o) => outs.push(o),
                Err(e) if e.code == ErrorCode::Cancelled => first_cancel = first_cancel.or(Some(e)),
                Err(e) => return Err(e),
            }
        }
        if let Some(e) = first_cancel {
            return Err(e);
        }
        Ok(outs)
    }
}

fn checksum_err(url: &str, d: impl std::fmt::Display) -> AtlasError {
    AtlasError::new(ErrorCode::ChecksumMismatch, "A downloaded file did not match its expected checksum.")
        .details(format!("{url}: {d}"))
        .suggest("The file was discarded. Try again; if it keeps happening, your connection or a proxy may be altering downloads.")
}

async fn backoff(attempt: u32, cancel: &CancellationToken) -> AtlasResult<()> {
    let d = Duration::from_millis(400 * 2u64.pow(attempt.min(5)));
    tokio::select! {
        _ = cancel.cancelled() => Err(AtlasError::cancelled()),
        _ = tokio::time::sleep(d) => Ok(()),
    }
}

pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

fn ensure_parent(p: &Path) -> AtlasResult<()> {
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io_err("create a folder", dir, e))?;
    }
    Ok(())
}

/// Hash a file on disk and compare. Missing file = `Ok(false)`. Symlinks are
/// never trusted (`Ok(false)`).
pub fn verify_file(path: &Path, checksum: &Checksum, size: Option<u64>) -> AtlasResult<bool> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(io_err("read a file", path, e)),
    };
    if !meta.is_file() || size.is_some_and(|s| s != meta.len()) {
        return Ok(false);
    }
    let mut f = std::fs::File::open(path).map_err(|e| io_err("read a file", path, e))?;
    let mut h = checksum.hasher();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| io_err("read a file", path, e))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finish() == checksum.hex().to_ascii_lowercase())
}

pub async fn verify_file_async(path: PathBuf, checksum: Checksum, size: Option<u64>) -> AtlasResult<bool> {
    tokio::task::spawn_blocking(move || verify_file(&path, &checksum, size))
        .await
        .map_err(|e| AtlasError::new(ErrorCode::Internal, "Verification task failed.").details(e))?
}

/// Hash bytes in memory (used for manifests whose checksum is known).
pub fn sha1_hex(bytes: &[u8]) -> String {
    hex::encode(sha1::Sha1::digest(bytes))
}

#[cfg(test)]
mod tests;
