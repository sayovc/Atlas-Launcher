use crate::app::{AtlasApp, Page};
use crate::{theme, widgets};
use atlas_core::{Confirmation, PingResult, Server};
use base64::Engine as _;
use egui::RichText;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};

#[derive(Default)]
pub struct ServersPage {
    add_open: bool,
    name: String,
    address: String,
    editing: Option<String>,
    confirm_remove: Option<Server>,
    pings: HashMap<String, Result<PingResult, String>>,
    pending: Vec<(String, Receiver<atlas_shared::AtlasResult<PingResult>>)>,
    textures: HashMap<String, egui::TextureHandle>,
    auto_pinged: bool,
}

impl ServersPage {
    fn ping(&mut self, atlas: &std::sync::Arc<atlas_core::Atlas>, id: &str, ctx: &egui::Context) {
        if self.pending.iter().any(|(i, _)| i == id) {
            return;
        }
        let (tx, rx) = channel();
        let (a, id2, c) = (atlas.clone(), id.to_string(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(a.ping_server(&id2));
            c.request_repaint();
        });
        self.pending.push((id.to_string(), rx));
    }

    fn icon(&mut self, ctx: &egui::Context, s: &Server) -> Option<egui::TextureHandle> {
        let b64 = s.icon.as_ref()?;
        if let Some(t) = self.textures.get(&s.id) {
            return Some(t.clone());
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
        let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?.to_rgba8();
        let size = [img.width() as usize, img.height() as usize];
        let ci = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
        let t = ctx.load_texture(format!("server-{}", s.id), ci, egui::TextureOptions::LINEAR);
        self.textures.insert(s.id.clone(), t.clone());
        Some(t)
    }
}

pub fn show(app: &mut AtlasApp, ui: &mut egui::Ui) {
    let atlas = app.atlas.clone();
    let accent = app.accent();
    let ctx = ui.ctx().clone();
    let list = atlas.servers().unwrap_or_default();
    let mut errors = vec![];
    let mut infos = vec![];
    let mut go_home = false;
    let sp = &mut app.servers_page;

    // collect finished pings
    let mut done = vec![];
    sp.pending.retain(|(id, rx)| match rx.try_recv() {
        Ok(r) => {
            done.push((id.clone(), r));
            false
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => true,
        Err(_) => false,
    });
    for (id, r) in done {
        sp.textures.remove(&id);
        sp.pings.insert(id, r.map_err(|e| e.message));
    }
    if !sp.auto_pinged {
        sp.auto_pinged = true;
        for s in &list {
            sp.ping(&atlas, &s.id, &ctx);
        }
    }

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("Servers").heading().strong());
            ui.label(RichText::new("Join through your selected installation.").color(theme::TEXT_DIM));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::primary_button(ui, "Add server", accent, true, egui::vec2(130.0, 38.0)).clicked() {
                sp.add_open = true;
                sp.editing = None;
                sp.name.clear();
                sp.address.clear();
            }
            if ui.button("Refresh").clicked() {
                for s in &list {
                    sp.ping(&atlas, &s.id, &ctx);
                }
            }
        });
    });
    ui.add_space(12.0);

    if list.is_empty() {
        widgets::card(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(18.0);
                crate::icons::show(ui, Page::Servers, 40.0, theme::TEXT_FAINT);
                ui.add_space(8.0);
                ui.label(RichText::new("No servers yet").size(17.0).strong());
                ui.label(RichText::new("Add one and Atlas will show who's online.").color(theme::TEXT_DIM));
                ui.add_space(18.0);
            });
        });
    }

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for s in &list {
            let tex = sp.icon(&ctx, s);
            let pinging = sp.pending.iter().any(|(i, _)| i == &s.id);
            widgets::card(ui, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
                    match &tex {
                        Some(t) => {
                            ui.painter().image(t.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                        }
                        None => {
                            ui.painter().rect_filled(rect, 8.0, theme::SURFACE_HI);
                            crate::icons::paint(ui.painter(), Page::Servers, rect.center(), 22.0, theme::TEXT_FAINT);
                        }
                    }
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            if crate::icons::star(ui, s.favorite, if s.favorite { theme::WARN } else { theme::TEXT_FAINT }, 16.0).on_hover_text("Favourite").clicked()
                                && let Err(e) = atlas.set_server_favorite(&s.id, !s.favorite)
                            {
                                errors.push(e);
                            }
                            ui.label(RichText::new(&s.name).size(16.0).strong());
                            ui.label(RichText::new(s.host_port()).monospace().color(theme::TEXT_FAINT));
                        });
                        match sp.pings.get(&s.id) {
                            Some(Ok(p)) => {
                                ui.horizontal(|ui| {
                                    widgets::pill(ui, &format!("{} / {} online", p.online, p.max), theme::OK);
                                    ui.label(RichText::new(format!("{}  ·  {} ms", p.version, p.latency_ms)).small().color(theme::TEXT_DIM));
                                });
                                if !p.motd.is_empty() {
                                    ui.add(egui::Label::new(RichText::new(&p.motd).small().color(theme::TEXT_DIM)).truncate());
                                }
                            }
                            Some(Err(_)) => {
                                widgets::pill(ui, "Can't reach server", theme::ERR);
                            }
                            None if pinging => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(RichText::new("Checking…").small().color(theme::TEXT_FAINT));
                                });
                            }
                            None => {}
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button("•••", |ui| {
                            if ui.button("Edit").clicked() {
                                sp.add_open = true;
                                sp.editing = Some(s.id.clone());
                                sp.name = s.name.clone();
                                sp.address = s.host_port();
                                ui.close();
                            }
                            if ui.button("Ping again").clicked() {
                                sp.ping(&atlas, &s.id, &ctx);
                                ui.close();
                            }
                            if ui.button(RichText::new("Remove").color(theme::ERR)).clicked() {
                                sp.confirm_remove = Some(s.clone());
                                ui.close();
                            }
                        });
                        if ui.add(egui::Button::new(RichText::new("Join").strong().color(accent)).stroke(egui::Stroke::new(1.0, accent.gamma_multiply(0.6)))).clicked() {
                            atlas.dismiss_game_error();
                            match atlas.connect_server(&s.id) {
                                Ok(()) => go_home = true,
                                Err(e) => errors.push(e),
                            }
                        }
                    });
                });
            });
            ui.add_space(8.0);
        }
    });

    if sp.add_open {
        let mut close = false;
        egui::Modal::new(egui::Id::unique("server-form")).show(&ctx, |ui| {
            ui.set_width(400.0);
            ui.label(RichText::new(if sp.editing.is_some() { "Edit server" } else { "Add server" }).size(18.0).strong());
            ui.add_space(8.0);
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut sp.name).hint_text("My server").desired_width(f32::INFINITY));
            ui.label("Address");
            ui.add(egui::TextEdit::singleline(&mut sp.address).hint_text("play.example.net:25565").desired_width(f32::INFINITY));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, "Save", accent, !sp.address.trim().is_empty(), egui::vec2(90.0, 34.0)).clicked() {
                    let r = match &sp.editing {
                        Some(id) => atlas.edit_server(id, Some(&sp.name), Some(&sp.address)),
                        None => atlas.add_server(&sp.name, &sp.address),
                    };
                    match r {
                        Ok(s) => {
                            sp.ping(&atlas, &s.id, &ctx);
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
            sp.add_open = false;
        }
    }

    if let Some(s) = sp.confirm_remove.clone() {
        let mut close = false;
        egui::Modal::new(egui::Id::unique("server-remove")).show(&ctx, |ui| {
            ui.set_width(360.0);
            ui.label(RichText::new(format!("Remove {}?", s.name)).size(18.0).strong());
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Remove").color(theme::ERR)).clicked() {
                    match atlas.remove_server(&s.id, Confirmation::Confirmed) {
                        Ok(()) => infos.push(format!("Removed {}.", s.name)),
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
            sp.confirm_remove = None;
        }
    }

    for e in errors {
        app.push_error(e);
    }
    for i in infos {
        app.push_info(i);
    }
    if go_home {
        app.navigate(Page::Home);
    }
}
