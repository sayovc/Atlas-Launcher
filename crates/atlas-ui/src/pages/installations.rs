use crate::app::{AtlasApp, Page};
use crate::{theme, widgets};
use atlas_core::{Confirmation, GameState, Installation, InstallationChanges};
use egui::RichText;

#[derive(Default)]
pub struct InstallationsPage {
    new_open: bool,
    new_name: String,
    new_version: String,
    new_dir: String,
    edit: Option<EditForm>,
    confirm_delete: Option<Installation>,
}

struct EditForm {
    id: String,
    name: String,
    version: String,
    mem_override: bool,
    min: u32,
    max: u32,
    jvm: String,
    java: String,
    res_override: bool,
    w: u32,
    h: u32,
}

impl EditForm {
    fn from(i: &Installation, defaults: &atlas_config::Settings) -> Self {
        Self {
            id: i.id.clone(),
            name: i.name.clone(),
            version: i.version.clone(),
            mem_override: i.memory_max_mb.is_some(),
            min: i.memory_min_mb.unwrap_or(defaults.memory.min_mb),
            max: i.memory_max_mb.unwrap_or(defaults.memory.max_mb),
            jvm: i.jvm_args.join(" "),
            java: i.java_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            res_override: i.width.is_some(),
            w: i.width.unwrap_or(defaults.game.width),
            h: i.height.unwrap_or(defaults.game.height),
        }
    }
}

/// A little procedural "block" icon so every installation gets its own look.
fn paint_block(ui: &mut egui::Ui, seed: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let h = seed.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let hue = (h % 360) as f32 / 360.0;
    let base = egui::ecolor::Hsva::new(hue, 0.45, 0.55, 1.0);
    let p = ui.painter();
    p.rect_filled(rect, 8.0, theme::SURFACE_HI);
    let inner = rect.shrink(size * 0.18);
    let cells = 4;
    let cs = inner.width() / cells as f32;
    for y in 0..cells {
        for x in 0..cells {
            let v = (h.rotate_left((x * 4 + y) as u32) % 7) as f32 / 7.0;
            let c = egui::ecolor::Hsva::new(hue, 0.45, 0.35 + 0.35 * v, 1.0);
            let r = egui::Rect::from_min_size(inner.min + egui::vec2(x as f32 * cs, y as f32 * cs), egui::vec2(cs, cs));
            p.rect_filled(r.shrink(0.5), 1.5, egui::Color32::from(c));
        }
    }
    p.rect_stroke(rect, 8.0, egui::Stroke::new(1.0, egui::Color32::from(base).gamma_multiply(0.6)), egui::StrokeKind::Inside);
}

fn ago(t: Option<u64>) -> String {
    let Some(t) = t else { return "never played".into() };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(t);
    let d = now.saturating_sub(t);
    match d {
        0..=59 => "played just now".into(),
        60..=3599 => format!("played {} min ago", d / 60),
        3600..=86_399 => format!("played {} h ago", d / 3600),
        _ => format!("played {} days ago", d / 86_400),
    }
}

