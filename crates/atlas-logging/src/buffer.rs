use crate::record::{LogFilter, LogLevel, LogRecord, LogSource};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::Context;

/// Bounded, thread-safe ring buffer of recent log records.
#[derive(Clone)]
pub struct LogBuffer {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    cap: usize,
    records: VecDeque<LogRecord>,
    /// Monotonic counter so UIs can cheaply detect new records.
    generation: u64,
}

impl LogBuffer {
    pub fn new(cap: usize) -> Self {
        Self { inner: Arc::new(Mutex::new(Inner { cap: cap.max(1), records: VecDeque::new(), generation: 0 })) }
    }

    pub fn push(&self, rec: LogRecord) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if g.records.len() == g.cap {
            g.records.pop_front();
        }
        g.records.push_back(rec);
        g.generation += 1;
    }

    pub fn generation(&self) -> u64 {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).generation
    }

    pub fn snapshot(&self, filter: &LogFilter) -> Vec<LogRecord> {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.records.iter().filter(|r| filter.matches(r)).cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// tracing layer that copies every event into a [`LogBuffer`].
pub struct BufferLayer {
    buffer: LogBuffer,
}

impl BufferLayer {
    pub fn new(buffer: LogBuffer) -> Self {
        Self { buffer }
    }
}

#[derive(Default)]
struct MsgVisitor {
    message: String,
    fields: String,
    source: Option<LogSource>,
}

impl Visit for MsgVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message.push_str(value),
            "log_source" => self.source = serde_json::from_value(serde_json::Value::String(value.into())).ok(),
            n => {
                let _ = write!(self.fields, " {n}={value}");
            }
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "message" => {
                let _ = write!(self.message, "{value:?}");
            }
            "log_source" => {
                let s = format!("{value:?}");
                self.source = serde_json::from_value(serde_json::Value::String(s.trim_matches('"').into())).ok();
            }
            n => {
                let _ = write!(self.fields, " {n}={value:?}");
            }
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for BufferLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut v = MsgVisitor::default();
        event.record(&mut v);
        let meta = event.metadata();
        let ts = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        let msg = format!("{}{}", v.message, v.fields);
        self.buffer.push(LogRecord {
            timestamp: ts,
            level: LogLevel::from(meta.level()),
            source: v.source.unwrap_or(LogSource::Launcher),
            target: meta.target().to_string(),
            message: atlas_shared::redact::redact(&msg).into_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn layer_captures_and_redacts() {
        let buf = LogBuffer::new(3);
        let sub = tracing_subscriber::registry().with(BufferLayer::new(buf.clone()));
        tracing::subscriber::with_default(sub, || {
            tracing::info!(files = 3, "installed");
            tracing::warn!(log_source = "download", "retrying");
            tracing::error!("token leak? access_token=abcdef123456");
            tracing::info!("fourth");
        });
        let all = buf.snapshot(&LogFilter::default());
        assert_eq!(all.len(), 3, "ring buffer capped at 3");
        assert_eq!(all[0].source, LogSource::Download);
        assert!(!all[1].message.contains("abcdef123456"), "{}", all[1].message);
        assert_eq!(buf.generation(), 4);
    }
}
