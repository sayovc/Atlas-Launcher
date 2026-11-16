//! Tiny HTTP/1.1 server for tests (supports Range and forced disconnects).
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Minimal HTTP/1.1 test server.
pub struct TestServer {
    pub base: String,
    pub hits: Arc<AtomicUsize>,
    pub range_hits: Arc<AtomicUsize>,
    /// Paths that were served as a resumed (206) response.
    pub range_paths: Arc<Mutex<Vec<String>>>,
    /// When set, the next response for that path is cut after N bytes.
    pub cut_after: Arc<Mutex<HashMap<String, usize>>>,
}

pub fn serve(files: HashMap<String, Vec<u8>>, honour_range: bool) -> TestServer {
    let (srv, map) = serve_dynamic(honour_range);
    map.lock().unwrap().extend(files);
    srv
}

/// Like [`serve`], but returns the file map so it can be filled after the
/// base URL is known (for documents that embed their own URLs).
/// Shared, mutable path → body map.
pub type FileMap = Arc<Mutex<HashMap<String, Vec<u8>>>>;

pub fn serve_dynamic(honour_range: bool) -> (TestServer, FileMap) {
    let files: FileMap = Arc::default();
    let files_out = files.clone();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let range_hits = Arc::new(AtomicUsize::new(0));
    let range_paths: Arc<Mutex<Vec<String>>> = Arc::default();
    let cut_after: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
    let (h, rh, cut, rp) = (hits.clone(), range_hits.clone(), cut_after.clone(), range_paths.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (files, h, rh, cut, rp) = (files.clone(), h.clone(), rh.clone(), cut.clone(), rp.clone());
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() { return; }
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut range_from: Option<usize> = None;
                loop {
                    let mut l = String::new();
                    if reader.read_line(&mut l).unwrap_or(0) == 0 || l == "\r\n" { break; }
                    let lower = l.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("range: bytes=") {
                        range_from = v.trim().trim_end_matches('-').parse().ok();
                    }
                }
                h.fetch_add(1, Ordering::SeqCst);
                let body = files.lock().unwrap().get(&path).cloned();
                let Some(body) = body else {
                    let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    return;
                };
                let (status, slice) = match range_from {
                    Some(from) if honour_range && from < body.len() => {
                        rh.fetch_add(1, Ordering::SeqCst);
                        rp.lock().unwrap().push(path.clone());
                        ("206 Partial Content", &body[from..])
                    }
                    _ => ("200 OK", &body[..]),
                };
                let limit = cut.lock().unwrap().remove(&path);
                let header = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", slice.len());
                let _ = stream.write_all(header.as_bytes());
                let send = limit.map(|n| n.min(slice.len())).unwrap_or(slice.len());
                let _ = stream.write_all(&slice[..send]);
                let _ = stream.flush();
            });
        }
    });
    (TestServer { base, hits, range_hits, range_paths, cut_after }, files_out)
}

