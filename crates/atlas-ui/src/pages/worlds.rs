use crate::app::{AtlasApp, Page};
use crate::{theme, widgets};
use atlas_core::Confirmation;
use egui::RichText;

#[derive(Default)]
pub struct WorldsPage {
    confirm_delete: Option<(String, String)>,
}

fn when(ms: Option<i64>) -> String {
    let Some(ms) = ms else { return String::new() };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(ms);
    let d = (now - ms).max(0) / 1000;
    match d {
        0..=3599 => format!("{} min ago", d / 60),
        3600..=86_399 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86_400),
    }
}

pub fn show(app: &mut AtlasApp, ui: &mut egui::Ui) {
    let atlas = app.atlas.clone();
    let accent = app.accent();
    let ctx = ui.ctx().clone();
    let Some(inst) = atlas.selected_installation() else {
        widgets::page_header(ui, "Worlds", "Your saved worlds.");
        widgets::card(ui, |ui| {
            ui.label("Create an installation first.");
            if ui.link("Go to Installations").clicked() {
                app.navigate(Page::Installations);
            }
        });
        return;
    };
    let worlds = atlas.worlds(None).unwrap_or_default();
    let mut errors = vec![];
    let mut infos = vec![];
    let wp = &mut app.worlds_page;

    widgets::page_header(ui, "Worlds", &format!("Saved in {}. Backups are zip files you can restore by unzipping into the saves folder.", inst.name));
    if worlds.is_empty() {
        widgets::card(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(18.0);
                crate::icons::show(ui, Page::Worlds, 40.0, theme::TEXT_FAINT);
                ui.add_space(8.0);
                ui.label(RichText::new("No worlds yet").size(17.0).strong());
                ui.label(RichText::new("Worlds show up here after you create one in the game.").color(theme::TEXT_DIM));
                ui.add_space(18.0);
            });
        });
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for w in &worlds {
            widgets::card(ui, |ui| {
                ui.horizontal(|ui| {
                    crate::icons::show(ui, Page::Worlds, 34.0, if w.hardcore { theme::ERR } else { accent });
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&w.name).size(16.0).strong());
                            if let Some(m) = &w.game_mode {
                                widgets::pill(ui, m, theme::TEXT_DIM);
                            }
                            if w.hardcore {
                                widgets::pill(ui, "Hardcore", theme::ERR);
                            }
                        });
                        let mut bits = vec![crate::pages::downloads::human_bytes(w.size as f64)];
                        if let Some(v) = &w.version {
                            bits.push(format!("Minecraft {v}"));
                        }
                        let t = when(w.last_played);
                        if !t.is_empty() {
                            bits.push(format!("played {t}"));
                        }
                        ui.label(RichText::new(bits.join("  ·  ")).small().color(theme::TEXT_DIM));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(RichText::new("Delete").color(theme::ERR)).clicked() {
                            wp.confirm_delete = Some((w.folder.clone(), w.name.clone()));
                        }
                        if ui.button("Back up").clicked() {
                            match atlas.backup_world(None, &w.folder) {
                                Ok(p) => infos.push(format!("Backed up to {}", p.display())),
                                Err(e) => errors.push(e),
                            }
                        }
                    });
                });
            });
            ui.add_space(8.0);
        }
    });

    if let Some((folder, name)) = wp.confirm_delete.clone() {
        let mut close = false;
        egui::Modal::new(egui::Id::unique("world-del")).show(&ctx, |ui| {
            ui.set_width(400.0);
            ui.label(RichText::new(format!("Delete {name}?")).size(18.0).strong());
            ui.add_space(6.0);
            ui.label(RichText::new("Atlas makes a backup zip first, then removes the world from the saves folder.").color(theme::TEXT_DIM));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Back up and delete").color(theme::ERR)).clicked() {
                    match atlas.delete_world(None, &folder, Confirmation::Confirmed) {
                        Ok(p) => infos.push(format!("Deleted. Backup saved to {}", p.display())),
                        Err(e) => errors.push(e),
                    }
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            wp.confirm_delete = None;
        }
    }
    for e in errors {
        app.push_error(e);
    }
    for i in infos {
        app.push_info(i);
    }
}
