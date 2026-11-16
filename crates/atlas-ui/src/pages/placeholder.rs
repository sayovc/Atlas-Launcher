//! Pages whose backend services are not built yet. They say so plainly
//! instead of showing fake data.

use crate::app::{AtlasApp, Page};
use crate::{theme, widgets};
use egui::RichText;

fn plan(page: Page) -> (&'static str, &'static str, &'static [&'static str]) {
    match page {
        Page::Installations => ("Phase 3", "Independent installations, each with its own game folder, version, memory and launch arguments.",
            &["Create, rename, duplicate, repair, update", "Delete only Atlas-owned folders, with confirmation", "Per-installation Java and memory settings"]),
        Page::Mods => ("Phase 4", "Manage mods and resource packs per installation.",
            &["Enable / disable local mod files", "Compatibility and dependency info", "Modpack and resource pack import"]),
        Page::Servers => ("Phase 4", "Your server list, connected through the selected installation.",
            &["Add, edit, remove, favourite", "Version and last-played info", "One-click connect"]),
        Page::Worlds => ("Phase 4", "Worlds stored in your installations, with safe backups.",
            &["List and back up worlds", "Never deleted as a side effect"]),
        _ => ("Later", "", &[]),
    }
}

pub fn show(app: &mut AtlasApp, ui: &mut egui::Ui, page: Page) {
    let (phase, blurb, bullets) = plan(page);
    widgets::page_header(ui, page.label(), blurb);
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            crate::icons::show(ui, page, 32.0, app.accent());
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Not built yet").size(17.0).strong());
                    widgets::pill(ui, phase, theme::WARN);
                });
                ui.label(RichText::new("This section has no backend yet, so it shows nothing rather than placeholder data.").color(theme::TEXT_DIM));
            });
        });
        ui.add_space(10.0);
        widgets::section_title(ui, "Planned");
        for b in bullets {
            ui.label(RichText::new(format!("•  {b}")).color(theme::TEXT_DIM));
        }
    });
}
