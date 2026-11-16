use crate::app::{AtlasApp, Page};
use crate::{theme, widgets};
use atlas_core::GameState;
use egui::RichText;

fn state_color(s: &GameState) -> egui::Color32 {
    match s {
        GameState::Ready | GameState::Running { .. } => theme::OK,
        GameState::Error { .. } => theme::ERR,
        GameState::NoInstallation | GameState::NotInstalled => theme::TEXT_DIM,
        _ => theme::WARN,
    }
}

fn state_text(s: &GameState) -> String {
    match s {
        GameState::NoInstallation => "No installation".into(),
        GameState::NotInstalled => "Not installed".into(),
        GameState::Installing { progress } => format!("Installing {:.0}%", progress * 100.0),
        GameState::Preparing { progress } => format!("Preparing {:.0}%", progress * 100.0),
        GameState::Updating { progress } => format!("Updating {:.0}%", progress * 100.0),
        GameState::Ready => "Ready".into(),
        GameState::Launching => "Launching".into(),
        GameState::Running { pid } => format!("Running (pid {pid})"),
        GameState::Stopping => "Stopping".into(),
        GameState::Error { error } => format!("Error: {}", error.code),
    }
}

/// Decorative contour-line backdrop for the hero card (procedural, original).
fn paint_contours(p: &egui::Painter, rect: egui::Rect, accent: egui::Color32, t: f32) {
    let center = rect.right_center() + egui::vec2(-rect.width() * 0.18, rect.height() * 0.1);
    for i in 0..9 {
        let base = 26.0 + i as f32 * 22.0;
        let pts: Vec<egui::Pos2> = (0..=72)
            .map(|k| {
                let a = k as f32 / 72.0 * std::f32::consts::TAU;
                let wobble = 1.0 + 0.10 * (a * 3.0 + i as f32 * 0.7 + t * 0.15).sin() + 0.05 * (a * 5.0 - i as f32).cos();
                center + egui::vec2(a.cos() * base * 1.5 * wobble, a.sin() * base * wobble)
            })
            .collect();
        let alpha = 0.22 - i as f32 * 0.02;
        p.with_clip_rect(rect).add(egui::Shape::closed_line(pts, egui::Stroke::new(1.0, accent.gamma_multiply(alpha.max(0.03)))));
    }
}

