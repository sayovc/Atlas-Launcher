use crate::app::{AtlasApp, Page};
use crate::{theme, widgets};
use atlas_core::{Confirmation, JobState, VersionKind, VersionList, VerifyReport};
use atlas_shared::AtlasResult;
use egui::RichText;
use std::sync::mpsc::{channel, Receiver};

pub struct VersionsPage {
    list: Option<VersionList>,
    loading: Option<Receiver<AtlasResult<VersionList>>>,
    verifying: Option<(String, Receiver<AtlasResult<VerifyReport>>)>,
    search: String,
    show_releases: bool,
    show_snapshots: bool,
    show_old: bool,
    installed_only: bool,
    confirm_remove: Option<String>,
    requested_initial: bool,
}

impl Default for VersionsPage {
    fn default() -> Self {
        Self { list: None, loading: None, verifying: None, search: String::new(), show_releases: true, show_snapshots: false, show_old: false, installed_only: false, confirm_remove: None, requested_initial: false }
    }
}

impl VersionsPage {
    fn load(&mut self, app_atlas: &std::sync::Arc<atlas_core::Atlas>, refresh: bool, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let a = app_atlas.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(a.versions(refresh));
            ctx.request_repaint();
        });
        self.loading = Some(rx);
    }

    /// Refresh installed flags locally (cheap) after jobs finish.
    fn sync_installed(&mut self, atlas: &atlas_core::Atlas) {
        if let (Some(list), Ok(inst)) = (&mut self.list, atlas.installed_versions()) {
            let set: std::collections::HashSet<_> = inst.into_iter().map(|v| v.id).collect();
            for v in &mut list.versions {
                v.installed = set.contains(&v.id);
            }
        }
    }
}

fn kind_color(k: VersionKind) -> egui::Color32 {
    match k {
        VersionKind::Release => theme::OK,
        VersionKind::Snapshot => theme::WARN,
        _ => theme::TEXT_DIM,
    }
}

