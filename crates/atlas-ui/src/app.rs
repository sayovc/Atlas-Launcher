use crate::pages;
use crate::theme;
use crate::widgets;
use atlas_core::{Atlas, StatusReport};
use atlas_shared::AtlasError;
use egui::{Color32, RichText};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Installations,
    Versions,
    Mods,
    Servers,
    Worlds,
    Downloads,
    Logs,
    Settings,
}

impl Page {
    pub const NAV: [Page; 9] = [
        Page::Home,
        Page::Installations,
        Page::Versions,
        Page::Mods,
        Page::Servers,
        Page::Worlds,
        Page::Downloads,
        Page::Logs,
        Page::Settings,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Page::Home => "Home",
            Page::Installations => "Installations",
            Page::Versions => "Versions",
            Page::Mods => "Mods",
            Page::Servers => "Servers",
            Page::Worlds => "Worlds",
            Page::Downloads => "Downloads",
            Page::Logs => "Logs",
            Page::Settings => "Settings",
        }
    }
    fn from_env() -> Option<Page> {
        let want = std::env::var("ATLAS_START_PAGE").ok()?.to_lowercase();
        Page::NAV.into_iter().find(|p| p.label().to_lowercase() == want)
    }
}

/// A notice shown at the top of the content area.
pub struct Notice {
    pub error: Option<AtlasError>,
    pub text: String,
    pub created: Instant,
}

pub struct AtlasApp {
    pub atlas: Arc<Atlas>,
    pub page: Page,
    page_changed_at: Instant,
    pub status: StatusReport,
    status_at: Instant,
    pub notices: Vec<Notice>,
    pub settings_page: pages::settings::SettingsPage,
    pub logs_page: pages::logs::LogsPage,
    pub versions_page: pages::versions::VersionsPage,
    pub installations_page: pages::installations::InstallationsPage,
    pub servers_page: pages::servers::ServersPage,
    pub mods_page: pages::mods::ModsPage,
    pub worlds_page: pages::worlds::WorldsPage,
    pub server_addr: String,
    was_running: bool,
    started: Instant,
    healthy_marked: bool,
    applied_scale: f32,
}

impl AtlasApp {
    pub fn new(cc: &eframe::CreationContext<'_>, atlas: Arc<Atlas>) -> Self {
        let s = atlas.settings();
        theme::apply(&cc.egui_ctx, theme::accent(s.appearance.accent));
        let status = atlas.status();
        let notices = atlas
            .startup_warnings()
            .iter()
            .map(|w| Notice { error: Some(w.clone()), text: String::new(), created: Instant::now() })
            .collect();
        Self {
            settings_page: pages::settings::SettingsPage::new(&atlas),
            logs_page: pages::logs::LogsPage::default(),
            versions_page: pages::versions::VersionsPage::default(),
            installations_page: Default::default(),
            servers_page: Default::default(),
            mods_page: Default::default(),
            worlds_page: Default::default(),
            server_addr: String::new(),
            was_running: false,
            started: Instant::now(),
            healthy_marked: false,
            atlas,
            page: Page::from_env().unwrap_or(Page::Home),
            page_changed_at: Instant::now(),
            status,
            status_at: Instant::now(),
            notices,
            applied_scale: 0.0,
        }
    }

    pub fn accent(&self) -> Color32 {
        theme::accent(self.atlas.settings().appearance.accent)
    }

    pub fn anim_time(&self) -> f32 {
        if self.atlas.settings().appearance.reduce_motion { 0.0 } else { 0.18 }
    }

    pub fn push_error(&mut self, e: AtlasError) {
        self.notices.push(Notice { error: Some(e), text: String::new(), created: Instant::now() });
    }
    pub fn push_info(&mut self, text: impl Into<String>) {
        self.notices.push(Notice { error: None, text: text.into(), created: Instant::now() });
    }

    pub fn navigate(&mut self, p: Page) {
        if p != self.page {
            tracing::debug!(page = p.label(), "navigate");
            self.page = p;
            self.page_changed_at = Instant::now();
        }
    }

