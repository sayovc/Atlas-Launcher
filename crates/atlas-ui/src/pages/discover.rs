use crate::app::{AtlasApp, Page};
use crate::{theme, widgets};
use atlas_core::{LoaderKind, ProjectType, SearchResult};
use egui::RichText;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver};

type IconRx = Receiver<(String, Option<egui::ColorImage>)>;

pub struct DiscoverPage {
    kind: ProjectType,
    query: String,
    last_query: Option<(String, ProjectType, String)>,
    results: Option<SearchResult>,
    searching: Option<Receiver<atlas_shared::AtlasResult<SearchResult>>>,
    icons: HashMap<String, egui::TextureHandle>,
    icon_wanted: HashSet<String>,
    icon_tx: std::sync::mpsc::Sender<(String, Option<egui::ColorImage>)>,
    icon_rx: IconRx,
    bundle_pick: HashSet<&'static str>,
}

impl Default for DiscoverPage {
    fn default() -> Self {
        let (icon_tx, icon_rx) = channel();
        Self {
            kind: ProjectType::Mod,
            query: String::new(),
            last_query: None,
            results: None,
            searching: None,
            icons: HashMap::new(),
            icon_wanted: HashSet::new(),
            icon_tx,
            icon_rx,
            bundle_pick: atlas_content_defaults(),
        }
    }
}

fn atlas_content_defaults() -> HashSet<&'static str> {
    ["performance", "qol"].into_iter().collect()
}

fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(bytes).ok()?.thumbnail(96, 96).to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
}

