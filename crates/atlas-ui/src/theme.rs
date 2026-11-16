//! Atlas visual identity: a dark "night-chart" palette with a single accent.
//! All colours live here so the rest of the UI never hardcodes them.

use atlas_config::settings::Accent;
use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};

pub const BG: Color32 = Color32::from_rgb(0x0C, 0x0F, 0x14);
pub const SIDEBAR: Color32 = Color32::from_rgb(0x10, 0x14, 0x1B);
pub const SURFACE: Color32 = Color32::from_rgb(0x16, 0x1B, 0x24);
pub const SURFACE_HI: Color32 = Color32::from_rgb(0x1D, 0x24, 0x30);
pub const BORDER: Color32 = Color32::from_rgb(0x26, 0x2E, 0x3B);
pub const TEXT: Color32 = Color32::from_rgb(0xE6, 0xEA, 0xF0);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8B, 0x95, 0xA5);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x5C, 0x66, 0x75);
pub const OK: Color32 = Color32::from_rgb(0x5B, 0xD1, 0x8B);
pub const WARN: Color32 = Color32::from_rgb(0xF2, 0xB8, 0x4B);
pub const ERR: Color32 = Color32::from_rgb(0xF2, 0x6D, 0x6D);

pub fn accent(a: Accent) -> Color32 {
    match a {
        Accent::Aurora => Color32::from_rgb(0x3F, 0xD0, 0xC0),
        Accent::Ember => Color32::from_rgb(0xF0, 0x8A, 0x4B),
        Accent::Moss => Color32::from_rgb(0x8C, 0xC8, 0x5A),
        Accent::Iris => Color32::from_rgb(0x9D, 0x8C, 0xFF),
    }
}

pub fn accent_name(a: Accent) -> &'static str {
    match a {
        Accent::Aurora => "Aurora",
        Accent::Ember => "Ember",
        Accent::Moss => "Moss",
        Accent::Iris => "Iris",
    }
}

pub const RADIUS: u8 = 10;

pub fn apply(ctx: &egui::Context, accent_color: Color32) {
    let mut v = Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.extreme_bg_color = Color32::from_rgb(0x0A, 0x0D, 0x11);
    v.faint_bg_color = SURFACE;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = accent_color;
    v.selection.bg_fill = accent_color.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, accent_color);
    v.window_corner_radius = CornerRadius::same(RADIUS);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.bg_fill = SURFACE;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(7);
    }
    v.widgets.inactive.bg_fill = SURFACE_HI;
    v.widgets.inactive.weak_bg_fill = SURFACE_HI;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x25, 0x2E, 0x3D);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x25, 0x2E, 0x3D);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent_color.gamma_multiply(0.6));
    v.widgets.active.bg_fill = accent_color.gamma_multiply(0.5);
    v.widgets.active.weak_bg_fill = accent_color.gamma_multiply(0.5);
    ctx.set_visuals(v);

    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(10.0, 8.0);
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        s.spacing.interact_size.y = 30.0;
        s.text_styles = [
            (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(24.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
        ]
        .into();
    });
}

/// Paint the Atlas mark: a globe of meridians and a latitude band, tilted.
pub fn paint_logo(p: &egui::Painter, c: egui::Pos2, r: f32, accent_color: Color32) {
    let stroke = Stroke::new((r * 0.11).max(1.2), accent_color);
    p.circle_stroke(c, r, stroke);
    let pts = |rx: f32, ry: f32, rot: f32| -> Vec<egui::Pos2> {
        (0..=48)
            .map(|i| {
                let t = i as f32 / 48.0 * std::f32::consts::TAU;
                let (x, y) = (rx * t.cos(), ry * t.sin());
                c + egui::vec2(x * rot.cos() - y * rot.sin(), x * rot.sin() + y * rot.cos())
            })
            .collect()
    };
    let tilt = -0.42;
    p.add(egui::Shape::line(pts(r * 0.42, r, tilt), Stroke::new(stroke.width * 0.8, accent_color.gamma_multiply(0.8))));
    p.add(egui::Shape::line(pts(r, r * 0.34, tilt), Stroke::new(stroke.width * 0.8, accent_color.gamma_multiply(0.8))));
    p.circle_filled(c + egui::vec2(r * 0.62, -r * 0.62), r * 0.16, accent_color);
}

/// Procedurally rendered window icon (no bundled artwork).
pub fn app_icon(size: u32) -> egui::IconData {
    let a = accent(Accent::Aurora);
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let c = size as f32 / 2.0;
    let r = c * 0.78;
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            let d = (dx * dx + dy * dy).sqrt();
            let i = ((y * size + x) * 4) as usize;
            let bg_alpha = if d < c * 0.98 { 255 } else { 0 };
            let (mut col, mut alpha) = ([0x10u8, 0x14, 0x1B], bg_alpha);
            let ring = (d - r).abs() < size as f32 * 0.05;
            // rotated ellipse meridian
            let (rx, ry) = (dx * 0.91 + dy * 0.41, -dx * 0.41 + dy * 0.91);
            let mer = ((rx / (r * 0.42)).powi(2) + (ry / r).powi(2) - 1.0).abs() < 0.12;
            let lat = ((rx / r).powi(2) + (ry / (r * 0.34)).powi(2) - 1.0).abs() < 0.14;
            if ring || ((mer || lat) && d < r) {
                col = [a.r(), a.g(), a.b()];
                alpha = 255;
            }
            rgba[i..i + 4].copy_from_slice(&[col[0], col[1], col[2], alpha]);
        }
    }
    egui::IconData { rgba, width: size, height: size }
}
