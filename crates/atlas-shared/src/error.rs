//! Structured, user-presentable errors.
//!
//! Every failure that can reach a user carries:
//! * a stable machine-readable [`ErrorCode`] (serialised as `SCREAMING_SNAKE_CASE`),
//! * a human explanation,
//! * technical details (always passed through [`crate::redact::redact`]),
//! * a suggested solution.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable error codes. New variants may be added; existing names never change
/// meaning because the CLI's JSON output and MCP clients depend on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum ErrorCode {
    // Configuration / data directory
    ConfigInvalid,
    ConfigIo,
    DataDirInvalid,
    DataDirMoveFailed,
    // Filesystem safety
    PathTraversal,
    UnsafeSymlink,
    // Confirmation gate for destructive operations
    ConfirmationRequired,
    // Reserved for later phases (declared now so codes stay stable)
    AuthExpired,
    AuthFailed,
    VersionNotFound,
    DownloadFailed,
    ChecksumMismatch,
    PreparationFailed,
    GameCrashed,
    InvalidInstallation,
    UpdateVerificationFailed,
    // Generic
    InvalidArgument,
    NotImplemented,
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConfigInvalid => "CONFIG_INVALID",
            Self::ConfigIo => "CONFIG_IO",
            Self::DataDirInvalid => "DATA_DIR_INVALID",
            Self::DataDirMoveFailed => "DATA_DIR_MOVE_FAILED",
            Self::PathTraversal => "PATH_TRAVERSAL",
            Self::UnsafeSymlink => "UNSAFE_SYMLINK",
            Self::ConfirmationRequired => "CONFIRMATION_REQUIRED",
            Self::AuthExpired => "AUTH_EXPIRED",
            Self::AuthFailed => "AUTH_FAILED",
            Self::VersionNotFound => "VERSION_NOT_FOUND",
            Self::DownloadFailed => "DOWNLOAD_FAILED",
            Self::ChecksumMismatch => "CHECKSUM_MISMATCH",
            Self::PreparationFailed => "PREPARATION_FAILED",
            Self::GameCrashed => "GAME_CRASHED",
            Self::InvalidInstallation => "INVALID_INSTALLATION",
            Self::UpdateVerificationFailed => "UPDATE_VERIFICATION_FAILED",
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::NotImplemented => "NOT_IMPLEMENTED",
            Self::Internal => "INTERNAL",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The error type returned by every Atlas service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("[{code}] {message}")]
pub struct AtlasError {
    pub code: ErrorCode,
    /// Plain-language explanation for the user.
    pub message: String,
    /// Technical details (redacted). May be empty.
    pub details: String,
    /// What the user can do about it.
    pub suggestion: String,
}

pub type AtlasResult<T> = Result<T, AtlasError>;

impl AtlasError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: String::new(),
            suggestion: String::new(),
        }
    }

    /// Attach technical details. Details are redacted before being stored so
    /// tokens can never leak through error messages.
    pub fn details(mut self, details: impl fmt::Display) -> Self {
        self.details = crate::redact::redact(&details.to_string()).into_owned();
        self
    }

    pub fn suggest(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = suggestion.into();
        self
    }

    /// Convenience for wrapping an I/O error that happened on a path.
    pub fn io(code: ErrorCode, what: &str, path: &std::path::Path, err: std::io::Error) -> Self {
        Self::new(code, format!("Could not {what}."))
            .details(format!("{}: {err}", path.display()))
            .suggest("Check that the location exists, is writable, and is not used by another program.")
    }

    pub fn confirmation_required(action: &str) -> Self {
        Self::new(
            ErrorCode::ConfirmationRequired,
            format!("'{action}' is destructive and needs explicit confirmation."),
        )
        .suggest("Re-run with confirmation (CLI: --yes; GUI: confirm the dialog).")
    }

    pub fn not_implemented(feature: &str) -> Self {
        Self::new(ErrorCode::NotImplemented, format!("{feature} is not implemented yet."))
            .suggest("This feature is planned for a later Atlas release.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_serialises_as_screaming_snake() {
        let json = serde_json::to_string(&ErrorCode::ChecksumMismatch).unwrap();
        assert_eq!(json, "\"CHECKSUM_MISMATCH\"");
        // as_str and serde agree for every variant we can name here
        for c in [ErrorCode::AuthExpired, ErrorCode::DataDirMoveFailed, ErrorCode::NotImplemented] {
            assert_eq!(serde_json::to_string(&c).unwrap(), format!("\"{}\"", c.as_str()));
        }
    }

    #[test]
    fn details_are_redacted() {
        let e = AtlasError::new(ErrorCode::AuthFailed, "x")
            .details("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abcdefghijklmnop");
        assert!(!e.details.contains("eyJ"), "{}", e.details);
    }
}
