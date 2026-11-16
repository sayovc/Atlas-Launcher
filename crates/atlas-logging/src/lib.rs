//! Logging.
//!
//! Log files are JSON lines in `logs/launcher/`, one file per day, and
//! everything is redacted before it's written. The GUI's log viewer reads
//! from an in-memory ring buffer; the CLI reads the files back, since it
//! runs in a different process.

mod buffer;
mod record;

pub use buffer::{BufferLayer, LogBuffer};
pub use record::{LogFilter, LogLevel, LogRecord, LogSource};

use atlas_shared::redact::redact;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

pub const LOG_FILE_PREFIX: &str = "atlas.log";

#[derive(Debug, Clone)]
pub struct LogConfig {
    pub dir: PathBuf,
    /// tracing filter directive, e.g. `info` or `atlas_core=debug,info`.
    pub level: String,
    pub keep_files: usize,
    /// Also print human-readable logs to stderr.
    pub stderr: bool,
    /// Capacity of the in-memory ring buffer.
    pub buffer_capacity: usize,
}

/// Keeps the background log writer alive; drop at shutdown to flush.
pub struct LogHandle {
    pub buffer: LogBuffer,
    pub dir: PathBuf,
    _guard: tracing_appender::non_blocking::WorkerGuard,
}

/// Install the global subscriber. Call once per process.
pub fn init(cfg: &LogConfig) -> io::Result<LogHandle> {
    std::fs::create_dir_all(&cfg.dir)?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .max_log_files(cfg.keep_files.max(1))
        .build(&cfg.dir)
        .map_err(io::Error::other)?;
    let (nb, guard) = tracing_appender::non_blocking(RedactingWriter(appender));

    let filter = || EnvFilter::try_new(&cfg.level).unwrap_or_else(|_| EnvFilter::new("info"));
    let buffer = LogBuffer::new(cfg.buffer_capacity);

    let file_layer = fmt::layer()
        .json()
        .with_ansi(false)
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(nb)
        .with_filter(filter());
    let stderr_layer = cfg.stderr.then(|| {
        fmt::layer()
            .with_writer(|| RedactingWriter(io::stderr()))
            .with_target(false)
            .with_filter(filter())
    });
    let buffer_layer = BufferLayer::new(buffer.clone()).with_filter(filter());

    tracing_subscriber::registry()
        .with(file_layer)
        .with(stderr_layer)
        .with(buffer_layer)
        .try_init()
        .map_err(io::Error::other)?;

    Ok(LogHandle { buffer, dir: cfg.dir.clone(), _guard: guard })
}

/// A writer that redacts credentials from every chunk before forwarding.
/// tracing's fmt layer writes one full event per `write` call, so patterns
/// are never split across chunks.
pub struct RedactingWriter<W: Write>(pub W);

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        let clean = redact(&text);
        self.0.write_all(clean.as_bytes())?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// Parse every Atlas log file in `dir` (oldest first), keeping records that
/// match `filter`, at most `limit` most-recent ones.
pub fn read_log_dir(dir: &Path, filter: &LogFilter, limit: usize) -> io::Result<Vec<LogRecord>> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(LOG_FILE_PREFIX)))
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    files.sort(); // date suffix sorts chronologically
    let mut out = std::collections::VecDeque::new();
    for f in files {
        let text = std::fs::read_to_string(&f)?;
        for line in text.lines() {
            if let Some(rec) = LogRecord::from_json_line(line, LogSource::Launcher)
                && filter.matches(&rec)
            {
                out.push_back(rec);
                if out.len() > limit {
                    out.pop_front();
                }
            }
        }
    }
    Ok(out.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacting_writer_scrubs() {
        let mut out = Vec::new();
        RedactingWriter(&mut out).write_all(br#"{"access_token":"supersecret"}"#).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(!s.contains("supersecret"));
    }

    #[test]
    fn reads_json_lines_with_filter_and_limit() {
        let t = tempfile::tempdir().unwrap();
        let lines = [
            r#"{"timestamp":"2026-11-16T10:00:00Z","level":"INFO","fields":{"message":"started"},"target":"atlas_core"}"#,
            r#"{"timestamp":"2026-11-16T10:00:01Z","level":"WARN","fields":{"message":"slow disk","ms":900},"target":"atlas_core"}"#,
            "not json",
            r#"{"timestamp":"2026-11-16T10:00:02Z","level":"ERROR","fields":{"message":"boom"},"target":"atlas_ui"}"#,
        ];
        std::fs::write(t.path().join("atlas.log.2026-11-16"), lines.join("\n")).unwrap();
        std::fs::write(t.path().join("unrelated.txt"), "x").unwrap();

        let all = read_log_dir(t.path(), &LogFilter::default(), 100).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[1].message, "slow disk ms=900");

        let warn = LogFilter { min_level: LogLevel::Warn, ..Default::default() };
        assert_eq!(read_log_dir(t.path(), &warn, 100).unwrap().len(), 2);

        let search = LogFilter { search: "BOOM".into(), ..Default::default() };
        assert_eq!(read_log_dir(t.path(), &search, 100).unwrap()[0].level, LogLevel::Error);

        let last = read_log_dir(t.path(), &LogFilter::default(), 1).unwrap();
        assert_eq!(last[0].message, "boom");
    }
}
