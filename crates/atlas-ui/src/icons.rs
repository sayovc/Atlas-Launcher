//! Hand-drawn vector icons (no icon font, no bundled artwork).

use crate::app::Page;
use egui::{vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

pub fn paint(p: &Painter, page: Page, c: Pos2, size: f32, color: Color32) {
    let s = size / 2.0;
    let st = Stroke::new((size * 0.09).max(1.3), color);
    let at = |x: f32, y: f32| c + vec2(x * s, y * s);
    match page {
        Page::Home => {
            p.add(Shape::line(vec![at(-0.9, -0.05), at(0.0, -0.85), at(0.9, -0.05)], st));
            p.add(Shape::line(vec![at(-0.65, -0.25), at(-0.65, 0.8), at(0.65, 0.8), at(0.65, -0.25)], st));
            p.add(Shape::line(vec![at(-0.18, 0.8), at(-0.18, 0.25), at(0.18, 0.25), at(0.18, 0.8)], st));
        }
        Page::Discover => {
            // compass
            p.circle_stroke(c, s * 0.86, st);
            let n = [at(0.0, -0.62), at(0.22, 0.0), at(0.0, 0.62), at(-0.22, 0.0)];
            p.add(Shape::convex_polygon(vec![n[0], n[1], n[3]], color, Stroke::NONE));
            p.add(Shape::closed_line(vec![n[1], n[2], n[3]], st));
        }
        Page::Installations => {
            for (i, dy) in [-0.45f32, 0.0, 0.45].iter().enumerate() {
                let pts = vec![at(0.0, dy - 0.38), at(0.85, *dy), at(0.0, dy + 0.38), at(-0.85, *dy)];
                if i == 0 {
                    p.add(Shape::closed_line(pts, st));
                } else {
                    p.add(Shape::line(vec![pts[3], pts[2], pts[1]], st));
                }
            }
        }
        Page::Versions => {
            p.add(Shape::closed_line(vec![at(-0.85, -0.85), at(0.05, -0.85), at(0.9, 0.0), at(0.0, 0.9), at(-0.85, 0.05)], st));
            p.circle_stroke(at(-0.4, -0.4), s * 0.16, st);
        }
        Page::Mods => {
            // isometric cube
            let top = vec![at(0.0, -0.9), at(0.8, -0.45), at(0.0, 0.0), at(-0.8, -0.45)];
            p.add(Shape::closed_line(top, st));
            p.add(Shape::line(vec![at(-0.8, -0.45), at(-0.8, 0.45), at(0.0, 0.9), at(0.8, 0.45), at(0.8, -0.45)], st));
            p.add(Shape::line_segment([at(0.0, 0.0), at(0.0, 0.9)], st));
        }
        Page::Servers => {
            for dy in [-0.45f32, 0.45] {
                let r = Rect::from_center_size(at(0.0, dy), vec2(1.7 * s, 0.7 * s));
                p.rect_stroke(r, 2.0, st, egui::StrokeKind::Middle);
                p.circle_filled(at(-0.5, dy), s * 0.1, color);
            }
        }
        Page::Worlds => {
            p.circle_stroke(c, s * 0.88, st);
            p.add(Shape::line_segment([at(-0.88, 0.0), at(0.88, 0.0)], st));
            let ell: Vec<Pos2> = (0..=32)
                .map(|i| {
                    let t = i as f32 / 32.0 * std::f32::consts::TAU;
                    at(0.38 * t.cos(), 0.88 * t.sin())
                })
                .collect();
            p.add(Shape::closed_line(ell, st));
        }
        Page::Downloads => {
            p.add(Shape::line_segment([at(0.0, -0.9), at(0.0, 0.35)], st));
            p.add(Shape::line(vec![at(-0.45, -0.1), at(0.0, 0.35), at(0.45, -0.1)], st));
            p.add(Shape::line(vec![at(-0.85, 0.35), at(-0.85, 0.85), at(0.85, 0.85), at(0.85, 0.35)], st));
        }
        Page::Logs => {
            p.add(Shape::closed_line(vec![at(-0.7, -0.9), at(0.35, -0.9), at(0.7, -0.55), at(0.7, 0.9), at(-0.7, 0.9)], st));
            for y in [-0.3f32, 0.1, 0.5] {
                p.add(Shape::line_segment([at(-0.4, y), at(0.4, y)], st));
            }
        }
        Page::Settings => {
            p.circle_stroke(c, s * 0.3, st);
            p.circle_stroke(c, s * 0.62, st);
            for k in 0..8 {
                let a = k as f32 / 8.0 * std::f32::consts::TAU;
                let d = vec2(a.cos(), a.sin());
                p.add(Shape::line_segment([c + d * s * 0.62, c + d * s * 0.92], Stroke::new(st.width * 1.6, color)));
            }
        }
    }
}

/// Allocate space in a layout and paint an icon there.
pub fn show(ui: &mut egui::Ui, page: Page, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    paint(ui.painter(), page, rect.center(), size, color);
}


/// Five-point star from triangles (egui only fills convex shapes).
pub fn star(ui: &mut egui::Ui, filled: bool, color: Color32, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), egui::Sense::click());
    let c = rect.center();
    let (ro, ri) = (size * 0.48, size * 0.2);
    let pt = |i: usize, r: f32| {
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        c + vec2(a.cos() * r, a.sin() * r)
    };
    let outline: Vec<Pos2> = (0..10).map(|i| pt(i, if i % 2 == 0 { ro } else { ri })).collect();
    let p = ui.painter();
    if filled {
        let inner: Vec<Pos2> = (0..5).map(|i| pt(i * 2 + 1, ri)).collect();
        p.add(Shape::convex_polygon(inner, color, Stroke::NONE));
        for i in 0..5 {
            p.add(Shape::convex_polygon(vec![pt(i * 2, ro), pt(i * 2 + 1, ri), pt((i * 2 + 9) % 10, ri)], color, Stroke::NONE));
        }
    } else {
        p.add(Shape::closed_line(outline, Stroke::new(1.2, color)));
    }
    resp
}
