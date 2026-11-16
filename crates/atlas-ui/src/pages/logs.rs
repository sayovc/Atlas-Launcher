use crate::app::AtlasApp;
use crate::{theme, widgets};
use atlas_logging::{LogFilter, LogLevel, LogRecord, LogSource};
use egui::RichText;

pub struct LogsPage {
    min_level: LogLevel,
    source: Option<LogSource>,
    search: String,
    follow: bool,
    cache: Vec<LogRecord>,
    cache_key: (u64, LogLevel, Option<LogSource>, String),
}

impl Default for LogsPage {
    fn default() -> Self {
        Self { min_level: LogLevel::Info, source: None, search: String::new(), follow: true, cache: vec![], cache_key: (u64::MAX, LogLevel::Info, None, String::new()) }
    }
}

fn level_color(l: LogLevel) -> egui::Color32 {
    match l {
        LogLevel::Error => theme::ERR,
        LogLevel::Warn => theme::WARN,
        LogLevel::Info => theme::TEXT,
        LogLevel::Debug => theme::TEXT_DIM,
        LogLevel::Trace => theme::TEXT_FAINT,
    }
}

pub fn show(app: &mut AtlasApp, ui: &mut egui::Ui) {
    widgets::page_header(ui, "Logs", "Launcher activity for this session. Credentials are redacted automatically.");
    let accent = app.accent();
    let logs_dir = app.atlas.paths().launcher_logs_dir();
    let lp = &mut app.logs_page;

    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut lp.search).hint_text("Search messages…").desired_width(260.0));
        egui::ComboBox::from_id_salt("log-level")
            .selected_text(format!("Level ≥ {}", lp.min_level.as_str()))
            .show_ui(ui, |ui| {
                for l in LogLevel::ALL {
                    ui.selectable_value(&mut lp.min_level, l, l.as_str());
                }
            });
        egui::ComboBox::from_id_salt("log-source")
            .selected_text(lp.source.map(|s| s.label()).unwrap_or("All sources"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut lp.source, None, "All sources");
                for s in LogSource::ALL {
                    ui.selectable_value(&mut lp.source, Some(s), s.label());
                }
            });
        ui.checkbox(&mut lp.follow, "Follow");
    });

    let key = (app.atlas.live_log_generation(), lp.min_level, lp.source, lp.search.clone());
    if key != lp.cache_key {
        let filter = LogFilter { min_level: lp.min_level, search: lp.search.clone(), source: lp.source };
        lp.cache = app.atlas.live_logs(&filter);
        lp.cache_key = key;
    }

    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} records", lp.cache.len())).small().color(theme::TEXT_FAINT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Copy visible").clicked() {
                let text = lp.cache.iter().map(|r| r.to_line()).collect::<Vec<_>>().join("\n");
                ui.ctx().copy_text(text);
            }
            if ui.button("Copy folder path").on_hover_text(logs_dir.display().to_string()).clicked() {
                ui.ctx().copy_text(logs_dir.display().to_string());
            }
        });
    });
    ui.add_space(4.0);

    egui::Frame::new()
        .fill(egui::Color32::from_rgb(0x0A, 0x0D, 0x11))
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(theme::RADIUS)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(ui.available_height());
            if lp.cache.is_empty() {
                ui.label(RichText::new("No records match the current filters.").color(theme::TEXT_FAINT));
                return;
            }
            let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .stick_to_bottom(lp.follow)
                .show_rows(ui, row_h, lp.cache.len(), |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.spacing_mut().interact_size.y = row_h;
                    for r in &lp.cache[range] {
                        ui.horizontal(|ui| {
                            let ts = r.timestamp.get(11..19).unwrap_or(&r.timestamp);
                            ui.label(RichText::new(ts).monospace().color(theme::TEXT_FAINT));
                            ui.label(RichText::new(format!("{:5}", r.level.as_str())).monospace().color(level_color(r.level)));
                            ui.label(RichText::new(r.source.label()).monospace().color(accent.gamma_multiply(0.8)));
                            let resp = ui.add(egui::Label::new(RichText::new(&r.message).monospace().color(level_color(r.level).max(theme::TEXT_DIM))).sense(egui::Sense::click()));
                            if resp.on_hover_text(format!("{}\nClick to copy", r.target)).clicked() {
                                ui.ctx().copy_text(r.to_line());
                            }
                        });
                    }
                });
        });
}

trait MaxColor {
    fn max(self, other: egui::Color32) -> egui::Color32;
}
impl MaxColor for egui::Color32 {
    fn max(self, other: egui::Color32) -> egui::Color32 {
        if self == theme::TEXT_FAINT { other } else { self }
    }
}