pub fn show(app: &mut AtlasApp, ui: &mut egui::Ui) {
    let atlas = app.atlas.clone();
    let accent = app.accent();
    let ctx = ui.ctx().clone();
    let list = atlas.installations().unwrap_or_default();
    let selected = atlas.selected_installation().map(|i| i.id);
    let state = atlas.game_state();
    let installed: Vec<String> = atlas.installed_versions().unwrap_or_default().into_iter().map(|v| v.id).collect();
    let mut errors = vec![];
    let mut infos = vec![];
    let mut goto_home = false;

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("Installations").heading().strong());
            ui.label(RichText::new("Each one has its own version, game folder and settings.").color(theme::TEXT_DIM));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::primary_button(ui, "New installation", accent, true, egui::vec2(170.0, 38.0)).clicked() {
                let ip = &mut app.installations_page;
                ip.new_open = true;
                ip.new_name = format!("Installation {}", list.len() + 1);
                ip.new_version = "latest".into();
                ip.new_dir.clear();
            }
        });
    });
    ui.add_space(14.0);

    if list.is_empty() {
        widgets::card(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                crate::icons::show(ui, Page::Installations, 40.0, theme::TEXT_FAINT);
                ui.add_space(8.0);
                ui.label(RichText::new("No installations yet").size(17.0).strong());
                ui.label(RichText::new("Create one to pick a version and start playing.").color(theme::TEXT_DIM));
                ui.add_space(20.0);
            });
        });
    }

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let ip = &mut app.installations_page;
        for inst in &list {
            let is_sel = selected.as_deref() == Some(inst.id.as_str());
            egui::Frame::new()
                .fill(theme::SURFACE)
                .stroke(egui::Stroke::new(1.0, if is_sel { accent.gamma_multiply(0.6) } else { theme::BORDER }))
                .corner_radius(theme::RADIUS)
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        paint_block(ui, &inst.id, 52.0);
                        ui.add_space(6.0);
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&inst.name).size(17.0).strong());
                                if is_sel {
                                    widgets::pill(ui, "Selected", accent);
                                }
                                if inst.uses_external_dir() {
                                    widgets::pill(ui, "Own folder", theme::TEXT_DIM);
                                }
                            });
                            let ver = if installed.contains(&inst.version) { format!("Minecraft {}", inst.version) } else { format!("Minecraft {} · not installed yet", inst.version) };
                            ui.label(RichText::new(format!("{ver}  ·  {}", ago(inst.last_played))).color(theme::TEXT_DIM));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.menu_button("•••", |ui| {
                                if ui.button("Edit").clicked() {
                                    ip.edit = Some(EditForm::from(inst, &atlas.settings()));
                                    ui.close();
                                }
                                if ui.button("Duplicate").clicked() {
                                    match atlas.duplicate_installation(&inst.id) {
                                        Ok(i) => infos.push(format!("Created {}.", i.name)),
                                        Err(e) => errors.push(e),
                                    }
                                    ui.close();
                                }
                                if ui.button(RichText::new("Delete").color(theme::ERR)).clicked() {
                                    ip.confirm_delete = Some(inst.clone());
                                    ui.close();
                                }
                            });
                            let busy = !matches!(state, GameState::Ready | GameState::NotInstalled | GameState::NoInstallation | GameState::Error { .. });
                            if ui.add_enabled(!busy, egui::Button::new(RichText::new("Play").strong().color(accent)).stroke(egui::Stroke::new(1.0, accent.gamma_multiply(0.6)))).clicked() {
                                atlas.dismiss_game_error();
                                let _ = atlas.select_installation(&inst.id);
                                match atlas.play(Some(&inst.id), Default::default()) {
                                    Ok(()) => goto_home = true,
                                    Err(e) => errors.push(e),
                                }
                            }
                            if !is_sel
                                && ui.button("Select").clicked()
                                && let Err(e) = atlas.select_installation(&inst.id)
                            {
                                errors.push(e);
                            }
                        });
                    });
                });
            ui.add_space(8.0);
        }
    });

    // New installation dialog
    let ip = &mut app.installations_page;
    if ip.new_open {
        let mut close = false;
        egui::Modal::new(egui::Id::unique("new-inst")).show(&ctx, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new("New installation").size(18.0).strong());
            ui.add_space(8.0);
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut ip.new_name).desired_width(f32::INFINITY));
            ui.add_space(4.0);
            ui.label("Version");
            egui::ComboBox::from_id_salt("new-ver").width(420.0).selected_text(if ip.new_version == "latest" { "Latest release".to_string() } else { ip.new_version.clone() }).show_ui(ui, |ui| {
                ui.selectable_value(&mut ip.new_version, "latest".into(), "Latest release");
                ui.selectable_value(&mut ip.new_version, "latest-snapshot".into(), "Latest snapshot");
                for v in &installed {
                    ui.selectable_value(&mut ip.new_version, v.clone(), format!("{v} (installed)"));
                }
            });
            ui.label(RichText::new("Or type any version id:").small().color(theme::TEXT_FAINT));
            ui.add(egui::TextEdit::singleline(&mut ip.new_version).desired_width(f32::INFINITY));
            ui.add_space(4.0);
            ui.label("Game folder (optional)");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut ip.new_dir).hint_text("Managed by Atlas").desired_width(320.0));
                if ui.button("Browse…").clicked()
                    && let Some(f) = rfd::FileDialog::new().pick_folder()
                {
                    ip.new_dir = f.display().to_string();
                }
            });
            ui.label(RichText::new("Pick a folder to use an existing game folder. Atlas will never delete it.").small().color(theme::TEXT_FAINT));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, "Create", accent, !ip.new_name.trim().is_empty(), egui::vec2(100.0, 34.0)).clicked() {
                    let dir = (!ip.new_dir.trim().is_empty()).then(|| std::path::PathBuf::from(ip.new_dir.trim()));
                    match atlas.create_installation(&ip.new_name, ip.new_version.trim(), dir) {
                        Ok(i) => {
                            infos.push(format!("Created {}.", i.name));
                            close = true;
                        }
                        Err(e) => errors.push(e),
                    }
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            ip.new_open = false;
        }
    }

    // Edit dialog
    let mut close_edit = false;
    if let Some(f) = &mut ip.edit {
        egui::Modal::new(egui::Id::unique("edit-inst")).show(&ctx, |ui| {
            ui.set_width(460.0);
            ui.label(RichText::new("Edit installation").size(18.0).strong());
            ui.add_space(8.0);
            egui::Grid::new("edit-grid").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut f.name).desired_width(280.0));
                ui.end_row();
                ui.label("Version");
                ui.add(egui::TextEdit::singleline(&mut f.version).desired_width(280.0));
                ui.end_row();
                ui.label("Memory");
                ui.vertical(|ui| {
                    ui.checkbox(&mut f.mem_override, "Use custom memory");
                    ui.add_enabled_ui(f.mem_override, |ui| {
                        ui.add(egui::Slider::new(&mut f.min, 512..=32768).step_by(256.0).text("min MB"));
                        ui.add(egui::Slider::new(&mut f.max, 512..=65536).step_by(256.0).text("max MB"));
                    });
                });
                ui.end_row();
                ui.label("Window");
                ui.vertical(|ui| {
                    ui.checkbox(&mut f.res_override, "Custom size");
                    ui.add_enabled_ui(f.res_override, |ui| {
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut f.w).range(320..=7680));
                            ui.label("×");
                            ui.add(egui::DragValue::new(&mut f.h).range(240..=4320));
                        });
                    });
                });
                ui.end_row();
                ui.label("JVM arguments");
                ui.add(egui::TextEdit::singleline(&mut f.jvm).hint_text("-XX:+UseZGC").desired_width(280.0));
                ui.end_row();
                ui.label("Java");
                ui.add(egui::TextEdit::singleline(&mut f.java).hint_text("automatic").desired_width(280.0));
                ui.end_row();
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, "Save", accent, true, egui::vec2(90.0, 34.0)).clicked() {
                    let ch = InstallationChanges {
                        name: Some(f.name.clone()),
                        version: Some(f.version.trim().to_string()),
                        memory_min_mb: Some(f.mem_override.then_some(f.min)),
                        memory_max_mb: Some(f.mem_override.then_some(f.max)),
                        width: Some(f.res_override.then_some(f.w)),
                        height: Some(f.res_override.then_some(f.h)),
                        jvm_args: Some(f.jvm.split_whitespace().map(str::to_owned).collect()),
                        java_path: Some((!f.java.trim().is_empty()).then(|| f.java.trim().into())),
                        ..Default::default()
                    };
                    match atlas.update_installation(&f.id, ch) {
                        Ok(_) => {
                            infos.push("Saved.".into());
                            close_edit = true;
                        }
                        Err(e) => errors.push(e),
                    }
                }
                if ui.button("Cancel").clicked() {
                    close_edit = true;
                }
            });
        });
    }
    if close_edit {
        ip.edit = None;
    }

    // Delete confirmation
    if let Some(inst) = ip.confirm_delete.clone() {
        let mut close = false;
        egui::Modal::new(egui::Id::unique("del-inst")).show(&ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(format!("Delete {}?", inst.name)).size(18.0).strong());
            ui.add_space(6.0);
            let msg = if inst.uses_external_dir() {
                "Atlas forgets this installation. Your own game folder (and the worlds in it) stays exactly where it is."
            } else {
                "This deletes the installation's game folder, including its worlds, screenshots and mods. Installed versions are kept."
            };
            ui.label(RichText::new(msg).color(theme::TEXT_DIM));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Delete").color(theme::ERR)).clicked() {
                    match atlas.delete_installation(&inst.id, Confirmation::Confirmed) {
                        Ok(_) => infos.push(format!("Deleted {}.", inst.name)),
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
            ip.confirm_delete = None;
        }
    }

    for e in errors {
        app.push_error(e);
    }
    for i in infos {
        app.push_info(i);
    }
    if goto_home {
        app.navigate(Page::Home);
    }
}
