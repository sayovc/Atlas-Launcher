use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "trace" => Self::Trace,
            "debug" => Self::Debug,
            "info" => Self::Info,
            "warn" | "warning" => Self::Warn,
            "error" => Self::Error,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
    pub const ALL: [LogLevel; 5] = [Self::Trace, Self::Debug, Self::Info, Self::Warn, Self::Error];
}

impl From<&tracing::Level> for LogLevel {
    fn from(l: &tracing::Level) -> Self {
        match *l {
            tracing::Level::TRACE => Self::Trace,
            tracing::Level::DEBUG => Self::Debug,
            tracing::Level::INFO => Self::Info,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::ERROR => Self::Error,
        }
    }
}

/// Where a record came from. Game / crash / install sources are populated in
/// later phases by the launch and install services.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogSource {
    Launcher,
    Game,
    Crash,
    Install,
    Download,
}

impl LogSource {
    pub const ALL: [LogSource; 5] = [Self::Launcher, Self::Game, Self::Crash, Self::Install, Self::Download];
    pub fn label(self) -> &'static str {
        match self {
            Self::Launcher => "Launcher",
            Self::Game => "Game",
            Self::Crash => "Crash",
            Self::Install => "Install",
            Self::Download => "Download",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRecord {
    /// RFC 3339 UTC timestamp.
    pub timestamp: String,
    pub level: LogLevel,
    pub source: LogSource,
    pub target: String,
    /// Message plus `key=value` fields, already redacted.
    pub message: String,
}

impl LogRecord {
    /// Parse a line written by tracing-subscriber's JSON formatter.
    pub fn from_json_line(line: &str, source: LogSource) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        let fields = v.get("fields")?.as_object()?;
        let mut message = fields.get("message").and_then(|m| m.as_str()).unwrap_or_default().to_string();
        let src = fields
            .get("log_source")
            .and_then(|s| serde_json::from_value::<LogSource>(s.clone()).ok())
            .unwrap_or(source);
        for (k, val) in fields {
            if k == "message" || k == "log_source" {
                continue;
            }
            let val = val.as_str().map(str::to_owned).unwrap_or_else(|| val.to_string());
            message.push_str(&format!(" {k}={val}"));
        }
        Some(Self {
            timestamp: v.get("timestamp")?.as_str()?.to_string(),
            level: LogLevel::parse(v.get("level")?.as_str()?)?,
            source: src,
            target: v.get("target").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
            message: atlas_shared::redact::redact(&message).into_owned(),
        })
    }

    /// One-line plain-text form, used for copy-to-clipboard and CLI output.
    pub fn to_line(&self) -> String {
        format!("{} {:5} [{}] {}: {}", self.timestamp, self.level.as_str(), self.source.label(), self.target, self.message)
    }
}

#[derive(Debug, Clone, Default)]
pub struct LogFilter {
    pub min_level: LogLevel,
    /// Case-insensitive substring over message and target. Empty = all.
    pub search: String,
    /// `None` = all sources.
    pub source: Option<LogSource>,
}

impl LogFilter {
    pub fn matches(&self, r: &LogRecord) -> bool {
        if r.level < self.min_level {
            return false;
        }
        if self.source.is_some_and(|s| s != r.source) {
            return false;
        }
        if self.search.is_empty() {
            return true;
        }
        let needle = self.search.to_lowercase();
        r.message.to_lowercase().contains(&needle) || r.target.to_lowercase().contains(&needle)
    }
}