fn short(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

pub fn show(app: &mut AtlasApp, ui: &mut egui::Ui) {
    let atlas = app.atlas.clone();
    let accent = app.accent();
    let ctx = ui.ctx().clone();
    let Some(inst) = atlas.selected_installation() else {
        widgets::page_header(ui, "Discover", "Mods, resource packs and shaders from Modrinth.");
        widgets::card(ui, |ui| {
            ui.label("Create an installation first, or use the one-click setup on Home.");
            if ui.link("Go to Home").clicked() {
                app.navigate(Page::Home);
            }
        });
        return;
    };
    let mut errors = vec![];
    let mut infos = vec![];
    let installed: HashSet<String> = atlas.installed_content(Some(&inst.id)).unwrap_or_default().into_iter().map(|i| i.project_id).collect();
    let busy_jobs: HashSet<String> = atlas.jobs().into_iter().filter(|j| j.state == atlas_core::JobState::Running).map(|j| j.version).collect();
    let dp = &mut app.discover_page;

    // icons that finished loading
    while let Ok((url, img)) = dp.icon_rx.try_recv() {
        if let Some(img) = img {
            let t = ctx.load_texture(url.clone(), img, egui::TextureOptions::LINEAR);
            dp.icons.insert(url, t);
        }
    }
    if let Some(rx) = &dp.searching
        && let Ok(r) = rx.try_recv()
    {
        dp.searching = None;
        match r {
            Ok(r) => dp.results = Some(r),
            Err(e) => errors.push(e),
        }
    }

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("Discover").heading().strong());
            ui.label(RichText::new(format!("For {} · Minecraft {} · {}", inst.name, inst.version, inst.loader.label())).color(theme::TEXT_DIM));
        });
    });
    ui.add_space(10.0);

    // bundles strip
    let modded = matches!(inst.loader, LoaderKind::Fabric | LoaderKind::Quilt | LoaderKind::Vanilla);
    if modded {
        widgets::section_title(ui, "Bundles");
        ui.horizontal_wrapped(|ui| {
            for b in atlas.bundles() {
                let on = dp.bundle_pick.contains(b.id);
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(214.0, 84.0), egui::Sense::click());
                let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                let p = ui.painter();
                p.rect(rect, 10.0, if on { accent.gamma_multiply(0.14) } else { theme::SURFACE }, egui::Stroke::new(1.0, if on { accent } else { theme::BORDER.lerp_to_gamma(accent, hover * 0.5) }), egui::StrokeKind::Inside);
                p.text(rect.left_top() + egui::vec2(14.0, 12.0), egui::Align2::LEFT_TOP, b.name, egui::FontId::proportional(15.0), theme::TEXT);
                let blurb = p.layout(b.blurb.to_string(), egui::FontId::proportional(11.0), theme::TEXT_DIM, rect.width() - 28.0);
                p.galley(rect.left_top() + egui::vec2(14.0, 32.0), blurb, theme::TEXT_DIM);
                p.text(rect.left_bottom() + egui::vec2(14.0, -10.0), egui::Align2::LEFT_BOTTOM, format!("{} mods", b.mods.len()), egui::FontId::proportional(11.0), theme::TEXT_FAINT);
                if on {
                    p.circle_filled(rect.right_top() + egui::vec2(-16.0, 16.0), 6.0, accent);
                }
                if resp.on_hover_text(b.mods.join(", ")).clicked() {
                    if on {
                        dp.bundle_pick.remove(b.id);
                    } else {
                        dp.bundle_pick.insert(b.id);
                    }
                }
            }
        });
        ui.horizontal(|ui| {
            let ids: Vec<String> = atlas.bundles().iter().filter(|b| dp.bundle_pick.contains(b.id)).map(|b| b.id.to_string()).collect();
            let label = if inst.loader == LoaderKind::Vanilla { "Add bundles (switches to Fabric)" } else { "Add bundles" };
            if widgets::primary_button(ui, label, accent, !ids.is_empty(), egui::vec2(260.0, 34.0)).clicked() {
                match atlas.install_bundles(Some(&inst.id), &ids) {
                    Ok(_) => infos.push(format!("Adding {} bundle(s) to {}. Progress is on the Downloads page.", ids.len(), inst.name)),
                    Err(e) => errors.push(e),
                }
            }
            ui.label(RichText::new("Missing builds for this version are skipped, never swapped for something else.").small().color(theme::TEXT_FAINT));
        });
        ui.add_space(12.0);
    }

    // search bar
    ui.horizontal(|ui| {
        for (k, l) in [(ProjectType::Mod, "Mods"), (ProjectType::Resourcepack, "Resource packs"), (ProjectType::Shader, "Shaders")] {
            let sel = dp.kind == k;
            let b = egui::Button::new(RichText::new(l).color(if sel { theme::TEXT } else { theme::TEXT_DIM }))
                .fill(if sel { theme::SURFACE_HI } else { egui::Color32::TRANSPARENT })
                .stroke(if sel { egui::Stroke::new(1.0, accent.gamma_multiply(0.6)) } else { egui::Stroke::NONE });
            if ui.add(b).clicked() {
                dp.kind = k;
            }
        }
        ui.add_space(8.0);
        let r = ui.add(egui::TextEdit::singleline(&mut dp.query).hint_text("Search Modrinth…").desired_width(280.0));
        if dp.searching.is_some() {
            ui.spinner();
        }
        let _ = r;
    });
    // search when the query, tab or installation changes (lightly debounced by the idle repaint)
    let key = (dp.query.trim().to_string(), dp.kind, inst.id.clone());
    if dp.last_query.as_ref() != Some(&key) && dp.searching.is_none() {
        dp.last_query = Some(key.clone());
        let (tx, rx) = channel();
        let (a, c, id, q, k) = (atlas.clone(), ctx.clone(), inst.id.clone(), key.0.clone(), key.1);
        std::thread::spawn(move || {
            let _ = tx.send(a.search_content(Some(&id), &q, k, 0));
            c.request_repaint();
        });
        dp.searching = Some(rx);
    }
    ui.add_space(8.0);

    if dp.kind == ProjectType::Mod && inst.loader == LoaderKind::Vanilla {
        egui::Frame::new().fill(theme::WARN.gamma_multiply(0.08)).stroke(egui::Stroke::new(1.0, theme::WARN.gamma_multiply(0.4))).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("This installation is vanilla, so mods won't load.").color(theme::TEXT_DIM));
                if ui.button("Switch to Fabric").clicked() {
                    match atlas.enable_mods(Some(&inst.id)) {
                        Ok(_) => infos.push("Switched to Fabric. It's installed the next time you play.".into()),
                        Err(e) => errors.push(e),
                    }
                }
            });
        });
        ui.add_space(6.0);
    }

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let Some(res) = &dp.results else {
            ui.label(RichText::new("Searching…").color(theme::TEXT_FAINT));
            return;
        };
        if res.hits.is_empty() {
            ui.label(RichText::new("Nothing found for this version.").color(theme::TEXT_FAINT));
        }
        for h in &res.hits {
            if let Some(u) = &h.icon_url
                && !dp.icons.contains_key(u)
                && dp.icon_wanted.insert(u.clone())
            {
                let (a, tx, u2, c) = (atlas.clone(), dp.icon_tx.clone(), u.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let img = a.fetch_icon(&u2).ok().and_then(|b| decode(&b));
                    let _ = tx.send((u2, img));
                    c.request_repaint();
                });
            }
            widgets::card(ui, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
                    match h.icon_url.as_ref().and_then(|u| dp.icons.get(u)) {
                        Some(t) => {
                            ui.painter().image(t.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                        }
                        None => {
                            ui.painter().rect_filled(rect, 8.0, theme::SURFACE_HI);
                        }
                    }
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&h.title).size(15.0).strong());
                            ui.label(RichText::new(format!("by {}", h.author)).small().color(theme::TEXT_FAINT));
                        });
                        ui.add(egui::Label::new(RichText::new(&h.description).small().color(theme::TEXT_DIM)).truncate());
                        ui.label(RichText::new(format!("{} downloads", short(h.downloads))).small().color(theme::TEXT_FAINT));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if installed.contains(&h.project_id) {
                            widgets::pill(ui, "Installed", theme::OK);
                        } else if busy_jobs.contains(&h.project_id) || busy_jobs.contains(&h.slug) {
                            ui.spinner();
                        } else if ui.add(egui::Button::new(RichText::new("Install").strong().color(accent)).stroke(egui::Stroke::new(1.0, accent.gamma_multiply(0.6)))).clicked() {
                            match atlas.install_content(Some(&inst.id), &h.project_id, dp.kind) {
                                Ok(_) => infos.push(format!("Installing {}…", h.title)),
                                Err(e) => errors.push(e),
                            }
                        }
                    });
                });
            });
            ui.add_space(6.0);
        }
    });

    for e in errors {
        app.push_error(e);
    }
    for i in infos {
        app.push_info(i);
    }
}