pub fn show(app: &mut AtlasApp, ui: &mut egui::Ui) {
    let accent = app.accent();
    let status = app.status.clone();
    let settings = app.atlas.settings();

    // Hero
    let hero_h = 150.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), hero_h), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, theme::RADIUS, theme::SURFACE);
    let t = if settings.appearance.reduce_motion { 0.0 } else { ui.input(|i| i.time) as f32 };
    paint_contours(p, rect, accent, t);
    p.rect_stroke(rect, theme::RADIUS, egui::Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
    p.text(rect.left_top() + egui::vec2(26.0, 30.0), egui::Align2::LEFT_TOP, "Chart your next world.", egui::FontId::proportional(28.0), theme::TEXT);
    p.text(
        rect.left_top() + egui::vec2(26.0, 70.0),
        egui::Align2::LEFT_TOP,
        "Atlas manages installations, versions and launches for Minecraft: Java Edition.",
        egui::FontId::proportional(14.0),
        theme::TEXT_DIM,
    );
    p.text(
        rect.left_top() + egui::vec2(26.0, 108.0),
        egui::Align2::LEFT_TOP,
        "Not affiliated with Mojang or Microsoft. Uses your own legally obtained copy of the game.",
        egui::FontId::proportional(11.5),
        theme::TEXT_FAINT,
    );
    ui.add_space(16.0);

    ui.columns(2, |cols| {
        // Status
        widgets::card(&mut cols[0], |ui| {
            widgets::section_title(ui, "Status");
            egui::Grid::new("home-status").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
                ui.label(RichText::new("Account").color(theme::TEXT_DIM));
                match &status.account {
                    Some(a) => {
                        ui.label(RichText::new(a));
                    }
                    None => {
                        if ui.link("Sign in").clicked() {
                            app.settings_page.open_accounts();
                            app.navigate(Page::Settings);
                        }
                    }
                }
                ui.end_row();
                widgets::kv(ui, "Installation", match &status.installation {
                    Some(i) => RichText::new(i),
                    None => RichText::new("None").color(theme::TEXT_FAINT),
                });
                widgets::kv(ui, "Minecraft version", RichText::new(status.minecraft_version.as_deref().unwrap_or("—")));
                widgets::kv(ui, "Atlas version", RichText::new(status.atlas_version));
                ui.label(RichText::new("Game status").color(theme::TEXT_DIM));
                widgets::pill(ui, &state_text(&status.game_state), state_color(&status.game_state));
                ui.end_row();
                let iu = match (&status.version_update, status.installed_versions) {
                    (Some(v), n) => RichText::new(format!("{n} installed · {v} available")).color(theme::WARN),
                    (None, 0) => RichText::new("No versions installed").color(theme::TEXT_FAINT),
                    (None, n) => RichText::new(format!("{n} installed · up to date")).color(theme::OK),
                };
                widgets::kv(ui, "Versions", iu);
            });
        });

        // Play
        widgets::card(&mut cols[1], |ui| {
            widgets::section_title(ui, "Play");
            let insts = app.atlas.installations().unwrap_or_default();
            if !insts.is_empty() {
                let sel = app.atlas.selected_installation();
                egui::ComboBox::from_id_salt("home-inst")
                    .width(ui.available_width())
                    .selected_text(sel.as_ref().map(|i| format!("{}  ·  {}", i.name, i.version)).unwrap_or_default())
                    .show_ui(ui, |ui| {
                        for i in &insts {
                            if ui.selectable_label(sel.as_ref().is_some_and(|s| s.id == i.id), format!("{}  ·  {}", i.name, i.version)).clicked() {
                                let _ = app.atlas.select_installation(&i.id);
                                app.refresh_status();
                            }
                        }
                    });
                ui.add_space(6.0);
            }
            let st = &status.game_state;
            let w = ui.available_width();
            let busy = matches!(st, GameState::Installing { .. } | GameState::Preparing { .. } | GameState::Launching | GameState::Stopping);
            let label = st.action_label();
            let resp = widgets::primary_button(ui, label, if matches!(st, GameState::Running { .. }) { theme::ERR } else { accent }, !busy, egui::vec2(w, 52.0));
            if let GameState::Installing { progress } | GameState::Preparing { progress } = st {
                ui.add(egui::ProgressBar::new(*progress).show_percentage().animate(true).desired_height(6.0));
            }
            if resp.clicked() {
                let r = match st {
                    GameState::NoInstallation => {
                        app.navigate(Page::Installations);
                        Ok(())
                    }
                    GameState::NotInstalled => match app.atlas.selected_installation() {
                        Some(i) => app.atlas.start_install(&i.version).map(|_| ()),
                        None => Ok(()),
                    },
                    GameState::Ready => {
                        let server = (!app.server_addr.trim().is_empty()).then(|| app.server_addr.trim().to_string());
                        app.atlas.play(None, atlas_core::PlayOptions { server })
                    }
                    GameState::Running { .. } => app.atlas.stop(None),
                    GameState::Error { error } => {
                        app.push_error(error.clone());
                        app.atlas.dismiss_game_error();
                        Ok(())
                    }
                    _ => Ok(()),
                };
                if let Err(e) = r {
                    app.push_error(e);
                }
                app.refresh_status();
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Join a server (optional)").color(theme::TEXT_DIM));
            ui.add(egui::TextEdit::singleline(&mut app.server_addr).hint_text("play.example.net:25565").desired_width(f32::INFINITY));
            ui.add_space(6.0);
            ui.label(RichText::new("Launch options").color(theme::TEXT_DIM));
            ui.label(
                RichText::new(format!(
                    "{}–{} MB memory · {}×{}{}",
                    settings.memory.min_mb,
                    settings.memory.max_mb,
                    settings.game.width,
                    settings.game.height,
                    if settings.game.fullscreen { " · fullscreen" } else { "" }
                ))
                .monospace(),
            );
            ui.horizontal(|ui| {
                if ui.link("Change in Settings").clicked() {
                    app.navigate(Page::Settings);
                }
                if status.account.is_none() {
                    ui.label(RichText::new("· demo mode until you sign in").small().color(theme::TEXT_FAINT));
                }
            });
        });
    });
}
