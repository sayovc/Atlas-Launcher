//! Atlas Launcher desktop application.
//!
//! The UI owns no business logic: it renders `atlas_core::Atlas` state and
//! forwards user intent to it.

#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod app;
mod icons;
mod pages;
mod theme;
mod widgets;

use atlas_core::Atlas;
use std::sync::Arc;

fn main() -> eframe::Result<()> {
    let mut atlas = match Atlas::open_default() {
        Ok(a) => a,
        Err(e) => {
            // Logging is not up yet; this is the only place stderr is used directly.
            eprintln!("Atlas could not start: [{}] {}\n  {}\n  {}", e.code, e.message, e.details, e.suggestion);
            std::process::exit(1);
        }
    };
    let log_handle = atlas.init_logging(cfg!(debug_assertions)).ok();
    let atlas = Arc::new(atlas);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Atlas Launcher")
            .with_app_id("atlaslauncher")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([900.0, 600.0])
            .with_icon(Arc::new(theme::app_icon(64))),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Atlas Launcher",
        options,
        Box::new(move |cc| Ok(Box::new(app::AtlasApp::new(cc, atlas)))),
    );
    tracing::info!("Atlas window closed");
    drop(log_handle);
    result
}
