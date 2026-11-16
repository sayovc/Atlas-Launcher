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
                widgets::kv(ui, "Account", match &status.account {
                    Some(a) => RichText::new(a),
                    None => RichText::new("Not signed in").color(theme::TEXT_FAINT),
                });
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
            // Phase 1: no installation/launch backend exists, so the action is
            // disabled. The label still comes from the backend state.
            let action_available = !status.unavailable.contains(&"installations");
            let w = ui.available_width();
            let resp = widgets::primary_button(ui, status.game_state.action_label(), accent, action_available, egui::vec2(w, 52.0));
            if !action_available {
                resp.on_disabled_hover_text("Installation management and launching are not built yet (Phase 3).");
                ui.label(RichText::new("Installing and launching arrive in a later build. This button shows the real backend state.").small().color(theme::TEXT_FAINT));
            } else if resp.clicked() && matches!(status.game_state, GameState::NoInstallation) {
                app.navigate(Page::Installations);
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Server address").color(theme::TEXT_DIM));
            let mut addr = String::new();
            ui.add_enabled(false, egui::TextEdit::singleline(&mut addr).hint_text("play.example.net:25565").desired_width(f32::INFINITY));
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
            if ui.link("Change in Settings").clicked() {
                app.navigate(Page::Settings);
            }
        });
    });
}
