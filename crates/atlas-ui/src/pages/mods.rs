use crate::app::{AtlasApp, Page};
use crate::{theme, widgets};
use atlas_core::{Confirmation, PackKind};
use egui::RichText;

#[derive(Default, PartialEq, Clone, Copy)]
enum Tab {
    #[default]
    Mods,
    Resource,
    Shader,
}

#[derive(Default)]
pub struct ModsPage {
    tab: Tab,
    search: String,
    confirm: Option<(Tab, String)>,
}

fn human(b: u64) -> String {
    crate::pages::downloads::human_bytes(b as f64)
}

pub fn show(app: &mut AtlasApp, ui: &mut egui::Ui) {
    let atlas = app.atlas.clone();
    let accent = app.accent();
    let ctx = ui.ctx().clone();
    let Some(inst) = atlas.selected_installation() else {
        widgets::page_header(ui, "Mods", "Mods, resource packs and shader packs for an installation.");
        widgets::card(ui, |ui| {
            ui.label("Create an installation first.");
            if ui.link("Go to Installations").clicked() {
                app.navigate(Page::Installations);
            }
        });
        return;
    };
    let mut errors = vec![];
    let mut infos = vec![];
    let mp = &mut app.mods_page;

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("Mods & packs").heading().strong());
            ui.label(RichText::new(format!("For {} · Minecraft {}", inst.name, inst.version)).color(theme::TEXT_DIM));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = match mp.tab {
                Tab::Mods => "Add mod",
                _ => "Add pack",
            };
            if widgets::primary_button(ui, label, accent, true, egui::vec2(120.0, 38.0)).clicked() {
                let filter = if mp.tab == Tab::Mods { ("Mods", &["jar"][..]) } else { ("Packs", &["zip"][..]) };
                if let Some(f) = rfd::FileDialog::new().add_filter(filter.0, filter.1).pick_file() {
                    let r = match mp.tab {
                        Tab::Mods => atlas.add_mod(None, &f),
                        Tab::Resource => atlas.add_pack(None, PackKind::Resource, &f),
                        Tab::Shader => atlas.add_pack(None, PackKind::Shader, &f),
                    };
                    match r {
                        Ok(n) => infos.push(format!("Added {n}.")),
                        Err(e) => errors.push(e),
                    }
                }
            }
        });
    });
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        for (t, l) in [(Tab::Mods, "Mods"), (Tab::Resource, "Resource packs"), (Tab::Shader, "Shader packs")] {
            let sel = mp.tab == t;
            let b = egui::Button::new(RichText::new(l).color(if sel { theme::TEXT } else { theme::TEXT_DIM }))
                .fill(if sel { theme::SURFACE_HI } else { egui::Color32::TRANSPARENT })
                .stroke(if sel { egui::Stroke::new(1.0, accent.gamma_multiply(0.6)) } else { egui::Stroke::NONE });
            if ui.add(b).clicked() {
                mp.tab = t;
            }
        }
        ui.add_space(10.0);
        ui.add(egui::TextEdit::singleline(&mut mp.search).hint_text("Filter…").desired_width(180.0));
    });
    ui.add_space(8.0);
    let needle = mp.search.to_lowercase();

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match mp.tab {
        Tab::Mods => {
            let mods = atlas.mods(None).unwrap_or_default();
            // honest note: we don't install loaders, so mods only load if one is already there
            egui::Frame::new().fill(theme::WARN.gamma_multiply(0.07)).stroke(egui::Stroke::new(1.0, theme::WARN.gamma_multiply(0.35))).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Atlas doesn't install Fabric, Forge or NeoForge yet. Mods here only load if this installation already has a loader.").small().color(theme::TEXT_DIM));
            });
            ui.add_space(8.0);
            if mods.is_empty() {
                widgets::card(ui, |ui| {
                    ui.label(RichText::new("No mods. Drop .jar files in with Add mod.").color(theme::TEXT_DIM));
                });
            }
            for m in mods.iter().filter(|m| needle.is_empty() || m.name.to_lowercase().contains(&needle) || m.file.to_lowercase().contains(&needle)) {
                widgets::card(ui, |ui| {
                    ui.horizontal(|ui| {
                        let mut on = m.enabled;
                        if ui.add(egui::Checkbox::without_text(&mut on)).on_hover_text(if m.enabled { "Disable" } else { "Enable" }).changed()
                            && let Err(e) = atlas.set_mod_enabled(None, &m.file, on)
                        {
                            errors.push(e);
                        }
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&m.name).strong().color(if m.enabled { theme::TEXT } else { theme::TEXT_FAINT }));
                                if let Some(v) = &m.version {
                                    ui.label(RichText::new(v).small().color(theme::TEXT_FAINT));
                                }
                                widgets::pill(ui, m.loader.label(), theme::TEXT_DIM);
                                if let Some(mc) = &m.minecraft {
                                    widgets::pill(ui, &format!("MC {mc}"), theme::TEXT_DIM);
                                }
                            });
                            if let Some(d) = &m.description {
                                ui.add(egui::Label::new(RichText::new(d).small().color(theme::TEXT_DIM)).truncate());
                            }
                            if !m.missing.is_empty() {
                                ui.label(RichText::new(format!("Needs: {}", m.missing.join(", "))).small().color(theme::WARN));
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(RichText::new("Delete").color(theme::ERR)).clicked() {
                                mp.confirm = Some((Tab::Mods, m.file.clone()));
                            }
                            ui.label(RichText::new(human(m.size)).small().color(theme::TEXT_FAINT));
                        });
                    });
                });
                ui.add_space(6.0);
            }
        }
        tab => {
            let kind = if tab == Tab::Resource { PackKind::Resource } else { PackKind::Shader };
            let packs = atlas.packs(None, kind).unwrap_or_default();
            if packs.is_empty() {
                widgets::card(ui, |ui| {
                    ui.label(RichText::new("Nothing here yet. Add a .zip pack to get started.").color(theme::TEXT_DIM));
                });
            }
            for p in packs.iter().filter(|p| needle.is_empty() || p.file.to_lowercase().contains(&needle)) {
                widgets::card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&p.file).strong());
                            if let Some(d) = &p.description {
                                ui.add(egui::Label::new(RichText::new(d).small().color(theme::TEXT_DIM)).truncate());
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(RichText::new("Delete").color(theme::ERR)).clicked() {
                                mp.confirm = Some((tab, p.file.clone()));
                            }
                            ui.label(RichText::new(human(p.size)).small().color(theme::TEXT_FAINT));
                            if let Some(f) = p.pack_format {
                                widgets::pill(ui, &format!("format {f}"), theme::TEXT_DIM);
                            }
                        });
                    });
                });
                ui.add_space(6.0);
            }
            if tab == Tab::Resource {
                ui.label(RichText::new("Turn packs on and off in the game's Options → Resource Packs.").small().color(theme::TEXT_FAINT));
            }
        }
    });

    if let Some((tab, file)) = mp.confirm.clone() {
        let mut close = false;
        egui::Modal::new(egui::Id::unique("mods-del")).show(&ctx, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new(format!("Delete {file}?")).size(17.0).strong());
            ui.add_space(6.0);
            ui.label(RichText::new("The file is removed from this installation's folder.").color(theme::TEXT_DIM));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Delete").color(theme::ERR)).clicked() {
                    let r = match tab {
                        Tab::Mods => atlas.remove_mod(None, &file, Confirmation::Confirmed),
                        Tab::Resource => atlas.remove_pack(None, PackKind::Resource, &file, Confirmation::Confirmed),
                        Tab::Shader => atlas.remove_pack(None, PackKind::Shader, &file, Confirmation::Confirmed),
                    };
                    if let Err(e) = r {
                        errors.push(e);
                    }
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            mp.confirm = None;
        }
    }
    for e in errors {
        app.push_error(e);
    }
    for i in infos {
        app.push_info(i);
    }
}
