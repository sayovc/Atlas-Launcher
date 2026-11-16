use crate::Outcome;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Lock-free progress counters shared between workers and front-ends.
#[derive(Debug, Default)]
pub struct Progress {
    total_bytes: AtomicU64,
    done_bytes: AtomicU64,
    total_files: AtomicU64,
    done_files: AtomicU64,
    downloaded_files: AtomicU64,
    reused_files: AtomicU64,
    valid_files: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct ProgressSnapshot {
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub total_files: u64,
    pub done_files: u64,
    pub downloaded_files: u64,
    pub reused_files: u64,
    pub already_valid_files: u64,
}

impl ProgressSnapshot {
    /// 0.0–1.0, by bytes when known, else by files.
    pub fn fraction(&self) -> f32 {
        if self.total_bytes > 0 {
            (self.done_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0) as f32
        } else if self.total_files > 0 {
            (self.done_files as f64 / self.total_files as f64) as f32
        } else {
            0.0
        }
    }
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }
    pub(crate) fn add_total_bytes(&self, n: u64) {
        self.total_bytes.fetch_add(n, Relaxed);
    }
    pub(crate) fn add_total_files(&self, n: u64) {
        self.total_files.fetch_add(n, Relaxed);
    }
    pub(crate) fn add_done_bytes(&self, n: u64) {
        self.done_bytes.fetch_add(n, Relaxed);
    }
    pub(crate) fn sub_done_bytes(&self, n: u64) {
        // CAS loop instead of fetch_update/try_update: works on old and new toolchains.
        let mut cur = self.done_bytes.load(Relaxed);
        while let Err(actual) = self.done_bytes.compare_exchange_weak(cur, cur.saturating_sub(n), Relaxed, Relaxed) {
            cur = actual;
        }
    }
    /// Announce work that will be done outside [`crate::Downloader::download_all`].
    pub fn plan(&self, files: u64, bytes: u64) {
        self.add_total_files(files);
        self.add_total_bytes(bytes);
    }
    pub fn file_done(&self, o: Outcome) {
        self.done_files.fetch_add(1, Relaxed);
        match o {
            Outcome::Downloaded => &self.downloaded_files,
            Outcome::Reused => &self.reused_files,
            Outcome::AlreadyValid => &self.valid_files,
        }
        .fetch_add(1, Relaxed);
    }
    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            total_bytes: self.total_bytes.load(Relaxed),
            done_bytes: self.done_bytes.load(Relaxed),
            total_files: self.total_files.load(Relaxed),
            done_files: self.done_files.load(Relaxed),
            downloaded_files: self.downloaded_files.load(Relaxed),
            reused_files: self.reused_files.load(Relaxed),
            already_valid_files: self.valid_files.load(Relaxed),
        }
    }
}
