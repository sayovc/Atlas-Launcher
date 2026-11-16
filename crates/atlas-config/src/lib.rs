//! Where Atlas keeps things, and the settings file.
//!
//! There are two folders. The bootstrap folder (in the platform config
//! dir) only holds `locator.toml`, which says where the data folder is.
//! The data folder holds everything else and can be moved. That split is
//! the whole trick behind "change data folder".
//!
//! We never write into another launcher's folders.

pub mod paths;
pub mod settings;

pub use paths::{AtlasPaths, Locator};
pub use settings::{LoadOutcome, Settings, SettingsStore};
