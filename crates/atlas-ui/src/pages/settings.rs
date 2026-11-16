use crate::app::{AtlasApp, Notice};
use crate::{theme, widgets};
use atlas_config::settings::{Accent, AfterLaunch, LogLevel, Settings, UpdateChannel};
use atlas_core::{Atlas, Confirmation, MoveReport};
use atlas_shared::AtlasResult;
use egui::RichText;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Appearance,
    Downloads,
    Game,
    Memory,
    Installations,
    Accounts,
    Privacy,
    Advanced,
}

impl Tab {
    const ALL: [Tab; 9] = [Tab::General, Tab::Appearance, Tab::Downloads, Tab::Game, Tab::Memory, Tab::Installations, Tab::Accounts, Tab::Privacy, Tab::Advanced];
    fn label(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Appearance => "Appearance",
            Tab::Downloads => "Downloads",
            Tab::Game => "Game",
            Tab::Memory => "Memory",
            Tab::Installations => "Installations",
            Tab::Accounts => "Accounts",
            Tab::Privacy => "Privacy",
            Tab::Advanced => "Advanced",
        }
    }
}

enum Confirm {
    Reset,
    Move(PathBuf),
}

pub struct SettingsPage {
    tab: Tab,
    draft: Settings,
    jvm_args_text: String,
    java_path_text: String,
    mc_dir_text: String,
    new_data_dir: String,
    confirm: Option<Confirm>,
    moving: Option<mpsc::Receiver<AtlasResult<MoveReport>>>,
}

impl SettingsPage {
    pub fn new(atlas: &Atlas) -> Self {
        let mut p = Self {
            tab: Tab::General,
            draft: Settings::default(),
            jvm_args_text: String::new(),
            java_path_text: String::new(),
            mc_dir_text: String::new(),
            new_data_dir: String::new(),
            confirm: None,
            moving: None,
        };
        p.load(atlas.settings());
        p
    }

    fn load(&mut self, s: Settings) {
        self.jvm_args_text = s.game.extra_jvm_args.join(" ");
        self.java_path_text = s.game.java_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
        self.mc_dir_text = s.game.minecraft_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
        self.draft = s;
    }

    fn assembled(&self) -> Settings {
        let mut s = self.draft.clone();
        s.game.extra_jvm_args = self.jvm_args_text.split_whitespace().map(str::to_owned).collect();
        let opt = |t: &str| (!t.trim().is_empty()).then(|| PathBuf::from(t.trim()));
        s.game.java_path = opt(&self.java_path_text);
        s.game.minecraft_dir = opt(&self.mc_dir_text);
        s
    }

    /// Collect the result of a background data-folder move.
    pub fn poll(&mut self, atlas: &Arc<Atlas>, notices: &mut Vec<Notice>) {
        let Some(rx) = &self.moving else { return };
        if let Ok(result) = rx.try_recv() {
            self.moving = None;
            match result {
                Ok(r) => {
                    let mut text = format!("Moved {} files to {}. Restart Atlas to finish switching log files.", r.files, r.new_root.display());
                    if r.symlinks_skipped > 0 {
                        text.push_str(&format!(" {} symbolic link(s) were left in place.", r.symlinks_skipped));
                    }
                    if !r.leftovers.is_empty() {
                        text.push_str(&format!(" {} original item(s) could not be removed and can be deleted manually.", r.leftovers.len()));
                    }
                    notices.push(Notice { error: None, text, created: Instant::now() });
                    self.new_data_dir.clear();
                    self.load(atlas.settings());
                }
                Err(e) => notices.push(Notice { error: Some(e), text: String::new(), created: Instant::now() }),
            }
        }
    }
}

fn row(ui: &mut egui::Ui, label: &str, help: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(260.0);
            ui.label(label);
            if !help.is_empty() {
                ui.label(RichText::new(help).small().color(theme::TEXT_FAINT));
            }
        });
        add(ui);
    });
    ui.add_space(10.0);
}

fn not_built(ui: &mut egui::Ui, what: &str) {
    ui.horizontal(|ui| {
        widgets::pill(ui, "Not built yet", theme::WARN);
        ui.label(RichText::new(what).color(theme::TEXT_DIM));
    });
    ui.add_space(8.0);
}

