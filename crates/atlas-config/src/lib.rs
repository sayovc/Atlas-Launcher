//! Atlas configuration: platform directories and strongly typed settings.
//!
//! Layout
//! ------
//! * **Bootstrap dir** (fixed, platform config dir): holds only `locator.toml`,
//!   which records where the data root lives. This is what makes the data
//!   root relocatable.
//! * **Data root** (default: platform data dir, user-movable): `settings.toml`,
//!   installations, versions, downloads cache, logs, account metadata.
//!
//! Atlas never stores anything inside another launcher's directory.

pub mod paths;
pub mod settings;

pub use paths::{AtlasPaths, Locator};
pub use settings::{LoadOutcome, Settings, SettingsStore};
