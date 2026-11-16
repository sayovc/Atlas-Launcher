use crate::theme;
use atlas_shared::AtlasError;
use egui::{Color32, RichText};

pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(theme::RADIUS)
        .inner_margin(egui::Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn page_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).heading().strong());
    ui.label(RichText::new(subtitle).color(theme::TEXT_DIM));
    ui.add_space(14.0);
}

pub fn section_title(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title.to_uppercase()).size(11.0).color(theme::TEXT_FAINT).extra_letter_spacing(1.5));
    ui.add_space(2.0);
}

pub fn pill(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.14))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.45)))
        .corner_radius(20)
        .inner_margin(egui::Margin::symmetric(9, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.5).color(color));
        });
}

/// Primary call-to-action button in the accent colour.
pub fn primary_button(ui: &mut egui::Ui, text: &str, accent: Color32, enabled: bool, size: egui::Vec2) -> egui::Response {
    let fill = if enabled { accent } else { theme::SURFACE_HI };
    let fg = if enabled { Color32::from_rgb(0x06, 0x12, 0x14) } else { theme::TEXT_DIM };
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).size(17.0).strong().color(fg))
            .fill(fill)
            .corner_radius(10)
            .min_size(size),
    )
}

/// Returns true if dismissed.
pub fn error_banner(ui: &mut egui::Ui, e: &AtlasError) -> bool {
    let mut closed = false;
    egui::Frame::new()
        .fill(theme::ERR.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, theme::ERR.gamma_multiply(0.5)))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Sides::new().shrink_left().show(
                ui,
                |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(e.code.as_str()).monospace().color(theme::ERR));
                        ui.label(RichText::new(&e.message).strong());
                    });
                },
                |ui| {
                    if ui.small_button("×").on_hover_text("Dismiss").clicked() {
                        closed = true;
                    }
                    if ui.small_button("Copy").on_hover_text("Copy error details").clicked() {
                        ui.ctx().copy_text(format!("[{}] {}\n{}\n{}", e.code, e.message, e.details, e.suggestion));
                    }
                },
            );
            if !e.suggestion.is_empty() {
                ui.add(egui::Label::new(RichText::new(&e.suggestion).color(theme::TEXT_DIM)).wrap());
            }
            if !e.details.is_empty() {
                egui::CollapsingHeader::new(RichText::new("Technical details").small().color(theme::TEXT_FAINT))
                    .id_salt(("err-details", e.code.as_str(), &e.message))
                    .show(ui, |ui| {
                        ui.label(RichText::new(&e.details).monospace().color(theme::TEXT_DIM));
                    });
            }
        });
    closed
}

pub fn info_banner(ui: &mut egui::Ui, text: &str) -> bool {
    let mut closed = false;
    egui::Frame::new()
        .fill(theme::OK.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, theme::OK.gamma_multiply(0.45)))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Sides::new().shrink_left().show(
                ui,
                |ui| {
                    ui.add(egui::Label::new(RichText::new(text)).wrap());
                },
                |ui| {
                    closed = ui.small_button("×").clicked();
                },
            );
        });
    closed
}

/// Label/value row used in status grids.
pub fn kv(ui: &mut egui::Ui, key: &str, value: RichText) {
    ui.label(RichText::new(key).color(theme::TEXT_DIM));
    ui.label(value);
    ui.end_row();
}