pub fn show(app: &mut AtlasApp, ui: &mut egui::Ui) {
    let atlas = app.atlas.clone();
    let accent = app.accent();
    let ctx = ui.ctx().clone();
    let jobs = atlas.jobs();
    let vp = &mut app.versions_page;

    if !vp.requested_initial {
        vp.requested_initial = true;
        vp.show_snapshots = atlas.settings().installations.show_snapshots;
        vp.load(&atlas, false, &ctx);
    }
    let mut errors = Vec::new();
    let mut infos = Vec::new();
    if let Some(rx) = &vp.loading
        && let Ok(r) = rx.try_recv()
    {
        vp.loading = None;
        match r {
            Ok(l) => {
                if l.stale {
                    infos.push("You're offline: showing the last saved version list.".to_string());
                }
                vp.list = Some(l);
            }
            Err(e) => errors.push(e),
        }
    }
    if let Some((id, rx)) = &vp.verifying
        && let Ok(r) = rx.try_recv()
    {
        let id = id.clone();
        vp.verifying = None;
        match r {
            Ok(rep) if rep.ok() => infos.push(format!("{id}: all {} files verified.", rep.checked)),
            Ok(rep) => errors.push(
                atlas_shared::AtlasError::new(atlas_shared::ErrorCode::InvalidInstallation, format!("{id}: {} missing and {} corrupt files.", rep.missing.len(), rep.corrupt.len()))
                    .suggest("Press Repair to re-download only the bad files."),
            ),
            Err(e) => errors.push(e),
        }
    }
    // Any job finished since last frame? refresh installed flags.
    if jobs.iter().any(|j| j.state != JobState::Running) {
        vp.sync_installed(&atlas);
    }

    widgets::page_header(ui, "Versions", "Official Minecraft: Java Edition releases. Every file is checked against Mojang's checksums.");

    // Hero: latest release
    if let Some(list) = &vp.list {
        let latest = list.latest_release.clone();
        let installed = list.versions.iter().any(|v| v.id == latest && v.installed);
        let job = jobs.iter().find(|j| j.version == latest && j.state == JobState::Running);
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("LATEST RELEASE").size(11.0).color(theme::TEXT_FAINT).extra_letter_spacing(1.5));
                    ui.label(RichText::new(&latest).size(30.0).strong());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(j) = job {
                        ui.add(egui::ProgressBar::new(j.progress.fraction()).desired_width(220.0).show_percentage().animate(true));
                    } else if installed {
                        widgets::pill(ui, "Installed", theme::OK);
                    } else if widgets::primary_button(ui, "Install", accent, true, egui::vec2(140.0, 42.0)).clicked() {
                        match atlas.start_install(&latest) {
                            Ok(_) => infos.push(format!("Installing {latest}…")),
                            Err(e) => errors.push(e),
                        }
                    }
                });
            });
        });
        ui.add_space(12.0);
    }

    // Toolbar
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut vp.search).hint_text("Search versions…").desired_width(200.0));
        ui.toggle_value(&mut vp.show_releases, "Releases");
        ui.toggle_value(&mut vp.show_snapshots, "Snapshots");
        ui.toggle_value(&mut vp.show_old, "Beta & Alpha");
        ui.toggle_value(&mut vp.installed_only, "Installed");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if vp.loading.is_some() {
                ui.spinner();
            } else if ui.button("Refresh").on_hover_text("Fetch the latest list from Mojang").clicked() {
                vp.load(&atlas, true, &ctx);
            }
        });
    });
    ui.add_space(6.0);

    let Some(list) = &vp.list else {
        widgets::card(ui, |ui| {
            if vp.loading.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading versions…");
                });
            } else {
                ui.label(RichText::new("No version list yet. Check your connection and press Refresh.").color(theme::TEXT_DIM));
            }
        });
        flush(app, errors, infos);
        return;
    };

    let needle = vp.search.to_lowercase();
    let rows: Vec<_> = list
        .versions
        .iter()
        .filter(|v| match v.kind {
            VersionKind::Release => vp.show_releases,
            VersionKind::Snapshot => vp.show_snapshots,
            _ => vp.show_old,
        })
        .filter(|v| !vp.installed_only || v.installed)
        .filter(|v| needle.is_empty() || v.id.to_lowercase().contains(&needle))
        .cloned()
        .collect();

    let mut nav_downloads = false;
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(theme::RADIUS)
        .inner_margin(egui::Margin::symmetric(6, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(ui.available_height());
            if rows.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new("No versions match these filters.").color(theme::TEXT_FAINT));
                return;
            }
            let row_h = 44.0;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, rows.len(), |ui, range| {
                for v in &rows[range] {
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), egui::Sense::hover());
                    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                    if hover > 0.0 {
                        ui.painter().rect_filled(rect.shrink2(egui::vec2(2.0, 2.0)), 8.0, theme::SURFACE_HI.gamma_multiply(hover));
                    }
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(14.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
                    let ui = &mut child;
                    let col = |ui: &mut egui::Ui, w: f32, text: RichText| {
                        ui.allocate_ui_with_layout(egui::vec2(w, row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.set_min_width(w);
                            ui.add(egui::Label::new(text).truncate());
                        });
                    };
                    col(ui, 150.0, RichText::new(&v.id).strong().size(15.0));
                    col(ui, 90.0, RichText::new(v.kind.label()).color(kind_color(v.kind)));
                    ui.label(RichText::new(v.release_time.get(..10).unwrap_or("")).color(theme::TEXT_FAINT).monospace());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let job = jobs.iter().find(|j| j.version == v.id && j.state == JobState::Running);
                        if let Some(j) = job {
                            if ui.small_button("Details").clicked() {
                                nav_downloads = true;
                            }
                            ui.add(egui::ProgressBar::new(j.progress.fraction()).desired_width(180.0).show_percentage().animate(true));
                        } else if v.installed {
                            if ui.button(RichText::new("Remove").color(theme::ERR)).clicked() {
                                vp.confirm_remove = Some(v.id.clone());
                            }
                            if ui.button("Repair").on_hover_text("Re-download only missing or corrupt files").clicked() {
                                match atlas.start_repair(&v.id) {
                                    Ok(_) => infos.push(format!("Repairing {}…", v.id)),
                                    Err(e) => errors.push(e),
                                }
                            }
                            let busy = vp.verifying.as_ref().is_some_and(|(id, _)| id == &v.id);
                            if busy {
                                ui.spinner();
                            } else if ui.add_enabled(vp.verifying.is_none(), egui::Button::new("Verify")).clicked() {
                                let (tx, rx) = channel();
                                let (a, id, c) = (atlas.clone(), v.id.clone(), ctx.clone());
                                std::thread::spawn(move || {
                                    let _ = tx.send(a.verify_version(&id));
                                    c.request_repaint();
                                });
                                vp.verifying = Some((v.id.clone(), rx));
                            }
                            widgets::pill(ui, "Installed", theme::OK);
                        } else if ui.add(egui::Button::new(RichText::new("Install").color(accent)).stroke(egui::Stroke::new(1.0, accent.gamma_multiply(0.5)))).clicked() {
                            match atlas.start_install(&v.id) {
                                Ok(_) => infos.push(format!("Installing {}…", v.id)),
                                Err(e) => errors.push(e),
                            }
                        }
                    });
                }
            });
        });

    // Remove confirmation
    if let Some(id) = vp.confirm_remove.clone() {
        let mut close = false;
        egui::Modal::new(egui::Id::unique("remove-version")).show(&ctx, |ui| {
            ui.set_width(400.0);
            ui.label(RichText::new(format!("Remove {id}?")).size(18.0).strong());
            ui.add_space(6.0);
            ui.label(RichText::new("The version's own files are deleted. Shared libraries and assets used by other versions are kept. Worlds are never touched.").color(theme::TEXT_DIM));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Remove").color(theme::ERR)).clicked() {
                    match atlas.remove_version(&id, Confirmation::Confirmed) {
                        Ok(()) => infos.push(format!("Removed {id}.")),
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
            vp.confirm_remove = None;
            vp.sync_installed(&atlas);
        }
    }
    if nav_downloads {
        app.navigate(Page::Downloads);
    }
    flush(app, errors, infos);
}

fn flush(app: &mut AtlasApp, errors: Vec<atlas_shared::AtlasError>, infos: Vec<String>) {
    for e in errors {
        app.push_error(e);
    }
    for i in infos {
        app.push_info(i);
    }
}
