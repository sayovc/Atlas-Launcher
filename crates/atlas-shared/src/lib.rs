//! Bits every other crate needs: the error type, log redaction and
//! some careful filesystem helpers.

pub mod error;
pub mod fs_safety;
pub mod redact;

pub use error::{AtlasError, AtlasResult, ErrorCode};

/// Product name shown to users.
pub const PRODUCT_NAME: &str = "Atlas Launcher";
/// Version of the Atlas launcher itself (not of any game build).
pub const ATLAS_VERSION: &str = env!("CARGO_PKG_VERSION");
