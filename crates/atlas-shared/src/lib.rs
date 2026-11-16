//! Shared primitives used by every Atlas crate.
//!
//! * [`error`] – the structured [`AtlasError`] type (code, explanation, details, suggestion).
//! * [`redact`] – credential scrubbing applied to every log line and error detail.
//! * [`fs_safety`] – path-traversal and symlink-safe filesystem helpers.

pub mod error;
pub mod fs_safety;
pub mod redact;

pub use error::{AtlasError, AtlasResult, ErrorCode};

/// Product name shown to users.
pub const PRODUCT_NAME: &str = "Atlas Launcher";
/// Version of the Atlas launcher itself (not of any game build).
pub const ATLAS_VERSION: &str = env!("CARGO_PKG_VERSION");
