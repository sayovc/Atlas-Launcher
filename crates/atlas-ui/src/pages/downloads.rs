use crate::app::{AtlasApp, Page};
use crate::{theme, widgets};
use atlas_core::{JobKind, JobState};
use egui::RichText;

pub fn human_bytes(b: f64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let (mut v, mut i) = (b, 0);
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b:.0} B") } else { format!("{v:.1} {}", U[i]) }
}

fn eta(remaining: f64, speed: f64) -> String {
    if speed <= 1.0 || remaining <= 0.0 {
        return "—".into();
    }
    let s = (remaining / speed) as u64;
    if s >= 3600 { format!("{}h {}m", s / 3600, (s % 3600) / 60) } else if s >= 60 { format!("{}m {}s", s / 60, s % 60) } else { format!("{s}s") }
}

pub fn show(app: &mut AtlasApp, ui: &mut egui::Ui) {
    let atlas = app.atlas.clone();
    let accent = app.accent();
    let jobs = atlas.jobs();
    widgets::page_header(ui, "Downloads", "Installs and repairs. Interrupted downloads resume where they stopped.");

    ui.horizontal(|ui| {
        let running = jobs.iter().filter(|j| j.state == JobState::Running).count();
        ui.label(RichText::new(format!("{running} active · {} total", jobs.len())).color(theme::TEXT_FAINT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(jobs.iter().any(|j| j.state != JobState::Running), egui::Button::new("Clear finished")).clicked() {
                atlas.clear_finished_jobs();
            }
        });
    });
    ui.add_space(6.0);

    if jobs.is_empty() {
        widgets::card(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(18.0);
                crate::icons::show(ui, Page::Downloads, 40.0, theme::TEXT_FAINT);
                ui.add_space(8.0);
                ui.label(RichText::new("Nothing downloading").size(17.0).strong());
                ui.label(RichText::new("Install a version to see its progress here.").color(theme::TEXT_DIM));
                ui.add_space(8.0);
                if ui.link("Browse versions").clicked() {
                    app.navigate(Page::Versions);
                }
                ui.add_space(14.0);
            });
        });
        return;
    }

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for j in &jobs {
            let p = &j.progress;
            widgets::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(match j.kind { JobKind::Install => "Install", JobKind::Repair => "Repair" }).color(theme::TEXT_FAINT));
                    ui.label(RichText::new(&j.version).size(17.0).strong());
                    match &j.state {
                        JobState::Running => widgets::pill(ui, "Running", accent),
                        JobState::Done => widgets::pill(ui, "Done", theme::OK),
                        JobState::Cancelled => widgets::pill(ui, "Cancelled", theme::TEXT_DIM),
                        JobState::Failed { error } => widgets::pill(ui, error.code.as_str(), theme::ERR),
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if j.state == JobState::Running && ui.button("Cancel").clicked() {
                            atlas.cancel_job(j.id);
                        }
                    });
                });
                ui.add_space(4.0);
                let bar = egui::ProgressBar::new(p.fraction()).show_percentage();
                let bar = if j.state == JobState::Running { bar.animate(true).fill(accent) } else { bar };
                ui.add(bar);
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    let small = |t: String| RichText::new(t).small().color(theme::TEXT_DIM);
                    ui.label(small(format!("{} / {} files", p.done_files, p.total_files)));
                    ui.label(small(format!("{} / {}", human_bytes(p.done_bytes as f64), human_bytes(p.total_bytes as f64))));
                    if j.state == JobState::Running {
                        ui.label(small(format!("{}/s", human_bytes(j.speed_bps))));
                        ui.label(small(format!("ETA {}", eta(p.total_bytes.saturating_sub(p.done_bytes) as f64, j.speed_bps))));
                    } else {
                        ui.label(small(format!("{:.1}s", j.elapsed_secs)));
                    }
                    if p.downloaded_files > 0 {
                        ui.label(small(format!("{} downloaded", p.downloaded_files)));
                    }
                    if p.already_valid_files > 0 {
                        ui.label(small(format!("{} already present", p.already_valid_files)));
                    }
                    if p.reused_files > 0 {
                        ui.label(small(format!("{} reused from other launchers", p.reused_files)));
                    }
                });
                if let JobState::Failed { error } = &j.state {
                    ui.add_space(6.0);
                    ui.label(RichText::new(&error.message).color(theme::ERR));
                    if !error.suggestion.is_empty() {
                        ui.label(RichText::new(&error.suggestion).small().color(theme::TEXT_DIM));
                    }
                }
            });
            ui.add_space(8.0);
        }
    });
}