pub fn show(app: &mut AtlasApp, ui: &mut egui::Ui) {
    widgets::page_header(ui, "Settings", "Saved to settings.toml in your Atlas data folder.");
    let accent = app.accent();
    let atlas = app.atlas.clone();
    let paths = atlas.paths();
    let saved = atlas.settings();
    let sp = &mut app.settings_page;

    // Tab strip
    ui.horizontal_wrapped(|ui| {
        for t in Tab::ALL {
            let sel = sp.tab == t;
            let text = RichText::new(t.label()).color(if sel { theme::TEXT } else { theme::TEXT_DIM });
            let b = egui::Button::new(text)
                .fill(if sel { theme::SURFACE_HI } else { egui::Color32::TRANSPARENT })
                .stroke(if sel { egui::Stroke::new(1.0, accent.gamma_multiply(0.6)) } else { egui::Stroke::NONE });
            if ui.add(b).clicked() {
                sp.tab = t;
            }
        }
    });
    ui.add_space(10.0);

    let dirty = sp.assembled() != saved;
    let mut action: Option<&'static str> = None;

    egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(ui.available_height() - 56.0).show(ui, |ui| {
        widgets::card(ui, |ui| {
            let d = &mut sp.draft;
            match sp.tab {
                Tab::General => {
                    row(ui, "Language", "More languages will be added later.", |ui| {
                        egui::ComboBox::from_id_salt("lang").selected_text("English").show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.general.language, "en".into(), "English");
                        });
                    });
                    row(ui, "After launching the game", "What the Atlas window does once the game starts.", |ui| {
                        ui.radio_value(&mut d.general.after_launch, AfterLaunch::KeepOpen, "Keep open");
                        ui.radio_value(&mut d.general.after_launch, AfterLaunch::Minimize, "Minimize");
                        ui.radio_value(&mut d.general.after_launch, AfterLaunch::Close, "Close");
                    });
                    row(ui, "Check for Atlas updates on start", "Takes effect once the updater exists.", |ui| {
                        ui.checkbox(&mut d.general.check_updates_on_start, "");
                    });
                }
                Tab::Appearance => {
                    row(ui, "Theme", "Atlas uses a dark theme.", |ui| {
                        ui.label(RichText::new("Dark").color(theme::TEXT_DIM));
                    });
                    row(ui, "Accent colour", "", |ui| {
                        for a in [Accent::Aurora, Accent::Ember, Accent::Moss, Accent::Iris] {
                            let c = theme::accent(a);
                            let sel = d.appearance.accent == a;
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
                            ui.painter().circle_filled(rect.center(), 10.0, c);
                            if sel {
                                ui.painter().circle_stroke(rect.center(), 12.5, egui::Stroke::new(2.0, theme::TEXT));
                            }
                            if resp.on_hover_text(theme::accent_name(a)).clicked() {
                                d.appearance.accent = a;
                            }
                        }
                    });
                    row(ui, "Interface scale", "0.75× – 2.0×", |ui| {
                        ui.add(egui::Slider::new(&mut d.appearance.ui_scale, 0.75..=2.0).step_by(0.05).suffix("×"));
                    });
                    row(ui, "Reduce motion", "Disables sidebar and page animations.", |ui| {
                        ui.checkbox(&mut d.appearance.reduce_motion, "");
                    });
                }
                Tab::Downloads => {
                    row(ui, "Parallel downloads", "1 – 32", |ui| {
                        ui.add(egui::Slider::new(&mut d.downloads.max_concurrent, 1..=32));
                    });
                    row(ui, "Bandwidth limit", "Leave off for unlimited.", |ui| {
                        let mut on = d.downloads.bandwidth_limit_kib.is_some();
                        if ui.checkbox(&mut on, "Limit").changed() {
                            d.downloads.bandwidth_limit_kib = on.then_some(4096);
                        }
                        if let Some(v) = &mut d.downloads.bandwidth_limit_kib {
                            ui.add(egui::DragValue::new(v).range(64..=1_000_000).suffix(" KiB/s"));
                        }
                    });
                    row(ui, "Keep download cache", "Verified files are reused instead of downloaded again.", |ui| {
                        ui.checkbox(&mut d.downloads.keep_cache, "");
                    });
                    ui.label(RichText::new("Checksum verification is always on and cannot be disabled.").small().color(theme::TEXT_FAINT));
                }
                Tab::Game => {
                    row(ui, "Window size", "Default for new installations.", |ui| {
                        ui.add(egui::DragValue::new(&mut d.game.width).range(320..=7680).suffix(" px"));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut d.game.height).range(240..=4320).suffix(" px"));
                        ui.checkbox(&mut d.game.fullscreen, "Fullscreen");
                    });
                    row(ui, "Java executable", "Empty = detect automatically.", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut sp.java_path_text).hint_text("auto").desired_width(320.0));
                        if ui.button("Browse…").clicked()
                            && let Some(f) = rfd::FileDialog::new().pick_file()
                        {
                            sp.java_path_text = f.display().to_string();
                        }
                    });
                    row(ui, "Extra JVM arguments", "Space-separated.", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut sp.jvm_args_text).hint_text("-XX:+UseG1GC").desired_width(320.0));
                    });
                    row(ui, "Minecraft folder", "Used if automatic detection fails. Atlas only reads from it.", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut sp.mc_dir_text).hint_text("auto-detect").desired_width(320.0));
                        if ui.button("Browse…").clicked()
                            && let Some(f) = rfd::FileDialog::new().pick_folder()
                        {
                            sp.mc_dir_text = f.display().to_string();
                        }
                    });
                }
                Tab::Memory => {
                    row(ui, "Minimum memory", "Initial Java heap (-Xms).", |ui| {
                        ui.add(egui::Slider::new(&mut d.memory.min_mb, 512..=32768).step_by(256.0).suffix(" MB"));
                    });
                    row(ui, "Maximum memory", "Maximum Java heap (-Xmx).", |ui| {
                        ui.add(egui::Slider::new(&mut d.memory.max_mb, 512..=65536).step_by(256.0).suffix(" MB"));
                    });
                    if d.memory.min_mb > d.memory.max_mb {
                        ui.label(RichText::new("Minimum is larger than maximum; saving will be refused.").color(theme::WARN));
                    }
                }
                Tab::Installations => {
                    not_built(ui, "Installation management arrives in Phase 3.");
                    row(ui, "Show snapshot versions", "Include development snapshots in version lists.", |ui| {
                        ui.checkbox(&mut d.installations.show_snapshots, "");
                    });
                }
                Tab::Accounts => {
                    not_built(ui, "Microsoft sign-in arrives in Phase 3.");
                    ui.label(RichText::new("Sign-in will happen in your web browser. Atlas never asks for your password, and tokens are kept in your system's secure credential store, never in plain files.").color(theme::TEXT_DIM));
                }
                Tab::Privacy => {
                    row(ui, "Hide username in log files", "Replaces your in-game name in saved logs.", |ui| {
                        ui.checkbox(&mut d.privacy.hide_username_in_logs, "");
                    });
                    ui.label(RichText::new("Atlas sends no telemetry or analytics. Tokens and secrets are always redacted from logs.").color(theme::TEXT_DIM));
                }
                Tab::Advanced => {
                    row(ui, "Log level", "Applies after restart.", |ui| {
                        egui::ComboBox::from_id_salt("loglevel").selected_text(format!("{:?}", d.advanced.log_level)).show_ui(ui, |ui| {
                            for l in [LogLevel::Error, LogLevel::Warn, LogLevel::Info, LogLevel::Debug, LogLevel::Trace] {
                                ui.selectable_value(&mut d.advanced.log_level, l, format!("{l:?}"));
                            }
                        });
                    });
                    row(ui, "Log files to keep", "Daily files; oldest are removed.", |ui| {
                        ui.add(egui::DragValue::new(&mut d.advanced.keep_log_files).range(1..=365));
                    });
                    row(ui, "Update channel", "", |ui| {
                        ui.radio_value(&mut d.advanced.update_channel, UpdateChannel::Stable, "Stable");
                        ui.radio_value(&mut d.advanced.update_channel, UpdateChannel::Beta, "Beta");
                    });
                    ui.separator();
                    widgets::section_title(ui, "AI & agent integration");
                    row(ui, "Enable AI features", "Global switch. When off, nothing AI-related can run.", |ui| {
                        ui.checkbox(&mut d.advanced.ai.enabled, "");
                    });
                    row(ui, "Local MCP server", "Opt-in, local-only, authenticated. Not built yet (Phase 5).", |ui| {
                        ui.add_enabled(d.advanced.ai.enabled, egui::Checkbox::new(&mut d.advanced.ai.mcp_enabled, ""));
                    });
                    ui.separator();
                    widgets::section_title(ui, "Data folder");
                    ui.label(RichText::new(paths.data_root.display().to_string()).monospace());
                    ui.label(RichText::new(if paths.data_root_is_custom { "Custom location" } else { "Default location" }).small().color(theme::TEXT_FAINT));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut sp.new_data_dir).hint_text("New empty folder…").desired_width(320.0));
                        if ui.button("Browse…").clicked()
                            && let Some(f) = rfd::FileDialog::new().pick_folder()
                        {
                            sp.new_data_dir = f.display().to_string();
                        }
                        let can = !sp.new_data_dir.trim().is_empty() && sp.moving.is_none();
                        if ui.add_enabled(can, egui::Button::new("Move data…")).clicked() {
                            sp.confirm = Some(Confirm::Move(PathBuf::from(sp.new_data_dir.trim())));
                        }
                        if sp.moving.is_some() {
                            ui.spinner();
                            ui.label("Moving…");
                        }
                    });
                    ui.label(RichText::new("Moves installations, downloads, settings, logs, account metadata and managed game data. The copy is verified before originals are removed. The target must be empty.").small().color(theme::TEXT_FAINT));
                    ui.separator();
                    if ui.button(RichText::new("Reset all settings…").color(theme::ERR)).clicked() {
                        sp.confirm = Some(Confirm::Reset);
                    }
                }
            }
        });
    });

    // Save bar
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if dirty {
            ui.label(RichText::new("• Unsaved changes").color(theme::WARN));
        } else {
            ui.label(RichText::new("All changes saved").color(theme::TEXT_FAINT));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::primary_button(ui, "Save", accent, dirty, egui::vec2(96.0, 34.0)).clicked() {
                action = Some("save");
            }
            if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                action = Some("revert");
            }
        });
    });

    // Confirmation dialogs for destructive actions.
    let mut confirmed: Option<Confirm> = None;
    if let Some(c) = &sp.confirm {
        let (title, body) = match c {
            Confirm::Reset => ("Reset all settings?".to_string(), "Every setting returns to its default. Installations and worlds are not affected.".to_string()),
            Confirm::Move(p) => ("Move Atlas data?".to_string(), format!("All Atlas-managed data will be copied to\n{}\nverified, and then removed from the current folder.", p.display())),
        };
        let mut close = false;
        egui::Modal::new(egui::Id::unique("settings-confirm")).show(ui.ctx(), |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(title).size(18.0).strong());
            ui.add_space(6.0);
            ui.label(RichText::new(body).color(theme::TEXT_DIM));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Confirm").color(theme::ERR)).clicked() {
                    confirmed = Some(match c {
                        Confirm::Reset => Confirm::Reset,
                        Confirm::Move(p) => Confirm::Move(p.clone()),
                    });
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            sp.confirm = None;
        }
    }

    match action {
        Some("save") => match atlas.save_settings(sp.assembled()) {
            Ok(s) => {
                theme::apply(ui.ctx(), theme::accent(s.appearance.accent));
                sp.load(s);
                app.push_info("Settings saved.");
            }
            Err(e) => app.push_error(e),
        },
        Some("revert") => sp.load(saved),
        _ => {}
    }

    let sp = &mut app.settings_page;
    match confirmed {
        Some(Confirm::Reset) => match atlas.reset_settings(Confirmation::Confirmed) {
            Ok(s) => {
                theme::apply(ui.ctx(), theme::accent(s.appearance.accent));
                sp.load(s);
                app.push_info("Settings reset to defaults.");
            }
            Err(e) => app.push_error(e),
        },
        Some(Confirm::Move(target)) => {
            let (tx, rx) = mpsc::channel();
            let a = atlas.clone();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let _ = tx.send(a.move_data_root(&target, Confirmation::Confirmed));
                ctx.request_repaint();
            });
            sp.moving = Some(rx);
        }
        None => {}
    }
}
