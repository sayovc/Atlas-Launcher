//! Turning an installed version + an installation into a running game.
//!
//! `command` builds the java command line (pure, easy to test), `natives`
//! unpacks old-style native jars, `process` runs the game and watches it.

pub mod command;
pub mod natives;
pub mod process;

pub use command::{build_command, LaunchAuth, LaunchCommand, LaunchSpec};
pub use process::{GameExit, GameProcess};