    pub fn refresh_status(&mut self) {
        self.status = self.atlas.status();
        self.status_at = Instant::now();
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let accent = self.accent();
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
            theme::paint_logo(ui.painter(), rect.center(), 14.0, accent);
            ui.vertical(|ui| {
                ui.add_space(1.0);
                ui.label(RichText::new("ATLAS").size(19.0).strong().color(theme::TEXT).extra_letter_spacing(3.0));
                ui.label(RichText::new("LAUNCHER").size(9.5).color(theme::TEXT_FAINT).extra_letter_spacing(2.5));
            });
        });
        ui.add_space(22.0);

        let item_h = 38.0;
        ui.spacing_mut().item_spacing.y = 0.0;
        let top = ui.cursor().top();
        let selected_idx = Page::NAV.iter().position(|p| *p == self.page).unwrap_or(0);
        // Animated selection indicator slides between items.
        let target_y = top + selected_idx as f32 * (item_h + 2.0);
        let y = ui.ctx().animate_value_with_time(egui::Id::unique("nav-indicator"), target_y, self.anim_time());
        let full = ui.available_rect_before_wrap();
        let ind = egui::Rect::from_min_size(egui::pos2(full.left() + 10.0, y), egui::vec2(full.width() - 20.0, item_h));
        ui.painter().rect_filled(ind, 8.0, theme::SURFACE_HI);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(egui::pos2(ind.left(), ind.top() + 9.0), egui::vec2(3.0, item_h - 18.0)),
            2.0,
            accent,
        );

        let mut clicked = None;
        for p in Page::NAV {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(full.width(), item_h), egui::Sense::click());
            let hovered_t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), self.anim_time());
            let sel = p == self.page;
            let col = if sel { theme::TEXT } else { theme::TEXT_DIM.lerp_to_gamma(theme::TEXT, hovered_t) };
            let x = rect.left() + 26.0 + hovered_t * 2.0;
            crate::icons::paint(ui.painter(), p, egui::pos2(x + 8.0, rect.center().y), 16.0, if sel { accent } else { col });
            ui.painter().text(egui::pos2(x + 28.0, rect.center().y), egui::Align2::LEFT_CENTER, p.label(), egui::FontId::proportional(14.0), col);
            if p == Page::Downloads {
                let running = self.atlas.jobs().iter().filter(|j| j.state == atlas_core::JobState::Running).count();
                if running > 0 {
                    let c = egui::pos2(rect.right() - 30.0, rect.center().y);
                    ui.painter().circle_filled(c, 9.0, accent);
                    ui.painter().text(c, egui::Align2::CENTER_CENTER, running.to_string(), egui::FontId::proportional(11.0), egui::Color32::from_rgb(6, 18, 20));
                }
            }
            if resp.clicked() {
                clicked = Some(p);
            }
            ui.add_space(2.0);
        }
        if let Some(p) = clicked {
            self.navigate(p);
        }

        // Footer: version + data folder kind
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.label(RichText::new(format!("v{}", atlas_shared::ATLAS_VERSION)).small().color(theme::TEXT_FAINT));
                ui.label(RichText::new("·").small().color(theme::TEXT_FAINT));
                ui.label(RichText::new("pre-alpha").small().color(theme::WARN.gamma_multiply(0.8)));
            });
        });
    }

    fn notices(&mut self, ui: &mut egui::Ui) {
        // Info notices auto-expire; errors stay until dismissed.
        self.notices.retain(|n| n.error.is_some() || n.created.elapsed() < Duration::from_secs(6));
        let mut dismiss = None;
        for (i, n) in self.notices.iter().enumerate() {
            let closed = match &n.error {
                Some(e) => widgets::error_banner(ui, e),
                None => widgets::info_banner(ui, &n.text),
            };
            if closed {
                dismiss = Some(i);
            }
            ui.add_space(6.0);
        }
        if let Some(i) = dismiss {
            self.notices.remove(i);
        }
    }
}

impl eframe::App for AtlasApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        let settings = self.atlas.settings();
        if (settings.appearance.ui_scale - self.applied_scale).abs() > f32::EPSILON {
            ctx.set_zoom_factor(settings.appearance.ui_scale);
            self.applied_scale = settings.appearance.ui_scale;
        }
        // poll faster while something is happening so the button keeps up
        let active = !matches!(self.status.game_state, atlas_core::GameState::Ready | atlas_core::GameState::NoInstallation | atlas_core::GameState::NotInstalled);
        if self.status_at.elapsed() > Duration::from_millis(if active { 300 } else { 1500 }) {
            self.refresh_status();
        }
        // an update that survives 10 seconds of normal running is a keeper
        if !self.healthy_marked && self.started.elapsed() > Duration::from_secs(10) {
            self.healthy_marked = true;
            self.atlas.mark_update_healthy();
        }
        // "after launching" setting
        let running = matches!(self.status.game_state, atlas_core::GameState::Running { .. });
        if running && !self.was_running {
            match settings.general.after_launch {
                atlas_config::settings::AfterLaunch::Minimize => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                atlas_config::settings::AfterLaunch::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                atlas_config::settings::AfterLaunch::KeepOpen => {}
            }
        }
        self.was_running = running;
        self.settings_page.poll(&self.atlas, &mut self.notices);

        egui::Panel::left("nav")
            .exact_size(212.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::SIDEBAR).stroke(egui::Stroke::new(1.0, theme::BORDER)))
            .show(ui, |ui| self.sidebar(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(28, 22)))
            .show(ui, |ui| {
                // Subtle fade-in on page change.
                let t = if self.anim_time() == 0.0 {
                    1.0
                } else {
                    (self.page_changed_at.elapsed().as_secs_f32() / 0.22).min(1.0)
                };
                if t < 1.0 {
                    ui.ctx().request_repaint();
                }
                ui.set_opacity(0.35 + 0.65 * t);
                self.notices(ui);
                match self.page {
                    Page::Home => pages::home::show(self, ui),
                    Page::Settings => pages::settings::show(self, ui),
                    Page::Logs => pages::logs::show(self, ui),
                    Page::Versions => pages::versions::show(self, ui),
                    Page::Installations => pages::installations::show(self, ui),
                    Page::Downloads => pages::downloads::show(self, ui),
                    Page::Servers => pages::servers::show(self, ui),
                    Page::Mods => pages::mods::show(self, ui),
                    Page::Worlds => pages::worlds::show(self, ui),
                }
            });
        // Keep the live log view and status fresh without busy-looping.
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}
